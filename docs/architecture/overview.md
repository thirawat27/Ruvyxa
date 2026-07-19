# Ruvyxa System Architecture

> Engineering reference — complete crate maps, data flow, algorithms, concurrency model, wire
> protocols, and contracts.

---

## 1. System Architecture & Design Decisions

```
Rust CLI + Rust toolchain (bundler, server, graph)
    + Node.js worker pool (React SSR, API execution, config eval)
    = Ruvyxa
```

| Decision                               | Rationale                                                                                                                        |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| **Rust core, Node renderers**          | Rust: fast startup, compile-time safety, single binary. Node: React ecosystem. Workers eliminate per-request spawn (~100-500ms). |
| **Oxc for TS/JSX** (not Babel/SWC/TSC) | 10-100x faster. Single binary deployment. No Node dependency for bundling.                                                       |
| **Persistent Node worker pool**        | Pool: 2-8 workers, default CPU count. NDJSON over stdin/stdout. Avoids per-request subprocess overhead.                          |
| **Radix trie router**                  | O(path_depth) matching vs O(n) linear scan. Recompiled on RouteManifest change.                                                  |
| **Content-hashed assets**              | Blake3 fingerprints → immutable caching (max-age=31536000).                                                                      |
| **Staging + atomic commit**            | Partial/crashed builds never corrupt existing output.                                                                            |

---

## 2. Crate Dependency Graph

```
ruvyxa_diagnostics     (foundation, deps: serde, thiserror only)
    ↑
    ├── ruvyxa_graph   (deps: diagnostics)
    ├── ruvyxa_bundler (deps: diagnostics, oxc, grass, dashmap, rayon, memmap2, blake3)
    ├── ruvyxa_middleware (deps: diagnostics, axum, tower, wasmtime*)
    │                    (* feat-gated "wasm-plugins", default on)
    └── ruvyxa_dev_server (deps: diagnostics, bundler, graph, middleware, axum, notify, tokio)
         │
         └── ruvyxa_cli (deps: ALL crates, binary entry via clap)
```

---

## 3. `ruvyxa_graph` — Route Discovery & Validation

**File**: `crates/ruvyxa_graph/src/lib.rs` (1696 lines, single file)

### Struct definitions

```rust
pub struct RouteParams(pub BTreeMap<String, serde_json::Value>);
// JSON-shaped: catch-all segments → Value::Array,
// omitted optional catch-all → no entry

pub struct RouteManifest {
    pub app_dir: PathBuf,
    pub routes: Vec<RouteEntry>,
}

pub struct RouteEntry {
    pub id: String,                    // e.g. "app/blog/[slug]/page"
    pub path: String,                  // e.g. "/blog/[slug]"
    pub kind: RouteKind,               // Page | Api
    pub file: PathBuf,                 // absolute path to page/route file
    pub layout_chain: Vec<String>,     // route IDs of ancestor layouts
    pub server_modules: Vec<String>,   // sibling server.ts, action.ts
    pub client_modules: Vec<String>,   // sibling client.tsx
    pub runtime: RuntimeTarget,        // Node | Edge | Static (all Node currently)
    pub render: RenderMeta,
}

pub enum RouteKind { Page, Api }       // serde: kebab-case
pub enum RuntimeTarget { Node, Edge, Static }
pub enum RenderStrategy { Ssr, Ssg, Isr, Csr, Ppr } // default: Ssr

pub struct RenderMeta {
    pub strategy: RenderStrategy,
    pub revalidate: Option<u64>,       // ISR seconds
    pub has_static_params: bool,
    pub static_paths: Vec<String>,
    pub has_dynamic_slots: bool,       // PPR Suspense boundaries
}

pub struct DiscoverOptions {
    pub app_dir: PathBuf,
    pub default_render_strategy: Option<RenderStrategy>,
    pub default_revalidate: Option<u64>,
}

pub struct ValidationReport {
    pub routes: usize,
    pub page_routes: usize,
    pub api_routes: usize,
    pub client_modules: usize,
    pub server_modules: usize,
    pub diagnostics: Vec<Diagnostic>,
}
```

### `discover_routes(options) → Result<RouteManifest>`

1. **Guard**: missing `app_dir` → RUV1001 error.
2. **Walk**: `WalkDir::new(&app_dir)`. Skip dirs starting with `_` or `@`.
3. **Match filenames**:

| File                                 | RouteKind |
| ------------------------------------ | --------- |
| `page.tsx` / `.jsx` / `.md` / `.mdx` | Page      |
| `route.ts` / `.js`                   | Api       |

4. **Compute fields**:
   - `path = route_path_from_dir(relative_dir)` — strips route groups `(name)` and parallel slots
     `@name`, maps `[param]` → `/param`, `[...rest]` → `/...rest`, `[[...opt]]` → `/...opt` (or `/`
     if last and empty).
   - `id = "app/" + relative_path_without_extension`
   - `layout_chain` — walk from `app_dir` to route dir, collect `layout.tsx` at each level.
   - `render` — Page: `detect_render_strategy()`; Api: default.
5. **Sort** by path then id.
6. **Conflict detection** via `route_match_shape(path)` — replace `[name]` → `:`, `[...name]` → `*`,
   `[[...name]]` → `*?`. Same shape → RUV1003.

### `route_segment(segment, is_last) → Result<String>`

Priority:

- `[[...name]]` → optional catch-all. Must be last.
- `[...name]` → required catch-all. Must be last.
- `[name]` → dynamic param. Non-empty, no `[]`, no leading `.`.
- Contains `[`/`]` but no match → RUV1002.
- Else → static literal.

### `detect_render_strategy(file, layout_chain) → RenderMeta`

Ordered first-match:

1. `"use client"` in source → CSR.
2. `export const ppr = true` → PPR, `has_dynamic_slots: true`.
3. `export const revalidate = <n>` → ISR with `revalidate: Some(n)`.
4. `getStaticParams` or `staticParams` export → SSG, `has_static_params: true`.
5. **Static candidate**: no dynamic segments AND no `fetch(`, `headers(`, `cookies(`,
   `searchParams`, `Date.now(`, `Math.random(`, `process.env.` in reachable code (page + layout
   chain) → SSG.
6. Default → SSR.

Static candidate check: `collect_relative_graph(page + layouts)`, strip strings/comments, scan
concatenated code for markers.

### `validate_app(root, manifest) → ValidationReport`

For each route:

- **Page**: check default export (RUV1004). BFS relative imports of page + layouts. Validate each
  client-reachable module:
  - `"server-only"` import → RUV1007.
  - `process.env.<NON_PUBLIC>` access → RUV1008.
  - `server/` dir import (project-root level) → RUV1010.
- **Api**: BFS relative imports. Validate each server module:
  - `"client-only"` import → RUV1009.

### `collect_relative_graph(entry) → BTreeSet<PathBuf>`

BFS from entry. Follow only relative imports (`./`). Dedup via BTreeSet. Resolution: probe `.ts`,
`.tsx`, `.js`, `.jsx`, `.md`, `.mdx`, `index.*` variants.

### Diagnostic codes

| Code    | Condition                             |
| ------- | ------------------------------------- |
| RUV1001 | `app/` not found                      |
| RUV1002 | Invalid dynamic segment syntax        |
| RUV1003 | Conflicting routes (same match shape) |
| RUV1004 | Page missing default export           |
| RUV1007 | `server-only` in client graph         |
| RUV1008 | Private env var in client             |
| RUV1009 | `client-only` in server graph         |
| RUV1010 | `server/` dir in client graph         |

---

## 4. `ruvyxa_bundler` — Compilation Pipeline

**Files**: `crates/ruvyxa_bundler/src/` (17 files, ~10K lines)

### Pipeline stages

```
Entry source (TSX/TS/JSX/JS/MD/MDX)
  │
  ├─ 1. resolver::resolve_graph()
  │      Parallel BFS via rayon. Resolution order:
  │      1. Plugin resolve_id() hook
  │      2. Relative (./ ../) → probe 20 extensions
  │      3. Absolute (/) → virtual framework imports
  │      4. tsconfig paths/baseUrl → @/ aliases
  │      5. Bare specifier → package.json exports map
  │         Conditional: Client→[browser,import,module,default,require]
  │                      SSR→[node,import,module,default,require]
  │      6. Project-relative fallback
  │
  ├─ 2. compiler::compile_graph()
  │      Oxc parser + transformer
  │      MD/MDX → markdown crate + @mdx-js/mdx bridge
  │      CSS modules → grass (Sass) + scope_css_module()
  │      Parallel via rayon. Cache: blake3(source + jsx + version) → CompileCache
  │
  ├─ 3. boundary::check()
  │      Re-checks RUV1007/1008/1009/1010 on compiled output
  │
  ├─ 4. linker::link()
  │      Topological sort, import/export rewriting, IIFE/ESM wrapping
  │      Rewrites:
  │        import Default from "./mod"  →  const Default = __ruv_HASH__.default
  │        export default expr          →  __exports.default = expr
  │        export * from "./mod"        →  Object.assign(__exports, __ruv_HASH__)
  │        import("./lazy")             →  import("./chunk.<hash>.js")
  │      IIFE wrapper:
  │        var __ruv_<hex16>__ = (function() {
  │          var __exports = {}; var module = { exports: __exports };
  │          var process = globalThis.process || { env: { NODE_ENV: "production" } };
  │          ...rewritten_source...
  │          return module.exports;
  │        })();
  │
  ├─ 5. minifier::tree_shake() + minify()
  │      Oxc AST minifier. Line-level export pruning.
  │      fold_production_node_env() for CommonJS NODE_ENV branches.
  │
  └─ 6. output::wrap()
        Client: hydrateRoot IIFE. SSR: ESM with export async function render(ctx).
```

### Input/output types

```rust
pub enum BundleTarget { Client, Ssr }
pub enum JsxRuntime { Classic, Automatic }     // default Automatic
pub enum SplitStrategy { Single, Route }        // default Single
pub enum EsTarget { Es2018..Es2022, EsNext }    // default Es2022

pub struct BundleInput {
    pub entry: PathBuf,
    pub project_root: PathBuf,
    pub app_dir: PathBuf,
    pub layouts: Vec<PathBuf>,
    pub request_path: String,
    pub target: BundleTarget,
    pub options: BundleOptions,
}

pub struct BundleOptions {
    pub minify: bool,                       // default true
    pub source_map: bool,
    pub tree_shaking: bool,                 // default true
    pub jsx_runtime: JsxRuntime,
    pub es_target: EsTarget,
    pub split_strategy: SplitStrategy,
    pub emit_chunk_manifest: bool,
    pub collect_module_manifest: bool,
}

pub struct BundleOutput {
    pub code: String,
    pub source_map: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub stats: BundleStats,
    pub chunk_manifest: Option<ChunkManifest>,
    pub chunks: Vec<OutputChunk>,
}

pub struct BundleStats {
    pub module_count: usize,
    pub output_bytes: usize,
    pub estimated_gz_bytes: usize,      // output_bytes * 0.35
    pub minified: bool,
    pub tree_shaken: bool,
    pub duration_ms: u64,
    pub tree_shaken_modules: usize,
    pub cache_hits: usize,
}

pub struct SharedRouteBundleOutput {
    pub code: String,
    pub modules: Vec<PathBuf>,
}
```

### Resolver details

**`ResolvedModule`**:

```rust
pub struct ResolvedModule {
    pub path: PathBuf,
    pub source: String,
    pub deps: Vec<PathBuf>,
    pub is_external: bool,   // from node_modules
}
```

**`ResolveGraphCache`** (lock-free, DashMap-backed):

```rust
pub struct ResolveGraphCache {
    resolutions: Arc<DashMap<(Arc<str>, Arc<str>), Option<PathBuf>>>,  // (base_dir, specifier)
    sources: Arc<DashMap<PathBuf, CachedSource>>,
    tsconfigs: Arc<DashMap<PathBuf, CachedTsConfig>>,
    dependencies: Arc<DashMap<DependencyCacheKey, Arc<[PathBuf]>>>,
    stable_snapshot: bool,  // for_build(): skip metadata revalidation
}
```

- DashMap: 64 shards, RwLock per shard. No single Mutex contention.
- `MMAP_THRESHOLD_BYTES = 64 * 1024`: files >=64KB use memmap2 Mmap.
- `for_build()` mode: filesystem treated as immutable during build.

**Extension probing** (in order):

```
<exact>, .ts, .tsx, .js, .jsx, .mts, .cts, .mjs, .cjs, .md, .mdx,
index.ts, index.tsx, index.js, index.jsx, index.mts, index.cts, index.mjs, index.cjs, index.md, index.mdx
```

**Package exports resolution** (`PackageJsonValue`):

```rust
enum PackageJsonValue {
    Null,                          // explicitly blocked
    String(String),
    Array(Vec<Self>),              // fallback array
    Object(Vec<(String, Self)>),   // ORDERED entries (not BTreeMap!)
    Unsupported,
}
```

Condition resolution order:

- Client target: `["browser", "import", "module", "default", "require"]`
- SSR target: `["node", "import", "module", "default", "require"]`

### Compiler details

**Oxc pipeline**:

```rust
let source_type = SourceType::mjs().with_typescript(true).with_jsx(has_jsx);
Parser::new(&allocator, &source, source_type).parse();
SemanticBuilder::new_compiler().with_enum_eval(true).build(&program);

options.jsx.runtime = match jsx_runtime {
    Classic => OxcJsxRuntime::Classic,
    Automatic => OxcJsxRuntime::Automatic,
};
options.jsx.throw_if_namespace = false;
options.jsx.pure = false;
options.typescript.optimize_const_enums = false;
options.typescript.optimize_enums = false;

Transformer::new(&allocator, Path::new("ruvyxa:module.tsx"), &options)
    .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
Codegen::new().build(&program).code
```

**CompileCache**:

```rust
pub struct CompileCache {
    cache_dir: PathBuf,                               // .ruvyxa/cache/bundler/
    enabled: bool,
    namespace: String,
    memory: Arc<Mutex<HashMap<String, MemEntry>>>,    // LRU: 512 entries
}

// key = blake3(source + "\0" + jsx_flag + "\0" + jsx_runtime + "\0" + compiler_version + "\0" + namespace)[..32]
// Disk: .ruvyxa/cache/bundler/<key>.js, atomic write via temp+rename
```

### Content compilation (content.rs)

```
MD:   markdown::to_mdast(body, ParseOptions::gfm()) → React.createElement(...)
MDX:  markdown::to_mdast(body, mdx_parse_options()) → collect ESM + React.createElement(...)

Output module:
  import React from "react";
  // ... ESM from MDX (if any) ...
  export const frontmatter = {...};
  export const headings = [...];
  export default function RuvyxaContentPage({ components = {} }) {
    return React.createElement("article", { className: "ruvyxa-content" }, ...children);
  }
```

- Frontmatter: YAML between `---` delimiters.
- AST nodes → `createElement(tag, props, children)`. Tables, code blocks, images, checkboxes, math,
  footnotes all get specific React element wrappers.
- Cache: `ContentModuleCache (HashMap, 512 LRU)`, key = `blake3(ext + "\0" + source)`.

### CSS Module compilation (style_module.rs)

```rust
pub struct CssModule {
    pub css: String,
    pub classes: BTreeMap<String, String>,  // local_name → scoped_name
}
```

**`scope_css_module(css, path, project_root) → CssModule`**:

- Character-level scanner. States: `quote`, `in_comment`, `block_allows_rules` stack.
- `.local-class` → `.scoped-class__hash`
- `:global(.selector)` → pass through unmodified.
- `composes: class1 class2` → appends scoped names.

**Scoped name generation**:

```rust
fn scoped_class_name(path, project_root, local) -> String {
    let relative = normalized_lowercase(path - project_root);
    let digest = fnv1a_64(format!("{relative}:{local}"));
    let stem = alphanumeric(file_stem minus ".module");
    format!("{stem}_{local}__{digest:016x}")
    // Example: card_card__feff5ad3a1e67b7b
}
```

Sass compilation via `grass::from_path()` with expanded style + `node_modules` load paths.

### Linker details

**Module ID**: `format!("__ruv_{:016x}__", blake3(path)[..8])` — deterministic, path-based.

**Cycle detection**: DFS with gray/black set. Finds `BundleError::CircularDependency { cycle }`.

**Topological sort**: DFS post-order. Dependencies before importers.

**Import rewriting**: Line-by-line scanner:

| Pattern                          | Rewrite                                            |
| -------------------------------- | -------------------------------------------------- |
| `import "./styles.css"`          | `// [bundled] import "./styles.css"`               |
| `import Default from "./mod"`    | `const Default = __ruv_xxx__.default`              |
| `import { a, b } from "./mod"`   | `const a = __ruv_xxx__.a; const b = __ruv_xxx__.b` |
| `import * as ns from "./mod"`    | `const ns = __ruv_xxx__`                           |
| `require("./mod")`               | `__ruv_xxx__`                                      |
| `import("./lazy")` (chunked)     | `import("./chunk.xxx.js")`                         |
| `import("./lazy")` (not chunked) | `Promise.resolve(__ruv_xxx__)`                     |
| `export default expr`            | `__exports.default = expr`                         |
| `export { a, b }`                | `__exports.a = a; __exports.b = b`                 |
| `export * from "./mod"`          | `Object.assign(__exports, __ruv_xxx__)`            |

**Parallel linking**: for >=8 modules, generate IIFE segments in parallel via rayon, concatenate
results.

### Shared route module registry

```
globalThis.__RUVYXA_SHARED_MODULES__[id] = moduleExports;
```

Cross-route dedup: `prepare_bundle()` for all routes, detect shared modules across >=2 routes, emit
`shared.js`, then each route imports from registry.

### Chunking (chunking.rs)

**Algorithm**:

1. Find dynamic import roots (targets of `import()` calls).
2. For each root, compute static transitive closure.
3. A root is split into its own chunk ONLY if its closure is:
   - Disjoint from entry's static closure
   - Disjoint from every other dynamic root's closure
   - If overlap exists → root stays in entry bundle.

**Chunk filename**: `chunk.{blake3(graph_fingerprint + "\0" + root_path)[..16]}.js`

### Minifier (minifier.rs)

```rust
pub fn minify_with_options(source, target, tree_shaking) -> Result<String>
```

Oxc `MinifierOptions`:

- With tree-shaking: `MinifierOptions::default()` (full mangle + compress).
- Without: `MinifierOptions { mangle: default, compress: Some(CompressOptions::safest()) }` +
  `CodegenOptions::minify()`.

**Tree-shaking**: line-level scanner. Collects used `__ruv_xxx__.member` patterns across bundle.
Removes unused `__exports.member = member` lines (always keeps `default`).

**`fold_production_node_env(source)`**: CommonJS branch folding for node_modules. Recognizes
`process.env.NODE_ENV === "production"`, `!==`, `==`, `!=` (both orders). Bounded loop (max 64
iterations). Finds innermost `if(cond) { ... } [else { ... }]`, replaces with appropriate branch.

### Source map builder (sourcemap.rs)

```rust
pub struct SourceMapBuilder {
    file: String,
    sources: Vec<String>,
    sources_content: Vec<Option<String>>,
    mappings: Vec<Mapping>,
    source_root: PathBuf,
    ignore_list: Vec<u32>,
}

pub struct Mapping {
    pub gen_line: u32, pub gen_col: u32,
    pub source_idx: u32, pub orig_line: u32, pub orig_col: u32,
}
```

Supports identity mappings, importing existing v3 source maps with line offset,
`x_google_ignoreList`.

### Plugin pipeline (plugin.rs)

```rust
pub trait RuvyxaBundlerPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn resolve_id(...) -> Result<Option<PathBuf>>;
    fn transform(...) -> Result<Option<TransformResult>>;
}
```

Hooks execute in order. First `resolve_id` match wins. `transform` chains, preserves last non-None
source map.

### Incremental cache (incremental.rs)

```rust
pub struct CachedModuleEntry {
    pub content_hash: String,      // blake3[..32]
    pub size: u64,
    pub mtime_secs: u64,
    pub deps: Vec<PathBuf>,
    pub compile_key: Option<String>,
}

pub struct GraphManifest {
    pub version: String,           // "ruvyxa_graph_cache:v1"
    pub modules: BTreeMap<PathBuf, CachedModuleEntry>,
}
```

Freshness: size fast-reject → blake3 content hash. Dirty set: BFS over reverse dependency graph from
changed paths.

---

## 5. `ruvyxa_dev_server` — HTTP Server, HMR, Cache

**Files**: `crates/ruvyxa_dev_server/src/` (6 files, ~8K lines)

### ServerConfig

```rust
pub struct ServerConfig {
    pub root: PathBuf,
    pub app_dir: PathBuf,                  // root/app
    pub public_dir: PathBuf,               // root/public
    pub client_dir: PathBuf,               // root/.ruvyxa/client
    pub prerender_dir: Option<PathBuf>,    // root/.ruvyxa/prerender
    pub host: String,                      // default "0.0.0.0"
    pub port: u16,                         // default 3000
    pub watch: bool,                       // dev vs production
    pub cache_route_manifest: bool,
    pub cache_css: bool,
    pub style_entries: Vec<PathBuf>,
    pub prebundle_dependencies: bool,
    pub jsx_runtime: JsxRuntime,
    pub error_overlay: bool,
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

### AppState

```rust
struct AppState {
    config: ServerConfig,
    reload_tx: broadcast::Sender<String>,         // HMR WebSocket fan-out
    runtime_cache: Arc<RuntimeCache>,              // manifest, router, CSS
    action_limiter: Arc<Mutex<ActionRateLimiter>>,
    worker_pool: Arc<NodeWorkerPool>,
    render_cache: Arc<RenderCache>,
    isr_revalidating: Arc<Mutex<HashSet<String>>>,
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
    files: BTreeSet<PathBuf>,  // normalized, case-folded on Windows
}
```

### `serve()` startup sequence

1. Validate config limits.
2. Discover routes via `ruvyxa_graph::discover_routes`.
3. Create broadcast channel: `broadcast::channel::<String>(64)`.
4. Load runtime env (`.env` + `.env.local`, insert `RUVYXA_JSX_RUNTIME`).
5. Start Node worker pool: `NodeWorkerPool::start(root, env)`.
6. Pre-bundle dependencies (dev + `prebundle_dependencies`).
7. Create RenderCache: dev=1024/300s, prod=512/1800s.
8. Populate HmrTracker from manifest.
9. Create MiddlewareStack + WasmPluginRuntime.
10. Build Axum Router with 4 routes + fallback.
11. Bind TCP listener (port fallback: try up to +100).
12. Graceful shutdown: signal → 5s grace → worker_pool.shutdown().

### Endpoints

| Endpoint                            | Handler             | Protocol                                                                    |
| ----------------------------------- | ------------------- | --------------------------------------------------------------------------- |
| `GET /__ruvyxa/hmr`                 | `hmr_ws()`          | WS upgrade, subscribe to `reload_tx`, send `Message::Text`                  |
| `GET /__ruvyxa/client?path=`        | `client_bundle()`   | Calls `render_client_bundle_pooled()`, returns JS                           |
| `POST /__ruvyxa/action?path=&name=` | `action_endpoint()` | Validate body/Origin/Sec-Fetch, rate limit, `render_server_action_pooled()` |
| `GET /__ruvyxa/trace?path=`         | `trace_endpoint()`  | Dev-only, returns `RuntimeTrace` JSON                                       |

### Request lifecycle (`handle_request`)

```
1. Start timer
2. Extract (parts, body)
3. Canonicalize path:
   - Split by /, percent-decode each segment
   - Reject empty, ., .., decoded / or \, control chars
   - Reject malformed percent encoding
4. Read body (if not GET/HEAD): to_bytes(body, api_body_limit_bytes)
5. Request-phase Wasm plugins (can short-circuit)
6. Render dispatch:
   ├── Static file: serve_client_file() → try public/ + ETag
   ├── Route match: RadixRouter::find()
   │   ├── Page: render_page_by_strategy() [SSR/SSG/ISR/CSR/PPR]
   │   └── API:  render_api_pooled()
7. Error handling: dev overlay or plain error page
8. Response-phase Wasm plugins
9. Dev request logging
```

### Radix router (router.rs)

```rust
pub struct RadixRouter { root: TrieNode }

struct TrieNode {
    static_children: Vec<(String, TrieNode)>,    // O(n) linear scan
    param_child: Option<Box<ParamChild>>,        // single [param]
    wildcard: Option<Box<WildcardChild>>,        // single [...rest]
    optional_wildcard: Option<Box<WildcardChild>>, // single [[...rest]]
    route_index: Option<usize>,                  // terminal if set
}

struct ParamChild { name: String, node: TrieNode }
struct WildcardChild { name: String, route_index: usize }
```

**Insert**: segment classification → `Static` (linear scan `static_children`), `Param`
(`get_or_insert_with`), `Wildcard`/`OptionalWildcard` (terminal, no further children).

**Match** (O(path_depth)):

1. Static children (first match)
2. Dynamic param (capture value, recurse)
3. Required catch-all (capture remaining segments as JSON array)
4. Optional catch-all (also checked at base if no segments left)

### Render cache (render_cache.rs)

```rust
pub struct RenderCache {
    entries: RwLock<HashMap<String, CacheEntry>>,  // tokio::sync
    order: RwLock<VecDeque<String>>,
    capacity: usize,                                // 1024 dev, 512 prod
    ttl: Duration,                                  // 300s dev, 1800s prod
    hits: AtomicU64,
    misses: AtomicU64,
}

struct CacheEntry {
    value: Arc<str>,       // zero-copy sharing
    created_at: Instant,
}
```

**Cache keys**: `ssr:{path}?{params_json}` | `client:{path}?{params_json}` | `ssg:{key}` |
`isr:{key}` | `ppr:{key}`

| Operation                         | Lock                                     |
| --------------------------------- | ---------------------------------------- |
| `get(key)`                        | read `entries` + write `order` (promote) |
| `put(key, value)`                 | write both `entries` + `order`           |
| `invalidate_all_blocking()`       | write both (sync, for file watcher)      |
| `invalidate_route_blocking(path)` | write both, prefix match                 |
| `is_valid(key)`                   | read `entries` + TTL check               |

**ISR stale-while-revalidate**: `get_stale_with_age(key)` → bypasses TTL, returns value + age.
Caller decides if age > revalidate interval.

**LRU eviction**: `VecDeque`: front=LRU, back=MRU. On `put()` while at capacity: pop front, remove
from HashMap.

### Worker pool (worker_pool.rs)

```rust
const DEFAULT_POOL_SIZE = 4;                    // min 2, max 8
const DEFAULT_WORKER_TIMEOUT_MS = 30_000;
const BUILD_WORKER_TIMEOUT_MS = 300_000;
const WORKER_SHUTDOWN_TIMEOUT = 2s;
const MAX_PENDING_RESPONSE_FRAMES = 16;

pub struct NodeWorkerPool {
    workers: StdRwLock<Vec<Arc<Worker>>>,
    worker_script: PathBuf,               // packages/ruvyxa/runtime/worker-pool.mjs
    env: BTreeMap<String, String>,
    next_worker: AtomicU64,                // round-robin
    response_timeout: Duration,
}

struct Worker {
    stdin_tx: StdMutex<Option<mpsc::Sender<String>>>,
    pending: PendingResponses,             // Arc<Mutex<BTreeMap<String, PendingResponse>>>
    child: Mutex<Option<Child>>,
    alive: Arc<AtomicBool>,
}
```

**Worker spawn**: `node <worker_script>` with piped stdin/stdout/stderr + kill_on_drop.

**Communication**: NDJSON (newline-delimited JSON) over stdin/stdout.

**`WorkerRequest`** (tagged JSON via `#[serde(tag = "type")]`):

| Variant        | Fields                                                                                                          |
| -------------- | --------------------------------------------------------------------------------------------------------------- |
| `Ssr`          | `id, projectRoot, appDir, pageFile, requestPath, params`                                                        |
| `Api`          | `id, projectRoot, routeFile, method, requestPath, headers/headerPairs, body/bodyBase64, streamResponse, params` |
| `Action`       | `id, projectRoot, actionFile, actionName, payloadJson, contentType, requestPath`                                |
| `Client`       | `id, projectRoot, appDir, pageFile, requestPath, params`                                                        |
| `Invalidate`   | `id, paths`                                                                                                     |
| `Ssg`          | `id, projectRoot, appDir, pageFile, requestPath, params, mode("full"\|"ppr"), fresh`                            |
| `StaticParams` | `id, projectRoot, pageFile, routePath, segments, routes`                                                        |

**`WorkerResponse`**:
`{ id, ok, frame, html, script, status, headers/headerPairs, body/bodyBase64, code, message, stack, pong, warmed, module_cache_size, params, dependency_hash, inputs }`

**Streaming API protocol**:

1. `{ id, ok: true, frame: "api-start", status, headers }`
2. `{ id, ok: true, frame: "api-chunk", bodyBase64 }` (zero or more)
3. `{ id, ok: true, frame: "api-end" }` (terminal)
4. Error: `{ id, ok: false, frame: "api-error", message, code }` (terminal)

**Failure recovery**:

1. On error: `replace_failed_worker(index)` → spawn new Worker, swap via Arc::ptr_eq.
2. If replacement succeeded AND request is idempotent (Ssr, Ssg, StaticParams, Client, Ping, Warmup,
   Invalidate): retry on new worker.
3. Non-idempotent (Api, Action): not retried.

### HMR tracker (hmr_tracker.rs)

```rust
pub struct HmrTracker {
    file_to_routes: Arc<RwLock<BTreeMap<PathBuf, BTreeSet<String>>>>,  // parking_lot sync locks
    route_to_files: Arc<RwLock<BTreeMap<String, BTreeSet<PathBuf>>>>,
}

pub struct HmrUpdate {
    pub affected_routes: Vec<String>,
    pub full_reload: bool,
    pub changed_files: Vec<PathBuf>,
    pub event_type: HmrEventType,      // CssUpdate | ComponentUpdate | FullReload
}
```

**Bidirectional maps**: populated from manifest at startup.
`file_to_routes[file].insert(route.path)` + `route_to_files[route.path] = files`.

**`compute_update(changed_paths)`**:

1. All `.css`/`.scss`/`.sass` → `CssUpdate`.
2. Any `layout.*` → `FullReload`.
3. Known files → `ComponentUpdate`.
4. Unknown file (tracker has routes) → `FullReload`.
5. Unknown file (empty tracker) → `ComponentUpdate`.

**Event dispatch**:

```
File watcher (notify) → compute_update() →
  ├── FullReload: runtime_cache.invalidate() + render_cache.invalidate_all()
  ├── Selective: runtime_cache.invalidate_styles_for_paths() + render_cache.invalidate_route() per affected route
  └── Broadcast JSON via reload_tx → WebSocket → browser
      {"type":"css-update"|"component-update"|"full-reload", "paths":[...], "affectedRoutes":[...], "fullReload":bool}
```

File watcher filters: `.git`, `.ruvyxa`, `target`, `dist`, `.npm-pack`, `.npm-smoke`,
`node_modules`, `.ruvyxa-*` prefixes.

### Style collection (style.rs)

**`collect_styles(config) → StyleCollection { css, files }`**:

**Phase 1**: Collect script seeds.

- Walk `app_dir` recursively → all `.ts/.tsx/.js/.jsx/.mts/.cts/.mjs/.cjs` files.
- Add configured `css.entries`.

**Phase 2**: BFS script import graph.

- `visited_scripts: BTreeSet<PathBuf>`.
- For each script, parse imports: CSS/Sass → style_seeds, Script → push to queue (within project,
  not node_modules).

**Phase 3**: Process style seeds.

- Sass: `compile_sass_file()` via `grass`.
- CSS Module: `scope_css_module()`.
- CSS @import: resolve inlined local imports, strip resolved lines.
- Tailwind: if source contains `@import "tailwindcss"`, run `tailwindcss -i <file> --minify`
  subprocess.
- Escape `</style` → `<\/style` for HTML inline safety.
- Minification: strip CSS comments + collapse whitespace (not for Sass).

**Sass import resolution** (`sass_dependency_paths`): DFS through `@use`, `@forward`, `@import`.
Filters `sass:` and remote URLs. Probes: exact, `.scss`, `.sass`, `_name.scss`, `_name.sass`,
`index.*` variants.

---

## 6. `ruvyxa_middleware` — Tower Middleware & Wasm Plugins

**Files**: `crates/ruvyxa_middleware/src/` (5 files)

### Middleware stack order (outermost first)

```
1. Compression (gzip + brotli) — tower_http::CompressionLayer
   Predicate: complete body size hint (known exact size)
2. CORS — CorsLayer (configurable origins, methods, headers)
3. Rate limiting — RateLimitLayerWithKey (token-bucket per key)
4. Timing — TimingLayer (X-Response-Time header)
5. Request logging — RequestLoggingLayer (method, path, status, duration, X-Request-ID)
6. Custom headers — CustomHeadersLayer (all configured headers)
7. Custom layers (rejected: unsupported)
8. Wasm plugin layers (if feature enabled)
```

### Rate limiter

```rust
struct ActionRateLimiter {
    hits: HashMap<String, Vec<Instant>>,  // sliding window
    max_hits: usize,
    window: Duration,
    max_keys: usize,     // 10,000 — prunes expired on insert
}
```

- Key by: `"ip"` or `"header:<name>"`.
- `allow(key)`: prune stale entries, check limit, record hit. Returns 429 with `Retry-After` on
  limit.

### Wasm plugin sandbox

```rust
pub struct PluginConfig {
    pub name: String,
    pub path: PathBuf,                     // .wasm file, project-relative, validated no ..
    pub phase: PluginPhase,                // Request | Response | Both
    pub routes: Option<Vec<String>>,       // None → all, else prefix match with * suffix
    pub config: Option<Value>,
    pub permissions: PluginPermissions,
}

pub struct PluginPermissions {
    pub env: Vec<String>,                  // allowed env vars
    pub fs_read: Vec<PathBuf>,             // REJECTED at startup
    pub net: Vec<String>,                  // REJECTED at startup
    pub timeout_ms: u64,                   // default 5000
    pub max_memory_bytes: u64,             // default 64MB
}
```

**Execution**:

1. Load `.wasm` via `wasmtime::Module::new(&engine, bytes)`.
2. Create `WasiCtxBuilder` with only permitted env vars.
3. Set `StoreLimitsBuilder::memory_size(max_memory_bytes)` + fuel budget (`timeout_ms * 1_000_000`).
4. Expects exports: `memory` + `on_request`/`on_response`.
5. Function signature: `fn(input_ptr: i32, input_len: i32) -> result_ptr: i32`.
6. Write serialized JSON to plugin memory at offset 0.
7. Call function, read result from `result_ptr` as NUL-terminated UTF-8 JSON (max 1MB).
8. Actions: `"continue"`, `"respond"`, `"modify-request"`, `"modify-response"`.

Diagnostics: `RUV2100` (load), `RUV2101` (execution).

---

## 7. `ruvyxa_cli` — Command Dispatch & Build Orchestration

**File**: `crates/ruvyxa_cli/src/main.rs` (6037 lines)

### Commands

```
ruvyxa
  dev          Dev server + HMR + file watcher
  build        Production build → .ruvyxa/
  check        TypeScript (tsc --noEmit) + test:parity
  start        Serve production build
  preview      Preview production build locally
  routes       Print route table (path, kind, strategy, file)
  analyze      Validate routes/imports/boundaries
  doctor       Project setup check
  clean        Remove .ruvyxa/
  trace        Inspect one route by path
  bench        Benchmark (route discovery, analysis, build)
  test:parity  Dev/prod route comparison + smoke renders

Global: --root (default "."), --host, --port
Build: --target (node|edge|static)
Bench: --samples (3), --json
```

### Config loading (2-phase)

**Phase 1 (Node)**: `node config-renderer.mjs <root>` → stdout JSON.

```typescript
interface ConfigRendererOutput {
  ok: boolean
  config?: ProjectConfig
  code?: string
  message?: string
  stack?: string
  dependency_hash: string // blake3 of config + dependencies
}
```

**Phase 2 (Rust)**: Parse JSON → `ProjectConfig` with `#[serde(deny_unknown_fields)]`. Validate path
bounds, limits, IPs, JSX runtime.

### ProjectConfig

```rust
pub struct ProjectConfig {
    pub app_dir: Option<String>,               // default "app"
    pub out_dir: Option<String>,               // default ".ruvyxa"
    pub runtime: Option<BuildTarget>,
    pub render: Option<RenderingConfig>,        // strategy, revalidate
    pub server: Option<ServerConfigOptions>,    // host, port
    pub css: Option<CssConfigOptions>,          // entries
    pub build: Option<BuildConfigOptions>,       // minify, map, treeShake, split, workers, jsx, target, warm, prerenderCache
    pub debug: Option<DebugConfigOptions>,      // overlay, traces
    pub image: Option<ImageOptimizationOptions>,// optimize, quality, lossless, workers
    pub security: Option<SecurityConfigOptions>, // actionLimit, apiLimit, rateLimit, sameOrigin, headers
    pub cache: Option<CacheConfigOptions>,      // routes, css, dir
    pub middleware: Option<MiddlewareConfig>,
    pub plugins: Option<Vec<BuildPluginConfig>>,
    pub adapter: Option<serde_json::Value>,
    pub adapter_options: Option<serde_json::Value>,
}
```

### Build pipeline

```
build(args)
  ├── load_project_config(root)           # Node eval → JSON → Rust struct
  ├── discover_routes(app_dir)            # ruvyxa_graph
  ├── validate_app(root, manifest)        # ruvyxa_graph
  ├── collect_styles(root, app_dir)       # dev_server::style
  ├── create_staging_dir(out_dir)
  │   ├── server/app/   ← app/ (compiled)
  │   ├── server/components/ ← components/
  │   ├── server/server/ ← server/
  │   ├── client/ ← per-route client bundles
  │   ├── assets/ ← optimized images (WebP)
  │   └── prerender/ ← SSG/ISR/PPR/CSR HTML
  ├── optimize_public_images()            # PNG/JPG → WebP via image crate, rayon parallel
  ├── write_manifest(manifest.json)
  ├── emit_client_bundles()
  │   For each page route:
  │     resolve → compile → boundary → link → tree-shake → minify → wrap
  │     Split strategy: Route→dedup shared modules across routes; Single→independent
  ├── prerender_static_routes()
  │   resolve_static_params → worker pool render → artifact cache → HTML files
  ├── write build.json                    # timing, config, image report
  ├── commit_staged_builds()              # atomic rename staging → output
  └── print_build_report()
```

### Pre-render artifact cache

```rust
struct PrerenderArtifactCache {
    directory: PathBuf,
    dependency_hash: String,
    render_context_hash: String,
    fingerprints: Arc<ArtifactFingerprintCache>,
    enabled: bool,
}
```

Validated by: version == 1, dependency_hash match, render_context_hash match, all file fingerprints
match.

### JS config plugin bridge (`JsConfigPluginBridge`)

Implements `RuvyxaBundlerPlugin`:

- `resolve_id`: sends `resolveId` to JS plugin runner Node subprocess, returns resolved path.
- `transform`: sends `transform` to JS plugin runner, returns transformed code + source map.

### Image optimizer (image_optimizer.rs)

```rust
pub fn optimize_public_images(public_dir, assets_dir, cache_dir, options) -> Report
```

- Discovers all files in `public_dir`.
- Decodes via `image` crate. Skips non-optimizable files (copy as-is).
- Cache key: blake3(quality + lossless + source bytes). Cache → hardlink. Miss → encode WebP, write
  cache, hardlink to output.
- Output: `.ruvyxa-images.json` manifest.
- Parallel: rayon (configurable `parallelism`, default global).

---

## 8. Build Output Structure

```
.ruvyxa/
├── build.json                  # Build metadata, timing, config, image report
├── manifest.json               # RouteManifest (serialized)
├── server/
│   ├── app/                    # Compiled server-side code
│   ├── components/             # Shared components
│   └── server/                 # Server-only modules
├── client/
│   ├── manifest.json           # Client chunk manifest per route
│   ├── <blake3_16>.js          # Per-route client bundles
│   ├── chunk.<blake3_16>.js    # Dynamic import chunks
│   └── shared.js               # Cross-route shared modules registry
├── assets/
│   ├── <file>.webp             # Optimized images
│   └── .ruvyxa-images.json     # Image manifest
└── prerender/
    ├── manifest.json           # Pre-rendered route metadata
    ├── index.html              # Root page
    └── <path>/index.html       # Nested route pre-renders
```

---

## 9. Diagnostic Codes

| Code           | Source        | Condition                       |
| -------------- | ------------- | ------------------------------- |
| **Graph**      |               |                                 |
| RUV1001        | graph         | `app/` not found                |
| RUV1002        | graph         | Invalid dynamic segment         |
| RUV1003        | graph         | Route path conflict             |
| RUV1004        | graph         | Page missing default export     |
| RUV1007        | graph/bundler | `server-only` in client graph   |
| RUV1008        | graph/bundler | Private `process.env` in client |
| RUV1009        | graph/bundler | `client-only` in server graph   |
| RUV1010        | graph/bundler | `server/` dir in client graph   |
| **Server**     |               |                                 |
| RUV1100        | dev_server    | React SSR failed                |
| RUV1102        | dev_server    | SSR renderer not found          |
| RUV1200        | dev_server    | API route execution failed      |
| RUV1201        | dev_server    | No available server port        |
| RUV1202        | dev_server    | API renderer not found          |
| RUV1300        | dev_server    | Client bundling failed          |
| RUV1303        | dev_server    | Client route not found          |
| RUV1304        | dev_server    | Client bundle for non-page      |
| RUV1402        | dev_server    | Sass compilation failed         |
| RUV1403        | dev_server    | Stylesheet import unresolved    |
| RUV1500        | dev_server    | SSG render failed               |
| RUV1501        | dev_server    | Action file not found           |
| RUV1550        | dev_server    | PPR render failed               |
| RUV1601        | dev_server    | Config value too small          |
| RUV1602        | dev_server    | Config value too large          |
| **Middleware** |               |                                 |
| RUV2000        | middleware    | Config error                    |
| RUV2001        | middleware    | Execution failed                |
| RUV2100        | middleware    | Wasm plugin load error          |
| RUV2101        | middleware    | Wasm plugin execution error     |

---

## 10. Concurrency Model

| Component         | Mechanism                             | Rationale                                                     |
| ----------------- | ------------------------------------- | ------------------------------------------------------------- |
| ResolveGraphCache | DashMap (64 shards, RwLock per shard) | Read-heavy, lock-free reads, no single Mutex                  |
| CompileCache      | Arc<Mutex<HashMap>> + disk            | Write infrequent, critical for correct LRU order              |
| RenderCache       | tokio::sync::RwLock (2 locks)         | Async access, read-mostly, TTL checks                         |
| HmrTracker        | parking_lot::RwLock                   | Synchronous use (notify callback, no tokio)                   |
| WorkerPool        | StdRwLock<Vec<Arc<Worker>>>           | Infrequent writes (failure recovery only)                     |
| Worker.pending    | Arc<Mutex<BTreeMap>>                  | Write on every request (insert/remove), fast critical section |
| ISR set           | tokio::sync::Mutex<HashSet>           | Async lock, coalesce concurrent revalidations                 |
| Worker stdin      | mpsc::channel(256) per worker         | Bounded backpressure, drop signals shutdown                   |
| Worker response   | mpsc::channel(16) per request         | Bounded streaming backpressure                                |
| Compilation       | rayon::par_iter()                     | CPU-bound parallel work                                       |
| Module resolution | rayon::par_iter() (stage 2 BFS)       | I/O + CPU mix                                                 |

---

## 11. Network Protocols

### Worker pool NDJSON

**Request** (one line):

```json
{
  "type": "ssr",
  "id": "uuid",
  "projectRoot": "/path",
  "pageFile": "...",
  "requestPath": "/about",
  "params": {}
}
```

**Response** (one line):

```json
{ "id": "uuid", "ok": true, "html": "<!doctype html><html>...</html>" }
```

**Streaming API** (multi-line):

```
{"id":"uuid","ok":true,"frame":"api-start","status":200,"headers":{"content-type":"text/plain"}}
{"id":"uuid","ok":true,"frame":"api-chunk","bodyBase64":"SGVsbG8g"}
{"id":"uuid","ok":true,"frame":"api-chunk","bodyBase64":"V29ybGQ="}
{"id":"uuid","ok":true,"frame":"api-end"}
```

### HMR WebSocket

**Server → Browser**:

```json
{"type":"css-update","paths":["app/styles.css"],"affectedRoutes":["/"],"fullReload":false}
{"type":"component-update","paths":["app/components/Button.tsx"],"affectedRoutes":["/","/about"],"fullReload":false}
{"type":"full-reload","paths":[],"affectedRoutes":[],"fullReload":true}
```

**Browser handler** (simplified, currently all trigger `location.reload()`):

```js
const socket = new WebSocket(`ws://${location.host}/__ruvyxa/hmr`)
socket.addEventListener('message', (event) => {
  const msg = JSON.parse(event.data)
  // "css-update" → replace <style data-ruvyxa-css>
  // "component-update" → React Fast Refresh
  // "full-reload" → location.reload()
})
```

### Plugin Wasm ABI

**Exports required**: `memory`, `on_request` | `on_response`.

**Function signature**: `(input_ptr: i32, input_len: i32) -> result_ptr: i32`.

**Input JSON**:

```json
{
  "request": {"method":"GET","path":"/about","headers":[["key","val"]],"body": [byte array]},
  "config": {"plugin_specific": "value"}
}
```

**Result JSON**:

```json
{
  "action": "continue" | "respond" | "modify-request" | "modify-response",
  "request": { "method": "...", "path": "...", "headers": [...], "body": [...] },
  "response": { "status": 200, "headers": [...], "body": [...] }
}
```

---

## 12. Security Model

### Environment variable isolation

```
RUVYXA_PUBLIC_*  →  allowed in client bundles
All other vars   →  server-only
```

Enforced at graph level (source scan) + bundler level (compiled output scan).

### Rate limiting

| Layer                                                            | Scope                 | Default                            |
| ---------------------------------------------------------------- | --------------------- | ---------------------------------- |
| HTTP middleware                                                  | Global, by IP/header  | Configurable via middleware config |
| Action endpoint                                                  | Per-route action POST | Configurable via security config   |
| Both use sliding-window token buckets. Max tracked keys: 10,000. |

### Request body limits

| Endpoint                                                                        | Config key             | Default      |
| ------------------------------------------------------------------------------- | ---------------------- | ------------ |
| Action POST                                                                     | `security.actionLimit` | Configurable |
| API POST/PUT                                                                    | `security.apiLimit`    | Configurable |
| Plugin response                                                                 | `security.pluginLimit` | Configurable |
| Enforced before body read in `handle_request`. Returns `413 PAYLOAD_TOO_LARGE`. |

### Same-origin actions

`same_origin_actions: true` → validate `Origin` matches server origin.

### Plugin sandbox

- No filesystem access (rejected at validation).
- No network access (rejected at validation).
- Memory limit: `max_memory_bytes` (default 64MB).
- CPU budget: `timeout_ms * 1_000_000` fuel units.
- Result size limit: 1MB.

### Trusted proxy IPs

`trusted_proxy_ips` config → which `X-Forwarded-*` headers to trust.

---

## 13. Performance Characteristics

| Component              | Default                     | Configurable                           | Notes                         |
| ---------------------- | --------------------------- | -------------------------------------- | ----------------------------- |
| Worker pool            | 4 (min 2, max 8)            | `build.workers`                        | CPU-count clamped             |
| Worker timeout         | 30s interactive, 300s build | `RUVYXA_WORKER_TIMEOUT_MS`             |                               |
| Render cache           | 1024 dev, 512 prod          | `RUVYXA_RENDER_CACHE_SIZE` (cap 16384) |                               |
| Render cache TTL       | 300s dev, 1800s prod        |                                        |                               |
| Compile cache (memory) | 512 entries                 |                                        | LRU eviction                  |
| Content cache (memory) | 512 entries                 |                                        | LRU eviction                  |
| Broadcast channel      | 64 capacity                 |                                        | HMR, drops oldest on overflow |
| mmap threshold         | 64 KB                       |                                        | memmap2 vs read_to_string     |
| Image quality          | 82                          | `image.quality`                        | WebP encoding                 |
| Port fallback          | +100 attempts               |                                        | Sequential bind attempts      |
| Plugin result limit    | 1 MB                        | `security.pluginLimit`                 |                               |

---

## 14. Crate File Maps

### `ruvyxa_graph` (single file)

```
src/lib.rs  — discover_routes(), validate_app(), detect_render_strategy(), detect_conflicts()
               collect_relative_graph(), layout_chain(), route_path_from_dir(), route_segment()
               write/read_manifest(), RouteManifest, RouteEntry, RenderMeta, ValidationReport
```

### `ruvyxa_bundler` (17 files)

```
src/
├── lib.rs           — bundle(), bundle_with_context(), bundle_shared_route_modules()
│                       PreparedBundle, prepare_bundle(), bundle_prepared()
├── types.rs         — BundleInput, BundleOutput, BundleOptions, BundleStats, BundleTarget,
│                       JsxRuntime, SplitStrategy, EsTarget, ChunkManifest, DynamicImportChunk
├── resolver.rs      — resolve_graph(), ResolvedModule, ResolveGraphCache, PackageJsonValue,
│                       tsconfig resolution, package.json exports map, parallel BFS
├── compiler.rs      — compile_graph(), CompileCache, Oxc pipeline (parse→transform→codegen)
├── boundary.rs      — RUV1007/1008/1009/1010 checks on compiled output
├── linker.rs        — link(), detect_cycles(), topological sort, import/export rewriting,
│                       IIFE/ESM wrap, shared module registry
├── minifier.rs      — minify(), tree_shake_exports(), fold_production_node_env()
├── output.rs        — build_entry_source(), wrap()
├── ast.rs           — parse_module(), ModuleAst, ImportEdge, byte-level scanner
├── cache.rs         — CompileCache (memory + disk), blake3 keying
├── chunking.rs      — plan_dynamic_chunk_files(), static_entry_modules(), graph_fingerprint()
├── context.rs       — BundleContext (shared caches + plugins)
├── style_module.rs  — CssModule, scope_css_module(), scoped_class_name(), sass compilation
├── content.rs       — compile_content_module(), MD/MDX → React createElement lowering
├── sourcemap.rs     — SourceMapBuilder, Mapping, VLQ encoding, identity maps
├── incremental.rs   — IncrementalGraphCache, CachedModuleEntry, freshness checking
└── plugin.rs        — PluginPipeline, RuvyxaBundlerPlugin trait, resolve_id/transform hooks
```

### `ruvyxa_dev_server` (6 files)

```
src/
├── lib.rs           — serve(), handle_request(), endpoint handlers, HTML composition,
│                       error overlay, file watcher, ISR revalidation (~4868 lines)
├── router.rs        — RadixRouter, TrieNode, insert(), lookup(), pattern parsing
├── render_cache.rs  — RenderCache, LRU + HashMap, TTL, ISR stale-while-revalidate
├── hmr_tracker.rs   — HmrTracker, bidirectional maps, compute_update(), event type detection
├── worker_pool.rs   — NodeWorkerPool, Worker, NDJSON protocol, streaming API, failure recovery
└── style.rs         — collect_styles(), Sass compilation, CSS modules, Tailwind integration
```

### `ruvyxa_middleware` (5 files)

```
src/
├── lib.rs           — Re-exports
├── config.rs        — MiddlewareConfig, BuiltinMiddlewareConfig, PluginConfig, PluginPermissions
├── stack.rs         — MiddlewareStack::apply() (compression→cors→rate→timing→log→headers→plugins)
├── builtin.rs       — Tower Service impls: TimingLayer, RequestLoggingLayer, CorsLayer,
│                       RateLimitLayerWithKey, CustomHeadersLayer
└── wasm.rs          — WasmPluginRuntime, sandboxed invocation, fuel budgeting, result reading
```

### `ruvyxa_diagnostics` (single file)

```
src/lib.rs  — Diagnostic, RuvyxaError (Diagnostic|Io|Message), SourceSpan, Result<T>
```

### `ruvyxa_cli` (commands in `src/`)

```
src/
├── main.rs              — Cli struct, Command enum, dispatch, config loading, build orchestration
├── image_optimizer.rs   — optimize_public_images(), ImageOptimizationOptions, WebP encoding
├── commands/
│   ├── dev.rs           — Dev server setup, watcher, worker pool
│   ├── build.rs         — build_with_output(), emit_client_bundles(), prerender_static_routes()
│   ├── check.rs         — TypeScript check + parity test
│   ├── start.rs         — Production server
│   ├── preview.rs       — Preview server
│   ├── routes.rs        — Route table printer
│   ├── analyze.rs       — Route/import/boundary validation
│   ├── doctor.rs        — Project setup check
│   ├── clean.rs         — Remove .ruvyxa/
│   ├── trace.rs         — Single route trace
│   ├── bench.rs         — Benchmarking
│   └── parity.rs        — Dev/prod route comparison + smoke render
├── config.rs            — ProjectConfig struct, ConfigRendererOutput, path validation, limit checks
├── build/
│   ├── orchestrate.rs   — Full build pipeline orchestration
│   ├── client.rs        — Client bundle emission with shared module dedup
│   ├── prerender.rs     — Static route pre-rendering + artifact cache
│   ├── public.rs        — Public asset processing
│   └── report.rs        — Build report TUI
└── checks.rs            — doctor() checks
```

---

## 15. NPM Package Exports

| Package                                                                  | Exports                                                                             |
| ------------------------------------------------------------------------ | ----------------------------------------------------------------------------------- |
| `ruvyxa`                                                                 | `.` → `dist/index.js`, `./server` → `dist/server.js`, `./config` → `dist/config.js` |
| `create-ruvyxa`                                                          | CLI binary                                                                          |
| `@ruvyxa/core`                                                           | Config types, server API, adapter contracts                                         |
| `@ruvyxa/react`                                                          | `<Image>`, `<Seo>`, hydration, error boundaries                                     |
| `@ruvyxa/adapter-{bun,cloudflare,netlify,node,static,vercel}`            | Platform deployment metadata                                                        |
| `@ruvyxa/cli-{darwin-arm64,linux-arm64,linux-x64,win32-arm64,win32-x64}` | Native binary                                                                       |

### Runtime scripts (packed in `ruvyxa`)

| Script                        | Purpose                          |
| ----------------------------- | -------------------------------- |
| `runtime/ssr-renderer.mjs`    | React SSR via `renderToString()` |
| `runtime/ssg-renderer.mjs`    | SSG rendering                    |
| `runtime/api-renderer.mjs`    | API route execution              |
| `runtime/client-renderer.mjs` | Client bundle compilation        |
| `runtime/config-renderer.mjs` | Config file evaluation           |
| `runtime/action-renderer.mjs` | Server action execution          |
| `runtime/worker-pool.mjs`     | Worker pool child process        |
| `runtime/compiler.mjs`        | JS-side compilation fallback     |
| `runtime/plugin-runner.mjs`   | JS plugin bridge for bundler     |

---

## 16. Adapter Contract

```typescript
interface Adapter {
  name: string
  build(options: AdapterBuildOptions): AdapterBuildOutput | Promise<AdapterBuildOutput>
}

interface AdapterBuildOutput {
  [key: string]: unknown // serialized to build.json
}
```

Loaded and executed by `config-renderer.mjs`. Output persisted in `build.json`. Adapters do **not**
deploy — they return platform metadata (e.g. Vercel config, Cloudflare wrangler config).

---

## 17. Configuration Contract

`ruvyxa.config.ts` — evaluated by Node `config-renderer.mjs`, validated in Rust with
`deny_unknown_fields`. All fields optional.

Full schema in `crates/ruvyxa_cli/src/config.rs` and `packages/@ruvyxa/core/src/config.ts`.

```typescript
export default defineConfig({
  appDir?: "app",
  outDir?: ".ruvyxa",
  runtime?: "node" | "edge" | "static",
  render?: { strategy?, revalidate? },
  server?: { host?, port? },
  css?: { entries: string[] },
  build?: { minify?, map?, treeShake?, split?, workers?, jsx?, target?, warm?, prerenderCache? },
  debug?: { overlay?, traces? },
  image?: { optimize?, quality?, lossless?, workers? },
  security?: { actionLimit?, apiLimit?, pluginLimit?, actionRateLimit?, sameOrigin?, headers? },
  cache?: { routes?, css?, dir? },
  middleware?: { builtin?, layers?, plugins? },
  plugins?: Array<{ name, enforce?, resolveId?, transform?, parallel? }>,
  adapter?: string,
  adapterOptions?: {}
})
```

---

_Keep in sync with `docs/developer-guide.md` and source code when architecture changes. Last
updated: 2026-07-19._
