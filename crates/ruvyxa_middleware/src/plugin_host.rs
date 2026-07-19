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

/// Persistent TypeScript plugin host shared by the request and response phases.
pub struct PluginHost {
    worker: Mutex<PluginWorker>,
    descriptor: PluginRegistryDescriptor,
}

impl PluginHost {
    /// Start the selected JavaScript runtime and validate the configured registry.
    pub async fn start(
        project_root: &Path,
        runtime_script: &Path,
        executable: &Path,
    ) -> Result<Self> {
        let mut child = Command::new(executable)
            .arg(runtime_script)
            .arg(project_root)
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

        let mut worker = PluginWorker {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let descriptor = call_worker(&mut worker, "describe", serde_json::json!({})).await?;
        let descriptor = serde_json::from_value(descriptor).map_err(|error| {
            RuvyxaError::Message(format!(
                "RUV1701 TypeScript plugin host returned an invalid registry descriptor: {error}"
            ))
        })?;

        Ok(Self {
            worker: Mutex::new(worker),
            descriptor,
        })
    }

    pub fn descriptor(&self) -> &PluginRegistryDescriptor {
        &self.descriptor
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
        let mut worker = self.worker.lock().await;
        call_worker(&mut worker, hook, payload).await
    }
}

async fn call_worker(
    worker: &mut PluginWorker,
    hook: &str,
    mut payload: serde_json::Value,
) -> Result<serde_json::Value> {
    payload["hook"] = serde_json::Value::String(hook.to_string());
    let mut encoded = serde_json::to_vec(&payload).map_err(|error| {
        RuvyxaError::Message(format!(
            "Failed to encode TypeScript plugin request: {error}"
        ))
    })?;
    encoded.push(b'\n');
    worker.stdin.write_all(&encoded).await.map_err(|error| {
        RuvyxaError::Message(format!(
            "Failed to write to TypeScript plugin host: {error}"
        ))
    })?;
    worker.stdin.flush().await.map_err(|error| {
        RuvyxaError::Message(format!(
            "Failed to flush TypeScript plugin request: {error}"
        ))
    })?;

    let mut line = String::new();
    let bytes = worker.stdout.read_line(&mut line).await.map_err(|error| {
        RuvyxaError::Message(format!(
            "Failed to read TypeScript plugin response: {error}"
        ))
    })?;
    if bytes == 0 {
        let status = worker
            .child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(RuvyxaError::Message(format!(
            "RUV1700 TypeScript plugin host exited before responding (status: {status})"
        )));
    }
    let output: RuntimeOutput = serde_json::from_str(line.trim()).map_err(|error| {
        RuvyxaError::Message(format!(
            "RUV1701 TypeScript plugin host returned invalid JSON: {error}"
        ))
    })?;
    if output.ok {
        return Ok(output.result.unwrap_or(serde_json::Value::Null));
    }
    Err(RuvyxaError::Message(format!(
        "{} {}",
        output.code.unwrap_or_else(|| "RUV1700".to_string()),
        output
            .message
            .or(output.stack)
            .unwrap_or_else(|| "TypeScript plugin hook failed".to_string())
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

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
