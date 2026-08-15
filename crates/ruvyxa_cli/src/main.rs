//! The Ruvyxa CLI entry point.
//!
//! This file holds the command surface — the clap definitions for every command
//! and flag — and `main`, which normalizes arguments, applies the runtime
//! override, and dispatches. The work each command performs lives in a sibling
//! module.
//!
//! Nothing here should grow an implementation. When a command needs more than
//! dispatch, it belongs next to the other logic of its kind: `build` for the
//! build pipeline, `commands` for the reporting commands, `config` for
//! configuration loading and validation, `runtime_config` for turning args plus
//! config into a runnable server.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Context;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Parser, Subcommand, ValueEnum};
use ruvyxa_dev_server::serve;

mod add;
mod analyzer_html;
mod artifact_cache;
#[path = "bench.rs"]
mod benchmark;
mod build;
mod build_output;
mod cli_args;
mod client_bundle;
mod commands;
mod config;
mod environment;
mod host_resources;
mod image_optimizer;
mod image_usage;
mod plugins;
mod prerender;
mod route_types;
mod runtime_config;
mod site_discovery;
mod ui;

// Re-exported crate-wide, not merely imported here: the modules below refer to
// each other through `crate::*`, and a plain `use` would keep those names
// private to this file.
pub(crate) use add::*;
pub(crate) use analyzer_html::*;
pub(crate) use artifact_cache::*;
pub(crate) use benchmark::*;
pub(crate) use build::*;
pub(crate) use build_output::*;
pub(crate) use cli_args::*;
pub(crate) use client_bundle::*;
pub(crate) use commands::*;
pub(crate) use config::*;
pub(crate) use environment::*;
pub(crate) use image_optimizer::{
    ImageOptimizationOptions, ImageOptimizationReport, optimize_public_images,
};
pub(crate) use image_usage::scan_raw_image_usage;
pub(crate) use plugins::*;
pub(crate) use prerender::*;
pub(crate) use route_types::*;
pub(crate) use runtime_config::*;
pub(crate) use site_discovery::{SiteConfigOptions, resolve_site_url, write_discovery_files};
pub(crate) use ui::*;

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

/// `--help` styling, mapped role by role onto the palette in
/// [`ruvyxa_tui::theme`] so `ruvyxa --help` and `ruvyxa build` are recognisably
/// the same tool. clap takes `AnsiColor` values rather than escape codes, which
/// is why the palette is restated here instead of imported; each line names the
/// theme function it mirrors, and the two must be changed together.
fn cli_styles() -> Styles {
    Styles::styled()
        // heading()
        .header(AnsiColor::Magenta.on_default().effects(Effects::BOLD))
        // brand()
        .usage(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
        // accent()
        .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
        // label()
        .placeholder(AnsiColor::BrightBlack.on_default())
        // ok_text()
        .valid(AnsiColor::Green.on_default())
        // alert_text()
        .invalid(AnsiColor::Red.on_default().effects(Effects::BOLD))
        .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
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
    Routes(RoutesArgs),
    #[command(about = "Validate routes, imports, and server/client boundaries")]
    Analyze(AnalyzeArgs),
    #[command(about = "Scaffold framework-native forms, data tables, or authentication flows")]
    Adds(AddArgs),
    #[command(about = "Check project setup, dependencies, and runtime compatibility")]
    Doctor(DoctorArgs),
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
    #[command(about = "Create a publishable plugin package")]
    Plugin(PluginArgs),
}

#[derive(Debug, Clone, Parser)]
struct ProjectArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// JavaScript runtime to use (node, bun, or deno); overrides RUVYXA_RUNTIME
    /// and config.runtime.
    #[arg(long, value_enum, ignore_case = true)]
    runtime: Option<CliRuntime>,
}

#[derive(Debug, Clone, Parser)]
struct RoutesArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// JavaScript runtime used to evaluate project configuration.
    #[arg(long, value_enum, ignore_case = true)]
    runtime: Option<CliRuntime>,

    /// Emit the route manifest as JSON for editor and automation consumers.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Parser)]
struct AnalyzeArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// JavaScript runtime used to evaluate project configuration.
    #[arg(long, value_enum, ignore_case = true)]
    runtime: Option<CliRuntime>,

    /// Report format. Auto keeps the existing terminal/pipe behavior.
    #[arg(long, value_enum, ignore_case = true, default_value_t = AnalyzeFormat::Auto)]
    format: AnalyzeFormat,

    /// Write the report to a file instead of standard output.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Build a self-contained interactive HTML bundle report.
    #[arg(long)]
    html: bool,
}

#[derive(Debug, Clone, Parser)]
struct AddArgs {
    /// One or more additive scaffolds to create.
    #[arg(required = true, value_enum, ignore_case = true)]
    templates: Vec<AddTemplate>,

    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// JavaScript runtime used to evaluate project configuration.
    #[arg(long, value_enum, ignore_case = true)]
    runtime: Option<CliRuntime>,

    /// Replace scaffold-owned files that already exist.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AddTemplate {
    Form,
    DataTable,
    Auth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AnalyzeFormat {
    Auto,
    Human,
    Json,
    Sarif,
    Html,
}

#[derive(Debug, Clone, Parser)]
struct DoctorArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// Production target to evaluate; overrides config.runtime.
    #[arg(long, value_enum, ignore_case = true)]
    target: Option<BuildTarget>,

    /// Inspect a deploy adapter without materializing its artifacts.
    #[arg(long, value_parser = parse_adapter_name)]
    adapter: Option<String>,

    /// JavaScript runtime used to evaluate configuration and adapters.
    #[arg(long, value_enum, ignore_case = true)]
    runtime: Option<CliRuntime>,

    /// Emit the complete compatibility report as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
struct ServerArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    #[arg(long)]
    host: Option<String>,

    #[arg(long)]
    port: Option<u16>,

    /// JavaScript runtime to use (node, bun, or deno); overrides RUVYXA_RUNTIME
    /// and config.runtime.
    #[arg(long, value_enum, ignore_case = true)]
    runtime: Option<CliRuntime>,
}

#[derive(Debug, Parser)]
struct BuildArgs {
    #[arg(long, default_value = ".")]
    root: PathBuf,

    #[arg(long, value_enum, ignore_case = true)]
    target: Option<BuildTarget>,

    /// Deploy adapter to run without editing ruvyxa.config
    /// (node, bun, static, vercel, netlify, cloudflare, railway,
    /// render, firebase, aws, or any
    /// adapter package name such as @scope/ruvyxa-adapter-deno).
    #[arg(long, value_parser = parse_adapter_name)]
    adapter: Option<String>,

    /// JavaScript runtime to use (node, bun, or deno); overrides RUVYXA_RUNTIME
    /// and config.runtime.
    #[arg(long, value_enum, ignore_case = true)]
    runtime: Option<CliRuntime>,

    /// Build an API-only artifact: no client bundles, page CSS, prerendered
    /// pages, or discovery files. Requires the node or bun target, and fails
    /// when the project contains a page route.
    #[arg(long)]
    server_only: bool,
}

const KNOWN_ADAPTER_NAMES: [&str; 11] = [
    "node",
    "bun",
    "deno",
    "static",
    "vercel",
    "netlify",
    "cloudflare",
    "railway",
    "render",
    "firebase",
    "aws",
];

/// Hosting platforms that identify themselves through build-environment
/// variables. When no adapter is configured, the matching adapter is selected
/// automatically so a fresh project deploys with zero configuration.
const PLATFORM_ADAPTER_ENV: [(&str, &str); 6] = [
    ("VERCEL", "vercel"),
    ("NETLIFY", "netlify"),
    ("CF_PAGES", "cloudflare"),
    ("RAILWAY_PROJECT_ID", "railway"),
    ("RENDER", "render"),
    ("AWS_APP_ID", "aws"),
];

fn parse_adapter_name(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if KNOWN_ADAPTER_NAMES.contains(&normalized.as_str()) || is_npm_package_name(&normalized) {
        Ok(normalized)
    } else {
        Err(format!(
            "unknown adapter `{value}`; expected one of {}, or an adapter package name",
            KNOWN_ADAPTER_NAMES.join(", ")
        ))
    }
}

/// Accept anything shaped like an npm package name (optionally scoped) so
/// third-party adapters can be selected from the command line. The JS adapter
/// runner resolves the actual package and reports RUV2203 when missing.
fn is_npm_package_name(value: &str) -> bool {
    fn valid_part(part: &str) -> bool {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-._~".contains(c))
    }
    match value.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, name)) => valid_part(scope) && valid_part(name),
            None => false,
        },
        None => !value.contains('/') && valid_part(value),
    }
}

/// Detect the hosting platform from build-environment variables. Returns the
/// adapter name and the variable that selected it. `RUVYXA_ADAPTER` overrides
/// platform detection; empty, `0`, and `false` values are ignored.
fn detect_platform_adapter(env: impl Fn(&str) -> Option<String>) -> Option<(String, String)> {
    if let Some(value) = env("RUVYXA_ADAPTER") {
        let name = value.trim().to_ascii_lowercase();
        if !name.is_empty() && parse_adapter_name(&name).is_ok() {
            return Some((name, "RUVYXA_ADAPTER".to_string()));
        }
    }
    for (variable, adapter) in PLATFORM_ADAPTER_ENV {
        if let Some(value) = env(variable) {
            let value = value.trim();
            if !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false") {
                return Some((adapter.to_string(), variable.to_string()));
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum BuildTarget {
    Node,
    Bun,
    Deno,
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

    /// JavaScript runtime used by build samples (node, bun, or deno).
    #[arg(long, value_enum, ignore_case = true)]
    runtime: Option<CliRuntime>,

    #[arg(long)]
    json: bool,

    /// Measure isolated cold, warm, and leaf-edit production builds.
    ///
    /// Each sample runs in a disposable project copy with a private cache, so
    /// the benchmark never deletes or warms the application's real cache.
    #[arg(long)]
    baseline: bool,
}

#[derive(Debug, Parser)]
struct PluginArgs {
    #[command(subcommand)]
    command: PluginCommand,
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    #[command(about = "Create a publishable plugin package")]
    Create(PluginCreateArgs),
}

#[derive(Debug, Parser)]
struct PluginCreateArgs {
    name: String,

    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// Directory to scaffold the plugin package into, relative to --root.
    /// Defaults to `<name>`.
    #[arg(long)]
    dir: Option<PathBuf>,
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

    set_cli_runtime_override(command_runtime(&cli.command));

    match cli.command {
        Command::Dev(args) => {
            let config = load_project_config(&args.root)?;
            serve(dev_server_config(&args, &config)?)
                .await
                .context("dev server failed")?;
        }
        Command::Build(args) => build(args).await.context("build failed")?,
        Command::Check(args) => check(args).await.context("check failed")?,
        Command::Start(args) | Command::Preview(args) => {
            let config = load_project_config(&args.root)?;
            serve(production_server_config(&args, &config)?)
                .await
                .context("production server failed")?;
        }
        Command::Routes(args) => print_routes(args).context("route discovery failed")?,
        Command::Analyze(args) => analyze(args).context("analyze failed")?,
        Command::Adds(args) => scaffold_add(args).context("scaffold failed")?,
        Command::Doctor(args) => doctor(args).context("doctor failed")?,
        Command::Clean(args) => clean(args).context("clean failed")?,
        Command::Trace(args) => trace(args).context("trace failed")?,
        Command::Bench(args) => bench(args).await.context("benchmark failed")?,
        Command::TestParity(args) => test_parity(args).await.context("parity test failed")?,
        Command::Plugin(args) => plugin(args).context("plugin command failed")?,
    }

    Ok(())
}

#[cfg(test)]
mod tests;
