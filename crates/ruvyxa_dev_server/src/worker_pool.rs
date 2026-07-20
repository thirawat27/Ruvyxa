//! Persistent JavaScript worker pool for eliminating subprocess spawn overhead.
//!
//! Instead of spawning a new JavaScript process for every SSR/API/action/client render,
//! this module maintains a pool of long-lived Node or Bun processes that communicate
//! via newline-delimited JSON over stdin/stdout.
//!
//! Performance impact: eliminates ~100-500ms of per-request overhead from process
//! creation, V8 startup, and renderer initialization.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::task::{Context, Poll};

use axum::body::{Body, Bytes};
use base64::Engine;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, warn};

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use ruvyxa_graph::RouteParams;

use crate::JavaScriptRuntime;

/// Number of worker processes to maintain in the pool.
/// Defaults to the number of available CPU cores (clamped to 2..8) for optimal
/// concurrency without over-subscribing memory.
const DEFAULT_POOL_SIZE: usize = 4;

/// Minimum pool size regardless of configuration.
const MIN_POOL_SIZE: usize = 2;

/// Maximum pool size to prevent excessive memory usage from many Node processes.
const MAX_POOL_SIZE: usize = 8;

const WORKER_TIMEOUT_ENV: &str = "RUVYXA_WORKER_TIMEOUT_MS";
/// Interactive fallback shared by the Rust response receiver and Node watchdog.
const DEFAULT_WORKER_TIMEOUT_MS: u64 = 30_000;
/// Build prerendering can legitimately take longer than an interactive request.
const BUILD_WORKER_TIMEOUT_MS: u64 = 300_000;
/// Node timers coerce larger delays to 1 ms instead of waiting longer.
const MAX_NODE_TIMEOUT_MS: u64 = 2_147_483_647;

/// Maximum time a worker receives to exit after its stdin closes before it is killed.
const WORKER_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Maximum number of decoded response frames waiting for one HTTP consumer.
/// At 64 KiB per frame this bounds queued raw body data to roughly 1 MiB.
/// The bounded channel applies backpressure to the Node worker instead of
/// failing an already-started HTTP response with an incomplete chunked body.
const MAX_PENDING_RESPONSE_FRAMES: usize = 16;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_request_id() -> String {
    REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed).to_string()
}

// --- Public Request/Response Types ---

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerRequest {
    #[serde(rename = "ssr")]
    Ssr {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        params: RouteParams,
    },
    #[serde(rename = "api")]
    Api {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "routeFile")]
        route_file: String,
        method: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        /// Legacy collapsed headers, retained so older worker scripts can still
        /// execute the request. New workers must prefer `headerPairs` below.
        headers: BTreeMap<String, String>,
        /// Ordered request header values. An HTTP header name can occur more
        /// than once, so a map would silently discard values at this boundary.
        #[serde(rename = "headerPairs")]
        header_pairs: Vec<(String, String)>,
        body: Option<String>,
        /// Lossless request body transport for bytes that are not valid UTF-8.
        /// The explicit field name is the NDJSON protocol tag for base64 data.
        #[serde(rename = "bodyBase64", skip_serializing_if = "Option::is_none")]
        body_base64: Option<String>,
        /// Ask workers that support framed responses to stream the API body.
        /// Older workers ignore this additive field and return the legacy body.
        #[serde(rename = "streamResponse")]
        stream_response: bool,
        params: RouteParams,
    },
    #[serde(rename = "action")]
    Action {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "actionFile")]
        action_file: String,
        #[serde(rename = "actionName")]
        action_name: String,
        #[serde(rename = "payloadJson")]
        payload_json: String,
        #[serde(rename = "contentType")]
        content_type: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        /// Ordered request header values so action handlers can observe the
        /// same cookies, authorization, and tracing headers as the endpoint.
        /// This additive field is ignored by older worker scripts.
        #[serde(rename = "headerPairs")]
        header_pairs: Vec<(String, String)>,
    },
    #[serde(rename = "client")]
    Client {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        params: RouteParams,
    },
    #[serde(rename = "invalidate")]
    Invalidate { id: String, paths: Vec<String> },
    #[serde(rename = "ping")]
    Ping { id: String },
    #[serde(rename = "warmup")]
    Warmup {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        routes: Vec<WarmupRoute>,
    },
    /// Pre-render a page (used for ISR background revalidation at runtime).
    #[serde(rename = "ssg")]
    Ssg {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        params: RouteParams,
        /// "full" | "ppr" — controls whether to wait for all content or just the shell.
        mode: String,
        /// Build-only isolation: reload the module without discarding the compiled bundle cache.
        fresh: bool,
    },
    /// Resolve static route parameters during production builds.
    #[serde(rename = "staticParams")]
    StaticParams {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "routePath")]
        route_path: String,
        segments: Vec<StaticParamSegment>,
        routes: Vec<StaticParamsRoute>,
    },
}

impl WorkerRequest {
    fn id(&self) -> &str {
        match self {
            Self::Ssr { id, .. }
            | Self::Api { id, .. }
            | Self::Action { id, .. }
            | Self::Client { id, .. }
            | Self::Invalidate { id, .. }
            | Self::Ping { id, .. }
            | Self::Warmup { id, .. }
            | Self::Ssg { id, .. }
            | Self::StaticParams { id, .. } => id,
        }
    }

    /// Returns `true` if this request type is safe to retry without risk of
    /// duplicate side effects. Actions and API calls are NOT idempotent.
    pub fn is_idempotent(&self) -> bool {
        matches!(
            self,
            Self::Ssr { .. }
                | Self::Ssg { .. }
                | Self::StaticParams { .. }
                | Self::Client { .. }
                | Self::Ping { .. }
                | Self::Warmup { .. }
                | Self::Invalidate { .. }
        )
    }
}

/// A route to pre-warm in the worker's module cache.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WarmupRoute {
    pub page_file: String,
    pub app_dir: String,
}

/// Route metadata passed to build-time parameter discovery.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticParamsRoute {
    pub path: String,
    pub id: String,
}

/// Dynamic segment metadata used to normalize the single-segment shorthand.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticParamSegment {
    pub name: String,
    pub catch_all: bool,
    pub optional: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerResponse {
    pub id: String,
    pub ok: bool,
    /// Framed API response discriminator. Absent for the legacy one-message protocol.
    pub frame: Option<String>,
    pub html: Option<String>,
    pub script: Option<String>,
    pub status: Option<u16>,
    pub headers: Option<BTreeMap<String, String>>,
    /// Ordered response headers. Prefer this over `headers` so repeated
    /// `Set-Cookie` values survive the Node-to-Rust boundary.
    pub header_pairs: Option<Vec<(String, String)>>,
    pub body: Option<String>,
    /// Base64-encoded bytes for an `api-chunk` frame.
    pub body_base64: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub stack: Option<String>,
    pub pong: Option<bool>,
    pub warmed: Option<usize>,
    pub module_cache_size: Option<usize>,
    pub params: Option<Vec<RouteParams>>,
    /// Content hash of the compiled SSG dependency graph.
    pub dependency_hash: Option<String>,
    /// Absolute source files used by the SSG bundle.
    pub inputs: Option<Vec<PathBuf>>,
}

impl WorkerResponse {
    fn is_terminal(&self) -> bool {
        !matches!(self.frame.as_deref(), Some("api-start" | "api-chunk"))
    }

    fn stream_error(id: String, message: impl Into<String>) -> Self {
        Self {
            id,
            frame: Some("api-error".to_string()),
            code: Some("RUV1704".to_string()),
            message: Some(message.into()),
            ..Self::default()
        }
    }
}

#[derive(Clone)]
struct PendingResponse {
    sender: mpsc::Sender<WorkerResponse>,
    queued: Arc<AtomicUsize>,
    /// Set after an API response has started streaming. A worker exit must be
    /// delivered to these consumers as a body error rather than a clean EOF.
    streaming: Arc<AtomicBool>,
}

type PendingResponses = Arc<Mutex<BTreeMap<String, PendingResponse>>>;

struct ResponseChannel {
    id: String,
    receiver: mpsc::Receiver<WorkerResponse>,
    queued: Arc<AtomicUsize>,
    streaming: Arc<AtomicBool>,
}

pub(crate) struct WorkerApiResponse {
    pub response: WorkerResponse,
    pub body: Option<Body>,
}

struct WorkerBodyStream {
    id: String,
    receiver: mpsc::Receiver<WorkerResponse>,
    queued: Arc<AtomicUsize>,
    pending: PendingResponses,
    idle_timeout: std::time::Duration,
    deadline: Pin<Box<tokio::time::Sleep>>,
    finished: bool,
}

impl WorkerBodyStream {
    fn new(
        channel: ResponseChannel,
        pending: PendingResponses,
        idle_timeout: std::time::Duration,
    ) -> Self {
        Self {
            id: channel.id,
            receiver: channel.receiver,
            queued: channel.queued,
            pending,
            idle_timeout,
            deadline: Box::pin(tokio::time::sleep(idle_timeout)),
            finished: false,
        }
    }

    fn remove_pending(&self) {
        let pending = Arc::clone(&self.pending);
        let id = self.id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                pending.lock().await.remove(&id);
            });
        }
    }
}

impl Stream for WorkerBodyStream {
    type Item = std::result::Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(response)) => {
                self.queued.fetch_sub(1, Ordering::AcqRel);
                let idle_timeout = self.idle_timeout;
                self.deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + idle_timeout);

                match response.frame.as_deref() {
                    Some("api-chunk") => {
                        let encoded = response.body_base64.ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "API stream chunk did not include bodyBase64",
                            )
                        });
                        Poll::Ready(Some(encoded.and_then(|encoded| {
                            base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .map(Bytes::from)
                                .map_err(|error| {
                                    io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        format!("API stream chunk was not valid base64: {error}"),
                                    )
                                })
                        })))
                    }
                    Some("api-error") => {
                        self.finished = true;
                        Poll::Ready(Some(Err(io::Error::other(
                            response
                                .message
                                .unwrap_or_else(|| "Node worker API stream failed".to_string()),
                        ))))
                    }
                    Some("api-end") => {
                        self.finished = true;
                        Poll::Ready(None)
                    }
                    frame => {
                        self.finished = true;
                        Poll::Ready(Some(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Unexpected worker API stream frame: {frame:?}"),
                        ))))
                    }
                }
            }
            Poll::Ready(None) => {
                self.finished = true;
                Poll::Ready(Some(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Node worker API stream ended before api-end",
                ))))
            }
            Poll::Pending => {
                if self.deadline.as_mut().poll(cx).is_ready() {
                    self.finished = true;
                    self.remove_pending();
                    Poll::Ready(Some(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "Worker API response stream was idle for {}ms",
                            self.idle_timeout.as_millis()
                        ),
                    ))))
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

impl Drop for WorkerBodyStream {
    fn drop(&mut self) {
        if !self.finished {
            self.remove_pending();
        }
    }
}

// --- Worker Process ---

struct Worker {
    stdin_tx: StdMutex<Option<mpsc::Sender<String>>>,
    pending: PendingResponses,
    child: Mutex<Option<Child>>,
    alive: Arc<AtomicBool>,
}

impl Worker {
    async fn spawn(
        worker_script: &Path,
        env: &BTreeMap<String, String>,
        runtime: JavaScriptRuntime,
    ) -> std::result::Result<Self, RuvyxaError> {
        let mut child = Command::new(runtime.executable())
            .arg(worker_script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env.iter())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| RuvyxaError::Io {
                message: format!("Failed to spawn {} worker process", runtime.command()),
                source,
            })?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let pending: PendingResponses = Arc::new(Mutex::new(BTreeMap::new()));
        let alive = Arc::new(AtomicBool::new(true));

        // Spawn stdin writer task. A broken pipe is a transport failure, not a
        // recoverable queue stall: mark the worker dead and close pending
        // non-stream requests so the pool can replace it immediately.
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(256);
        let writer_pending = pending.clone();
        let writer_alive = alive.clone();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                    writer_alive.store(false, Ordering::Release);
                    let pending = std::mem::take(&mut *writer_pending.lock().await);
                    for (id, pending_response) in pending {
                        if !pending_response.streaming.load(Ordering::Acquire) {
                            // Dropping the sender makes the request receiver
                            // fail immediately instead of waiting for a
                            // response timeout. Stream consumers are handled
                            // by the explicit api-error path below.
                            continue;
                        }
                        let error = WorkerResponse::stream_error(
                            id,
                            "Node worker stdin closed before completing API response stream",
                        );
                        pending_response.queued.fetch_add(1, Ordering::AcqRel);
                        let _ = pending_response.sender.send(error).await;
                    }
                    break;
                }
            }
        });

        // Spawn stderr drain task — prevents the pipe buffer from filling up and
        // blocking the Node process. Logs lines at warn level for visibility.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                warn!(target: "ruvyxa::worker_stderr", "{}", line);
            }
        });

        // Spawn stdout reader task
        let reader_pending = pending.clone();
        let reader_alive = alive.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let response: WorkerResponse = match serde_json::from_str(&line) {
                    Ok(response) => response,
                    Err(error) => {
                        warn!(%error, "worker returned invalid JSON");
                        continue;
                    }
                };
                let id = response.id.clone();
                let terminal = response.is_terminal();
                let pending_response = {
                    let mut map = reader_pending.lock().await;
                    if terminal {
                        map.remove(&id)
                    } else {
                        map.get(&id).cloned()
                    }
                };
                let Some(pending_response) = pending_response else {
                    continue;
                };

                pending_response.queued.fetch_add(1, Ordering::AcqRel);
                if pending_response.sender.send(response).await.is_err() {
                    pending_response.queued.fetch_sub(1, Ordering::AcqRel);
                    reader_pending.lock().await.remove(&id);
                }
            }
            // Requests that have not started streaming still observe their
            // channel closing and let the pool replace the failed worker.
            // Streams must instead receive an explicit error: a clean EOF is
            // only valid after the worker has sent `api-end`.
            reader_alive.store(false, Ordering::Release);
            let pending = std::mem::take(&mut *reader_pending.lock().await);
            for (id, pending_response) in pending {
                if !pending_response.streaming.load(Ordering::Acquire) {
                    continue;
                }

                let error = WorkerResponse::stream_error(
                    id,
                    "Node worker exited before completing API response stream",
                );
                pending_response.queued.fetch_add(1, Ordering::AcqRel);
                let _ = pending_response.sender.send(error).await;
            }
            debug!("worker stdout reader exited");
        });

        Ok(Self {
            stdin_tx: StdMutex::new(Some(stdin_tx)),
            pending,
            child: Mutex::new(Some(child)),
            alive,
        })
    }

    /// Close the worker input, then force-stop it if graceful shutdown takes too long.
    async fn shutdown(&self) {
        self.alive.store(false, Ordering::Release);
        let sender = self.stdin_tx.lock().ok().and_then(|mut guard| guard.take());
        drop(sender);
        self.pending.lock().await.clear();

        let Some(mut child) = self.child.lock().await.take() else {
            return;
        };

        match tokio::time::timeout(WORKER_SHUTDOWN_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => debug!(?status, "Node worker stopped gracefully"),
            Ok(Err(error)) => warn!(%error, "failed while waiting for Node worker shutdown"),
            Err(_) => {
                warn!("Node worker did not stop in time; terminating it");
                if let Err(error) = child.start_kill() {
                    warn!(%error, "failed to terminate Node worker");
                }
                if let Err(error) = child.wait().await {
                    warn!(%error, "failed while waiting for terminated Node worker");
                }
            }
        }
    }

    async fn send(
        &self,
        request: &WorkerRequest,
        response_timeout: std::time::Duration,
    ) -> Result<WorkerResponse> {
        let mut channel = self.open_response(request).await?;

        match tokio::time::timeout(response_timeout, channel.receiver.recv()).await {
            Ok(Some(response)) => {
                channel.queued.fetch_sub(1, Ordering::AcqRel);
                Ok(response)
            }
            Ok(None) => Err(RuvyxaError::Message(
                "Worker response channel closed unexpectedly".to_string(),
            )),
            Err(_) => {
                self.pending.lock().await.remove(&channel.id);
                Err(RuvyxaError::Message(format!(
                    "Worker request timed out after {}ms",
                    response_timeout.as_millis()
                )))
            }
        }
    }

    async fn start_api_response(
        &self,
        request: &WorkerRequest,
        response_timeout: std::time::Duration,
    ) -> Result<WorkerApiResponse> {
        let mut channel = self.open_response(request).await?;
        let response = match tokio::time::timeout(response_timeout, channel.receiver.recv()).await {
            Ok(Some(response)) => {
                channel.queued.fetch_sub(1, Ordering::AcqRel);
                response
            }
            Ok(None) => {
                return Err(RuvyxaError::Message(
                    "Worker response channel closed unexpectedly".to_string(),
                ));
            }
            Err(_) => {
                self.pending.lock().await.remove(&channel.id);
                return Err(RuvyxaError::Message(format!(
                    "Worker request timed out after {}ms",
                    response_timeout.as_millis()
                )));
            }
        };

        match response.frame.as_deref() {
            Some("api-start") => {
                channel.streaming.store(true, Ordering::Release);
                Ok(WorkerApiResponse {
                    response,
                    body: Some(Body::from_stream(WorkerBodyStream::new(
                        channel,
                        Arc::clone(&self.pending),
                        response_timeout,
                    ))),
                })
            }
            None => Ok(WorkerApiResponse {
                response,
                body: None,
            }),
            frame => {
                self.pending.lock().await.remove(&channel.id);
                Err(RuvyxaError::Message(format!(
                    "Worker returned an unexpected first API response frame: {frame:?}"
                )))
            }
        }
    }

    async fn open_response(&self, request: &WorkerRequest) -> Result<ResponseChannel> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(RuvyxaError::Message(
                "Worker process has exited".to_string(),
            ));
        }

        let line = serde_json::to_string(request)
            .map_err(|error| RuvyxaError::Message(error.to_string()))?
            + "\n";
        let stdin_tx = self
            .stdin_tx
            .lock()
            .map_err(|_| RuvyxaError::Message("Worker input lock poisoned".to_string()))?
            .clone()
            .ok_or_else(|| RuvyxaError::Message("Worker process is shutting down".to_string()))?;

        let id = request.id().to_string();
        let (sender, receiver) = mpsc::channel(MAX_PENDING_RESPONSE_FRAMES);
        let queued = Arc::new(AtomicUsize::new(0));
        let streaming = Arc::new(AtomicBool::new(false));
        self.pending.lock().await.insert(
            id.clone(),
            PendingResponse {
                sender,
                queued: Arc::clone(&queued),
                streaming: Arc::clone(&streaming),
            },
        );
        if !self.alive.load(Ordering::Acquire) {
            self.pending.lock().await.remove(&id);
            return Err(RuvyxaError::Message(
                "Worker process has exited".to_string(),
            ));
        }

        if stdin_tx.send(line).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(RuvyxaError::Message(
                "Worker process stdin closed".to_string(),
            ));
        }

        Ok(ResponseChannel {
            id,
            receiver,
            queued,
            streaming,
        })
    }
}

// --- Worker Pool ---

pub struct NodeWorkerPool {
    workers: StdRwLock<Vec<Arc<Worker>>>,
    worker_script: PathBuf,
    env: BTreeMap<String, String>,
    runtime: JavaScriptRuntime,
    next_worker: AtomicU64,
    response_timeout: std::time::Duration,
}

pub(crate) struct RenderApiRequest<'a> {
    pub project_root: &'a Path,
    pub route_file: &'a Path,
    pub method: &'a str,
    pub request_path: &'a str,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
    pub params: &'a RouteParams,
}

pub(crate) struct RenderActionRequest<'a> {
    pub project_root: &'a Path,
    pub action_file: &'a Path,
    pub action_name: &'a str,
    pub payload_json: &'a str,
    pub content_type: &'a str,
    pub request_path: &'a str,
    pub headers: &'a [(String, String)],
}

impl NodeWorkerPool {
    pub async fn start(root: &Path, env: BTreeMap<String, String>) -> Result<Self> {
        Self::start_with_runtime(root, env, JavaScriptRuntime::detect()).await
    }

    pub async fn start_with_runtime(
        root: &Path,
        mut env: BTreeMap<String, String>,
        runtime: JavaScriptRuntime,
    ) -> Result<Self> {
        let response_timeout = configure_worker_timeout(&mut env, DEFAULT_WORKER_TIMEOUT_MS);
        Self::start_with_timeout(root, env, runtime, None, response_timeout).await
    }

    /// Start a pool with an optional bounded worker count.
    ///
    /// Build-time prerendering uses this to avoid starting idle Node processes
    /// beyond its already configured render concurrency.
    pub async fn start_with_size(
        root: &Path,
        env: BTreeMap<String, String>,
        worker_count: Option<usize>,
    ) -> Result<Self> {
        Self::start_with_size_and_runtime(root, env, worker_count, JavaScriptRuntime::detect())
            .await
    }

    pub async fn start_with_size_and_runtime(
        root: &Path,
        mut env: BTreeMap<String, String>,
        worker_count: Option<usize>,
        runtime: JavaScriptRuntime,
    ) -> Result<Self> {
        let response_timeout = configure_worker_timeout(&mut env, BUILD_WORKER_TIMEOUT_MS);
        Self::start_with_timeout(root, env, runtime, worker_count, response_timeout).await
    }

    async fn start_with_timeout(
        root: &Path,
        env: BTreeMap<String, String>,
        runtime: JavaScriptRuntime,
        worker_count: Option<usize>,
        response_timeout: std::time::Duration,
    ) -> Result<Self> {
        let worker_script = find_worker_script(root).ok_or_else(|| {
            Diagnostic::new("RUV1702", "Worker pool script was not found")
                .explain(
                    "Ruvyxa could not find the persistent Node worker script (worker-pool.mjs).",
                )
                .suggest(
                    "Run pnpm install from the monorepo root, or install the ruvyxa package in the app.",
                )
        })?;

        let pool_size = match worker_count {
            // A short-lived build may have one prerender job. Do not start an
            // idle second process solely because the long-lived dev server has
            // a higher minimum concurrency target.
            Some(worker_count) => worker_count.clamp(1, MAX_POOL_SIZE),
            None => std::env::var("RUVYXA_WORKER_POOL_SIZE")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or_else(|| {
                    std::thread::available_parallelism()
                        .map(usize::from)
                        .unwrap_or(DEFAULT_POOL_SIZE)
                })
                .clamp(MIN_POOL_SIZE, MAX_POOL_SIZE),
        };

        // Spawn all worker processes concurrently; each spawn performs blocking
        // process setup, so overlapping them shortens pool startup.
        let mut spawns = tokio::task::JoinSet::new();
        for index in 0..pool_size {
            let worker_script = worker_script.clone();
            let env = env.clone();
            spawns
                .spawn(async move { (index, Worker::spawn(&worker_script, &env, runtime).await) });
        }
        let mut spawned = Vec::with_capacity(pool_size);
        while let Some(joined) = spawns.join_next().await {
            let (index, worker) = joined.map_err(|error| {
                RuvyxaError::Message(format!("worker spawn task panicked: {error}"))
            })?;
            spawned.push((index, worker?));
        }
        spawned.sort_by_key(|(index, _)| *index);
        let workers = spawned
            .into_iter()
            .map(|(_, worker)| Arc::new(worker))
            .collect::<Vec<_>>();

        // Health check: ping first worker to verify it's alive
        let ping = WorkerRequest::Ping {
            id: next_request_id(),
        };
        match workers[0].send(&ping, response_timeout).await {
            Ok(response) if response.ok => {
                debug!(
                    pool_size,
                    runtime = runtime.command(),
                    "JavaScript worker pool started successfully"
                );
            }
            Ok(response) => {
                error!(message = ?response.message, "Worker pool health check failed");
                return Err(RuvyxaError::Message(
                    "Node worker pool health check returned error".to_string(),
                ));
            }
            Err(error) => {
                return Err(RuvyxaError::Message(format!(
                    "Node worker pool health check failed: {error}"
                )));
            }
        }

        Ok(Self {
            workers: StdRwLock::new(workers),
            worker_script,
            env,
            runtime,
            next_worker: AtomicU64::new(0),
            response_timeout,
        })
    }

    /// Stop every owned Node worker before the server releases its process resources.
    pub async fn shutdown(&self) {
        let Ok(workers) = self.workers.read().map(|workers| workers.clone()) else {
            warn!("worker pool lock poisoned during shutdown");
            return;
        };
        for worker in workers {
            worker.shutdown().await;
        }
    }

    /// Send a request to the next available worker (round-robin).
    pub async fn send(&self, request: WorkerRequest) -> Result<WorkerResponse> {
        let (index, worker) = self.select_worker()?;
        let response = worker.send(&request, self.response_timeout).await;

        if response.is_err()
            && let Some(replacement) = self.replace_failed_worker(index, &worker).await
            && request.is_idempotent()
        {
            warn!(
                failed_worker = index,
                "retrying idempotent request on replacement worker"
            );
            return replacement.send(&request, self.response_timeout).await;
        }

        response
    }

    fn select_worker(&self) -> Result<(usize, Arc<Worker>)> {
        let workers = self
            .workers
            .read()
            .map_err(|_| RuvyxaError::Message("Worker pool lock poisoned".to_string()))?;
        if workers.is_empty() {
            return Err(RuvyxaError::Message(
                "Worker pool has no workers".to_string(),
            ));
        }
        let index = self.next_worker.fetch_add(1, Ordering::Relaxed) as usize % workers.len();
        Ok((index, workers[index].clone()))
    }

    /// Replaces a failed worker before the next request can select its slot.
    /// The caller decides whether the failed request is safe to retry.
    async fn replace_failed_worker(
        &self,
        index: usize,
        failed: &Arc<Worker>,
    ) -> Option<Arc<Worker>> {
        let replacement = match Worker::spawn(&self.worker_script, &self.env, self.runtime).await {
            Ok(worker) => Arc::new(worker),
            Err(error) => {
                warn!(%error, failed_worker = index, "failed to replace Node worker");
                return None;
            }
        };

        let active = {
            let Ok(mut workers) = self.workers.write() else {
                warn!(
                    failed_worker = index,
                    "worker pool lock poisoned during replacement"
                );
                return None;
            };
            if workers
                .get(index)
                .is_some_and(|worker| Arc::ptr_eq(worker, failed))
            {
                workers[index] = replacement.clone();
                replacement.clone()
            } else {
                workers.get(index)?.clone()
            }
        };

        if Arc::ptr_eq(&active, &replacement) {
            failed.shutdown().await;
        } else {
            replacement.shutdown().await;
        }

        Some(active)
    }

    /// Invalidate bundle caches in all workers concurrently (called on file change).
    ///
    /// Sends the invalidation request to all workers in parallel rather than
    /// sequentially, reducing latency from `n * RTT` to `max(RTT)`.
    pub async fn invalidate(&self, paths: Vec<String>) {
        let Ok(workers) = self.workers.read().map(|workers| workers.clone()) else {
            warn!("worker pool lock poisoned during invalidation");
            return;
        };
        // Build one request per worker (each needs its own unique id).
        let requests: Vec<WorkerRequest> = (0..workers.len())
            .map(|_| WorkerRequest::Invalidate {
                id: next_request_id(),
                paths: paths.clone(),
            })
            .collect();

        // Send all concurrently — tokio::join! doesn't work for dynamic counts,
        // so we collect futures and poll them all.
        let mut set = tokio::task::JoinSet::new();
        for (i, request) in requests.into_iter().enumerate() {
            let Ok(stdin_tx) = workers[i].stdin_tx.lock().map(|guard| guard.clone()) else {
                warn!(worker = i, "worker stdin lock poisoned during invalidation");
                continue;
            };
            set.spawn(async move {
                let line = serde_json::to_string(&request).unwrap_or_default() + "\n";
                if let Some(stdin_tx) = stdin_tx {
                    let _ = stdin_tx.send(line).await;
                }
            });
        }
        // Wait for all to complete.
        while set.join_next().await.is_some() {}
    }

    /// Queue cache invalidation from a synchronous file-watcher callback.
    ///
    /// `notify` invokes callbacks on its own OS thread, where no Tokio runtime
    /// is installed. `try_send` keeps the callback runtime-independent and
    /// avoids panicking while the async writer tasks flush the messages.
    pub fn invalidate_from_watcher(
        &self,
        paths: Vec<String>,
    ) -> std::result::Result<usize, String> {
        let workers = self
            .workers
            .read()
            .map_err(|_| "worker pool lock poisoned".to_string())?;
        let mut queued = 0;
        for (worker_index, worker) in workers.iter().enumerate() {
            let request = WorkerRequest::Invalidate {
                id: next_request_id(),
                paths: paths.clone(),
            };
            let line = serde_json::to_string(&request)
                .map_err(|error| format!("worker invalidation serialization failed: {error}"))?;
            worker
                .stdin_tx
                .lock()
                .map_err(|_| format!("worker {worker_index} stdin lock poisoned"))?
                .as_ref()
                .ok_or_else(|| format!("worker {worker_index} is shutting down"))?
                .try_send(format!("{line}\n"))
                .map_err(|error| {
                    format!("worker {worker_index} invalidation queue rejected the update: {error}")
                })?;
            queued += 1;
        }
        Ok(queued)
    }

    /// Pre-warm module caches in a worker by importing route bundles during idle time.
    ///
    /// This eliminates the cold-start penalty for the first request to each route.
    /// Warm every worker because Node's ESM cache is process-local.
    pub async fn warmup(&self, project_root: &str, routes: Vec<WarmupRoute>) -> usize {
        let Ok(workers) = self.workers.read().map(|workers| workers.clone()) else {
            warn!("worker pool lock poisoned during warmup");
            return 0;
        };
        if routes.is_empty() || workers.is_empty() {
            return 0;
        }

        let mut pending = tokio::task::JoinSet::new();
        for worker in &workers {
            let worker = worker.clone();
            let project_root = project_root.to_string();
            let routes = routes.clone();
            let response_timeout = self.response_timeout;
            pending.spawn(async move {
                let request = WorkerRequest::Warmup {
                    id: next_request_id(),
                    project_root,
                    routes,
                };
                worker.send(&request, response_timeout).await
            });
        }

        let mut warmed = 0;
        while let Some(result) = pending.join_next().await {
            match result {
                Ok(Ok(response)) if response.ok => {
                    warmed += response.warmed.unwrap_or_default();
                }
                Ok(Ok(response)) => {
                    debug!(message = ?response.message, "worker warmup returned non-ok");
                }
                Ok(Err(_)) | Err(_) => {
                    // Non-fatal: warmup is an optimization, not a requirement.
                }
            }
        }
        debug!(warmed, workers = workers.len(), "worker warmup completed");
        warmed
    }

    // --- Convenience methods for each render type ---

    pub async fn render_ssr(
        &self,
        project_root: &Path,
        app_dir: &Path,
        page_file: &Path,
        request_path: &str,
        params: &RouteParams,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Ssr {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            app_dir: app_dir.display().to_string(),
            page_file: page_file.display().to_string(),
            request_path: request_path.to_string(),
            params: params.clone(),
        };
        self.send(request).await
    }

    pub(crate) async fn render_api(&self, api: RenderApiRequest<'_>) -> Result<WorkerApiResponse> {
        let headers = api.headers.iter().cloned().collect::<BTreeMap<_, _>>();
        let body_base64 = api.body.map(base64_encode);
        let request = WorkerRequest::Api {
            id: next_request_id(),
            project_root: api.project_root.display().to_string(),
            route_file: api.route_file.display().to_string(),
            method: api.method.to_string(),
            request_path: api.request_path.to_string(),
            headers,
            header_pairs: api.headers.to_vec(),
            // Keep the legacy field for text-only workers. Binary data is sent
            // exclusively through the explicitly tagged base64 field.
            body: api
                .body
                .and_then(|body| std::str::from_utf8(body).ok().map(str::to_string)),
            body_base64,
            stream_response: true,
            params: api.params.clone(),
        };
        let (index, worker) = self.select_worker()?;
        let response = worker
            .start_api_response(&request, self.response_timeout)
            .await;
        if response.is_err() {
            self.replace_failed_worker(index, &worker).await;
        }
        response
    }

    pub(crate) async fn render_action(
        &self,
        action: RenderActionRequest<'_>,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Action {
            id: next_request_id(),
            project_root: action.project_root.display().to_string(),
            action_file: action.action_file.display().to_string(),
            action_name: action.action_name.to_string(),
            payload_json: action.payload_json.to_string(),
            content_type: action.content_type.to_string(),
            request_path: action.request_path.to_string(),
            header_pairs: action.headers.to_vec(),
        };
        self.send(request).await
    }

    pub async fn render_client(
        &self,
        project_root: &Path,
        app_dir: &Path,
        page_file: &Path,
        request_path: &str,
        params: &RouteParams,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Client {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            app_dir: app_dir.display().to_string(),
            page_file: page_file.display().to_string(),
            request_path: request_path.to_string(),
            params: params.clone(),
        };
        self.send(request).await
    }

    /// Pre-render a page (SSG/ISR background revalidation).
    pub async fn render_ssg(
        &self,
        project_root: &Path,
        app_dir: &Path,
        page_file: &Path,
        request_path: &str,
        params: &RouteParams,
        mode: &str,
    ) -> Result<WorkerResponse> {
        self.render_ssg_with_fresh(
            project_root,
            app_dir,
            page_file,
            request_path,
            params,
            mode,
            false,
        )
        .await
    }

    /// Pre-render with a fresh module import while keeping compiled bundles cached.
    ///
    /// Production builds historically used one Node process per path. Retaining
    /// import isolation avoids exposing mutable page-module state across paths.
    pub async fn render_ssg_isolated(
        &self,
        project_root: &Path,
        app_dir: &Path,
        page_file: &Path,
        request_path: &str,
        params: &RouteParams,
        mode: &str,
    ) -> Result<WorkerResponse> {
        self.render_ssg_with_fresh(
            project_root,
            app_dir,
            page_file,
            request_path,
            params,
            mode,
            true,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn render_ssg_with_fresh(
        &self,
        project_root: &Path,
        app_dir: &Path,
        page_file: &Path,
        request_path: &str,
        params: &RouteParams,
        mode: &str,
        fresh: bool,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Ssg {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            app_dir: app_dir.display().to_string(),
            page_file: page_file.display().to_string(),
            request_path: request_path.to_string(),
            params: params.clone(),
            mode: mode.to_string(),
            fresh,
        };
        self.send(request).await
    }

    /// Resolve dynamic SSG parameters through the persistent worker cache.
    pub async fn resolve_static_params(
        &self,
        project_root: &Path,
        page_file: &Path,
        route_path: &str,
        segments: &[StaticParamSegment],
        routes: &[StaticParamsRoute],
    ) -> Result<WorkerResponse> {
        self.send(WorkerRequest::StaticParams {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            page_file: page_file.display().to_string(),
            route_path: route_path.to_string(),
            segments: segments.to_vec(),
            routes: routes.to_vec(),
        })
        .await
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn configure_worker_timeout(
    env: &mut BTreeMap<String, String>,
    fallback_ms: u64,
) -> std::time::Duration {
    let inherited = std::env::var(WORKER_TIMEOUT_ENV).ok();
    let configured = env
        .get(WORKER_TIMEOUT_ENV)
        .map(String::as_str)
        .or(inherited.as_deref());
    let timeout_ms = configured
        .and_then(positive_worker_timeout_ms)
        .unwrap_or(fallback_ms);

    // Explicitly pass the normalized value so Node and Rust cannot apply
    // different parsing or fallback behavior to the same worker request.
    env.insert(WORKER_TIMEOUT_ENV.to_string(), timeout_ms.to_string());
    std::time::Duration::from_millis(timeout_ms)
}

fn positive_worker_timeout_ms(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && *value <= MAX_NODE_TIMEOUT_MS)
}

fn find_worker_script(root: &Path) -> Option<PathBuf> {
    let cwd_script = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("packages/ruvyxa/runtime/worker-pool.mjs"));
    if let Some(path) = cwd_script.filter(|p| p.is_file()) {
        return Some(path);
    }

    let package_script = root.join("node_modules/ruvyxa/runtime/worker-pool.mjs");
    if package_script.is_file() {
        return Some(package_script);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_timeout_normalizes_valid_project_configuration() {
        let mut env = BTreeMap::from([(WORKER_TIMEOUT_ENV.to_string(), " 45000 ".to_string())]);

        let timeout = configure_worker_timeout(&mut env, DEFAULT_WORKER_TIMEOUT_MS);

        assert_eq!(timeout, std::time::Duration::from_millis(45_000));
        assert_eq!(env[WORKER_TIMEOUT_ENV], "45000");
    }

    #[test]
    fn worker_timeout_normalizes_invalid_configuration_to_each_mode_fallback() {
        for (configured, fallback_ms) in [
            ("0", DEFAULT_WORKER_TIMEOUT_MS),
            ("invalid", DEFAULT_WORKER_TIMEOUT_MS),
            ("30000ms", DEFAULT_WORKER_TIMEOUT_MS),
            ("2147483648", BUILD_WORKER_TIMEOUT_MS),
        ] {
            let mut env =
                BTreeMap::from([(WORKER_TIMEOUT_ENV.to_string(), configured.to_string())]);

            let timeout = configure_worker_timeout(&mut env, fallback_ms);

            assert_eq!(timeout, std::time::Duration::from_millis(fallback_ms));
            assert_eq!(env[WORKER_TIMEOUT_ENV], fallback_ms.to_string());
        }
    }

    #[test]
    fn api_worker_request_serializes_lossless_body_and_header_pairs() {
        let request = WorkerRequest::Api {
            id: "test".to_string(),
            project_root: "/project".to_string(),
            route_file: "/project/app/api/upload/route.ts".to_string(),
            method: "POST".to_string(),
            request_path: "/api/upload".to_string(),
            headers: BTreeMap::from([("x-repeat".to_string(), "second".to_string())]),
            header_pairs: vec![
                ("x-repeat".to_string(), "first".to_string()),
                ("x-repeat".to_string(), "second".to_string()),
            ],
            body: None,
            body_base64: Some(base64_encode(&[0, 255, 128, 13, 10])),
            stream_response: true,
            params: BTreeMap::new(),
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(
            value["headerPairs"][0],
            serde_json::json!(["x-repeat", "first"])
        );
        assert_eq!(
            value["headerPairs"][1],
            serde_json::json!(["x-repeat", "second"])
        );
        assert_eq!(value["bodyBase64"], "AP+ADQo=");
        assert_eq!(value["streamResponse"], true);
    }

    #[test]
    fn action_worker_request_serializes_lossless_request_header_pairs() {
        let request = WorkerRequest::Action {
            id: "action".to_string(),
            project_root: "/project".to_string(),
            action_file: "/project/app/action.ts".to_string(),
            action_name: "inspect".to_string(),
            payload_json: "{}".to_string(),
            content_type: "application/json".to_string(),
            request_path: "/account".to_string(),
            header_pairs: vec![
                ("authorization".to_string(), "Bearer token".to_string()),
                ("cookie".to_string(), "a=1".to_string()),
                ("cookie".to_string(), "b=2".to_string()),
            ],
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(
            value["headerPairs"][0],
            serde_json::json!(["authorization", "Bearer token"])
        );
        assert_eq!(
            value["headerPairs"][1],
            serde_json::json!(["cookie", "a=1"])
        );
        assert_eq!(
            value["headerPairs"][2],
            serde_json::json!(["cookie", "b=2"])
        );
    }

    #[tokio::test]
    async fn api_body_stream_decodes_binary_frames_without_text_conversion() {
        let (sender, receiver) = mpsc::channel(2);
        let queued = Arc::new(AtomicUsize::new(2));
        sender
            .send(WorkerResponse {
                id: "stream".to_string(),
                ok: true,
                frame: Some("api-chunk".to_string()),
                body_base64: Some("AP+ADQo=".to_string()),
                ..WorkerResponse::default()
            })
            .await
            .unwrap();
        sender
            .send(WorkerResponse {
                id: "stream".to_string(),
                ok: true,
                frame: Some("api-end".to_string()),
                ..WorkerResponse::default()
            })
            .await
            .unwrap();
        drop(sender);

        let body = Body::from_stream(WorkerBodyStream::new(
            ResponseChannel {
                id: "stream".to_string(),
                receiver,
                queued,
                streaming: Arc::new(AtomicBool::new(true)),
            },
            Arc::new(Mutex::new(BTreeMap::new())),
            std::time::Duration::from_secs(1),
        ));
        let bytes = axum::body::to_bytes(body, 1024).await.unwrap();

        assert_eq!(bytes.as_ref(), &[0, 255, 128, 13, 10]);
    }

    #[tokio::test]
    async fn api_body_stream_rejects_eof_before_api_end() {
        let (sender, receiver) = mpsc::channel(1);
        sender
            .send(WorkerResponse {
                id: "stream".to_string(),
                ok: true,
                frame: Some("api-chunk".to_string()),
                body_base64: Some("AA==".to_string()),
                ..WorkerResponse::default()
            })
            .await
            .unwrap();
        drop(sender);

        let body = Body::from_stream(WorkerBodyStream::new(
            ResponseChannel {
                id: "stream".to_string(),
                receiver,
                queued: Arc::new(AtomicUsize::new(1)),
                streaming: Arc::new(AtomicBool::new(true)),
            },
            Arc::new(Mutex::new(BTreeMap::new())),
            std::time::Duration::from_secs(1),
        ));
        let error = axum::body::to_bytes(body, 1024).await.unwrap_err();

        assert!(error.to_string().contains("before api-end"));
    }

    #[tokio::test]
    async fn api_body_stream_propagates_worker_errors() {
        let (sender, receiver) = mpsc::channel(1);
        let queued = Arc::new(AtomicUsize::new(1));
        sender
            .send(WorkerResponse::stream_error(
                "stream".to_string(),
                "route stream failed",
            ))
            .await
            .unwrap();
        drop(sender);

        let body = Body::from_stream(WorkerBodyStream::new(
            ResponseChannel {
                id: "stream".to_string(),
                receiver,
                queued,
                streaming: Arc::new(AtomicBool::new(true)),
            },
            Arc::new(Mutex::new(BTreeMap::new())),
            std::time::Duration::from_secs(1),
        ));
        let error = axum::body::to_bytes(body, 1024).await.unwrap_err();

        assert!(error.to_string().contains("route stream failed"));
    }

    #[tokio::test]
    async fn api_body_stream_times_out_when_worker_stalls() {
        let (_sender, receiver) = mpsc::channel::<WorkerResponse>(1);
        let body = Body::from_stream(WorkerBodyStream::new(
            ResponseChannel {
                id: "stream".to_string(),
                receiver,
                queued: Arc::new(AtomicUsize::new(0)),
                streaming: Arc::new(AtomicBool::new(true)),
            },
            Arc::new(Mutex::new(BTreeMap::new())),
            std::time::Duration::from_millis(20),
        ));
        let error = axum::body::to_bytes(body, 1024).await.unwrap_err();

        assert!(error.to_string().contains("idle for 20ms"));
    }

    #[tokio::test]
    async fn api_response_accepts_legacy_single_message_workers() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "import { createInterface } from 'node:readline'; createInterface({ input: process.stdin }).on('line', (line) => { const { id } = JSON.parse(line); process.stdout.write(JSON.stringify({ id, ok: true, status: 200, body: 'legacy' }) + '\\n'); });",
        )
        .unwrap();
        let worker = Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
            .await
            .unwrap();
        let request = WorkerRequest::Api {
            id: next_request_id(),
            project_root: temp.path().display().to_string(),
            route_file: temp.path().join("route.mjs").display().to_string(),
            method: "GET".to_string(),
            request_path: "/api/legacy".to_string(),
            headers: BTreeMap::new(),
            header_pairs: Vec::new(),
            body: None,
            body_base64: None,
            stream_response: true,
            params: BTreeMap::new(),
        };

        let response = worker
            .start_api_response(&request, std::time::Duration::from_secs(2))
            .await
            .unwrap();

        assert!(response.body.is_none());
        assert_eq!(response.response.body.as_deref(), Some("legacy"));
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn api_response_queue_applies_backpressure_without_truncating_body() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "import { createInterface } from 'node:readline'; createInterface({ input: process.stdin }).on('line', (line) => { const { id } = JSON.parse(line); const write = (value) => process.stdout.write(JSON.stringify({ id, ...value }) + '\\n'); write({ frame: 'api-start', ok: true, status: 200 }); for (let index = 0; index < 17; index++) write({ frame: 'api-chunk', ok: true, bodyBase64: 'AA==' }); write({ frame: 'api-end', ok: true }); });",
        )
        .unwrap();
        let worker = Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
            .await
            .unwrap();
        let request = WorkerRequest::Api {
            id: next_request_id(),
            project_root: temp.path().display().to_string(),
            route_file: temp.path().join("route.mjs").display().to_string(),
            method: "GET".to_string(),
            request_path: "/api/overflow".to_string(),
            headers: BTreeMap::new(),
            header_pairs: Vec::new(),
            body: None,
            body_base64: None,
            stream_response: true,
            params: BTreeMap::new(),
        };
        let response = worker
            .start_api_response(&request, std::time::Duration::from_secs(2))
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let body = axum::body::to_bytes(response.body.unwrap(), 1024)
            .await
            .unwrap();

        assert_eq!(body.len(), 17);
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn api_response_stream_reports_worker_exit_before_api_end() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "import { createInterface } from 'node:readline'; createInterface({ input: process.stdin }).on('line', (line) => { const { id } = JSON.parse(line); const write = (value, done) => process.stdout.write(JSON.stringify({ id, ...value }) + '\\n', done); write({ frame: 'api-start', ok: true, status: 200 }); write({ frame: 'api-chunk', ok: true, bodyBase64: 'AA==' }, () => process.exit(0)); });",
        )
        .unwrap();
        let worker = Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
            .await
            .unwrap();
        let request = WorkerRequest::Api {
            id: next_request_id(),
            project_root: temp.path().display().to_string(),
            route_file: temp.path().join("route.mjs").display().to_string(),
            method: "GET".to_string(),
            request_path: "/api/interrupted".to_string(),
            headers: BTreeMap::new(),
            header_pairs: Vec::new(),
            body: None,
            body_base64: None,
            stream_response: true,
            params: BTreeMap::new(),
        };

        let response = worker
            .start_api_response(&request, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        let error = axum::body::to_bytes(response.body.unwrap(), 1024)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("exited before completing API response stream")
        );
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn pool_shutdown_closes_owned_node_workers() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "process.stdin.on('end', () => process.exit(0)); process.stdin.resume();",
        )
        .unwrap();

        let worker = Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
            .await
            .unwrap();
        let pool = NodeWorkerPool {
            workers: StdRwLock::new(vec![Arc::new(worker)]),
            worker_script,
            env: BTreeMap::new(),
            runtime: JavaScriptRuntime::Node,
            next_worker: AtomicU64::new(0),
            response_timeout: std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
        };

        pool.shutdown().await;

        let worker = pool.workers.read().expect("worker pool lock poisoned")[0].clone();
        assert!(worker.child.lock().await.is_none());
        assert!(
            worker
                .stdin_tx
                .lock()
                .expect("worker stdin mutex poisoned")
                .is_none()
        );
    }

    #[tokio::test]
    async fn worker_exit_closes_pending_requests_promptly() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "process.stdin.once('data', () => process.exit(0)); process.stdin.resume();",
        )
        .unwrap();

        let worker = Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
            .await
            .unwrap();
        let request = WorkerRequest::Ping {
            id: next_request_id(),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            worker.send(
                &request,
                std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            ),
        )
        .await;

        assert!(result.is_ok(), "worker exit left the request pending");
        let error = result.unwrap().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("response channel closed unexpectedly")
        );

        worker.shutdown().await;
    }

    #[tokio::test]
    async fn replaces_a_failed_worker_before_retrying_an_idempotent_request() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "import { createInterface } from 'node:readline'; createInterface({ input: process.stdin }).on('line', (line) => { const { id } = JSON.parse(line); process.stdout.write(JSON.stringify({ id, ok: true, pong: true }) + '\\n'); });",
        )
        .unwrap();

        let failed_worker = Arc::new(
            Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                .await
                .unwrap(),
        );
        let pool = NodeWorkerPool {
            workers: StdRwLock::new(vec![failed_worker.clone()]),
            worker_script,
            env: BTreeMap::new(),
            runtime: JavaScriptRuntime::Node,
            next_worker: AtomicU64::new(0),
            response_timeout: std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
        };

        let mut child = failed_worker.child.lock().await;
        child.as_mut().unwrap().start_kill().unwrap();
        child.as_mut().unwrap().wait().await.unwrap();
        drop(child);

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            pool.send(WorkerRequest::Ping {
                id: next_request_id(),
            }),
        )
        .await
        .expect("worker replacement timed out")
        .expect("worker replacement failed");

        assert!(response.ok);
        assert_eq!(response.pong, Some(true));
        assert!(!Arc::ptr_eq(
            &failed_worker,
            &pool.workers.read().expect("worker pool lock poisoned")[0]
        ));

        pool.shutdown().await;
    }
}
