use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use ruvyxa_dev_server::{serve, ServerConfig};
use ruvyxa_graph::{discover_routes, write_manifest, DiscoverOptions};
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
    Doctor(ProjectArgs),
    Clean(ProjectArgs),
    Trace(TraceArgs),
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
        Command::Doctor(args) => doctor(args).context("doctor failed")?,
        Command::Clean(args) => clean(args).context("clean failed")?,
        Command::Trace(args) => trace(args).context("trace failed")?,
    }

    Ok(())
}

fn build(args: BuildArgs) -> anyhow::Result<()> {
    let app_dir = args.root.join("app");
    let out_dir = args.root.join(".ruvyxa");

    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)
            .with_context(|| format!("failed to clean {}", out_dir.display()))?;
    }

    let manifest = discover_routes(DiscoverOptions::new(&app_dir))?;
    copy_dir_all(&app_dir, &out_dir.join("app"))?;
    copy_public(&args.root, &out_dir)?;
    write_manifest(&manifest, &out_dir.join("manifest.json"))?;

    let build_info = serde_json::json!({
        "framework": "Ruvyxa",
        "version": env!("CARGO_PKG_VERSION"),
        "target": format!("{:?}", args.target).to_lowercase(),
        "routes": manifest.routes.len()
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

fn doctor(args: ProjectArgs) -> anyhow::Result<()> {
    let app_dir = args.root.join("app");
    let package_json = args.root.join("package.json");
    let tsconfig = args.root.join("tsconfig.json");

    println!("Ruvyxa doctor");
    println!("root: {}", args.root.display());
    println!("app directory: {}", exists_label(&app_dir));
    println!("package.json: {}", exists_label(&package_json));
    println!("tsconfig.json: {}", exists_label(&tsconfig));

    let manifest = discover_routes(DiscoverOptions::new(&app_dir))?;
    println!("routes: {}", manifest.routes.len());
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

fn exists_label(path: &Path) -> &'static str {
    if path.exists() {
        "ok"
    } else {
        "missing"
    }
}

fn copy_public(root: &Path, out_dir: &Path) -> anyhow::Result<()> {
    let public = root.join("public");
    if public.exists() {
        copy_dir_all(&public, &out_dir.join("public"))?;
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
