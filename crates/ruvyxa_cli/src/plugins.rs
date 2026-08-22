//! TypeScript build-plugin bridge.
//!
//! Ruvyxa's build plugins are written in TypeScript and run in a long-lived
//! JavaScript worker process; the bundler calls them through the synchronous
//! [`BuildHooks`](ruvyxa_bundler::hooks::BuildHooks) trait. This module is the
//! adapter between those two worlds: it owns the worker's lifetime, frames
//! newline-delimited JSON over its stdio, and turns a worker fault into a build
//! error rather than a hang.
//!
//! One worker is shared by every route in a build session, so plugin startup
//! cost is paid once instead of once per route.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use ruvyxa_dev_server::{JavaScriptRuntime, find_runtime_script};

use crate::BuildPluginConfig;

#[derive(Clone)]
pub(crate) struct TypeScriptPluginBridge {
    pub(crate) project_root: PathBuf,
    pub(crate) workers: Arc<Vec<Mutex<TypeScriptPluginWorker>>>,
    pub(crate) next_worker: Arc<AtomicUsize>,
    pub(crate) content_compiler_enabled: bool,
}

/// Longest one build-plugin hook may run before its worker is stopped.
///
/// Matches `DEFAULT_PLUGIN_HOOK_TIMEOUT_MS` for middleware plugin hooks, so a
/// plugin author sees the same budget on both sides of the framework. Without
/// it a plugin that never resolves hung the whole build with no diagnostic —
/// the failure this module's own documentation promises not to have.
const PLUGIN_HOOK_TIMEOUT: Duration =
    Duration::from_millis(ruvyxa_middleware::config::DEFAULT_PLUGIN_HOOK_TIMEOUT_MS);

pub(crate) struct TypeScriptPluginWorker {
    pub(crate) child: Child,
    pub(crate) stdin: ChildStdin,
    /// Response lines pushed by this worker's reader thread. See
    /// [`TypeScriptPluginWorker::spawn`] for why the read is not inline.
    responses: mpsc::Receiver<std::io::Result<String>>,
    /// Set once a hook timed out; the worker is dead and must not be reused.
    poisoned: bool,
}

/// Owns the single persistent plugin registry used by one production build.
///
/// Lifecycle and bundler hooks intentionally share this host so config
/// compilation, plugin registration, and process startup happen only once.
pub(crate) struct TypeScriptPluginBuildSession {
    pub(crate) bridge: Option<TypeScriptPluginBridge>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginRuntimeOutput {
    pub(crate) ok: bool,
    pub(crate) result: Option<serde_json::Value>,
    pub(crate) code: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) stack: Option<String>,
}

impl ruvyxa_bundler::hooks::BuildHooks for TypeScriptPluginBridge {
    fn host_name(&self) -> &str {
        "ruvyxa-typescript-plugin-host"
    }

    fn resolve_id(
        &self,
        specifier: &str,
        importer: Option<&Path>,
        ctx: &ruvyxa_bundler::hooks::BuildHookContext,
    ) -> ruvyxa_bundler::Result<Option<PathBuf>> {
        let payload = serde_json::json!({
            "id": specifier,
            "importer": importer.map(|path| path.display().to_string()),
            "environment": plugin_environment(ctx.target)
        });
        let Some(value) = self.call_runner("build.resolve", payload)? else {
            return Ok(None);
        };
        let Some(path) = value.as_str() else {
            return Ok(None);
        };

        let resolved = PathBuf::from(path);
        let resolved = if resolved.is_absolute() {
            resolved
        } else {
            self.project_root.join(resolved)
        };

        Ok(Some(ruvyxa_diagnostics::normalized_canonical_path(
            &resolved,
        )))
    }

    fn load(
        &self,
        id: &Path,
        ctx: &ruvyxa_bundler::hooks::BuildHookContext,
    ) -> ruvyxa_bundler::Result<Option<ruvyxa_bundler::hooks::TransformOutput>> {
        let payload = serde_json::json!({
            "id": id.display().to_string(),
            "environment": plugin_environment(ctx.target)
        });
        let Some(value) = self.call_runner("build.load", payload)? else {
            return Ok(None);
        };
        let Some(code) = value.get("code").and_then(serde_json::Value::as_str) else {
            return Ok(None);
        };
        Ok(Some(ruvyxa_bundler::hooks::TransformOutput {
            code: code.to_string(),
            map: value
                .get("map")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        }))
    }

    fn transform(
        &self,
        code: &str,
        id: &Path,
        ctx: &ruvyxa_bundler::hooks::BuildHookContext,
    ) -> ruvyxa_bundler::Result<Option<ruvyxa_bundler::hooks::TransformOutput>> {
        let payload = serde_json::json!({
            "code": code,
            "id": id.display().to_string(),
            "environment": plugin_environment(ctx.target)
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
        ctx: &ruvyxa_bundler::hooks::BuildHookContext,
    ) -> ruvyxa_bundler::Result<Option<ruvyxa_bundler::hooks::TransformOutput>> {
        if !self.content_compiler_enabled {
            return Ok(None);
        }
        let payload = serde_json::json!({
            "code": code,
            "id": id.display().to_string(),
            "environment": plugin_environment(ctx.target)
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

impl TypeScriptPluginBridge {
    pub(crate) fn call_worker(
        &self,
        payload: &serde_json::Value,
    ) -> ruvyxa_bundler::Result<PluginRuntimeOutput> {
        let worker_index = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let mut worker = self.workers[worker_index].lock().map_err(|_| {
            ruvyxa_bundler::BundleError::Compiler(
                "TypeScript plugin worker lock was poisoned".into(),
            )
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

        Err(ruvyxa_bundler::BundleError::Compiler(format!(
            "{} {}",
            result.code.unwrap_or_else(|| "RUV1700".to_string()),
            result
                .message
                .or(result.stack)
                .unwrap_or_else(|| "TypeScript plugin hook failed".to_string())
        )))
    }
}

impl TypeScriptPluginBuildSession {
    pub(crate) fn new(
        root: &Path,
        plugins: &[BuildPluginConfig],
        runtime: JavaScriptRuntime,
        content_compiler_enabled: bool,
        react_compiler_enabled: bool,
    ) -> anyhow::Result<Self> {
        if plugins.is_empty() && !content_compiler_enabled && !react_compiler_enabled {
            return Ok(Self { bridge: None });
        }

        let runner = find_runtime_script(root, "plugin-runtime.mjs")
            .ok_or_else(|| anyhow::anyhow!("RUV1701 TypeScript plugin runtime not found"))?;
        let project_root = ruvyxa_diagnostics::normalized_canonical_path(root);
        let worker =
            TypeScriptPluginWorker::spawn(&runner, &project_root, runtime).map_err(|error| {
                anyhow::anyhow!("failed to start TypeScript plugin runtime: {error}")
            })?;
        Ok(Self {
            bridge: Some(TypeScriptPluginBridge {
                project_root,
                workers: Arc::new(vec![Mutex::new(worker)]),
                next_worker: Arc::new(AtomicUsize::new(0)),
                content_compiler_enabled,
            }),
        })
    }

    pub(crate) fn bridge(&self) -> Option<&TypeScriptPluginBridge> {
        self.bridge.as_ref()
    }

    pub(crate) fn run_start(&self, out_dir: &Path) -> anyhow::Result<()> {
        self.call_lifecycle(
            "build.start",
            serde_json::json!({ "outDir": out_dir }),
            "build-start",
        )
    }

    pub(crate) fn run_complete(
        &self,
        out_dir: &Path,
        manifest: &serde_json::Value,
    ) -> anyhow::Result<()> {
        self.call_lifecycle(
            "build.complete",
            serde_json::json!({
                "outDir": out_dir,
                "manifest": manifest,
            }),
            "build-complete",
        )
    }

    pub(crate) fn call_lifecycle(
        &self,
        hook: &str,
        mut payload: serde_json::Value,
        label: &str,
    ) -> anyhow::Result<()> {
        let Some(bridge) = &self.bridge else {
            return Ok(());
        };
        payload["hook"] = serde_json::Value::String(hook.to_string());
        let result = bridge
            .call_worker(&payload)
            .map_err(|error| anyhow::anyhow!("TypeScript plugin {label} hook failed: {error}"))?;
        if !result.ok {
            anyhow::bail!(
                "{} {}",
                result.code.unwrap_or_else(|| "RUV1700".to_string()),
                result
                    .message
                    .or(result.stack)
                    .unwrap_or_else(|| format!("TypeScript plugin {label} hook failed"))
            );
        }
        Ok(())
    }
}

impl TypeScriptPluginWorker {
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
            // This worker exists to run build hooks, so its plugins see the
            // environment a build runs in. Nothing here serves requests.
            .arg("--environment=production")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Stdout is reserved for the NDJSON protocol. The runtime routes
            // plugin console output to stderr, so inherit it instead of
            // silently discarding diagnostics during production builds.
            .stderr(Stdio::inherit())
            .env("RUVYXA_RUNTIME", runtime.command())
            .spawn()
            .map_err(|err| {
                ruvyxa_bundler::BundleError::Compiler(format!(
                    "failed to start persistent TypeScript plugin worker: {err}"
                ))
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ruvyxa_bundler::BundleError::Compiler(
                "failed to open TypeScript plugin worker stdin".into(),
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ruvyxa_bundler::BundleError::Compiler(
                "failed to open TypeScript plugin worker stdout".into(),
            )
        })?;

        // Responses are read on a dedicated thread rather than inline, so a
        // plugin that never answers costs a bounded wait instead of the whole
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
    ) -> ruvyxa_bundler::Result<PluginRuntimeOutput> {
        self.call_with_timeout(payload, PLUGIN_HOOK_TIMEOUT)
    }

    /// [`Self::call`] with an explicit budget, so the timeout path can be tested
    /// without a test that waits out the production one.
    pub(crate) fn call_with_timeout(
        &mut self,
        payload: &serde_json::Value,
        timeout: Duration,
    ) -> ruvyxa_bundler::Result<PluginRuntimeOutput> {
        // A worker that timed out may still answer the previous request later.
        // Reusing it would pair that stale line with the next call's payload, so
        // it stays refused rather than silently returning another hook's result.
        if self.poisoned {
            return Err(ruvyxa_bundler::BundleError::Compiler(
                "TypeScript plugin worker was stopped after an earlier hook timed out".into(),
            ));
        }

        writeln!(self.stdin, "{payload}").map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "failed to send TypeScript plugin worker payload: {err}"
            ))
        })?;
        self.stdin.flush().map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "failed to flush TypeScript plugin worker payload: {err}"
            ))
        })?;

        let stdout = match self.responses.recv_timeout(timeout) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                return Err(ruvyxa_bundler::BundleError::Compiler(format!(
                    "failed to read TypeScript plugin worker response: {error}"
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
                    "TypeScript plugin worker exited before responding (status: {status})"
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.poisoned = true;
                // Kill now rather than at drop: the build is about to fail, and
                // a plugin stuck in an infinite loop would otherwise keep a core
                // busy until the CLI process itself exits.
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(ruvyxa_bundler::BundleError::Compiler(format!(
                    "RUV1701 a TypeScript build plugin hook did not respond within {} seconds. \
                     The plugin worker was stopped. Check the plugin for an unresolved promise \
                     or a blocking loop.",
                    timeout.as_secs()
                )));
            }
        };

        serde_json::from_str(stdout.trim()).map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "TypeScript plugin worker returned invalid output: {err}; stdout: {}",
                stdout.trim()
            ))
        })
    }
}

impl Drop for TypeScriptPluginWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn plugin_environment(target: ruvyxa_bundler::BundleTarget) -> &'static str {
    match target {
        ruvyxa_bundler::BundleTarget::Client => "client",
        ruvyxa_bundler::BundleTarget::Ssr => "server",
        ruvyxa_bundler::BundleTarget::Edge => "edge",
        // A plugin sees the runtime it is transforming for. The server
        // components graph runs on the server, so `server` is what a
        // `PluginEnvironment` consumer already knows how to handle; the
        // `react-server` condition is a resolution detail, not a new host.
        ruvyxa_bundler::BundleTarget::ReactServer => "server",
    }
}

pub(crate) fn bundle_context_for_build(
    config_dependency_hash: &str,
    cache_dir: &Path,
    plugin_session: &TypeScriptPluginBuildSession,
    server_references: &[ruvyxa_dev_server::ServerReferenceSource],
) -> anyhow::Result<ruvyxa_bundler::BundleContext> {
    let artifact_graph_enabled = !matches!(
        std::env::var("RUVYXA_DISABLE_ARTIFACT_CACHE").as_deref(),
        Ok("1" | "true")
    );
    let compile_cache = ruvyxa_bundler::cache::CompileCache::at_dir_with_namespace(
        cache_dir,
        true,
        config_dependency_hash,
    );
    // Ordered before any project plugin: a `'use server'` module is not the
    // file on disk as far as a browser bundle is concerned, so a plugin that
    // transforms it should see the reference, not the server code it replaced.
    let mut hosts: Vec<Arc<dyn ruvyxa_bundler::hooks::BuildHooks>> = Vec::new();
    let substitutions =
        crate::server_references::ServerReferenceSources::new(server_references.iter().cloned());
    if !substitutions.is_empty() {
        hosts.push(Arc::new(substitutions));
    }
    if let Some(bridge) = plugin_session.bridge() {
        hosts.push(Arc::new(bridge.clone()));
    }
    if hosts.is_empty() {
        return Ok(ruvyxa_bundler::BundleContext::for_build_with_artifacts(
            compile_cache,
            ruvyxa_bundler::resolver::ResolveGraphCache::for_build(),
            cache_dir,
            config_dependency_hash,
            artifact_graph_enabled,
        ));
    }

    Ok(ruvyxa_bundler::BundleContext::with_build_hooks_for_build(
        compile_cache,
        ruvyxa_bundler::resolver::ResolveGraphCache::for_build(),
        ruvyxa_bundler::incremental::IncrementalGraphCache::disabled(),
        ruvyxa_bundler::hooks::BuildHookPipeline::new(hosts),
        cache_dir,
        config_dependency_hash,
        artifact_graph_enabled,
    ))
}
