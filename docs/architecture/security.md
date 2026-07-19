# Security Model

Security boundaries and enforcement across request handling, environment variables, and plugin
execution.

---

## Environment Variable Isolation

### Rule

```
RUVYXA_PUBLIC_*  →  embedded in client bundles, visible in browser
All other vars   →  server-only, never compiled into client
```

### Enforcement layers

| Layer        | When                                                     | Mechanism                                                                                                                              |
| ------------ | -------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Graph-level  | `ruvyxa_graph::validate_app()` — after route discovery   | Source scan for `process.env.<NAME>` and `process.env['<NAME>']`. Rejects non-`RUVYXA_PUBLIC_*` in client-reachable modules → RUV1008. |
| Bundle-level | `ruvyxa_bundler::boundary::check()` — during compilation | Same scan on compiled JS output. Second pass after transforms.                                                                         |
| Runtime      | `ruvyxa.config.ts` eval                                  | Only `RUVYXA_PUBLIC_*` accessible via `defineConfig()` when config is evaluated by Node for client-visible values.                     |

### Implementation: `private_env_reads(source)`

Byte-level scanner that recognizes:

- `process.env.NAME` → captures `NAME`
- `process.env["NAME"]` or `process.env['NAME']` → captures `NAME`

Handles:

- String literals (skipped, but `${expr}` recursed into for template literals)
- Template literals (depth counter for nested expressions)
- Block comments `/* */` and line comments `//`

Exemptions:

- `process.env.NODE_ENV` — always allowed (build-time folding)
- `process.env.RUVYXA_PUBLIC_*` — allowed (explicitly public)

### Example violation

```typescript
// app/components/secret.tsx — imported by client page
const apiKey = process.env.MY_API_KEY // ← RUV1008
```

### Fix

```typescript
// Option A: move to server-only
// server/api.ts
const apiKey = process.env.MY_API_KEY

// Option B: make public (only if safe)
const apiKey = process.env.RUVYXA_PUBLIC_API_KEY
```

---

## Server/Client Boundary

Two-level enforcement prevents server code from leaking into client bundles.

### Level 1: Graph validation (`ruvyxa_graph::validate_app`)

Source-dcan on every route after discovery.

| Violation                                       | Detection                                                      | Code    |
| ----------------------------------------------- | -------------------------------------------------------------- | ------- |
| `import "server-only"` in client-reachable code | Text scan for `import "server-only"` or `import 'server-only'` | RUV1007 |
| Private `process.env.*` in client graph         | `private_env_reads()` on all client-reachable source           | RUV1008 |
| `server/` directory import in client graph      | File path starts with `<root>/server/` after canonicalization  | RUV1010 |
| `import "client-only"` in server/API code       | Text scan for `import "client-only"`                           | RUV1009 |

### Level 2: Bundle boundary (`ruvyxa_bundler::boundary::check`)

Re-checks on compiled JS output after transforms (re-export patterns could bypass source-only
checks).

### `server/` directory rule

Only the project-root `server/` directory is checked:

```
project/
├── server/          ← CHECKED: imports from here → RUV1010
│   └── db.ts
├── app/
│   └── blog/
│       └── server.ts  ← NOT checked by RUV1010 (app-internal)
```

---

## Request Validation

### Path canonicalization

`canonical_request_path(path)`:

- Splits by `/`, percent-decodes each segment (`percent_encoding::percent_decode_str`)
- Rejects: empty segments, `.`, `..`, decoded `/` or `\`, control characters (0x00–0x1F)
- Rejects: malformed percent encoding (invalid hex, truncation)

Prevents: path traversal (`/../../../etc/passwd`), null byte injection, CRLF injection.

### Body size limits

| Endpoint         | Config key             | Default      | Check point                           |
| ---------------- | ---------------------- | ------------ | ------------------------------------- |
| Action POST      | `security.actionLimit` | Configurable | Before body read in `action_endpoint` |
| API POST/PUT/etc | `security.apiLimit`    | Configurable | Before body read in `handle_request`  |
| Plugin response  | `security.pluginLimit` | Configurable | After plugin execution                |

Returns `413 Payload Too Large` on violation.

### Content-Type validation

Action endpoint validates:

- Content-Type header present and valid
- Body is valid JSON (if `application/json`) or valid form data

---

## Same-Origin Protection

### Action endpoint (`same_origin_actions`)

When `same_origin_actions: true`:

```rust
fn validate_same_origin(config: &ServerConfig, headers: &HeaderMap) -> Result<()> {
    let origin = headers.get("origin").ok_or(INVALID_ORIGIN)?;
    let expected = format!("{}:{}", config.host, config.port);
    if origin != expected {
        return Err(FORBIDDEN);
    }
    Ok(())
}
```

### Sec-Fetch-Metadata (`fetch_metadata_actions`)

When `fetch_metadata_actions: true`:

```rust
fn validate_sec_fetch(headers: &HeaderMap) -> Result<()> {
    let site = headers.get("sec-fetch-site").ok_or(FORBIDDEN)?;
    if site != "same-origin" {
        return Err(FORBIDDEN);
    }
    Ok(())
}
```

---

## Rate Limiting

### Two-tier architecture

| Tier            | Layer                     | Config                          | Key                         |
| --------------- | ------------------------- | ------------------------------- | --------------------------- |
| HTTP middleware | All requests (dev server) | `middleware.builtin.rate_limit` | `"ip"` or `"header:<name>"` |
| Action-specific | POST `/__ruvyxa/action`   | `security.actionRateLimit`      | Complex: IP + header + path |

### Token-bucket implementation

```rust
struct TokenBucket {
    hits: HashMap<String, Vec<Instant>>,   // sliding window
    max_tokens: usize,
    window: Duration,
    max_keys: usize,                        // 10,000
}

impl TokenBucket {
    fn allow(&mut self, key: &str) -> bool {
        let hits = self.hits.entry(key.to_string()).or_default();
        // Prune expired hits (older than window)
        hits.retain(|t| t.elapsed() < self.window);
        if hits.len() >= self.max_tokens {
            return false;
        }
        hits.push(Instant::now());
        true
    }
}
```

### Key management

- At 10,000 tracked keys: full sweep to evict all expired entries before inserting new.
- Expired entries pruned lazily on `allow()` calls.
- `Retry-After` header returned on rate limit: seconds until oldest hit expires.

### Response on limit

```
HTTP 429 Too Many Requests
Retry-After: <seconds>
```

---

## Security Headers

Applied as Axum middleware on every response:

| Header                   | Value                                      | Configurable?  |
| ------------------------ | ------------------------------------------ | -------------- |
| `X-Content-Type-Options` | `nosniff`                                  | No (always on) |
| `X-Frame-Options`        | `DENY`                                     | No (always on) |
| Custom headers           | Configured in `middleware.builtin.headers` | Yes            |

Enabled/disabled via `security.headers` config (default depends on build).

---

## Trusted Proxy IPs

When behind a reverse proxy, `security.trustedProxyIps` configures which IPs to trust for:

```rust
fn determine_client_ip(
    config: &ServerConfig,
    remote_addr: SocketAddr,
    headers: &HeaderMap,
) -> SocketAddr {
    if config.trusted_proxy_ips.contains(&remote_addr.ip()) {
        // Trust X-Forwarded-For
        if let Some(forwarded) = headers.get("x-forwarded-for") {
            // Use leftmost (original client) IP
            return parse_first_ip(forwarded);
        }
    }
    remote_addr
}
```

Also used for `X-Forwarded-Proto` (HTTPS detection).

---

## Plugin Sandbox

### Capability deny-list (rejected at validation)

| Capability       | Config field          | Status                                       |
| ---------------- | --------------------- | -------------------------------------------- |
| Filesystem read  | `permissions.fs_read` | **REJECTED** — returns validation error      |
| Filesystem write | (no config)           | Not possible (no `preopens`)                 |
| Network access   | `permissions.net`     | **REJECTED** — returns validation error      |
| All env vars     | (no config)           | Not possible (only configured ones injected) |

### Capability allow-list

| Capability        | Config field                        | Default            |
| ----------------- | ----------------------------------- | ------------------ |
| Specific env vars | `permissions.env: Vec<String>`      | `[]` (no env vars) |
| CPU time          | `permissions.timeout_ms: u64`       | `5000`             |
| Maximum memory    | `permissions.max_memory_bytes: u64` | `67108864` (64MB)  |
| Result size       | (hardcoded)                         | 1 MB               |

### Execution sandboxing

```rust
// Engine with fuel metering
let engine = Engine::new(Config::new().consume_fuel(true))?;

// Per-invocation store
let mut store = Store::new(&engine, PluginStore { ... });

// Memory limit
store.limiter(|s| &mut s.limits).memory_size(max_memory_bytes);

// Fuel budget (= CPU time budget)
store.set_fuel(timeout_ms * 1_000_000)?;

// WASI context with only allowed env vars
let wasi = WasiCtxBuilder::new()
    .env("ALLOWED_VAR", "value")  // only configured vars
    .build();
store.data_mut().wasi = wasi;
```

### Plugin path validation

```rust
fn validate_plugin_path(project_root: &Path, plugin_path: &Path) -> Result<PathBuf> {
    // Must be relative
    if plugin_path.is_absolute() { bail!("absolute path not allowed"); }
    // No parent traversal
    if plugin_path.components().any(|c| c == Component::ParentDir) {
        bail!("'..' not allowed in plugin path");
    }
    // Must be .wasm extension
    if plugin_path.extension() != Some("wasm") {
        bail!("plugin must be .wasm file");
    }
    // Resolve inside project root
    let full = project_root.join(plugin_path);
    full.canonicalize().ok()
}
```

---

## Configuration Security

### Path validation

All configured paths (`appDir`, `outDir`, `css.entries[*]`) must:

- Be relative (no absolute paths → `C:\` or `/`)
- Not traverse above project root (no `..`)
- Not be the project root itself

Enforced in `ProjectConfig::validate_paths()`.

### Limit validation

All configured limits must be within safe bounds:

- Body limits: `> 0` and `≤ MAX_BODY_LIMIT`
- Rate limits: `max > 0`, `window > 0`
- Plugin limits: `timeout_ms > 0`, `max_memory > 0`

### Config immutability

`#[serde(deny_unknown_fields)]` on all config structs — no silent defaults for typos.

---

## Build-Time Security

### Staging + atomic commit

Build writes to `out_dir/.ruvyxa-staging-<random>`. On success: renames old `out_dir` →
`out_dir.old`, stging → `out_dir`, removes `.old`. On failure: staging cleanup, existing output
preserved.

### No secrets in build output

- Client bundles exclude `process.env.<private>` references (enforced at compile).
- Server bundles contain server-only app code (not browser-accessible in production).
- `build.json` contains no source code or env vars.

### Dependency hash

`config_dependency_hash = blake3(config + config dependencies)`. Used for:

- Compile cache key namespace
- Prerender artifact cache validation
- Build cache invalidation when config changes
