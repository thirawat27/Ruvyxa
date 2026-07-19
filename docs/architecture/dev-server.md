# Dev Server (`ruvyxa_dev_server`)

**Files**: `crates/ruvyxa_dev_server/src/` (6 files, ~8,000 lines)

Am server powered by Axum + Tokio. Handles HTTP requests, WebSocket HMR, route matching via Radix
trie, render caching, Node worker pool management, and style collection.

---

## Configuration Types

### `ServerConfig`

```rust
pub struct ServerConfig {
    pub root: PathBuf,                          // Project root
    pub app_dir: PathBuf,                       // root/app (dev) or out_dir/server/app (prod)
    pub public_dir: PathBuf,                    // root/public (dev) or out_dir/assets (prod)
    pub client_dir: PathBuf,                    // root/.ruvyxa/client
    pub prerender_dir: Option<PathBuf>,         // root/.ruvyxa/prerender
    pub host: String,                           // default "0.0.0.0"
    pub port: u16,                              // default 3000
    pub watch: bool,                            // true = dev mode
    pub cache_route_manifest: bool,
    pub cache_css: bool,
    pub style_entries: Vec<PathBuf>,            // additional CSS entry points
    pub prebundle_dependencies: bool,
    pub jsx_runtime: JsxRuntime,
    pub error_overlay: bool,                    // show diagnostic overlay in browser
    pub debug_traces: bool,
    pub action_body_limit_bytes: usize,
    pub api_body_limit_bytes: usize,
    pub plugin_response_body_limit_bytes: usize,
    pub action_rate_limit_max: usize,
    pub action_rate_limit_window: Duration,
    pub same_origin_actions: bool,
    pub fetch_metadata_actions: bool,
    pub trusted_proxy_ips: Vec<IpAddr>,
    pub security_headers: bool,
    pub middleware: MiddlewareConfig,
    pub default_render_strategy: Option<RenderStrategy>,
    pub default_revalidate: Option<u64>,
}
```

### `AppState`

```rust
struct AppState {
    config: ServerConfig,
    reload_tx: broadcast::Sender<String>,              // HMR WebSocket fan-out
    runtime_cache: Arc<RuntimeCache>,                   // manifest, router, CSS
    action_limiter: Arc<Mutex<ActionRateLimiter>>,
    worker_pool: Arc<NodeWorkerPool>,
    render_cache: Arc<RenderCache>,
    isr_revalidating: Arc<tokio::sync::Mutex<HashSet<String>>>,
    hmr_tracker: Arc<HmrTracker>,
    plugin_runtime: Option<Arc<WasmPluginRuntime>>,
}

struct RuntimeCache {
    manifest: tokio::sync::RwLock<Option<RouteManifest>>,
    styles: tokio::sync::RwLock<Option<StyleCacheEntry>>,
    router: tokio::sync::RwLock<Option<RadixRouter>>,
}

struct StyleCacheEntry {
    css: String,
    files: BTreeSet<PathBuf>,   // normalized, case-folded on Windows
}
```

---

## `serve(config) → Result<()>` — Full Startup Sequence

### 1. Validate limits

```rust
config.validate_limits()  // body limits > 0, rate limits > 0
```

### 2. Route discovery

```rust
let manifest = discover_routes(discover_options(&config))?;
```

### 3. Broadcast channel

```rust
let (reload_tx, _) = broadcast::channel::<String>(64);  // capacity 64, drops oldest
```

### 4. Runtime environment

```rust
let env = runtime_env(&config);
// Loads .env + .env.local from project root
// Inserts RUVYXA_JSX_RUNTIME=automatic|classic
```

### 5. Node worker pool

```rust
let worker_pool = NodeWorkerPool::start(&config.root, env).await?;
```

### 6. Pre-bundle warmup (dev only)

```rust
if config.watch && config.prebundle_dependencies {
    let warmup_pool = worker_pool.clone();
    tokio::spawn(async move {
        warmup_pool.warmup(warmup_root, warmup_routes).await;
    });
}
```

### 7. Render cache

```rust
let render_cache = if config.watch {
    RenderCache::default_dev()   // capacity=1024, TTL=300s
} else {
    RenderCache::default_production()  // capacity=512, TTL=1800s
};
```

Capacity also configurable via `RUVYXA_RENDER_CACHE_SIZE` env var (capped at 16384).

### 8. HMR tracker

```rust
let hmr_tracker = Arc::new(HmrTracker::new());
hmr_tracker.populate_from_manifest(&manifest.routes);
```

### 9. Middleware & plugins

```rust
let middleware_stack = MiddlewareStack::new(config.middleware.clone());
middleware_stack.validate()?;

let plugin_runtime = if !config.middleware.plugins.is_empty() {
    Some(Arc::new(WasmPluginRuntime::new(&config.root, &config.middleware.plugins)?))
} else {
    None
};
```

### 10. Axum Router

```rust
let state = Arc::new(AppState { ... });

let router = Router::new()
    .route("/__ruvyxa/hmr", get(hmr_ws))
    .route("/__ruvyxa/client", get(client_bundle))
    .route("/__ruvyxa/action",
        post(action_endpoint)
            .layer(DefaultBodyLimit::max(config.action_body_limit_bytes)))
    .route("/__ruvyxa/trace", get(trace_endpoint))
    .fallback(handle_request)
    .with_state(state.clone());
```

Then applied middleware stack layers (compression, CORS, rate limiting, timing, logging, headers,
custom, plugins) + security headers.

### 11. Bind listener

```rust
let mut port = config.port;
let listener = loop {
    match TcpListener::bind((config.host, port)).await {
        Ok(l) => break l,
        Err(_) if port < config.port + 100 => port += 1,
        Err(e) => return Err(e.into()),
    }
};
// Port fallback: try up to config.port + 100
```

### 12. Graceful shutdown

```rust
let (shutdown_tx, shutdown_rx) = watch::channel(false);

axum::serve(listener, router)
    .with_graceful_shutdown(async {
        tokio::signal::ctrl_c().await.ok();
        // OR watch channel true
    })
    .await?;

// After shutdown:
worker_pool.shutdown().await;  // 5s grace period
```

---

## Request Lifecycle (`handle_request`)

### 1. Parse canonical path

```rust
fn canonical_request_path(path: &str) -> Result<String, StatusCode>
```

- Split by `/`.
- Percent-decode each segment (`percent_encoding::percent_decode_str`).
- Reject: empty segments, `.`, `..`, decoded `/` or `\`, control characters (0x00-0x1F).
- Reject: malformed percent encoding (invalid hex, truncated).

### 2. Read body

```rust
if request.method() != Method::GET && request.method() != Method::HEAD {
    let body_bytes = axum::body::to_bytes(body, config.api_body_limit_bytes)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
}
```

### 3. Request-phase Wasm plugins

```rust
if let Some(runtime) = &state.plugin_runtime {
    if let Some(result) = runtime.execute_request_plugins(&req_parts).await {
        if result.action == "respond" {
            return result.response.into();  // Short-circuit
        }
        // Apply modifications to request parts
    }
}
```

### 4. Render dispatch

```rust
// Try static files first
if let Some(resp) = serve_client_file(&state, &path).await {
    return resp;
}
if let Some(resp) = serve_public_file(&state, &path, &req).await {
    return resp;
}

// Route lookup
let router = state.runtime_cache.router().await;
let Some(route_match) = router.find(&path) else {
    return 404 plain error page;
};

match route_match.route.kind {
    RouteKind::Page => render_page_by_strategy(&state, &route_match, body).await,
    RouteKind::Api  => render_api_pooled(&state, &route_match, method, headers, body).await,
}
```

### 5. HTML composition (Page routes)

```rust
fn compose_document(rendered: &str, head_content: &str, hmr: &str) -> String
```

Algorithm (all tag searches case-insensitive):

1. If `<html` found:
   - If `<head` found → inject `head_content` before `</head>`.
   - Else if `<body` found → insert `<head>{head_content}</head>` before `<body>`.
   - Else → insert `<head>{head_content}</head>` after opening `<html>` tag.
   - Inject `hmr` before `</body>`.
2. Otherwise → wrap in full HTML scaffold:
   ```html
   <!doctype html>
   <html lang="en">
     <head>
       <meta charset="utf-8" />
       <meta name="viewport" content="width=device-width,initial-scale=1" />
       {head_content}
     </head>
     <body>
       {rendered}{hmr}
     </body>
   </html>
   ```

**Head content**: `<link rel="icon" href="/ruvyxa.png">` + `<style data-ruvyxa-css>{css}</style>`

**HMR content**: `<script>/* WebSocket HMR */</script>` (dev only)

**Client hydration**: Injects `__RUVYXA_ROUTE_PARAMS__`, `__RUVYXA_REQUEST_PATH__`, preload hints,
`<script type="module" src="/__ruvyxa/client?path=...">`.

### 6. Response-phase plugins

```rust
if let Some(runtime) = &state.plugin_runtime {
    if let Some(result) = runtime.execute_response_plugins(&req, &resp).await {
        if result.action == "respond" { return result.response.into(); }
        // Apply modifications
    }
}
```

### 7. Security headers

```rust
fn finalize_security_headers(response: Response) -> Response {
    response.headers_mut().insert("X-Content-Type-Options", "nosniff");
    response.headers_mut().insert("X-Frame-Options", "DENY");
    // ... configured headers
}
```

---

## Render Strategies

### SSR (`render_page_ssr`)

```
1. RenderCache::get(ssr_cache_key(path, params))
2. On miss: worker_pool.render_ssr() → compose HTML → RenderCache::put()
3. Return cached/rendered HTML
```

```rust
pub fn ssr_cache_key(request_path: &str, params: &RouteParams) -> String {
    if params.is_empty() {
        format!("ssr:{}", request_path)
    } else {
        format!("ssr:{}?{}", request_path, serde_json::to_string(params).unwrap())
    }
}
```

### SSG (`render_page_ssg`)

```
1. Production: check prerender_dir/<path>/index.html
2. Dev: RenderCache::get(ssg_cache_key)
3. On miss: worker_pool.render_ssg(mode="full")
4. Cache indefinitely (no TTL, kept until invalidated)
```

### ISR (`render_page_isr`)

```
1. RenderCache::get_stale_with_age(isr_cache_key) → (value, age)
2. If cached:
   - If age >= revalidate_seconds: spawn_isr_revalidation()
   - Serve stale value (even if expired)
3. If not cached:
   - Production prerender: check prerender_dir
   - Dev: render synchronously
4. Return HTML
```

**Revalidation coalescing**:

```rust
pub async fn spawn_isr_revalidation(
    state: &AppState, key: String, ...
) {
    let mut in_flight = state.isr_revalidating.lock().await;
    if in_flight.contains(&key) {
        return;  // Already revalidating
    }
    in_flight.insert(key.clone());
    drop(in_flight);

    let state = Arc::clone(state);
    tokio::spawn(async move {
        let html = render_isr_background(&state, ...).await;
        state.render_cache.put(&key, &html).await;
        state.isr_revalidating.lock().await.remove(&key);
    });
}
```

### CSR (`render_page_csr`)

Returns minimal HTML shell:

```html
<!doctype html>
<html lang="en">
  <head>
    ...{head_content}...
  </head>
  <body>
    <div id="__ruvyxa"></div>
    {hmr}{client_hydration}
  </body>
</html>
```

### PPR (`render_page_ppr`)

```rust
worker_pool.render_ssg(mode="ppr")  // static shell → dynamic slots streamed
```

---

## Endpoint Handlers

### `GET /__ruvyxa/hmr` → WebSocket

```rust
async fn hmr_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(|mut socket| async move {
        let mut rx = state.reload_tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if socket.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}
```

### `GET /__ruvyxa/client?path=` → JS bundle

```rust
async fn client_bundle(
    State(state): State<Arc<AppState>>,
    Query(ClientBundleQuery { path }): Query<ClientBundleQuery>,
) -> Response {
    match render_client_bundle_pooled(&state, &path).await {
        Ok(js) => (
            StatusCode::OK,
            [("content-type", "text/javascript; charset=utf-8")],
            js,
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/javascript; charset=utf-8")],
            format!("console.error({});", serde_json::to_string(&e.to_string()).unwrap()),
        ).into_response(),
    }
}
```

### `POST /__ruvyxa/action?path=&name=` → Server action

```rust
async fn action_endpoint(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(ActionQuery { path, name }): Query<ActionQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1. Validate request
    validate_action_request(&state.config, &headers, &body)?;

    // 2. Parse payload
    validate_action_payload(&headers, &body)?;

    // 3. Rate limit
    let key = action_rate_limit_key(peer, &headers, &path, &name);
    if !state.action_limiter.lock().unwrap().allow(&key) {
        let retry = state.action_limiter.lock().unwrap().retry_after_seconds(&key);
        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", retry.to_string())]).into_response();
    }

    // 4. Execute
    let result = state.worker_pool.render_action(
        &state.config.root, &path, &name,
        payload_json, content_type,
    ).await;

    match result {
        Ok(response) => response.into(),
        Err(e) => error_response(e),
    }
}
```

**`validate_action_request`** checks:

- Body size ≤ `action_body_limit_bytes`
- Content-Type is valid (JSON or form)
- `same_origin_actions` → validate `Origin` header matches server origin
- `fetch_metadata_actions` → validate `Sec-Fetch-Site` is `same-origin`

### `GET /__ruvyxa/trace?path=`

```rust
async fn trace_endpoint(
    State(state): State<Arc<AppState>>,
    Query(TraceQuery { path }): Query<TraceQuery>,
) -> Response {
    if !state.config.watch || !state.config.debug_traces {
        return StatusCode::NOT_FOUND.into_response();
    }
    let manifest = state.runtime_cache.manifest.read().await;
    let route = manifest.as_ref()
        .and_then(|m| m.routes.iter().find(|r| r.path == path));
    serde_json::to_value(route).unwrap().into_response()
}
```

---

## Dev Error Overlay

When a `Diagnostic` error occurs in dev mode:

```rust
fn dev_diagnostic_overlay(diag: &Diagnostic) -> Response {
    let frame = extract_code_frame(&diag.span);  // 5 lines around error
    render_error_overlay(ErrorOverlayView {
        code: diag.code,
        title: diag.title,
        location: diag.span.as_ref().map(span_to_string),
        detail: &diag.explanation,
        code_frame: frame,
        suggested_fix: &diag.suggested_fix,
        import_chain: &diag.import_chain,
        affected_routes: &diag.affected_routes,
    })
}
```

**`render_error_overlay`** produces a full HTML page with:

- Dark backdrop with blur
- Card dialog: error code (red badge), title, location
- Source code frame (dark terminal-style, error line marked with `>` and `← error`)
- Suggested fix (green box with lightbulb)
- Import chain (collapsible accordion)
- Affected routes (collapsible)
- Stack trace (collapsible)
- Close button (hides overlay, shows page underneath)

All values HTML-escaped via `escape_html()`.

---

## File Watcher (dev mode)

Uses `notify::recommended_watcher()`. Watches project root.

### Event filter

```rust
fn is_ignored(path: &Path) -> bool {
    path_contains(path, ".git") ||
    path_contains(path, ".ruvyxa") ||
    path_contains(path, "target") ||
    path_contains(path, "dist") ||
    path_contains(path, ".npm-pack") ||
    path_contains(path, ".npm-smoke") ||
    path_contains(path, "node_modules") ||
    path_starts_with(path, ".ruvyxa-")
}
```

### Event processing

```rust
fn handle_change(state: &AppState, paths: Vec<PathBuf>) {
    // Filter Access events, filter ignored paths
    let update = hmr_tracker.compute_update(&paths);

    if update.full_reload {
        runtime_cache.invalidate();
        render_cache.invalidate_all_blocking();
    } else {
        runtime_cache.invalidate_styles_for_paths(&paths);
        for route_path in &update.affected_routes {
            render_cache.invalidate_route_blocking(route_path);
        }
    }

    // Notify workers about changed files
    worker_pool.invalidate_from_watcher(&path_strings);

    // Broadcast to browsers
    let payload = serde_json::json!({
        "type": update.event_type.as_str(),
        "paths": paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "affectedRoutes": update.affected_routes,
        "fullReload": update.full_reload,
    });
    let _ = reload_tx.send(payload.to_string());
}
```

Worker invalidation uses `try_send()` (non-blocking, file watcher callback has no tokio runtime). If
fails → force full reload.

---

## Public File Serving

```rust
async fn serve_public_file(state: &AppState, path: &str, req: &Request) -> Option<Response>
```

1. `public_dir.join(path.trim_start_matches('/'))`.
2. Check file exists, is file (not directory).
3. Read file + compute blake3 ETag.
4. Conditional GET: if `If-None-Match` matches ETag → return `304 Not Modified`.
5. Determine MIME type from extension.
6. Return with `cache-control: public, max-age=31536000, immutable` + ETag.

MIME types: `.js`→text/javascript, `.css`→text/css, `.html`→text/html, `.json`→application/json,
`.svg`→image/svg+xml, `.png`/`.jpg`/`.jpeg`/`.webp`/`.ico`→image/*, `.woff2`→font/woff2, etc.

---

## Action Rate Limiter

```rust
struct ActionRateLimiter {
    hits: HashMap<String, Vec<Instant>>,   // key → hit timestamps
    max_hits: usize,
    window: Duration,
    max_keys: usize,                        // 10,000 — evict expired on insert
}

impl ActionRateLimiter {
    fn allow(&mut self, key: &str) -> bool {
        // 1. Get or insert key's hits vec
        // 2. Prune expired hits (outside window)
        // 3. If hits.len() >= max_hits: return false
        // 4. Push Instant::now(), return true
    }

    fn retry_after_seconds(&self, key: &str) -> u64 {
        // Seconds until oldest hit falls out of window
    }
}
```

Key format depends on config:

- `key_by: "ip"` → `peer_addr.to_string()`
- `key_by: "header:<name>"` → `headers.get(name).to_str()`

---

## Radix Router

See [RadixRouter](#radix-router-internals) for trie implementation details: compilation from
RouteManifest, segment classification, lookup algorithm, param extraction, static-vs-dynamic
priority.

---

## Render Cache

See [render_cache.rs](#render-cache-internals) for caching details: LRU implementation, TTL
expiration, cache key formats, ISR stale-while-revalidate, blockng invalidation for file watcher.

---

## HMR Tracker

See [hmr_tracker.rs](#hmr-tracker-internals) for bidirectional maps, compute_update algorithm, event
type determination (CssUpdate vs ComponentUpdate vs FullReload).

---

## Style Collection

See [style.rs](#style-collection-internals) for import-graph-driven CSS collection, Sass
compilation, Tailwind integration, CSS module scoping, minification.

---

## Worker Pool

See [Worker Pool doc](worker-pool.md) for complete internals: pool initialization, NDJSON protocol,
streaming API responses, failure recovery, bundle cache invalidation.
