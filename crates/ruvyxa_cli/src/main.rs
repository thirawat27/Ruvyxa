use std::collections::BTreeMap;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::SystemTime;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use ruvyxa_dev_server::{serve, ServerConfig};
use ruvyxa_diagnostics::Diagnostic;
use ruvyxa_graph::{
    discover_routes, validate_app, write_manifest, DiscoverOptions, RouteEntry, RouteManifest,
};
use tracing::info;
use walkdir::WalkDir;

#[derive(Debug, Parser)]
#[command(name = "ruvyxa")]
#[command(about = "Rust-powered full-stack TypeScript framework")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Dev(ServerArgs),
    Build(BuildArgs),
    Start(ServerArgs),
    Preview(ServerArgs),
    Routes(ProjectArgs),
    Analyze(ProjectArgs),
    Doctor(ProjectArgs),
    Clean(ProjectArgs),
    Trace(TraceArgs),
    Bench(BenchArgs),
    #[command(name = "test:parity", alias = "parity")]
    TestParity(ProjectArgs),
}

#[derive(Debug, Parser)]
struct ProjectArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

#[derive(Debug, Parser)]
struct ServerArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    #[arg(long)]
    host: Option<String>,

    #[arg(long)]
    port: Option<u16>,
}

#[derive(Debug, Parser)]
struct BuildArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    #[arg(long, value_enum, default_value_t = BuildTarget::Node)]
    target: BuildTarget,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BuildTarget {
    Node,
    Edge,
    Static,
}

#[derive(Debug, Parser)]
struct TraceArgs {
    route: String,

    #[arg(long, default_value = ".")]
    root: PathBuf,
}

#[derive(Debug, Parser)]
struct BenchArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    #[arg(long, default_value_t = 3)]
    samples: usize,

    #[arg(long)]
    json: bool,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectConfig {
    app_dir: Option<String>,
    out_dir: Option<String>,
    #[serde(default)]
    server: ServerConfigOptions,
    #[serde(default)]
    build: BuildConfigOptions,
    #[serde(default)]
    security: SecurityConfigOptions,
    #[serde(default)]
    cache: CacheConfigOptions,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfigOptions {
    host: Option<String>,
    port: Option<u16>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildConfigOptions {
    minify: Option<bool>,
    sourcemap: Option<bool>,
    split_strategy: Option<String>,
    parallelism: Option<usize>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecurityConfigOptions {
    action_body_limit_bytes: Option<usize>,
    same_origin_actions: Option<bool>,
    fetch_metadata_actions: Option<bool>,
    security_headers: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheConfigOptions {
    route_manifest: Option<bool>,
    css: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigRendererOutput {
    ok: bool,
    config: Option<ProjectConfig>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

impl ProjectConfig {
    fn app_dir(&self) -> &str {
        self.app_dir.as_deref().unwrap_or("app")
    }

    fn out_dir(&self) -> &str {
        self.out_dir.as_deref().unwrap_or(".ruvyxa")
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .without_time()
        .with_target(false)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Dev(args) => {
            let config = load_project_config(&args.root)?;
            serve(dev_server_config(&args, &config))
                .await
                .context("dev server failed")?;
        }
        Command::Build(args) => build(args).context("build failed")?,
        Command::Start(args) | Command::Preview(args) => {
            let config = load_project_config(&args.root)?;
            serve(production_server_config(&args, &config))
                .await
                .context("production server failed")?;
        }
        Command::Routes(args) => print_routes(args).context("route discovery failed")?,
        Command::Analyze(args) => analyze(args).context("analyze failed")?,
        Command::Doctor(args) => doctor(args).context("doctor failed")?,
        Command::Clean(args) => clean(args).context("clean failed")?,
        Command::Trace(args) => trace(args).context("trace failed")?,
        Command::Bench(args) => bench(args).context("benchmark failed")?,
        Command::TestParity(args) => test_parity(args).context("parity test failed")?,
    }

    Ok(())
}

fn dev_server_config(args: &ServerArgs, config: &ProjectConfig) -> ServerConfig {
    let mut server = ServerConfig::dev(
        &args.root,
        args.host
            .clone()
            .or_else(|| config.server.host.clone())
            .unwrap_or_else(|| "localhost".to_string()),
        args.port.or(config.server.port).unwrap_or(3000),
    );
    let out_dir = args.root.join(config.out_dir());
    server.app_dir = args.root.join(config.app_dir());
    server.public_dir = args.root.join("public");
    server.client_dir = out_dir.join("client");
    server.cache_route_manifest = config.cache.route_manifest.unwrap_or(true);
    server.cache_css = config.cache.css.unwrap_or(true);
    server
}

fn production_server_config(args: &ServerArgs, config: &ProjectConfig) -> ServerConfig {
    let mut server = ServerConfig::production(
        &args.root,
        args.host
            .clone()
            .or_else(|| config.server.host.clone())
            .unwrap_or_else(|| "localhost".to_string()),
        args.port.or(config.server.port).unwrap_or(3000),
    );
    let out_dir = args.root.join(config.out_dir());
    server.app_dir = out_dir.join("server").join(config.app_dir());
    server.public_dir = out_dir.join("assets");
    server.client_dir = out_dir.join("client");
    server.cache_route_manifest = config.cache.route_manifest.unwrap_or(true);
    server.cache_css = config.cache.css.unwrap_or(true);
    server
}

fn load_project_config(root: &Path) -> anyhow::Result<ProjectConfig> {
    let Some(renderer) = find_runtime_script(root, "config-renderer.mjs") else {
        return Ok(ProjectConfig::default());
    };

    let output = ProcessCommand::new("node")
        .arg(&renderer)
        .arg(root)
        .output()
        .with_context(|| format!("failed to load config for {}", root.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: ConfigRendererOutput = serde_json::from_str(&stdout).with_context(|| {
        format!(
            "config renderer returned invalid output for {}\nstdout:\n{}",
            root.display(),
            stdout
        )
    })?;

    if output.status.success() && result.ok {
        return Ok(result.config.unwrap_or_default());
    }

    anyhow::bail!(
        "config load failed: {} {}",
        result.code.unwrap_or_else(|| "RUV1600".to_string()),
        result
            .message
            .or(result.stack)
            .unwrap_or_else(|| "unknown config error".to_string())
    )
}

fn build(args: BuildArgs) -> anyhow::Result<()> {
    build_with_output(args, true)
}

fn build_with_output(args: BuildArgs, show_summary: bool) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let app_dir = args.root.join(config.app_dir());
    let out_dir = args.root.join(config.out_dir());
    let server_dir = out_dir.join("server");
    let client_dir = out_dir.join("client");
    let assets_dir = out_dir.join("assets");

    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)
            .with_context(|| format!("failed to clean {}", out_dir.display()))?;
    }

    let manifest = discover_routes(DiscoverOptions::new(&app_dir))?;
    let validation = validate_app(&args.root, &manifest)?;
    fail_on_diagnostics(&validation.diagnostics)?;

    copy_dir_all(&app_dir, &server_dir.join("app"))?;
    copy_optional_dir(
        &args.root.join("components"),
        &server_dir.join("components"),
    )?;
    copy_optional_dir(&args.root.join("server"), &server_dir.join("server"))?;
    copy_public_assets(&args.root, &assets_dir)?;
    fs::create_dir_all(&client_dir)?;
    write_manifest(&manifest, &out_dir.join("manifest.json"))?;

    let client_manifest = emit_client_bundles(
        &args.root,
        &app_dir,
        &manifest,
        &client_dir,
        config.build.minify.unwrap_or(true),
        config.build.parallelism,
    )?;
    fs::write(
        client_dir.join("manifest.json"),
        serde_json::to_string_pretty(&client_manifest)?,
    )?;

    let build_info = serde_json::json!({
        "framework": "Ruvyxa",
        "version": env!("CARGO_PKG_VERSION"),
        "target": format!("{:?}", args.target).to_lowercase(),
        "profile": "production",
        "createdAtUnix": SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default(),
        "routes": manifest.routes.len(),
        "serverDir": "server",
        "clientDir": "client",
        "assetsDir": "assets",
        "hashAlgorithm": "blake3-128",
        "security": {
            "actionBodyLimitBytes": config.security.action_body_limit_bytes.unwrap_or(65536),
            "sameOriginActions": config.security.same_origin_actions.unwrap_or(true),
            "fetchMetadataActions": config.security.fetch_metadata_actions.unwrap_or(true),
            "securityHeaders": config.security.security_headers.unwrap_or(true)
        },
        "build": {
            "minify": config.build.minify.unwrap_or(true),
            "sourcemap": config.build.sourcemap.unwrap_or(false),
            "splitStrategy": config.build.split_strategy.unwrap_or_else(|| "route".to_string()),
            "parallelism": client_manifest.get("parallelism").cloned().unwrap_or(serde_json::Value::Null)
        }
    });
    fs::write(
        out_dir.join("build.json"),
        serde_json::to_string_pretty(&build_info)?,
    )?;

    info!(
        target = ?args.target,
        routes = manifest.routes.len(),
        output = %out_dir.display(),
        "build complete"
    );
    if show_summary {
        println!(
            "\n{}\n  {} {}\n  {} {}\n  {} Built into {}\n",
            heading("Ruvyxa build"),
            label("target"),
            accent(format!("{:?}", args.target).to_lowercase()),
            label("routes"),
            accent(manifest.routes.len().to_string()),
            success(),
            path_text(&out_dir)
        );
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientBundleOutput {
    ok: bool,
    script: Option<String>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

struct ClientBundle {
    path: String,
    entry: PathBuf,
    file_name: String,
    script: String,
}

fn emit_client_bundles(
    root: &Path,
    app_dir: &Path,
    manifest: &RouteManifest,
    client_dir: &Path,
    minify: bool,
    configured_parallelism: Option<usize>,
) -> anyhow::Result<serde_json::Value> {
    let renderer = find_runtime_script(root, "client-renderer.mjs")
        .context("client renderer was not found for production build")?;
    let page_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .cloned()
        .collect::<Vec<_>>();
    let available_parallelism = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let parallelism = configured_parallelism
        .unwrap_or(available_parallelism)
        .clamp(1, page_routes.len().max(1));
    let chunk_size = page_routes.len().max(1).div_ceil(parallelism);
    let mut bundles = Vec::new();

    std::thread::scope(|scope| -> anyhow::Result<()> {
        let mut handles = Vec::new();

        for (chunk_index, chunk) in page_routes.chunks(chunk_size).enumerate() {
            let routes = chunk.to_vec();
            let offset = chunk_index * chunk_size;
            let renderer = renderer.clone();

            handles.push(
                scope.spawn(move || -> anyhow::Result<Vec<(usize, ClientBundle)>> {
                    routes
                        .iter()
                        .enumerate()
                        .map(|(index, route)| {
                            bundle_client_route(root, app_dir, &renderer, route, minify)
                                .map(|bundle| (offset + index, bundle))
                        })
                        .collect()
                }),
            );
        }

        for handle in handles {
            let mut next = handle
                .join()
                .map_err(|_| anyhow::anyhow!("client bundler worker panicked"))??;
            bundles.append(&mut next);
        }

        Ok(())
    })?;

    bundles.sort_by_key(|(index, _)| *index);

    let mut routes = Vec::new();
    for (_, bundle) in bundles {
        fs::write(client_dir.join(&bundle.file_name), bundle.script.as_bytes())?;
        routes.push(serde_json::json!({
            "path": bundle.path,
            "entry": bundle.entry,
            "file": bundle.file_name,
            "src": format!("/__ruvyxa/client/{}", bundle.file_name),
            "bytes": bundle.script.len(),
            "optimized": true,
            "treeShaken": true,
            "chunkStrategy": "route"
        }));
    }

    Ok(serde_json::json!({
        "chunkStrategy": "route",
        "minify": minify,
        "treeShaking": true,
        "parallelism": parallelism,
        "routes": routes
    }))
}

fn bundle_client_route(
    root: &Path,
    app_dir: &Path,
    renderer: &Path,
    route: &RouteEntry,
    minify: bool,
) -> anyhow::Result<ClientBundle> {
    let output = ProcessCommand::new("node")
        .env("RUVYXA_CLIENT_MINIFY", if minify { "1" } else { "0" })
        .arg(renderer)
        .arg(root)
        .arg(app_dir)
        .arg(&route.file)
        .arg(&route.path)
        .arg("{}")
        .output()
        .with_context(|| format!("failed to bundle client route {}", route.path))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: ClientBundleOutput = serde_json::from_str(&stdout).with_context(|| {
        format!(
            "client renderer returned invalid output for {}\nstdout:\n{}",
            route.path, stdout
        )
    })?;

    if !output.status.success() || !result.ok {
        anyhow::bail!(
            "client bundle failed for {}: {} {}",
            route.path,
            result.code.unwrap_or_else(|| "RUV1300".to_string()),
            result
                .message
                .or(result.stack)
                .unwrap_or_else(|| "unknown client build error".to_string())
        );
    }

    let script = result
        .script
        .context("client renderer completed without script output")?;
    let file_name = format!("{}.js", content_hash(&script));

    Ok(ClientBundle {
        path: route.path.clone(),
        entry: route.file.clone(),
        file_name,
        script,
    })
}

fn content_hash(input: &str) -> String {
    blake3::hash(input.as_bytes()).to_hex()[..16].to_string()
}

fn find_runtime_script(root: &Path, file_name: &str) -> Option<PathBuf> {
    let cwd_renderer = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("packages/ruvyxa/runtime").join(file_name));
    if let Some(path) = cwd_renderer.filter(|path| path.is_file()) {
        return Some(path);
    }

    let package_renderer = root.join("node_modules/ruvyxa/runtime").join(file_name);
    if package_renderer.is_file() {
        return Some(package_renderer);
    }

    None
}

fn print_routes(args: ProjectArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let manifest = discover_routes(DiscoverOptions::new(args.root.join(config.app_dir())))?;

    println!("\n{}", heading("Ruvyxa routes"));
    print_route_row("kind", label("kind"), "path", label("path"), label("id"));
    for route in manifest.routes {
        let kind = format!("{:?}", route.kind);
        print_route_row(
            &kind,
            accent(&kind),
            &route.path,
            route.path.clone(),
            dim(route.id),
        );
    }
    println!();

    Ok(())
}

fn analyze(args: ProjectArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let manifest = discover_routes(DiscoverOptions::new(args.root.join(config.app_dir())))?;
    let validation = validate_app(&args.root, &manifest)?;

    println!("{}", serde_json::to_string_pretty(&validation)?);

    if !validation.is_ok() {
        anyhow::bail!(
            "analysis found {} diagnostic(s); fix them before building",
            validation.diagnostics.len()
        );
    }

    Ok(())
}

fn doctor(args: ProjectArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let app_dir = args.root.join(config.app_dir());
    let package_json = args.root.join("package.json");
    let tsconfig = args.root.join("tsconfig.json");

    println!("\n{}", heading("Ruvyxa doctor"));
    print_field("root", path_text(&args.root));
    print_field("config", exists_status(&args.root.join("ruvyxa.config.ts")));
    print_field("app dir", path_text(&app_dir));
    print_field("out dir", path_text(&args.root.join(config.out_dir())));
    print_field("app directory", exists_status(&app_dir));
    print_field("package.json", exists_status(&package_json));
    print_field("tsconfig.json", exists_status(&tsconfig));
    print_field(
        "package manager",
        accent(detect_package_manager(&args.root)),
    );
    print_field("node", tool_status(tool_version("node", &["--version"])));
    print_field("bun", tool_status(tool_version("bun", &["--version"])));
    print_field("deno", tool_status(tool_version("deno", &["--version"])));

    if package_json.exists() {
        let package = read_package_json(&package_json)?;
        print_field(
            "react",
            tool_status(
                dependency_version(&package, "react").unwrap_or_else(|| "missing".to_string()),
            ),
        );
        print_field(
            "react-dom",
            tool_status(
                dependency_version(&package, "react-dom").unwrap_or_else(|| "missing".to_string()),
            ),
        );
        print_field(
            "react compatibility",
            compatibility_status(react_compatibility(&package)),
        );

        let duplicates = duplicate_dependencies(&package);
        if duplicates.is_empty() {
            print_field("dependency duplicates", ok_text("ok"));
        } else {
            print_field("dependency duplicates", warn_text(duplicates.join(", ")));
        }
    }

    let manifest = discover_routes(DiscoverOptions::new(&app_dir))?;
    let validation = validate_app(&args.root, &manifest)?;
    print_field("routes", accent(manifest.routes.len().to_string()));
    print_field("page routes", accent(validation.page_routes.to_string()));
    print_field("api routes", accent(validation.api_routes.to_string()));
    print_field(
        "client modules",
        accent(validation.client_modules.to_string()),
    );
    print_field(
        "server modules",
        accent(validation.server_modules.to_string()),
    );
    print_field(
        "diagnostics",
        if validation.diagnostics.is_empty() {
            ok_text("0")
        } else {
            warn_text(validation.diagnostics.len().to_string())
        },
    );
    print_field("env schema", exists_status(&args.root.join(".env.example")));
    print_field("native binary", ok_text("ok"));
    println!();
    Ok(())
}

fn clean(args: ProjectArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let out_dir = args.root.join(config.out_dir());
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)?;
    }
    println!("\n{} Removed {}\n", success(), path_text(&out_dir));
    Ok(())
}

fn trace(args: TraceArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let manifest = discover_routes(DiscoverOptions::new(args.root.join(config.app_dir())))?;
    let route = manifest
        .routes
        .iter()
        .find(|entry| entry.path == args.route)
        .with_context(|| format!("route {} was not found", args.route))?;

    println!("{}", serde_json::to_string_pretty(route)?);
    Ok(())
}

fn bench(args: BenchArgs) -> anyhow::Result<()> {
    let samples = args.samples.max(1);
    let root = args.root;
    let config = load_project_config(&root)?;
    let app_dir = root.join(config.app_dir());
    let mut results = Vec::new();

    results.push(run_benchmark("route-discovery", samples, || {
        let _manifest = discover_routes(DiscoverOptions::new(&app_dir))?;
        Ok(())
    })?);
    results.push(run_benchmark("analyze-validation", samples, || {
        let manifest = discover_routes(DiscoverOptions::new(&app_dir))?;
        let validation = validate_app(&root, &manifest)?;
        fail_on_diagnostics(&validation.diagnostics)?;
        Ok(())
    })?);
    results.push(run_benchmark("production-build", samples, || {
        build_with_output(
            BuildArgs {
                root: root.clone(),
                target: BuildTarget::Node,
            },
            false,
        )
    })?);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print_benchmark_table(samples, &results);
        println!();
    }

    Ok(())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkResult {
    name: String,
    samples: usize,
    min_ms: f64,
    median_ms: f64,
    avg_ms: f64,
    max_ms: f64,
}

fn run_benchmark(
    name: &str,
    samples: usize,
    mut run: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<BenchmarkResult> {
    let mut timings = Vec::with_capacity(samples);

    for _ in 0..samples {
        let started = Instant::now();
        run()?;
        timings.push(started.elapsed());
    }

    Ok(summarize_benchmark(name, timings))
}

fn summarize_benchmark(name: &str, mut timings: Vec<Duration>) -> BenchmarkResult {
    timings.sort();
    let samples = timings.len();
    let min_ms = duration_ms(timings[0]);
    let max_ms = duration_ms(timings[samples - 1]);
    let median_ms = duration_ms(timings[samples / 2]);
    let avg_ms = timings
        .iter()
        .map(|duration| duration_ms(*duration))
        .sum::<f64>()
        / samples as f64;

    BenchmarkResult {
        name: name.to_string(),
        samples,
        min_ms,
        median_ms,
        avg_ms,
        max_ms,
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn test_parity(args: ProjectArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    build(BuildArgs {
        root: args.root.clone(),
        target: BuildTarget::Node,
    })?;

    let dev_manifest = discover_routes(DiscoverOptions::new(args.root.join(config.app_dir())))?;
    let prod_manifest = discover_routes(DiscoverOptions::new(
        args.root
            .join(config.out_dir())
            .join("server")
            .join(config.app_dir()),
    ))?;
    let dev_routes = parity_routes(&dev_manifest);
    let prod_routes = parity_routes(&prod_manifest);
    let mut failures = Vec::new();

    for (key, dev_route) in &dev_routes {
        match prod_routes.get(key) {
            Some(prod_route) if prod_route == dev_route => {
                println!("{} {} dev/prod match", success(), key);
            }
            Some(prod_route) => {
                failures.push(format!(
                    "{key} mismatch\n  dev:  {:?}\n  prod: {:?}",
                    dev_route, prod_route
                ));
            }
            None => failures.push(format!("{key} exists in dev but not production")),
        }
    }

    for key in prod_routes.keys() {
        if !dev_routes.contains_key(key) {
            failures.push(format!("{key} exists in production but not dev"));
        }
    }

    if failures.is_empty() {
        println!(
            "\n{} Parity passed for {} routes\n",
            success(),
            accent(dev_routes.len().to_string())
        );
        return Ok(());
    }

    for failure in failures {
        eprintln!("{} {failure}", error_label());
    }

    anyhow::bail!("dev/prod parity failed")
}

#[derive(Debug, PartialEq, Eq)]
struct ParityRoute {
    file: String,
    layout_chain: Vec<String>,
    server_modules: Vec<String>,
    client_modules: Vec<String>,
    runtime: String,
}

fn parity_routes(manifest: &RouteManifest) -> BTreeMap<String, ParityRoute> {
    manifest
        .routes
        .iter()
        .map(|route| {
            (
                format!("{:?} {}", route.kind, route.path),
                parity_route(manifest, route),
            )
        })
        .collect()
}

fn parity_route(manifest: &RouteManifest, route: &RouteEntry) -> ParityRoute {
    ParityRoute {
        file: normalize_route_path(&manifest.app_dir, &route.file),
        layout_chain: route.layout_chain.clone(),
        server_modules: normalize_module_paths(manifest, &route.server_modules),
        client_modules: normalize_module_paths(manifest, &route.client_modules),
        runtime: format!("{:?}", route.runtime),
    }
}

fn normalize_module_paths(manifest: &RouteManifest, paths: &[String]) -> Vec<String> {
    let mut paths = paths
        .iter()
        .map(|path| normalize_route_path(&manifest.app_dir, Path::new(path)))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn normalize_route_path(app_dir: &Path, path: &Path) -> String {
    path.strip_prefix(app_dir)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().replace('\\', "/")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn copy_public_assets(root: &Path, assets_dir: &Path) -> anyhow::Result<()> {
    let public = root.join("public");
    if public.exists() {
        copy_dir_all(&public, assets_dir)?;
    }
    Ok(())
}

fn copy_optional_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    if from.exists() {
        copy_dir_all(from, to)?;
    }
    Ok(())
}

fn copy_dir_all(from: &Path, to: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(to)?;

    for entry in WalkDir::new(from)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let relative = entry.path().strip_prefix(from)?;
        let target = to.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }

    Ok(())
}

fn fail_on_diagnostics(diagnostics: &[Diagnostic]) -> anyhow::Result<()> {
    if diagnostics.is_empty() {
        return Ok(());
    }

    for diagnostic in diagnostics {
        eprintln!("{diagnostic}");
    }

    anyhow::bail!(
        "build validation failed with {} diagnostic(s)",
        diagnostics.len()
    )
}

fn print_field(name: &str, value: String) {
    let padding = spaces(20, name.len());
    println!("  {}{} {}", label(name), padding, value);
}

fn print_route_row(kind: &str, styled_kind: String, path: &str, styled_path: String, id: String) {
    println!(
        "  {}{} {}{} {}",
        styled_kind,
        spaces(10, kind.len()),
        styled_path,
        spaces(24, path.len()),
        id
    );
}

fn print_benchmark_table(samples: usize, results: &[BenchmarkResult]) {
    println!(
        "\n{}",
        heading(format!("Ruvyxa benchmark ({samples} sample(s))"))
    );

    let rows = results
        .iter()
        .map(|result| {
            [
                result.name.clone(),
                format!("{:.2}ms", result.min_ms),
                format!("{:.2}ms", result.median_ms),
                format!("{:.2}ms", result.avg_ms),
                format!("{:.2}ms", result.max_ms),
            ]
        })
        .collect::<Vec<_>>();
    let headers = ["Scenario", "Min", "Median", "Avg", "Max"];
    let widths = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .map(|row| row[index].len())
                .max()
                .unwrap_or(0)
                .max(header.len())
        })
        .collect::<Vec<_>>();

    print_table_separator(&widths);
    print_box_row(
        headers,
        [
            label(headers[0]),
            label(headers[1]),
            label(headers[2]),
            label(headers[3]),
            label(headers[4]),
        ],
        &widths,
    );
    print_table_separator(&widths);

    for row in rows {
        print_box_row(
            [&row[0], &row[1], &row[2], &row[3], &row[4]],
            [
                accent(&row[0]),
                ok_text(&row[1]),
                ok_text(&row[2]),
                ok_text(&row[3]),
                ok_text(&row[4]),
            ],
            &widths,
        );
    }
    print_table_separator(&widths);
}

fn print_table_separator(widths: &[usize]) {
    print!("  {}", dim("+"));
    for width in widths {
        print!("{}", dim("-".repeat(*width + 2)));
        print!("{}", dim("+"));
    }
    println!();
}

fn print_box_row<const N: usize>(raw: [&str; N], styled: [String; N], widths: &[usize]) {
    print!("  {}", dim("|"));
    for index in 0..N {
        if index == 0 {
            print!(
                " {}{} {}",
                styled[index],
                spaces(widths[index], raw[index].len()),
                dim("|")
            );
        } else {
            print!(
                " {}{} {}",
                spaces(widths[index], raw[index].len()),
                styled[index],
                dim("|")
            );
        }
    }
    println!();
}

fn spaces(width: usize, len: usize) -> String {
    " ".repeat(width.saturating_sub(len))
}

fn heading(value: impl AsRef<str>) -> String {
    paint(value, "1;35")
}

fn label(value: impl AsRef<str>) -> String {
    paint(value, "90")
}

fn accent(value: impl AsRef<str>) -> String {
    paint(value, "36")
}

fn dim(value: impl AsRef<str>) -> String {
    paint(value, "90")
}

fn ok_text(value: impl AsRef<str>) -> String {
    paint(value, "32")
}

fn warn_text(value: impl AsRef<str>) -> String {
    paint(value, "33")
}

fn error_label() -> String {
    paint("[error]", "31")
}

fn success() -> String {
    ok_text("[ok]")
}

fn path_text(path: &Path) -> String {
    paint(path.display().to_string(), "34")
}

fn exists_status(path: &Path) -> String {
    if path.exists() {
        ok_text("ok")
    } else {
        warn_text("missing")
    }
}

fn tool_status(value: String) -> String {
    if value == "missing" {
        warn_text(value)
    } else {
        ok_text(value)
    }
}

fn compatibility_status(value: String) -> String {
    if value.starts_with("ok ") {
        ok_text(value)
    } else {
        warn_text(value)
    }
}

fn paint(value: impl AsRef<str>, code: &str) -> String {
    let value = value.as_ref();
    if !std::io::stdout().is_terminal() {
        return value.to_string();
    }

    if std::env::var_os("NO_COLOR").is_some() {
        return value.to_string();
    }

    if std::env::var("TERM")
        .map(|term| term.eq_ignore_ascii_case("dumb"))
        .unwrap_or(false)
    {
        return value.to_string();
    }

    format!("\x1b[{code}m{value}\x1b[0m")
}

fn detect_package_manager(root: &Path) -> String {
    if find_upwards(root, "pnpm-lock.yaml").is_some() {
        "pnpm".to_string()
    } else if find_upwards(root, "package-lock.json").is_some() {
        "npm".to_string()
    } else if find_upwards(root, "yarn.lock").is_some() {
        "yarn".to_string()
    } else if find_upwards(root, "bun.lockb").is_some() {
        "bun".to_string()
    } else {
        "unknown".to_string()
    }
}

fn find_upwards(root: &Path, file_name: &str) -> Option<PathBuf> {
    let mut current = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    loop {
        let candidate = current.join(file_name);
        if candidate.exists() {
            return Some(candidate);
        }

        if !current.pop() {
            return None;
        }
    }
}

fn tool_version(command: &str, args: &[&str]) -> String {
    match ProcessCommand::new(command).args(args).output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "missing".to_string(),
    }
}

fn read_package_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

fn dependency_version(package: &serde_json::Value, name: &str) -> Option<String> {
    ["dependencies", "devDependencies", "peerDependencies"]
        .into_iter()
        .find_map(|section| {
            package
                .get(section)
                .and_then(|deps| deps.get(name))
                .and_then(|version| version.as_str())
                .map(str::to_string)
        })
}

fn react_compatibility(package: &serde_json::Value) -> String {
    let Some(react) = dependency_version(package, "react") else {
        return "missing react".to_string();
    };
    let Some(react_dom) = dependency_version(package, "react-dom") else {
        return "missing react-dom".to_string();
    };

    match (major_version(&react), major_version(&react_dom)) {
        (Some(left), Some(right)) if left == right => format!("ok (major {left})"),
        (Some(left), Some(right)) => format!("mismatch react {left} vs react-dom {right}"),
        _ => "unknown version format".to_string(),
    }
}

fn major_version(version: &str) -> Option<u64> {
    let digits = version
        .trim_start_matches(|character: char| !character.is_ascii_digit())
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn duplicate_dependencies(package: &serde_json::Value) -> Vec<String> {
    let mut seen = BTreeMap::<String, String>::new();
    let mut duplicates = Vec::new();

    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        let Some(deps) = package.get(section).and_then(|value| value.as_object()) else {
            continue;
        };

        for (name, version) in deps {
            let version = version.as_str().unwrap_or("unknown").to_string();
            if let Some(previous) = seen.insert(name.clone(), version.clone()) {
                if previous != version {
                    duplicates.push(format!("{name} ({previous}, {version})"));
                }
            }
        }
    }

    duplicates.sort();
    duplicates
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_dependency_major_versions() {
        assert_eq!(major_version("^19.0.0"), Some(19));
        assert_eq!(major_version("~18.3.1"), Some(18));
        assert_eq!(major_version("workspace:*"), None);
    }

    #[test]
    fn detects_react_version_compatibility() {
        let package = json!({
            "dependencies": {
                "react": "^19.0.0",
                "react-dom": "^19.1.0"
            }
        });

        assert_eq!(react_compatibility(&package), "ok (major 19)");
    }

    #[test]
    fn detects_duplicate_dependency_versions() {
        let package = json!({
            "dependencies": {
                "react": "^19.0.0"
            },
            "devDependencies": {
                "react": "^18.0.0"
            }
        });

        assert_eq!(
            duplicate_dependencies(&package),
            vec!["react (^19.0.0, ^18.0.0)"]
        );
    }

    #[test]
    fn summarizes_benchmark_samples() {
        let result = summarize_benchmark(
            "sample",
            vec![
                Duration::from_millis(30),
                Duration::from_millis(10),
                Duration::from_millis(20),
            ],
        );

        assert_eq!(result.name, "sample");
        assert_eq!(result.samples, 3);
        assert_eq!(result.min_ms, 10.0);
        assert_eq!(result.median_ms, 20.0);
        assert_eq!(result.max_ms, 30.0);
    }

    #[test]
    fn content_hash_is_deterministic() {
        assert_eq!(
            content_hash("console.log('a')"),
            content_hash("console.log('a')")
        );
        assert_ne!(
            content_hash("console.log('a')"),
            content_hash("console.log('b')")
        );
        assert_eq!(content_hash("console.log('a')").len(), 16);
    }
}
