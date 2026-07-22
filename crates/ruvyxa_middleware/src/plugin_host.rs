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
        let spawn = PluginSpawnConfig {
            project_root: project_root.to_path_buf(),
            runtime_script: runtime_script.to_path_buf(),
            executable: executable.to_path_buf(),
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
            Err(CallFailure::WorkerGone(error)) => {
                warn!(
                    target: "ruvyxa::plugin",
                    "TypeScript plugin host stopped responding ({error}); restarting it once"
                );
                replace_worker(&mut worker, &self.spawn)?;
                match call_worker_with_timeout(&mut worker, hook, payload, self.call_timeout).await
                {
                    Ok(value) => Ok(value),
                    Err(CallFailure::Hook(error)) => Err(error),
                    Err(
                        failure @ (CallFailure::WorkerGone(_) | CallFailure::WorkerPoisoned(_)),
                    ) => {
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

fn replace_worker(worker: &mut PluginWorker, spawn: &PluginSpawnConfig) -> Result<()> {
    let _ = worker.child.start_kill();
    *worker = spawn_worker(spawn)?;
    Ok(())
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
    /// The worker may still be alive, but its request/response stream can no
    /// longer be correlated safely. Replaced without retrying the hook.
    WorkerPoisoned(RuvyxaError),
}

impl CallFailure {
    fn into_error(self) -> RuvyxaError {
        match self {
            Self::Hook(error) | Self::WorkerGone(error) | Self::WorkerPoisoned(error) => error,
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

    #[tokio::test]
    async fn hanging_hook_times_out_as_a_poisoned_worker_without_retry() {
        let failure = enforce_call_timeout(
            "middlewareRequest",
            Duration::from_millis(5),
            std::future::pending(),
        )
        .await
        .unwrap_err();

        match failure {
            CallFailure::WorkerPoisoned(RuvyxaError::Message(message)) => {
                assert!(message.contains("middlewareRequest"), "{message}");
                assert!(message.contains("5 ms"), "{message}");
            }
            _ => panic!("a timed-out call must poison its protocol stream"),
        }
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
    setup({ addMiddleware }) {
      addMiddleware({
        async onRequest(request) {
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
            Ok(MiddlewareRequestResult::Request { .. })
        ));

        let timeout = host.execute_request(&request("/hang")).await.unwrap_err();
        assert!(timeout.to_string().contains("timed out"), "{timeout}");
        assert!(matches!(
            host.execute_request(&request("/ok-after-timeout")).await,
            Ok(MiddlewareRequestResult::Request { .. })
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
    setup({ addMiddleware }) {
      addMiddleware({
        async onRequest(request, { root }) {
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
