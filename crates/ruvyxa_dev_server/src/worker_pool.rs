//! Persistent Node.js worker pool for eliminating subprocess spawn overhead.
//!
//! Instead of spawning a new `node` process for every SSR/API/action/client render,
//! this module maintains a pool of long-lived Node processes that communicate
//! via newline-delimited JSON over stdin/stdout.
//!
//! Performance impact: eliminates ~100-500ms of per-request overhead from process
//! creation, V8 startup, and renderer initialization.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, error, warn};

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use ruvyxa_graph::RouteParams;

/// Number of worker processes to maintain in the pool.
/// Defaults to the number of available CPU cores (clamped to 2..8) for optimal
/// concurrency without over-subscribing memory.
const DEFAULT_POOL_SIZE: usize = 4;

/// Minimum pool size regardless of configuration.
const MIN_POOL_SIZE: usize = 2;

/// Maximum pool size to prevent excessive memory usage from many Node processes.
const MAX_POOL_SIZE: usize = 8;

/// Maximum time to wait for a worker response before considering it dead.
const WORKER_TIMEOUT_MS: u64 = 10_000;
/// Build prerendering can legitimately take longer than an interactive request.
const BUILD_WORKER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Maximum time a worker receives to exit after its stdin closes before it is killed.
const WORKER_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
        #[serde(rename = "requestPath")]
        request_path: String,
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
    },
}

impl WorkerRequest {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerResponse {
    pub id: String,
    pub ok: bool,
    pub html: Option<String>,
    pub script: Option<String>,
    pub status: Option<u16>,
    pub headers: Option<BTreeMap<String, String>>,
    /// Ordered response headers. Prefer this over `headers` so repeated
    /// `Set-Cookie` values survive the Node-to-Rust boundary.
    pub header_pairs: Option<Vec<(String, String)>>,
    pub body: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub stack: Option<String>,
    pub pong: Option<bool>,
    pub warmed: Option<usize>,
    pub module_cache_size: Option<usize>,
    pub params: Option<Vec<RouteParams>>,
}

// --- Worker Process ---

struct Worker {
    stdin_tx: StdMutex<Option<mpsc::Sender<String>>>,
    pending: Arc<Mutex<BTreeMap<String, oneshot::Sender<WorkerResponse>>>>,
    child: Mutex<Option<Child>>,
    alive: Arc<AtomicBool>,
}

impl Worker {
    async fn spawn(
        worker_script: &Path,
        env: &BTreeMap<String, String>,
    ) -> std::result::Result<Self, RuvyxaError> {
        let mut child = Command::new("node")
            .arg(worker_script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env.iter())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| RuvyxaError::Io {
                message: "Failed to spawn Node worker process".to_string(),
                source,
            })?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let pending: Arc<Mutex<BTreeMap<String, oneshot::Sender<WorkerResponse>>>> =
            Arc::new(Mutex::new(BTreeMap::new()));
        let alive = Arc::new(AtomicBool::new(true));

        // Spawn stdin writer task
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(256);
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
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
                let sender = {
                    let mut map = reader_pending.lock().await;
                    map.remove(&id)
                };
                if let Some(sender) = sender {
                    let _ = sender.send(response);
                }
            }
            // Dropping every sender wakes requests immediately when the worker
            // exits, instead of leaving them to wait for the request timeout.
            reader_alive.store(false, Ordering::Release);
            reader_pending.lock().await.clear();
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
        let sender = self
            .stdin_tx
            .lock()
            .expect("worker stdin mutex poisoned")
            .take();
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
        if !self.alive.load(Ordering::Acquire) {
            return Err(RuvyxaError::Message(
                "Worker process has exited".to_string(),
            ));
        }
        let id = match request {
            WorkerRequest::Ssr { id, .. }
            | WorkerRequest::Api { id, .. }
            | WorkerRequest::Action { id, .. }
            | WorkerRequest::Client { id, .. }
            | WorkerRequest::Invalidate { id, .. }
            | WorkerRequest::Ping { id, .. }
            | WorkerRequest::Warmup { id, .. }
            | WorkerRequest::Ssg { id, .. }
            | WorkerRequest::StaticParams { id, .. } => id.clone(),
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), tx);
        }
        if !self.alive.load(Ordering::Acquire) {
            self.pending.lock().await.remove(&id);
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
            .expect("worker stdin mutex poisoned")
            .clone()
            .ok_or_else(|| RuvyxaError::Message("Worker process is shutting down".to_string()))?;

        if stdin_tx.send(line).await.is_err() {
            let mut pending = self.pending.lock().await;
            pending.remove(&id);
            return Err(RuvyxaError::Message(
                "Worker process stdin closed".to_string(),
            ));
        }

        match tokio::time::timeout(response_timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(RuvyxaError::Message(
                "Worker response channel closed unexpectedly".to_string(),
            )),
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(RuvyxaError::Message(format!(
                    "Worker request timed out after {}ms",
                    response_timeout.as_millis()
                )))
            }
        }
    }
}

// --- Worker Pool ---

pub struct NodeWorkerPool {
    workers: StdRwLock<Vec<Arc<Worker>>>,
    worker_script: PathBuf,
    env: BTreeMap<String, String>,
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

impl NodeWorkerPool {
    pub async fn start(root: &Path, env: BTreeMap<String, String>) -> Result<Self> {
        Self::start_with_timeout(
            root,
            env,
            None,
            std::time::Duration::from_millis(WORKER_TIMEOUT_MS),
        )
        .await
    }

    /// Start a pool with an optional bounded worker count.
    ///
    /// Build-time prerendering uses this to avoid starting idle Node processes
    /// beyond its already configured render concurrency.
    pub async fn start_with_size(
        root: &Path,
        mut env: BTreeMap<String, String>,
        worker_count: Option<usize>,
    ) -> Result<Self> {
        // Keep the Node worker's own watchdog aligned with the Rust caller.
        // This is build-only; the interactive server continues to use its
        // shorter response budget.
        env.entry("RUVYXA_WORKER_TIMEOUT_MS".to_string())
            .or_insert_with(|| BUILD_WORKER_TIMEOUT.as_millis().to_string());
        Self::start_with_timeout(root, env, worker_count, BUILD_WORKER_TIMEOUT).await
    }

    async fn start_with_timeout(
        root: &Path,
        env: BTreeMap<String, String>,
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

        let mut workers = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            workers.push(Arc::new(Worker::spawn(&worker_script, &env).await?));
        }

        // Health check: ping first worker to verify it's alive
        let ping = WorkerRequest::Ping {
            id: next_request_id(),
        };
        match workers[0].send(&ping, response_timeout).await {
            Ok(response) if response.ok => {
                debug!(pool_size, "Node worker pool started successfully");
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
            next_worker: AtomicU64::new(0),
            response_timeout,
        })
    }

    /// Stop every owned Node worker before the server releases its process resources.
    pub async fn shutdown(&self) {
        let workers = self
            .workers
            .read()
            .expect("worker pool lock poisoned")
            .clone();
        for worker in workers {
            worker.shutdown().await;
        }
    }

    /// Send a request to the next available worker (round-robin).
    pub async fn send(&self, request: WorkerRequest) -> Result<WorkerResponse> {
        let (index, worker) = {
            let workers = self.workers.read().expect("worker pool lock poisoned");
            if workers.is_empty() {
                return Err(RuvyxaError::Message(
                    "Worker pool has no workers".to_string(),
                ));
            }
            let index = self.next_worker.fetch_add(1, Ordering::Relaxed) as usize % workers.len();
            (index, workers[index].clone())
        };
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

    /// Replaces a failed worker before the next request can select its slot.
    /// The caller decides whether the failed request is safe to retry.
    async fn replace_failed_worker(
        &self,
        index: usize,
        failed: &Arc<Worker>,
    ) -> Option<Arc<Worker>> {
        let replacement = match Worker::spawn(&self.worker_script, &self.env).await {
            Ok(worker) => Arc::new(worker),
            Err(error) => {
                warn!(%error, failed_worker = index, "failed to replace Node worker");
                return None;
            }
        };

        let active = {
            let mut workers = self.workers.write().expect("worker pool lock poisoned");
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
        let workers = self
            .workers
            .read()
            .expect("worker pool lock poisoned")
            .clone();
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
            let stdin_tx = workers[i]
                .stdin_tx
                .lock()
                .expect("worker stdin mutex poisoned")
                .clone();
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
        let workers = self.workers.read().expect("worker pool lock poisoned");
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
                .expect("worker stdin mutex poisoned")
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
        let workers = self
            .workers
            .read()
            .expect("worker pool lock poisoned")
            .clone();
        if routes.is_empty() || workers.is_empty() {
            return 0;
        }

        let mut warmed = 0;
        for worker in &workers {
            let request = WorkerRequest::Warmup {
                id: next_request_id(),
                project_root: project_root.to_string(),
                routes: routes.clone(),
            };
            match worker.send(&request, self.response_timeout).await {
                Ok(response) if response.ok => {
                    warmed += response.warmed.unwrap_or_default();
                }
                Ok(response) => {
                    debug!(message = ?response.message, "worker warmup returned non-ok");
                }
                Err(_) => {
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

    pub(crate) async fn render_api(&self, api: RenderApiRequest<'_>) -> Result<WorkerResponse> {
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
            params: api.params.clone(),
        };
        self.send(request).await
    }

    pub async fn render_action(
        &self,
        project_root: &Path,
        action_file: &Path,
        action_name: &str,
        payload_json: &str,
        request_path: &str,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Action {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            action_file: action_file.display().to_string(),
            action_name: action_name.to_string(),
            payload_json: payload_json.to_string(),
            request_path: request_path.to_string(),
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

    /// Resolve `getStaticParams` through the persistent worker cache.
    pub async fn resolve_static_params(
        &self,
        project_root: &Path,
        page_file: &Path,
    ) -> Result<WorkerResponse> {
        self.send(WorkerRequest::StaticParams {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            page_file: page_file.display().to_string(),
        })
        .await
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[(((first & 0b0000_0011) << 4) | (second >> 4)) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(((second & 0b0000_1111) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(third & 0b0011_1111) as usize] as char
        } else {
            '='
        });
    }
    encoded
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

        let worker = Worker::spawn(&worker_script, &BTreeMap::new())
            .await
            .unwrap();
        let pool = NodeWorkerPool {
            workers: StdRwLock::new(vec![Arc::new(worker)]),
            worker_script,
            env: BTreeMap::new(),
            next_worker: AtomicU64::new(0),
            response_timeout: std::time::Duration::from_millis(WORKER_TIMEOUT_MS),
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

        let worker = Worker::spawn(&worker_script, &BTreeMap::new())
            .await
            .unwrap();
        let request = WorkerRequest::Ping {
            id: next_request_id(),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            worker.send(
                &request,
                std::time::Duration::from_millis(WORKER_TIMEOUT_MS),
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
            Worker::spawn(&worker_script, &BTreeMap::new())
                .await
                .unwrap(),
        );
        let pool = NodeWorkerPool {
            workers: StdRwLock::new(vec![failed_worker.clone()]),
            worker_script,
            env: BTreeMap::new(),
            next_worker: AtomicU64::new(0),
            response_timeout: std::time::Duration::from_millis(WORKER_TIMEOUT_MS),
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
