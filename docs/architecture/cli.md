# CLI & Build Pipeline (`ruvyxa_cli`)

**Files**: `crates/ruvyxa_cli/src/main.rs` (6037 lines), `crates/ruvyxa_cli/src/image_optimizer.rs`
(443 lines)

Command dispatch via clap, config loading from `ruvyxa.config.ts` (evaluated by the selected
Node/Bun runtime), build orchestration, and image optimization.

---

## Command Structure

```rust
#[derive(clap::Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

pub enum Command {
    Dev(ServerArgs),              // --root, --host, --port
    Build(BuildArgs),             // --root, --target
    Check(ProjectArgs),           // --root
    Start(ServerArgs),            // --root, --host, --port
    Preview(ServerArgs),          // --root, --host, --port
    Routes(ProjectArgs),          // --root
    Analyze(ProjectArgs),         // --root
    Doctor(ProjectArgs),          // --root
    Clean(ProjectArgs),           // --root
    Trace(TraceArgs),             // <route> --root
    Bench(BenchArgs),             // --root, --samples (3), --json
    TestParity(ProjectArgs),      // --root  (alias: parity)
}

pub struct ProjectArgs { pub root: Option<PathBuf> }  // default "."
pub struct ServerArgs { pub root: Option<PathBuf>, pub host: Option<String>, pub port: Option<u16> }
pub struct BuildArgs { pub root: Option<PathBuf>, pub target: Option<BuildTarget> }
pub struct TraceArgs { pub route: String, pub root: Option<PathBuf> }
pub struct BenchArgs { pub root: Option<PathBuf>, pub samples: Option<usize>, pub json: bool }
```

| Command       | What it does                              |
| ------------- | ----------------------------------------- |
| `dev`         | Start dev server with HMR                 |
| `build`       | Production build → `.ruvyxa/`             |
| `check`       | `tsc --noEmit` + `test:parity`            |
| `start`       | Serve production build                    |
| `preview`     | Preview production build locally          |
| `routes`      | Print discovered route table              |
| `analyze`     | Validate routes/imports/boundaries        |
| `doctor`      | Check project setup                       |
| `clean`       | Remove `.ruvyxa/`                         |
| `trace`       | Inspect one route by path (JSON)          |
| `bench`       | Benchmark (discovery, analysis, build)    |
| `test:parity` | Dev/prod route comparison + smoke renders |

---

## Dev Command

```rust
fn dev(args: ServerArgs) -> Result<()> {
    let config = load_project_config(&args.root)?;
    let server_config = dev_server_config(&args, &config);
    ruvyxa_dev_server::serve(server_config).await
}
```

Maps ProjectConfig fields to `ServerConfig::dev()`.

## Build Command

```rust
fn build(args: BuildArgs) -> Result<()> {
    build_with_output(args, true)  // produce output = true
}
```

Full pipeline (see [Build Pipeline](#build-pipeline) below).

## Check Command

```rust
fn check(args: ProjectArgs) -> Result<()> {
    run_typecheck(&args.root)?;      // tsc --noEmit
    test_parity(ProjectArgs { ... }).await  // full parity test
}
```

## Start Command

```rust
fn start(args: ServerArgs) -> Result<()> {
    let config = load_project_config(&args.root)?;
    let server_config = production_server_config(&args, &config);
    // app_dir → out_dir/server/app
    // public_dir → out_dir/assets
    ruvyxa_dev_server::serve(server_config).await
}
```

---

## Configuration System

### Two-phase loading

**Phase 1: Node/Bun evaluation**

```rust
fn load_project_config(root: &Path) -> Result<ProjectConfig> {
    let renderer = find_runtime_script(root, "config-renderer.mjs")?;
    // If not found → return ProjectConfig::default() with hash "no-config"

    let output = Command::new(runtime.executable())
        .arg(&renderer)
        .arg(root)
        .output()?;

    let result: ConfigRendererOutput = serde_json::from_slice(&output.stdout)?;
    if !result.ok {
        return Err(RuvyxaError::Message(format!(
            "config evaluation failed: {} - {}",
            result.code.unwrap_or_default(),
            result.message.unwrap_or_default()
        )));
    }

    let mut config = result.config.unwrap_or_default();
    config.config_dependency_hash = result.dependency_hash.unwrap_or_default();
    config.validate_paths(root)?;
    Ok(config)
}
```

**ConfigRendererOutput**:

```rust
struct ConfigRendererOutput {     // #[serde(rename_all = "camelCase")]
    ok: bool,
    config: Option<ProjectConfig>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
    dependency_hash: Option<String>,
}
```

`config-renderer.mjs` evaluated `ruvyxa.config.ts`, maps `defineConfig(...)` output, serializes to
JSON stdout.

**Phase 2: Rust validation**

```rust
impl ProjectConfig {
    fn validate_paths(&self, root: &Path) -> Result<()> {
        validate_project_relative_path("appDir", &self.app_dir(), root)?;
        validate_project_relative_path("outDir", &self.out_dir(), root)?;
        for (i, entry) in self.css.entries.iter().enumerate() {
            validate_project_relative_path(&format!("css.entries[{}]", i), entry, root)?;
        }
        validate_bounded_limit("actionLimit", self.security.action_body_limit_bytes)?;
        validate_bounded_limit("apiLimit", self.security.api_body_limit_bytes)?;
        validate_plugin_response_limit(self.security.plugin_response_body_limit_bytes)?;
        validate_trusted_proxy_ips(&self.security.trusted_proxy_ips)?;
        self.parse_jsx_runtime()?;
        Ok(())
    }
}
```

- `validate_project_relative_path`: reject absolute, root-dir, parent-dir. Reject path traversal
  (`..`).
- `validate_bounded_limit`: must be > 0 and ≤ `MAX_BODY_LIMIT`.
- `validate_plugin_response_limit`: must be > 0 and ≤ `MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES`.
- `validate_trusted_proxy_ips`: each parses as `std::net::IpAddr`.
- `parse_jsx_runtime`: `"jsx"` config value → `JsxRuntime::Automatic` (default) or `Classic`.

### `ProjectConfig` struct

```rust
#[derive(Debug, Clone, Default, Deserialize)]      // #[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfig {
    pub app_dir: Option<String>,                    // default "app"
    pub out_dir: Option<String>,                    // default ".ruvyxa"
    pub runtime: Option<BuildTarget>,
    #[serde(rename = "react")]
    pub _react: Option<serde_json::Value>,          // reserved, unused
    #[serde(rename = "typescript")]
    pub _typescript: Option<serde_json::Value>,     // reserved, unused

    #[serde(default, rename = "render")]
    pub rendering: RenderingConfigOptions,
    #[serde(default)]
    pub server: ServerConfigOptions,
    #[serde(default)]
    pub css: CssConfigOptions,
    #[serde(default)]
    pub build: BuildConfigOptions,
    #[serde(default)]
    pub debug: DebugConfigOptions,
    #[serde(default, rename = "image")]
    pub images: ImageOptimizationOptions,
    #[serde(default)]
    pub security: SecurityConfigOptions,
    #[serde(default)]
    pub cache: CacheConfigOptions,
    #[serde(default)]
    pub middleware: MiddlewareConfig,
    #[serde(default)]
    pub plugins: Vec<BuildPluginConfig>,
    #[serde(rename = "adapter")]
    pub adapter: Option<serde_json::Value>,
    #[serde(rename = "adapterOptions")]
    pub adapter_options: Option<serde_json::Value>,

    #[serde(skip)]
    pub config_dependency_hash: String,
}

// Sub-config structs:
pub struct RenderingConfigOptions {
    #[serde(rename = "strategy")]
    pub default_strategy: Option<RenderStrategy>,
    #[serde(rename = "revalidate")]
    pub default_revalidate: Option<u64>,
}

pub struct ServerConfigOptions {
    pub host: Option<String>,
    pub port: Option<u16>,
}

pub struct CssConfigOptions {
    #[serde(default)]
    pub entries: Vec<String>,
}

pub struct BuildConfigOptions {
    pub minify: Option<bool>,
    #[serde(rename = "map")]
    pub sourcemap: Option<bool>,
    #[serde(rename = "treeShake")]
    pub tree_shaking: Option<bool>,
    #[serde(rename = "split")]
    pub split_strategy: Option<String>,
    #[serde(rename = "workers")]
    pub parallelism: Option<usize>,
    #[serde(rename = "jsx")]
    pub jsx_runtime: Option<String>,
    #[serde(rename = "target")]
    pub es_target: Option<String>,
    #[serde(rename = "manifest")]
    pub emit_chunk_manifest: Option<bool>,
    #[serde(rename = "warm")]
    pub prebundle_dependencies: Option<bool>,
    #[serde(rename = "prerenderCache")]
    pub prerender_cache: Option<bool>,
}

pub struct DebugConfigOptions {
    pub overlay: Option<bool>,
    pub traces: Option<bool>,
}

pub struct ImageOptimizationOptions {
    pub optimize: Option<bool>,    // default true
    pub quality: Option<u8>,       // default 82
    pub lossless: Option<bool>,    // default false
    pub workers: Option<usize>,    // default 0 = rayon global
}

pub struct SecurityConfigOptions {
    #[serde(rename = "actionLimit")]
    pub action_body_limit_bytes: Option<usize>,
    #[serde(rename = "apiLimit")]
    pub api_body_limit_bytes: Option<usize>,
    #[serde(rename = "pluginLimit")]
    pub plugin_response_body_limit_bytes: Option<usize>,
    #[serde(rename = "actionRateLimit")]
    pub action_rate_limit: Option<ActionRateLimitOptions>,
    #[serde(rename = "sameOrigin")]
    pub same_origin_actions: Option<bool>,
    #[serde(rename = "fetchMeta")]
    pub fetch_metadata_actions: Option<bool>,
    #[serde(default, rename = "trustedProxyIps")]
    pub trusted_proxy_ips: Vec<String>,
    #[serde(rename = "headers")]
    pub security_headers: Option<bool>,
}

pub struct ActionRateLimitOptions {
    pub max: usize,
    pub window: u64,
}

pub struct CacheConfigOptions {
    #[serde(rename = "routes")]
    pub route_manifest: Option<bool>,
    pub css: Option<bool>,
    #[serde(rename = "dir")]
    pub build_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildPluginConfig {
    pub name: String,
}
```

---

## Build Pipeline (`build_with_output`)

```rust
fn build_with_output(args: BuildArgs, produce_output: bool) -> Result<()>
```

### Phase 1: Config load

```rust
let config = load_project_config(&args.root)?;
```

### Phase 2: Route discovery

```rust
let discover_opts = DiscoverOptions::new(app_dir)
    .with_rendering_defaults(
        config.rendering.default_strategy,
        config.rendering.default_revalidate,
    );
let manifest = discover_routes(discover_opts)?;
```

### Phase 3: Validation

```rust
let report = validate_app(&args.root, &manifest)?;
if !report.is_ok() {
    // Print diagnostics, bail
}
```

### Phase 4: Style collection

```rust
let styles = collect_styles(&args.root, &app_dir, &config.style_entries)?;
```

### Phase 5: Staging directory

```rust
let out_dir = args.root.join(config.out_dir());
let staging = out_dir.join(".ruvyxa-staging-<random>");
// Copy directories:
copy_dir(app_dir, staging.join("server/app"))?;
copy_dir_if_exists(components_dir, staging.join("server/components"))?;
copy_dir_if_exists(server_dir, staging.join("server/server"))?;
// Copy style files to staging
```

### Phase 6: Image optimization

```rust
if produce_output {
    optimize_public_images(
        &args.root.join("public"),
        &staging.join("assets"),
        &out_dir.join("cache/images"),
        &config.images,
    )?;
}
```

### Phase 7: Write manifest

```rust
write_manifest(&manifest, staging.join("manifest.json"))?;
```

### Phase 8: Emit client bundles

```rust
if produce_output {
    let client_manifest = emit_client_bundles(
        &manifest, &config, &args.root, &app_dir, &staging,
    )?;
    write_json(staging.join("client/manifest.json"), &client_manifest)?;
}
```

**`emit_client_bundles` details**:

```rust
fn emit_client_bundles(
    manifest: &RouteManifest, config: &ProjectConfig,
    root: &Path, app_dir: &Path, staging: &Path,
) -> Result<ClientBundleManifest>
```

1. Filter page routes only.
2. Determine parallelism: `config.build.parallelism.unwrap_or_else(num_cpus::get)`.
3. Build `BundleContext` (with caches). If plugins are configured, create the ordered plugin hook
   host.
4. Split strategy:
   - **Route** (default): prepare all routes in parallel → detect shared modules across >=2 routes →
     emit `shared.js` → emit per-route bundles importing shared registry.
   - **Single**: emit each route independently.
5. Write output JS + source maps + chunk manifest.
6. Print build stats (module counts, sizes, cache hits).

### Phase 9: Pre-render static routes

```rust
if produce_output {
    let prerender_result = prerender_static_routes(
        &manifest, config, &staging, &args.root,
    )?;
    write_json(staging.join("prerender/manifest.json"), &prerender_result)?;
}
```

**`prerender_static_routes` details**:

Filter SSG/ISR/PPR/CSR routes. For each:

| Strategy               | Action                                                              |
| ---------------------- | ------------------------------------------------------------------- |
| CSR                    | Emit minimal HTML shell                                             |
| SSG with params        | `resolve_static_params()` via worker pool → render each param combo |
| SSG static             | Single render                                                       |
| ISR                    | Single render (dev) or skip (prod — request-time)                   |
| PPR with params        | `resolve_static_params()` → `render_ssg(mode="ppr")`                |
| PPR static             | `render_ssg(mode="ppr")`                                            |
| Dynamic without params | Skip (request-time only)                                            |

Prerender jobs dispatched with max parallelism 2. Each job writes HTML to
`prerender/<path>/index.html`.

**Prerender artifact cache**:

```rust
struct PrerenderArtifactCache {
    directory: PathBuf,                    // out_dir/cache/prerender
    dependency_hash: String,
    render_context_hash: String,
    fingerprints: Arc<ArtifactFingerprintCache>,
    enabled: bool,
}
```

Cache validation: version==1, dependency_hash match, render_context_hash match, all file
fingerprints match. On hit → hardlink cached HTML to output. On miss → render + write to cache.

### Phase 10: Build metadata

```rust
let build_info = BuildInfo {
    version: env!("CARGO_PKG_VERSION"),
    timestamp: chrono::Utc::now(),
    routes: manifest.routes.len(),
    page_routes: report.page_routes,
    api_routes: report.api_routes,
    target: args.target,
    config_hash: config.config_dependency_hash.clone(),
    image_report: image_report.summary(),
    client_bundle_manifest: ...,
    prerender_manifest: ...,
};
write_json(staging.join("build.json"), &build_info);
```

### Phase 11: Atomic commit

```rust
commit_staged_build_outputs(&staging, &out_dir)?;
// Replaces/creates out_dir atomically:
// 1. If out_dir exists: rename to out_dir + ".old"
// 2. Rename staging → out_dir
// 3. Remove old (failure → restore from old)
```

### Phase 12: Print report

```rust
print_build_report(&build_info, &styles, elapsed);
// Route table, sizes, timing summary, image report
```

---

## Image Optimizer (`image_optimizer.rs`)

### `ImageOptimizationOptions`

```rust
pub struct ImageOptimizationOptions {
    pub optimize: bool,        // default true
    pub quality: u8,           // default 82
    pub lossless: bool,        // default false
    pub workers: usize,        // default 0 = rayon global default
}
```

### `optimize_public_images(public_dir, assets_dir, cache_dir, options) → ImageReport`

```rust
pub struct ImageReport {
    pub input_files: usize,
    pub output_files: usize,
    pub optimized: usize,
    pub copied: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub duration_ms: u64,
}
```

Algorithm:

1. **Discover**: walk `public_dir` recursively, collect all files.
2. **Collision check**: if both `name.png` and `name.jpg` exist → error (same output stem).
3. **Process each file**:

```rust
for entry in entries {
    let ext = entry.extension().to_lowercase();

    // Non-optimizable formats → copy as-is
    if !matches!(ext, "png" | "jpg" | "jpeg") || !options.optimize {
        fs::copy(entry, assets_dir.join(entry.file_name()))?;
        continue;
    }

    // Decode
    let img = match image::open(&entry) {
        Ok(img) => img,
        Err(_) => {
            // Decode failed → copy as-is
            fs::copy(entry, assets_dir.join(entry.file_name()))?;
            continue;
        }
    };

    // Cache key
    let cache_key = blake3::hash(format!(
        "{}:{}\n{}",
        options.quality,
        options.lossless as u8,
        blake3::hash(&fs::read(&entry)?)
    )).to_hex();

    let cache_file = cache_dir.join(&cache_key).with_extension("webp");

    // Cache hit → hardlink
    if cache_file.exists() {
        fs::hard_link(&cache_file, output_file)?;
        continue;
    }

    // Encode WebP
    let encoder = webp::Encoder::from_image(&img)?;
    let webp = if options.lossless {
        encoder.encode_lossless()
    } else {
        encoder.encode(options.quality as f32)
    };

    // Write to cache + hardlink to output
    fs::write(&cache_file, &*webp)?;
    fs::hard_link(&cache_file, output_file)?;
}
```

4. **Parallelism**: `rayon::ThreadPoolBuilder::new().num_threads(options.workers).build()` if
   workers specified, else default global pool.

5. **Write manifest**: `.ruvyxa-images.json` with
   `{ files: [{ input, output, width, height, format, optimized }] }`.

---

## Plugin Build Hook Bridge

Bridges Rust bundler plugin system to JS plugins configured in `ruvyxa.config.ts`:

```rust
struct PluginBuildHookHost {
    workers: Vec<Arc<JsPluginWorker>>,
    next_worker: AtomicU64,
    plugins: Vec<BuildPluginConfig>,
}

struct PluginWorker {
    child: Mutex<Option<Child>>,  // Node/Bun subprocess running plugin-runtime.mjs
    stdin: StdMutex<mpsc::Sender<String>>,
    // NDJSON communication
}
```

**`resolveId`**: sends the module specifier, importer, and environment to the plugin runtime and
returns a resolved path or no result.

**`transform`**: sends source code, module id, and environment to the plugin runtime and returns
transformed code plus an optional source map.

One persistent runtime owns the setup registry, so closures and module-level plugin state are shared
across build calls. `onBuildComplete` runs after the committed production output.

---

## Dev/Production Server Config Mapping

### `dev_server_config(args, config) → ServerConfig`

```rust
ServerConfig {
    root: args.root,
    app_dir: root / config.app_dir(),
    public_dir: root / "public",
    client_dir: out_dir / "client",
    prerender_dir: Some(out_dir / "prerender"),
    host: args.host.unwrap_or(config.server.host.unwrap_or(DEFAULT_HOST)),
    port: args.port.unwrap_or(config.server.port.unwrap_or(DEFAULT_PORT)),
    watch: true,
    error_overlay: config.debug.overlay.unwrap_or(true),
    debug_traces: config.debug.traces.unwrap_or(false),
    action_body_limit_bytes: config.security.action_body_limit_bytes.unwrap_or(DEFAULT),
    action_rate_limit_max: action_rate.max,
    action_rate_limit_window: Duration::from_secs(action_rate.window),
    // ... map all other fields from config
}
```

### `production_server_config(args, config) → ServerConfig`

Same structure but:

- `app_dir = out_dir / "server" / config.app_dir()` (compiled output)
- `public_dir = out_dir / "assets"` (optimized images)
- `watch = false`
- `error_overlay = false`
