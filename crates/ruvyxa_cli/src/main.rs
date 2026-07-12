use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand, Stdio};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::Local;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Parser, Subcommand, ValueEnum};
use ruvyxa_dev_server::{ServerConfig, render_request, serve};
use ruvyxa_diagnostics::Diagnostic;
use ruvyxa_graph::{
    DiscoverOptions, RenderStrategy, RouteEntry, RouteManifest, discover_routes, validate_app,
    write_manifest,
};
use tracing::info;
use walkdir::WalkDir;

mod image_optimizer;
use image_optimizer::{ImageOptimizationOptions, optimize_public_images};

const ASSET_HASH_ALGORITHM: &str = "blake3-256";

#[derive(Debug, Parser)]
#[command(name = "Ruvyxa")]
#[command(bin_name = "Ruvyxa")]
#[command(override_usage = "Ruvyxa <COMMAND>")]
#[command(color = clap::ColorChoice::Auto)]
#[command(styles = cli_styles())]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

fn cli_styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
        .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
        .literal(AnsiColor::BrightBlue.on_default().effects(Effects::BOLD))
        .placeholder(AnsiColor::Yellow.on_default())
        .valid(AnsiColor::BrightGreen.on_default())
        .invalid(AnsiColor::BrightRed.on_default().effects(Effects::BOLD))
        .error(AnsiColor::BrightRed.on_default().effects(Effects::BOLD))
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Run the development server with hot reload and route watching")]
    Dev(ServerArgs),
    #[command(about = "Build the application for production output")]
    Build(BuildArgs),
    #[command(about = "Run app-level production readiness checks")]
    Check(ProjectArgs),
    #[command(about = "Serve an existing production build")]
    Start(ServerArgs),
    #[command(about = "Preview an existing production build locally")]
    Preview(ServerArgs),
    #[command(about = "Print the discovered route table")]
    Routes(ProjectArgs),
    #[command(about = "Validate routes, imports, and server/client boundaries")]
    Analyze(ProjectArgs),
    #[command(about = "Check project setup, dependencies, and runtime compatibility")]
    Doctor(ProjectArgs),
    #[command(about = "Remove generated Ruvyxa build output")]
    Clean(ProjectArgs),
    #[command(about = "Inspect one route manifest entry by path")]
    Trace(TraceArgs),
    #[command(about = "Benchmark route discovery, analysis, and production build")]
    Bench(BenchArgs),
    #[command(
        name = "test:parity",
        alias = "parity",
        about = "Compare dev/prod routes and smoke-render page routes"
    )]
    TestParity(ProjectArgs),
}

#[derive(Debug, Clone, Parser)]
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

    #[arg(long, value_enum, ignore_case = true)]
    target: Option<BuildTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectConfig {
    app_dir: Option<String>,
    out_dir: Option<String>,
    runtime: Option<BuildTarget>,
    #[serde(rename = "react")]
    _react: Option<serde_json::Value>,
    #[serde(rename = "typescript")]
    _typescript: Option<serde_json::Value>,
    #[serde(default, rename = "render")]
    rendering: RenderingConfigOptions,
    #[serde(default)]
    server: ServerConfigOptions,
    #[serde(default)]
    css: CssConfigOptions,
    #[serde(default)]
    build: BuildConfigOptions,
    #[serde(default)]
    debug: DebugConfigOptions,
    #[serde(default, rename = "image")]
    images: ImageOptimizationOptions,
    #[serde(default)]
    security: SecurityConfigOptions,
    #[serde(default)]
    cache: CacheConfigOptions,
    #[serde(default)]
    middleware: ruvyxa_middleware::MiddlewareConfig,
    #[serde(default)]
    plugins: Vec<BuildPluginConfig>,
    #[serde(rename = "adapter")]
    adapter: Option<serde_json::Value>,
    #[serde(rename = "adapterOptions")]
    adapter_options: Option<serde_json::Value>,
    #[serde(skip)]
    config_dependency_hash: String,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ServerConfigOptions {
    host: Option<String>,
    port: Option<u16>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CssConfigOptions {
    #[serde(default)]
    entries: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuildConfigOptions {
    minify: Option<bool>,
    #[serde(rename = "map")]
    sourcemap: Option<bool>,
    #[serde(rename = "treeShake")]
    tree_shaking: Option<bool>,
    #[serde(rename = "split")]
    split_strategy: Option<String>,
    #[serde(rename = "workers")]
    parallelism: Option<usize>,
    #[serde(rename = "jsx")]
    jsx_runtime: Option<String>,
    #[serde(rename = "target")]
    es_target: Option<String>,
    #[serde(rename = "manifest")]
    emit_chunk_manifest: Option<bool>,
    #[serde(rename = "warm")]
    prebundle_dependencies: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RenderingConfigOptions {
    #[serde(rename = "strategy")]
    default_strategy: Option<RenderStrategy>,
    #[serde(rename = "revalidate")]
    default_revalidate: Option<u64>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DebugConfigOptions {
    overlay: Option<bool>,
    traces: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecurityConfigOptions {
    #[serde(rename = "actionLimit")]
    action_body_limit_bytes: Option<usize>,
    #[serde(rename = "apiLimit")]
    api_body_limit_bytes: Option<usize>,
    #[serde(rename = "actionRateLimit")]
    action_rate_limit: Option<ActionRateLimitOptions>,
    #[serde(rename = "sameOrigin")]
    same_origin_actions: Option<bool>,
    #[serde(rename = "fetchMeta")]
    fetch_metadata_actions: Option<bool>,
    #[serde(rename = "headers")]
    security_headers: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActionRateLimitOptions {
    max: Option<usize>,
    window: Option<u64>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CacheConfigOptions {
    #[serde(rename = "routes")]
    route_manifest: Option<bool>,
    css: Option<bool>,
    #[serde(rename = "dir")]
    build_dir: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildPluginConfig {
    name: String,
    enforce: Option<String>,
    resolve_id: bool,
    transform: bool,
}

struct NativeBuildCache<'a> {
    dependency_hash: &'a str,
    directory: &'a Path,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigRendererOutput {
    ok: bool,
    config: Option<ProjectConfig>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
    dependency_hash: Option<String>,
}

impl ProjectConfig {
    fn build_target(&self, cli_target: Option<BuildTarget>) -> BuildTarget {
        cli_target.or(self.runtime).unwrap_or(BuildTarget::Node)
    }

    fn app_dir(&self) -> &str {
        self.app_dir.as_deref().unwrap_or("app")
    }

    fn out_dir(&self) -> &str {
        self.out_dir.as_deref().unwrap_or(".ruvyxa")
    }

    fn validate_paths(&self) -> anyhow::Result<()> {
        validate_project_relative_path("appDir", self.app_dir())?;
        validate_project_relative_path("outDir", self.out_dir())?;
        for entry in &self.css.entries {
            validate_project_relative_path("css.entries", entry)?;
        }
        validate_positive_limit(
            "security.actionLimit",
            self.security.action_body_limit_bytes,
        )?;
        validate_positive_limit("security.apiLimit", self.security.api_body_limit_bytes)?;
        if let Some(rate_limit) = &self.security.action_rate_limit {
            validate_positive_limit("security.actionRateLimit.max", rate_limit.max)?;
            validate_positive_limit("security.actionRateLimit.window", rate_limit.window)?;
        }
        Ok(())
    }

    fn style_entries(&self, root: &Path) -> Vec<PathBuf> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        self.css
            .entries
            .iter()
            .map(|entry| root.join(entry))
            .collect()
    }

    fn discover_options(&self, root: &Path) -> DiscoverOptions {
        DiscoverOptions::new(root.join(self.app_dir())).with_rendering_defaults(
            self.rendering.default_strategy,
            self.rendering.default_revalidate,
        )
    }
}

fn validate_positive_limit<T>(field: &str, value: Option<T>) -> anyhow::Result<()>
where
    T: PartialEq + From<u8>,
{
    if value.is_some_and(|value| value == T::from(0)) {
        anyhow::bail!("RUV1601 config field `{field}` must be greater than zero");
    }
    Ok(())
}

fn discover_project_routes(root: &Path, config: &ProjectConfig) -> anyhow::Result<RouteManifest> {
    discover_routes(config.discover_options(root)).map_err(Into::into)
}

fn validate_project_relative_path(field: &str, value: &str) -> anyhow::Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("RUV1601 config field `{field}` must not be empty");
    }

    let path = Path::new(trimmed);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
                    | std::path::Component::ParentDir
            )
        })
    {
        anyhow::bail!(
            "RUV1601 config field `{field}` must be a project-relative path inside the project root"
        );
    }

    Ok(())
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

    let cli = Cli::parse_from(normalized_cli_args(std::env::args_os()));

    match cli.command {
        Command::Dev(args) => {
            let config = load_project_config(&args.root)?;
            serve(dev_server_config(&args, &config))
                .await
                .context("dev server failed")?;
        }
        Command::Build(args) => build(args).context("build failed")?,
        Command::Check(args) => check(args).context("check failed")?,
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

fn normalized_cli_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args = args.into_iter().collect::<Vec<_>>();
    normalize_option_args(&mut args);

    if let Some(command_index) = first_command_arg_index(&args) {
        normalize_command_arg(&mut args, command_index);

        if args[command_index] == "help"
            && let Some(help_target_index) = first_command_arg_index(&args[command_index..])
        {
            normalize_command_arg(&mut args, command_index + help_target_index);
        }
    }

    args
}

fn normalize_option_args(args: &mut [OsString]) {
    for arg in args.iter_mut().skip(1) {
        let Some(normalized) = normalized_option_arg(arg) else {
            continue;
        };

        *arg = OsString::from(normalized);
    }
}

fn normalized_option_arg(arg: &OsString) -> Option<String> {
    let arg = arg.to_str()?;

    if arg.eq_ignore_ascii_case("-h") {
        return Some("-h".to_string());
    }

    let option = arg.strip_prefix("--")?;
    let (name, value) = option
        .split_once('=')
        .map_or((option, None), |(name, value)| (name, Some(value)));
    let canonical = canonical_option_name(name)?;

    Some(match value {
        Some(value) => format!("--{canonical}={value}"),
        None => format!("--{canonical}"),
    })
}

fn canonical_option_name(option: &str) -> Option<&'static str> {
    match option.to_ascii_lowercase().as_str() {
        "help" => Some("help"),
        "root" => Some("root"),
        "host" => Some("host"),
        "port" => Some("port"),
        "target" => Some("target"),
        "samples" => Some("samples"),
        "json" => Some("json"),
        _ => None,
    }
}

fn first_command_arg_index(args: &[OsString]) -> Option<usize> {
    for (index, arg) in args.iter().enumerate().skip(1) {
        let arg = arg.to_string_lossy();

        if arg == "--" {
            return None;
        }

        if arg.starts_with('-') {
            continue;
        }

        return Some(index);
    }

    None
}

fn normalize_command_arg(args: &mut [OsString], index: usize) {
    let Some(command) = args[index].to_str() else {
        return;
    };
    let Some(canonical) = canonical_command_name(command) else {
        return;
    };

    args[index] = OsString::from(canonical);
}

fn canonical_command_name(command: &str) -> Option<&'static str> {
    match command.to_ascii_lowercase().as_str() {
        "dev" => Some("dev"),
        "build" => Some("build"),
        "check" => Some("check"),
        "start" => Some("start"),
        "preview" => Some("preview"),
        "routes" => Some("routes"),
        "analyze" => Some("analyze"),
        "doctor" => Some("doctor"),
        "clean" => Some("clean"),
        "trace" => Some("trace"),
        "bench" => Some("bench"),
        "test:parity" => Some("test:parity"),
        "parity" => Some("parity"),
        "help" => Some("help"),
        _ => None,
    }
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
    server.prerender_dir = out_dir.join("prerender");
    server.cache_route_manifest = config.cache.route_manifest.unwrap_or(true);
    server.cache_css = config.cache.css.unwrap_or(true);
    server.style_entries = config.style_entries(&args.root);
    server.prebundle_dependencies = config.build.prebundle_dependencies.unwrap_or(true);
    server.error_overlay = config.debug.overlay.unwrap_or(true);
    server.debug_traces = config.debug.traces.unwrap_or(false);
    server.action_body_limit_bytes = config
        .security
        .action_body_limit_bytes
        .unwrap_or(server.action_body_limit_bytes);
    server.api_body_limit_bytes = config
        .security
        .api_body_limit_bytes
        .unwrap_or(server.api_body_limit_bytes);
    if let Some(rate_limit) = &config.security.action_rate_limit {
        server.action_rate_limit_max = rate_limit.max.unwrap_or(server.action_rate_limit_max);
        server.action_rate_limit_window = Duration::from_secs(
            rate_limit
                .window
                .unwrap_or(server.action_rate_limit_window.as_secs()),
        );
    }
    server.same_origin_actions = config
        .security
        .same_origin_actions
        .unwrap_or(server.same_origin_actions);
    server.fetch_metadata_actions = config
        .security
        .fetch_metadata_actions
        .unwrap_or(server.fetch_metadata_actions);
    server.security_headers = config
        .security
        .security_headers
        .unwrap_or(server.security_headers);
    server.middleware = config.middleware.clone();
    server.default_render_strategy = config.rendering.default_strategy;
    server.default_revalidate = config.rendering.default_revalidate;
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
    server.prerender_dir = out_dir.join("prerender");
    server.cache_route_manifest = config.cache.route_manifest.unwrap_or(true);
    server.cache_css = config.cache.css.unwrap_or(true);
    server.style_entries = config.style_entries(&out_dir.join("server"));
    server.action_body_limit_bytes = config
        .security
        .action_body_limit_bytes
        .unwrap_or(server.action_body_limit_bytes);
    server.api_body_limit_bytes = config
        .security
        .api_body_limit_bytes
        .unwrap_or(server.api_body_limit_bytes);
    if let Some(rate_limit) = &config.security.action_rate_limit {
        server.action_rate_limit_max = rate_limit.max.unwrap_or(server.action_rate_limit_max);
        server.action_rate_limit_window = Duration::from_secs(
            rate_limit
                .window
                .unwrap_or(server.action_rate_limit_window.as_secs()),
        );
    }
    server.same_origin_actions = config
        .security
        .same_origin_actions
        .unwrap_or(server.same_origin_actions);
    server.fetch_metadata_actions = config
        .security
        .fetch_metadata_actions
        .unwrap_or(server.fetch_metadata_actions);
    server.security_headers = config
        .security
        .security_headers
        .unwrap_or(server.security_headers);
    server.middleware = config.middleware.clone();
    server.default_render_strategy = config.rendering.default_strategy;
    server.default_revalidate = config.rendering.default_revalidate;
    server
}

fn load_project_config(root: &Path) -> anyhow::Result<ProjectConfig> {
    let Some(renderer) = find_runtime_script(root, "config-renderer.mjs") else {
        let config = ProjectConfig {
            config_dependency_hash: "no-config".to_string(),
            ..ProjectConfig::default()
        };
        config.validate_paths()?;
        return Ok(config);
    };

    let output = ProcessCommand::new("node")
        .arg(&renderer)
        .arg(root)
        .output()
        .with_context(|| format!("failed to load config for {}", root.display()))?;
    let result = parse_config_renderer_output(
        root,
        &output.stdout,
        &output.stderr,
        &output.status.to_string(),
    )?;

    if output.status.success() && result.ok {
        let mut config = result.config.unwrap_or_default();
        config.config_dependency_hash = result
            .dependency_hash
            .unwrap_or_else(|| "legacy-config-renderer".to_string());
        config.validate_paths()?;
        return Ok(config);
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

fn parse_config_renderer_output(
    root: &Path,
    stdout: &[u8],
    stderr: &[u8],
    status: &str,
) -> anyhow::Result<ConfigRendererOutput> {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    serde_json::from_str(&stdout).with_context(|| {
        format!(
            "config renderer returned invalid output for {}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            root.display(),
            status,
            diagnostic_stream(&stdout),
            diagnostic_stream(&stderr),
        )
    })
}

fn build_cache_dir(root: &Path, cache: &CacheConfigOptions) -> PathBuf {
    resolve_build_cache_dir(
        root,
        cache.build_dir.as_deref(),
        std::env::var_os("RUVYXA_BUILD_CACHE_DIR"),
    )
}

fn resolve_build_cache_dir(
    root: &Path,
    configured: Option<&str>,
    environment: Option<OsString>,
) -> PathBuf {
    let selected = environment
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            configured
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        });

    match selected {
        Some(path) if path.is_absolute() => path,
        Some(path) => root.join(path),
        None => root.join(".ruvyxa").join("cache").join("bundler"),
    }
}

fn diagnostic_stream(value: &str) -> String {
    if value.trim().is_empty() {
        "(empty)".to_string()
    } else {
        value.to_string()
    }
}

fn build(args: BuildArgs) -> anyhow::Result<()> {
    build_with_output(args, true)
}

fn build_with_output(args: BuildArgs, show_summary: bool) -> anyhow::Result<()> {
    let started = Instant::now();
    let config = load_project_config(&args.root)?;
    let target = config.build_target(args.target);
    let app_dir = args.root.join(config.app_dir());
    let out_dir = args.root.join(config.out_dir());

    let manifest = discover_project_routes(&args.root, &config)?;
    let validation = validate_app(&args.root, &manifest)?;
    fail_on_diagnostics(&validation.diagnostics)?;
    let style_collection =
        ruvyxa_dev_server::collect_styles(&args.root, &app_dir, &config.style_entries(&args.root))?;

    let staging_dir = create_build_staging_dir(&out_dir).with_context(|| {
        format!(
            "failed to create build staging dir in {}",
            out_dir.display()
        )
    })?;
    let server_dir = staging_dir.join("server");
    let client_dir = staging_dir.join("client");
    let assets_dir = staging_dir.join("assets");

    copy_dir_all(&app_dir, &server_dir.join("app"))?;
    copy_optional_dir(
        &args.root.join("components"),
        &server_dir.join("components"),
    )?;
    copy_optional_dir(&args.root.join("server"), &server_dir.join("server"))?;
    copy_style_sources(&args.root, &server_dir, &style_collection.files)?;
    let image_cache_dir = build_cache_dir(&args.root, &config.cache).join("images");
    let image_report = optimize_public_images(
        &args.root.join("public"),
        &assets_dir,
        &image_cache_dir,
        &config.images,
    )?;
    let asset_files = count_files(&assets_dir);
    fs::create_dir_all(&client_dir)?;
    write_manifest(&manifest, &staging_dir.join("manifest.json"))?;

    let client_manifest = emit_client_bundles(
        &args.root,
        &app_dir,
        &manifest,
        &client_dir,
        &config.build,
        &config.plugins,
        NativeBuildCache {
            dependency_hash: &config.config_dependency_hash,
            directory: &build_cache_dir(&args.root, &config.cache),
        },
    )?;
    fs::write(
        client_dir.join("manifest.json"),
        serde_json::to_string_pretty(&client_manifest)?,
    )?;

    // ─── SSG / ISR / PPR pre-rendering at build time ──────────────────────────
    let prerender_dir = staging_dir.join("prerender");
    let prerendered = prerender_static_routes(
        &args.root,
        &app_dir,
        &manifest,
        &prerender_dir,
        &client_dir,
        &style_collection.css,
        config.build.parallelism,
    )?;

    let build_info = serde_json::json!({
        "framework": "Ruvyxa",
        "version": env!("CARGO_PKG_VERSION"),
        "target": format!("{:?}", target).to_lowercase(),
        "profile": "production",
        "createdAtUnix": SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default(),
        "routes": manifest.routes.len(),
        "serverDir": "server",
        "clientDir": "client",
        "assetsDir": "assets",
        "adapter": config.adapter.clone(),
        "adapterOptions": config.adapter_options.clone(),
        "images": image_report,
        "hashAlgorithm": ASSET_HASH_ALGORITHM,
        "security": {
            "actionLimit": config.security.action_body_limit_bytes.unwrap_or(1024 * 1024),
            "apiLimit": config.security.api_body_limit_bytes.unwrap_or(10 * 1024 * 1024),
            "actionRateLimit": {
                "max": config.security.action_rate_limit.as_ref().and_then(|value| value.max).unwrap_or(600),
                "window": config.security.action_rate_limit.as_ref().and_then(|value| value.window).unwrap_or(60)
            },
            "sameOrigin": config.security.same_origin_actions.unwrap_or(true),
            "fetchMeta": config.security.fetch_metadata_actions.unwrap_or(true),
            "headers": config.security.security_headers.unwrap_or(true)
        },
        "build": {
            "minify": config.build.minify.unwrap_or(true),
            "map": config.build.sourcemap.unwrap_or(false),
            "treeShake": config.build.tree_shaking.unwrap_or(true),
            "split": config.build.split_strategy.as_deref().unwrap_or("route"),
            "jsx": config.build.jsx_runtime.as_deref().unwrap_or("classic"),
            "target": config.build.es_target.as_deref().unwrap_or("es2022"),
            "manifest": config.build.emit_chunk_manifest.unwrap_or(false),
            "warm": config.build.prebundle_dependencies.unwrap_or(true),
            "workers": client_manifest.get("parallelism").cloned().unwrap_or(serde_json::Value::Null)
        },
        "render": {
            "prerendered": prerendered.len(),
            "routes": prerendered.iter().map(|p| serde_json::json!({
                "path": p.path,
                "strategy": format!("{:?}", p.strategy).to_lowercase(),
                "revalidate": p.revalidate,
            })).collect::<Vec<_>>()
        }
    });
    fs::write(
        staging_dir.join("build.json"),
        serde_json::to_string_pretty(&build_info)?,
    )?;

    commit_staged_build_outputs(&staging_dir, &out_dir)
        .with_context(|| format!("failed to commit build output into {}", out_dir.display()))?;

    info!(
        target = ?target,
        routes = manifest.routes.len(),
        output = %out_dir.display(),
        "build complete"
    );
    if show_summary {
        let page_routes = manifest
            .routes
            .iter()
            .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
            .count();
        let api_routes = manifest.routes.len().saturating_sub(page_routes);
        let client_bundles = client_manifest
            .get("routes")
            .and_then(|routes| routes.as_array())
            .map(Vec::len)
            .unwrap_or_default();
        print_tui_header("Build");
        print_field("status", ok_text("built"));
        print_field("target", accent(format!("{:?}", target).to_lowercase()));
        print_field("profile", accent("production"));
        print_field("root", path_text(&args.root));
        print_field("app dir", path_text(&app_dir));
        print_field("out dir", path_text(&out_dir));
        print_field("routes", accent(manifest.routes.len().to_string()));
        print_field("pages", accent(page_routes.to_string()));
        print_field("api", accent(api_routes.to_string()));
        print_field("client bundles", accent(client_bundles.to_string()));
        print_field("asset files", accent(asset_files.to_string()));
        if image_report.optimized_images > 0 {
            print_field(
                "optimized images",
                accent(image_report.optimized_images.to_string()),
            );
            print_field(
                "image cache hits",
                accent(image_report.cache_hits.to_string()),
            );
        }
        if !prerendered.is_empty() {
            print_field("prerendered", accent(prerendered.len().to_string()));
        }
        print_field("duration", accent(format_duration(started.elapsed())));
        println!("  {} Built into {}\n", success(), path_text(&out_dir));
    }
    Ok(())
}

#[allow(dead_code)]
fn print_build_report(
    manifest: &RouteManifest,
    client_manifest: &serde_json::Value,
    prerendered: &[PrerenderedRoute],
    image_report: &image_optimizer::ImageOptimizationReport,
    asset_files: usize,
    target: BuildTarget,
    out_dir: &Path,
    duration: Duration,
) {
    let page_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .collect::<Vec<_>>();
    let client_routes = client_manifest
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let shared_chunks = client_manifest
        .get("sharedRouteChunks")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    println!("\n   {} Ruvyxa {}", accent("▲"), env!("CARGO_PKG_VERSION"));
    println!("\n   Creating an optimized production build ...");
    println!(
        " {} Compiled and validated {} routes",
        success(),
        manifest.routes.len()
    );
    println!(
        " {} Generated {} pre-rendered page{}",
        success(),
        prerendered.len(),
        if prerendered.len() == 1 { "" } else { "s" }
    );
    println!(
        " {} Emitted {} client bundle{} and {} asset file{}",
        success(),
        client_routes.len(),
        if client_routes.len() == 1 { "" } else { "s" },
        asset_files,
        if asset_files == 1 { "" } else { "s" }
    );
    if image_report.optimized_images > 0 {
        println!(
            " {} Optimized {} image{} ({} cache hit{})",
            success(),
            image_report.optimized_images,
            if image_report.optimized_images == 1 {
                ""
            } else {
                "s"
            },
            image_report.cache_hits,
            if image_report.cache_hits == 1 {
                ""
            } else {
                "s"
            }
        );
    }

    println!();
    println!(
        "Route (app){}Size{}First Load JS",
        spaces(39, "Route (app)".len()),
        spaces(16, "Size".len())
    );
    for (index, route) in page_routes.iter().enumerate() {
        let client_route = client_routes.iter().find(|entry| {
            entry.get("path").and_then(serde_json::Value::as_str) == Some(route.path.as_str())
        });
        let route_bytes = client_route.map(manifest_entry_bytes).unwrap_or_default();
        let first_load_bytes = client_route.map(first_load_bytes).unwrap_or_default();
        let branch = if index + 1 == page_routes.len() {
            "└"
        } else {
            "├"
        };
        let symbol = route_render_symbol(route.render.strategy);
        println!(
            "{branch} {symbol} {}{}{}{}{}",
            route.path,
            spaces(39, route.path.len()),
            format_bytes(route_bytes),
            spaces(16, format_bytes(route_bytes).len()),
            format_bytes(first_load_bytes),
        );
    }

    let shared_bytes = shared_chunks
        .iter()
        .map(manifest_entry_bytes)
        .sum::<usize>();
    println!(
        "+ First Load JS shared by all{}{}",
        spaces(39, "First Load JS shared by all".len()),
        format_bytes(shared_bytes)
    );
    for (index, chunk) in shared_chunks.iter().enumerate() {
        let file = chunk
            .get("file")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("shared chunk");
        let branch = if index + 1 == shared_chunks.len() {
            "└"
        } else {
            "├"
        };
        println!(
            "  {branch} {file}{}{}",
            spaces(47, file.len()),
            format_bytes(manifest_entry_bytes(chunk))
        );
    }

    println!("\n○  (CSR)      client-rendered");
    println!("●  (Static)   pre-rendered at build time");
    println!("◐  (ISR/PPR)  pre-rendered with revalidation or streamed slots");
    println!("ƒ  (Dynamic)  server-rendered on demand");
    println!(
        "\n {} Built {} output for {} in {}\n",
        success(),
        path_text(out_dir),
        accent(format!("{:?}", target).to_lowercase()),
        accent(format_duration(duration))
    );
}

fn manifest_entry_bytes(entry: &serde_json::Value) -> usize {
    entry
        .get("bytes")
        .and_then(serde_json::Value::as_u64)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .unwrap_or_default()
}

fn first_load_bytes(entry: &serde_json::Value) -> usize {
    let mut files = BTreeSet::new();
    let mut total = 0;
    add_manifest_entry_bytes(entry, &mut files, &mut total);
    for section in ["chunks", "sharedChunks"] {
        for chunk in entry
            .get(section)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            add_manifest_entry_bytes(chunk, &mut files, &mut total);
        }
    }
    total
}

fn add_manifest_entry_bytes(
    entry: &serde_json::Value,
    files: &mut BTreeSet<String>,
    total: &mut usize,
) {
    let should_count = entry
        .get("file")
        .and_then(serde_json::Value::as_str)
        .map(|file| files.insert(file.to_string()))
        .unwrap_or(true);
    if should_count {
        *total += manifest_entry_bytes(entry);
    }
}

fn route_render_symbol(strategy: RenderStrategy) -> &'static str {
    match strategy {
        RenderStrategy::Csr => "○",
        RenderStrategy::Ssg => "●",
        RenderStrategy::Isr | RenderStrategy::Ppr => "◐",
        RenderStrategy::Ssr => "ƒ",
    }
}

const BUILD_OUTPUT_DIRS: [&str; 4] = ["server", "client", "assets", "prerender"];
const BUILD_OUTPUT_FILES: [&str; 2] = ["manifest.json", "build.json"];
const MAX_PRERENDER_PARALLELISM: usize = 2;
const WINDOWS_RENAME_RETRY_COUNT: usize = 5;

/// A route that was pre-rendered at build time.
#[derive(Debug)]
struct PrerenderedRoute {
    path: String,
    strategy: RenderStrategy,
    revalidate: Option<u64>,
    html_file: PathBuf,
}

#[derive(Debug, Clone)]
enum PrerenderJobKind {
    Csr,
    Render {
        route_file: PathBuf,
        mode: &'static str,
    },
}

#[derive(Debug, Clone)]
struct PrerenderJob {
    route_path: String,
    render_path: String,
    params: BTreeMap<String, String>,
    strategy: RenderStrategy,
    revalidate: Option<u64>,
    kind: PrerenderJobKind,
}

/// Pre-render all SSG, ISR, and PPR routes at build time.
///
/// For each qualifying route:
/// - SSG static routes: rendered once, saved as `.html`
/// - SSG dynamic routes (with `getStaticParams`): calls the export to discover params, renders each
/// - ISR routes: same as SSG but metadata records the revalidation interval
/// - PPR routes: renders the static shell (Suspense fallbacks, not dynamic content)
/// - CSR routes: emits a minimal shell HTML (no server rendering)
///
/// Returns a list of all pre-rendered routes with their metadata.
fn prerender_static_routes(
    root: &Path,
    app_dir: &Path,
    manifest: &RouteManifest,
    prerender_dir: &Path,
    client_dir: &Path,
    styles: &str,
    configured_parallelism: Option<usize>,
) -> anyhow::Result<Vec<PrerenderedRoute>> {
    use ruvyxa_graph::RouteKind;

    let routes_to_prerender: Vec<&RouteEntry> = manifest
        .routes
        .iter()
        .filter(|route| {
            route.kind == RouteKind::Page
                && matches!(
                    route.render.strategy,
                    RenderStrategy::Ssg
                        | RenderStrategy::Isr
                        | RenderStrategy::Ppr
                        | RenderStrategy::Csr
                )
        })
        .collect();

    if routes_to_prerender.is_empty() {
        return Ok(Vec::new());
    }

    fs::create_dir_all(prerender_dir)?;

    let Some(renderer_script) = find_runtime_script(root, "ssg-renderer.mjs") else {
        // If the SSG renderer isn't available, skip pre-rendering with a warning
        tracing::warn!(
            "SSG renderer script not found; skipping pre-rendering of {} routes",
            routes_to_prerender.len()
        );
        return Ok(Vec::new());
    };

    let mut jobs = Vec::new();

    for route in routes_to_prerender {
        match route.render.strategy {
            RenderStrategy::Csr => {
                jobs.push(PrerenderJob {
                    route_path: route.path.clone(),
                    render_path: route.path.clone(),
                    params: BTreeMap::new(),
                    strategy: RenderStrategy::Csr,
                    revalidate: None,
                    kind: PrerenderJobKind::Csr,
                });
            }
            RenderStrategy::Ssg | RenderStrategy::Isr | RenderStrategy::Ppr => {
                // For dynamic routes with getStaticParams, resolve static paths first
                let paths_to_render = if route.render.has_static_params
                    && (route.path.contains(':') || route.path.contains('*'))
                {
                    resolve_static_params(root, app_dir, route, &renderer_script)?
                } else if !route.path.contains(':') && !route.path.contains('*') {
                    // Pure static route — render the single path
                    vec![StaticRouteParams {
                        path: route.path.clone(),
                        params: BTreeMap::new(),
                    }]
                } else {
                    // Dynamic route without getStaticParams — skip (will be rendered at request time)
                    continue;
                };

                let mode = match route.render.strategy {
                    RenderStrategy::Ppr => "ppr",
                    _ => "full",
                };
                for static_route in paths_to_render {
                    jobs.push(PrerenderJob {
                        route_path: route.path.clone(),
                        render_path: static_route.path,
                        params: static_route.params,
                        strategy: route.render.strategy,
                        revalidate: route.render.revalidate,
                        kind: PrerenderJobKind::Render {
                            route_file: route.file.clone(),
                            mode,
                        },
                    });
                }
            }
            _ => {}
        }
    }

    let parallelism = prerender_parallelism(configured_parallelism, jobs.len());
    let chunk_size = jobs.len().max(1).div_ceil(parallelism);
    let mut prerendered = Vec::with_capacity(jobs.len());

    std::thread::scope(|scope| -> anyhow::Result<()> {
        let mut handles = Vec::new();

        for (chunk_index, chunk) in jobs.chunks(chunk_size).enumerate() {
            let jobs = chunk.to_vec();
            let offset = chunk_index * chunk_size;
            let renderer_script = renderer_script.clone();

            handles.push(
                scope.spawn(move || -> anyhow::Result<Vec<(usize, PrerenderedRoute)>> {
                    jobs.iter()
                        .enumerate()
                        .map(|(index, job)| {
                            render_prerender_job(
                                root,
                                app_dir,
                                prerender_dir,
                                client_dir,
                                styles,
                                &renderer_script,
                                job,
                            )
                            .map(|route| (offset + index, route))
                        })
                        .collect()
                }),
            );
        }

        for handle in handles {
            let mut next = handle
                .join()
                .map_err(|_| anyhow::anyhow!("pre-render worker panicked"))??;
            prerendered.append(&mut next);
        }

        Ok(())
    })?;
    prerendered.sort_by_key(|(index, _)| *index);
    let prerendered = prerendered
        .into_iter()
        .map(|(_, route)| route)
        .collect::<Vec<_>>();

    // Write pre-render manifest for the production server
    let prerender_manifest = serde_json::json!({
        "routes": prerendered.iter().map(|p| serde_json::json!({
            "path": p.path,
            "strategy": format!("{:?}", p.strategy).to_lowercase(),
            "revalidate": p.revalidate,
            "htmlFile": p.html_file.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
        })).collect::<Vec<_>>()
    });
    fs::write(
        prerender_dir.join("manifest.json"),
        serde_json::to_string_pretty(&prerender_manifest)?,
    )?;

    info!(
        prerendered = prerendered.len(),
        "pre-rendered static routes"
    );

    Ok(prerendered)
}

fn render_prerender_job(
    root: &Path,
    app_dir: &Path,
    prerender_dir: &Path,
    client_dir: &Path,
    styles: &str,
    renderer_script: &Path,
    job: &PrerenderJob,
) -> anyhow::Result<PrerenderedRoute> {
    let html_path = prerender_html_path(prerender_dir, &job.render_path);
    if let Some(parent) = html_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let html = match &job.kind {
        PrerenderJobKind::Csr => csr_shell_html(&job.route_path, client_dir, styles),
        PrerenderJobKind::Render { route_file, mode } => {
            let output = ProcessCommand::new("node")
                .arg(renderer_script)
                .arg(root)
                .arg(app_dir)
                .arg(route_file)
                .arg(&job.render_path)
                .arg(mode)
                .arg(serde_json::to_string(&job.params)?)
                .output()
                .with_context(|| format!("SSG renderer failed for route {}", job.render_path))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                anyhow::bail!(
                    "Pre-rendering failed for {}: {}{}",
                    job.render_path,
                    stderr,
                    stdout
                );
            }

            let result: serde_json::Value =
                serde_json::from_slice(&output.stdout).with_context(|| {
                    format!("SSG renderer returned invalid JSON for {}", job.render_path)
                })?;

            if result.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                let message = result
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                anyhow::bail!("Pre-rendering failed for {}: {}", job.render_path, message);
            }

            let html = result
                .get("html")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let html = inject_prerender_styles(html, styles);
            inject_prerender_client_assets(
                &html,
                client_dir,
                &job.route_path,
                &job.render_path,
                &job.params,
            )
        }
    };

    fs::write(&html_path, html)?;
    Ok(PrerenderedRoute {
        path: job.render_path.clone(),
        strategy: job.strategy,
        revalidate: job.revalidate,
        html_file: html_path,
    })
}

/// Resolve static params for a dynamic SSG route by calling getStaticParams
/// via the SSG renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StaticRouteParams {
    path: String,
    params: BTreeMap<String, String>,
}

fn resolve_static_params(
    root: &Path,
    app_dir: &Path,
    route: &RouteEntry,
    renderer_script: &Path,
) -> anyhow::Result<Vec<StaticRouteParams>> {
    let output = ProcessCommand::new("node")
        .arg(renderer_script)
        .arg(root)
        .arg(app_dir)
        .arg(&route.file)
        .arg("__resolve_params__")
        .arg("params")
        .output()
        .with_context(|| format!("Failed to resolve static params for {}", route.path))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("getStaticParams failed for {}: {}", route.path, stderr);
    }

    let result: serde_json::Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("Invalid JSON from getStaticParams for {}", route.path))?;

    if result.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let message = result
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("getStaticParams failed for {}: {}", route.path, message);
    }

    let params_list = result
        .get("params")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    params_list
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let params = value.as_object().ok_or_else(|| {
                anyhow::anyhow!(
                    "getStaticParams for {} returned item {index}, which is not an object",
                    route.path
                )
            })?;
            let params = params
                .iter()
                .map(|(key, value)| {
                    let value = value.as_str().ok_or_else(|| {
                        anyhow::anyhow!(
                            "getStaticParams for {} returned a non-string value for '{key}'",
                            route.path
                        )
                    })?;
                    Ok((key.clone(), value.to_string()))
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
            Ok(StaticRouteParams {
                path: static_route_path(&route.path, &params)?,
                params,
            })
        })
        .collect()
}

fn static_route_path(
    route_path: &str,
    params: &BTreeMap<String, String>,
) -> anyhow::Result<String> {
    let mut segments = Vec::new();
    for segment in route_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        if let Some(name) = segment.strip_prefix(':') {
            let value = params.get(name).ok_or_else(|| {
                anyhow::anyhow!("getStaticParams is missing '{name}' for route {route_path}")
            })?;
            validate_static_path_segment(value, name, route_path)?;
            segments.push(value.clone());
        } else if let Some(name) = segment.strip_prefix('*') {
            let optional = name.ends_with('?');
            let name = name.trim_end_matches('?');
            let Some(value) = params.get(name) else {
                if optional {
                    continue;
                }
                anyhow::bail!("getStaticParams is missing '{name}' for route {route_path}");
            };
            for value_segment in value.split('/') {
                validate_static_path_segment(value_segment, name, route_path)?;
                segments.push(value_segment.to_string());
            }
        } else {
            segments.push(segment.to_string());
        }
    }
    Ok(if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    })
}

fn validate_static_path_segment(value: &str, name: &str, route_path: &str) -> anyhow::Result<()> {
    if value.is_empty() || matches!(value, "." | "..") || value.contains(['/', '\\', '?', '#']) {
        anyhow::bail!(
            "getStaticParams returned unsafe value '{value}' for '{name}' in route {route_path}"
        );
    }
    Ok(())
}

/// Generate the output HTML file path for a pre-rendered route.
fn prerender_html_path(prerender_dir: &Path, route_path: &str) -> PathBuf {
    let sanitized = route_path.trim_matches('/');
    if sanitized.is_empty() {
        prerender_dir.join("index.html")
    } else {
        prerender_dir.join(sanitized).join("index.html")
    }
}

/// Generate a minimal CSR shell HTML document.
fn csr_shell_html(route_path: &str, client_dir: &Path, styles: &str) -> String {
    let assets = prerender_client_assets(client_dir, route_path);
    let preload_links = assets
        .as_ref()
        .map(|assets| module_preload_links(&assets.preloads))
        .unwrap_or_default();
    let client_src = assets.map(|assets| assets.src).unwrap_or_else(|| {
        format!(
            "/__ruvyxa/client/{}.js",
            route_path.trim_start_matches('/').replace('/', "__")
        )
    });
    format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Loading...</title>
  <style data-ruvyxa-css>{styles}</style>
  {preload_links}
  <script>window.__RUVYXA_REQUEST_PATH__ = {path_json};</script>
</head>
<body>
  <div id="__ruvyxa"></div>
  <script type="module" src="{client_src}"></script>
</body>
</html>"#,
        path_json = serde_json::to_string(route_path).unwrap_or_else(|_| "\"\"".to_string()),
    )
}

fn inject_prerender_styles(html: &str, styles: &str) -> String {
    let style_tag = format!(r#"<style data-ruvyxa-css>{styles}</style>"#);
    let lower = html.to_ascii_lowercase();
    if let Some(head_end) = lower.find("</head>") {
        let mut output = String::with_capacity(html.len() + style_tag.len());
        output.push_str(&html[..head_end]);
        output.push_str(&style_tag);
        output.push_str(&html[head_end..]);
        return output;
    }

    format!("<!doctype html><html><head>{style_tag}</head><body>{html}</body></html>")
}

#[derive(Debug)]
struct PrerenderClientAssets {
    src: String,
    preloads: Vec<String>,
}

fn prerender_client_assets(client_dir: &Path, route_path: &str) -> Option<PrerenderClientAssets> {
    let source = fs::read_to_string(client_dir.join("manifest.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&source).ok()?;
    let route = manifest
        .get("routes")?
        .as_array()?
        .iter()
        .find(|route| route.get("path").and_then(|path| path.as_str()) == Some(route_path))?;
    let src = route.get("src")?.as_str()?.to_string();
    let preloads = route
        .get("sharedChunks")
        .and_then(|chunks| chunks.as_array())
        .into_iter()
        .flatten()
        .filter_map(|chunk| chunk.get("src").and_then(|src| src.as_str()))
        .map(str::to_string)
        .collect();
    Some(PrerenderClientAssets { src, preloads })
}

fn module_preload_links(preloads: &[String]) -> String {
    preloads
        .iter()
        .map(|src| format!(r#"<link rel="modulepreload" href="{src}">"#))
        .collect()
}

fn inject_prerender_client_assets(
    html: &str,
    client_dir: &Path,
    route_path: &str,
    request_path: &str,
    params: &BTreeMap<String, String>,
) -> String {
    let Some(assets) = prerender_client_assets(client_dir, route_path) else {
        return html.to_string();
    };
    let preload_links = module_preload_links(&assets.preloads);
    let request_path_json = inline_script_json(request_path, "\"/\"");
    let params_json = inline_script_json(params, "{}");
    let scripts = format!(
        r#"<script>globalThis.__RUVYXA_ROUTE_PARAMS__ = {params_json};globalThis.__RUVYXA_REQUEST_PATH__ = {request_path_json};</script><script type="module" src="{}"></script>"#,
        assets.src
    );
    let lower = html.to_ascii_lowercase();
    if let (Some(head_end), Some(body_end)) = (lower.find("</head>"), lower.rfind("</body>"))
        && head_end <= body_end
    {
        let mut output = String::with_capacity(html.len() + preload_links.len() + scripts.len());
        output.push_str(&html[..head_end]);
        output.push_str(&preload_links);
        output.push_str(&html[head_end..body_end]);
        output.push_str(&scripts);
        output.push_str(&html[body_end..]);
        return output;
    }

    format!("<!doctype html><html><head>{preload_links}</head><body>{html}{scripts}</body></html>")
}

fn inline_script_json<T: serde::Serialize + ?Sized>(value: &T, fallback: &str) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| fallback.to_string())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

struct ClientBundle {
    path: String,
    entry: PathBuf,
    file_name: String,
    script: String,
    source_map_file: Option<String>,
    source_map: Option<String>,
    output_bytes: usize,
    estimated_gz_bytes: usize,
    duration_ms: u64,
    module_count: usize,
    cache_hits: usize,
    tree_shaken_modules: usize,
    chunk_manifest: Option<serde_json::Value>,
    chunks: Vec<ruvyxa_bundler::OutputChunk>,
}

struct SharedRouteChunk {
    file_name: String,
    code: String,
    modules: Vec<String>,
    routes: Vec<String>,
}

#[derive(Clone)]
struct JsConfigPluginBridge {
    project_root: PathBuf,
    worker: Arc<Mutex<JsPluginWorker>>,
    has_resolve_id: bool,
    has_transform: bool,
}

struct JsPluginWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginRunnerOutput {
    ok: bool,
    result: Option<serde_json::Value>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

impl ruvyxa_bundler::plugin::NativeBundlerPlugin for JsConfigPluginBridge {
    fn name(&self) -> &str {
        "ruvyxa-config-js-plugins"
    }

    fn resolve_id(
        &self,
        specifier: &str,
        importer: Option<&Path>,
        ctx: &ruvyxa_bundler::plugin::PluginContext,
    ) -> ruvyxa_bundler::Result<Option<PathBuf>> {
        if !self.has_resolve_id {
            return Ok(None);
        }

        let payload = serde_json::json!({
            "id": specifier,
            "importer": importer.map(|path| path.display().to_string()),
            "environment": plugin_environment(ctx.target)
        });
        let Some(value) = self.call_runner("resolveId", payload)? else {
            return Ok(None);
        };
        let Some(path) = value.as_str() else {
            return Ok(None);
        };

        let resolved = PathBuf::from(path);
        let resolved = if resolved.is_absolute() {
            resolved
        } else {
            self.project_root.join(resolved)
        };

        Ok(Some(resolved.canonicalize().unwrap_or(resolved)))
    }

    fn transform(
        &self,
        code: &str,
        id: &Path,
        ctx: &ruvyxa_bundler::plugin::PluginContext,
    ) -> ruvyxa_bundler::Result<Option<ruvyxa_bundler::plugin::TransformResult>> {
        if !self.has_transform {
            return Ok(None);
        }

        let payload = serde_json::json!({
            "code": code,
            "id": id.display().to_string(),
            "environment": plugin_environment(ctx.target)
        });
        let Some(value) = self.call_runner("transform", payload)? else {
            return Ok(None);
        };
        let Some(code) = value.get("code").and_then(|value| value.as_str()) else {
            return Ok(None);
        };

        let map = value
            .get("map")
            .and_then(|value| value.as_str())
            .map(str::to_string);

        Ok(Some(ruvyxa_bundler::plugin::TransformResult {
            code: code.to_string(),
            map,
        }))
    }
}

impl JsConfigPluginBridge {
    fn call_runner(
        &self,
        hook: &str,
        mut payload: serde_json::Value,
    ) -> ruvyxa_bundler::Result<Option<serde_json::Value>> {
        payload["hook"] = serde_json::Value::String(hook.to_string());
        let mut worker = self.worker.lock().map_err(|_| {
            ruvyxa_bundler::BundleError::Compiler("JS plugin worker lock was poisoned".into())
        })?;
        let result = worker.call(&payload)?;

        if result.ok {
            return Ok(result.result);
        }

        Err(ruvyxa_bundler::BundleError::Compiler(format!(
            "{} {}",
            result.code.unwrap_or_else(|| "RUV1700".to_string()),
            result
                .message
                .or(result.stack)
                .unwrap_or_else(|| "JS plugin hook failed".to_string())
        )))
    }
}

impl JsPluginWorker {
    fn spawn(runner: &Path, project_root: &Path) -> ruvyxa_bundler::Result<Self> {
        let mut child = ProcessCommand::new("node")
            .arg(runner)
            .arg(project_root)
            .arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                ruvyxa_bundler::BundleError::Compiler(format!(
                    "failed to start persistent JS plugin worker: {err}"
                ))
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ruvyxa_bundler::BundleError::Compiler("failed to open JS plugin worker stdin".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ruvyxa_bundler::BundleError::Compiler("failed to open JS plugin worker stdout".into())
        })?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn call(&mut self, payload: &serde_json::Value) -> ruvyxa_bundler::Result<PluginRunnerOutput> {
        writeln!(self.stdin, "{payload}").map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "failed to send JS plugin worker payload: {err}"
            ))
        })?;
        self.stdin.flush().map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "failed to flush JS plugin worker payload: {err}"
            ))
        })?;

        let mut stdout = String::new();
        let bytes_read = self.stdout.read_line(&mut stdout).map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "failed to read JS plugin worker response: {err}"
            ))
        })?;
        if bytes_read == 0 {
            let status = self
                .child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| status.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return Err(ruvyxa_bundler::BundleError::Compiler(format!(
                "JS plugin worker exited before responding (status: {status})"
            )));
        }

        serde_json::from_str(stdout.trim()).map_err(|err| {
            ruvyxa_bundler::BundleError::Compiler(format!(
                "JS plugin worker returned invalid output: {err}; stdout: {}",
                stdout.trim()
            ))
        })
    }
}

impl Drop for JsPluginWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn plugin_environment(target: ruvyxa_bundler::BundleTarget) -> &'static str {
    match target {
        ruvyxa_bundler::BundleTarget::Client => "client",
        ruvyxa_bundler::BundleTarget::Ssr => "server",
    }
}

fn bundle_context_for_build(
    root: &Path,
    plugins: &[BuildPluginConfig],
    config_dependency_hash: &str,
    cache_dir: &Path,
) -> anyhow::Result<ruvyxa_bundler::BundleContext> {
    let compile_cache = ruvyxa_bundler::cache::CompileCache::at_dir_with_namespace(
        cache_dir,
        true,
        config_dependency_hash,
    );
    let has_resolve_id = plugins.iter().any(|plugin| plugin.resolve_id);
    let has_transform = plugins.iter().any(|plugin| plugin.transform);
    if !has_resolve_id && !has_transform {
        return Ok(ruvyxa_bundler::BundleContext::with_all_caches(
            compile_cache,
            ruvyxa_bundler::resolver::ResolveGraphCache::new(),
            ruvyxa_bundler::incremental::IncrementalGraphCache::new(root, true),
        ));
    }

    let runner = find_runtime_script(root, "plugin-runner.mjs")
        .ok_or_else(|| anyhow::anyhow!("RUV1701 JS plugin runner not found"))?;
    let project_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let worker = JsPluginWorker::spawn(&runner, &project_root)?;
    let bridge = JsConfigPluginBridge {
        project_root,
        worker: Arc::new(Mutex::new(worker)),
        has_resolve_id,
        has_transform,
    };

    Ok(ruvyxa_bundler::BundleContext::with_plugins(
        compile_cache,
        ruvyxa_bundler::resolver::ResolveGraphCache::new(),
        ruvyxa_bundler::incremental::IncrementalGraphCache::new(root, true),
        ruvyxa_bundler::plugin::PluginPipeline::new(vec![Arc::new(bridge)]),
    ))
}

fn emit_client_bundles(
    root: &Path,
    app_dir: &Path,
    manifest: &RouteManifest,
    client_dir: &Path,
    build: &BuildConfigOptions,
    plugins: &[BuildPluginConfig],
    cache: NativeBuildCache<'_>,
) -> anyhow::Result<serde_json::Value> {
    let bundle_context =
        bundle_context_for_build(root, plugins, cache.dependency_hash, cache.directory)?;
    let page_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .cloned()
        .collect::<Vec<_>>();
    let parallelism = build_parallelism(build.parallelism, page_routes.len());
    let chunk_size = page_routes.len().max(1).div_ceil(parallelism);
    let mut bundles = Vec::new();

    std::thread::scope(|scope| -> anyhow::Result<()> {
        let mut handles = Vec::new();

        for (chunk_index, chunk) in page_routes.chunks(chunk_size).enumerate() {
            let routes = chunk.to_vec();
            let offset = chunk_index * chunk_size;
            let bundle_context = bundle_context.clone();

            handles.push(
                scope.spawn(move || -> anyhow::Result<Vec<(usize, ClientBundle)>> {
                    routes
                        .iter()
                        .enumerate()
                        .map(|(index, route)| {
                            bundle_client_route(root, app_dir, route, build, &bundle_context)
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
    let mut route_chunk_manifests = Vec::new();
    let mut total_output_bytes = 0usize;
    let mut total_estimated_gz_bytes = 0usize;
    let mut total_duration_ms = 0u64;
    let mut total_modules = 0usize;
    let mut total_cache_hits = 0usize;
    let mut total_tree_shaken_modules = 0usize;

    for (_, bundle) in bundles {
        fs::write(client_dir.join(&bundle.file_name), bundle.script.as_bytes())?;
        if let (Some(source_map_file), Some(source_map)) =
            (&bundle.source_map_file, &bundle.source_map)
        {
            fs::write(client_dir.join(source_map_file), source_map.as_bytes())?;
        }
        total_output_bytes += bundle.output_bytes;
        total_estimated_gz_bytes += bundle.estimated_gz_bytes;
        total_duration_ms += bundle.duration_ms;
        total_modules += bundle.module_count;
        total_cache_hits += bundle.cache_hits;
        total_tree_shaken_modules += bundle.tree_shaken_modules;

        if let Some(chunk_manifest) = &bundle.chunk_manifest {
            route_chunk_manifests.push(chunk_manifest.clone());
        }

        for chunk in &bundle.chunks {
            fs::write(client_dir.join(&chunk.file_name), chunk.code.as_bytes())?;
        }

        let mut route_info = serde_json::json!({
            "path": bundle.path,
            "entry": bundle.entry,
            "file": bundle.file_name,
            "src": format!("/__ruvyxa/client/{}", bundle.file_name),
            "sourceMap": bundle.source_map_file,
            "bytes": bundle.script.len(),
            "outputBytes": bundle.output_bytes,
            "estimatedGzBytes": bundle.estimated_gz_bytes,
            "durationMs": bundle.duration_ms,
            "moduleCount": bundle.module_count,
            "cacheHits": bundle.cache_hits,
            "treeShakenModules": bundle.tree_shaken_modules,
            "optimized": true,
            "treeShaken": build.tree_shaking.unwrap_or(true),
            "chunkStrategy": build.split_strategy.as_deref().unwrap_or("route")
        });

        if let Some(chunk_manifest) = bundle.chunk_manifest {
            route_info["chunkManifest"] = chunk_manifest;
        }
        route_info["chunks"] = serde_json::Value::Array(
            bundle
                .chunks
                .iter()
                .map(output_chunk_manifest)
                .collect::<Vec<_>>(),
        );

        routes.push(route_info);
    }

    let shared_route_chunks = if parse_split_strategy(build.split_strategy.as_deref())?
        == ruvyxa_bundler::SplitStrategy::Route
    {
        emit_shared_route_chunks(client_dir, &route_chunk_manifests)?
    } else {
        Vec::new()
    };

    for route in &mut routes {
        let route_path = route
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("/");
        let route_shared_chunks = shared_route_chunks
            .iter()
            .filter(|chunk| chunk.routes.iter().any(|path| path == route_path))
            .map(shared_route_chunk_manifest)
            .collect::<Vec<_>>();
        route["sharedChunks"] = serde_json::Value::Array(route_shared_chunks);
        if let Some(chunk_manifest) = route.get_mut("chunkManifest") {
            attach_shared_chunks_to_manifest(chunk_manifest, &shared_route_chunks);
        }
    }

    if build.emit_chunk_manifest.unwrap_or(false) {
        fs::write(
            client_dir.join("chunk-manifest.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "routes": route_chunk_manifests
                    .iter()
                    .map(|manifest| {
                        let mut manifest = manifest.clone();
                        attach_shared_chunks_to_manifest(&mut manifest, &shared_route_chunks);
                        manifest
                    })
                    .collect::<Vec<_>>(),
                "shared": shared_route_chunks
                    .iter()
                    .map(shared_route_chunk_manifest)
                    .collect::<Vec<_>>()
            }))?,
        )?;
    }

    Ok(serde_json::json!({
        "chunkStrategy": build.split_strategy.as_deref().unwrap_or("route"),
        "minify": build.minify.unwrap_or(true),
        "sourcemap": build.sourcemap.unwrap_or(false),
        "treeShaking": build.tree_shaking.unwrap_or(true),
        "jsxRuntime": build.jsx_runtime.as_deref().unwrap_or("classic"),
        "esTarget": build.es_target.as_deref().unwrap_or("es2022"),
        "emitChunkManifest": build.emit_chunk_manifest.unwrap_or(false),
        "parallelism": parallelism,
        "moduleCount": total_modules,
        "outputBytes": total_output_bytes,
        "estimatedGzBytes": total_estimated_gz_bytes,
        "durationMs": total_duration_ms,
        "cacheHits": total_cache_hits,
        "treeShakenModules": total_tree_shaken_modules,
        "plugins": build_plugin_manifest(plugins),
        "sharedRouteChunks": shared_route_chunks
            .iter()
            .map(shared_route_chunk_manifest)
            .collect::<Vec<_>>(),
        "cache": {
            "directory": bundle_context.compile_cache().cache_dir(),
            "compileEntries": bundle_context.compile_cache().entry_count(),
            "compileBytes": bundle_context.compile_cache().total_bytes()
        },
        "routes": routes
    }))
}

fn build_parallelism(configured: Option<usize>, work_items: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    configured.unwrap_or(available).clamp(1, work_items.max(1))
}

fn prerender_parallelism(configured: Option<usize>, work_items: usize) -> usize {
    let default = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(MAX_PRERENDER_PARALLELISM);
    configured
        .unwrap_or(default)
        .min(MAX_PRERENDER_PARALLELISM)
        .clamp(1, work_items.max(1))
}

fn build_plugin_manifest(plugins: &[BuildPluginConfig]) -> serde_json::Value {
    serde_json::Value::Array(
        plugins
            .iter()
            .map(|plugin| {
                serde_json::json!({
                    "name": plugin.name,
                    "enforce": plugin.enforce,
                    "resolveId": plugin.resolve_id,
                    "transform": plugin.transform
                })
            })
            .collect(),
    )
}

/// Bundle a client route using the native Rust bundler (ruvyxa_bundler).
fn bundle_client_route(
    root: &Path,
    app_dir: &Path,
    route: &RouteEntry,
    build: &BuildConfigOptions,
    bundle_context: &ruvyxa_bundler::BundleContext,
) -> anyhow::Result<ClientBundle> {
    use ruvyxa_bundler::{BundleInput, BundleOptions, BundleTarget};

    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let app_dir = app_dir
        .canonicalize()
        .unwrap_or_else(|_| app_dir.to_path_buf());
    let entry = canonical_route_file(&root, &route.file);
    let layouts: Vec<PathBuf> = route
        .layout_chain
        .iter()
        .filter_map(|layout_path| resolve_layout_file(&root, &app_dir, layout_path))
        .collect();

    let input = BundleInput {
        entry,
        project_root: root,
        app_dir,
        layouts,
        request_path: route.path.clone(),
        target: BundleTarget::Client,
        options: BundleOptions {
            minify: build.minify.unwrap_or(true),
            source_map: build.sourcemap.unwrap_or(false),
            tree_shaking: build.tree_shaking.unwrap_or(true),
            jsx_runtime: parse_jsx_runtime(build.jsx_runtime.as_deref())?,
            es_target: parse_es_target(build.es_target.as_deref())?,
            split_strategy: parse_split_strategy(build.split_strategy.as_deref())?,
            emit_chunk_manifest: build.emit_chunk_manifest.unwrap_or(false),
        },
    };

    let output = ruvyxa_bundler::bundle_with_context(input, bundle_context)
        .map_err(|e| anyhow::anyhow!("native bundler error for {}: {e}", route.path))?;

    // Report non-fatal diagnostics.
    for diagnostic in &output.diagnostics {
        tracing::warn!("{diagnostic}");
    }

    let hash = content_hash(&output.code);
    let file_name = format!("{hash}.js");
    let source_map_file = output.source_map.as_ref().map(|_| format!("{hash}.js.map"));
    let script = if let Some(source_map_file) = &source_map_file {
        format!("{}\n//# sourceMappingURL={source_map_file}\n", output.code)
    } else {
        output.code.clone()
    };

    Ok(ClientBundle {
        path: route.path.clone(),
        entry: route.file.clone(),
        file_name,
        script,
        source_map_file,
        source_map: output.source_map,
        output_bytes: output.stats.output_bytes,
        estimated_gz_bytes: output.stats.estimated_gz_bytes,
        duration_ms: output.stats.duration_ms,
        module_count: output.stats.module_count,
        cache_hits: output.stats.cache_hits,
        tree_shaken_modules: output.stats.tree_shaken_modules,
        chunk_manifest: output
            .chunk_manifest
            .map(serde_json::to_value)
            .transpose()?,
        chunks: output.chunks,
    })
}

fn output_chunk_manifest(chunk: &ruvyxa_bundler::OutputChunk) -> serde_json::Value {
    serde_json::json!({
        "file": chunk.file_name,
        "src": format!("/__ruvyxa/client/{}", chunk.file_name),
        "kind": chunk.kind,
        "modules": chunk.modules,
        "bytes": chunk.code.len()
    })
}

fn emit_shared_route_chunks(
    client_dir: &Path,
    route_manifests: &[serde_json::Value],
) -> anyhow::Result<Vec<SharedRouteChunk>> {
    let mut module_routes = BTreeMap::<String, BTreeSet<String>>::new();

    for manifest in route_manifests {
        let route = manifest
            .get("route")
            .and_then(|value| value.as_str())
            .unwrap_or("/")
            .to_string();
        let Some(modules) = manifest.get("modules").and_then(|value| value.as_array()) else {
            continue;
        };

        for module in modules.iter().filter_map(|value| value.as_str()) {
            module_routes
                .entry(module.to_string())
                .or_default()
                .insert(route.clone());
        }
    }

    let shared_modules = module_routes
        .iter()
        .filter(|(_, routes)| routes.len() >= 2)
        .map(|(module, _)| module.clone())
        .collect::<Vec<_>>();

    if shared_modules.is_empty() {
        return Ok(Vec::new());
    }

    let shared_routes = module_routes
        .values()
        .flat_map(|routes| routes.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let code = format!(
        "// Generated by Ruvyxa. Shared route chunk metadata.\nexport const modules = {};\nexport const routes = {};\nexport default {{ modules, routes }};\n",
        serde_json::to_string(&shared_modules)?,
        serde_json::to_string(&shared_routes)?,
    );
    let file_name = format!("shared.{}.js", content_hash(&code));
    fs::write(client_dir.join(&file_name), code.as_bytes())?;

    Ok(vec![SharedRouteChunk {
        file_name,
        code,
        modules: shared_modules,
        routes: shared_routes,
    }])
}

fn shared_route_chunk_manifest(chunk: &SharedRouteChunk) -> serde_json::Value {
    serde_json::json!({
        "file": chunk.file_name,
        "src": format!("/__ruvyxa/client/{}", chunk.file_name),
        "modules": chunk.modules,
        "routes": chunk.routes,
        "bytes": chunk.code.len()
    })
}

fn attach_shared_chunks_to_manifest(
    manifest: &mut serde_json::Value,
    shared_chunks: &[SharedRouteChunk],
) {
    let route_modules = manifest
        .get("modules")
        .and_then(|value| value.as_array())
        .map(|modules| {
            modules
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let route_shared = shared_chunks
        .iter()
        .filter(|chunk| {
            chunk
                .modules
                .iter()
                .any(|module| route_modules.contains(module.as_str()))
        })
        .map(shared_route_chunk_manifest)
        .collect::<Vec<_>>();

    manifest["sharedChunks"] = serde_json::Value::Array(route_shared);
}

fn parse_jsx_runtime(value: Option<&str>) -> anyhow::Result<ruvyxa_bundler::JsxRuntime> {
    match value.unwrap_or("classic").to_ascii_lowercase().as_str() {
        "classic" => Ok(ruvyxa_bundler::JsxRuntime::Classic),
        "automatic" => Ok(ruvyxa_bundler::JsxRuntime::Automatic),
        other => anyhow::bail!(
            "RUV1601 build.jsxRuntime must be `classic` or `automatic`, got `{other}`"
        ),
    }
}

fn parse_es_target(value: Option<&str>) -> anyhow::Result<ruvyxa_bundler::EsTarget> {
    match value.unwrap_or("es2022").to_ascii_lowercase().as_str() {
        "es2018" => Ok(ruvyxa_bundler::EsTarget::Es2018),
        "es2019" => Ok(ruvyxa_bundler::EsTarget::Es2019),
        "es2020" => Ok(ruvyxa_bundler::EsTarget::Es2020),
        "es2022" => Ok(ruvyxa_bundler::EsTarget::Es2022),
        "esnext" => Ok(ruvyxa_bundler::EsTarget::EsNext),
        other => anyhow::bail!(
            "RUV1601 build.esTarget must be es2018, es2019, es2020, es2022, or esnext, got `{other}`"
        ),
    }
}

fn parse_split_strategy(value: Option<&str>) -> anyhow::Result<ruvyxa_bundler::SplitStrategy> {
    match value.unwrap_or("route").to_ascii_lowercase().as_str() {
        "single" | "manual" => Ok(ruvyxa_bundler::SplitStrategy::Single),
        "route" => Ok(ruvyxa_bundler::SplitStrategy::Route),
        other => anyhow::bail!(
            "RUV1601 build.splitStrategy must be `single`, `route`, or `manual`, got `{other}`"
        ),
    }
}

fn content_hash(input: &str) -> String {
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

fn canonical_route_file(root: &Path, file: &Path) -> PathBuf {
    if file.is_absolute() {
        return file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    }

    file.canonicalize()
        .or_else(|_| root.join(file).canonicalize())
        .unwrap_or_else(|_| root.join(file))
}

fn resolve_layout_file(root: &Path, app_dir: &Path, layout_path: &str) -> Option<PathBuf> {
    let path = PathBuf::from(layout_path);
    let mut candidates = Vec::new();

    if path.is_absolute() {
        candidates.push(path);
    } else {
        candidates.push(root.join(&path));

        let app_relative = path
            .strip_prefix("app")
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.clone());
        candidates.push(app_dir.join(app_relative));
    }

    let mut expanded = Vec::new();
    for candidate in candidates {
        expanded.push(candidate.clone());
        if candidate.extension().is_none() {
            for extension in ["tsx", "jsx", "ts", "js"] {
                expanded.push(candidate.with_extension(extension));
            }
        }
    }

    expanded
        .into_iter()
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.canonicalize().ok().or(Some(candidate)))
}

fn create_build_staging_dir(out_dir: &Path) -> anyhow::Result<PathBuf> {
    create_build_temp_dir(out_dir, ".build-staging")
}

fn create_build_temp_dir(out_dir: &Path, prefix: &str) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(out_dir)?;
    let created_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp_dir = out_dir.join(format!("{prefix}-{}-{created_at}", std::process::id()));
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;
    Ok(temp_dir)
}

fn commit_staged_build_outputs(staging_dir: &Path, out_dir: &Path) -> anyhow::Result<()> {
    let backup_dir = create_build_temp_dir(out_dir, ".build-rollback")?;
    let moved_existing = match move_named_build_outputs(out_dir, &backup_dir) {
        Ok(moved) => moved,
        Err(error) => {
            let _ = fs::remove_dir_all(&backup_dir);
            return Err(error);
        }
    };
    let commit_result = move_named_build_outputs(staging_dir, out_dir);

    match commit_result {
        Ok(_) => {
            fs::remove_dir_all(&backup_dir)?;
            if staging_dir.exists() {
                fs::remove_dir_all(staging_dir)?;
            }
            Ok(())
        }
        Err(error) => {
            let _ = remove_named_build_outputs(out_dir);
            let rollback_result =
                restore_named_build_outputs(&backup_dir, out_dir, &moved_existing);
            let _ = fs::remove_dir_all(&backup_dir);
            if let Err(rollback_error) = rollback_result {
                return Err(error).with_context(|| {
                    format!(
                        "rollback also failed while restoring previous output: {rollback_error}"
                    )
                });
            }
            Err(error)
        }
    }
}

fn move_named_build_outputs(from: &Path, to: &Path) -> anyhow::Result<Vec<String>> {
    fs::create_dir_all(to)?;
    let mut moved = Vec::new();

    for name in BUILD_OUTPUT_DIRS.into_iter().chain(BUILD_OUTPUT_FILES) {
        let source = from.join(name);
        if !source.exists() {
            continue;
        }
        let destination = to.join(name);
        if destination.exists() {
            remove_path(&destination)?;
        }
        if let Err(error) = rename_with_windows_retry(&source, &destination) {
            let rollback_result = restore_named_build_outputs(to, from, &moved);
            let mut move_error: anyhow::Error = error.into();
            move_error = move_error.context(format!(
                "failed to move {} to {}",
                source.display(),
                destination.display()
            ));
            if let Err(rollback_error) = rollback_result {
                return Err(move_error).with_context(|| {
                    format!("rollback of partially moved outputs also failed: {rollback_error}")
                });
            }
            return Err(move_error);
        }
        moved.push(name.to_string());
    }

    Ok(moved)
}

fn restore_named_build_outputs(
    backup_dir: &Path,
    out_dir: &Path,
    moved_existing: &[String],
) -> anyhow::Result<()> {
    for name in moved_existing {
        let source = backup_dir.join(name);
        if !source.exists() {
            continue;
        }
        let destination = out_dir.join(name);
        if destination.exists() {
            remove_path(&destination)?;
        }
        rename_with_windows_retry(&source, &destination).with_context(|| {
            format!(
                "failed to restore {} to {}",
                source.display(),
                destination.display()
            )
        })?;
    }

    Ok(())
}

fn rename_with_windows_retry(source: &Path, destination: &Path) -> std::io::Result<()> {
    let mut delay = Duration::from_millis(25);

    for attempt in 0..WINDOWS_RENAME_RETRY_COUNT {
        match fs::rename(source, destination) {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && error.kind() == std::io::ErrorKind::PermissionDenied
                    && attempt + 1 < WINDOWS_RENAME_RETRY_COUNT =>
            {
                std::thread::sleep(delay);
                delay = delay.saturating_mul(2);
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("the retry loop returns on its final attempt")
}

fn remove_named_build_outputs(out_dir: &Path) -> anyhow::Result<()> {
    for name in BUILD_OUTPUT_DIRS.into_iter().chain(BUILD_OUTPUT_FILES) {
        let path = out_dir.join(name);
        if path.exists() {
            remove_path(&path)?;
        }
    }

    Ok(())
}

fn remove_path(path: &Path) -> anyhow::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn find_runtime_script(root: &Path, file_name: &str) -> Option<PathBuf> {
    if let Ok(mut cwd) = std::env::current_dir() {
        loop {
            let candidate = cwd.join("packages/ruvyxa/runtime").join(file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
            if !cwd.pop() {
                break;
            }
        }
    }

    let package_renderer = root.join("node_modules/ruvyxa/runtime").join(file_name);
    if package_renderer.is_file() {
        return Some(package_renderer);
    }

    None
}

fn print_routes(args: ProjectArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let app_dir = args.root.join(config.app_dir());
    let manifest = discover_project_routes(&args.root, &config)?;
    let page_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .count();
    let api_routes = manifest.routes.len().saturating_sub(page_routes);

    print_tui_header("Routes");
    print_field("root", path_text(&args.root));
    print_field("app dir", path_text(&app_dir));
    print_field("routes", accent(manifest.routes.len().to_string()));
    print_field("pages", accent(page_routes.to_string()));
    print_field("api", accent(api_routes.to_string()));
    println!();
    print_route_row(
        "kind",
        label("kind"),
        "path",
        label("path"),
        "file",
        label("file"),
        label("id"),
    );
    for route in manifest.routes {
        let kind = format!("{:?}", route.kind);
        let file = display_path_relative(&args.root, &route.file);
        print_route_row(
            &kind,
            accent(&kind),
            &route.path,
            route.path.clone(),
            &file,
            dim(&file),
            dim(route.id),
        );
    }
    println!();

    Ok(())
}

fn analyze(args: ProjectArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let manifest = discover_project_routes(&args.root, &config)?;
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

fn check(args: ProjectArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    print_tui_header("Check");
    print_field("root", path_text(&args.root));
    println!();

    run_typecheck(&args.root)?;
    test_parity(args)?;

    println!(
        "{} Production readiness checks passed in {}\n",
        success(),
        accent(format_duration(started.elapsed()))
    );
    Ok(())
}

fn run_typecheck(root: &Path) -> anyhow::Result<()> {
    if !root.join("tsconfig.json").exists() {
        println!("{} TypeScript skipped (no tsconfig.json)", success());
        return Ok(());
    }

    let tsc = local_binary_upwards(root, "tsc").unwrap_or_else(|| PathBuf::from("tsc"));
    let output = ProcessCommand::new(&tsc)
        .arg("--noEmit")
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run TypeScript type check with {}", tsc.display()))?;

    if output.status.success() {
        println!("{} TypeScript type check passed", success());
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!("TypeScript type check failed\nstdout:\n{stdout}\nstderr:\n{stderr}")
}

fn doctor(args: ProjectArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let app_dir = args.root.join(config.app_dir());
    let package_json = args.root.join("package.json");
    let tsconfig = args.root.join("tsconfig.json");

    print_tui_header("Doctor");
    print_field("ruvyxa", accent(env!("CARGO_PKG_VERSION")));
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
    print_field("rustc", tool_status(tool_version("rustc", &["--version"])));
    print_field("cargo", tool_status(tool_version("cargo", &["--version"])));
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

    let manifest = discover_project_routes(&args.root, &config)?;
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
    let started = Instant::now();
    let config = load_project_config(&args.root)?;
    let out_dir = args.root.join(config.out_dir());
    let removed = out_dir.exists();
    if removed {
        fs::remove_dir_all(&out_dir)?;
    }
    print_tui_header("Clean");
    print_field(
        "status",
        if removed {
            ok_text("removed")
        } else {
            dim("already clean")
        },
    );
    print_field("out dir", path_text(&out_dir));
    print_field("duration", accent(format_duration(started.elapsed())));
    println!();
    Ok(())
}

fn trace(args: TraceArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let manifest = discover_project_routes(&args.root, &config)?;
    let route = manifest
        .routes
        .iter()
        .find(|entry| entry.path == args.route)
        .with_context(|| format!("route {} was not found", args.route))?;

    println!("{}", serde_json::to_string_pretty(route)?);
    Ok(())
}

fn bench(args: BenchArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    let samples = args.samples.max(1);
    let root = args.root;
    let config = load_project_config(&root)?;
    let app_dir = root.join(config.app_dir());
    let mut results = Vec::new();

    results.push(run_benchmark("route-discovery", samples, || {
        let _manifest = discover_project_routes(&root, &config)?;
        Ok(())
    })?);
    results.push(run_benchmark("analyze-validation", samples, || {
        let manifest = discover_project_routes(&root, &config)?;
        let validation = validate_app(&root, &manifest)?;
        fail_on_diagnostics(&validation.diagnostics)?;
        Ok(())
    })?);
    results.push(run_benchmark("production-build", samples, || {
        build_with_output(
            BuildArgs {
                root: root.clone(),
                target: Some(BuildTarget::Node),
            },
            false,
        )
    })?);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print_benchmark_table(samples, &results, &root, &app_dir, started.elapsed());
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
    let started = Instant::now();
    let config = load_project_config(&args.root)?;
    print_tui_header("Parity");
    print_field("root", path_text(&args.root));
    print_field("dev app", path_text(&args.root.join(config.app_dir())));
    print_field(
        "prod app",
        path_text(
            &args
                .root
                .join(config.out_dir())
                .join("server")
                .join(config.app_dir()),
        ),
    );
    println!();
    build(BuildArgs {
        root: args.root.clone(),
        target: Some(BuildTarget::Node),
    })?;

    let dev_manifest = discover_project_routes(&args.root, &config)?;
    let prod_manifest = discover_routes(
        DiscoverOptions::new(
            args.root
                .join(config.out_dir())
                .join("server")
                .join(config.app_dir()),
        )
        .with_rendering_defaults(
            config.rendering.default_strategy,
            config.rendering.default_revalidate,
        ),
    )?;
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

    failures.extend(smoke_render_parity(
        &dev_server_config(
            &ServerArgs {
                root: args.root.clone(),
                host: None,
                port: None,
            },
            &config,
        ),
        &production_server_config(
            &ServerArgs {
                root: args.root.clone(),
                host: None,
                port: None,
            },
            &config,
        ),
        &dev_manifest,
    ));

    if failures.is_empty() {
        println!(
            "\n{} Parity passed for {} routes in {}\n",
            success(),
            accent(dev_routes.len().to_string()),
            accent(format_duration(started.elapsed()))
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

fn smoke_render_parity(
    dev_config: &ServerConfig,
    prod_config: &ServerConfig,
    manifest: &RouteManifest,
) -> Vec<String> {
    let mut failures = Vec::new();

    for route in manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
    {
        let request_path = parity_smoke_path(&route.path);

        match render_request(dev_config, &request_path, "GET") {
            Ok(response) if !response.status().is_server_error() => {
                println!("{} Page {} dev render ok", success(), route.path);
            }
            Ok(response) => failures.push(format!(
                "Page {} dev runtime render returned {} for {}",
                route.path,
                response.status(),
                request_path
            )),
            Err(error) => failures.push(format!(
                "Page {} dev runtime render failed for {}: {error}",
                route.path, request_path
            )),
        }

        match render_request(prod_config, &request_path, "GET") {
            Ok(response) if !response.status().is_server_error() => {
                println!("{} Page {} prod render ok", success(), route.path);
            }
            Ok(response) => failures.push(format!(
                "Page {} prod runtime render returned {} for {}",
                route.path,
                response.status(),
                request_path
            )),
            Err(error) => failures.push(format!(
                "Page {} prod runtime render failed for {}: {error}",
                route.path, request_path
            )),
        }
    }

    failures
}

fn parity_smoke_path(route_path: &str) -> String {
    if route_path == "/" {
        return "/".to_string();
    }

    let segments = route_path
        .trim_start_matches('/')
        .split('/')
        .filter_map(|segment| {
            if segment.starts_with('*') && segment.ends_with('?') {
                None
            } else if segment.starts_with('*') {
                Some("smoke/path")
            } else if segment.starts_with(':') {
                Some("smoke")
            } else {
                Some(segment)
            }
        })
        .collect::<Vec<_>>();

    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
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

fn copy_style_sources(root: &Path, server_dir: &Path, files: &[PathBuf]) -> anyhow::Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for file in files {
        let file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        let Ok(relative) = file.strip_prefix(&root) else {
            continue;
        };
        if relative.starts_with("node_modules") {
            continue;
        }
        let target = server_dir.join(relative);
        if target == file {
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(file, target)?;
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

fn count_files(path: &Path) -> usize {
    if !path.exists() {
        return 0;
    }

    WalkDir::new(path)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count()
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

fn print_route_row(
    kind: &str,
    styled_kind: String,
    path: &str,
    styled_path: String,
    file: &str,
    styled_file: String,
    id: String,
) {
    println!(
        "  {}{} {}{} {}{} {}",
        styled_kind,
        spaces(10, kind.len()),
        styled_path,
        spaces(24, path.len()),
        styled_file,
        spaces(32, file.len()),
        id
    );
}

fn print_benchmark_table(
    samples: usize,
    results: &[BenchmarkResult],
    root: &Path,
    app_dir: &Path,
    elapsed: Duration,
) {
    print_tui_header(format!("Benchmark ({samples} sample(s))"));
    print_field("root", path_text(root));
    print_field("app dir", path_text(app_dir));
    print_field("scenarios", accent(results.len().to_string()));
    print_field("duration", accent(format_duration(elapsed)));
    println!();

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

fn print_tui_header(title: impl AsRef<str>) {
    println!("\n{}", heading(tui_header_title(title)));
    println!();
    print_field("time", accent(current_timestamp()));
}

fn tui_header_title(title: impl AsRef<str>) -> String {
    format!("🦊 Ruvyxa {}", title.as_ref())
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

fn current_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{:.2}s", duration.as_secs_f64())
    } else {
        format!("{:.0}ms", duration.as_secs_f64() * 1000.0)
    }
}

fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;

    if bytes < KIB as usize {
        return format!("{bytes} B");
    }

    let kibibytes = bytes as f64 / KIB;
    if bytes < MIB as usize {
        return if kibibytes < 10.0 {
            format!("{kibibytes:.1} kB")
        } else {
            format!("{kibibytes:.0} kB")
        };
    }

    let mebibytes = bytes as f64 / MIB;
    if mebibytes < 10.0 {
        format!("{mebibytes:.1} MB")
    } else {
        format!("{mebibytes:.0} MB")
    }
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

fn display_path_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
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

fn local_binary_upwards(root: &Path, binary: &str) -> Option<PathBuf> {
    let binary = if cfg!(windows) {
        format!("{binary}.cmd")
    } else {
        binary.to_string()
    };
    let mut current = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    loop {
        let candidate = current.join("node_modules").join(".bin").join(&binary);
        if candidate.is_file() {
            return Some(candidate);
        }

        if !current.pop() {
            return None;
        }
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
            if let Some(previous) = seen.insert(name.clone(), version.clone())
                && previous != version
            {
                duplicates.push(format!("{name} ({previous}, {version})"));
            }
        }
    }

    duplicates.sort();
    duplicates
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use serde_json::json;

    use super::*;

    #[test]
    fn config_renderer_invalid_output_reports_empty_stdout_and_stderr() {
        let error = parse_config_renderer_output(
            Path::new("."),
            b"",
            b"SyntaxError: Unexpected token",
            "exit status: 1",
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("config renderer returned invalid output for ."));
        assert!(error.contains("status: exit status: 1"));
        assert!(error.contains("stdout:\n(empty)"));
        assert!(error.contains("stderr:\nSyntaxError: Unexpected token"));
    }

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
    fn caps_build_parallelism_to_available_work() {
        assert_eq!(build_parallelism(Some(0), 4), 1);
        assert_eq!(build_parallelism(Some(3), 1), 1);
        assert_eq!(build_parallelism(Some(3), 5), 3);
        assert_eq!(build_parallelism(Some(usize::MAX), 2), 2);
    }

    #[test]
    fn caps_default_prerender_parallelism_to_limit_and_available_work() {
        assert_eq!(prerender_parallelism(None, 1), 1);
        assert!(prerender_parallelism(None, 10) <= MAX_PRERENDER_PARALLELISM);
        assert_eq!(prerender_parallelism(Some(3), 2), 2);
        assert_eq!(prerender_parallelism(Some(3), 10), 2);
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
        assert_eq!(content_hash("console.log('a')").len(), 64);
        assert_eq!(ASSET_HASH_ALGORITHM, "blake3-256");
        assert_eq!(content_hash("metadata-check").len() * 4, 256);
    }

    #[test]
    fn dev_config_respects_overlay_and_trace_flags() {
        let args = ServerArgs {
            root: PathBuf::from("."),
            host: None,
            port: None,
        };
        let enabled: ProjectConfig = serde_json::from_value(json!({
            "debug": { "overlay": true, "traces": true }
        }))
        .unwrap();
        let disabled: ProjectConfig = serde_json::from_value(json!({
            "debug": { "overlay": false, "traces": false }
        }))
        .unwrap();

        let enabled = dev_server_config(&args, &enabled);
        let disabled = dev_server_config(&args, &disabled);
        assert!(enabled.error_overlay);
        assert!(enabled.debug_traces);
        assert!(!disabled.error_overlay);
        assert!(!disabled.debug_traces);
    }

    #[test]
    fn server_configs_apply_action_security_options() {
        let args = ServerArgs {
            root: PathBuf::from("."),
            host: None,
            port: None,
        };
        let config: ProjectConfig = serde_json::from_value(json!({
            "security": {
                "actionLimit": 8192,
                "apiLimit": 16384,
                "actionRateLimit": { "max": 240, "window": 30 },
                "sameOrigin": false,
                "fetchMeta": false,
                "headers": false
            }
        }))
        .unwrap();

        for server in [
            dev_server_config(&args, &config),
            production_server_config(&args, &config),
        ] {
            assert_eq!(server.action_body_limit_bytes, 8192);
            assert_eq!(server.api_body_limit_bytes, 16384);
            assert_eq!(server.action_rate_limit_max, 240);
            assert_eq!(server.action_rate_limit_window, Duration::from_secs(30));
            assert!(!server.same_origin_actions);
            assert!(!server.fetch_metadata_actions);
            assert!(!server.security_headers);
        }
    }

    #[test]
    fn rejects_unknown_rust_config_fields() {
        let error = serde_json::from_value::<ProjectConfig>(json!({
            "debug": { "overlay": true, "unsupported": true }
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `unsupported`"));

        let error = serde_json::from_value::<ProjectConfig>(json!({
            "unsupportedTopLevel": true
        }))
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown field `unsupportedTopLevel`")
        );
    }

    #[test]
    fn rejects_zero_security_limits() {
        let config: ProjectConfig = serde_json::from_value(json!({
            "security": {
                "apiLimit": 0,
                "actionRateLimit": { "max": 0, "window": 0 }
            }
        }))
        .unwrap();

        let error = config.validate_paths().unwrap_err();
        assert!(error.to_string().contains("security.apiLimit"));
    }

    #[test]
    fn parses_native_bundler_build_options() {
        assert!(matches!(
            parse_jsx_runtime(Some("automatic")).unwrap(),
            ruvyxa_bundler::JsxRuntime::Automatic
        ));
        assert!(matches!(
            parse_es_target(Some("esnext")).unwrap(),
            ruvyxa_bundler::EsTarget::EsNext
        ));
        assert!(matches!(
            parse_split_strategy(Some("route")).unwrap(),
            ruvyxa_bundler::SplitStrategy::Route
        ));
        assert!(matches!(
            parse_split_strategy(Some("manual")).unwrap(),
            ruvyxa_bundler::SplitStrategy::Single
        ));

        let config: BuildConfigOptions = serde_json::from_value(json!({
            "treeShake": false,
            "manifest": true,
            "warm": false
        }))
        .unwrap();
        assert_eq!(config.tree_shaking, Some(false));
        assert_eq!(config.emit_chunk_manifest, Some(true));
        assert_eq!(config.prebundle_dependencies, Some(false));
    }

    #[test]
    fn parses_js_build_plugin_metadata() {
        let config: ProjectConfig = serde_json::from_value(json!({
            "plugins": [
                {
                    "name": "banner",
                    "enforce": "pre",
                    "resolveId": true,
                    "transform": true
                }
            ]
        }))
        .unwrap();

        assert_eq!(config.plugins.len(), 1);
        assert_eq!(config.plugins[0].name, "banner");
        assert_eq!(config.plugins[0].enforce.as_deref(), Some("pre"));
        assert!(config.plugins[0].resolve_id);
        assert!(config.plugins[0].transform);

        let manifest = build_plugin_manifest(&config.plugins);
        assert_eq!(manifest[0]["name"], "banner");
        assert_eq!(manifest[0]["resolveId"], true);
    }

    #[test]
    fn parses_global_rendering_defaults() {
        let config: ProjectConfig = serde_json::from_value(json!({
            "render": {
                "strategy": "isr",
                "revalidate": 90
            }
        }))
        .unwrap();

        assert_eq!(config.rendering.default_strategy, Some(RenderStrategy::Isr));
        assert_eq!(config.rendering.default_revalidate, Some(90));
    }

    #[test]
    fn resolves_shared_build_cache_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let shared = temp.path().join("shared-cache");

        assert_eq!(
            resolve_build_cache_dir(&root, Some(".cache/build"), None),
            root.join(".cache/build")
        );
        assert_eq!(
            resolve_build_cache_dir(
                &root,
                Some("ignored"),
                Some(shared.clone().into_os_string())
            ),
            shared
        );
        assert_eq!(
            resolve_build_cache_dir(&root, None, None),
            root.join(".ruvyxa/cache/bundler")
        );
    }

    #[test]
    fn rejects_invalid_native_bundler_build_options() {
        assert!(parse_jsx_runtime(Some("runtime-x")).is_err());
        assert!(parse_es_target(Some("es5")).is_err());
        assert!(parse_split_strategy(Some("vendor")).is_err());
    }

    #[test]
    fn emit_client_bundles_writes_chunk_manifest_when_enabled() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        let client_dir = root.join(".ruvyxa").join("client");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            app.join("page.tsx"),
            "export default function Page() { return <main>Home</main>; }",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let build = BuildConfigOptions {
            minify: Some(false),
            sourcemap: Some(false),
            tree_shaking: Some(true),
            split_strategy: Some("route".to_string()),
            parallelism: Some(1),
            jsx_runtime: Some("classic".to_string()),
            es_target: Some("es2022".to_string()),
            emit_chunk_manifest: Some(true),
            prebundle_dependencies: Some(true),
        };

        let client_manifest = emit_client_bundles(
            root,
            &app,
            &manifest,
            &client_dir,
            &build,
            &[],
            NativeBuildCache {
                dependency_hash: "no-config",
                directory: &root.join(".ruvyxa/cache/bundler"),
            },
        )
        .unwrap();

        assert!(client_dir.join("chunk-manifest.json").is_file());
        assert_eq!(client_manifest["emitChunkManifest"], true);
        assert!(client_manifest["moduleCount"].as_u64().unwrap() > 0);
        assert!(client_manifest["routes"][0]["chunkManifest"].is_object());
    }

    #[test]
    fn shared_route_chunks_are_emitted_for_modules_used_by_multiple_routes() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join("client");
        std::fs::create_dir_all(&client_dir).unwrap();

        let route_manifests = vec![
            serde_json::json!({
                "route": "/",
                "modules": ["/app/page.tsx", "/app/shared.ts"]
            }),
            serde_json::json!({
                "route": "/about",
                "modules": ["/app/about/page.tsx", "/app/shared.ts"]
            }),
        ];

        let chunks = emit_shared_route_chunks(&client_dir, &route_manifests).unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].modules.contains(&"/app/shared.ts".to_string()));
        assert!(client_dir.join(&chunks[0].file_name).is_file());

        let manifest = shared_route_chunk_manifest(&chunks[0]);
        assert_eq!(manifest["routes"].as_array().unwrap().len(), 2);
        assert!(
            manifest["src"]
                .as_str()
                .unwrap()
                .starts_with("/__ruvyxa/client/shared.")
        );
    }

    #[test]
    fn client_manifest_attaches_shared_chunks_to_affected_routes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        let client_dir = root.join("client");
        std::fs::create_dir_all(app.join("about")).unwrap();
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(app.join("shared.ts"), "export const label = 'shared'").unwrap();
        std::fs::write(
            app.join("page.tsx"),
            "import { label } from './shared'; export default function Page() { return <main>{label}</main> }",
        )
        .unwrap();
        std::fs::write(
            app.join("about/page.tsx"),
            "import { label } from '../shared'; export default function About() { return <main>{label}</main> }",
        )
        .unwrap();
        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let build = BuildConfigOptions {
            minify: Some(false),
            split_strategy: Some("route".to_string()),
            emit_chunk_manifest: Some(true),
            ..BuildConfigOptions::default()
        };

        let client_manifest = emit_client_bundles(
            root,
            &app,
            &manifest,
            &client_dir,
            &build,
            &[],
            NativeBuildCache {
                dependency_hash: "no-config",
                directory: &root.join(".ruvyxa/cache/bundler"),
            },
        )
        .unwrap();

        for route in client_manifest["routes"].as_array().unwrap() {
            assert_eq!(route["sharedChunks"].as_array().unwrap().len(), 1);
            assert!(
                route["sharedChunks"][0]["src"]
                    .as_str()
                    .unwrap()
                    .starts_with("/__ruvyxa/client/shared.")
            );
        }
    }

    #[test]
    fn prerender_html_includes_hashed_hydration_and_preload_assets() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join("client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            client_dir.join("manifest.json"),
            r#"{"routes":[{"path":"/docs/:slug","src":"/__ruvyxa/client/docs.123.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.456.js"}]}]}"#,
        )
        .unwrap();

        let html = inject_prerender_client_assets(
            "<!doctype html><html><head><title>Docs</title></head><body><main>Guide</main></body></html>",
            &client_dir,
            "/docs/:slug",
            "/docs/start",
            &BTreeMap::from([("slug".to_string(), "start".to_string())]),
        );

        assert!(
            html.contains(r#"<link rel="modulepreload" href="/__ruvyxa/client/shared.456.js">"#)
        );
        assert!(
            html.contains(r#"<script type="module" src="/__ruvyxa/client/docs.123.js"></script>"#)
        );
        assert!(html.contains(r#"globalThis.__RUVYXA_REQUEST_PATH__ = "/docs/start""#));
        assert!(html.contains(r#"globalThis.__RUVYXA_ROUTE_PARAMS__ = {"slug":"start"}"#));
        assert!(html.find("modulepreload").unwrap() < html.find("</head>").unwrap());
        assert!(html.find("docs.123.js").unwrap() < html.find("</body>").unwrap());
    }

    #[test]
    fn prerender_html_includes_global_styles_in_the_document_head() {
        let html = inject_prerender_styles(
            "<!doctype html><html><head><title>Docs</title></head><body><main>Guide</main></body></html>",
            "body { color: rebeccapurple; }",
        );

        assert!(html.contains(r#"<style data-ruvyxa-css>body { color: rebeccapurple; }</style>"#));
        assert!(html.find("data-ruvyxa-css").unwrap() < html.find("</head>").unwrap());
        assert!(html.contains("<main>Guide</main>"));
    }

    #[test]
    fn native_client_build_applies_js_config_transform_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        let client_dir = root.join(".ruvyxa").join("client");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            app.join("page.tsx"),
            "export default function Page() { return <main>Before</main>; }",
        )
        .unwrap();
        std::fs::write(
            root.join("ruvyxa.config.ts"),
            r#"
import { config } from "ruvyxa/config"

export default config({
  build: {
    minify: false,
    map: true,
    manifest: true,
  },
  plugins: [
    {
      name: "replace-before",
      transform(code, id, ctx) {
        if (ctx.environment !== "client" || !id.endsWith("page.tsx")) return null
        return {
          code: code.replace("Before", "After"),
          map: {
            version: 3,
            sources: ["plugin-original.tsx"],
            sourcesContent: [code],
            names: [],
            mappings: "AAAA",
          },
        }
      },
    },
  ],
})
"#,
        )
        .unwrap();

        let config = load_project_config(root).unwrap();
        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let client_manifest = emit_client_bundles(
            root,
            &app,
            &manifest,
            &client_dir,
            &config.build,
            &config.plugins,
            NativeBuildCache {
                dependency_hash: &config.config_dependency_hash,
                directory: &build_cache_dir(root, &config.cache),
            },
        )
        .unwrap();
        let route_file = client_manifest["routes"][0]["file"].as_str().unwrap();
        let output = std::fs::read_to_string(client_dir.join(route_file)).unwrap();

        assert!(output.contains("After"), "{output}");
        assert!(!output.contains("Before"), "{output}");
        assert_eq!(client_manifest["plugins"][0]["name"], "replace-before");
        let source_map_file = client_manifest["routes"][0]["sourceMap"].as_str().unwrap();
        let source_map: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(client_dir.join(source_map_file)).unwrap(),
        )
        .unwrap();
        assert!(
            source_map["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|source| source.as_str() == Some("plugin-original.tsx"))
        );
    }

    #[test]
    fn imported_plugin_change_invalidates_compile_cache_without_clean() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        let client_dir = root.join(".ruvyxa").join("client");
        let plugin_file = root.join("build-plugin.ts");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            app.join("page.tsx"),
            "export default function Page() { return <main>Before</main>; }",
        )
        .unwrap();
        std::fs::write(
            root.join("ruvyxa.config.ts"),
            r#"
import { plugin } from "./build-plugin.js"
export default { build: { minify: false }, plugins: [plugin] }
"#,
        )
        .unwrap();

        let write_plugin = |replacement: &str| {
            std::fs::write(
                &plugin_file,
                format!(
                    r#"export const plugin = {{
  name: "replace-label",
  transform(code, id) {{
    if (!id.endsWith("page.tsx")) return null
    return {{ code: code.replace("Before", "{replacement}") }}
  }}
}}
"#
                ),
            )
            .unwrap();
        };

        write_plugin("FirstBuild");
        let first_config = load_project_config(root).unwrap();
        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let cache_dir = build_cache_dir(root, &first_config.cache);
        let first_manifest = emit_client_bundles(
            root,
            &app,
            &manifest,
            &client_dir,
            &first_config.build,
            &first_config.plugins,
            NativeBuildCache {
                dependency_hash: &first_config.config_dependency_hash,
                directory: &cache_dir,
            },
        )
        .unwrap();
        let first_file = first_manifest["routes"][0]["file"].as_str().unwrap();
        let first_output = std::fs::read_to_string(client_dir.join(first_file)).unwrap();

        write_plugin("SecondRun");
        let second_config = load_project_config(root).unwrap();
        assert_ne!(
            first_config.config_dependency_hash,
            second_config.config_dependency_hash
        );
        let second_manifest = emit_client_bundles(
            root,
            &app,
            &manifest,
            &client_dir,
            &second_config.build,
            &second_config.plugins,
            NativeBuildCache {
                dependency_hash: &second_config.config_dependency_hash,
                directory: &cache_dir,
            },
        )
        .unwrap();
        let second_file = second_manifest["routes"][0]["file"].as_str().unwrap();
        let second_output = std::fs::read_to_string(client_dir.join(second_file)).unwrap();

        assert!(first_output.contains("FirstBuild"), "{first_output}");
        assert!(second_output.contains("SecondRun"), "{second_output}");
        assert!(!second_output.contains("FirstBuild"), "{second_output}");
    }

    #[test]
    fn js_config_plugin_bridge_reuses_worker_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(
            root.join("ruvyxa.config.mjs"),
            r#"
let calls = 0
export default {
  plugins: [{
    name: "counter",
    transform(code) {
      calls += 1
      return {
        code: `${code}\nexport const pluginCall = ${calls}`,
        map: {
          version: 3,
          sources: ["counter-input.ts"],
          sourcesContent: [code],
          names: [],
          mappings: "AAAA",
        },
      }
    },
  }],
}
"#,
        )
        .unwrap();

        let runner = find_runtime_script(root, "plugin-runner.mjs").unwrap();
        let worker = JsPluginWorker::spawn(&runner, root).unwrap();
        let bridge = JsConfigPluginBridge {
            project_root: root.to_path_buf(),
            worker: Arc::new(Mutex::new(worker)),
            has_resolve_id: false,
            has_transform: true,
        };
        let context = ruvyxa_bundler::plugin::PluginContext {
            project_root: root.to_path_buf(),
            importer: None,
            target: ruvyxa_bundler::BundleTarget::Client,
        };

        let first = ruvyxa_bundler::plugin::NativeBundlerPlugin::transform(
            &bridge,
            "export const value = 1",
            &root.join("first.ts"),
            &context,
        )
        .unwrap()
        .unwrap();
        let second = ruvyxa_bundler::plugin::NativeBundlerPlugin::transform(
            &bridge,
            "export const value = 2",
            &root.join("second.ts"),
            &context,
        )
        .unwrap()
        .unwrap();

        assert!(first.code.contains("pluginCall = 1"));
        assert!(second.code.contains("pluginCall = 2"));
        assert!(second.map.unwrap().contains("counter-input.ts"));
    }

    #[test]
    fn top_level_help_uses_framework_name_and_command_descriptions() {
        let help = Cli::command().render_long_help().to_string();

        assert!(help.contains("Usage: Ruvyxa <COMMAND>"));
        assert!(!help.contains("Ruvyxa Framework"));
        assert!(!help.contains("+==============================================================+"));
        assert!(!help.contains("build  |  validate  |  serve"));
        assert!(!help.contains("Rust-powered full-stack TypeScript framework"));
        assert!(!help.contains("ruvyxa.exe"));
        assert!(help.contains("dev          Run the development server with hot reload"));
        assert!(help.contains("build        Build the application for production output"));
        assert!(help.contains("check        Run app-level production readiness checks"));
        assert!(help.contains("test:parity  Compare dev/prod routes and smoke-render page routes"));
    }

    #[test]
    fn tui_headers_use_the_shared_fox_branding() {
        assert_eq!(tui_header_title("Build"), "🦊 Ruvyxa Build");
        assert_eq!(tui_header_title("Check"), "🦊 Ruvyxa Check");
        assert_eq!(
            tui_header_title("Benchmark (3 sample(s))"),
            "🦊 Ruvyxa Benchmark (3 sample(s))"
        );
    }

    #[test]
    fn config_paths_must_stay_project_relative() {
        assert!(validate_project_relative_path("outDir", ".ruvyxa").is_ok());
        assert!(validate_project_relative_path("appDir", "src/app").is_ok());
        assert!(validate_project_relative_path("css.entries", "styles/theme.css").is_ok());
        assert!(validate_project_relative_path("outDir", "../outside").is_err());
        assert!(validate_project_relative_path("css.entries", "../outside.css").is_err());
        assert!(validate_project_relative_path("outDir", "/tmp/out").is_err());
        assert!(validate_project_relative_path("appDir", "").is_err());
    }

    #[test]
    fn copies_external_style_sources_into_server_output() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let source = root.join("styles/theme.css");
        let server = root.join("output/server");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, ":root { color-scheme: dark; }").unwrap();

        copy_style_sources(root, &server, std::slice::from_ref(&source)).unwrap();

        assert_eq!(
            std::fs::read_to_string(server.join("styles/theme.css")).unwrap(),
            ":root { color-scheme: dark; }"
        );
    }

    #[test]
    fn parses_top_level_commands_case_insensitively() {
        let cli = Cli::try_parse_from(normalized_cli_args(os_args([
            "Ruvyxa",
            "BUILD",
            "--root",
            "examples/basic-app",
        ])))
        .unwrap();

        assert!(matches!(cli.command, Command::Build(_)));
    }

    #[test]
    fn parses_check_command_case_insensitively() {
        let cli = Cli::try_parse_from(normalized_cli_args(os_args([
            "Ruvyxa",
            "CHECK",
            "--root",
            "examples/basic-app",
        ])))
        .unwrap();

        assert!(matches!(cli.command, Command::Check(_)));
    }

    #[test]
    fn parses_value_enums_case_insensitively() {
        let cli = Cli::try_parse_from(normalized_cli_args(os_args([
            "Ruvyxa",
            "BUILD",
            "--target",
            "EDGE",
            "--root",
            "examples/basic-app",
        ])))
        .unwrap();

        let Command::Build(args) = cli.command else {
            panic!("expected build command");
        };
        assert!(matches!(args.target, Some(BuildTarget::Edge)));
    }

    #[test]
    fn parses_long_options_case_insensitively() {
        let cli = Cli::try_parse_from(normalized_cli_args(os_args([
            "Ruvyxa",
            "BUILD",
            "--TARGET=EDGE",
            "--ROOT",
            "examples/basic-app",
        ])))
        .unwrap();

        let Command::Build(args) = cli.command else {
            panic!("expected build command");
        };
        assert!(matches!(args.target, Some(BuildTarget::Edge)));
        assert_eq!(args.root, PathBuf::from("examples/basic-app"));
    }

    #[test]
    fn parses_command_aliases_case_insensitively() {
        let cli = Cli::try_parse_from(normalized_cli_args(os_args([
            "Ruvyxa",
            "PARITY",
            "--root",
            "examples/basic-app",
        ])))
        .unwrap();

        assert!(matches!(cli.command, Command::TestParity(_)));
    }

    #[test]
    fn uses_config_runtime_when_the_cli_target_is_omitted() {
        let config = ProjectConfig {
            runtime: Some(BuildTarget::Static),
            ..ProjectConfig::default()
        };

        assert_eq!(config.build_target(None), BuildTarget::Static);
        assert_eq!(
            config.build_target(Some(BuildTarget::Edge)),
            BuildTarget::Edge
        );
        assert_eq!(
            ProjectConfig::default().build_target(None),
            BuildTarget::Node
        );
    }

    #[test]
    fn normalizes_help_target_command_case() {
        let args = normalized_cli_args(os_args(["Ruvyxa", "HELP", "BUILD"]));

        assert_eq!(args[1], OsString::from("help"));
        assert_eq!(args[2], OsString::from("build"));
    }

    #[test]
    fn normalizes_help_option_case() {
        let args = normalized_cli_args(os_args(["Ruvyxa", "--HELP"]));

        assert_eq!(args[1], OsString::from("--help"));
    }

    #[test]
    fn builds_smoke_paths_for_dynamic_routes() {
        assert_eq!(parity_smoke_path("/"), "/");
        assert_eq!(parity_smoke_path("/blog/:slug"), "/blog/smoke");
        assert_eq!(parity_smoke_path("/docs/*path"), "/docs/smoke/path");
        assert_eq!(parity_smoke_path("/shop/*category?"), "/shop");
    }

    #[test]
    fn staged_build_commit_replaces_outputs_and_preserves_cache_directory() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join(".ruvyxa");
        let cache_dir = out_dir.join("cache").join("bundler");
        let old_server_dir = out_dir.join("server");
        let old_assets_dir = out_dir.join("assets");
        let staging_dir = create_build_staging_dir(&out_dir).unwrap();
        let new_server_dir = staging_dir.join("server");
        let new_client_dir = staging_dir.join("client");

        fs::create_dir_all(&cache_dir).unwrap();
        fs::create_dir_all(&old_server_dir).unwrap();
        fs::create_dir_all(&old_assets_dir).unwrap();
        fs::create_dir_all(&new_server_dir).unwrap();
        fs::create_dir_all(&new_client_dir).unwrap();
        fs::write(cache_dir.join("cached.js"), "compiled").unwrap();
        fs::write(old_server_dir.join("old.js"), "old").unwrap();
        fs::write(old_assets_dir.join("old.txt"), "old").unwrap();
        fs::write(out_dir.join("manifest.json"), "{}").unwrap();
        fs::write(out_dir.join("build.json"), "{}").unwrap();
        fs::write(new_server_dir.join("new.js"), "new").unwrap();
        fs::write(new_client_dir.join("new.js"), "new").unwrap();
        fs::write(staging_dir.join("manifest.json"), "{\"routes\":[]}").unwrap();
        fs::write(staging_dir.join("build.json"), "{\"framework\":\"Ruvyxa\"}").unwrap();

        commit_staged_build_outputs(&staging_dir, &out_dir).unwrap();

        assert!(cache_dir.join("cached.js").exists());
        assert!(out_dir.join("server/new.js").exists());
        assert!(out_dir.join("client/new.js").exists());
        assert!(!out_dir.join("server/old.js").exists());
        assert!(!out_dir.join("assets").exists());
        assert!(out_dir.join("manifest.json").exists());
        assert!(out_dir.join("build.json").exists());
        assert!(!staging_dir.exists());
        assert!(!has_temp_build_dir(&out_dir, ".build-rollback"));
    }

    #[test]
    fn staged_build_commit_removes_old_output_when_staging_omits_it() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join(".ruvyxa");
        let staging_dir = create_build_staging_dir(&out_dir).unwrap();

        fs::create_dir_all(out_dir.join("assets")).unwrap();
        fs::write(out_dir.join("assets/old.txt"), "old").unwrap();
        fs::write(staging_dir.join("manifest.json"), "{}").unwrap();
        fs::write(staging_dir.join("build.json"), "{}").unwrap();

        commit_staged_build_outputs(&staging_dir, &out_dir).unwrap();

        assert!(!out_dir.join("assets").exists());
        assert!(out_dir.join("manifest.json").exists());
    }

    #[test]
    fn static_route_path_preserves_page_params_and_rejects_traversal() {
        let params = BTreeMap::from([("slug".to_string(), "hello-world".to_string())]);
        assert_eq!(
            static_route_path("/blog/:slug", &params).unwrap(),
            "/blog/hello-world"
        );

        let unsafe_params = BTreeMap::from([("slug".to_string(), "../manifest.json".to_string())]);
        assert!(static_route_path("/blog/:slug", &unsafe_params).is_err());
    }

    #[test]
    fn static_route_path_allows_valid_catch_all_segments() {
        let params = BTreeMap::from([("path".to_string(), "guides/routing".to_string())]);
        assert_eq!(
            static_route_path("/docs/*path", &params).unwrap(),
            "/docs/guides/routing"
        );
    }

    fn os_args<const N: usize>(args: [&str; N]) -> Vec<OsString> {
        args.into_iter().map(OsString::from).collect()
    }

    fn has_temp_build_dir(out_dir: &Path, prefix: &str) -> bool {
        fs::read_dir(out_dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .any(|entry| {
                entry.file_type().is_ok_and(|file_type| file_type.is_dir())
                    && entry.file_name().to_string_lossy().starts_with(prefix)
            })
    }
}
