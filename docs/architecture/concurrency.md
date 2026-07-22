# Concurrency Model & Performance

How Ruvyxa uses threads, locks, channels, and parallelism across the Rust layer.

---

## Lock & synchronization map

| Component                          | Mechanism                                        | Crate       | Rationale                                            |
| ---------------------------------- | ------------------------------------------------ | ----------- | ---------------------------------------------------- |
| **ResolveGraphCache.resolutions**  | `DashMap<(Arc<str>, Arc<str>), Option<PathBuf>>` | dashmap     | Read-heavy, lock-free reads, 64 shards               |
| **ResolveGraphCache.sources**      | `DashMap<PathBuf, CachedSource>`                 | dashmap     | Concurrent source reads                              |
| **ResolveGraphCache.tsconfigs**    | `DashMap<PathBuf, CachedTsConfig>`               | dashmap     | Infrequent tsconfig reads                            |
| **ResolveGraphCache.dependencies** | `DashMap<DependencyCacheKey, Arc<[PathBuf]>>`    | dashmap     | Cached dep lists                                     |
| **CompileCache.memory**            | `Arc<Mutex<HashMap<String, MemEntry>>>`          | std::sync   | Write infrequent, LRU needs correct order            |
| **CompileCache.disk**              | Atomic file writes (temp + rename)               | std::fs     | No concurrent write to same key possible             |
| **RenderCache.entries**            | `tokio::sync::RwLock<HashMap<...>>`              | tokio       | Async access, read-mostly                            |
| **RenderCache.order**              | `tokio::sync::RwLock<VecDeque<...>>`             | tokio       | Held together with entries during write              |
| **RenderCache.hits/misses**        | `AtomicU64`                                      | std::sync   | Relaxed ordering, stats only                         |
| **HmrTracker.file_to_routes**      | `parking_lot::RwLock<BTreeMap<...>>`             | parking_lot | Synchronous use (notify callback, no tokio)          |
| **HmrTracker.route_to_files**      | `parking_lot::RwLock<BTreeMap<...>>`             | parking_lot | Same as above                                        |
| **RuntimeCache.manifest**          | `tokio::sync::RwLock<Option<...>>`               | tokio       | Async manifest reads/writes                          |
| **RuntimeCache.styles**            | `tokio::sync::RwLock<Option<...>>`               | tokio       | Async style reads/invalidation                       |
| **RuntimeCache.router**            | `tokio::sync::RwLock<Option<...>>`               | tokio       | Async router rebuild on manifest change              |
| **WorkerPool.workers**             | `StdRwLock<Vec<Arc<Worker>>>`                    | std::sync   | Infrequent writes (failure recovery)                 |
| **WorkerPool.next_worker**         | `AtomicU64`                                      | std::sync   | Relaxed cursor for fair ties after load comparison   |
| **Worker.stdin_tx**                | `StdMutex<Option<mpsc::Sender<String>>>`         | std::sync   | Drop = signal shutdown                               |
| **Worker.pending**                 | `Arc<Mutex<BTreeMap<String, PendingResponse>>>`  | std::sync   | Write on every request (insert/remove), fast section |
| **Worker.child**                   | `Mutex<Option<Child>>`                           | std::sync   | Protect kill_on_drop + shutdown                      |
| **ISR revalidating set**           | `tokio::sync::Mutex<HashSet<String>>`            | tokio       | Async lock, coalesce concurrent revalidations        |
| **Action rate limiter**            | `Arc<Mutex<ActionRateLimiter>>`                  | std::sync   | Single writer, fast section                          |
| **Content module cache**           | `OnceLock<Mutex<HashMap<...>>>`                  | std::sync   | Global shared, lazy init                             |
| **PluginHost.worker**              | `tokio::sync::Mutex<PluginWorker>`               | tokio       | Serialize calls to the persistent plugin runtime     |

---

## Channel types & capacities

| Channel             | Type                                       | Capacity                         | Purpose                                                          |
| ------------------- | ------------------------------------------ | -------------------------------- | ---------------------------------------------------------------- |
| HMR broadcast       | `tokio::sync::broadcast::Sender<String>`   | 64                               | HMR events to all WebSocket clients. Drops oldest on overflow.   |
| Worker stdin        | `mpsc::Sender<String>` per worker          | 256                              | Request serialization to Node subprocess                         |
| Worker response     | `mpsc::Sender<WorkerResponse>` per request | 16 (MAX_PENDING_RESPONSE_FRAMES) | Per-request response channel, bounded backpressure for streaming |
| Dev server shutdown | `tokio::sync::watch::Sender<bool>`         | 1                                | Signal server shutdown                                           |

---

## Parallelism model

### Rayon parallelism (CPU-bound)

| Task                            | Pattern                                        | Notes                                                                  |
| ------------------------------- | ---------------------------------------------- | ---------------------------------------------------------------------- |
| Module resolution (phase 2 BFS) | `frontier.par_iter()`                          | I/O + CPU mix; each frontier level resolved in parallel                |
| Module compilation              | `compiled.par_iter()` → Oxc transform          | Per-module compilation fully parallel                                  |
| Linker (parallel)               | `modules.par_chunks()` → IIFE generation       | For >=8 modules; generates segments in parallel, concatenates serially |
| Image optimization              | `entries.par_iter()` → decode + encode WebP    | Configurable `workers` limits thread pool                              |
| Build: prepare bundles          | `routes.par_iter()` → `prepare_bundle()`       | All routes resolved+compiled in parallel                               |
| Prerender rendering             | `route_groups.par_iter()` → worker pool render | Max parallelism 2 (build mode uses dedicated pool)                     |

### Tokio async parallelism (I/O-bound)

| Task                         | Pattern                               | Notes                               |
| ---------------------------- | ------------------------------------- | ----------------------------------- |
| Dev server: request handling | Concurrent Axum handlers              | One task per request                |
| Worker pool: send to workers | Concurrent `JoinSet`                  | All workers invalidated in parallel |
| ISR revalidation             | `tokio::spawn()` per revalidation     | Non-blocking background refresh     |
| File watcher                 | OS notify callback → sync → broadcast | notify runs on dedicated OS thread  |
| Worker stdin/out/stderr      | 3 concurrent tokio tasks per worker   | Independent reader/writer/drain     |

---

## Critical paths & bottleneck analysis

### Hot paths (per-request, dev server)

1. **Route lookup**: `RadixRouter::find()` — O(path_depth). Radix trie with linear-scan static
   children. Acceptable for typical routes (depth < 10).

2. **Render cache get**: `RwLock<HashMap>.read()` + `VecDeque` promote — O(1). Tokio RwLock handles
   concurrent reads efficiently.

3. **Worker pool send**: inspect each worker's pending count, starting at a rotating atomic cursor,
   then use the least-loaded worker. The NDJSON channel write itself is O(1).

4. **HTML composition**: string search (`find_ascii_case`) + `format!()` — negligible.

5. **Style collection refresh** (on CSS change): import graph BFS + `grass` Sass compilation — may
   take 50-200ms. Cached normally, only recomputed on invalidation.

### Hot paths (per-request, production)

Same as dev minus HMR + error overlay overhead. Cache TTL is higher (1800s vs 300s).

### Build hot paths

1. **Route discovery**: `WalkDir` of `app/`. Typical depth < 5, file count < 500.

2. **Client bundling**: parallel route preparation (dominated by Oxc transforms). Okx is 10-100x
   faster than Babel/SWC. Typical 50-500ms per route depending on import depth.

3. **Image optimization**: `image::open()` decode + `webp::Encoder`. Heaviest per-file task.
   Parallelized via rayon.

4. **Prerendering**: Node worker rendering (SSG/ISR/PPR). I/O-bound, max parallelism 2 to avoid
   worker pool exhaustion.

---

## Lock contention scenarios

| Scenario                                       | Risk     | Mitigation                                                             |
| ---------------------------------------------- | -------- | ---------------------------------------------------------------------- |
| Concurrent render cache reads                  | Low      | tokio RwLock, read-mostly                                              |
| Render cache write + invalidate simultaneously | Medium   | Both locks held together briefly; write path uncommon in prod (cached) |
| Worker pending map insert/remove               | Low      | Mutex held for insert+send or remove+drain; microsecond-scale          |
| Compile cache LRU eviction                     | Low      | Local Mutex, held only during check-and-evict                          |
| ResolveGraphCache high concurrency             | Very low | DashMap: 64 shards, RwLock per shard, avg 1/64 contention              |
| HMR event during render cache write            | Low      | Different lock types (parking_lot vs tokio)                            |
| ISR revalidation set                           | Low      | Tokio Mutex held only for insert/remove; short section                 |

---

## Performance tuning levers

| Parameter                         | Where set          | Default             | Max               | Effect                                 |
| --------------------------------- | ------------------ | ------------------- | ----------------- | -------------------------------------- |
| `build.workers`                   | `ruvyxa.config.ts` | CPU count           | —                 | Parallel bundling & prerendering       |
| `RUVYXA_RENDER_CACHE_SIZE`        | Env var            | 1024 dev / 512 prod | 16384             | More cache = fewer SSR renders         |
| `RUVYXA_WORKER_TIMEOUT_MS`        | Env var            | 30000               | i32::MAX          | Timeout for stalled workers            |
| `RUVYXA_JSX_RUNTIME`              | Env var (auto-set) | automatic           | automatic/classic | JSX transform runtime                  |
| `security.actionRateLimit.max`    | Config             | varies              | —                 | Actions per window per key             |
| `security.actionRateLimit.window` | Config             | varies              | —                 | Rate limit window seconds              |
| `image.quality`                   | Config             | 82                  | 0-100             | WebP quality (lower = faster, smaller) |
| `image.parallelism`               | Config             | 0 (global)          | —                 | Dedicated image thread count           |

---

## Memory characteristics

| Component                        | Approx memory per unit                       |
| -------------------------------- | -------------------------------------------- |
| Compiled module (compiled cache) | ~50-200KB (JS string + deps)                 |
| Resolved module (resolve cache)  | ~10-100KB (source + deps)                    |
| Render cache entry               | ~5-500KB (HTML string)                       |
| Source file cache                | ~size of file (mmap for >64KB)               |
| Compile cache (disk)             | ~500KB-2MB per key (blake3-keyed JS on disk) |
| Worker process (Node)            | ~50-150MB per worker                         |

Total per-dev-session: ~200-500MB (4 workers + compile cache + render cache + source cache).
