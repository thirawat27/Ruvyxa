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

use std::collections::BTreeSet;
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
    /// Modules a `build.onTransform` hook actually rewrote.
    ///
    /// Recorded because a transform is applied in the browser compile and
    /// nowhere else: the server render reads the module through
    /// `runtime/compiler.mjs`, which has no plugin hooks, so a rewritten value
    /// that reaches rendered markup makes the two documents disagree. Nothing
    /// downstream can notice on its own — both halves are internally
    /// consistent — so the build has to remember what was changed in order to
    /// say so. See `plugin_transform_divergence_diagnostics`.
    pub(crate) transformed_modules: Arc<Mutex<BTreeSet<PathBuf>>>,
}

/// What modules a plugin transform rewrote, remembered across builds.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredPluginTransforms {
    dependency_hash: String,
    modules: Vec<String>,
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

        // A resolve hook answers with a **path**. The file itself may be
        // virtual — a load hook can supply its contents — but the value still
        // has to name a location, because everything downstream treats it as
        // one. The two spellings every other ecosystem uses for a virtual
        // module are not paths, and both were joined onto the project root and
        // handed to the filesystem: `'\0virtual:x'` came back as
        // `strings passed to WinAPI cannot contain NULs` and `'virtual:x'` as
        // `The system cannot find the file specified`, neither naming a plugin.
        if !looks_like_a_path(path) {
            return Err(ruvyxa_bundler::BundleError::Compiler(format!(
                "a plugin resolved `{specifier}` to `{path}`, which is not a file path. Return a \
                 path — the file may be virtual and a `build.onLoad` hook can supply its contents \
                 — such as `${{root}}/virtual-{specifier}.ts`."
            )));
        }

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

        // Remembered, not merely applied: this rewrite happens in one of the
        // two compiles that read the module, and only the build can tell that
        // the other one will disagree.
        if let Ok(mut transformed) = self.transformed_modules.lock() {
            transformed.insert(ruvyxa_diagnostics::normalized_canonical_path(id));
        }

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

        Err(ruvyxa_bundler::BundleError::Compiler(
            ruvyxa_diagnostics::label_with_code(
                &result.code.unwrap_or_else(|| "RUV1700".to_string()),
                &result
                    .message
                    .or(result.stack)
                    .unwrap_or_else(|| "TypeScript plugin hook failed".to_string()),
            ),
        ))
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
                transformed_modules: Arc::new(Mutex::new(BTreeSet::new())),
            }),
        })
    }

    pub(crate) fn bridge(&self) -> Option<&TypeScriptPluginBridge> {
        self.bridge.as_ref()
    }

    /// Whether a plugin rewrites this module for the browser but not for the
    /// server.
    ///
    /// Both compilers run `build.onTransform` now, so an unguarded hook
    /// produces the same text on both sides and there is nothing to report. A
    /// hook that inspects `environment` can still rewrite one lane only — which
    /// is a legitimate thing to do, and a guaranteed hydration mismatch on a
    /// route that renders on the server and hydrates.
    ///
    /// Asked rather than inferred: the hook is right here, and running it twice
    /// over one file answers exactly the question. A handful of calls, once per
    /// build, for the modules a plugin actually touched.
    fn transform_differs_by_environment(&self, module: &Path) -> bool {
        let Some(bridge) = self.bridge.as_ref() else {
            return false;
        };
        let Ok(source) = std::fs::read_to_string(module) else {
            return false;
        };
        let lane = |environment: &str| -> Option<String> {
            let payload = serde_json::json!({
                "code": source,
                "id": module.display().to_string(),
                "environment": environment,
            });
            let value = bridge.call_runner("build.transform", payload).ok()??;
            value
                .get("code")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        let client = lane("client").unwrap_or_else(|| source.clone());
        let server = lane("server").unwrap_or_else(|| source.clone());
        client != server
    }

    /// Modules a `build.onTransform` hook rewrote during this build.
    pub(crate) fn transformed_modules(&self) -> BTreeSet<PathBuf> {
        self.bridge
            .as_ref()
            .and_then(|bridge| bridge.transformed_modules.lock().ok())
            .map(|transformed| transformed.clone())
            .unwrap_or_default()
    }

    /// Every module a transform hook has rewritten for these plugins, including
    /// the ones this build was too warm to recompile.
    ///
    /// A hook only runs on a compile that actually happens, so on a second
    /// build the browser bundle comes from cache, no hook is called, and the
    /// build learns nothing about what a plugin rewrites. A warning that
    /// appears once and then goes away is worse than no warning: it reads as
    /// something that was fixed.
    ///
    /// Keyed by the config dependency hash, which covers the plugin sources
    /// themselves — editing a plugin invalidates the record along with the
    /// bundle. Merged rather than replaced, because a partially warm build
    /// records only the modules it recompiled. A remembered module that no
    /// route reaches any more produces no warning, so a stale entry is inert
    /// rather than wrong.
    pub(crate) fn lane_divergent_modules(
        &self,
        cache_dir: &Path,
        dependency_hash: &str,
    ) -> BTreeSet<PathBuf> {
        self.remembered_transformed_modules(cache_dir, dependency_hash)
            .into_iter()
            .filter(|module| self.transform_differs_by_environment(module))
            .collect()
    }

    fn remembered_transformed_modules(
        &self,
        cache_dir: &Path,
        dependency_hash: &str,
    ) -> BTreeSet<PathBuf> {
        let store = cache_dir.join("plugin-transforms.json");
        let mut modules = self.transformed_modules();
        if let Ok(source) = std::fs::read_to_string(&store)
            && let Ok(stored) = serde_json::from_str::<StoredPluginTransforms>(&source)
            && stored.dependency_hash == dependency_hash
        {
            modules.extend(stored.modules.into_iter().map(PathBuf::from));
        }
        // A module the project no longer has is dropped here rather than
        // carried forever. The merge above is deliberate -- a module the current
        // build did not transform may still be one a previous build did, and
        // forgetting it loses the warning -- but nothing ever removed an entry,
        // so a deleted file stayed in the record until the dependency hash
        // changed. Every survivor is then re-examined on every build by
        // `transform_differs_by_environment`, which reads the file and calls the
        // plugin hook twice: two serial NDJSON round-trips through a single
        // `Mutex`-guarded worker, each carrying the module's whole source. For a
        // deleted module all of that work produces `false` by way of a failed
        // read, and the path is written back to be tried again next time.
        //
        // One `stat` per entry replaces it. The filter is at the write site so
        // the merge keeps its meaning: a module absent for a moment -- mid
        // rename, or generated and not yet written -- is forgotten and
        // remembered again the next time a build transforms it.
        modules.retain(|module| module.is_file());
        // Rewritten even when nothing survives, so a store whose every entry
        // named a deleted module is pruned rather than left on disk to be read
        // and discarded by every build after this one. Still no file created
        // for a project that has never had one.
        if modules.is_empty() && !store.exists() {
            return modules;
        }
        let record = StoredPluginTransforms {
            dependency_hash: dependency_hash.to_string(),
            modules: modules
                .iter()
                .map(|module| module.display().to_string())
                .collect(),
        };
        if let Ok(serialized) = serde_json::to_string(&record) {
            let _ = std::fs::create_dir_all(cache_dir);
            // A cache the build can rebuild: a failed write costs the next
            // build a warning it can recompute, not correctness.
            let _ = std::fs::write(&store, serialized);
        }
        modules
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
                "{}",
                ruvyxa_diagnostics::label_with_code(
                    &result.code.unwrap_or_else(|| "RUV1700".to_string()),
                    &result
                        .message
                        .or(result.stack)
                        .unwrap_or_else(|| format!("TypeScript plugin {label} hook failed")),
                )
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

/// The `environment` a build hook is told it is transforming for.
///
/// Replayed against `tests/fixtures/plugin-transform-lane-conformance.json`,
/// which `runtime/compiler.mjs` answers from too. Both compilers read every
/// module and both run `build.onTransform`, so a plugin that branches on
/// `environment` is choosing a lane — and it can only choose correctly while
/// the two agree on what the lanes are called.
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
    build_dependency_hash: &str,
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
        build_dependency_hash,
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

/// Whether a resolve hook's answer names a location on disk.
///
/// A separator or a file extension is what distinguishes a path from an id:
/// `virtual:x` and `\0virtual:x` have neither, and both used to be joined onto
/// the project root and opened.
fn looks_like_a_path(value: &str) -> bool {
    if value.contains('\0') {
        return false;
    }
    let has_separator = value.contains('/') || value.contains('\\');
    let has_extension = Path::new(value)
        .extension()
        .is_some_and(|extension| !extension.is_empty());
    has_separator || has_extension
}

#[cfg(test)]
mod plugin_lane_tests {
    use super::*;

    /// The Rust half of the plugin-transform lane contract.
    ///
    /// The JavaScript half is `tests/packages/ruvyxa/plugin-transform-lane.test.mjs`,
    /// which drives `runtime/compiler.mjs` through the same table. Both
    /// compilers read every module and both run `build.onTransform`; a lane
    /// renamed on one side and not the other would silently change which half
    /// of a project a plugin rewrites.
    #[test]
    fn build_hook_environments_match_the_shared_conformance_table() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/plugin-transform-lane-conformance.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let fixture: serde_json::Value =
            serde_json::from_str(&source).expect("the lane fixture parses");
        let cases = fixture["environments"]["cases"]
            .as_array()
            .expect("environment cases");
        assert!(!cases.is_empty(), "the fixture must carry cases");

        for case in cases {
            let target = match case["rustTarget"].as_str().expect("rustTarget") {
                "Client" => ruvyxa_bundler::BundleTarget::Client,
                "Ssr" => ruvyxa_bundler::BundleTarget::Ssr,
                "Edge" => ruvyxa_bundler::BundleTarget::Edge,
                "ReactServer" => ruvyxa_bundler::BundleTarget::ReactServer,
                other => panic!("unknown bundle target in fixture: {other}"),
            };
            assert_eq!(
                plugin_environment(target),
                case["expect"].as_str().expect("expect"),
                "{:?} — {}",
                target,
                case["why"].as_str().unwrap_or_default()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module the project no longer has leaves the record.
    ///
    /// Nothing removed an entry before: `remembered_transformed_modules` merged
    /// the stored set with the current one and wrote it back unfiltered, so a
    /// deleted file stayed until the dependency hash changed. Every survivor is
    /// re-examined on every build by `transform_differs_by_environment`, which
    /// reads the module and calls the plugin hook twice -- so a deleted module
    /// cost two serial worker round-trips per build to conclude nothing, for as
    /// long as the generation lasted.
    #[test]
    fn a_deleted_module_leaves_the_plugin_transform_record() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("cache dir");

        let live = temp.path().join("kept.ts");
        std::fs::write(&live, b"export const kept = 1\n").expect("write");
        let gone = temp.path().join("deleted.ts");

        let store = cache_dir.join("plugin-transforms.json");
        std::fs::write(
            &store,
            serde_json::to_string(&StoredPluginTransforms {
                dependency_hash: "hash".to_string(),
                modules: vec![live.display().to_string(), gone.display().to_string()],
            })
            .expect("serialize"),
        )
        .expect("write store");

        let session = TypeScriptPluginBuildSession { bridge: None };
        let remembered = session.remembered_transformed_modules(&cache_dir, "hash");

        assert!(
            remembered.contains(&live),
            "a module that still exists stays remembered",
        );
        assert!(
            !remembered.contains(&gone),
            "a module the project no longer has must not be handed to the caller, \
             which would read it and call the plugin hook twice for nothing",
        );

        let rewritten = std::fs::read_to_string(&store).expect("the record is rewritten");
        assert!(
            !rewritten.contains("deleted.ts"),
            "and it must not be written back, or the next build pays for it again",
        );
        assert!(rewritten.contains("kept.ts"));
    }

    /// A record whose every entry is dead is pruned, not left to be re-read.
    #[test]
    fn a_record_of_only_deleted_modules_is_emptied() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp.path().join("cache");
        std::fs::create_dir_all(&cache_dir).expect("cache dir");
        let store = cache_dir.join("plugin-transforms.json");
        std::fs::write(
            &store,
            serde_json::to_string(&StoredPluginTransforms {
                dependency_hash: "hash".to_string(),
                modules: vec![temp.path().join("gone.ts").display().to_string()],
            })
            .expect("serialize"),
        )
        .expect("write store");

        let session = TypeScriptPluginBuildSession { bridge: None };
        assert!(
            session
                .remembered_transformed_modules(&cache_dir, "hash")
                .is_empty()
        );
        let rewritten = std::fs::read_to_string(&store).expect("the record survives, emptied");
        assert!(!rewritten.contains("gone.ts"));
    }

    /// A project that has never had a record does not get an empty one.
    #[test]
    fn no_record_is_created_for_a_project_with_no_transforms() {
        let temp = tempfile::tempdir().expect("temp dir");
        let cache_dir = temp.path().join("cache");
        let session = TypeScriptPluginBuildSession { bridge: None };
        assert!(
            session
                .remembered_transformed_modules(&cache_dir, "hash")
                .is_empty()
        );
        assert!(!cache_dir.join("plugin-transforms.json").exists());
    }
}
