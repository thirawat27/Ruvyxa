use std::collections::BTreeMap;
use std::fs;
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

    #[arg(long, default_value = "localhost")]
    host: String,

    #[arg(long, default_value_t = 3000)]
    port: u16,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ruvyxa=info,tower_http=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Dev(args) => {
            serve(ServerConfig::dev(args.root, args.host, args.port))
                .await
                .context("dev server failed")?;
        }
        Command::Build(args) => build(args).context("build failed")?,
        Command::Start(args) | Command::Preview(args) => {
            serve(ServerConfig::production(args.root, args.host, args.port))
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

fn build(args: BuildArgs) -> anyhow::Result<()> {
    let app_dir = args.root.join("app");
    let out_dir = args.root.join(".ruvyxa");
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

    let client_manifest = emit_client_bundles(&args.root, &app_dir, &manifest, &client_dir)?;
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
            "actionBodyLimitBytes": 65536,
            "sameOriginActions": true,
            "fetchMetadataActions": true,
            "securityHeaders": true
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
    println!(
        "Built {} routes into {}",
        manifest.routes.len(),
        out_dir.display()
    );
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

fn emit_client_bundles(
    root: &Path,
    app_dir: &Path,
    manifest: &RouteManifest,
    client_dir: &Path,
) -> anyhow::Result<serde_json::Value> {
    let renderer = find_runtime_script(root, "client-renderer.mjs")
        .context("client renderer was not found for production build")?;
    let mut routes = Vec::new();

    for route in manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
    {
        let output = ProcessCommand::new("node")
            .env("RUVYXA_CLIENT_MINIFY", "1")
            .arg(&renderer)
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
        fs::write(client_dir.join(&file_name), script.as_bytes())?;

        routes.push(serde_json::json!({
            "path": route.path,
            "entry": route.file,
            "file": file_name,
            "src": format!("/__ruvyxa/client/{}", file_name),
            "bytes": script.len(),
            "optimized": true,
            "treeShaken": true,
            "chunkStrategy": "route"
        }));
    }

    Ok(serde_json::json!({
        "chunkStrategy": "route",
        "minify": true,
        "treeShaking": true,
        "routes": routes
    }))
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
    let manifest = discover_routes(DiscoverOptions::new(args.root.join("app")))?;

    for route in manifest.routes {
        println!(
            "{:<8} {:<24} {}",
            format!("{:?}", route.kind),
            route.path,
            route.id
        );
    }

    Ok(())
}

fn analyze(args: ProjectArgs) -> anyhow::Result<()> {
    let manifest = discover_routes(DiscoverOptions::new(args.root.join("app")))?;
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
    let app_dir = args.root.join("app");
    let package_json = args.root.join("package.json");
    let tsconfig = args.root.join("tsconfig.json");

    println!("Ruvyxa doctor");
    println!("root: {}", args.root.display());
    println!("app directory: {}", exists_label(&app_dir));
    println!("package.json: {}", exists_label(&package_json));
    println!("tsconfig.json: {}", exists_label(&tsconfig));
    println!("package manager: {}", detect_package_manager(&args.root));
    println!("node: {}", tool_version("node", &["--version"]));
    println!("bun: {}", tool_version("bun", &["--version"]));
    println!("deno: {}", tool_version("deno", &["--version"]));

    if package_json.exists() {
        let package = read_package_json(&package_json)?;
        println!(
            "react: {}",
            dependency_version(&package, "react").unwrap_or_else(|| "missing".to_string())
        );
        println!(
            "react-dom: {}",
            dependency_version(&package, "react-dom").unwrap_or_else(|| "missing".to_string())
        );
        println!("react compatibility: {}", react_compatibility(&package));

        let duplicates = duplicate_dependencies(&package);
        if duplicates.is_empty() {
            println!("dependency duplicates: ok");
        } else {
            println!("dependency duplicates: {}", duplicates.join(", "));
        }
    }

    let manifest = discover_routes(DiscoverOptions::new(&app_dir))?;
    let validation = validate_app(&args.root, &manifest)?;
    println!("routes: {}", manifest.routes.len());
    println!("page routes: {}", validation.page_routes);
    println!("api routes: {}", validation.api_routes);
    println!("client modules: {}", validation.client_modules);
    println!("server modules: {}", validation.server_modules);
    println!("diagnostics: {}", validation.diagnostics.len());
    println!(
        "env schema: {}",
        exists_label(&args.root.join(".env.example"))
    );
    println!("native binary: ok");
    Ok(())
}

fn clean(args: ProjectArgs) -> anyhow::Result<()> {
    let out_dir = args.root.join(".ruvyxa");
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)?;
    }
    println!("Removed {}", out_dir.display());
    Ok(())
}

fn trace(args: TraceArgs) -> anyhow::Result<()> {
    let manifest = discover_routes(DiscoverOptions::new(args.root.join("app")))?;
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
    let mut results = Vec::new();

    results.push(run_benchmark("route-discovery", samples, || {
        let _manifest = discover_routes(DiscoverOptions::new(root.join("app")))?;
        Ok(())
    })?);
    results.push(run_benchmark("analyze-validation", samples, || {
        let manifest = discover_routes(DiscoverOptions::new(root.join("app")))?;
        let validation = validate_app(&root, &manifest)?;
        fail_on_diagnostics(&validation.diagnostics)?;
        Ok(())
    })?);
    results.push(run_benchmark("production-build", samples, || {
        build(BuildArgs {
            root: root.clone(),
            target: BuildTarget::Node,
        })
    })?);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        println!("Ruvyxa benchmark ({samples} sample(s))");
        println!(
            "{:<20} {:>10} {:>10} {:>10} {:>10}",
            "scenario", "min", "median", "avg", "max"
        );
        for result in &results {
            println!(
                "{:<20} {:>9.2}ms {:>9.2}ms {:>9.2}ms {:>9.2}ms",
                result.name, result.min_ms, result.median_ms, result.avg_ms, result.max_ms
            );
        }
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
    build(BuildArgs {
        root: args.root.clone(),
        target: BuildTarget::Node,
    })?;

    let dev_manifest = discover_routes(DiscoverOptions::new(args.root.join("app")))?;
    let prod_manifest =
        discover_routes(DiscoverOptions::new(args.root.join(".ruvyxa/server/app")))?;
    let dev_routes = parity_routes(&dev_manifest);
    let prod_routes = parity_routes(&prod_manifest);
    let mut failures = Vec::new();

    for (key, dev_route) in &dev_routes {
        match prod_routes.get(key) {
            Some(prod_route) if prod_route == dev_route => {
                println!("PASS {} dev/prod match", key);
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
        println!("Parity passed for {} routes", dev_routes.len());
        return Ok(());
    }

    for failure in failures {
        eprintln!("FAIL {failure}");
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

fn exists_label(path: &Path) -> &'static str {
    if path.exists() {
        "ok"
    } else {
        "missing"
    }
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
