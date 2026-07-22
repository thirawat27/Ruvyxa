# Compilation Pipeline (`ruvyxa_bundler`)

**Files**: `crates/ruvyxa_bundler/src/` (17 files, ~10000 lines)

The bundler transforms TypeScript/JSX/MD/MDX sources into optimized JavaScript bundles (client
hydration or SSR). Pipeline: resolve → compile → boundary check → link → minify → output.

---

## Type Definitions (`types.rs`)

```rust
pub enum BundleTarget { Client, Ssr }
pub enum JsxRuntime { Classic, Automatic }               // default Automatic
pub enum SplitStrategy { Single, Route }                  // default Single
pub enum EsTarget { Es2018, Es2019, Es2020, Es2022, EsNext }  // default Es2022

pub struct BundleInput {
    pub entry: PathBuf,              // page/route file
    pub project_root: PathBuf,
    pub app_dir: PathBuf,
    pub layouts: Vec<PathBuf>,       // ancestor layout files
    pub request_path: String,
    pub target: BundleTarget,
    pub options: BundleOptions,
}

pub struct BundleOptions {
    pub minify: bool,                          // default true
    pub source_map: bool,                      // default false
    pub tree_shaking: bool,                    // default true
    pub jsx_runtime: JsxRuntime,               // default Automatic
    pub es_target: EsTarget,                   // default Es2022
    pub split_strategy: SplitStrategy,         // default Single
    pub emit_chunk_manifest: bool,             // default false
    pub collect_module_manifest: bool,         // default false
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
    pub estimated_gz_bytes: usize,       // output_bytes * 0.35
    pub minified: bool,
    pub tree_shaken: bool,
    pub duration_ms: u64,
    pub tree_shaken_modules: usize,
    pub cache_hits: usize,
}

pub struct SharedRouteBundleOutput {
    pub code: String,                    // module registry on globalThis.__RUVYXA_SHARED_MODULES__
    pub modules: Vec<PathBuf>,
}

pub struct ChunkManifest {
    pub bundle_id: String,              // blake3(code)[..16]
    pub route: String,
    pub modules: Vec<String>,
    pub output_file: String,            // "{bundle_id}.js"
    pub source_map_file: Option<String>,
    pub size_bytes: usize,
    pub dynamic_imports: Vec<DynamicImportChunk>,
}

pub struct DynamicImportChunk {
    pub importer: String,
    pub module: String,
    pub file: String,
}

pub struct OutputChunk {
    pub file_name: String,
    pub code: String,
    pub modules: Vec<String>,
    pub kind: OutputChunkKind,            // DynamicImport | SharedRoute
}

pub enum BundleError {
    Diagnostic(Box<Diagnostic>),
    Io(std::io::Error),
    Compiler(String),
    Unresolved { specifier: String, importer: PathBuf },
    CircularDependency { cycle: String },
}
```

---

## Top-Level API (`lib.rs`)

### `bundle(input: &BundleInput) → Result<BundleOutput>`

**Full pipeline**:

```
1. build_entry_source(input)                  → virtual entry (entry_source, entry_label)
2. resolve_graph_with_hooks(entry, ...)       → Vec<ResolvedModule>
3. compile_graph_with_pipeline_and_maps(...)  → (Vec<CompiledModule>, BTreeMap<PathBuf, String>)
4. boundary::check(modules, input, &mut diag) → diagnostics appended
5. plan_dynamic_chunk_files(...)              → BTreeMap<PathBuf, String>  (client + chunk_manifest only)
6. static_entry_modules(...)                 → Vec<CompiledModule>
7. link_parallel_with_dynamic_imports_and_shared_modules(...)  → String
8. tree_shake_exports(linked)                → String
9. minify_with_options(linked, target, tree_shaking)  → String
10. output::wrap(linked, input)              → String + source_map → output wrapping
11. Source map generation (optional)
12. Chunk manifest + output chunk building (optional)
```

### `prepare_bundle(input) → PreparedBundle`

Partial pipeline: resolve + compile + boundary check. Stops before link/minify. Enables cross-route
shared module discovery.

```rust
let prepared: Vec<PreparedRoute> = routes.iter().map(prepare_bundle).collect();
let shared = extract_shared_modules(&prepared);
let outputs = prepared.into_iter().map(|p| bundle_prepared(p, &shared)).collect();
```

### `bundle_shared_route_modules(modules, input) → SharedRouteBundleOutput`

Compiles shared module set into a registry accessing `globalThis.__RUVYXA_SHARED_MODULES__`.

---

## 1. Resolver (`resolver.rs`)

### `resolve_graph_with_cache(entry_source, entry_label, project_root, app_dir, cache) → Vec<ResolvedModule>`

**Two-phase parallel BFS**:

**Phase 1 (sequential)**: Resolve entry module's deps. Store in
`visited: BTreeMap<PathBuf, ResolvedModule>`.

**Phase 2 (parallel BFS)**:

```
while !frontier.is_empty() {
    frontier = frontier.par_iter()
        .filter_map(|dep_path| {
            cache.read_source(dep_path)?;
            // Fold NODE_ENV for client node_modules modules
            // Compile MD/MDX content first for dep extraction
            // Extract deps via collect_deps_cached()
            // Determine is_external (SSR only)
        })
        .collect();
    // Collect results, build next_frontier from unvisited deps
    // Repeat
}
```

Returns modules in BFS discovery order.

### `ResolvedModule`

```rust
pub struct ResolvedModule {
    pub path: PathBuf,            // Canonical absolute path
    pub source: String,           // Original source text
    pub deps: Vec<PathBuf>,       // Resolved absolute dependency paths
    pub is_external: bool,        // From node_modules
}
```

### `ResolveGraphCache` (lock-free, DashMap-backed)

```rust
pub struct ResolveGraphCache {
    resolutions: Arc<DashMap<(Arc<str>, Arc<str>), Option<PathBuf>>>,  // (base_dir, specifier) → resolved
    sources: Arc<DashMap<PathBuf, CachedSource>>,
    tsconfigs: Arc<DashMap<PathBuf, CachedTsConfig>>,
    dependencies: Arc<DashMap<DependencyCacheKey, Arc<[PathBuf]>>>,     // dep lists cached
    stable_snapshot: bool,  // for_build(): skip metadata revalidation — filesystem immutable
}
```

- DashMap: 64 shards internally, `RwLock` per shard. No single `Mutex` contention.
- **`MMAP_THRESHOLD_BYTES = 65536`** (64KB): files >=64KB use `memmap2::Mmap`. Falls back to
  `fs::read_to_string` on mmap failure.
- Source cache validates `modified_time + len` before returning stale entries.

### Resolution order (per specifier)

```
1. Plugin resolve_id() hook              — first match wins
2. Relative path (./, ../)              → extension probing (20 variants)
3. Absolute path (/)                    → project_root.join()
4. tsconfig paths / baseUrl             → @/Component style aliases
5. Bare specifier                       → package.json "exports" map
6. Project-relative fallback            → root.join(specifier)
```

### Extension probing (`resolve_file_candidate`)

In order:

```
<exact_path>
<path>.ts, .tsx, .js, .jsx, .mts, .cts, .mjs, .cjs, .md, .mdx
<path>/index.ts, index.tsx, index.js, index.jsx, index.mts, index.cts, index.mjs, index.cjs, index.md, index.mdx
```

Asset filtering: non-CSS-module `.css`/`.scss`/`.sass` are excluded from dependency edges
(side-effect only, not added to graph).

### Package exports resolution (`PackageJsonValue`)

```rust
enum PackageJsonValue {
    Null,                              // Explicitly blocked (no access)
    String(String),
    Array(Vec<Self>),                  // Fallback array (try each)
    Object(Vec<(String, Self)>),       // ORDERED entries (preserving declaration order matters)
    Unsupported,                        // boolean, number
}
```

**`resolve_package_exports(pkg_name, export_key, target)`**:

1. Read `node_modules/<pkg>/package.json`, parse `exports` field.
2. **Subpath matching** (keys starting with `.`):
   - Exact match first.
   - Wildcard `*` match: longest prefix+suffix combined wins.
   - `Null` value → blocked.
3. **Condition matching** (condition-keyed objects):
   - Client target: `["browser", "import", "module", "default", "require"]`
   - SSR target: `["node", "import", "module", "default", "require"]`
   - First condition with a non-null value wins.
4. **Target validation**: resolved target must start with `./`, no `..` escaping, stays within
   package root.

### tsconfig paths

```rust
pub struct TsConfigPaths {
    pub config_dir: PathBuf,
    pub base_url: Option<PathBuf>,
    pub paths: Vec<(String, Vec<String>)>,  // e.g. ("@/*", ["./src/*"])
}
```

Resolution algorithm:

1. For each `(pattern, targets)`:
   - If alias is exact match → resolve targets as-is.
   - If alias has `*` suffix → match prefix, extract suffix, substitute `*` in targets with suffix.
2. If no alias matches and specifier is bare + `base_url` set → `base_url.join(specifier)`.
3. Probe filesystem for each resolved candidate.

### `collect_deps_cached` / `collect_deps_uncached`

1. Extract specifiers via `ast::parse_module(source).import_specifiers()`.
2. For each specifier:
   - Plugin `resolve_id` hook (first match wins).
   - Relative → `resolve_specifier` with extension probing.
   - Absolute → `resolve_project_specifier`.
   - Bare → tsconfig paths → package exports → project-relative fallback.
   - External if all fail and bare.
3. Cache dependency lists (blake3 of source + target) if no plugins active.

---

## 2. Compiler (`compiler.rs`)

### `compile_graph_with_cache(graph, input, cache) → Vec<CompiledModule>`

Parallel compilation via `rayon::par_iter()`. Modules dispatched based on file type:

| File type                      | Pipeline                                                               |
| ------------------------------ | ---------------------------------------------------------------------- |
| `.module.css`/`.module.scss`   | `compile_css_module` → `css_module_javascript` → skip Oxc              |
| `.md`/`.mdx`                   | `compile_content_module` → plugin transforms → Oxc                     |
| `.js`/`.mjs`/`.cjs` (no JSX)   | Plugin transforms only → skip Oxc                                      |
| Virtual `ruvyxa:` paths        | Plugin transforms only → skip Oxc                                      |
| `.ts`/`.tsx`/`.jsx` (with JSX) | Plugin → `ast::parse_module` → cache lookup → `transform_with_options` |

### `CompiledModule`

```rust
pub struct CompiledModule {
    pub path: PathBuf,             // Canonical or virtual ("ruvyxa:bundle-entry.tsx")
    pub js: String,                // Plain JS after Oxc transformation
    pub deps: Vec<PathBuf>,        // Resolved absolute dependency paths
    pub is_external: bool,         // From node_modules
    pub cache_hit: bool,           // CompileCache hit
}
```

### Oxc pipeline

```rust
let source_type = SourceType::mjs()
    .with_typescript(true)
    .with_jsx(has_jsx);

// Parse
let parser = Parser::new(&allocator, &source, source_type);
let result = parser.parse();

// Semantic analysis
let semantic = SemanticBuilder::new_compiler()
    .with_enum_eval(true)
    .build(&result.program);

// Transform options
let mut options = TransformOptions::default();
options.jsx.runtime = match jsx_runtime {
    Classic => OxcJsxRuntime::Classic,
    Automatic => OxcJsxRuntime::Automatic,
};
options.jsx.jsx_plugin = has_jsx;
options.jsx.throw_if_namespace = false;
options.jsx.pure = false;
options.typescript.optimize_const_enums = false;
options.typescript.optimize_enums = false;

// Transform
Transformer::new(&allocator, Path::new("ruvyxa:module.tsx"), &options)
    .build_with_scoping(semantic.semantic.into_scoping(), &mut result.program);

// Codegen
let code = Codegen::new().build(&result.program).code;
```

**Decorator stripping pre-pass**: legacy `@Decorator(args?)` patterns removed via char-level scanner
(handles strings, nested parens, preserves line positions). Applied before Oxc parse.

### `transform(source, has_jsx) → Result<String, String>`

Shorthand using classic JSX. Calls `transform_with_options`.

### `transform_with_options(source, has_jsx, jsx_runtime) → Result<String, String>`

Full Oxc pipeline as above. Return plain JS string.

### `CompileCache` (`cache.rs`)

```rust
const MEMORY_CACHE_LIMIT: usize = 512;
const COMPILER_VERSION: &str = concat!("ruvyxa_bundler:", env!("CARGO_PKG_VERSION"), ":ast-plugin-pipeline");

pub struct CompileCache {
    cache_dir: PathBuf,                      // .ruvyxa/cache/bundler/
    enabled: bool,
    namespace: String,
    memory: Arc<Mutex<HashMap<String, MemEntry>>>,  // LRU: 512 entries
    generation: AtomicU64,                    // LRU timing
}

pub enum CacheLookup { Hit(String), Miss(String) }
```

**Cache key**:
`blake3(source + "\0" + has_jsx_flag + "\0" + jsx_runtime + "\0" + COMPILER_VERSION + "\0" + namespace)[..32]`
as hex.

**Disk storage**: `.ruvyxa/cache/bundler/<key>.js`. Atomic writes via temp file + rename.

**LRU eviction**: `generation` counter increments each access. Evicts min `last_used` when
memory > 512.

---

## 3. Boundary Checker (`boundary.rs`)

### `check(modules, input, out: &mut Vec<Diagnostic>) → Result<()>`

Re-checks compiled graph for boundary violations. Two modes:

#### Client bundle checks

| Check                  | Method                                                                                                                                                   | Code    | Severity |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------- | -------- |
| `"server-only"` import | `ast::parse_module(source).imports` contains `specifier == "server-only"`                                                                                | RUV1007 | Error    |
| Private env access     | Byte-level scanner for `process.env.<ID>` and `process.env['<ID>']`. Skips strings, comments, template literals. Allows `NODE_ENV` and `RUVYXA_PUBLIC_*` | RUV1008 | Error    |
| `server/` dir import   | File path starts with `<root>/server/` (project-root level only, NOT `app/.../server/`)                                                                  | RUV1010 | Error    |

#### SSR bundle checks

| Check                  | Method                                                                    | Code    | Severity |
| ---------------------- | ------------------------------------------------------------------------- | ------- | -------- |
| `"client-only"` import | `ast::parse_module(source).imports` contains `specifier == "client-only"` | RUV1009 | Warning  |

### `private_env_reads(source: &str) → Vec<String>`

Byte-level scanner. Recognizes:

- `process.env.NAME` — captures NAME
- `process.env["NAME"]` or `process.env['NAME']` — captures NAME

Handles:

- String literals (skipped, but `${expr}` recurse into)
- Template literals (depth counter for nested `${}`)
- Block comments `/* */`
- Line comments `//`

---

## 4. Linker (`linker.rs`)

### `detect_cycles(modules) → Result<()>`

DFS with explicit stack tracking:

- `visited: BTreeSet<PathBuf>` (black set)
- `stack: Vec<PathBuf>` (gray set, current path)
- If `stack.contains(path)` → extract cycle → `BundleError::CircularDependency { cycle_string }`

### `ordered_project_modules(modules) → Vec<&CompiledModule>`

DFS-based topological sort. `visiting: BTreeSet` (pre), `visited: BTreeSet` (post). Push module
after all deps visited (post-order). Result: dependencies before importers.

### `module_id(path: &Path) → String`

`format!("__ruv_{:016x}__", blake3(path_str)[..8])` — deterministic, path-based. Example:
`__ruv_abcdef1234567890__`.

### Import/export rewriting

Line-by-line scanner. Transforms:

| Pattern                                | Rewrite                                                        |
| -------------------------------------- | -------------------------------------------------------------- |
| `import "./styles.css"`                | `// [bundled] import "./styles.css"`                           |
| `import Default from "./mod"`          | `const Default = __ruv_xxx__.default`                          |
| `import { a, b } from "./mod"`         | `const a = __ruv_xxx__.a; const b = __ruv_xxx__.b`             |
| `import * as ns from "./mod"`          | `const ns = __ruv_xxx__`                                       |
| `import Default, { a } from "./mod"`   | `const Default = __ruv_xxx__.default; const a = __ruv_xxx__.a` |
| `import Default, * as ns from "./mod"` | `const Default = __ruv_xxx__.default; const ns = __ruv_xxx__`  |
| `import type { T } from "./mod"`       | Commented out                                                  |
| `import "side-effect"` (external)      | Kept if `!drop_external_imports`                               |

| `export default expr` | `__exports.default = expr` | | `export default function Foo() {}` |
`function Foo() {} __exports.default = Foo` | | `export { a, b as c } from "./mod"` |
`__exports.a = __ruv_xxx__.a; __exports.c = __ruv_xxx__.b` | | `export * from "./mod"` |
`Object.assign(__exports, __ruv_xxx__)` | | `export const name = val` |
`const name = val; __exports.name = name` | | `export function name() {}` |
`function name() {} __exports.name = name` | | `export { a, b }` |
`__exports.a = a; __exports.b = b` |

| `require("./module")` | `__ruv_xxx__` | | `import("./lazy")` (in chunk map) |
`import("./chunk.<hash>.js").then(m => m.default)` | | `import("./lazy")` (not in chunk map) |
`Promise.resolve(__ruv_xxx__)` |

### IIFE wrapper (per module)

```javascript
var __ruv_<hex16>__ = (function() {
  "use strict";
  var __exports = {};
  var module = { exports: __exports };
  var exports = module.exports;
  var process = globalThis.process || { env: { NODE_ENV: "production" } };
  // ... rewritten module source ...
  return module.exports;
})();
```

### SSR output

Appends `export const render = __ruv_<hex16>__.render;` after IIFE list.

### Shared module registry

```
// shared.js:
var __ruvyxa_shared_modules__ = globalThis.__RUVYXA_SHARED_MODULES__ || (globalThis.__RUVYXA_SHARED_MODULES__ = {});
__ruvyxa_shared_modules__["<module_id>"] = <module_exports>;

// route bundle:
var __shared_<id>__ = __ruvyxa_shared_modules__["<module_id>"];
```

### `link_parallel(modules, input) → Result<String>`

Parallel linking for >=8 modules: generates IIFE segments in parallel via `rayon::par_iter()`, then
concatenates results sequentially.

### Entry functions

- **`link()`**: sequential. Calls `detect_cycles` + `link_inner`.
- **`link_parallel()`**: parallel for >=8 modules.
- **`link_parallel_with_dynamic_imports()`**: passes chunk map for `import()` rewriting.
- **`link_parallel_with_dynamic_imports_and_shared_modules()`**: also handles shared registry.
- **`link_shared_route_modules()`**: generates shared registry only.

---

## 5. Minifier (`minifier.rs`)

### `minify(source, target) → Result<String>`

### `minify_with_options(source, target, tree_shaking) → Result<String>`

Oxc AST minifier:

- **With tree-shaking**: `MinifierOptions::default()` — full mangle + compress.
- **Without tree-shaking**:
  `MinifierOptions { mangle: default, compress: Some(CompressOptions::safest()) }` +
  `CodegenOptions::minify()`.

### `tree_shake_exports(source) → String`

Line-level export pruning:

1. **`collect_used_members(source)`**: scan for all `__ruv_<hex16>__.<member>` patterns across
   entire bundle → `BTreeSet<"module_id.member">`.
2. **Per-line processing**:
   - Track current module via `var __ruv_xxx__ = (function() {` pattern.
   - End module at `})();`.
   - If line is `__exports.<name> = <name>;` and `<name>` not in `used_members` AND
     `<name> != "default"`: Replace with `// [tree-shaken] __exports.<name> = <name>;`.

Default exports always kept.

### `fold_production_node_env(source) → String`

Text-level CommonJS `NODE_ENV` branch folding for node_modules client bundles. Recognized patterns
(after normalizing quotes+whitespace):

```
process.env.NODE_ENV === "production"     → keep consequent
process.env.NODE_ENV == "production"      → keep consequent
"production" === process.env.NODE_ENV     → keep consequent
"production" == process.env.NODE_ENV      → keep consequent
process.env.NODE_ENV !== "production"     → keep alternative
process.env.NODE_ENV != "production"      → keep alternative
"production" !== process.env.NODE_ENV     → keep alternative
"production" != process.env.NODE_ENV      → keep alternative
```

Bounded loop (max 64 iterations): finds innermost matching `if(cond){...}[else{...}]` block,
replaces with appropriate branch body. Handles nested guards, strings, comments, regexes with full
delimiter matching.

---

## 6. Chunking (`chunking.rs`)

### `plan_dynamic_chunk_files(compiled, entry) → BTreeMap<PathBuf, String>`

**Overlapping closures logic**:

1. `dynamic_roots(compiled)` — find modules targeted by `import()` calls.
2. For each root, compute **static transitive closure** (follows static import, re-export,
   side-effect, CommonJS edges; excludes dynamic edges).
3. **Split criterion**: root is split into own chunk ONLY if its closure is:
   - Disjoint from entry's static closure
   - Disjoint from every other dynamic root's closure
   - If overlap exists with anything → root stays in entry bundle.

### `graph_fingerprint(compiled) → String`

Blake3 hash of all non-external modules' paths + JS content. Input to chunk filename hash — when any
dependency changes, all chunk filenames change (stale reference avoidance).

### Chunk filename

`format!("chunk.{:016x}.js", blake3(fingerprint + "\0" + root_path)[..8])`

### `dynamic_import_chunks(compiled, dynamic_import_files) → Vec<DynamicImportChunk>`

Returns `{ importer, module, file }` for each chunked dynamic import.

### `build_dynamic_output_chunks(compiled, input, dynamic_import_files) → Vec<OutputChunk>`

For each chunk: link with dynamic sub-imports, append `export default <module_id>;`, optionally
minify (no tree-shaking).

---

## 7. Output (`output.rs`)

### `build_entry_source(input) → (String, String)` (`output.rs:32`)

Generates virtual entry module. Returns `(entry_source, entry_label="ruvyxa:bundle-entry.tsx")`.

**Client entry**:

```javascript
import React from "react";
import { hydrateRoot } from "react-dom/client";
import Page from "<page_absolute_path>";
import Layout0 from "<layout0_absolute_path>";
// ... more layouts ...

const params = globalThis.__RUVYXA_ROUTE_PARAMS__ ?? {};
const currentPath = globalThis.__RUVYXA_REQUEST_PATH__ ?? "/";

let tree = React.createElement(Page, { params, requestPath: currentPath });
for (const Layout of [Layout0, ...].reverse()) {
    tree = React.createElement(Layout, null, tree);
}

if (globalThis.__RUVYXA_ROOT__) {
    globalThis.__RUVYXA_ROOT__.render(tree);
} else {
    globalThis.__RUVYXA_ROOT__ = hydrateRoot(document, tree);
}
window.__RUVYXA_HYDRATED = true;
```

Layouts iterate in **reverse** — outermost layout wraps innermost.

**SSR entry**:

```javascript
import React from "react";
import { renderToString } from "react-dom/server";
import Page from "<page_absolute_path>";
import Layout0 from "<layout0_absolute_path>";
// ...

export async function render(ctx) {
    let tree = React.createElement(Page, {
        params: ctx.params ?? {},
        requestPath: ctx.path
    });
    for (const Layout of [Layout0, ...].reverse()) {
        tree = React.createElement(Layout, null, tree);
    }
    return "<!doctype html>" + renderToString(tree);
}
```

### `wrap(linked, input) → String`

| Target     | Behavior                                                                                                   |
| ---------- | ---------------------------------------------------------------------------------------------------------- |
| **Client** | Pass through (linker already produces IIFE-wrapped code; browser loads via `<script type="module">`)       |
| **SSR**    | Prepend `// Ruvyxa SSR bundle\n`. Linker already hoists ESM imports + appends `export const render = ...`. |

---

## 8. AST Scanner (`ast.rs`)

### `parse_module(source: &str) → ModuleAst`

Byte-level scanner (not a full parser). Walks source linearly, tracking facts:

```rust
pub struct ModuleAst {
    pub imports: Vec<ImportEdge>,
    pub exports: Vec<String>,        // Named export identifiers
    pub has_jsx: bool,
    pub has_typescript: bool,
    pub has_decorators: bool,
    pub has_enums: bool,
}

pub struct ImportEdge {
    pub specifier: String,
    pub kind: ImportKind,            // Static | Dynamic | Require | ReExport | SideEffect
}
```

**Scanned facts**:

- `import` keyword → determines `ImportKind`:
  - `import(` → `Dynamic`
  - `import "..."` or `import '...'` → `SideEffect`
  - `import type` → skipped entirely (not a runtime import)
  - `import {x} from "..."` → `Static` (uses `find_from_specifier`)
- `export {x} from "..."` → `ReExport`
- `require("...")` → `Require` (if not preceded by `.`)
- `@` on own line → `has_decorators = true`
- `<` followed by identifier → `has_jsx = true`
- `enum` keyword → `has_enums = true`, `has_typescript = true`
- `interface`, `type`, `satisfies`, `implements`, `declare`, `abstract`, `readonly`, `public`,
  `private`, `protected`, `override` → `has_typescript = true`
- `as` after non-whitespace → `has_typescript = true`
- `export default/async function/class/const/let/var <name>` → captures export name

Skips: strings (all quote types), comments (line + block), whitespace.

---

## 9. CSS Modules (`style_module.rs`)

### `is_css_module_path(path) → bool`

Ends with `.module.css`, `.module.scss`, or `.module.sass` (case-insensitive).

### `is_sass_path(path) → bool`

Ends with `.scss` or `.sass` (case-insensitive).

### `compile_sass_file(path, project_root) → Result<String>`

Uses `grass` crate:
`Options::default().style(Expanded).load_path(&project_root).load_path(project_root.join("node_modules"))`.

### `compile_css_module(path, project_root) → Result<CssModule>`

If Sass → `compile_sass_file` first, then `scope_css_module`. If CSS → `scope_css_module` directly.

### `scope_css_module(css, path, project_root) → CssModule`

Character-level scanner with state machine. States tracked:

- `quote: Option<char>` — inside string literal
- `in_comment: bool` — inside `/* */`
- `block_allows_rules: Vec<bool>` — stack tracking whether current block allows selector rules
- `rule_local_classes: Vec<Vec<String>>` — local classes per nested rule
- `prelude: String` — selector text before `{`

**Transforms**:

1. `.local-class` → `.scoped-class__hash`
2. `:global(.selector)` → `.selector` (passes through unmodified)
3. `composes: class1 class2` → appends scoped class names to containing rule's `class` set

### `scoped_class_name(path, project_root, local) → String`

Deterministic naming:

1. `relative` = normalized lowercase path relative to project_root (forward slashes).
2. `digest` = fnv1a_64(format!("{relative}:{local}")).
3. `stem` = alphanumeric-only file_stem (removes `.module` suffix).
4. Result: `{stem}_{local}__{digest:016x}`.

### `css_module_javascript(module) → Result<String>`

Returns: `export default {"local":"scoped","other":"scoped_other__hex"};`

---

## 10. Content Compilation (`content.rs`)

### `compile_content_module(source, path) → Result<String>`

1. `split_frontmatter(source)` → extract YAML between `---` delimiters.
2. `parse_frontmatter(yaml)` → `serde_yaml_ng::from_str<Value>`. Must be a mapping.
3. Branch on extension:
   - `.md`: `markdown::to_mdast(body, ParseOptions::gfm())` → React `createElement` calls.
   - `.mdx`: `markdown::to_mdast(body, mdx_parse_options())` → collect ESM + React `createElement`
     calls.

### MDX parse options

```
Constructs: GFM minus autolink, code_indented, html_flow, html_text
Plus: mdx_esm, mdx_expression_flow, mdx_expression_text, mdx_jsx_flow, mdx_jsx_text
mdx_esm_parse: uses Oxc Parser (TypeScript + JSX MJS) to validate ESM syntax blocks
```

### Output module

```javascript
import React from "react";
// ... MDX ESM blocks (if any) ...
export const frontmatter = { /* parsed YAML */ };
export const meta = frontmatter;
export const headings = [ { depth, text, id }, ... ];
export const contentFormat = "md" | "mdx";
export default function RuvyxaContentPage({ components = {} } = {}) {
    return React.createElement("article", { className: "ruvyxa-content", ... }, ...children);
}
```

**Named export deduplication**: if MDX ESM already exports `frontmatter`, `meta`, `headings`, or
`contentFormat`, auto-generated export omitted.

### AST node lowering

Every Markdown/MDX node lowered to `React.createElement` calls:

- Intrinsic HTML → `"tag"` string
- MDX components → `(components["Tag"] || "Tag")` for lowercased, bare identifier for capitalized
- Dotted custom elements → raw name
- Fragments → `React.Fragment`
- MDX expressions → `({expression})` (inline expression nodes)
- HTML → `React.createElement("span", null, escaped_string)` (XSS-safe escaping)
- Tables → `<table><thead>...</thead><tbody>...</tbody></table>` with alignment styles
- Code blocks → `<pre><code className="language-xxx">...</code></pre>`
- Images → `<img src="..." alt="..." loading="lazy" decoding="async" />`
- Checkboxes → `<input type="checkbox" disabled readOnly />`
- Math → `<span className="math math-inline">` / `<div className="math math-display">`
- Footnotes → `<aside role="doc-footnote">` with back-reference link

### Content cache

```rust
static CONTENT_MODULE_CACHE: OnceLock<Mutex<ContentModuleCache>>;
// HashMap<String, Arc<str>> + VecDeque<String> (insertion order)
// Max 512 entries, LRU eviction
// Key: blake3(extension + "\0" + source).to_hex()
```

---

## 11. Build hooks (`hooks.rs`)

### `BuildHooks` trait

```rust
pub trait BuildHooks: Send + Sync {
    fn name(&self) -> &str;

    fn resolve_id(
        &self, specifier: &str, importer: Option<&Path>, ctx: &BuildHookContext
    ) -> Result<Option<PathBuf>>;  // default: Ok(None)

    fn transform(
        &self, code: &str, id: &Path, ctx: &BuildHookContext
    ) -> Result<Option<TransformResult>>;  // default: Ok(None)
}

pub struct BuildHookContext {
    pub project_root: PathBuf,
    pub importer: Option<PathBuf>,
    pub target: BundleTarget,
}

pub struct TransformResult {
    pub code: String,
    pub map: Option<String>,
}
```

### `BuildHookPipeline`

```rust
pub struct BuildHookPipeline {
    hosts: Arc<Vec<Arc<dyn BuildHooks>>>,
}
```

Hooks execute in order. First `resolve_id` match wins. `transform_with_map` chains transforms,
preserves last non-None source map.

---

## 12. Supplemental modules

### `BundleContext` (`context.rs`)

```rust
pub struct BundleContext {
    compile_cache: CompileCache,
    graph_cache: ResolveGraphCache,
    incremental: IncrementalGraphCache,
    build_hooks: BuildHookPipeline,
}
```

Constructors: `new(project_root)`, `with_caches(...)`, `with_all_caches(...)`, and
`with_build_hooks(...)`.

### `IncrementalGraphCache` (`incremental.rs`)

```rust
pub struct IncrementalGraphCache {
    manifest_path: PathBuf,          // .ruvyxa/cache/graph/manifest.json
    previous: GraphManifest,
    current: GraphManifest,
    enabled: bool,
}

pub struct CachedModuleEntry {
    pub content_hash: String,        // blake3[..32]
    pub size: u64,
    pub mtime_secs: u64,
    pub deps: Vec<PathBuf>,
    pub compile_key: Option<String>,
}
```

Value: version == 1, dependency_hash match, render_context_hash match, all file fingerprints match.
Dirty set: BFS over reverse dependency graph from changed paths.

### `SourceMapBuilder` (`sourcemap.rs`)

```rust
pub struct SourceMapBuilder {
    file: String,
    sources: Vec<String>,
    sources_content: Vec<Option<String>>,
    mappings: Vec<Mapping>,
    source_root: PathBuf,
    names: Vec<String>,
    ignore_list: Vec<u32>,
}

pub struct Mapping {
    pub gen_line: u32, pub gen_col: u32,
    pub source_idx: u32, pub orig_line: u32, pub orig_col: u32,
}
```

Supports: identity mappings, vlq encoding/decoding, `x_google_ignoreList`, importing v3 source maps
with line offset.
