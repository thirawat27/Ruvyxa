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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, warn};

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};

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
        params: BTreeMap<String, String>,
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
        params: BTreeMap<String, String>,
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
        params: BTreeMap<String, String>,
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
        params: BTreeMap<String, String>,
        /// "full" | "ppr" — controls whether to wait for all content or just the shell.
        mode: String,
    },
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
    pub body: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub stack: Option<String>,
    pub pong: Option<bool>,
}

// --- Worker Process ---

struct Worker {
    stdin_tx: mpsc::Sender<String>,
    pending: Arc<Mutex<BTreeMap<String, oneshot::Sender<WorkerResponse>>>>,
    #[allow(dead_code)]
    child: Child,
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

        let pending: Arc<Mutex<BTreeMap<String, oneshot::Sender<WorkerResponse>>>> =
            Arc::new(Mutex::new(BTreeMap::new()));

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

        // Spawn stdout reader task
        let reader_pending = pending.clone();
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
                let mut map = reader_pending.lock().await;
                if let Some(sender) = map.remove(&id) {
                    let _ = sender.send(response);
                }
            }
            debug!("worker stdout reader exited");
        });

        Ok(Self {
            stdin_tx,
            pending,
            child,
        })
    }

    async fn send(&self, request: &WorkerRequest) -> Result<WorkerResponse> {
        let id = match request {
            WorkerRequest::Ssr { id, .. }
            | WorkerRequest::Api { id, .. }
            | WorkerRequest::Action { id, .. }
            | WorkerRequest::Client { id, .. }
            | WorkerRequest::Invalidate { id, .. }
            | WorkerRequest::Ping { id, .. }
            | WorkerRequest::Warmup { id, .. }
            | WorkerRequest::Ssg { id, .. } => id.clone(),
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), tx);
        }

        let line = serde_json::to_string(request)
            .map_err(|error| RuvyxaError::Message(error.to_string()))?
            + "\n";

        if self.stdin_tx.send(line).await.is_err() {
            let mut pending = self.pending.lock().await;
            pending.remove(&id);
            return Err(RuvyxaError::Message(
                "Worker process stdin closed".to_string(),
            ));
        }

        match tokio::time::timeout(std::time::Duration::from_millis(WORKER_TIMEOUT_MS), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(RuvyxaError::Message(
                "Worker response channel closed unexpectedly".to_string(),
            )),
            Err(_) => {
                let mut pending = self.pending.lock().await;
                pending.remove(&id);
                Err(RuvyxaError::Message(
                    "Worker request timed out after 30s".to_string(),
                ))
            }
        }
    }
}

// --- Worker Pool ---

pub struct NodeWorkerPool {
    workers: Vec<Worker>,
    next_worker: AtomicU64,
    worker_script: PathBuf,
    env: BTreeMap<String, String>,
}

impl NodeWorkerPool {
    pub async fn start(root: &Path, env: BTreeMap<String, String>) -> Result<Self> {
        let worker_script = find_worker_script(root).ok_or_else(|| {
            Diagnostic::new("RUV1702", "Worker pool script was not found")
                .explain(
                    "Ruvyxa could not find the persistent Node worker script (worker-pool.mjs).",
                )
                .suggest(
                    "Run pnpm install from the monorepo root, or install the ruvyxa package in the app.",
                )
        })?;

        let pool_size = std::env::var("RUVYXA_WORKER_POOL_SIZE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| {
                // Auto-size: use available CPU cores clamped to a reasonable range.
                std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(DEFAULT_POOL_SIZE)
            })
            .clamp(MIN_POOL_SIZE, MAX_POOL_SIZE);

        let mut workers = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            workers.push(Worker::spawn(&worker_script, &env).await?);
        }

        // Health check: ping first worker to verify it's alive
        let ping = WorkerRequest::Ping {
            id: next_request_id(),
        };
        match workers[0].send(&ping).await {
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
            workers,
            next_worker: AtomicU64::new(0),
            worker_script,
            env,
        })
    }

    /// Send a request to the next available worker (round-robin).
    pub async fn send(&self, request: WorkerRequest) -> Result<WorkerResponse> {
        let index = self.next_worker.fetch_add(1, Ordering::Relaxed) as usize % self.workers.len();
        let response = self.workers[index].send(&request).await;

        // If the worker failed, try the next one
        if response.is_err() && self.workers.len() > 1 {
            let fallback_index = (index + 1) % self.workers.len();
            warn!(
                failed_worker = index,
                fallback_worker = fallback_index,
                "retrying request on fallback worker"
            );
            return self.workers[fallback_index].send(&request).await;
        }

        response
    }

    /// Invalidate bundle caches in all workers concurrently (called on file change).
    ///
    /// Sends the invalidation request to all workers in parallel rather than
    /// sequentially, reducing latency from `n * RTT` to `max(RTT)`.
    pub async fn invalidate(&self, paths: Vec<String>) {
        // Build one request per worker (each needs its own unique id).
        let requests: Vec<WorkerRequest> = (0..self.workers.len())
            .map(|_| WorkerRequest::Invalidate {
                id: next_request_id(),
                paths: paths.clone(),
            })
            .collect();

        // Send all concurrently — tokio::join! doesn't work for dynamic counts,
        // so we collect futures and poll them all.
        let mut set = tokio::task::JoinSet::new();
        for (i, request) in requests.into_iter().enumerate() {
            let stdin_tx = self.workers[i].stdin_tx.clone();
            set.spawn(async move {
                let line = serde_json::to_string(&request).unwrap_or_default() + "\n";
                let _ = stdin_tx.send(line).await;
            });
        }
        // Wait for all to complete.
        while set.join_next().await.is_some() {}
    }

    /// Restart a dead worker (for self-healing).
    #[allow(dead_code)]
    pub async fn restart_worker(&mut self, index: usize) -> Result<()> {
        if index >= self.workers.len() {
            return Err(RuvyxaError::Message(format!(
                "Worker index {index} out of bounds"
            )));
        }
        let new_worker = Worker::spawn(&self.worker_script, &self.env).await?;
        self.workers[index] = new_worker;
        Ok(())
    }

    /// Pre-warm module caches in a worker by importing route bundles during idle time.
    ///
    /// This eliminates the cold-start penalty for the first request to each route.
    /// Sends the warmup request to worker 0 only (one warmed cache is enough;
    /// hot modules will be cached per-worker on first real request).
    pub async fn warmup(&self, project_root: &str, routes: Vec<WarmupRoute>) {
        if routes.is_empty() || self.workers.is_empty() {
            return;
        }

        let request = WorkerRequest::Warmup {
            id: next_request_id(),
            project_root: project_root.to_string(),
            routes,
        };

        // Send to first worker only (warmup is best-effort).
        match self.workers[0].send(&request).await {
            Ok(response) if response.ok => {
                debug!("worker warmup completed");
            }
            Ok(response) => {
                debug!(message = ?response.message, "worker warmup returned non-ok");
            }
            Err(_) => {
                // Non-fatal: warmup is an optimization, not a requirement.
            }
        }
    }

    // --- Convenience methods for each render type ---

    pub async fn render_ssr(
        &self,
        project_root: &Path,
        app_dir: &Path,
        page_file: &Path,
        request_path: &str,
        params: &BTreeMap<String, String>,
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

    pub async fn render_api(
        &self,
        project_root: &Path,
        route_file: &Path,
        method: &str,
        request_path: &str,
        params: &BTreeMap<String, String>,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Api {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            route_file: route_file.display().to_string(),
            method: method.to_string(),
            request_path: request_path.to_string(),
            params: params.clone(),
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
        params: &BTreeMap<String, String>,
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
        params: &BTreeMap<String, String>,
        mode: &str,
    ) -> Result<WorkerResponse> {
        let request = WorkerRequest::Ssg {
            id: next_request_id(),
            project_root: project_root.display().to_string(),
            app_dir: app_dir.display().to_string(),
            page_file: page_file.display().to_string(),
            request_path: request_path.to_string(),
            params: params.clone(),
            mode: mode.to_string(),
        };
        self.send(request).await
    }
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
