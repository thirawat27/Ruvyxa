//! Persistent JavaScript worker pool for eliminating subprocess spawn overhead.
//!
//! Instead of spawning a new JavaScript process for every SSR/API/action/client render,
//! this module maintains a pool of long-lived Node, Bun, or Deno processes that communicate
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
use std::sync::{Arc, Mutex as StdMutex, OnceLock, RwLock as StdRwLock};
use std::task::{Context, Poll};

use crate::worker_protocol::{
    StaticParamSegment, StaticParamsRoute, WarmupRoute, WorkerRequest, WorkerResponse,
    base64_encode, next_request_id,
};
use axum::body::{Body, Bytes};
use base64::Engine;
use futures_core::Stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify, mpsc};
use tracing::{debug, error, info, warn};

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

/// Maximum time a retired worker is given to finish the requests it already
/// holds before it is shut down anyway.
///
/// A retired worker is drained rather than killed so an API stream mid-flight
/// still completes, and that wait used to be unbounded: the task held the only
/// `Arc` to the process, so one request that never reached a terminal frame
/// kept a whole Node process — and its module graph — alive for the life of the
/// server. `recycle` runs on every instrumentation change, so those accumulate.
/// The ceiling is generous compared with a request timeout, because exceeding
/// it means something is already wrong and the process has to go regardless.
const WORKER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Isolated prerenders one worker performs before it is retired and replaced.
///
/// A build asks for import isolation per path so page-module state cannot leak
/// between paths, and that isolation is implemented by importing the bundle
/// under a fresh module URL. Node's ESM registry never releases a URL, so each
/// isolated import permanently retains one more module graph — the worker's
/// memory grows linearly with the number of prerendered paths, and no cache
/// eviction inside the worker can reclaim it. Replacing the process is the only
/// operation that frees those graphs, so the pool retires a worker once it has
/// accumulated this many of them. Isolation is unchanged; only the retention is
/// bounded.
const DEFAULT_ISOLATED_RENDERS_PER_WORKER: usize = 32;

/// Distinct ESM module URLs a long-lived dev worker may retain before the pool
/// replaces it. Node cannot unload these graphs, so process replacement is the
/// only operation that keeps HMR memory use bounded.
const DEFAULT_RETAINED_MODULE_URLS_PER_DEV_WORKER: usize = 64;

const ISOLATED_RENDER_RECYCLE_ENV: &str = "RUVYXA_PRERENDER_RECYCLE_AFTER";

/// Maximum number of decoded response frames waiting for one HTTP consumer.
/// At 64 KiB per frame this bounds queued raw body data to roughly 1 MiB.
/// The bounded channel applies backpressure to the Node worker instead of
/// failing an already-started HTTP response with an incomplete chunked body.
const MAX_PENDING_RESPONSE_FRAMES: usize = 16;

#[derive(Clone)]
struct PendingResponse {
    sender: mpsc::Sender<WorkerResponse>,
    /// Set after an API response has started streaming. A worker exit must be
    /// delivered to these consumers as a body error rather than a clean EOF.
    streaming: Arc<AtomicBool>,
}

#[derive(Default)]
struct PendingResponseSet {
    entries: Mutex<BTreeMap<String, PendingResponse>>,
    count: AtomicUsize,
    idle: Notify,
}

impl PendingResponseSet {
    async fn insert(&self, id: String, response: PendingResponse) {
        let mut entries = self.entries.lock().await;
        if entries.insert(id, response).is_none() {
            self.count.fetch_add(1, Ordering::Release);
        }
    }

    async fn remove(&self, id: &str) -> Option<PendingResponse> {
        let mut entries = self.entries.lock().await;
        let removed = entries.remove(id);
        if removed.is_some() {
            let previous = self.count.fetch_sub(1, Ordering::AcqRel);
            debug_assert!(previous > 0, "pending response count underflow");
            if previous == 1 {
                self.idle.notify_waiters();
            }
        }
        removed
    }

    async fn response(&self, id: &str, terminal: bool) -> Option<PendingResponse> {
        if terminal {
            return self.remove(id).await;
        }
        self.entries.lock().await.get(id).cloned()
    }

    async fn take_all(&self) -> BTreeMap<String, PendingResponse> {
        let mut entries = self.entries.lock().await;
        let pending = std::mem::take(&mut *entries);
        self.count.store(0, Ordering::Release);
        self.idle.notify_waiters();
        pending
    }

    async fn clear(&self) {
        let mut entries = self.entries.lock().await;
        entries.clear();
        self.count.store(0, Ordering::Release);
        self.idle.notify_waiters();
    }

    fn len(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    async fn wait_until_idle(&self) {
        loop {
            let idle = self.idle.notified();
            if self.len() == 0 {
                return;
            }
            idle.await;
        }
    }
}

type PendingResponses = Arc<PendingResponseSet>;

struct ResponseChannel {
    id: String,
    receiver: mpsc::Receiver<WorkerResponse>,
    streaming: Arc<AtomicBool>,
}

pub(crate) struct WorkerApiResponse {
    pub response: WorkerResponse,
    pub body: Option<Body>,
    /// The terminal frame's fields, readable once the body has finished.
    ///
    /// A streamed server-components document carries its Flight payload here:
    /// the payload is complete only when the render is, by which time the first
    /// frame is long gone. Empty for every other streamed response.
    pub trailer: WorkerStreamTrailer,
}

/// Shared slot the body stream fills from the frame that ends it.
pub(crate) type WorkerStreamTrailer = Arc<OnceLock<WorkerResponse>>;

struct WorkerBodyStream {
    id: String,
    receiver: mpsc::Receiver<WorkerResponse>,
    pending: PendingResponses,
    idle_timeout: std::time::Duration,
    deadline: Pin<Box<tokio::time::Sleep>>,
    finished: bool,
    trailer: WorkerStreamTrailer,
}

impl WorkerBodyStream {
    fn new(
        channel: ResponseChannel,
        pending: PendingResponses,
        idle_timeout: std::time::Duration,
        trailer: WorkerStreamTrailer,
    ) -> Self {
        Self {
            id: channel.id,
            receiver: channel.receiver,
            pending,
            idle_timeout,
            deadline: Box::pin(tokio::time::sleep(idle_timeout)),
            finished: false,
            trailer,
        }
    }

    fn remove_pending(&self) {
        let pending = Arc::clone(&self.pending);
        let id = self.id.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                pending.remove(&id).await;
            });
        }
    }
}

impl Stream for WorkerBodyStream {
    type Item = std::result::Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(cx) {
            Poll::Ready(Some(response)) => {
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
                        // Before the stream closes, so a consumer that reads the
                        // slot after its last item cannot race the write.
                        let _ = self.trailer.set(response);
                        Poll::Ready(None)
                    }
                    frame => {
                        self.finished = true;
                        // `finished` means "this stream is over", not "the entry
                        // is gone": the stdout reader only removes an entry it
                        // saw a terminal frame for, and `api-start` is not one.
                        // A worker that repeats `api-start` mid-stream ends up
                        // here, and without this the entry outlives every
                        // consumer — `in_flight` never returns to zero, so the
                        // worker is permanently avoided by `select_worker` and
                        // retiring it waits out `WORKER_DRAIN_TIMEOUT`.
                        // Removing an entry the reader already took is a no-op.
                        self.remove_pending();
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

/// Log one line of worker stderr at the severity the worker asked for.
///
/// stdout carries the NDJSON response protocol, so stderr is the worker's only
/// other channel home and routine lifecycle notices share it with real
/// failures. This side cannot tell them apart from the text, and treating the
/// whole channel as warnings turned an ordinary end-of-build shutdown into one
/// warning per worker on every build.
///
/// The worker tags the lines it knows are routine; anything untagged — a Node
/// stack trace, an unhandled rejection, a bare `console.error` — stays a
/// warning, so the noisy default only applies where nobody has classified the
/// line. An unrecognized tag is logged whole rather than unwrapped: better a
/// loud odd-looking line than a silently downgraded failure.
fn log_worker_stderr(line: &str) {
    match parse_worker_stderr_tag(line) {
        Some(("debug", message)) => debug!(target: "ruvyxa::worker_stderr", "{message}"),
        Some(("info", message)) => info!(target: "ruvyxa::worker_stderr", "{message}"),
        Some(("warn", message)) => warn!(target: "ruvyxa::worker_stderr", "{message}"),
        Some(("error", message)) => error!(target: "ruvyxa::worker_stderr", "{message}"),
        _ => warn!(target: "ruvyxa::worker_stderr", "{line}"),
    }
}

/// Split a `[ruvyxa:<level>] <message>` line into its level and message.
///
/// Emitted by `note()` in `packages/ruvyxa/runtime/worker-pool.mjs`; keep the
/// two in step.
fn parse_worker_stderr_tag(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("[ruvyxa:")?;
    let (level, message) = rest.split_once(']')?;
    Some((level, message.strip_prefix(' ').unwrap_or(message)))
}

struct Worker {
    stdin_tx: StdMutex<Option<mpsc::Sender<String>>>,
    pending: PendingResponses,
    child: Mutex<Option<Child>>,
    alive: Arc<AtomicBool>,
    /// Latest known number of module URLs this process has retained. Older
    /// worker scripts do not report telemetry, so isolated prerenders use this
    /// as a compatibility counter too.
    retained_module_urls: AtomicUsize,
}

impl Worker {
    async fn spawn(
        worker_script: &Path,
        env: &BTreeMap<String, String>,
        runtime: JavaScriptRuntime,
    ) -> std::result::Result<Self, RuvyxaError> {
        let mut child = Command::new(runtime.executable())
            .args(runtime.script_args())
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

        let pending: PendingResponses = Arc::new(PendingResponseSet::default());
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
                    let pending = writer_pending.take_all().await;
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
                        let _ = pending_response.sender.send(error).await;
                    }
                    break;
                }
            }
        });

        // Spawn stderr drain task — prevents the pipe buffer from filling up and
        // blocking the Node process. Severity comes from the worker (see
        // `log_worker_stderr`); untagged lines stay warnings.
        tokio::spawn(async move {
            let limit = max_worker_line_bytes();
            let mut reader = BufReader::new(stderr);
            let mut buffer = Vec::new();
            loop {
                match read_line_bounded(&mut reader, &mut buffer, limit).await {
                    Ok(LineRead::Line) => log_worker_stderr(&String::from_utf8_lossy(&buffer)),
                    Ok(LineRead::Eof { had_data }) => {
                        if had_data {
                            log_worker_stderr(&String::from_utf8_lossy(&buffer));
                        }
                        break;
                    }
                    Ok(LineRead::TooLong) => {
                        warn!(
                            limit,
                            "worker stderr line exceeded {MAX_WORKER_LINE_BYTES_ENV}; \
                             dropping the rest of the diagnostic stream"
                        );
                        break;
                    }
                    Err(_) => break,
                }
            }
        });

        // Spawn stdout reader task
        let reader_pending = pending.clone();
        let reader_alive = alive.clone();
        tokio::spawn(async move {
            let limit = max_worker_line_bytes();
            let mut reader = BufReader::new(stdout);
            let mut buffer = Vec::new();
            loop {
                match read_line_bounded(&mut reader, &mut buffer, limit).await {
                    Ok(LineRead::Line) => {}
                    // A partial trailing line is not a response: the worker died
                    // mid-write, and the teardown below is what the waiters need.
                    Ok(LineRead::Eof { .. }) | Err(_) => break,
                    Ok(LineRead::TooLong) => {
                        // The reader is parked mid-line, so the NDJSON framing is
                        // lost and nothing after this point can be trusted. Fall
                        // through to teardown: the pool replaces this worker and
                        // its waiters get an error instead of the server dying
                        // somewhere unrelated with an allocation failure.
                        error!(
                            limit,
                            "worker response exceeded {MAX_WORKER_LINE_BYTES_ENV}; \
                             replacing the worker"
                        );
                        break;
                    }
                }
                let response: WorkerResponse = match serde_json::from_slice(&buffer) {
                    Ok(response) => response,
                    Err(error) => {
                        warn!(%error, "worker returned invalid JSON");
                        continue;
                    }
                };
                let id = response.id.clone();
                let terminal = response.is_terminal();
                let starts_stream = response.frame.as_deref() == Some("api-start");
                let pending_response = reader_pending.response(&id, terminal).await;
                let Some(pending_response) = pending_response else {
                    continue;
                };

                // A request becomes a stream the moment `api-start` is read,
                // not when the HTTP consumer observes it. Marking it here keeps
                // the transport-failure paths below correct even when the
                // worker exits before the consumer task is scheduled.
                if starts_stream {
                    pending_response.streaming.store(true, Ordering::Release);
                }

                if pending_response.sender.send(response).await.is_err() {
                    reader_pending.remove(&id).await;
                }
            }
            // Requests that have not started streaming still observe their
            // channel closing and let the pool replace the failed worker.
            // Streams must instead receive an explicit error: a clean EOF is
            // only valid after the worker has sent `api-end`.
            reader_alive.store(false, Ordering::Release);
            let pending = reader_pending.take_all().await;
            for (id, pending_response) in pending {
                if !pending_response.streaming.load(Ordering::Acquire) {
                    continue;
                }

                let error = WorkerResponse::stream_error(
                    id,
                    "Node worker exited before completing API response stream",
                );
                let _ = pending_response.sender.send(error).await;
            }
            debug!("worker stdout reader exited");
        });

        Ok(Self {
            stdin_tx: StdMutex::new(Some(stdin_tx)),
            pending,
            child: Mutex::new(Some(child)),
            alive,
            retained_module_urls: AtomicUsize::new(0),
        })
    }

    /// Close the worker input, then force-stop it if graceful shutdown takes too long.
    /// Number of requests registered but not yet delivered a terminal frame.
    ///
    /// The pending map is the exact in-flight set: the stdout reader removes an
    /// entry the moment its terminal response arrives (and stream bodies remove
    /// theirs on `api-end`/drop), so its length is the worker's live load. The
    /// pool uses this to route new work to the least-busy worker.
    fn in_flight(&self) -> usize {
        self.pending.len()
    }

    // Twenty-four lines with one `match` and two `if let`s, which Clippy scores at
    // 43. The count is inflated by the `warn!`/`debug!` expansions in the timeout
    // arm, not by branching a reader has to hold: splitting this would hide the
    // graceful-then-kill sequence behind a call for no gain.
    #[allow(clippy::cognitive_complexity)]
    async fn shutdown(&self) {
        self.alive.store(false, Ordering::Release);
        let sender = self.stdin_tx.lock().ok().and_then(|mut guard| guard.take());
        drop(sender);
        self.pending.clear().await;

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

        let received = tokio::time::timeout(response_timeout, channel.receiver.recv()).await;
        // Unconditionally, on every path. This request has exactly one frame,
        // so the entry is the stdout reader's to remove when that frame is
        // terminal — and nobody's when it is not. A non-terminal frame here
        // would leave an entry whose receiver has just been dropped, which
        // `wait_until_idle` can never observe as idle: retiring that worker
        // would then hang until `WORKER_DRAIN_TIMEOUT`. Removing an entry the
        // reader already took is a no-op, so this costs one lock.
        self.pending.remove(&channel.id).await;

        match received {
            Ok(Some(response)) => Ok(response),
            Ok(None) => Err(RuvyxaError::Message(
                "Worker response channel closed unexpectedly".to_string(),
            )),
            Err(_) => Err(RuvyxaError::Message(format!(
                "Worker request timed out after {}ms",
                response_timeout.as_millis()
            ))),
        }
    }

    async fn start_api_response(
        &self,
        request: &WorkerRequest,
        response_timeout: std::time::Duration,
    ) -> Result<WorkerApiResponse> {
        let mut channel = self.open_response(request).await?;
        let response = match tokio::time::timeout(response_timeout, channel.receiver.recv()).await {
            Ok(Some(response)) => response,
            Ok(None) => {
                return Err(RuvyxaError::Message(
                    "Worker response channel closed unexpectedly".to_string(),
                ));
            }
            Err(_) => {
                self.pending.remove(&channel.id).await;
                return Err(RuvyxaError::Message(format!(
                    "Worker request timed out after {}ms",
                    response_timeout.as_millis()
                )));
            }
        };

        match response.frame.as_deref() {
            Some("api-start") => {
                // The stdout reader already flagged this request as streaming;
                // repeat it here so the flag holds for any caller that builds a
                // body stream without going through that reader.
                channel.streaming.store(true, Ordering::Release);
                let trailer: WorkerStreamTrailer = Arc::new(OnceLock::new());
                Ok(WorkerApiResponse {
                    response,
                    body: Some(Body::from_stream(WorkerBodyStream::new(
                        channel,
                        Arc::clone(&self.pending),
                        response_timeout,
                        Arc::clone(&trailer),
                    ))),
                    trailer,
                })
            }
            None => Ok(WorkerApiResponse {
                response,
                body: None,
                trailer: Arc::new(OnceLock::new()),
            }),
            frame => {
                self.pending.remove(&channel.id).await;
                Err(RuvyxaError::Message(format!(
                    "Worker returned an unexpected first API response frame: {frame:?}"
                )))
            }
        }
    }

    /// Queue a bundle-cache invalidation without awaiting the worker's reply.
    ///
    /// Callable from a non-Tokio thread: the bounded `try_send` never blocks
    /// and never panics outside a runtime, so the file watcher can drive it
    /// directly. Errors describe this worker alone, letting the pool keep
    /// invalidating its siblings.
    fn try_queue_invalidation(
        &self,
        paths: &[String],
        trace_id: Option<&str>,
    ) -> std::result::Result<(), String> {
        let request = WorkerRequest::Invalidate {
            id: next_request_id(),
            paths: paths.to_vec(),
            trace_id: trace_id.map(str::to_string),
        };
        let line = serde_json::to_string(&request)
            .map_err(|error| format!("invalidation serialization failed: {error}"))?;
        self.stdin_tx
            .lock()
            .map_err(|_| "stdin lock poisoned".to_string())?
            .as_ref()
            .ok_or_else(|| "worker is shutting down".to_string())?
            .try_send(format!("{line}\n"))
            .map_err(|error| format!("invalidation queue rejected the update: {error}"))
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
        let streaming = Arc::new(AtomicBool::new(false));
        self.pending
            .insert(
                id.clone(),
                PendingResponse {
                    sender,
                    streaming: Arc::clone(&streaming),
                },
            )
            .await;
        if !self.alive.load(Ordering::Acquire) {
            self.pending.remove(&id).await;
            return Err(RuvyxaError::Message(
                "Worker process has exited".to_string(),
            ));
        }

        if stdin_tx.send(line).await.is_err() {
            self.pending.remove(&id).await;
            return Err(RuvyxaError::Message(
                "Worker process stdin closed".to_string(),
            ));
        }

        Ok(ResponseChannel {
            id,
            receiver,
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
    /// Module URLs a worker may retain before it is retired. `None` disables
    /// recycling. Current workers report the actual ESM registry count; the
    /// isolated-render counter remains as compatibility for older workers.
    retained_module_urls_per_worker: Option<usize>,
    /// Workers taken out of selection that are still finishing admitted work.
    ///
    /// A retired process is no longer in `workers`, so `shutdown` could not see
    /// it. Both retirement paths are ordinary traffic rather than edge cases:
    /// `ruvyxa build` retires a worker every `RUVYXA_PRERENDER_RECYCLE_AFTER`
    /// isolated renders, and `recycle` retires the whole generation at once
    /// whenever instrumentation changes. Each left a live child the pool no
    /// longer owned, and a CLI that exited before the drain task finished never
    /// dropped that `Child` — so `kill_on_drop` never ran and the `node`
    /// process was orphaned, still holding its handles on the build directory.
    retiring: Arc<StdMutex<Vec<Arc<Worker>>>>,
}

/// One server-side page render.
///
/// Grouped into a struct rather than passed as seven positional arguments
/// because `request_path`, `route_path`, and `method` are all `&str` and
/// transposing two of them compiles.
pub struct RenderSsrRequest<'a> {
    pub project_root: &'a Path,
    pub app_dir: &'a Path,
    pub page_file: &'a Path,
    pub request_path: &'a str,
    /// Original path and query. Kept separate so query data reaches the
    /// request context without changing route or render-path semantics.
    pub request_target: &'a str,
    /// Route pattern, not the concrete URL.
    pub route_path: &'a str,
    pub params: &'a RouteParams,
    /// Ordered request headers, for `cookies()` and `headers()`.
    pub headers: &'a [(String, String)],
    pub method: &'a str,
    /// Whether this route opted into the React Server Components pipeline.
    pub server_components: bool,
    /// A `<form action={fn}>` this request is the submission of.
    pub form_action: Option<PostedForm<'a>>,
}

/// A form submitted to a page by a browser that is running no JavaScript.
///
/// The hydrated page posts to `/__ruvyxa/rsc` and patches itself from the
/// reply. A page whose bundle has not loaded — or never will — submits the form
/// the way HTML always has: to the URL it is on, with the reference id in a
/// hidden field React wrote while rendering it.
#[derive(Debug, Clone, Copy)]
pub struct PostedForm<'a> {
    pub content_type: &'a str,
    pub body: &'a [u8],
}

/// One pre-render, whether at build time or on an ISR revalidation.
///
/// A struct rather than eight positional arguments: the two `&str` route
/// fields next to each other are `requestPath` and `routePath`, and swapping
/// them compiles and renders the wrong URL.
pub struct RenderSsgRequest<'a> {
    pub project_root: &'a Path,
    pub app_dir: &'a Path,
    pub page_file: &'a Path,
    pub request_path: &'a str,
    /// Route pattern, not the concrete URL.
    pub route_path: &'a str,
    pub params: &'a RouteParams,
    /// `"full"` or `"ppr"` — whether to wait for all content or just the shell.
    pub mode: &'a str,
    /// Whether this route opted into the React Server Components pipeline.
    pub server_components: bool,
}

/// One public Flight render, bound to the client artifact requesting it.
pub(crate) struct RenderFlightRequest<'a> {
    pub project_root: &'a Path,
    pub app_dir: &'a Path,
    pub page_file: &'a Path,
    pub request_path: &'a str,
    pub route_path: &'a str,
    pub params: &'a RouteParams,
    pub artifact_version: &'a str,
}

pub(crate) struct RenderApiRequest<'a> {
    pub project_root: &'a Path,
    pub route_file: &'a Path,
    pub method: &'a str,
    pub request_path: &'a str,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
    pub params: &'a RouteParams,
    pub known_inputs_version: Option<&'a str>,
}

pub(crate) struct RenderActionRequest<'a> {
    pub project_root: &'a Path,
    pub action_file: &'a Path,
    pub action_name: &'a str,
    pub payload_json: &'a str,
    pub content_type: &'a str,
    pub request_path: &'a str,
    pub headers: &'a [(String, String)],
    pub known_inputs_version: Option<&'a str>,
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
        // Normal HMR rebuilds import changed bundles under content-addressed
        // URLs that Node cannot unload. Bound those graphs in long-lived dev
        // sessions by recycling only after the worker reports the real count.
        Self::start_with_timeout(
            root,
            env,
            runtime,
            None,
            response_timeout,
            Some(DEFAULT_RETAINED_MODULE_URLS_PER_DEV_WORKER),
        )
        .await
    }

    /// Start a pool with an optional bounded worker count.
    ///
    /// Build-time prerendering uses this to avoid starting idle Node processes
    /// beyond its already configured render concurrency. This is also the only
    /// caller that asks for isolated imports, so it is the pool that needs the
    /// worker-recycling bound.
    pub async fn start_with_size_and_runtime(
        root: &Path,
        mut env: BTreeMap<String, String>,
        worker_count: Option<usize>,
        runtime: JavaScriptRuntime,
    ) -> Result<Self> {
        let response_timeout = configure_worker_timeout(&mut env, BUILD_WORKER_TIMEOUT_MS);
        let recycle_after = isolated_renders_per_worker();
        Self::start_with_timeout(
            root,
            env,
            runtime,
            worker_count,
            response_timeout,
            recycle_after,
        )
        .await
    }

    async fn start_with_timeout(
        root: &Path,
        env: BTreeMap<String, String>,
        runtime: JavaScriptRuntime,
        worker_count: Option<usize>,
        response_timeout: std::time::Duration,
        retained_module_urls_per_worker: Option<usize>,
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
            retained_module_urls_per_worker,
            retiring: Arc::new(StdMutex::new(Vec::new())),
        })
    }

    /// Stop every owned Node worker before the server releases its process resources.
    ///
    /// "Owned" includes the workers already taken out of selection: see
    /// [`NodeWorkerPool::retiring`] for why leaving those to their drain tasks
    /// orphaned them. Shutting a draining worker down here also unblocks its
    /// task, because closing the worker clears its pending set.
    /// How many worker processes are currently in selection.
    ///
    /// Exposed so a caller with a batch of independent requests can size its own
    /// concurrency to the pool instead of guessing. Sending more than this does
    /// not go faster — the extra requests queue on a worker — and it does raise
    /// how much compilation output is alive at once.
    #[must_use]
    pub fn size(&self) -> usize {
        self.workers
            .read()
            .map(|workers| workers.len())
            .unwrap_or(1)
            .max(1)
    }

    pub async fn shutdown(&self) {
        let live = match self.workers.read() {
            Ok(workers) => workers.clone(),
            Err(_) => {
                warn!("worker pool lock poisoned during shutdown");
                Vec::new()
            }
        };
        let retiring = match self.retiring.lock() {
            Ok(mut retiring) => std::mem::take(&mut *retiring),
            Err(_) => {
                warn!("retiring worker list poisoned during shutdown");
                Vec::new()
            }
        };

        // Concurrently. Each worker closes its stdin and then waits up to
        // `WORKER_SHUTDOWN_TIMEOUT` for the process to exit; one at a time that
        // is `2s × pool size` between Ctrl-C and the terminal coming back, and
        // the waits are independent.
        let mut stopping = tokio::task::JoinSet::new();
        for worker in live.into_iter().chain(retiring) {
            stopping.spawn(async move { worker.shutdown().await });
        }
        while stopping.join_next().await.is_some() {}
    }

    /// Retire a worker the pool has already replaced, keeping it owned.
    ///
    /// The worker is registered before the drain task starts and deregistered
    /// after it finishes, so `shutdown` sees exactly the processes that are
    /// still alive.
    fn retire_in_background(&self, worker: Arc<Worker>, index: usize) {
        let register = Arc::clone(&self.retiring);
        match register.lock() {
            Ok(mut retiring) => retiring.push(Arc::clone(&worker)),
            Err(_) => warn!(
                worker = index,
                "retiring worker list poisoned; the drain task still owns this process"
            ),
        }
        tokio::spawn(async move {
            drain_then_shutdown(Arc::clone(&worker), index).await;
            if let Ok(mut retiring) = register.lock() {
                retiring.retain(|retired| !Arc::ptr_eq(retired, &worker));
            }
        });
    }

    /// Send a request to the least-loaded worker.
    pub async fn send(&self, request: WorkerRequest) -> Result<WorkerResponse> {
        let (index, worker) = self.select_worker().await?;
        let isolated = request.retains_an_isolated_module_graph();
        let mut active_worker = Arc::clone(&worker);
        let mut response = worker.send(&request, self.response_timeout).await;

        if response.is_err()
            && let Some(replacement) = self.replace_failed_worker(index, &worker).await
            && request.is_idempotent()
        {
            warn!(
                failed_worker = index,
                "retrying idempotent request on replacement worker"
            );
            active_worker = replacement;
            response = active_worker.send(&request, self.response_timeout).await;
        }

        if let Ok(worker_response) = &response {
            self.retire_worker_if_saturated(
                index,
                &active_worker,
                worker_response.retained_module_urls,
                isolated,
            )
            .await;
        }

        response
    }

    /// Replace a worker that has retained its budgeted number of module graphs.
    /// The saturated process is removed from selection immediately, but its
    /// child stays alive until every already-admitted request has completed.
    async fn retire_worker_if_saturated(
        &self,
        index: usize,
        worker: &Arc<Worker>,
        observed_retained_module_urls: Option<usize>,
        isolated: bool,
    ) {
        let Some(budget) = self.retained_module_urls_per_worker else {
            return;
        };
        let retained = match observed_retained_module_urls {
            Some(retained) => {
                worker
                    .retained_module_urls
                    .store(retained, Ordering::Release);
                retained
            }
            None if isolated => worker.retained_module_urls.fetch_add(1, Ordering::AcqRel) + 1,
            None => return,
        };
        if retained < budget {
            return;
        }

        debug!(
            worker = index,
            retained,
            budget,
            in_flight = worker.in_flight(),
            "retiring worker to release retained module graphs"
        );
        if !self.replace_saturated_worker(index, worker).await {
            // The replacement could not start. Keeping the saturated worker is
            // strictly better than losing pool capacity mid-build, so reset the
            // counter and let it try again after another full budget.
            worker.retained_module_urls.store(0, Ordering::Release);
            warn!(
                worker = index,
                "could not replace a saturated worker; continuing with it"
            );
        }
    }

    /// Remove a saturated process from selection immediately, then let every
    /// request already using it finish before closing its stdin. This makes
    /// recycling safe for framed API streams and concurrent SSR renders: new
    /// work goes to the replacement while the old process drains in place.
    async fn replace_saturated_worker(&self, index: usize, saturated: &Arc<Worker>) -> bool {
        let replacement = match Worker::spawn(&self.worker_script, &self.env, self.runtime).await {
            Ok(worker) => Arc::new(worker),
            Err(error) => {
                warn!(%error, worker = index, "failed to replace saturated Node worker");
                return false;
            }
        };

        let replaced = {
            let Ok(mut workers) = self.workers.write() else {
                warn!(
                    worker = index,
                    "worker pool lock poisoned during retirement"
                );
                replacement.shutdown().await;
                return false;
            };
            if workers
                .get(index)
                .is_some_and(|worker| Arc::ptr_eq(worker, saturated))
            {
                workers[index] = Arc::clone(&replacement);
                true
            } else {
                false
            }
        };

        if !replaced {
            replacement.shutdown().await;
            return true;
        }

        self.retire_in_background(Arc::clone(saturated), index);
        true
    }

    /// Replace the complete worker generation after process-wide startup code changes.
    ///
    /// Instrumentation installs exporters and global hooks once per process. Re-importing
    /// it in a live worker would duplicate that state, while ordinary bundle invalidation
    /// cannot undo registrations from a deleted file. Build every replacement first, swap
    /// the pool atomically, then let requests already admitted by the old generation drain.
    pub(crate) async fn recycle(&self) -> Result<usize> {
        let worker_count = self
            .workers
            .read()
            .map_err(|_| RuvyxaError::Message("Worker pool lock poisoned".to_string()))?
            .len();
        let mut replacements = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            match Worker::spawn(&self.worker_script, &self.env, self.runtime).await {
                Ok(worker) => replacements.push(Arc::new(worker)),
                Err(error) => {
                    for replacement in replacements {
                        replacement.shutdown().await;
                    }
                    return Err(RuvyxaError::Message(format!(
                        "Failed to replace Node worker {worker_index} after instrumentation change: {error}"
                    )));
                }
            }
        }

        let swapped = match self.workers.write() {
            Ok(mut workers) if workers.len() == replacements.len() => {
                Ok(std::mem::replace(&mut *workers, replacements))
            }
            Ok(_) => Err((
                "Worker pool size changed during recycle".to_string(),
                replacements,
            )),
            Err(_) => Err(("Worker pool lock poisoned".to_string(), replacements)),
        };
        let old_workers = match swapped {
            Ok(workers) => workers,
            Err((message, replacements)) => {
                for replacement in replacements {
                    replacement.shutdown().await;
                }
                return Err(RuvyxaError::Message(message));
            }
        };

        for (index, worker) in old_workers.into_iter().enumerate() {
            self.retire_in_background(worker, index);
        }
        Ok(worker_count)
    }

    /// Pick the worker with the fewest in-flight requests.
    ///
    /// Blind round-robin ignores load, so a burst can stack several requests
    /// behind one worker still blocked in a CPU-bound `renderToString` while a
    /// sibling worker sits idle. Selecting the least-loaded worker keeps a slow
    /// render from serializing unrelated requests. A rotating start offset
    /// breaks ties fairly (and preserves round-robin behavior when every worker
    /// is equally idle), and an idle worker short-circuits the scan.
    async fn select_worker(&self) -> Result<(usize, Arc<Worker>)> {
        // Clone the Arcs out of the sync lock so worker replacement never races
        // with this selection snapshot.
        let workers = {
            let guard = self
                .workers
                .read()
                .map_err(|_| RuvyxaError::Message("Worker pool lock poisoned".to_string()))?;
            if guard.is_empty() {
                return Err(RuvyxaError::Message(
                    "Worker pool has no workers".to_string(),
                ));
            }
            guard.clone()
        };

        let len = workers.len();
        let start = self.next_worker.fetch_add(1, Ordering::Relaxed) as usize;
        let mut best_index = start % len;
        let mut best_load = usize::MAX;
        for offset in 0..len {
            let index = (start + offset) % len;
            let load = workers[index].in_flight();
            if load == 0 {
                // An idle worker is optimal; stop probing the rest.
                return Ok((index, workers[index].clone()));
            }
            if load < best_load {
                best_load = load;
                best_index = index;
            }
        }
        Ok((best_index, workers[best_index].clone()))
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

        // `replacement` is a live child process from here on, so no path may
        // leave this function without either installing it or shutting it down.
        // Deciding that under the lock and acting on it after the guard drops
        // keeps every exit through the single cleanup below — an earlier
        // version returned straight out of this block on a poisoned lock and on
        // a missing slot, orphaning the process it had just spawned.
        let active = match self.workers.write() {
            Ok(mut workers) => {
                if workers
                    .get(index)
                    .is_some_and(|worker| Arc::ptr_eq(worker, failed))
                {
                    workers[index] = replacement.clone();
                    Some(replacement.clone())
                } else {
                    // A slot that no longer holds `failed` was already replaced
                    // by someone else; `None` means the pool shrank out from
                    // under this index.
                    workers.get(index).cloned()
                }
            }
            Err(_) => {
                warn!(
                    failed_worker = index,
                    "worker pool lock poisoned during replacement"
                );
                None
            }
        };

        match &active {
            // The replacement took the slot, so the process it displaced goes.
            Some(worker) if Arc::ptr_eq(worker, &replacement) => failed.shutdown().await,
            // Either another worker already holds the slot or the pool could
            // not be read. Both leave the replacement unused, so it must not
            // outlive this call.
            _ => replacement.shutdown().await,
        }

        active
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
                trace_id: None,
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
    ///
    /// Every worker is attempted even when an earlier one fails. Node's ESM
    /// cache is process-local, so each worker holds its own compiled bundles:
    /// stopping at the first failure would leave the remaining workers serving
    /// stale code that a browser reload cannot clear. Failures are collected
    /// and reported together so the caller still learns the update was partial.
    pub fn invalidate_from_watcher(
        &self,
        paths: Vec<String>,
        trace_id: Option<&str>,
    ) -> std::result::Result<usize, String> {
        let workers = self
            .workers
            .read()
            .map_err(|_| "worker pool lock poisoned".to_string())?;
        let mut queued = 0;
        let mut failures = Vec::new();
        for (worker_index, worker) in workers.iter().enumerate() {
            match worker.try_queue_invalidation(&paths, trace_id) {
                Ok(()) => queued += 1,
                Err(error) => failures.push(format!("worker {worker_index}: {error}")),
            }
        }

        if failures.is_empty() {
            Ok(queued)
        } else {
            Err(format!(
                "invalidated {queued}/{} workers ({})",
                workers.len(),
                failures.join("; ")
            ))
        }
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
        for (index, worker) in workers.iter().enumerate() {
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
                let response = worker.send(&request, response_timeout).await;
                (index, worker, response)
            });
        }

        let mut warmed = 0;
        while let Some(result) = pending.join_next().await {
            match result {
                Ok((index, worker, Ok(response))) if response.ok => {
                    warmed += response.warmed.unwrap_or_default();
                    self.retire_worker_if_saturated(
                        index,
                        &worker,
                        response.retained_module_urls,
                        false,
                    )
                    .await;
                }
                Ok((index, worker, Ok(response))) => {
                    debug!(message = ?response.message, "worker warmup returned non-ok");
                    self.retire_worker_if_saturated(
                        index,
                        &worker,
                        response.retained_module_urls,
                        false,
                    )
                    .await;
                }
                Ok((_, _, Err(_))) | Err(_) => {
                    // Non-fatal: warmup is an optimization, not a requirement.
                }
            }
        }
        debug!(warmed, workers = workers.len(), "worker warmup completed");
        warmed
    }

    // --- Convenience methods for each render type ---

    pub async fn render_ssr(&self, page: RenderSsrRequest<'_>) -> Result<WorkerResponse> {
        let request = WorkerRequest::Ssr {
            id: next_request_id(),
            project_root: page.project_root.display().to_string(),
            app_dir: page.app_dir.display().to_string(),
            page_file: page.page_file.display().to_string(),
            request_path: page.request_path.to_string(),
            request_target: page.request_target.to_string(),
            route_path: page.route_path.to_string(),
            params: page.params.clone(),
            header_pairs: page.headers.to_vec(),
            method: page.method.to_ascii_uppercase(),
            server_components: page.server_components,
            form_content_type: page.form_action.map(|form| form.content_type.to_string()),
            form_body: page.form_action.map(|form| base64_encode(form.body)),
        };
        self.send(request).await
    }

    pub(crate) async fn render_flight(
        &self,
        page: RenderFlightRequest<'_>,
    ) -> Result<WorkerResponse> {
        self.send(WorkerRequest::Flight {
            id: next_request_id(),
            project_root: page.project_root.display().to_string(),
            app_dir: page.app_dir.display().to_string(),
            page_file: page.page_file.display().to_string(),
            request_path: page.request_path.to_string(),
            route_path: page.route_path.to_string(),
            params: page.params.clone(),
            artifact_version: page.artifact_version.to_string(),
        })
        .await
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
            known_inputs_version: api.known_inputs_version.map(str::to_string),
        };
        let (index, worker) = self.select_worker().await?;
        let response = worker
            .start_api_response(&request, self.response_timeout)
            .await;
        if response.is_err() {
            self.replace_failed_worker(index, &worker).await;
        } else if let Ok(api_response) = &response {
            // Importing the route module is complete before `api-start`, so its
            // retention telemetry is final for this request. Remove a saturated
            // worker from selection now; the retirement path keeps it alive
            // until the framed body reaches api-end or is dropped.
            self.retire_worker_if_saturated(
                index,
                &worker,
                api_response.response.retained_module_urls,
                false,
            )
            .await;
        }
        response
    }

    /// Render a server-components document as a stream.
    ///
    /// Framed like an API response because it is the same shape: a body the
    /// host forwards as it arrives rather than a value it waits for. The Flight
    /// payload comes back in the trailer, after the last chunk, because it is
    /// only complete once the render is.
    pub(crate) async fn render_rsc_document(
        &self,
        page: RenderSsrRequest<'_>,
    ) -> Result<WorkerApiResponse> {
        let request = WorkerRequest::RscDocument {
            id: next_request_id(),
            project_root: page.project_root.display().to_string(),
            app_dir: page.app_dir.display().to_string(),
            page_file: page.page_file.display().to_string(),
            request_path: page.request_path.to_string(),
            request_target: page.request_target.to_string(),
            route_path: page.route_path.to_string(),
            params: page.params.clone(),
            header_pairs: page.headers.to_vec(),
            method: page.method.to_ascii_uppercase(),
            form_content_type: page.form_action.map(|form| form.content_type.to_string()),
            form_body: page.form_action.map(|form| base64_encode(form.body)),
        };
        let (index, worker) = self.select_worker().await?;
        let response = worker
            .start_api_response(&request, self.response_timeout)
            .await;
        if response.is_err() {
            self.replace_failed_worker(index, &worker).await;
        } else if let Ok(streamed) = &response {
            self.retire_worker_if_saturated(
                index,
                &worker,
                streamed.response.retained_module_urls,
                false,
            )
            .await;
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
            known_inputs_version: action.known_inputs_version.map(str::to_string),
        };
        self.send(request).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn render_client(
        &self,
        project_root: &Path,
        app_dir: &Path,
        page_file: &Path,
        request_path: &str,
        route_path: &str,
        params: &RouteParams,
        server_components: bool,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Client {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            app_dir: app_dir.display().to_string(),
            page_file: page_file.display().to_string(),
            request_path: request_path.to_string(),
            route_path: route_path.to_string(),
            params: params.clone(),
            server_components,
        };
        self.send(request).await
    }

    /// Compile one shared browser module by the name its URL carries.
    pub(crate) async fn render_client_vendor(
        &self,
        project_root: &Path,
        name: &str,
    ) -> Result<WorkerResponse> {
        self.send(WorkerRequest::ClientVendor {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            name: name.to_string(),
        })
        .await
    }

    /// Render a server-components route's Flight payload for a soft navigation.
    pub(crate) async fn render_rsc_payload(
        &self,
        page: RenderSsrRequest<'_>,
    ) -> Result<WorkerResponse> {
        self.send(WorkerRequest::RscPayload {
            id: next_request_id(),
            project_root: page.project_root.display().to_string(),
            app_dir: page.app_dir.display().to_string(),
            page_file: page.page_file.display().to_string(),
            request_path: page.request_path.to_string(),
            request_target: page.request_target.to_string(),
            route_path: page.route_path.to_string(),
            params: page.params.clone(),
            header_pairs: page.headers.to_vec(),
            method: page.method.to_ascii_uppercase(),
        })
        .await
    }

    /// Run one of a server-components route's server functions.
    ///
    /// `body` is handed over base64-encoded because React's encoder produces
    /// UTF-8 text for plain arguments and multipart bytes when one of them is a
    /// file, and the worker protocol is line-delimited JSON.
    pub(crate) async fn render_rsc_action(
        &self,
        page: RenderSsrRequest<'_>,
        reference: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<WorkerResponse> {
        self.send(WorkerRequest::RscAction {
            id: next_request_id(),
            project_root: page.project_root.display().to_string(),
            app_dir: page.app_dir.display().to_string(),
            page_file: page.page_file.display().to_string(),
            request_path: page.request_path.to_string(),
            request_target: page.request_target.to_string(),
            route_path: page.route_path.to_string(),
            params: page.params.clone(),
            header_pairs: page.headers.to_vec(),
            method: page.method.to_ascii_uppercase(),
            reference: reference.to_string(),
            content_type: content_type.to_string(),
            body: base64_encode(body),
        })
        .await
    }

    /// Ask a worker for a server-components route's browser entry source.
    pub async fn rsc_client_entry(
        &self,
        project_root: &Path,
        app_dir: &Path,
        page_file: &Path,
        route_path: &str,
    ) -> Result<WorkerResponse> {
        self.send(WorkerRequest::RscClientEntry {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            app_dir: app_dir.display().to_string(),
            page_file: page_file.display().to_string(),
            route_path: route_path.to_string(),
        })
        .await
    }

    /// Pre-render a page (SSG/ISR background revalidation).
    pub async fn render_ssg(&self, page: RenderSsgRequest<'_>) -> Result<WorkerResponse> {
        self.render_ssg_with_fresh(page, false).await
    }

    /// Pre-render with a fresh module import while keeping compiled bundles cached.
    ///
    /// Production builds historically used one Node process per path. Retaining
    /// import isolation avoids exposing mutable page-module state across paths.
    pub async fn render_ssg_isolated(&self, page: RenderSsgRequest<'_>) -> Result<WorkerResponse> {
        self.render_ssg_with_fresh(page, true).await
    }

    async fn render_ssg_with_fresh(
        &self,
        page: RenderSsgRequest<'_>,
        fresh: bool,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Ssg {
            id: next_request_id(),
            project_root: page.project_root.display().to_string(),
            app_dir: page.app_dir.display().to_string(),
            page_file: page.page_file.display().to_string(),
            request_path: page.request_path.to_string(),
            route_path: page.route_path.to_string(),
            params: page.params.clone(),
            mode: page.mode.to_string(),
            fresh,
            server_components: page.server_components,
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

/// Let a retired worker finish what it holds, then stop it either way.
///
/// The wait is bounded because this task owns the last `Arc` to the process: a
/// request that never reaches a terminal frame used to keep the whole Node
/// process alive for the life of the server, and `recycle` retires a full
/// generation every time instrumentation changes. Past the ceiling the process
/// is shut down with work still in flight, which is the lesser failure — those
/// requests were already past any timeout a client would wait through.
async fn drain_then_shutdown(worker: Arc<Worker>, index: usize) {
    if tokio::time::timeout(WORKER_DRAIN_TIMEOUT, worker.pending.wait_until_idle())
        .await
        .is_err()
    {
        warn!(
            worker = index,
            pending = worker.pending.len(),
            timeout_secs = WORKER_DRAIN_TIMEOUT.as_secs(),
            "retired Node worker did not drain; stopping it with requests in flight"
        );
    }
    worker.shutdown().await;
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

/// How many isolated prerenders a build worker may serve before retirement.
///
/// `0` disables recycling for builds that would rather trade memory for the
/// absence of process churn.
fn isolated_renders_per_worker() -> Option<usize> {
    let configured = std::env::var(ISOLATED_RENDER_RECYCLE_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok());
    match configured {
        Some(0) => None,
        Some(budget) => Some(budget),
        None => Some(DEFAULT_ISOLATED_RENDERS_PER_WORKER),
    }
}

fn positive_worker_timeout_ms(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && *value <= MAX_NODE_TIMEOUT_MS)
}

/// Largest NDJSON line this process will accumulate from a worker.
///
/// `AsyncBufReadExt::lines` grows without bound: it returns only when it finds a
/// newline, so a worker that emits an enormous line — a runaway render, a
/// corrupted pipe, a response that echoed its own input — is allocated in full
/// on this side before anything gets to reject it. The failure mode is the
/// server running out of memory, which reports nothing useful about the worker
/// that caused it.
///
/// 64 MiB is far above any real response. A rendered page is measured in
/// hundreds of kilobytes and JSON escaping at worst doubles it, so this is a
/// backstop against a broken worker rather than a limit a working one can meet.
const DEFAULT_MAX_WORKER_LINE_BYTES: usize = 64 * 1024 * 1024;

const MAX_WORKER_LINE_BYTES_ENV: &str = "RUVYXA_WORKER_MAX_LINE_BYTES";

fn max_worker_line_bytes() -> usize {
    std::env::var(MAX_WORKER_LINE_BYTES_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_WORKER_LINE_BYTES)
}

/// Why a bounded read stopped.
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// A complete line, newline consumed and not included.
    Line,
    /// The stream ended. `had_data` distinguishes a trailing unterminated line
    /// from a clean EOF.
    Eof { had_data: bool },
    /// The line exceeded the limit. The reader is left mid-line: the stream is
    /// no longer interpretable, so the caller must stop rather than resynchronize.
    TooLong,
}

/// Read one newline-delimited line into `buffer`, refusing to grow past `limit`.
///
/// `buffer` is cleared first and reused across calls so a steady stream of
/// responses does not reallocate per line.
async fn read_line_bounded<R>(
    reader: &mut BufReader<R>,
    buffer: &mut Vec<u8>,
    limit: usize,
) -> io::Result<LineRead>
where
    R: tokio::io::AsyncRead + Unpin,
{
    buffer.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(LineRead::Eof {
                had_data: !buffer.is_empty(),
            });
        }

        if let Some(index) = available.iter().position(|byte| *byte == b'\n') {
            if buffer.len() + index > limit {
                return Ok(LineRead::TooLong);
            }
            buffer.extend_from_slice(&available[..index]);
            reader.consume(index + 1);
            return Ok(LineRead::Line);
        }

        let consumed = available.len();
        if buffer.len() + consumed > limit {
            return Ok(LineRead::TooLong);
        }
        buffer.extend_from_slice(available);
        reader.consume(consumed);
    }
}

fn find_worker_script(root: &Path) -> Option<PathBuf> {
    // Shared with every other runtime script so a project that resolves its
    // renderers cannot fail to resolve the worker that runs them.
    crate::find_runtime_script(root, "worker-pool.mjs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_protocol::request_id_of;

    /// `lines()` grows until it finds a newline, so a worker emitting one huge
    /// line was allocated in full before anything could reject it — the server
    /// died of an allocation failure rather than reporting a broken worker.
    #[tokio::test]
    async fn a_bounded_read_refuses_to_grow_past_its_limit() {
        let payload = format!("{}\n", "x".repeat(1024));
        let mut reader = BufReader::new(payload.as_bytes());
        let mut buffer = Vec::new();

        assert_eq!(
            read_line_bounded(&mut reader, &mut buffer, 64)
                .await
                .unwrap(),
            LineRead::TooLong
        );
        assert!(
            buffer.len() <= 64 + 1,
            "an over-limit line must not be accumulated: {} bytes",
            buffer.len()
        );
    }

    /// The limit is a backstop, not a ceiling real traffic meets: a line that
    /// fits must come back whole, and the reader must stay framed for the next.
    #[tokio::test]
    async fn bounded_reads_return_whole_lines_and_stay_framed() {
        let mut reader = BufReader::new(&b"first\n\nsecond\ntrailing"[..]);
        let mut buffer = Vec::new();

        assert_eq!(
            read_line_bounded(&mut reader, &mut buffer, 1024)
                .await
                .unwrap(),
            LineRead::Line
        );
        assert_eq!(buffer, b"first");

        assert_eq!(
            read_line_bounded(&mut reader, &mut buffer, 1024)
                .await
                .unwrap(),
            LineRead::Line
        );
        assert!(buffer.is_empty(), "an empty line is a line, not an EOF");

        assert_eq!(
            read_line_bounded(&mut reader, &mut buffer, 1024)
                .await
                .unwrap(),
            LineRead::Line
        );
        assert_eq!(buffer, b"second");

        assert_eq!(
            read_line_bounded(&mut reader, &mut buffer, 1024)
                .await
                .unwrap(),
            LineRead::Eof { had_data: true },
            "a final line with no newline is still data the worker sent"
        );
        assert_eq!(buffer, b"trailing");

        assert_eq!(
            read_line_bounded(&mut reader, &mut buffer, 1024)
                .await
                .unwrap(),
            LineRead::Eof { had_data: false }
        );
    }

    /// A JSON response exactly at the limit still parses; the guard must not
    /// shave a byte off what a working worker is allowed to send.
    #[tokio::test]
    async fn a_line_at_the_limit_is_accepted() {
        let line = "{\"id\":\"a\",\"ok\":true}";
        let payload = format!("{line}\n");
        let mut reader = BufReader::new(payload.as_bytes());
        let mut buffer = Vec::new();

        assert_eq!(
            read_line_bounded(&mut reader, &mut buffer, line.len())
                .await
                .unwrap(),
            LineRead::Line
        );
        assert!(serde_json::from_slice::<WorkerResponse>(&buffer).is_ok());
    }

    #[test]
    fn worker_stderr_severity_comes_from_the_tag() {
        assert_eq!(
            parse_worker_stderr_tag("[ruvyxa:debug] worker shutting down (stdin-close)"),
            Some(("debug", "worker shutting down (stdin-close)"))
        );
        assert_eq!(
            parse_worker_stderr_tag("[ruvyxa:error] render failed"),
            Some(("error", "render failed"))
        );
    }

    #[test]
    fn untagged_and_malformed_worker_stderr_stays_a_warning() {
        // A thrown stack trace has no tag and must not be quieted.
        assert_eq!(
            parse_worker_stderr_tag("TypeError: x is not a function"),
            None
        );
        assert_eq!(parse_worker_stderr_tag("[ruvyxa] not a level"), None);
        // A tag this side does not know falls through to the `_` arm, which
        // logs the whole line rather than unwrapping it at an assumed level.
        assert_eq!(
            parse_worker_stderr_tag("[ruvyxa:trace] noisy"),
            Some(("trace", "noisy"))
        );
        assert!(!matches!(
            parse_worker_stderr_tag("[ruvyxa:trace] noisy"),
            Some(("debug" | "info" | "warn" | "error", _))
        ));
    }

    /// Build a worker backed by a plain channel instead of a real process, so
    /// queueing behavior can be asserted without spawning Node.
    fn stub_worker(stdin_tx: Option<mpsc::Sender<String>>) -> Arc<Worker> {
        Arc::new(Worker {
            stdin_tx: StdMutex::new(stdin_tx),
            pending: Arc::new(PendingResponseSet::default()),
            child: Mutex::new(None),
            alive: Arc::new(AtomicBool::new(true)),
            retained_module_urls: AtomicUsize::new(0),
        })
    }

    /// A pool wrapped around workers a test already built.
    ///
    /// This literal was written out ten times, differing only in the four
    /// arguments below. Every field added to `NodeWorkerPool` had to be copied
    /// into all ten, and the tenth was always the one that was missed.
    fn pool_over(
        workers: Vec<Arc<Worker>>,
        worker_script: PathBuf,
        response_timeout: std::time::Duration,
        retained_module_urls_per_worker: Option<usize>,
    ) -> NodeWorkerPool {
        NodeWorkerPool {
            workers: StdRwLock::new(workers),
            worker_script,
            env: BTreeMap::new(),
            runtime: JavaScriptRuntime::Node,
            next_worker: AtomicU64::new(0),
            response_timeout,
            retained_module_urls_per_worker,
            retiring: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    fn stub_pool(workers: Vec<Arc<Worker>>) -> NodeWorkerPool {
        pool_over(
            workers,
            PathBuf::from("worker-pool.mjs"),
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            None,
        )
    }

    fn ssg_request(fresh: bool) -> WorkerRequest {
        WorkerRequest::Ssg {
            id: next_request_id(),
            project_root: "/project".to_string(),
            app_dir: "/project/app".to_string(),
            page_file: "/project/app/blog/[slug]/page.tsx".to_string(),
            request_path: "/blog/hello".to_string(),
            route_path: "/blog/[slug]".to_string(),
            params: BTreeMap::new(),
            mode: "full".to_string(),
            fresh,
            server_components: false,
        }
    }

    /// Only the isolated form mints a new module URL, so only it should count
    /// against a worker's retention budget.
    #[test]
    fn only_isolated_prerenders_count_against_the_retention_budget() {
        assert!(ssg_request(true).retains_an_isolated_module_graph());
        assert!(!ssg_request(false).retains_an_isolated_module_graph());
        assert!(
            !WorkerRequest::Ping {
                id: next_request_id()
            }
            .retains_an_isolated_module_graph()
        );
    }

    /// `0` disables recycling; anything else is a real bound. Without a bound a
    /// large static site keeps accumulating module graphs until the worker runs
    /// out of heap.
    #[test]
    fn recycle_budget_treats_zero_as_disabled() {
        // `isolated_renders_per_worker` reads a process-wide environment
        // variable, so assert the mapping it applies rather than mutating the
        // environment underneath other tests.
        let interpret = |configured: Option<usize>| match configured {
            Some(0) => None,
            Some(budget) => Some(budget),
            None => Some(DEFAULT_ISOLATED_RENDERS_PER_WORKER),
        };

        assert_eq!(interpret(Some(0)), None, "0 must disable recycling");
        assert_eq!(interpret(Some(4)), Some(4));
        assert_eq!(
            interpret(None),
            Some(DEFAULT_ISOLATED_RENDERS_PER_WORKER),
            "an unset variable must fall back to the documented default"
        );
        assert_eq!(isolated_renders_per_worker().is_none(), {
            let configured = std::env::var(ISOLATED_RENDER_RECYCLE_ENV)
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok());
            configured == Some(0)
        });
    }

    /// A saturated worker leaves the selection pool immediately but is not
    /// stopped until its existing requests drain.
    #[tokio::test]
    async fn a_saturated_worker_drains_in_flight_work_before_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "process.stdin.on('end', () => process.exit(0)); process.stdin.resume();",
        )
        .unwrap();
        let worker = Arc::new(
            Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                .await
                .unwrap(),
        );
        let mut pool = stub_pool(vec![Arc::clone(&worker)]);
        pool.worker_script = worker_script;
        pool.retained_module_urls_per_worker = Some(2);

        // Register an in-flight sibling request on the same worker.
        let (sender, _receiver) = mpsc::channel(1);
        worker
            .pending
            .insert(
                "sibling".to_string(),
                PendingResponse {
                    sender,
                    streaming: Arc::new(AtomicBool::new(false)),
                },
            )
            .await;
        assert_eq!(worker.in_flight(), 1);

        pool.retire_worker_if_saturated(0, &worker, Some(2), false)
            .await;

        assert!(
            !Arc::ptr_eq(&pool.workers.read().unwrap()[0], &worker),
            "new requests must select the replacement"
        );
        assert!(
            worker.child.lock().await.is_some(),
            "the saturated process must stay alive while a request is pending"
        );

        worker.pending.remove("sibling").await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if worker.child.lock().await.is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the drained worker was not shut down");
        pool.shutdown().await;
    }

    /// With recycling disabled the counter must not advance at all, so the dev
    /// server pays nothing for a bound it does not need.
    #[tokio::test]
    async fn recycling_disabled_never_counts_or_replaces() {
        let (tx, _rx) = mpsc::channel::<String>(4);
        let worker = stub_worker(Some(tx));
        let pool = stub_pool(vec![Arc::clone(&worker)]);
        assert!(pool.retained_module_urls_per_worker.is_none());

        for _ in 0..8 {
            pool.retire_worker_if_saturated(0, &worker, None, true)
                .await;
        }

        assert_eq!(worker.retained_module_urls.load(Ordering::Acquire), 0);
        assert!(Arc::ptr_eq(&pool.workers.read().unwrap()[0], &worker));
    }

    #[tokio::test]
    async fn watcher_invalidation_reaches_every_healthy_worker_after_one_fails() {
        // Node's ESM cache is process-local, so a worker skipped here keeps
        // serving the stale bundle no matter how often the browser reloads.
        let (healthy_tx, mut healthy_rx) = mpsc::channel::<String>(4);
        let pool = stub_pool(vec![
            stub_worker(None), // shutting down: its stdin sender is gone
            stub_worker(Some(healthy_tx)),
        ]);

        let result = pool.invalidate_from_watcher(
            vec!["/project/app/page.tsx".to_string()],
            Some("0123456789abcdef0123456789abcdef"),
        );

        let error = result.expect_err("a failed worker must still surface an error");
        assert!(
            error.contains("1/2"),
            "error should report partial progress, got: {error}"
        );
        assert!(error.contains("worker 0"), "error should name the failure");

        let queued = healthy_rx
            .try_recv()
            .expect("worker after the failing one must still be invalidated");
        assert!(queued.contains("invalidate"));
        assert!(queued.contains("/project/app/page.tsx"));
        assert!(queued.contains("0123456789abcdef0123456789abcdef"));
        assert!(
            queued.ends_with('\n'),
            "protocol frames are newline-delimited"
        );
    }

    #[tokio::test]
    async fn watcher_invalidation_reports_the_queued_count_when_all_workers_accept() {
        let (first_tx, mut first_rx) = mpsc::channel::<String>(4);
        let (second_tx, mut second_rx) = mpsc::channel::<String>(4);
        let pool = stub_pool(vec![
            stub_worker(Some(first_tx)),
            stub_worker(Some(second_tx)),
        ]);

        let queued = pool
            .invalidate_from_watcher(vec!["/project/app/page.tsx".to_string()], None)
            .expect("healthy workers must not report an error");

        assert_eq!(queued, 2);
        // Each worker needs its own request id, or a worker could mistake a
        // sibling's reply for its own.
        let first = first_rx.try_recv().expect("first worker was skipped");
        let second = second_rx.try_recv().expect("second worker was skipped");
        assert_ne!(
            request_id_of(&first),
            request_id_of(&second),
            "each worker must receive a unique request id"
        );
    }

    #[tokio::test]
    async fn recycle_replaces_the_process_generation() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "import { createInterface } from 'node:readline'; createInterface({ input: process.stdin }).on('line', (line) => { const { id } = JSON.parse(line); process.stdout.write(JSON.stringify({ id, ok: true, pong: true }) + '\\n'); });",
        )
        .unwrap();

        let original = Arc::new(
            Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                .await
                .unwrap(),
        );
        let pool = pool_over(
            vec![Arc::clone(&original)],
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            None,
        );

        let replaced = pool
            .recycle()
            .await
            .expect("recycle must start a new worker generation");
        assert_eq!(replaced, 1);
        let replacement = pool.workers.read().unwrap()[0].clone();
        assert!(!Arc::ptr_eq(&original, &replacement));

        let response = pool
            .send(WorkerRequest::Ping {
                id: next_request_id(),
            })
            .await
            .expect("replacement worker must accept requests");
        assert_eq!(response.pong, Some(true));

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if original.child.lock().await.is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the drained instrumentation worker did not stop");
        pool.shutdown().await;
    }

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

    #[tokio::test]
    async fn api_body_stream_decodes_binary_frames_without_text_conversion() {
        let (sender, receiver) = mpsc::channel(2);
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
                streaming: Arc::new(AtomicBool::new(true)),
            },
            Arc::new(PendingResponseSet::default()),
            std::time::Duration::from_secs(1),
            Arc::new(OnceLock::new()),
        ));
        let bytes = axum::body::to_bytes(body, 1024).await.unwrap();

        assert_eq!(bytes.as_ref(), &[0, 255, 128, 13, 10]);
    }

    /// A frame the stream rejects still releases the worker's pending entry.
    ///
    /// Ending the stream and releasing the entry are two different things. The
    /// stdout reader only removes an entry it has seen a terminal frame for,
    /// and `api-start` is not terminal — so a worker that repeats it mid-stream
    /// ends the body here while leaving the entry behind, and nothing else ever
    /// removes it. `in_flight` then never returns to zero: `select_worker`
    /// permanently avoids the worker as its busiest, and retiring it sits out
    /// the full `WORKER_DRAIN_TIMEOUT` before the process is closed.
    #[tokio::test]
    async fn a_rejected_stream_frame_releases_the_pending_entry() {
        let pending: PendingResponses = Arc::new(PendingResponseSet::default());
        let (sender, receiver) = mpsc::channel(2);
        let streaming = Arc::new(AtomicBool::new(true));
        pending
            .insert(
                "stream".to_string(),
                PendingResponse {
                    sender: sender.clone(),
                    streaming: Arc::clone(&streaming),
                },
            )
            .await;
        // A second `api-start`: not a chunk, not an end, and not terminal, so
        // the reader that delivered it kept the entry.
        sender
            .send(WorkerResponse {
                id: "stream".to_string(),
                ok: true,
                frame: Some("api-start".to_string()),
                ..WorkerResponse::default()
            })
            .await
            .unwrap();
        drop(sender);
        assert_eq!(pending.len(), 1);

        let body = Body::from_stream(WorkerBodyStream::new(
            ResponseChannel {
                id: "stream".to_string(),
                receiver,
                streaming,
            },
            Arc::clone(&pending),
            std::time::Duration::from_secs(1),
            Arc::new(OnceLock::new()),
        ));
        let error = axum::body::to_bytes(body, 1024).await.unwrap_err();
        assert!(error.to_string().contains("Unexpected worker API stream"));

        // Removal is spawned, so give that task a turn rather than asserting on
        // scheduling order.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while pending.len() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the rejected stream never released its pending entry");
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
                streaming: Arc::new(AtomicBool::new(true)),
            },
            Arc::new(PendingResponseSet::default()),
            std::time::Duration::from_secs(1),
            Arc::new(OnceLock::new()),
        ));
        let error = axum::body::to_bytes(body, 1024).await.unwrap_err();

        assert!(error.to_string().contains("before api-end"));
    }

    #[tokio::test]
    async fn api_body_stream_propagates_worker_errors() {
        let (sender, receiver) = mpsc::channel(1);
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
                streaming: Arc::new(AtomicBool::new(true)),
            },
            Arc::new(PendingResponseSet::default()),
            std::time::Duration::from_secs(1),
            Arc::new(OnceLock::new()),
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
                streaming: Arc::new(AtomicBool::new(true)),
            },
            Arc::new(PendingResponseSet::default()),
            std::time::Duration::from_millis(20),
            Arc::new(OnceLock::new()),
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
            known_inputs_version: None,
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
            known_inputs_version: None,
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
            known_inputs_version: None,
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

    /// Shutdown reaches a worker that was retired and is still draining.
    ///
    /// A retired worker leaves `self.workers`, so the only thing that used to
    /// close it was its own detached drain task — and that task is by
    /// definition still waiting. `ruvyxa build` retires a worker every
    /// `RUVYXA_PRERENDER_RECYCLE_AFTER` isolated renders and `recycle` retires
    /// the whole generation at once, so the last thing a build does before
    /// exiting is create these. Whatever the CLI does next, the `node` children
    /// were no longer anybody's: the process exits without unwinding the drain
    /// task, nothing drops the `Child`, and `kill_on_drop` never runs.
    ///
    /// The pending entry below is what a real in-flight request is to the drain
    /// task: it makes `wait_until_idle` block, which is the state the bug needs.
    #[tokio::test]
    async fn shutdown_closes_a_worker_that_is_still_draining() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "import { createInterface } from 'node:readline'; createInterface({ input: process.stdin }).on('line', (line) => { const { id } = JSON.parse(line); process.stdout.write(JSON.stringify({ id, ok: true, pong: true }) + '\\n'); });",
        )
        .unwrap();

        let retired = Arc::new(
            Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                .await
                .unwrap(),
        );
        let pool = pool_over(
            vec![Arc::clone(&retired)],
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            None,
        );

        // Held for the rest of the test: dropping the receiver would let the
        // entry be reaped and the worker would drain on its own.
        let (sender, _receiver) = mpsc::channel::<WorkerResponse>(MAX_PENDING_RESPONSE_FRAMES);
        retired
            .pending
            .insert(
                "in-flight".to_string(),
                PendingResponse {
                    sender,
                    streaming: Arc::new(AtomicBool::new(false)),
                },
            )
            .await;

        assert_eq!(pool.recycle().await.unwrap(), 1);
        assert!(
            !Arc::ptr_eq(&retired, &pool.workers.read().unwrap()[0]),
            "recycle must have taken the worker out of selection"
        );
        assert_eq!(
            retired.in_flight(),
            1,
            "the retired worker must still be draining when shutdown runs"
        );

        pool.shutdown().await;

        assert!(
            retired.child.lock().await.is_none(),
            "shutdown left a retired worker process running"
        );
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
        let pool = pool_over(
            vec![Arc::new(worker)],
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            None,
        );

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
    async fn reported_module_url_growth_retires_a_normal_dev_worker() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            r#"
import { createInterface } from 'node:readline'
let retained = 0
createInterface({ input: process.stdin }).on('line', (line) => {
  const { id } = JSON.parse(line)
  process.stdout.write(JSON.stringify({
    id,
    ok: true,
    html: String(process.pid),
    retainedModuleUrls: ++retained,
  }) + '\n')
})
process.stdin.resume()
"#,
        )
        .unwrap();

        let worker = Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
            .await
            .unwrap();
        let pool = pool_over(
            vec![Arc::new(worker)],
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            Some(3),
        );

        let mut pids = Vec::new();
        for _ in 0..6 {
            let request = ssg_request(false);
            assert!(
                !request.retains_an_isolated_module_graph(),
                "the regression must exercise normal content-addressed imports"
            );
            let response = pool.send(request).await.unwrap();
            pids.push(response.html.unwrap());
        }

        assert_eq!(pids[0], pids[1], "{pids:?}");
        assert_eq!(pids[1], pids[2], "{pids:?}");
        assert_ne!(
            pids[2], pids[3],
            "reported ESM retention must replace the process: {pids:?}"
        );
        assert_eq!(pids[3], pids[4], "{pids:?}");
        assert_eq!(pids[4], pids[5], "{pids:?}");
        pool.shutdown().await;
    }

    #[tokio::test]
    async fn api_only_retention_retires_after_the_stream_drains() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            r#"
import { createInterface } from 'node:readline'
createInterface({ input: process.stdin }).on('line', (line) => {
  const { id } = JSON.parse(line)
  process.stdout.write(JSON.stringify({
    id,
    frame: 'api-start',
    ok: true,
    status: 200,
    headers: {},
    headerPairs: [],
    retainedModuleUrls: 2,
  }) + '\n')
  setTimeout(() => process.stdout.write(JSON.stringify({
    id,
    frame: 'api-end',
    ok: true,
    retainedModuleUrls: 2,
  }) + '\n'), 500)
})
process.stdin.resume()
"#,
        )
        .unwrap();

        let worker = Arc::new(
            Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                .await
                .unwrap(),
        );
        let pool = pool_over(
            vec![Arc::clone(&worker)],
            worker_script,
            std::time::Duration::from_secs(2),
            Some(2),
        );
        let params = BTreeMap::new();
        let route_file = temp.path().join("route.ts");

        let response = pool
            .render_api(RenderApiRequest {
                project_root: temp.path(),
                route_file: &route_file,
                method: "GET",
                request_path: "/api/only",
                headers: &[],
                body: None,
                params: &params,
                known_inputs_version: None,
            })
            .await
            .unwrap();

        assert!(
            !Arc::ptr_eq(&pool.workers.read().unwrap()[0], &worker),
            "api-start telemetry must remove the saturated worker from selection"
        );
        assert!(
            worker.child.lock().await.is_some(),
            "the old process must remain alive until api-end"
        );
        let body = axum::body::to_bytes(response.body.unwrap(), 1024)
            .await
            .unwrap();
        assert!(body.is_empty());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if worker.child.lock().await.is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the streamed API worker was not shut down after api-end");
        pool.shutdown().await;
    }

    /// Retiring the process is the only operation that releases the module
    /// graphs an isolated import pins, so the test asserts on the OS process
    /// identity rather than on the counter: a reset counter without a new
    /// process would free nothing.
    #[tokio::test]
    async fn isolated_prerenders_retire_the_worker_process_once_the_budget_is_reached() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        // Answers every request with its own pid so the test can see whether a
        // different process served the next render.
        std::fs::write(
            &worker_script,
            r#"
import { createInterface } from 'node:readline'
createInterface({ input: process.stdin }).on('line', (line) => {
  const { id } = JSON.parse(line)
  process.stdout.write(JSON.stringify({ id, ok: true, html: String(process.pid) }) + '\n')
})
process.stdin.resume()
"#,
        )
        .unwrap();

        let worker = Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
            .await
            .unwrap();
        let pool = pool_over(
            vec![Arc::new(worker)],
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            Some(3),
        );

        let mut pids = Vec::new();
        for _ in 0..6 {
            let response = pool
                .send(ssg_request(true))
                .await
                .expect("the stub worker always answers");
            pids.push(response.html.expect("the stub reports its pid"));
        }

        // Budget of 3: the first three renders share one process, then it is
        // retired and the next three share its replacement.
        assert_eq!(pids[0], pids[1], "{pids:?}");
        assert_eq!(pids[1], pids[2], "{pids:?}");
        assert_ne!(
            pids[2], pids[3],
            "the saturated worker must be replaced by a new process: {pids:?}"
        );
        assert_eq!(pids[3], pids[4], "{pids:?}");
        assert_eq!(pids[4], pids[5], "{pids:?}");

        // A cached (non-isolated) prerender adds no retention, so it must not
        // advance the budget or trigger another retirement.
        let before = pool
            .send(ssg_request(false))
            .await
            .unwrap()
            .html
            .expect("pid");
        for _ in 0..5 {
            let pid = pool
                .send(ssg_request(false))
                .await
                .unwrap()
                .html
                .expect("pid");
            assert_eq!(pid, before, "cached prerenders must not retire a worker");
        }

        pool.shutdown().await;
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
        let pool = pool_over(
            vec![failed_worker.clone()],
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            None,
        );

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

    #[tokio::test]
    async fn select_worker_avoids_the_busy_worker() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "process.stdin.on('end', () => process.exit(0)); process.stdin.resume();",
        )
        .unwrap();

        let spawn = || async {
            Arc::new(
                Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                    .await
                    .unwrap(),
            )
        };
        let busy = spawn().await;
        let idle = spawn().await;

        // Register three in-flight requests on the first worker without touching
        // the second, so a load-aware selector must route away from it.
        for _ in 0..3 {
            let (sender, _receiver) = mpsc::channel(1);
            busy.pending
                .insert(
                    next_request_id(),
                    PendingResponse {
                        sender,
                        streaming: Arc::new(AtomicBool::new(false)),
                    },
                )
                .await;
        }

        let pool = pool_over(
            vec![busy.clone(), idle.clone()],
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            None,
        );

        // Every selection must land on the idle worker regardless of the
        // rotating start offset, never the loaded one.
        for _ in 0..6 {
            let (index, worker) = pool.select_worker().await.unwrap();
            assert_eq!(index, 1, "selector routed to the busy worker");
            assert!(Arc::ptr_eq(&worker, &idle));
        }

        pool.shutdown().await;
    }

    #[tokio::test]
    async fn select_worker_does_not_wait_for_pending_map_lock() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "process.stdin.on('end', () => process.exit(0)); process.stdin.resume();",
        )
        .unwrap();

        let busy = Arc::new(
            Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                .await
                .unwrap(),
        );
        let idle = Arc::new(
            Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                .await
                .unwrap(),
        );
        let pool = pool_over(
            vec![busy.clone(), idle],
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            None,
        );

        // Request completion also needs this mutex. Routing new work must not
        // join that contention chain just to observe a worker's load.
        let pending_guard = busy.pending.entries.lock().await;
        let selection =
            tokio::time::timeout(std::time::Duration::from_millis(100), pool.select_worker()).await;
        drop(pending_guard);

        assert!(
            selection.is_ok(),
            "worker selection waited for the pending response map"
        );
        pool.shutdown().await;
    }

    #[tokio::test]
    async fn pending_response_count_tracks_the_map_lifecycle() {
        let pending = PendingResponseSet::default();
        let response = || {
            let (sender, _receiver) = mpsc::channel(1);
            PendingResponse {
                sender,
                streaming: Arc::new(AtomicBool::new(false)),
            }
        };

        pending.insert("first".to_string(), response()).await;
        assert_eq!(pending.len(), 1);
        assert!(pending.response("first", false).await.is_some());
        assert_eq!(pending.len(), 1);
        assert!(pending.response("first", true).await.is_some());
        assert_eq!(pending.len(), 0);

        pending.insert("second".to_string(), response()).await;
        pending.insert("third".to_string(), response()).await;
        assert_eq!(pending.len(), 2);
        assert_eq!(pending.take_all().await.len(), 2);
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn select_worker_rotates_when_all_idle() {
        let temp = tempfile::tempdir().unwrap();
        let worker_script = temp.path().join("worker.mjs");
        std::fs::write(
            &worker_script,
            "process.stdin.on('end', () => process.exit(0)); process.stdin.resume();",
        )
        .unwrap();

        let mut workers = Vec::new();
        for _ in 0..3 {
            workers.push(Arc::new(
                Worker::spawn(&worker_script, &BTreeMap::new(), JavaScriptRuntime::Node)
                    .await
                    .unwrap(),
            ));
        }

        let pool = pool_over(
            workers.clone(),
            worker_script,
            std::time::Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
            None,
        );

        // With every worker idle the rotating offset spreads work round-robin.
        let mut picked = Vec::new();
        for _ in 0..3 {
            picked.push(pool.select_worker().await.unwrap().0);
        }
        picked.sort();
        assert_eq!(picked, vec![0, 1, 2]);

        pool.shutdown().await;
    }

    /// A single-frame request leaves nothing behind, whatever frame arrives.
    ///
    /// `send` is used for the requests that answer in one frame, so the stdout
    /// reader removes the entry only when it sees a terminal one. A worker that
    /// replied with a non-terminal frame instead used to leave an entry whose
    /// receiver had just been dropped — a pending count that never returns to
    /// zero, so retiring that worker waited out the full drain timeout on every
    /// recycle. Nothing in the protocol should produce that frame; the point is
    /// that a worker which does cannot strand the pool.
    #[tokio::test]
    async fn a_single_frame_request_never_strands_its_pending_entry() {
        for frame in [None, Some("api-start")] {
            let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(4);
            let worker = stub_worker(Some(stdin_tx));
            let request = ssg_request(true);

            let responder = {
                let worker = Arc::clone(&worker);
                tokio::spawn(async move {
                    let line = stdin_rx.recv().await.expect("the request reaches stdin");
                    let id = request_id_of(&line);
                    // Exactly what the stdout reader does: a non-terminal frame
                    // keeps the entry, a terminal one takes it.
                    let terminal = frame.is_none();
                    let pending = worker
                        .pending
                        .response(&id, terminal)
                        .await
                        .expect("the request registered a pending response");
                    let _ = pending
                        .sender
                        .send(WorkerResponse {
                            id,
                            frame: frame.map(str::to_string),
                            ..Default::default()
                        })
                        .await;
                })
            };

            let sent = worker
                .send(&request, std::time::Duration::from_secs(5))
                .await;
            responder.await.unwrap();

            assert!(sent.is_ok(), "frame {frame:?} should answer the request");
            assert_eq!(
                worker.pending.len(),
                0,
                "frame {frame:?} left a pending entry behind"
            );
            // The property that actually matters: the worker can be retired
            // without waiting out the drain ceiling.
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                worker.pending.wait_until_idle(),
            )
            .await
            .unwrap_or_else(|_| panic!("frame {frame:?} left the worker undrainable"));
        }
    }
}
