//! Persistent process bridge for TypeScript plugin middleware.

use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::warn;

use ruvyxa_diagnostics::{Result, RuvyxaError};

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
pub enum MiddlewareRequestResult {
    Request { request: PluginHttpRequest },
    Response { response: PluginHttpResponse },
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MiddlewareHookCounts {
    pub request: usize,
    pub response: usize,
    /// Union of request-middleware route patterns. `None` means every path can
    /// match, either because a middleware declared no routes or because the
    /// runtime predates route reporting.
    #[serde(default)]
    pub request_routes: Option<Vec<String>>,
    /// Union of response-middleware route patterns with the same semantics.
    #[serde(default)]
    pub response_routes: Option<Vec<String>>,
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

/// Hook counts reported after the TypeScript registry has completed setup.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginRegistryDescriptor {
    pub plugins: Vec<String>,
    pub middleware: MiddlewareHookCounts,
    pub resolve_id: usize,
    pub transform: usize,
    pub build_complete: usize,
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

/// Spawn parameters retained so a crashed plugin host can be restarted.
struct PluginSpawnConfig {
    project_root: std::path::PathBuf,
    runtime_script: std::path::PathBuf,
    executable: std::path::PathBuf,
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
        let spawn = PluginSpawnConfig {
            project_root: project_root.to_path_buf(),
            runtime_script: runtime_script.to_path_buf(),
            executable: executable.to_path_buf(),
        };
        let mut worker = spawn_worker(&spawn)?;
        let descriptor = call_worker(&mut worker, "describe", serde_json::json!({}))
            .await
            .map_err(CallFailure::into_error)?;
        let descriptor: PluginRegistryDescriptor =
            serde_json::from_value(descriptor).map_err(|error| {
                RuvyxaError::Message(format!(
                    "RUV1701 TypeScript plugin host returned an invalid registry descriptor: {error}"
                ))
            })?;

        let mut workers = vec![Mutex::new(worker)];
        // Extra workers only pay off for middleware traffic; a registry
        // without middleware never fans out.
        let middleware_hooks = descriptor.middleware.request + descriptor.middleware.response;
        if middleware_hooks > 0 {
            for _ in 1..pool_size.max(1) {
                workers.push(Mutex::new(spawn_worker(&spawn)?));
            }
        }

        Ok(Self {
            workers,
            next_worker: std::sync::atomic::AtomicUsize::new(0),
            descriptor,
            spawn,
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
        let middleware = &self.descriptor.middleware;
        middleware.request > 0
            && matches_route_patterns(middleware.request_routes.as_deref(), pathname)
    }

    /// Whether any response middleware could match this pathname.
    pub fn wants_response(&self, pathname: &str) -> bool {
        let middleware = &self.descriptor.middleware;
        middleware.response > 0
            && matches_route_patterns(middleware.response_routes.as_deref(), pathname)
    }

    pub async fn execute_request(
        &self,
        request: &PluginHttpRequest,
    ) -> Result<MiddlewareRequestResult> {
        let value = self
            .call(
                "middlewareRequest",
                serde_json::json!({ "request": request }),
            )
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
                "middlewareResponse",
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

    async fn call(&self, hook: &str, payload: serde_json::Value) -> Result<serde_json::Value> {
        let index = self
            .next_worker
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.workers.len();
        let mut worker = self.workers[index].lock().await;
        match call_worker(&mut worker, hook, payload.clone()).await {
            Ok(value) => Ok(value),
            Err(CallFailure::Hook(error)) => Err(error),
            Err(CallFailure::WorkerGone(error)) => {
                warn!(
                    target: "ruvyxa::plugin",
                    "TypeScript plugin host stopped responding ({error}); restarting it once"
                );
                *worker = spawn_worker(&self.spawn)?;
                call_worker(&mut worker, hook, payload)
                    .await
                    .map_err(CallFailure::into_error)
            }
        }
    }
}

fn spawn_worker(spawn: &PluginSpawnConfig) -> Result<PluginWorker> {
    let mut child = Command::new(&spawn.executable)
        .arg(&spawn.runtime_script)
        .arg(&spawn.project_root)
        .arg("--persistent")
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
    /// The worker exited or its pipes broke. Eligible for one restart.
    WorkerGone(RuvyxaError),
}

impl CallFailure {
    fn into_error(self) -> RuvyxaError {
        match self {
            Self::Hook(error) | Self::WorkerGone(error) => error,
        }
    }
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
    worker.stdin.write_all(&encoded).await.map_err(|error| {
        CallFailure::WorkerGone(RuvyxaError::Message(format!(
            "Failed to write to TypeScript plugin host: {error}"
        )))
    })?;
    worker.stdin.flush().await.map_err(|error| {
        CallFailure::WorkerGone(RuvyxaError::Message(format!(
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
    let output: RuntimeOutput = serde_json::from_str(line.trim()).map_err(|error| {
        CallFailure::Hook(RuvyxaError::Message(format!(
            "RUV1701 TypeScript plugin host returned invalid JSON: {error}"
        )))
    })?;
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
    fn descriptor_route_unions_are_optional_for_older_runtimes() {
        let counts: MiddlewareHookCounts = serde_json::from_value(serde_json::json!({
            "request": 2,
            "response": 1
        }))
        .unwrap();
        assert_eq!(counts.request_routes, None);
        assert_eq!(counts.response_routes, None);

        let counts: MiddlewareHookCounts = serde_json::from_value(serde_json::json!({
            "request": 1,
            "response": 1,
            "requestRoutes": ["/admin/*"],
            "responseRoutes": null
        }))
        .unwrap();
        assert_eq!(counts.request_routes, Some(vec!["/admin/*".to_string()]));
        assert_eq!(counts.response_routes, None);
    }

    #[test]
    fn decodes_request_and_response_continuations() {
        let request: MiddlewareRequestResult = serde_json::from_value(serde_json::json!({
            "kind": "request",
            "request": { "method": "GET", "path": "/", "headers": [] }
        }))
        .unwrap();
        assert!(matches!(request, MiddlewareRequestResult::Request { .. }));

        let response: MiddlewareRequestResult = serde_json::from_value(serde_json::json!({
            "kind": "response",
            "response": { "status": 204, "headers": [] }
        }))
        .unwrap();
        assert!(matches!(response, MiddlewareRequestResult::Response { .. }));
    }
}
