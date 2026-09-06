//! The build's bridge to the project worker.
//!
//! `ruvyxa.config.ts` carries code a build has to run in a JavaScript runtime:
//! the Markdown pipeline with its unified plugins, the React compiler, and the
//! content engine that derives `/content.json` and its siblings from the
//! content tree. `packages/ruvyxa/runtime/project-worker.mjs` is a long-lived
//! process that answers all three; the bundler reaches it through the
//! synchronous [`BuildHooks`](ruvyxa_bundler::hooks::BuildHooks) trait, and the
//! build asks it to write the content artifacts once the output is committed.
//! This module owns the worker's lifetime, frames newline-delimited JSON over
//! its stdio, and turns a worker fault into a build error rather than a hang.
//!
//! One worker is shared by every route in a build session, so startup cost is
//! paid once instead of once per route.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use ruvyxa_dev_server::{JavaScriptRuntime, find_runtime_script};

/// Which parts of the worker a build session needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WorkerOptions {
    /// `config.markdown` is set: `.md`/`.mdx` compile through the JavaScript
    /// MDX pipeline rather than the native fallback.
    pub(crate) markdown: bool,
    /// `config.reactCompiler` is on.
    pub(crate) react_compiler: bool,
    /// `config.content` turns the content engine on.
    pub(crate) content_engine: bool,
}

impl WorkerOptions {
    fn needs_worker(self) -> bool {
        self.markdown || self.react_compiler || self.content_engine
    }
}

#[derive(Clone)]
pub(crate) struct BuildWorkerBridge {
    pub(crate) workers: Arc<Vec<Mutex<BuildWorker>>>,
    pub(crate) next_worker: Arc<AtomicUsize>,
    pub(crate) markdown: bool,
}

/// Longest one worker call may run before its worker is stopped.
///
/// Matches `DEFAULT_WORKER_CALL_TIMEOUT_MS` on the native host, so the same
/// budget applies on both sides of the framework. Without it a call that never
/// resolves hung the whole build with no diagnostic — the failure this module's
/// own documentation promises not to have.
const WORKER_CALL_TIMEOUT: Duration =
    Duration::from_millis(ruvyxa_middleware::config::DEFAULT_WORKER_CALL_TIMEOUT_MS);

pub(crate) struct BuildWorker {
    pub(crate) child: Child,
    pub(crate) stdin: ChildStdin,
    /// Response lines pushed by this worker's reader thread. See
    /// [`BuildWorker::spawn`] for why the read is not inline.
    responses: mpsc::Receiver<std::io::Result<String>>,
    /// Set once a call timed out; the worker is dead and must not be reused.
    poisoned: bool,
}

/// Owns the single persistent worker used by one production build.
///
/// The bundler hooks and the content-artifact write intentionally share this
/// session so config compilation and process startup happen only once.
pub(crate) struct BuildWorkerSession {
    pub(crate) bridge: Option<BuildWorkerBridge>,
    content_engine: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerOutput {
    pub(crate) ok: bool,
    pub(crate) result: Option<serde_json::Value>,
    pub(crate) code: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) stack: Option<String>,
}

impl ruvyxa_bundler::hooks::BuildHooks for BuildWorkerBridge {
    fn host_name(&self) -> &str {
        "ruvyxa-project-worker"
    }

    /// The React compiler, when the project turned it on. The worker answers
    /// `null` for a module it leaves alone.
    fn transform(
        &self,
        code: &str,
        id: &Path,
        _ctx: &ruvyxa_bundler::hooks::BuildHookContext,
    ) -> ruvyxa_bundler::Result<Option<ruvyxa_bundler::hooks::TransformOutput>> {
        let payload = serde_json::json!({
            "code": code,
            "id": id.display().to_string(),
        });
        let Some(value) = self.call_runner("build.transform", payload)? else {
            return Ok(None);
        };
        let Some(code) = value.get("code").and_then(|value| value.as_str()) else {
            return Ok(None);
        };
        let map = value
            .get("map")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        Ok(Some(ruvyxa_bundler::hooks::TransformOutput {
            code: code.to_string(),
            map,
        }))
    }

    fn compile_content(
        &self,
        code: &str,
        id: &Path,
        _ctx: &ruvyxa_bundler::hooks::BuildHookContext,
    ) -> ruvyxa_bundler::Result<Option<ruvyxa_bundler::hooks::TransformOutput>> {
        if !self.markdown {
            return Ok(None);
        }
        let payload = serde_json::json!({
            "code": code,
            "id": id.display().to_string(),
        });
        let Some(value) = self.call_runner("content.compile", payload)? else {
            return Ok(None);
        };
        let Some(code) = value.get("code").and_then(serde_json::Value::as_str) else {
            return Ok(None);
        };
        Ok(Some(ruvyxa_bundler::hooks::TransformOutput::code(code)))
    }
}

impl BuildWorkerBridge {
    pub(crate) fn call_worker(
        &self,
        payload: &serde_json::Value,
    ) -> ruvyxa_bundler::Result<WorkerOutput> {
        let worker_index = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let mut worker = self.workers[worker_index].lock().map_err(|_| {
            ruvyxa_bundler::BundleError::Compiler("project worker lock was poisoned".into())
        })?;
        worker.call(payload)
    }

    pub(crate) fn call_runner(
        &self,
        hook: &str,
        mut payload: serde_json::Value,
    ) -> ruvyxa_bundler::Result<Option<serde_json::Value>> {
        payload["hook"] = serde_json::Value::String(hook.to_string());
        let result = self.call_worker(&payload)?;

        if result.ok {
            return Ok(result.result);
        }

        Err(ruvyxa_bundler::BundleError::Compiler(
            ruvyxa_diagnostics::label_with_code(
                &result.code.unwrap_or_else(|| "RUV1700".to_string()),
                &result
                    .message
                    .or(result.stack)
                    .unwrap_or_else(|| "project worker call failed".to_string()),
            ),
        ))
    }
}

impl BuildWorkerSession {
    pub(crate) fn new(
        root: &Path,
        runtime: JavaScriptRuntime,
        options: WorkerOptions,
    ) -> anyhow::Result<Self> {
        if !options.needs_worker() {
            return Ok(Self {
                bridge: None,
                content_engine: false,
            });
        }

        let runner = find_runtime_script(root, "project-worker.mjs")
            .ok_or_else(|| anyhow::anyhow!("RUV1701 project-worker.mjs not found"))?;
        let project_root = ruvyxa_diagnostics::normalized_canonical_path(root);
        let worker = BuildWorker::spawn(&runner, &project_root, runtime)
            .map_err(|error| anyhow::anyhow!("failed to start the project worker: {error}"))?;
        Ok(Self {
            bridge: Some(BuildWorkerBridge {
                workers: Arc::new(vec![Mutex::new(worker)]),
                next_worker: Arc::new(AtomicUsize::new(0)),
                markdown: options.markdown,
            }),
            content_engine: options.content_engine,
        })
    }

    pub(crate) fn bridge(&self) -> Option<&BuildWorkerBridge> {
        self.bridge.as_ref()
    }

    /// Write the content engine's artifacts into `<out_dir>/assets`.
    ///
    /// Called after the build output is committed, so the files land in the
    /// directory every adapter snapshots as the site's public root.
    pub(crate) fn write_content_artifacts(&self, out_dir: &Path) -> anyhow::Result<()> {
        let (Some(bridge), true) = (&self.bridge, self.content_engine) else {
            return Ok(());
        };
        let mut payload = serde_json::json!({ "outDir": out_dir });
        payload["hook"] = serde_json::Value::String("content.write".to_string());
        let result = bridge
            .call_worker(&payload)
            .map_err(|error| anyhow::anyhow!("content engine failed: {error}"))?;
        if !result.ok {
            anyhow::bail!(
                "{}",
                ruvyxa_diagnostics::label_with_code(
                    &result.code.unwrap_or_else(|| "RUV1700".to_string()),
                    &result
                        .message
                        .or(result.stack)
                        .unwrap_or_else(|| "content engine failed".to_string()),
                )
            );
        }
        Ok(())
    }
}

impl BuildWorker {
    pub(crate) fn spawn(
        runner: &Path,
        project_root: &Path,
        runtime: JavaScriptRuntime,
    ) -> ruvyxa_bundler::Result<Self> {
        let mut child = ProcessCommand::new(runtime.executable())
            .args(runtime.script_args())
            .arg(runner)
            .arg(project_root)
            .arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Stdout is reserved for the NDJSON protocol. The worker routes
            // console output to stderr, so inherit it instead of silently
            // discarding diagnostics during production builds.
            .stderr(Stdio::inherit())
            .env("RUVYXA_RUNTIME", runtime.command())
            .spawn()
            .map_err(|err| {
                ruvyxa_bundler::BundleError::Compiler(format!(
                    "failed to start the project worker: {err}"
                ))
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ruvyxa_bundler::BundleError::Compiler(
                "failed to open the project worker's stdin".into(),
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ruvyxa_bundler::BundleError::Compiler(
                "failed to open the project worker's stdout".into(),
            )
        })?;

        // Responses are read on a dedicated thread rather than inline, so a
        // call that never answers costs a bounded wait instead of the whole
        // build. `BuildHooks` is a synchronous trait called from rayon workers,
        // so there is no runtime here to time the read out against — the thread
        // plus channel is what makes `recv_timeout` possible at all.
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    // Clean EOF: the worker closed stdout. Dropping the sender
                    // is the signal; an explicit message would race the exit.
                    Ok(0) => break,
                    Ok(_) => {
                        if responses.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = responses.send(Err(error));
                        break;
                    }
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            responses: receiver,
            poisoned: false,
        })
    }

    pub(crate) fn call(
        &mut self,
        payload: &serde_json::Value,
    ) -> ruvyxa_bundler::Result<WorkerOutput> {
        self.call_with_timeout(payload, WORKER_CALL_TIMEOUT)
    }

    /// [`Self::call`] with an explicit budget, so the timeout path can be tested
    /// without a test that waits out the production one.
    pub(crate) fn call_with_timeout(
        &mut self,
        payload: &serde_json::Value,
        timeout: Duration,
    ) -> ruvyxa_bundler::Result<WorkerOutput> {
        // A worker that timed out may still answer the previous request later.
        // Reusing it would pair that stale line with the next call's payload, so
        // it stays refused rather than silently returning another call's result.
        if self.poisoned {
            return Err(ruvyxa_bundler::BundleError::Compiler(
                "the project worker was stopped after an earlier call timed out".into(),
            ));
        }

        writeln!(self.stdin, "{payload}").map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "failed to send a project worker payload: {err}"
            ))
        })?;
        self.stdin.flush().map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "failed to flush a project worker payload: {err}"
            ))
        })?;

        let stdout = match self.responses.recv_timeout(timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                return Err(ruvyxa_bundler::BundleError::Compiler(format!(
                    "failed to read a project worker response: {error}"
                )));
            }
            // The reader thread ended, which only happens at EOF: the worker
            // exited without answering.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = self
                    .child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                return Err(ruvyxa_bundler::BundleError::Compiler(format!(
                    "the project worker exited before responding (status: {status})"
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.poisoned = true;
                // Kill now rather than at drop: the build is about to fail, and
                // a call stuck in an infinite loop would otherwise keep a core
                // busy until the CLI process itself exits.
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(ruvyxa_bundler::BundleError::Compiler(format!(
                    "RUV1701 the project worker did not respond within {} seconds. \
                     The worker was stopped. Check ruvyxa.config.ts and its imports for an \
                     unresolved promise or a blocking loop.",
                    timeout.as_secs()
                )));
            }
        };

        serde_json::from_str(stdout.trim()).map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "the project worker returned invalid output: {err}; stdout: {}",
                stdout.trim()
            ))
        })
    }
}

impl Drop for BuildWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn bundle_context_for_build(
    build_dependency_hash: &str,
    cache_dir: &Path,
    worker_session: &BuildWorkerSession,
    server_references: &[ruvyxa_dev_server::ServerReferenceSource],
) -> anyhow::Result<ruvyxa_bundler::BundleContext> {
    let artifact_graph_enabled = !matches!(
        std::env::var("RUVYXA_DISABLE_ARTIFACT_CACHE").as_deref(),
        Ok("1" | "true")
    );
    let compile_cache = ruvyxa_bundler::cache::CompileCache::at_dir_with_namespace(
        cache_dir,
        true,
        build_dependency_hash,
    );
    // Ordered before the worker: a `'use server'` module is not the file on
    // disk as far as a browser bundle is concerned, so the React compiler
    // should see the reference, not the server code it replaced.
    let mut hosts: Vec<Arc<dyn ruvyxa_bundler::hooks::BuildHooks>> = Vec::new();
    let substitutions =
        crate::server_references::ServerReferenceSources::new(server_references.iter().cloned());
    if !substitutions.is_empty() {
        hosts.push(Arc::new(substitutions));
    }
    if let Some(bridge) = worker_session.bridge() {
        hosts.push(Arc::new(bridge.clone()));
    }
    if hosts.is_empty() {
        return Ok(ruvyxa_bundler::BundleContext::for_build_with_artifacts(
            compile_cache,
            ruvyxa_bundler::resolver::ResolveGraphCache::for_build(),
            cache_dir,
            build_dependency_hash,
            artifact_graph_enabled,
        ));
    }

    Ok(ruvyxa_bundler::BundleContext::with_build_hooks_for_build(
        compile_cache,
        ruvyxa_bundler::resolver::ResolveGraphCache::for_build(),
        ruvyxa_bundler::incremental::IncrementalGraphCache::disabled(),
        ruvyxa_bundler::hooks::BuildHookPipeline::new(hosts),
        cache_dir,
        build_dependency_hash,
        artifact_graph_enabled,
    ))
}
