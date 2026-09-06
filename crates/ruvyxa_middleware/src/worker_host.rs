//! Persistent process bridge to the project worker.
//!
//! `ruvyxa.config.ts` is a TypeScript module. Most of it renders to JSON the
//! CLI reads directly, but some of it is code that only a JavaScript runtime
//! can run: `proxy.handler`, the Markdown pipeline and its unified plugins, the
//! React compiler, and the content engine's derived artifacts. One persistent
//! process — `packages/ruvyxa/runtime/project-worker.mjs` — answers all of
//! them over newline-delimited JSON on stdio, and this module is the other end
//! of that pipe for the native server.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::warn;

use ruvyxa_diagnostics::{Result, RuvyxaError};

use crate::config::DEFAULT_WORKER_CALL_TIMEOUT_MS;

/// HTTP request representation transported losslessly over the worker protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WireRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
}

/// HTTP response representation transported losslessly over the worker protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WireResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
}

/// What `proxy.handler` decided: answer with a response, or continue with a
/// (possibly changed) request.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WireRequestResult {
    Request { request: WireRequest },
    Response { response: WireResponse },
}

/// What the worker can do for this host, reported once at startup.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkerDescriptor {
    /// Whether the config declares a `proxy.handler`.
    #[serde(default)]
    pub proxy: bool,
    /// Whether the config turned the React compiler on.
    #[serde(default)]
    pub react_compiler: bool,
    /// The public paths the content engine answers live, in a stable order.
    #[serde(default)]
    pub content: Vec<String>,
}

impl WorkerDescriptor {
    /// Whether the content engine generates `path`.
    pub fn serves_content(&self, path: &str) -> bool {
        self.content.iter().any(|candidate| candidate == path)
    }
}

/// One content-engine artifact, as the worker derives it live.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContentArtifact {
    pub body: String,
    pub content_type: String,
}

#[derive(Debug, Deserialize)]
struct RuntimeOutput {
    ok: bool,
    result: Option<serde_json::Value>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Spawn parameters retained so a crashed worker can be restarted.
struct SpawnConfig {
    project_root: std::path::PathBuf,
    runtime_script: std::path::PathBuf,
    executable: std::path::PathBuf,
    executable_args: Vec<String>,
}

/// The project worker pool the native server talks to.
///
/// One or more identical worker processes answer calls round-robin. Every
/// worker loads the same compiled config, and module-level state is
/// per-process — which is why the pool defaults to a single worker, and why
/// it only fans out when a `proxy.handler` exists to take the traffic.
pub struct WorkerHost {
    workers: Vec<Mutex<Worker>>,
    next_worker: std::sync::atomic::AtomicUsize,
    descriptor: WorkerDescriptor,
    spawn: SpawnConfig,
    call_timeout: Duration,
}

impl WorkerHost {
    /// Start one worker with the default call timeout.
    pub async fn start(
        project_root: &Path,
        runtime_script: &Path,
        executable: &Path,
    ) -> Result<Self> {
        Self::start_pool(
            project_root,
            runtime_script,
            executable,
            &[],
            1,
            Duration::from_millis(DEFAULT_WORKER_CALL_TIMEOUT_MS),
        )
        .await
    }

    /// Start a pool of identical workers dispatched round-robin, passing
    /// runtime-specific arguments before the script.
    pub async fn start_pool(
        project_root: &Path,
        runtime_script: &Path,
        executable: &Path,
        executable_args: &[&str],
        pool_size: usize,
        call_timeout: Duration,
    ) -> Result<Self> {
        let spawn = SpawnConfig {
            project_root: project_root.to_path_buf(),
            runtime_script: runtime_script.to_path_buf(),
            executable: executable.to_path_buf(),
            executable_args: executable_args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect(),
        };
        let mut worker = spawn_worker(&spawn)?;
        let descriptor =
            call_worker_with_timeout(&mut worker, "describe", serde_json::json!({}), call_timeout)
                .await
                .map_err(CallFailure::into_error)?;
        let descriptor: WorkerDescriptor = serde_json::from_value(descriptor).map_err(|error| {
            RuvyxaError::Message(format!(
                "RUV1701 the project worker returned an invalid descriptor: {error}"
            ))
        })?;

        let mut workers = vec![Mutex::new(worker)];
        // Extra workers only pay off for request traffic, and only
        // `proxy.handler` takes any; a project without one never fans out.
        if descriptor.proxy {
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

    pub fn descriptor(&self) -> &WorkerDescriptor {
        &self.descriptor
    }

    /// Run the project's `proxy.handler` from `ruvyxa.config.ts` on a request.
    ///
    /// The handler answers with a response, or with the request to continue
    /// with. Whether a request reaches here at all is the matcher's decision,
    /// made natively before the process boundary is crossed.
    pub async fn execute_proxy(&self, request: &WireRequest) -> Result<WireRequestResult> {
        let value = self
            .call("proxy", serde_json::json!({ "request": request }))
            .await?;
        serde_json::from_value(value).map_err(|error| {
            RuvyxaError::Message(format!(
                "RUV1701 proxy.handler returned an invalid result: {error}"
            ))
        })
    }

    /// The content engine's live artifact at `path`, or `None`.
    ///
    /// Development only: the build writes the same bytes once, and a
    /// production server serves those from disk.
    pub async fn content_artifact(&self, path: &str) -> Result<Option<ContentArtifact>> {
        let value = self
            .call("content.artifact", serde_json::json!({ "path": path }))
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value).map(Some).map_err(|error| {
            RuvyxaError::Message(format!(
                "RUV1701 the content engine returned an invalid artifact: {error}"
            ))
        })
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
        mut worker: tokio::sync::MutexGuard<'_, Worker>,
        hook: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match call_worker_with_timeout(&mut worker, hook, payload.clone(), self.call_timeout).await
        {
            Ok(value) => Ok(value),
            Err(CallFailure::Hook(error)) => Err(error),
            Err(failure @ (CallFailure::NotDelivered(_) | CallFailure::WorkerGone(_))) => {
                // Either way the worker is unusable and must be replaced. What
                // differs is whether the call may already have run:
                // `NotDelivered` proves it did not, while `WorkerGone` leaves it
                // unknown — the worker can have executed the handler's side
                // effects and died before answering. Replaying an unknown call
                // applies those effects twice, which is why `worker_pool` gates
                // its own retry on `WorkerRequest::is_idempotent`. This bridge
                // draws the same line instead of retrying blindly.
                let delivered = matches!(failure, CallFailure::WorkerGone(_));
                let error = failure.into_error();
                let retryable = !delivered || hook_is_idempotent(hook);
                warn!(
                    target: "ruvyxa::worker",
                    delivered,
                    retryable,
                    "the project worker stopped responding ({error}); restarting it"
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
                    target: "ruvyxa::worker",
                    "the project worker's protocol became unusable ({error}); replacing it without retrying the call"
                );
                replace_worker(&mut worker, &self.spawn)?;
                Err(error)
            }
        }
    }
}

/// Whether replaying `hook` after a worker death is free of duplicate effects.
///
/// `describe` reports the worker's shape and `content.artifact` derives a file
/// from the source tree; neither runs project code with side effects. `proxy`
/// runs `proxy.handler`, which may write to a database, emit a message, or
/// increment a counter, and a worker that died after running it but before
/// answering is indistinguishable from one that died before running it.
fn hook_is_idempotent(hook: &str) -> bool {
    matches!(hook, "describe" | "content.artifact")
}

fn replace_worker(worker: &mut Worker, spawn: &SpawnConfig) -> Result<()> {
    let _ = worker.child.start_kill();
    *worker = spawn_worker(spawn)?;
    Ok(())
}

fn spawn_worker(spawn: &SpawnConfig) -> Result<Worker> {
    let mut child = Command::new(&spawn.executable)
        .args(&spawn.executable_args)
        .arg(&spawn.runtime_script)
        .arg(&spawn.project_root)
        .arg("--persistent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            RuvyxaError::Message(format!("Failed to start the project worker: {error}"))
        })?;
    let stdin = child.stdin.take().ok_or_else(|| {
        RuvyxaError::Message("the project worker's stdin was not available".to_string())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        RuvyxaError::Message("the project worker's stdout was not available".to_string())
    })?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!(target: "ruvyxa::worker", "{line}");
            }
        });
    }
    Ok(Worker {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

/// Whether a failed call left the worker process unusable.
#[derive(Debug)]
enum CallFailure {
    /// The worker is alive; the call itself failed. Never retried.
    Hook(RuvyxaError),
    /// Writing the request failed, so the worker never received it. The call
    /// cannot have run, so replaying it on a fresh worker is always safe.
    NotDelivered(RuvyxaError),
    /// The request was written but no response came back — the worker exited or
    /// its output pipe broke. The call may have run to completion first, so
    /// only a side-effect-free call may be replayed.
    WorkerGone(RuvyxaError),
    /// The worker may still be alive, but its request/response stream can no
    /// longer be correlated safely. Replaced without retrying the call.
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
    worker: &mut Worker,
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
                "RUV1700 project worker call `{hook}` timed out after {} ms",
                call_timeout.as_millis()
            ))))
        })
}

async fn call_worker(
    worker: &mut Worker,
    hook: &str,
    mut payload: serde_json::Value,
) -> std::result::Result<serde_json::Value, CallFailure> {
    payload["hook"] = serde_json::Value::String(hook.to_string());
    let mut encoded = serde_json::to_vec(&payload).map_err(|error| {
        CallFailure::Hook(RuvyxaError::Message(format!(
            "Failed to encode a project worker request: {error}"
        )))
    })?;
    encoded.push(b'\n');
    // A broken pipe here means the request never reached the worker, so the
    // call definitely did not run and is safe to replay.
    worker.stdin.write_all(&encoded).await.map_err(|error| {
        CallFailure::NotDelivered(RuvyxaError::Message(format!(
            "Failed to write to the project worker: {error}"
        )))
    })?;
    worker.stdin.flush().await.map_err(|error| {
        CallFailure::NotDelivered(RuvyxaError::Message(format!(
            "Failed to flush a project worker request: {error}"
        )))
    })?;

    let mut line = String::new();
    let bytes = worker.stdout.read_line(&mut line).await.map_err(|error| {
        CallFailure::WorkerGone(RuvyxaError::Message(format!(
            "Failed to read a project worker response: {error}"
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
            "RUV1700 the project worker exited before responding (status: {status})"
        ))));
    }
    let output = decode_runtime_output(line.trim())?;
    if output.ok {
        return Ok(output.result.unwrap_or(serde_json::Value::Null));
    }
    Err(CallFailure::Hook(RuvyxaError::Message(
        ruvyxa_diagnostics::label_with_code(
            &output.code.unwrap_or_else(|| "RUV1700".to_string()),
            &output
                .message
                .or(output.stack)
                .unwrap_or_else(|| "project worker call failed".to_string()),
        ),
    )))
}

fn decode_runtime_output(line: &str) -> std::result::Result<RuntimeOutput, CallFailure> {
    serde_json::from_str(line).map_err(|error| {
        CallFailure::WorkerPoisoned(RuvyxaError::Message(format!(
            "RUV1701 the project worker returned invalid JSON: {error}"
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_decodes_every_capability_and_defaults_the_rest() {
        let descriptor: WorkerDescriptor = serde_json::from_value(serde_json::json!({
            "proxy": true,
            "reactCompiler": false,
            "content": ["/content.json", "/rss.xml"]
        }))
        .unwrap();
        assert!(descriptor.proxy);
        assert!(descriptor.serves_content("/rss.xml"));
        assert!(!descriptor.serves_content("/rss.xml/"));

        let bare: WorkerDescriptor = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(bare, WorkerDescriptor::default());
    }

    #[test]
    fn decodes_request_and_response_continuations() {
        let request: WireRequestResult = serde_json::from_value(serde_json::json!({
            "kind": "request",
            "request": { "method": "GET", "path": "/", "headers": [] }
        }))
        .unwrap();
        assert!(matches!(request, WireRequestResult::Request { .. }));

        let response: WireRequestResult = serde_json::from_value(serde_json::json!({
            "kind": "response",
            "response": { "status": 204, "headers": [] }
        }))
        .unwrap();
        assert!(matches!(response, WireRequestResult::Response { .. }));
    }

    #[tokio::test]
    async fn hanging_call_times_out_as_a_poisoned_worker_without_retry() {
        let failure =
            enforce_call_timeout("proxy", Duration::from_millis(5), std::future::pending())
                .await
                .unwrap_err();

        match failure {
            CallFailure::WorkerPoisoned(RuvyxaError::Message(message)) => {
                assert!(message.contains("proxy"), "{message}");
                assert!(message.contains("5 ms"), "{message}");
            }
            _ => panic!("a timed-out call must poison its protocol stream"),
        }
    }

    /// A call that runs `proxy.handler` must not be replayed once the request
    /// reached the worker: the handler may already have written to a database
    /// or sent a message before the process went away.
    #[test]
    fn only_side_effect_free_calls_are_retried_after_a_delivered_request_is_lost() {
        assert!(hook_is_idempotent("describe"));
        assert!(hook_is_idempotent("content.artifact"));
        assert!(!hook_is_idempotent("proxy"));
    }

    #[test]
    fn worker_output_is_decoded_or_poisons_the_stream() {
        let ok = decode_runtime_output(r#"{"ok":true,"result":{"a":1}}"#).unwrap();
        assert!(ok.ok);
        assert!(matches!(
            decode_runtime_output("not json"),
            Err(CallFailure::WorkerPoisoned(_))
        ));
    }
}
