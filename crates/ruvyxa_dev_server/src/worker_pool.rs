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
use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
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
        headers: BTreeMap<String, String>,
        body: Option<String>,
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
    pub warmed: Option<usize>,
    pub module_cache_size: Option<usize>,
}

// --- Worker Process ---

struct Worker {
    stdin_tx: StdMutex<Option<mpsc::Sender<String>>>,
    pending: Arc<Mutex<BTreeMap<String, oneshot::Sender<WorkerResponse>>>>,
    child: Mutex<Option<Child>>,
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
                let sender = {
                    let mut map = reader_pending.lock().await;
                    map.remove(&id)
                };
                if let Some(sender) = sender {
                    let _ = sender.send(response);
                }
            }
            debug!("worker stdout reader exited");
        });

        Ok(Self {
            stdin_tx: StdMutex::new(Some(stdin_tx)),
            pending,
            child: Mutex::new(Some(child)),
        })
    }

    /// Close the worker input, then force-stop it if graceful shutdown takes too long.
    async fn shutdown(&self) {
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
}

pub(crate) struct RenderApiRequest<'a> {
    pub project_root: &'a Path,
    pub route_file: &'a Path,
    pub method: &'a str,
    pub request_path: &'a str,
    pub headers: &'a BTreeMap<String, String>,
    pub body: Option<&'a str>,
    pub params: &'a BTreeMap<String, String>,
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
        })
    }

    /// Stop every owned Node worker before the server releases its process resources.
    pub async fn shutdown(&self) {
        for worker in &self.workers {
            worker.shutdown().await;
        }
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
            let stdin_tx = self.workers[i]
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
        let mut queued = 0;
        for (worker_index, worker) in self.workers.iter().enumerate() {
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
        if routes.is_empty() || self.workers.is_empty() {
            return 0;
        }

        let mut warmed = 0;
        for worker in &self.workers {
            let request = WorkerRequest::Warmup {
                id: next_request_id(),
                project_root: project_root.to_string(),
                routes: routes.clone(),
            };
            match worker.send(&request).await {
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
        debug!(
            warmed,
            workers = self.workers.len(),
            "worker warmup completed"
        );
        warmed
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

    pub(crate) async fn render_api(&self, api: RenderApiRequest<'_>) -> Result<WorkerResponse> {
        let request = WorkerRequest::Api {
            id: next_request_id(),
            project_root: api.project_root.display().to_string(),
            route_file: api.route_file.display().to_string(),
            method: api.method.to_string(),
            request_path: api.request_path.to_string(),
            headers: api.headers.clone(),
            body: api.body.map(str::to_string),
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

#[cfg(test)]
mod tests {
    use super::*;

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
            workers: vec![worker],
            next_worker: AtomicU64::new(0),
        };

        pool.shutdown().await;

        assert!(pool.workers[0].child.lock().await.is_none());
        assert!(
            pool.workers[0]
                .stdin_tx
                .lock()
                .expect("worker stdin mutex poisoned")
                .is_none()
        );
    }
}
