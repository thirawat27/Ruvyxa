//! Persistent process bridge for TypeScript plugin middleware.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::warn;

use ruvyxa_diagnostics::{Result, RuvyxaError};

use crate::config::DEFAULT_PLUGIN_HOOK_TIMEOUT_MS;

/// HTTP request representation transported losslessly over the plugin protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginHttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
}

/// HTTP response representation transported losslessly over the plugin protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
}

/// Request-middleware continuation returned by the TypeScript registry.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PluginHttpRequestResult {
    Request { request: PluginHttpRequest },
    Response { response: PluginHttpResponse },
}

/// Mirror of the TypeScript registry's route matching: `*` matches everything,
/// a trailing `*` matches by prefix, anything else is an exact pathname match.
fn matches_route_patterns(patterns: Option<&[String]>, pathname: &str) -> bool {
    let Some(patterns) = patterns else {
        return true;
    };
    patterns.iter().any(|pattern| {
        if pattern == "*" {
            true
        } else if let Some(prefix) = pattern.strip_suffix('*') {
            pathname.starts_with(prefix)
        } else {
            pathname == pattern
        }
    })
}

/// HTTP socket registrations reported after every plugin has registered.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginHttpDescriptor {
    pub request: usize,
    pub response: usize,
    pub routes: usize,
    #[serde(default)]
    pub request_match: Option<Vec<String>>,
    #[serde(default)]
    pub response_match: Option<Vec<String>>,
}

/// Build socket registrations reported by the TypeScript registry.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginBuildDescriptor {
    pub start: usize,
    pub resolve: usize,
    pub load: usize,
    pub transform: usize,
    pub complete: usize,
}

/// Development socket registrations reported by the TypeScript registry.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginDevDescriptor {
    pub file_change: usize,
}

/// Registration-time diagnostic emitted by a plugin.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PluginDiagnosticDescriptor {
    pub plugin: String,
    pub level: String,
    pub code: String,
    pub message: String,
}

/// Framework-owned native capabilities. Unknown IDs fail deserialization.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "id")]
pub enum NativeCapabilityDescriptor {
    #[serde(rename = "realtime@1", rename_all = "camelCase")]
    Realtime {
        plugin: String,
        path: String,
        heartbeat_ms: u64,
        capacity: usize,
    },
    #[serde(rename = "presence@1", rename_all = "camelCase")]
    Presence {
        plugin: String,
        path: String,
        heartbeat_ms: u64,
    },
}

/// Descriptor for the plugin registry contract.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginRegistryDescriptor {
    pub plugins: Vec<String>,
    pub http: PluginHttpDescriptor,
    pub build: PluginBuildDescriptor,
    pub dev: PluginDevDescriptor,
    pub diagnostics: Vec<PluginDiagnosticDescriptor>,
    pub capabilities: Vec<NativeCapabilityDescriptor>,
}

/// Native realtime registration exposed to the development server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeDescriptor {
    pub plugin: String,
    pub path: String,
    pub heartbeat_ms: u64,
    pub capacity: usize,
}

/// Native presence registration exposed to the development server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceDescriptor {
    pub plugin: String,
    pub path: String,
    pub heartbeat_ms: u64,
}

impl PluginRegistryDescriptor {
    pub fn realtime(&self) -> Option<RealtimeDescriptor> {
        self.capabilities
            .iter()
            .find_map(|capability| match capability {
                NativeCapabilityDescriptor::Realtime {
                    plugin,
                    path,
                    heartbeat_ms,
                    capacity,
                } => Some(RealtimeDescriptor {
                    plugin: plugin.clone(),
                    path: path.clone(),
                    heartbeat_ms: *heartbeat_ms,
                    capacity: *capacity,
                }),
                _ => None,
            })
    }

    pub fn presence(&self) -> Option<PresenceDescriptor> {
        self.capabilities
            .iter()
            .find_map(|capability| match capability {
                NativeCapabilityDescriptor::Presence {
                    plugin,
                    path,
                    heartbeat_ms,
                } => Some(PresenceDescriptor {
                    plugin: plugin.clone(),
                    path: path.clone(),
                    heartbeat_ms: *heartbeat_ms,
                }),
                _ => None,
            })
    }
}

#[derive(Debug, Deserialize)]
struct RuntimeOutput {
    ok: bool,
    result: Option<serde_json::Value>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

struct PluginWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Which environment the plugin host is serving.
///
/// A plugin cannot otherwise tell `ruvyxa dev` from `ruvyxa start`, and some
/// first-party plugins need to: `feed` and `searchIndex` answer a request for
/// the file they generate only in development, because in production that file
/// has already been written by the build and re-running a project-supplied
/// loader per request would put a file read or a database query on the
/// response path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginEnvironment {
    Development,
    Production,
}

impl PluginEnvironment {
    fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Production => "production",
        }
    }
}

/// Spawn parameters retained so a crashed plugin host can be restarted.
struct PluginSpawnConfig {
    project_root: std::path::PathBuf,
    runtime_script: std::path::PathBuf,
    executable: std::path::PathBuf,
    executable_args: Vec<String>,
    environment: PluginEnvironment,
}

/// Persistent TypeScript plugin host shared by the request and response phases.
///
/// One or more identical worker processes serve hook calls round-robin. Every
/// worker loads the same registry from `ruvyxa.config.ts`; module-level plugin
/// state is per-process, which is why the pool defaults to a single worker.
pub struct PluginHost {
    workers: Vec<Mutex<PluginWorker>>,
    next_worker: std::sync::atomic::AtomicUsize,
    descriptor: PluginRegistryDescriptor,
    spawn: PluginSpawnConfig,
    call_timeout: Duration,
}

impl PluginHost {
    /// Start the selected JavaScript runtime and validate the configured registry.
    pub async fn start(
        project_root: &Path,
        runtime_script: &Path,
        executable: &Path,
    ) -> Result<Self> {
        Self::start_pool(project_root, runtime_script, executable, 1).await
    }

    /// Start a pool of identical plugin host workers dispatched round-robin.
    pub async fn start_pool(
        project_root: &Path,
        runtime_script: &Path,
        executable: &Path,
        pool_size: usize,
    ) -> Result<Self> {
        Self::start_pool_with_timeout(
            project_root,
            runtime_script,
            executable,
            pool_size,
            Duration::from_millis(DEFAULT_PLUGIN_HOOK_TIMEOUT_MS),
        )
        .await
    }

    /// Start a pool with an explicit upper bound for each registry call.
    pub async fn start_pool_with_timeout(
        project_root: &Path,
        runtime_script: &Path,
        executable: &Path,
        pool_size: usize,
        call_timeout: Duration,
    ) -> Result<Self> {
        // Production is the safe default for a host that did not state its
        // environment: it withholds development-only request handling rather
        // than enabling it for a server that may be serving real traffic.
        Self::start_pool_with_timeout_and_args(
            project_root,
            runtime_script,
            executable,
            &[],
            pool_size,
            call_timeout,
            PluginEnvironment::Production,
        )
        .await
    }

    /// Start a pool while passing runtime-specific arguments before the script.
    pub async fn start_pool_with_timeout_and_args(
        project_root: &Path,
        runtime_script: &Path,
        executable: &Path,
        executable_args: &[&str],
        pool_size: usize,
        call_timeout: Duration,
        environment: PluginEnvironment,
    ) -> Result<Self> {
        let spawn = PluginSpawnConfig {
            project_root: project_root.to_path_buf(),
            runtime_script: runtime_script.to_path_buf(),
            executable: executable.to_path_buf(),
            executable_args: executable_args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect(),
            environment,
        };
        let mut worker = spawn_worker(&spawn)?;
        let descriptor =
            call_worker_with_timeout(&mut worker, "describe", serde_json::json!({}), call_timeout)
                .await
                .map_err(CallFailure::into_error)?;
        let descriptor: PluginRegistryDescriptor =
            serde_json::from_value(descriptor).map_err(|error| {
                RuvyxaError::Message(format!(
                    "RUV1701 TypeScript plugin host returned an invalid registry descriptor: {error}"
                ))
            })?;
        for diagnostic in &descriptor.diagnostics {
            warn!(
                target: "ruvyxa::plugin",
                plugin = %diagnostic.plugin,
                code = %diagnostic.code,
                level = %diagnostic.level,
                "{}",
                diagnostic.message
            );
        }

        let mut workers = vec![Mutex::new(worker)];
        // Extra workers only pay off for middleware traffic; a registry
        // without middleware never fans out.
        let http_hooks = descriptor.http.request + descriptor.http.response;
        if http_hooks > 0 {
            for _ in 1..pool_size.max(1) {
                workers.push(Mutex::new(spawn_worker(&spawn)?));
            }
        }

        Ok(Self {
            workers,
            next_worker: std::sync::atomic::AtomicUsize::new(0),
            descriptor,
            spawn,
            call_timeout,
        })
    }

    /// Number of live worker processes in the pool.
    pub fn pool_size(&self) -> usize {
        self.workers.len()
    }

    pub fn descriptor(&self) -> &PluginRegistryDescriptor {
        &self.descriptor
    }

    /// Whether any request middleware could match this pathname. Lets the
    /// server skip the plugin round-trip entirely for non-matching requests.
    pub fn wants_request(&self, pathname: &str) -> bool {
        let http = &self.descriptor.http;
        http.request > 0 && matches_route_patterns(http.request_match.as_deref(), pathname)
    }

    /// Whether any response middleware could match this pathname.
    pub fn wants_response(&self, pathname: &str) -> bool {
        let http = &self.descriptor.http;
        http.response > 0 && matches_route_patterns(http.response_match.as_deref(), pathname)
    }

    pub async fn execute_request(
        &self,
        request: &PluginHttpRequest,
    ) -> Result<PluginHttpRequestResult> {
        let value = self
            .call("http.request", serde_json::json!({ "request": request }))
            .await?;
        serde_json::from_value(value).map_err(|error| {
            RuvyxaError::Message(format!(
                "RUV1701 TypeScript request middleware returned an invalid result: {error}"
            ))
        })
    }

    pub async fn execute_response(
        &self,
        request: &PluginHttpRequest,
        response: &PluginHttpResponse,
    ) -> Result<PluginHttpResponse> {
        let value = self
            .call(
                "http.response",
                serde_json::json!({ "request": request, "response": response }),
            )
            .await?;
        serde_json::from_value(value.get("response").cloned().unwrap_or_default()).map_err(
            |error| {
                RuvyxaError::Message(format!(
                    "RUV1701 TypeScript response middleware returned an invalid result: {error}"
                ))
            },
        )
    }

    /// Notify development-only file-change hooks after the framework invalidates its caches.
    pub async fn notify_file_change(&self, paths: &[String]) -> Result<()> {
        if self.descriptor.dev.file_change == 0 {
            return Ok(());
        }
        self.call("dev.fileChange", serde_json::json!({ "paths": paths }))
            .await?;
        Ok(())
    }

    async fn call(&self, hook: &str, payload: serde_json::Value) -> Result<serde_json::Value> {
        let start = self
            .next_worker
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.workers.len();

        // Preserve round-robin fairness while avoiding head-of-line blocking:
        // if the selected worker is busy, use another idle process before
        // queueing behind it.
        for offset in 0..self.workers.len() {
            let index = (start + offset) % self.workers.len();
            if let Ok(worker) = self.workers[index].try_lock() {
                return self.call_locked(worker, hook, payload).await;
            }
        }

        let worker = self.workers[start].lock().await;
        self.call_locked(worker, hook, payload).await
    }

    async fn call_locked(
        &self,
        mut worker: tokio::sync::MutexGuard<'_, PluginWorker>,
        hook: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match call_worker_with_timeout(&mut worker, hook, payload.clone(), self.call_timeout).await
        {
            Ok(value) => Ok(value),
            Err(CallFailure::Hook(error)) => Err(error),
            Err(failure @ (CallFailure::NotDelivered(_) | CallFailure::WorkerGone(_))) => {
                // Either way the worker is unusable and must be replaced. What
                // differs is whether the hook may already have run:
                // `NotDelivered` proves it did not, while `WorkerGone` leaves it
                // unknown — the worker can have executed the handler's side
                // effects and died before answering. Replaying an unknown call
                // applies those effects twice, which is why `worker_pool` gates
                // its own retry on `WorkerRequest::is_idempotent`. This bridge
                // now draws the same line instead of retrying blindly.
                let delivered = matches!(failure, CallFailure::WorkerGone(_));
                let error = failure.into_error();
                let retryable = !delivered || hook_is_idempotent(hook);
                warn!(
                    target: "ruvyxa::plugin",
                    delivered,
                    retryable,
                    "TypeScript plugin host stopped responding ({error}); restarting it"
                );
                replace_worker(&mut worker, &self.spawn)?;
                if !retryable {
                    return Err(error);
                }
                match call_worker_with_timeout(&mut worker, hook, payload, self.call_timeout).await
                {
                    Ok(value) => Ok(value),
                    Err(CallFailure::Hook(error)) => Err(error),
                    Err(failure) => {
                        let error = failure.into_error();
                        replace_worker(&mut worker, &self.spawn)?;
                        Err(error)
                    }
                }
            }
            Err(CallFailure::WorkerPoisoned(error)) => {
                warn!(
                    target: "ruvyxa::plugin",
                    "TypeScript plugin host protocol became unusable ({error}); replacing it without retrying the hook"
                );
                replace_worker(&mut worker, &self.spawn)?;
                Err(error)
            }
        }
    }
}

/// Whether replaying `hook` after a worker death is free of duplicate effects.
///
/// Only `describe` qualifies: it reports the registry's shape and runs no
/// user-supplied handler. Every other hook invokes plugin code that may write to
/// a database, emit a message, or increment a counter, and a worker that died
/// after running the handler but before answering is indistinguishable from one
/// that died before running it.
fn hook_is_idempotent(hook: &str) -> bool {
    matches!(hook, "describe")
}

fn replace_worker(worker: &mut PluginWorker, spawn: &PluginSpawnConfig) -> Result<()> {
    let _ = worker.child.start_kill();
    *worker = spawn_worker(spawn)?;
    Ok(())
}

fn spawn_worker(spawn: &PluginSpawnConfig) -> Result<PluginWorker> {
    let mut child = Command::new(&spawn.executable)
        .args(&spawn.executable_args)
        .arg(&spawn.runtime_script)
        .arg(&spawn.project_root)
        .arg("--persistent")
        // Read by name rather than by position: the one-shot build path passes
        // a hook name where this path passes `--persistent`.
        .arg(format!("--environment={}", spawn.environment.as_str()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            RuvyxaError::Message(format!("Failed to start TypeScript plugin host: {error}"))
        })?;
    let stdin = child.stdin.take().ok_or_else(|| {
        RuvyxaError::Message("TypeScript plugin host stdin was not available".to_string())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        RuvyxaError::Message("TypeScript plugin host stdout was not available".to_string())
    })?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(target: "ruvyxa::plugin", "{line}");
            }
        });
    }
    Ok(PluginWorker {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

/// Whether a failed hook call left the worker process unusable.
enum CallFailure {
    /// The worker is alive; the hook itself failed. Never retried.
    Hook(RuvyxaError),
    /// Writing the request failed, so the worker never received it. The hook
    /// cannot have run, so replaying it on a fresh worker is always safe.
    NotDelivered(RuvyxaError),
    /// The request was written but no response came back — the worker exited or
    /// its output pipe broke. The hook may have run to completion first, so only
    /// a side-effect-free hook may be replayed.
    WorkerGone(RuvyxaError),
    /// The worker may still be alive, but its request/response stream can no
    /// longer be correlated safely. Replaced without retrying the hook.
    WorkerPoisoned(RuvyxaError),
}

impl CallFailure {
    fn into_error(self) -> RuvyxaError {
        match self {
            Self::Hook(error)
            | Self::NotDelivered(error)
            | Self::WorkerGone(error)
            | Self::WorkerPoisoned(error) => error,
        }
    }
}

async fn call_worker_with_timeout(
    worker: &mut PluginWorker,
    hook: &str,
    payload: serde_json::Value,
    call_timeout: Duration,
) -> std::result::Result<serde_json::Value, CallFailure> {
    enforce_call_timeout(hook, call_timeout, call_worker(worker, hook, payload)).await
}

async fn enforce_call_timeout<F>(
    hook: &str,
    call_timeout: Duration,
    call: F,
) -> std::result::Result<serde_json::Value, CallFailure>
where
    F: std::future::Future<Output = std::result::Result<serde_json::Value, CallFailure>>,
{
    tokio::time::timeout(call_timeout, call)
        .await
        .unwrap_or_else(|_| {
            Err(CallFailure::WorkerPoisoned(RuvyxaError::Message(format!(
                "RUV1700 TypeScript plugin hook `{hook}` timed out after {} ms",
                call_timeout.as_millis()
            ))))
        })
}

async fn call_worker(
    worker: &mut PluginWorker,
    hook: &str,
    mut payload: serde_json::Value,
) -> std::result::Result<serde_json::Value, CallFailure> {
    payload["hook"] = serde_json::Value::String(hook.to_string());
    let mut encoded = serde_json::to_vec(&payload).map_err(|error| {
        CallFailure::Hook(RuvyxaError::Message(format!(
            "Failed to encode TypeScript plugin request: {error}"
        )))
    })?;
    encoded.push(b'\n');
    // A broken pipe here means the request never reached the worker, so the
    // hook definitely did not run and the call is safe to replay.
    worker.stdin.write_all(&encoded).await.map_err(|error| {
        CallFailure::NotDelivered(RuvyxaError::Message(format!(
            "Failed to write to TypeScript plugin host: {error}"
        )))
    })?;
    worker.stdin.flush().await.map_err(|error| {
        CallFailure::NotDelivered(RuvyxaError::Message(format!(
            "Failed to flush TypeScript plugin request: {error}"
        )))
    })?;

    let mut line = String::new();
    let bytes = worker.stdout.read_line(&mut line).await.map_err(|error| {
        CallFailure::WorkerGone(RuvyxaError::Message(format!(
            "Failed to read TypeScript plugin response: {error}"
        )))
    })?;
    if bytes == 0 {
        let status = worker
            .child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(CallFailure::WorkerGone(RuvyxaError::Message(format!(
            "RUV1700 TypeScript plugin host exited before responding (status: {status})"
        ))));
    }
    let output = decode_runtime_output(line.trim())?;
    if output.ok {
        return Ok(output.result.unwrap_or(serde_json::Value::Null));
    }
    Err(CallFailure::Hook(RuvyxaError::Message(format!(
        "{} {}",
        output.code.unwrap_or_else(|| "RUV1700".to_string()),
        output
            .message
            .or(output.stack)
            .unwrap_or_else(|| "TypeScript plugin hook failed".to_string())
    ))))
}

fn decode_runtime_output(line: &str) -> std::result::Result<RuntimeOutput, CallFailure> {
    serde_json::from_str(line).map_err(|error| {
        CallFailure::WorkerPoisoned(RuvyxaError::Message(format!(
            "RUV1701 TypeScript plugin host returned invalid JSON: {error}"
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_patterns_match_exact_prefix_and_wildcard() {
        let patterns = vec!["/api/users".to_string(), "/blog/*".to_string()];
        assert!(matches_route_patterns(Some(&patterns), "/api/users"));
        assert!(matches_route_patterns(Some(&patterns), "/blog/hello"));
        assert!(matches_route_patterns(Some(&patterns), "/blog/"));
        assert!(!matches_route_patterns(Some(&patterns), "/api/users/1"));
        assert!(!matches_route_patterns(Some(&patterns), "/about"));

        assert!(matches_route_patterns(
            Some(&["*".to_string()]),
            "/anything"
        ));
        assert!(!matches_route_patterns(Some(&[]), "/anything"));
        assert!(matches_route_patterns(None, "/anything"));
    }

    #[test]
    fn descriptor_decodes_grouped_sockets_and_capabilities() {
        let descriptor: PluginRegistryDescriptor = serde_json::from_value(serde_json::json!({

            "plugins": ["ruvyxa:realtime"],
            "http": { "request": 1, "response": 0, "routes": 1, "requestMatch": ["/events"] },
            "build": { "start": 0, "resolve": 0, "load": 0, "transform": 0, "complete": 1 },
            "dev": { "fileChange": 1 },
            "diagnostics": [],
            "capabilities": [{
                "id": "realtime@1",
                "plugin": "ruvyxa:realtime",
                "path": "/__ruvyxa/realtime",
                "heartbeatMs": 25000,
                "capacity": 256
            }]
        }))
        .unwrap();
        assert_eq!(descriptor.http.routes, 1);
        assert_eq!(descriptor.dev.file_change, 1);
        assert_eq!(
            descriptor.realtime().map(|value| value.path),
            Some("/__ruvyxa/realtime".to_string())
        );
    }

    #[test]
    fn decodes_request_and_response_continuations() {
        let request: PluginHttpRequestResult = serde_json::from_value(serde_json::json!({
            "kind": "request",
            "request": { "method": "GET", "path": "/", "headers": [] }
        }))
        .unwrap();
        assert!(matches!(request, PluginHttpRequestResult::Request { .. }));

        let response: PluginHttpRequestResult = serde_json::from_value(serde_json::json!({
            "kind": "response",
            "response": { "status": 204, "headers": [] }
        }))
        .unwrap();
        assert!(matches!(response, PluginHttpRequestResult::Response { .. }));
    }

    #[tokio::test]
    async fn hanging_hook_times_out_as_a_poisoned_worker_without_retry() {
        let failure = enforce_call_timeout(
            "http.request",
            Duration::from_millis(5),
            std::future::pending(),
        )
        .await
        .unwrap_err();

        match failure {
            CallFailure::WorkerPoisoned(RuvyxaError::Message(message)) => {
                assert!(message.contains("http.request"), "{message}");
                assert!(message.contains("5 ms"), "{message}");
            }
            _ => panic!("a timed-out call must poison its protocol stream"),
        }
    }

    /// A hook that runs user-supplied plugin code must not be replayed once the
    /// request reached the worker: the handler may already have written to a
    /// database or sent a message before the process went away.
    #[test]
    fn only_side_effect_free_hooks_are_retried_after_a_delivered_request_is_lost() {
        assert!(hook_is_idempotent("describe"));
        for hook in ["http.request", "http.response", "dev.fileChange"] {
            assert!(
                !hook_is_idempotent(hook),
                "{hook} runs plugin code and must not be replayed"
            );
        }
    }

    /// A write that never reached the worker cannot have run the hook, so that
    /// case stays retryable for every hook — this is the common
    /// "worker crashed earlier, reconnect and continue" path, and losing it
    /// would turn a recoverable crash into a failed request.
    #[test]
    fn undelivered_requests_stay_retryable_for_every_hook() {
        // `retryable` mirrors the decision in `call_locked`.
        let retryable = |delivered: bool, hook: &str| !delivered || hook_is_idempotent(hook);

        for hook in [
            "http.request",
            "http.response",
            "dev.fileChange",
            "describe",
        ] {
            assert!(
                retryable(false, hook),
                "{hook} must be retried when the request was never delivered"
            );
        }
        assert!(!retryable(true, "http.request"));
        assert!(retryable(true, "describe"));
    }

    #[test]
    fn malformed_runtime_output_poisons_the_protocol_stream() {
        assert!(matches!(
            decode_runtime_output("plugin wrote to stdout"),
            Err(CallFailure::WorkerPoisoned(_))
        ));
    }

    #[tokio::test]
    async fn poisoned_workers_are_replaced_before_the_next_hook() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ruvyxa-plugin-host-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("ruvyxa.config.mjs"),
            r#"
export default {
  plugins: [{
    name: "recovery",
    register({ http }) {
      http.onRequest({
        async handler({ request }) {
          const pathname = new URL(request.url).pathname
          if (pathname === "/hang") await new Promise(() => {})
          if (pathname === "/corrupt") process.stdout.write("protocol-noise\n")
          return request
        },
      })
    },
  }],
}
"#,
        )
        .unwrap();

        let runtime_script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/ruvyxa/runtime/plugin-runtime.mjs");
        let executable = if cfg!(windows) { "node.exe" } else { "node" };
        let mut host = PluginHost::start_pool_with_timeout(
            &root,
            &runtime_script,
            Path::new(executable),
            1,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        host.call_timeout = Duration::from_secs(1);

        let request = |path: &str| PluginHttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            headers: Vec::new(),
            body_base64: None,
        };

        let corrupt = host
            .execute_request(&request("/corrupt"))
            .await
            .unwrap_err();
        assert!(corrupt.to_string().contains("invalid JSON"), "{corrupt}");
        assert!(matches!(
            host.execute_request(&request("/ok-after-corrupt")).await,
            Ok(PluginHttpRequestResult::Request { .. })
        ));

        let timeout = host.execute_request(&request("/hang")).await.unwrap_err();
        assert!(timeout.to_string().contains("timed out"), "{timeout}");
        assert!(matches!(
            host.execute_request(&request("/ok-after-timeout")).await,
            Ok(PluginHttpRequestResult::Request { .. })
        ));

        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn pool_uses_an_idle_worker_instead_of_queueing_behind_the_cursor() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ruvyxa-plugin-pool-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("ruvyxa.config.mjs"),
            r#"
import { writeFileSync } from "node:fs"

export default {
  plugins: [{
    name: "pool-selection",
    register({ http }) {
      http.onRequest({
        async handler({ request, root }) {
          if (new URL(request.url).pathname === "/slow") {
            writeFileSync(root + "/slow-started", "yes")
            await new Promise((resolve) => setTimeout(resolve, 10_000))
          }
          return request
        },
      })
    },
  }],
}
"#,
        )
        .unwrap();

        let runtime_script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/ruvyxa/runtime/plugin-runtime.mjs");
        let executable = if cfg!(windows) { "node.exe" } else { "node" };
        let host = std::sync::Arc::new(
            PluginHost::start_pool_with_timeout(
                &root,
                &runtime_script,
                Path::new(executable),
                2,
                Duration::from_secs(30),
            )
            .await
            .unwrap(),
        );
        assert_eq!(host.pool_size(), 2);

        let request = |path: &str| PluginHttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            headers: Vec::new(),
            body_base64: None,
        };
        let slow_host = std::sync::Arc::clone(&host);
        let slow_request = request("/slow");
        let slow = tokio::spawn(async move { slow_host.execute_request(&slow_request).await });

        for _ in 0..200 {
            if root.join("slow-started").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(root.join("slow-started").exists());

        // Advances the rotating cursor to worker zero again after warming the
        // second process. The next call must scan past busy worker zero.
        host.execute_request(&request("/warm-second-worker"))
            .await
            .unwrap();
        let fast = tokio::time::timeout(
            Duration::from_secs(1),
            host.execute_request(&request("/must-not-queue")),
        )
        .await;
        assert!(
            fast.is_ok(),
            "the idle worker should answer without head-of-line blocking"
        );
        assert!(fast.unwrap().is_ok());

        slow.abort();
        let _ = slow.await;
        drop(host);
        std::fs::remove_dir_all(root).unwrap();
    }
}
