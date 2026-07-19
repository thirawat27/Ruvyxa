# Middleware System (`ruvyxa_middleware`)

**Files**: `crates/ruvyxa_middleware/src/` (5 files)

Composable middleware stack via Tower `Layer`/`Service` pattern. Built-in layers: compression, CORS,
rate limiting, timing, logging, custom headers. Optional Wasmtime-based plugin runtime for
request/response phase plugins.

---

## Configuration Types (`config.rs`)

### `MiddlewareConfig`

```rust
pub struct MiddlewareConfig {      // #[serde(deny_unknown_fields)]
    pub builtin: BuiltinMiddlewareConfig,  // default
    pub layers: Vec<LayerConfig>,          // custom Tower layers (unsupported)
    pub plugins: Vec<PluginConfig>,        // Wasm plugins
}
```

### `BuiltinMiddlewareConfig`

```rust
pub struct BuiltinMiddlewareConfig {
    pub cors: Option<CorsConfig>,           // default None
    pub timing: bool,                       // default true
    pub logging: bool,                      // default true  (serde: "log")
    pub rate_limit: Option<RateLimitConfig>,// default None (serde: "rate")
    pub headers: BTreeMap<String, String>,  // default empty
}
```

### `CorsConfig`

```rust
pub struct CorsConfig {
    pub origins: Vec<String>,        // default empty; "*" for all
    pub methods: Vec<String>,        // default: GET, POST, PUT, DELETE, OPTIONS
    pub headers: Vec<String>,        // default empty
    pub credentials: bool,           // default false
    pub max_age: u64,                // default 86400
}
```

### `RateLimitConfig`

```rust
pub struct RateLimitConfig {
    pub max_requests: usize,         // serde: "max"
    pub window_secs: u64,            // serde: "window"
    pub key_by: String,              // default "ip", serde: "key"
}
```

### `LayerConfig`

```rust
pub struct LayerConfig {
    pub kind: String,                // Tower layer type name
    pub options: serde_json::Value,  // layer-specific config
}
// Currently UNSUPPORTED — validation rejects custom layers
```

### `PluginConfig`

```rust
pub struct PluginConfig {
    pub name: String,
    pub path: PathBuf,                       // .wasm file, project-relative, validated no ".."
    pub phase: PluginPhase,                  // default Request
    pub routes: Option<Vec<String>>,         // None → all routes; else prefix match (* suffix)
    pub config: serde_json::Value,           // plugin-specific JSON, default {}
    pub permissions: PluginPermissions,      // serde: "allow"
}

pub enum PluginPhase {
    #[default] Request,
    Response,
    // "Both" = configure two PluginConfigs, one per phase
}
```

### `PluginPermissions`

```rust
pub struct PluginPermissions {      // #[serde(deny_unknown_fields)]
    pub env: Vec<String>,            // allowed env vars
    pub fs_read: Vec<PathBuf>,       // REJECTED (serde: "read")
    pub net: Vec<String>,            // REJECTED
    pub timeout_ms: u64,             // default 5000
    pub max_memory_bytes: u64,       // default 67_108_864 (64MB)
}
```

---

## Middleware Stack (`stack.rs`)

### `MiddlewareStack`

```rust
pub struct MiddlewareStack {
    config: MiddlewareConfig,
}

impl MiddlewareStack {
    pub fn new(config: MiddlewareConfig) -> Self;
    pub fn apply<S>(&self, router: Router<S>) -> Router<S>;
    pub fn validate(&self) -> Result<(), String>;
    pub fn plugin_configs(&self) -> &[PluginConfig];
    pub fn count_builtin_layers(&self) -> usize;
}
```

### Application order (outermost first)

```
pub fn apply<S>(&self, router: Router<S>) -> Router<S>
```

1. **Compression** — `tower_http::CompressionLayer` with `CompleteBodyCompressionPredicate`
2. **CORS** — `CorsLayer::from_config(cors)`, only if `cors.is_some()`
3. **Rate Limiting** — `RateLimitLayerWithKey::from_config(rate)`, only if `rate.is_some()`
4. **Timing** — `TimingLayer`
5. **Request Logging** — `RequestLoggingLayer`
6. **Custom Headers** — `CustomHeadersLayer::new(&config.builtin.headers)`
7. **Custom Layers** — none supported yet (validated out)
8. **Wasm Plugin Layers** — if `wasm-plugins` feature enabled, wrap with plugin middleware

### `validate() → Result<(), String>`

Checks:

- No custom layers configured (`config.layers.is_empty()`)
- Custom header names are valid HTTP
- CORS: no `credentials: true` with `origins: ["*"]` (conflict)
- CORS: methods and headers are valid HTTP
- Rate limit: `max > 0`, `window > 0`, key is `"ip"` or `"header:<name>"` with valid header name
- Plugin: timeout > 0, max_memory > 0
- Plugin: no `fs_read` or `net` permissions (rejected)
- If `wasm-plugins` feature disabled and plugins configured → error

### `CompleteBodyCompressionPredicate`

```rust
struct CompleteBodyCompressionPredicate;

impl<B> Predicate<B> for CompleteBodyCompressionPredicate {
    fn should_compress(&self, response: &Response<B>) -> bool
    where B: http_body::Body
    {
        // Only compress if size_hint().exact() is Some (known exact size)
        response.body().size_hint().exact().is_some()
            && DEFAULT_PREDICATE.should_compress(response)
    }
}
```

Avoids compressing streaming/chunked responses where size is unknown.

---

## Built-in Middleware Implementations (`builtin.rs`)

All implement `tower::Layer` + inner `tower::Service`.

### `TimingLayer` / `TimingService<S>`

```rust
impl<S, B> Service<Request<B>> for TimingService<S>
where S: Service<Request<B>>
{
    async fn call(&self, req: Request<B>) -> Result<Response<B>, S::Error> {
        let start = Instant::now();
        let mut response = self.inner.call(req).await?;
        let elapsed = start.elapsed();
        response.headers_mut().insert(
            "X-Response-Time",
            format!("{}ms", elapsed.as_millis()).parse().unwrap(),
        );
        Ok(response)
    }
}
```

### `RequestLoggingLayer` / `RequestLoggingService<S>`

```rust
// Uses existing X-Request-ID header if present, otherwise generates "ruvyxa-<counter>"
// Injects X-Request-ID on both request extensions and response headers

tracing::info!(
    request_id = %request_id,
    method = %req.method(),
    path = %req.uri().path(),
    status = %status.as_u16(),
    duration_ms = %elapsed.as_millis(),
    "request"
);
```

### `CorsLayer` / `CorsService<S>`

**Preflight detection**:
`method == OPTIONS && req.headers().contains_key("Access-Control-Request-Method")`.

**Preflight response**: `204 No Content` with `Access-Control-Allow-Origin`, `Allow-Methods`,
`Allow-Headers`, `Max-Age`.

**Normal request**: passes through inner, adds `Access-Control-Allow-Origin` if origin allowed,
appends `Origin` to `Vary` header.

**Origin matching**: `"*"` → allow all. `Vec<String>` → exact match.

### `RateLimitLayerWithKey` / `RateLimitService<S>`

Token-bucket with window-based refill.

```rust
const MAX_TRACKED_RATE_LIMIT_KEYS: usize = 10_000;

struct RateLimitState {
    window_secs: u64,
    max_tokens: usize,
    tokens: HashMap<String, Vec<Instant>>,
}
```

- **`extract_key`**: if `key_by` starts with `"header:"`, read that header. Else use `SocketAddr`
  from extensions (set by Axum's `ConnectInfo`).
- **`allow(key)`**: prune expired entries, check `tokens[key].len() < max_tokens`, record hit.
- On limit: `429 Too Many Requests` + `Retry-After` header.

### `CustomHeadersLayer` / `CustomHeadersService<S>`

Parses `BTreeMap<String, String>` → `Vec<(HeaderName, HeaderValue)>` once at construction. Applies
to every response.

---

## Wasm Plugin Runtime (`wasm.rs`)

**Feature-gated**: `#[cfg(feature = "wasm-plugins")]` (default on).

### Data Types

```rust
pub struct PluginRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

pub struct PluginResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct PluginResult {
    pub action: String,                  // "continue" | "respond" | "modify-request" | "modify-response"
    pub request: Option<PluginRequest>,
    pub response: Option<PluginResponse>,
}
// Default: action="continue", request=None, response=None
```

### `WasmPluginRuntime`

```rust
pub struct WasmPluginRuntime {
    engine: Engine,                        // wasmtime::Engine (consume_fuel=true, no component model)
    plugins: Arc<RwLock<Vec<LoadedPlugin>>>,
}

struct LoadedPlugin {
    name: String,
    module: Module,                        // wasmtime::Module
    config_json: String,                   // serialized plugin config
    phase: PluginPhase,
    routes: Option<Vec<String>>,
    permissions: PluginPermissions,
}
```

### `WasmPluginRuntime::new(project_root, configs) → Result<Self>`

1. Create `wasmtime::Engine`:
   - `consume_fuel(true)`
   - No component model
2. For each `PluginConfig`:
   - Validate path: relative, no `..`, resolve inside project root, extension `.wasm`.
   - Read `.wasm` bytes.
   - `Module::new(&engine, &wasm_bytes)` — pre-compile.
   - Serialize `config` field to JSON string.
   - Push `LoadedPlugin { name, module, config_json, phase, routes, permissions }`.

### Route matching

If plugin `routes` is `None` → apply to all requests. If `routes` is `Some(vec)`:

- Exact match: path == route
- Wildcard match: route ends with `*`, path starts with prefix

### `execute_request_plugins(request)`

Iterates request-phase plugins matching route. Spawns blocking task per plugin (Wasm execution is
non-async). Calls `invoke_on_request()`. First non-continue result wins.

### `execute_response_plugins(request, response)`

Same for response-phase plugins. Calls `invoke_on_response()`.

### Sandbox creation (`create_store`)

```rust
fn create_store(
    engine: &Engine,
    permissions: &PluginPermissions,
    project_root: &Path,
) -> Result<Store<PluginStore>>
```

1. `WasiCtxBuilder::new()`:
   - Inject only permitted env vars (from `permissions.env`).
   - No filesystem preopens (rejected at validation).
   - No network access (rejected at validation).
2. `StoreLimitsBuilder::new()`:
   - `memory_size(permissions.max_memory_bytes)`.
   - No other limits.
3. Set fuel budget: `permissions.timeout_ms * 1_000_000` fuel units.
4. Create `Store` with WASI context + limits.
5. Instantiate module, verify exports: `memory` + `on_request`/`on_response`.

### Function invocation

**Expected exports**: `memory: Memory`, `on_request: func(i32, i32) -> i32` (or `on_response`).

**Call protocol**:

```rust
fn call_plugin_func(
    store: &mut Store<PluginStore>,
    instance: &Instance,
    func_name: &str,
    input_json: &str,
) -> Result<PluginResult>
```

1. Get plugin's `memory` export.
2. Write serialized JSON into memory at offset 0:
   ```rust
   memory.write(store, 0, input_json.as_bytes())?;
   ```
3. Get exported function by name (`on_request` or `on_response`).
4. Call: `func.call(store, &[Val::I32(0), Val::I32(input_json.len() as i32)], &mut [Val::I32(0)])`.
5. Parse result: `result_ptr` → read from memory as NUL-terminated UTF-8 JSON.
6. If result empty → return `PluginResult::default()` (continue).
7. Parse JSON → `PluginResult`.

### Result reading (`read_nul_terminated_result`)

```rust
fn read_nul_terminated_result(
    memory: &Memory, store: &mut Store<PluginStore>, result_ptr: i32
) -> Result<Vec<u8>>
```

- Maximum result size: 1 MB + 1 byte (to detect NUL at exact limit).
- Reads in 4 KB chunks from `result_ptr` as `i32`.
- Scans for NUL byte (`0x00`).
- Returns error if result exceeds 1 MB or has no NUL terminator.

**Fuel exhaustion**: if plugin consumes all fuel (timeout * 1M units), `call` returns
`wasmtime::Error` → mapped to `RUV2101`.

### Plugin WIT interface (documented)

```wit
interface ruvyxa-plugin {
    type http-request = record {
        method: string,
        path: string,
        headers: list<tuple<string, string>>,
        body: option<list<u8>>,
    };

    type http-response = record {
        status: u16,
        headers: list<tuple<string, string>>,
        body: list<u8>,
    };

    type plugin-result = record {
        action: string,
        request: option<http-request>,
        response: option<http-response>,
    };

    on-request: func(req: http-request, config: string) -> plugin-result;
    on-response: func(req: http-request, res: http-response, config: string) -> plugin-result;
}
```

### Input JSON format

```json
{
  "request": {
    "method": "GET",
    "path": "/about",
    "headers": [
      ["accept", "text/html"],
      ["cookie", "session=abc"]
    ],
    "body": null
  },
  "config": { "key": "value" }
}
```

### Result JSON (actions)

| Action              | Effect                               |
| ------------------- | ------------------------------------ |
| `"continue"`        | No-op, request passes through        |
| `"respond"`         | Short-circuit with provided response |
| `"modify-request"`  | Update method, path, headers, body   |
| `"modify-response"` | Update status, headers, body         |

### Error handling

| Code    | Condition                                                                        |
| ------- | -------------------------------------------------------------------------------- |
| RUV2100 | Wasm load error: file not found, invalid wasm, missing exports                   |
| RUV2101 | Wasm execution error: trap, fuel exhausted, memory out of bounds, invalid result |

---

## Validation Rules Summary

| Rule                                           | Error                                         |
| ---------------------------------------------- | --------------------------------------------- |
| `custom_layers` not empty                      | "Custom layers are not yet supported"         |
| `custom_headers` invalid name                  | "Invalid header name: .."                     |
| `custom_headers` invalid value                 | "Invalid header value: .."                    |
| `cors.credentials=true` + `cors.origins=["*"]` | "Cannot use credentials with wildcard origin" |
| `cors.methods` invalid                         | "Invalid CORS method"                         |
| `cors.headers` invalid                         | "Invalid CORS header"                         |
| `rate_limit.max` ≤ 0                           | "Rate limit max must be positive"             |
| `rate_limit.window` ≤ 0                        | "Rate limit window must be positive"          |
| `rate_limit.key` invalid header                | "Invalid header name for rate limit key"      |
| `plugin.timeout_ms` ≤ 0                        | "Plugin timeout must be positive"             |
| `plugin.max_memory_bytes` ≤ 0                  | "Plugin memory limit must be positive"        |
| `plugin.fs_read` non-empty                     | "Plugin filesystem access is not supported"   |
| `plugin.net` non-empty                         | "Plugin network access is not supported"      |
| `wasm-plugins` disabled + plugins configured   | "Wasm plugin support is not enabled"          |
