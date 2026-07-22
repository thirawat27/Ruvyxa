# Worker Pool (`NodeWorkerPool`)

**File**: `crates/ruvyxa_dev_server/src/worker_pool.rs`

Persistent Node.js or Bun worker processes communicating via newline-delimited JSON (NDJSON) over
stdin/stdout. Eliminates per-request JavaScript process spawn overhead (~100-500ms). The public Rust
type remains `NodeWorkerPool` for backwards compatibility.

---

## Architecture

```
                    ┌──────────────────────┐
                    │    NodeWorkerPool     │
                    │  - workers: Vec<Arc<Worker>>    │
                    │  - next_worker: AtomicU64       │
                    └──────┬───────────────┘
                           │ least in-flight load
                           │ rotating tie-break
        ┌──────────────────┼──────────────────┐
        ▼                  ▼                  ▼
  ┌──────────┐      ┌──────────┐      ┌──────────┐
  │ Worker 0 │      │ Worker 1 │      │ Worker 2 │
  │ Node/Bun │      │ Node/Bun │      │ Node/Bun │
  │ subproc  │      │ subproc  │      │ subproc  │
  └──────────┘      └──────────┘      └──────────┘
       │                  │                  │
  stdin/stdout       stdin/stdout       stdin/stdout
  NDJSON lines       NDJSON lines       NDJSON lines
```

## Constants

```rust
const DEFAULT_POOL_SIZE: usize = 4;                       // min 2, max 8
const DEFAULT_WORKER_TIMEOUT_MS: u64 = 30_000;           // interactive requests
const BUILD_WORKER_TIMEOUT_MS: u64 = 300_000;             // prerendering
const MAX_NODE_TIMEOUT_MS: u64 = 2_147_483_647;           // i32::MAX
const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PENDING_RESPONSE_FRAMES: usize = 16;             // streaming backpressure
```

Pool size: available parallelism clamped to 2–8 by default. An explicit `build.workers` value is
clamped to 1–8, allowing a short-lived build with one rendering job to avoid an idle process.

---

## Data Structures

### `NodeWorkerPool`

```rust
pub struct NodeWorkerPool {
    workers: StdRwLock<Vec<Arc<Worker>>>,
    worker_script: PathBuf,              // packages/ruvyxa/runtime/worker-pool.mjs
    env: BTreeMap<String, String>,
    runtime: JavaScriptRuntime,           // node when available, otherwise bun if unspecified
    next_worker: AtomicU64,               // rotating tie-break cursor
    response_timeout: Duration,           // configurable via RUVYXA_WORKER_TIMEOUT_MS
}
```

### `Worker`

```rust
struct Worker {
    stdin_tx: StdMutex<Option<mpsc::Sender<String>>>,  // None = shutting down
    pending: PendingResponses,           // Arc<PendingResponseSet>
    child: Mutex<Option<Child>>,         // std::process::Child
    alive: Arc<AtomicBool>,
}

struct PendingResponseSet {
    entries: Mutex<BTreeMap<String, PendingResponse>>,
    count: AtomicUsize,                  // lock-free worker load
}

type PendingResponses = Arc<PendingResponseSet>;
```

### `PendingResponse`

```rust
struct PendingResponse {
    sender: mpsc::Sender<WorkerResponse>,   // bounded(16)
    streaming: Arc<AtomicBool>,             // true after api-start frame
}
```

### `WorkerBodyStream`

Implements `Stream<Item = Result<Bytes, io::Error>>`:

```rust
struct WorkerBodyStream {
    receiver: mpsc::Receiver<WorkerResponse>,
    idle_deadline: Option<Instant>,    // resets on each frame
    finished: bool,
}
```

Frame handling:

- `"api-start"` → start streaming, return empty
- `"api-chunk"` → base64-decode `body_base64` → `Bytes`
- `"api-end"` → `None` (stream terminated)
- `"api-error"` → `io::Error::new(kind, message)`
- Premature EOF → `io::ErrorKind::UnexpectedEof`
- Idle timeout → `io::ErrorKind::TimedOut`

---

## Pool Initialization

### `NodeWorkerPool::start(root, env) → Self`

```rust
pub async fn start(root: &Path, env: BTreeMap<String, String>) -> Result<Arc<Self>> {
    let worker_script = find_worker_script(root)?;
    let pool_size = detect_pool_size();
    let pool = Arc::new(NodeWorkerPool {
        workers: StdRwLock::new(Vec::with_capacity(pool_size)),
        worker_script,
        env,
        next_worker: AtomicU64::new(0),
        response_timeout: Duration::from_millis(DEFAULT_WORKER_TIMEOUT_MS),
    });

    for _ in 0..pool_size {
        let worker = Worker::spawn(&pool.worker_script, &pool.env).await?;
        pool.workers.write().unwrap().push(Arc::new(worker));
    }

    Ok(pool)
}
```

### `find_worker_script(root) → Option<PathBuf>`

Resolution order:

1. Walk up directories from current dir looking for `packages/ruvyxa/runtime/<script>` (monorepo)
2. `{root}/node_modules/ruvyxa/runtime/<script>` (installed package)

Returns first that exists.

### `Worker::spawn(script, env) → Result<Self>`

```rust
async fn spawn(worker_script: &Path, env: &BTreeMap<String, String>) -> Result<Self>
```

1. **Spawn the selected Node or Bun process**:

   ```rust
   let mut cmd = Command::new(runtime.executable());
   cmd.arg(worker_script);
   cmd.stdin(Stdio::piped());
   cmd.stdout(Stdio::piped());
   cmd.stderr(Stdio::piped());
   cmd.envs(env);
   cmd.kill_on_drop(true);

   let mut child = cmd.spawn()?;
   ```

2. **Stdin writer task** (async):

   ```rust
   let stdin = child.stdin.take().unwrap();
   tokio::spawn(async move {
       let mut stdin = BufWriter::new(stdin);
       while let Some(line) = rx.recv().await {
           stdin.write_all(line.as_bytes()).await?;
           stdin.write_all(b"\n").await?;
           stdin.flush().await?;
           generation += 1;
       }
       // On channel close → stdin drops, Node sees EOF
   });
   ```

3. **Stderr drain task** (async):

   ```rust
   let stderr = child.stderr.take().unwrap();
   tokio::spawn(async move {
       let reader = BufReader::new(stderr);
       let mut lines = reader.lines();
       while let Some(Ok(line)) = lines.next_line().await {
           tracing::warn!("[worker stderr] {}", line);
       }
   });
   ```

4. **Stdout reader task** (async):

   ```rust
   let stdout = child.stdout.take().unwrap();
   tokio::spawn(async move {
       let reader = BufReader::new(stdout);
       let mut lines = reader.lines();
       while let Some(Ok(line)) = lines.next_line().await {
           let response: WorkerResponse = serde_json::from_str(&line)?;

           let terminal = response.is_terminal();
           if let Some(pr) = pending.response(&response.id, terminal).await {
               pr.sender.send(response).await.ok();
           }
       }
       // On EOF: drain pending, mark dead
       alive.store(false, Ordering::Release);
       for (_, pr) in pending.take_all().await {
           pr.sender.send(stream_error()).await.ok();
       }
   });
   ```

5. Return `Worker { stdin_tx, pending, child, alive }`.

---

## Communication Protocol

### Request serialization (`WorkerRequest`)

Tagged JSON via `#[serde(tag = "type")]`:

```rust
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerRequest {
    Ssr {
        id: String,
        project_root: PathBuf,        // serde: projectRoot
        app_dir: PathBuf,             // serde: appDir
        page_file: PathBuf,           // serde: pageFile
        request_path: String,         // serde: requestPath
        params: RouteParams,
    },
    Api {
        id: String,
        project_root: PathBuf,
        route_file: PathBuf,          // serde: routeFile
        method: String,
        request_path: String,         // serde: requestPath
        headers: HashMap<String, String>,
        header_pairs: Vec<(String, String)>,  // serde: headerPairs (preserves order)
        body: Option<String>,
        body_base64: Option<String>,  // serde: bodyBase64
        stream_response: bool,        // serde: streamResponse
        params: RouteParams,
    },
    Action {
        id: String,
        project_root: PathBuf,
        action_file: PathBuf,         // serde: actionFile
        action_name: String,          // serde: actionName
        payload_json: String,         // serde: payloadJson
        content_type: String,         // serde: contentType
        request_path: String,         // serde: requestPath
    },
    Client {
        id: String,
        project_root: PathBuf,
        app_dir: PathBuf,
        page_file: PathBuf,
        request_path: String,
        params: RouteParams,
    },
    Invalidate {
        id: String,
        paths: Vec<String>,
    },
    Ping {
        id: String,
    },
    Warmup {
        id: String,
        project_root: PathBuf,
        routes: Vec<WarmupRoute>,
    },
    Ssg {
        id: String,
        project_root: PathBuf,
        app_dir: PathBuf,
        page_file: PathBuf,
        request_path: String,
        params: RouteParams,
        mode: Option<String>,          // "full" | "ppr"
        fresh: Option<bool>,
    },
    StaticParams {
        id: String,
        project_root: PathBuf,
        page_file: PathBuf,           // serde: pageFile
        route_path: String,           // serde: routePath
        segments: Vec<String>,        // dynamic segment names
        routes: Vec<RouteEntry>,   // serde: routes (for global params resolve)
    },
}
```

### Response deserialization (`WorkerResponse`)

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerResponse {
    pub id: String,
    pub ok: bool,
    pub frame: Option<String>,          // "api-start" | "api-chunk" | "api-end" | "api-error"
    pub html: Option<String>,
    pub script: Option<String>,
    pub status: Option<u16>,
    pub headers: Option<HashMap<String, String>>,
    pub header_pairs: Option<Vec<(String, String)>>,  // serde: headerPairs
    pub body: Option<String>,
    pub body_base64: Option<String>,    // serde: bodyBase64
    pub code: Option<String>,           // error code
    pub message: Option<String>,        // error message
    pub stack: Option<String>,          // JS stack trace
    pub pong: Option<bool>,
    pub warmed: Option<usize>,
    pub module_cache_size: Option<usize>,  // serde: moduleCacheSize
    pub params: Option<Vec<RouteParams>>,   // for StaticParams response
    pub dependency_hash: Option<String>,    // serde: dependencyHash
    pub inputs: Option<Vec<PathBuf>>,
}
```

### Example exchanges

**SSR request**:

```
→ {"type":"ssr","id":"abc-123","projectRoot":"/project","appDir":"/project/app","pageFile":"/project/app/page.tsx","requestPath":"/about","params":{}}
← {"id":"abc-123","ok":true,"html":"<!doctype html><html>...</html>"}
```

**API request (streaming)**:

```
→ {"type":"api","id":"def-456","projectRoot":"/project","routeFile":"/project/app/api/stream/route.ts","method":"GET","requestPath":"/api/stream","headers":{},"streamResponse":true,"params":{}}
← {"id":"def-456","ok":true,"frame":"api-start","status":200,"headers":{"content-type":"text/plain"}}
← {"id":"def-456","ok":true,"frame":"api-chunk","bodyBase64":"SGVsbG8="}
← {"id":"def-456","ok":true,"frame":"api-chunk","bodyBase64":"V29ybGQ="}
← {"id":"def-456","ok":true,"frame":"api-end"}
```

**Error response**:

```
→ {"type":"ssr","id":"ghi-789",...}
← {"id":"ghi-789","ok":false,"code":"RUV1100","message":"React SSR failed","stack":"Error: ...\n    at ..."}
```

---

## Sending Requests

### `send<F>(&self, build_request: F) → Result<WorkerResponse> where F: FnOnce(String) -> WorkerRequest`

```rust
async fn send<F>(&self, build_request: F) -> Result<WorkerResponse>
{
    let (index, worker) = self.select_worker().await?;
    let id = Uuid::new_v4().to_string();
    let request = build_request(id.clone());
    let line = serde_json::to_string(&request)?;

    // Create response channel first (before sending, to avoid race)
    let (tx, rx) = mpsc::channel(MAX_PENDING_RESPONSE_FRAMES);
    worker.pending
        .insert(id.clone(), PendingResponse {
            sender: tx,
            streaming: Arc::new(AtomicBool::new(false)),
        })
        .await;

    // Send to worker stdin
    let stdin = worker.stdin_tx.lock().unwrap();
    if let Some(tx) = stdin.as_ref() {
        tx.send(line).await?;
    } else {
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "worker stdin closed"));
    }

    // Wait for response with timeout
    let result = tokio::time::timeout(self.response_timeout, async {
        let mut response = rx.recv().await?;
        while response.frame.is_some() && response.frame.as_deref() != Some("api-error") {
            response = rx.recv().await?;
        }
        Ok(response)
    }).await;

    // Cleanup pending entry
    worker.pending.remove(&id).await;

    match result {
        Ok(Ok(resp)) if resp.ok => Ok(resp),
        Ok(Ok(resp)) => Err(worker_error(resp)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(timeout_error()),
    }
}
```

### `send_streaming<F>(&self, build_request: F) → Result<(WorkerResponse, WorkerBodyStream)>`

Same pattern but returns a `WorkerBodyStream` for chunked consumption. The initial response contains
status + headers (`api-start` frame). Subsequent frames arrive on the stream.

---

## Worker Selection & Failure Recovery

### Least-loaded selection with fair ties

```rust
async fn select_worker(&self) -> Result<(usize, Arc<Worker>)> {
    let workers = {
        let guard = self.workers.read().map_err(|_| worker_pool_lock_error())?;
        guard.clone() // stable worker snapshot; pending counts are atomic
    };
    let start = self.next_worker.fetch_add(1, Ordering::Relaxed) as usize;
    let mut best = None;
    for offset in 0..workers.len() {
        let index = (start + offset) % workers.len();
        let load = workers[index].in_flight();
        if load == 0 { return Ok((index, Arc::clone(&workers[index]))); }
        if best.as_ref().is_none_or(|(_, best_load)| load < *best_load) {
            best = Some((index, load));
        }
    }
    let index = best.unwrap().0;
    Ok((index, Arc::clone(&workers[index])))
}
```

The cursor only decides where an equal-load scan begins. A busy worker is skipped when a sibling has
fewer pending requests, while all-idle workers still rotate fairly. Selection never waits for a
pending-response map lock, so response delivery and request routing do not form a contention chain.

### Failure recovery in `send()`

```rust
if !response.ok || response.code.is_some() {
    // Replace failed worker
    let new_worker = replace_failed_worker(&self.workers, index, &worker)?;

    // Retry if idempotent
    if is_idempotent(&request) {
        return self.send_on_worker(&new_worker, build_request).await;
    }
    return Err(error);
}
```

### Idempotent requests

| Request type | Idempotent?        |
| ------------ | ------------------ |
| Ssr          | Yes (GET-like)     |
| Ssg          | Yes (pure render)  |
| StaticParams | Yes (pure compute) |
| Client       | Yes (pure compile) |
| Ping         | Yes                |
| Warmup       | Yes                |
| Invalidate   | Yes                |
| Api          | No (side effects)  |
| Action       | No (mutations)     |

### `replace_failed_worker(workers, index, old_worker) → Result<Arc<Worker>>`

```rust
fn replace_failed_worker(
    workers: &StdRwLock<Vec<Arc<Worker>>>,
    index: usize,
    old_worker: &Arc<Worker>,
) -> Result<Arc<Worker>> {
    let new_worker = Worker::spawn(&self.worker_script, &self.env)?;
    let mut guard = workers.write().unwrap();

    // Check if worker at index still matches old_worker (may have been replaced concurrently)
    if Arc::ptr_eq(&guard[index], old_worker) {
        guard[index] = Arc::new(new_worker);
        Ok(guard[index].clone())
    } else {
        // Already replaced by another caller. Shutdown our spurious replacement.
        shutdown_worker(&new_worker);
        Ok(guard[index].clone())
    }
}
```

---

## Worker Shutdown

```rust
impl NodeWorkerPool {
    pub async fn shutdown(&self) {
        // 1. Send shutdown signal to each worker stdin (close mpsc sender → Node sees EOF)
        for worker in self.workers.read().unwrap().iter() {
            worker.stdin_tx.lock().unwrap().take(); // Drop sender
        }

        // 2. Wait up to WORKER_SHUTDOWN_TIMEOUT (2s)
        tokio::time::sleep(WORKER_SHUTDOWN_TIMEOUT).await;

        // 3. Kill any still-running children
        for worker in self.workers.read().unwrap().iter() {
            if let Ok(mut child) = worker.child.lock() {
                if let Some(ref mut child) = *child {
                    let _ = child.start_kill();
                    let _ = child.wait();
                }
            }
        }
    }
}
```

---

## Bundle Cache Invalidation

### Async: `invalidate(paths)`

```rust
pub async fn invalidate(&self, paths: &[String]) -> Result<()> {
    let workers = self.workers.read().unwrap().clone();
    let mut join_set = JoinSet::new();

    for worker in &workers {
        let worker = worker.clone();
        let paths = paths.to_vec();
        join_set.spawn(async move {
            worker.send(|id| WorkerRequest::Invalidate { id, paths }).await
        });
    }

    // Wait for all workers to acknowledge invalidation
    while let Some(result) = join_set.join_next().await {
        result??;  // Propagate errors
    }
    Ok(())
}
```

Parallel invalidation across all workers: `max(worker_latency)` instead of `sum(worker_latency)`.

### Synchronous: `invalidate_from_watcher(paths)`

```rust
pub fn invalidate_from_watcher(&self, paths: &[String]) -> Result<()> {
    // File watcher callback — no tokio runtime available
    let workers = self.workers.read().unwrap();
    for worker in workers.iter() {
        let request = WorkerRequest::Invalidate {
            id: Uuid::new_v4().to_string(),
            paths: paths.to_vec(),
        };
        let line = serde_json::to_string(&request)? + "\n";

        let stdin = worker.stdin_tx.lock().unwrap();
        if let Some(tx) = stdin.as_ref() {
            tx.try_send(line).ok();  // Non-blocking, ignore errors
        }
    }
    Ok(())
}
```

Uses `try_send()` — if channel is full, drops the invalidation (non-critical, worker will get stale
cache). Caller falls back to full reload if this fails.

---

## Public API Methods

```rust
impl NodeWorkerPool {
    pub async fn start(root: &Path, env: BTreeMap<String, String>) -> Result<Arc<Self>>;
    pub async fn shutdown(&self);

    // Rendering
    pub async fn render_ssr(&self, root, app_dir, page_file, request_path, params) -> Result<SsrResult>;
    pub async fn render_ssg(&self, root, app_dir, page_file, request_path, params, mode) -> Result<SsgResult>;
    pub async fn render_api(&self, root, route_file, method, path, headers, body, stream, params) -> Result<ApiResult>;
    pub async fn render_action(&self, root, action_file, name, payload, content_type, path) -> Result<ActionResult>;
    pub async fn render_client(&self, root, app_dir, page_file, request_path, params) -> Result<ClientResult>;

    // Maintenance
    pub async fn ping(&self) -> Result<bool>;
    pub async fn warmup(&self, root: PathBuf, routes: Vec<RouteWarmupEntry>) -> Result<usize>;
    pub async fn resolve_static_params(&self, root, page_file, route_path, segments, routes) -> Result<Vec<RouteParams>>;

    // Cache management
    pub async fn invalidate(&self, paths: &[String]) -> Result<()>;
    pub fn invalidate_from_watcher(&self, paths: &[String]) -> Result<()>;

    // Internal
    async fn select_worker(&self) -> Result<(usize, Arc<Worker>)>;
    async fn send<F>(&self, build_request: F) -> Result<WorkerResponse>;
    async fn send_streaming<F>(&self, build_request: F) -> Result<(WorkerResponse, WorkerBodyStream)>;
}
```

---

## Timeout Handling

| Request type                              | Timeout                          |
| ----------------------------------------- | -------------------------------- |
| Interactive (dev SSR, API, Action)        | `response_timeout` (default 30s) |
| Build (SSG, StaticParams, Client, Warmup) | `BUILD_WORKER_TIMEOUT_MS` (300s) |
| Config eval                               | `response_timeout`               |
| Invalidate                                | `response_timeout`               |

Timeout configurable via `RUVYXA_WORKER_TIMEOUT_MS` env var (normalized and passed to Node as well).

On timeout:

- Remove pending entry from worker's pending map.
- Return `io::ErrorKind::TimedOut`.
- Worker is NOT replaced (timeout ≠ worker failure).

---

## Error Handling

| Failure mode         | Error type                            | Recovery                                          |
| -------------------- | ------------------------------------- | ------------------------------------------------- |
| Worker stdin closed  | `BrokenPipe`                          | `replace_failed_worker()`                         |
| Worker stdout EOF    | `UnexpectedEof`                       | `alive = false`, drain pending                    |
| Request timeout      | `TimedOut`                            | Remove pending, caller handles                    |
| Response `ok: false` | `WorkerError` with code/message/stack | `replace_failed_worker()` + retry if idempotent   |
| Worker process crash | `alive = false`                       | Detected on next send → `replace_failed_worker()` |
