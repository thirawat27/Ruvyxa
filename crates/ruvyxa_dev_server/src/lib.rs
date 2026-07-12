use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::future::IntoFuture;
use std::io::{ErrorKind, IsTerminal};
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use chrono::Local;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use ruvyxa_graph::{
    DiscoverOptions, RenderStrategy, RouteEntry, RouteKind, RouteManifest, discover_routes,
};
use ruvyxa_middleware::{
    MiddlewareConfig, MiddlewareStack, PluginRequest, PluginResponse, WasmPluginRuntime,
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{info, warn};

mod worker_pool;
pub use worker_pool::NodeWorkerPool;
use worker_pool::RenderApiRequest;

mod router;
pub use router::RadixRouter;

mod render_cache;
pub use render_cache::RenderCache;

mod hmr_tracker;
pub use hmr_tracker::{HmrEventType, HmrTracker, HmrUpdate};

mod style;
pub use style::{StyleCollection, collect_css, collect_styles, minify_css};

const MAX_ACTION_BODY_BYTES: usize = 1024 * 1024;
const MAX_API_BODY_BYTES: usize = 10 * 1024 * 1024;
const ACTION_RATE_LIMIT_MAX: usize = 600;
const ACTION_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const MAX_TRACKED_ACTION_RATE_LIMIT_KEYS: usize = 10_000;
const PORT_FALLBACK_SCAN_LIMIT: u16 = 100;
const SERVER_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub root: PathBuf,
    pub app_dir: PathBuf,
    pub public_dir: PathBuf,
    pub client_dir: PathBuf,
    /// Directory containing pre-rendered HTML files from the build step.
    pub prerender_dir: PathBuf,
    pub host: String,
    pub port: u16,
    pub watch: bool,
    pub cache_route_manifest: bool,
    pub cache_css: bool,
    /// Additional project-relative global stylesheet files or directories.
    pub style_entries: Vec<PathBuf>,
    /// Precompile route modules and load their dependencies in dev workers.
    pub prebundle_dependencies: bool,
    /// Render actionable source-aware error overlays in development.
    pub error_overlay: bool,
    /// Expose runtime route traces from the development diagnostics endpoint.
    pub debug_traces: bool,
    /// Maximum accepted action request payload size.
    pub action_body_limit_bytes: usize,
    /// Maximum accepted API route request payload size.
    pub api_body_limit_bytes: usize,
    /// Maximum action requests per client/action in the configured window.
    pub action_rate_limit_max: usize,
    /// Window used by the action rate limiter.
    pub action_rate_limit_window: Duration,
    /// Reject action requests whose Origin does not match the request Host.
    pub same_origin_actions: bool,
    /// Reject action requests initiated from a cross-site browser context.
    pub fetch_metadata_actions: bool,
    /// Apply Ruvyxa's default security response headers.
    pub security_headers: bool,
    pub middleware: MiddlewareConfig,
    pub default_render_strategy: Option<RenderStrategy>,
    pub default_revalidate: Option<u64>,
}

impl ServerConfig {
    pub fn dev(root: impl Into<PathBuf>, host: impl Into<String>, port: u16) -> Self {
        let root = root.into();
        Self {
            app_dir: root.join("app"),
            public_dir: root.join("public"),
            client_dir: root.join(".ruvyxa/client"),
            prerender_dir: root.join(".ruvyxa/prerender"),
            root,
            host: host.into(),
            port,
            watch: true,
            cache_route_manifest: true,
            cache_css: true,
            style_entries: Vec::new(),
            prebundle_dependencies: true,
            error_overlay: true,
            debug_traces: false,
            action_body_limit_bytes: MAX_ACTION_BODY_BYTES,
            api_body_limit_bytes: MAX_API_BODY_BYTES,
            action_rate_limit_max: ACTION_RATE_LIMIT_MAX,
            action_rate_limit_window: ACTION_RATE_LIMIT_WINDOW,
            same_origin_actions: true,
            fetch_metadata_actions: true,
            security_headers: true,
            middleware: MiddlewareConfig::default(),
            default_render_strategy: None,
            default_revalidate: None,
        }
    }

    pub fn production(root: impl Into<PathBuf>, host: impl Into<String>, port: u16) -> Self {
        let root = root.into();
        Self {
            app_dir: root.join(".ruvyxa/server/app"),
            public_dir: root.join(".ruvyxa/assets"),
            client_dir: root.join(".ruvyxa/client"),
            prerender_dir: root.join(".ruvyxa/prerender"),
            root,
            host: host.into(),
            port,
            watch: false,
            cache_route_manifest: true,
            cache_css: true,
            style_entries: Vec::new(),
            prebundle_dependencies: false,
            error_overlay: false,
            debug_traces: false,
            action_body_limit_bytes: MAX_ACTION_BODY_BYTES,
            api_body_limit_bytes: MAX_API_BODY_BYTES,
            action_rate_limit_max: ACTION_RATE_LIMIT_MAX,
            action_rate_limit_window: ACTION_RATE_LIMIT_WINDOW,
            same_origin_actions: true,
            fetch_metadata_actions: true,
            security_headers: true,
            middleware: MiddlewareConfig::default(),
            default_render_strategy: None,
            default_revalidate: None,
        }
    }
}

#[derive(Clone)]
struct AppState {
    config: ServerConfig,
    reload_tx: broadcast::Sender<String>,
    runtime_cache: Arc<RuntimeCache>,
    action_limiter: Arc<Mutex<ActionRateLimiter>>,
    worker_pool: Arc<NodeWorkerPool>,
    render_cache: Arc<RenderCache>,
    hmr_tracker: Arc<HmrTracker>,
    plugin_runtime: Option<Arc<WasmPluginRuntime>>,
}

#[derive(Default)]
struct RuntimeCache {
    manifest: tokio::sync::RwLock<Option<RouteManifest>>,
    styles: tokio::sync::RwLock<Option<String>>,
    router: tokio::sync::RwLock<Option<RadixRouter>>,
}

impl RuntimeCache {
    fn with_manifest(manifest: RouteManifest) -> Self {
        let router = RadixRouter::compile(&manifest);
        Self {
            manifest: tokio::sync::RwLock::new(Some(manifest)),
            styles: tokio::sync::RwLock::new(None),
            router: tokio::sync::RwLock::new(Some(router)),
        }
    }

    async fn manifest(&self, config: &ServerConfig) -> Result<RouteManifest> {
        if !config.cache_route_manifest {
            return discover_routes(discover_options(config));
        }

        {
            let cached = self.manifest.read().await;
            if let Some(manifest) = cached.as_ref() {
                return Ok(manifest.clone());
            }
        }

        let manifest = discover_routes(discover_options(config))?;
        {
            let mut cached = self.manifest.write().await;
            *cached = Some(manifest.clone());
        }
        {
            let mut router_cache = self.router.write().await;
            *router_cache = Some(RadixRouter::compile(&manifest));
        }

        Ok(manifest)
    }

    async fn router(&self, config: &ServerConfig) -> Result<(RouteManifest, RadixRouter)> {
        let manifest = self.manifest(config).await?;
        let router_cache = self.router.read().await;
        let router = router_cache
            .as_ref()
            .cloned()
            .unwrap_or_else(|| RadixRouter::compile(&manifest));
        Ok((manifest, router))
    }

    async fn styles(&self, config: &ServerConfig) -> Result<String> {
        if !config.cache_css {
            let css = collect_styles(&config.root, &config.app_dir, &config.style_entries)?.css;
            return Ok(if config.watch {
                css
            } else {
                style::minify_css(&css)
            });
        }

        {
            let cached = self.styles.read().await;
            if let Some(styles) = cached.as_ref() {
                return Ok(styles.clone());
            }
        }

        let mut styles = collect_styles(&config.root, &config.app_dir, &config.style_entries)?.css;
        // Minify CSS in production mode to reduce inline style payload.
        if !config.watch {
            styles = style::minify_css(&styles);
        }
        {
            let mut cached = self.styles.write().await;
            *cached = Some(styles.clone());
        }
        Ok(styles)
    }

    fn invalidate(&self) {
        // Use blocking_write for sync context (file watcher callback)
        *self.manifest.blocking_write() = None;
        *self.styles.blocking_write() = None;
        *self.router.blocking_write() = None;
    }

    #[cfg(test)]
    async fn invalidate_async(&self) {
        *self.manifest.write().await = None;
        *self.styles.write().await = None;
        *self.router.write().await = None;
    }
}

fn discover_options(config: &ServerConfig) -> DiscoverOptions {
    DiscoverOptions::new(&config.app_dir)
        .with_rendering_defaults(config.default_render_strategy, config.default_revalidate)
}

pub async fn serve(config: ServerConfig) -> Result<()> {
    let startup_started = Instant::now();
    let manifest = discover_routes(discover_options(&config))?;
    info!(routes = manifest.routes.len(), "discovered routes");

    let (reload_tx, _) = broadcast::channel(64);
    let runtime_cache = Arc::new(RuntimeCache::with_manifest(manifest.clone()));

    let env = project_env(&config.root)?;
    let worker_pool = Arc::new(NodeWorkerPool::start(&config.root, env).await?);
    info!("Node worker pool ready");

    let warmup_routes = dependency_warmup_routes(&config, &manifest);
    if !warmup_routes.is_empty() {
        let warmup_pool = worker_pool.clone();
        let warmup_root = config.root.display().to_string();
        tokio::spawn(async move {
            let warmed = warmup_pool.warmup(&warmup_root, warmup_routes).await;
            info!(warmed, "dependency pre-bundling complete");
        });
    }

    let render_cache = Arc::new(if config.watch {
        RenderCache::default_dev()
    } else {
        RenderCache::default_production()
    });

    let watcher_pool = worker_pool.clone();
    let watcher_render_cache = render_cache.clone();
    let hmr_tracker = Arc::new(HmrTracker::new());
    hmr_tracker.populate_from_manifest(&manifest.routes);
    let middleware_stack = MiddlewareStack::new(config.middleware.clone());
    middleware_stack.validate().map_err(RuvyxaError::Message)?;
    let plugin_runtime = if config.middleware.plugins.is_empty() {
        None
    } else {
        Some(Arc::new(
            WasmPluginRuntime::new(&config.root, &config.middleware.plugins).await?,
        ))
    };
    let state = AppState {
        config: config.clone(),
        reload_tx,
        runtime_cache,
        action_limiter: Arc::new(Mutex::new(ActionRateLimiter::new(
            config.action_rate_limit_max,
            config.action_rate_limit_window,
        ))),
        worker_pool: worker_pool.clone(),
        render_cache,
        hmr_tracker,
        plugin_runtime,
    };

    let _watcher = if config.watch {
        Some(start_watcher(
            &config.root,
            &watch_paths(&config),
            state.reload_tx.clone(),
            state.runtime_cache.clone(),
            watcher_pool,
            watcher_render_cache,
            state.hmr_tracker.clone(),
        )?)
    } else {
        None
    };

    let app = Router::new()
        .route("/__ruvyxa/hmr", get(hmr_ws))
        .route("/__ruvyxa/client", get(client_bundle))
        .route(
            "/__ruvyxa/action",
            post(action_endpoint).layer(DefaultBodyLimit::max(config.action_body_limit_bytes)),
        )
        .route("/__ruvyxa/trace", get(trace_endpoint))
        .fallback(handle_request)
        .with_state(Arc::new(state));

    // Apply middleware stack from config (compression, CORS, timing, logging, custom headers)
    let app = middleware_stack.apply(app);
    let security_headers = config.security_headers;
    let app =
        app.layer(axum::middleware::map_response(
            move |response: Response| async move {
                finalize_security_headers(response, security_headers)
            },
        ));

    let address: SocketAddr = format!("{}:{}", config.host, config.port)
        .to_socket_addrs()
        .map_err(|error| RuvyxaError::Message(format!("Invalid server address: {error}")))?
        .next()
        .ok_or_else(|| RuvyxaError::Message("Server address did not resolve".to_string()))?;
    let (listener, bound_address) = bind_listener(&config, address).await?;

    info!("Ruvyxa server listening on http://{bound_address}");
    print_server_ready(&config, &manifest, bound_address, startup_started.elapsed());
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        })
        .into_future();
    tokio::pin!(server);

    let server_result = tokio::select! {
        result = &mut server => result,
        signal = shutdown_signal() => {
            info!(signal, "shutting down Ruvyxa server");
            let _ = shutdown_tx.send(true);
            match tokio::time::timeout(SERVER_SHUTDOWN_GRACE, &mut server).await {
                Ok(result) => result,
                Err(_) => {
                    warn!("server shutdown timed out; closing remaining connections");
                    Ok(())
                }
            }
        }
    };

    worker_pool.shutdown().await;
    server_result?;
    Ok(())
}

/// Wait for an interactive interrupt or the Unix termination signal.
async fn shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate())
            .expect("failed to register SIGTERM handler for Ruvyxa server");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = terminate.recv() => "SIGTERM",
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        "CTRL_C"
    }
}

fn dependency_warmup_routes(
    config: &ServerConfig,
    manifest: &RouteManifest,
) -> Vec<worker_pool::WarmupRoute> {
    if !config.watch || !config.prebundle_dependencies {
        return Vec::new();
    }

    manifest
        .routes
        .iter()
        .filter(|route| route.kind == RouteKind::Page)
        .map(|route| worker_pool::WarmupRoute {
            page_file: route.file.display().to_string(),
            app_dir: config.app_dir.display().to_string(),
        })
        .collect()
}

async fn bind_listener(
    config: &ServerConfig,
    address: SocketAddr,
) -> Result<(TcpListener, SocketAddr)> {
    let mut first_addr_in_use = None;

    for offset in 0..=PORT_FALLBACK_SCAN_LIMIT {
        let Some(port) = address.port().checked_add(offset) else {
            break;
        };
        let mut candidate = address;
        candidate.set_port(port);

        let bind_result = TcpListener::bind(candidate).await;
        match bind_result {
            Ok(listener) => {
                let bound_address = listener.local_addr().unwrap_or(candidate);
                if offset > 0 {
                    print_port_fallback(config, address, bound_address);
                }
                return Ok((listener, bound_address));
            }
            Err(error) if error.kind() == ErrorKind::AddrInUse => {
                if offset == 0 {
                    first_addr_in_use = Some(error);
                }
            }
            Err(source) => {
                return Err(RuvyxaError::Io {
                    message: format!("Failed to bind server address {candidate}"),
                    source,
                });
            }
        }
    }

    let error =
        first_addr_in_use.unwrap_or_else(|| std::io::Error::from(ErrorKind::AddrNotAvailable));
    Err(port_conflict_diagnostic(config, address, &error).into())
}

fn print_port_fallback(config: &ServerConfig, requested: SocketAddr, bound: SocketAddr) {
    let message = format!(
        "Port {} is already in use; using {} instead.",
        requested.port(),
        bound.port()
    );
    warn!(
        requested = requested.port(),
        bound = bound.port(),
        "{message}"
    );
    println!("  {} {}", warn_text("warning"), accent(message));
    if let Some(owner) = port_owner(requested.port()) {
        println!("  {} {}", dim("port owner"), accent(owner));
    }
    println!(
        "  {} {}",
        dim("requested"),
        accent(format!("{}:{}", config.host, requested.port()))
    );
}

fn port_conflict_diagnostic(
    config: &ServerConfig,
    address: SocketAddr,
    error: &std::io::Error,
) -> Diagnostic {
    let owner = port_owner(address.port())
        .map(|owner| format!("\n\nDetected owner:\n  {owner}"))
        .unwrap_or_default();
    let end_port = address.port().saturating_add(PORT_FALLBACK_SCAN_LIMIT);
    let os_hint = port_lookup_hint(address.port());

    Diagnostic::new("RUV1201", "No available server port was found")
        .explain(format!(
            "{}:{} could not be bound, and Ruvyxa could not find a free port through {} ({error}).{owner}",
            config.host,
            address.port(),
            end_port
        ))
        .suggest(format!(
            "Stop the process using port {}, free a port in the {}-{} range, or pass `--port <free-port>`. {os_hint}",
            address.port(),
            address.port(),
            end_port
        ))
}

fn port_owner(port: u16) -> Option<String> {
    if cfg!(windows) {
        return windows_port_owner(port);
    }

    unix_port_owner(port)
}

fn windows_port_owner(port: u16) -> Option<String> {
    let output = Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid = stdout.lines().find_map(|line| {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        let local = columns.get(1)?;
        let state = columns.get(3)?;
        let pid = columns.get(4)?;

        if local.ends_with(&format!(":{port}")) && state.eq_ignore_ascii_case("LISTENING") {
            Some((*pid).to_string())
        } else {
            None
        }
    })?;

    let process = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .next()
                .and_then(|line| line.split(',').next())
                .map(|name| name.trim_matches('"').to_string())
        })
        .filter(|name| !name.is_empty());

    Some(match process {
        Some(process) => format!("PID {pid} ({process})"),
        None => format!("PID {pid}"),
    })
}

fn unix_port_owner(port: u16) -> Option<String> {
    let output = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().skip(1).find_map(|line| {
        if !line.contains(&format!(":{port}")) {
            return None;
        }
        let columns = line.split_whitespace().collect::<Vec<_>>();
        let process = columns.first()?;
        let pid = columns.get(1)?;
        Some(format!("PID {pid} ({process})"))
    })
}

fn port_lookup_hint(port: u16) -> String {
    if cfg!(windows) {
        format!(
            "On Windows, inspect it with `Get-NetTCPConnection -LocalPort {port} | Select-Object OwningProcess`."
        )
    } else {
        format!("On macOS/Linux, inspect it with `lsof -nP -iTCP:{port} -sTCP:LISTEN`.")
    }
}

fn print_server_ready(
    config: &ServerConfig,
    manifest: &RouteManifest,
    address: SocketAddr,
    ready_in: Duration,
) {
    let mode = if config.watch {
        "Development"
    } else {
        "Production"
    };
    let url = local_display_url(config, address);
    let page_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == RouteKind::Page)
        .count();
    let api_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == RouteKind::Api)
        .count();

    println!();
    if config.watch {
        println!("{}", heading("🦊 Ruvyxa Dev Server"));
        println!();
    } else {
        println!("{}", heading("🦊 Ruvyxa Server"));
        println!();
    }
    print_field("time", accent(current_timestamp()));
    print_field("mode", accent(mode));
    print_field("local", link(&url));
    print_field("root", path_text(&config.root));
    print_field("app dir", path_text(&config.app_dir));
    print_field("public", path_text(&config.public_dir));
    print_field("client", path_text(&config.client_dir));
    print_field("routes", accent(manifest.routes.len().to_string()));
    print_field("pages", accent(page_routes.to_string()));
    print_field("api", accent(api_routes.to_string()));
    print_field(
        "hmr",
        if config.watch {
            ok("enabled")
        } else {
            dim("off")
        },
    );
    print_field(
        "cache",
        accent(format!(
            "routes {}, css {}",
            enabled_text(config.cache_route_manifest),
            enabled_text(config.cache_css)
        )),
    );
    print_field("watch paths", accent(watch_paths(config).len().to_string()));
    print_field("ready in", accent(format_update_elapsed(ready_in)));
    print_field("middleware", accent(middleware_summary(&config.middleware)));
    println!();
}

fn local_display_url(config: &ServerConfig, address: SocketAddr) -> String {
    let host = config.host.trim();
    let display_host = if host.eq_ignore_ascii_case("localhost")
        || host == "0.0.0.0"
        || host == "::"
        || host == "[::]"
        || address.ip().is_loopback()
    {
        "localhost".to_string()
    } else if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    format!("http://{}:{}", display_host, address.port())
}

fn start_watcher(
    root: &Path,
    watch_paths: &[PathBuf],
    reload_tx: broadcast::Sender<String>,
    runtime_cache: Arc<RuntimeCache>,
    worker_pool: Arc<NodeWorkerPool>,
    render_cache: Arc<RenderCache>,
    hmr_tracker: Arc<HmrTracker>,
) -> Result<RecommendedWatcher> {
    let root = root.to_path_buf();
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let paths = event
                    .paths
                    .into_iter()
                    .filter(|path| !ignored_watch_path(&root, path))
                    .collect::<Vec<_>>();
                if paths.is_empty() {
                    return;
                }

                // Use HmrTracker for selective invalidation.
                let mut hmr_update = hmr_tracker.compute_update(&paths);
                if hmr_update.full_reload {
                    hmr_update.event_type = HmrEventType::FullReload;
                }
                // Selective cache invalidation based on affected routes.
                if hmr_update.full_reload || hmr_update.affected_routes.is_empty() {
                    // Full invalidation: manifest may have changed (new/deleted routes).
                    runtime_cache.invalidate();
                    render_cache.invalidate_all_blocking();
                } else {
                    // Selective invalidation: only evict affected route caches.
                    // Styles always need refresh if any source file changed.
                    *runtime_cache.styles.blocking_write() = None;

                    // Selectively invalidate render cache for affected routes only.
                    for route_path in &hmr_update.affected_routes {
                        render_cache.invalidate_route_blocking(route_path);
                    }
                }

                // Invalidate worker bundle caches for changed files.
                let path_strings: Vec<String> = paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                let worker_result = worker_pool.invalidate_from_watcher(path_strings.clone());
                if worker_result.is_err() {
                    hmr_update.full_reload = true;
                    hmr_update.event_type = HmrEventType::FullReload;
                }

                // Send targeted HMR payload with affected routes.
                let payload = serde_json::json!({
                    "type": hmr_update.event_type.as_str(),
                    "paths": path_strings,
                    "affectedRoutes": hmr_update.affected_routes,
                    "fullReload": hmr_update.full_reload,
                })
                .to_string();
                let _ = reload_tx.send(payload);
                if let Err(error) = worker_result {
                    warn!(%error, "worker invalidation failed; browser full reload requested");
                }
            }
            Err(error) => {
                println!("✖ File watcher failed (0ms)");
                println!("  Reason: {error}");
                println!(
                    "  Watcher remains active; refresh the browser after the next detected change."
                );
                warn!(%error, "file watcher error");
            }
        })
        .map_err(|error| RuvyxaError::Message(format!("Failed to start file watcher: {error}")))?;

    for path in watch_paths {
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|error| {
                RuvyxaError::Message(format!("Failed to watch {}: {error}", path.display()))
            })?;
    }

    Ok(watcher)
}

fn watch_paths(config: &ServerConfig) -> Vec<PathBuf> {
    let mut paths = vec![config.root.clone()];
    paths.retain(|path| path.exists());
    paths.sort();
    paths.dedup();
    paths
}

fn ignored_watch_path(root: &Path, path: &Path) -> bool {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let relative = if path.is_absolute() {
        path.strip_prefix(&canonical_root)
            .or_else(|_| path.strip_prefix(root))
            .unwrap_or(path)
    } else {
        path.strip_prefix(Path::new(".")).unwrap_or(path)
    };
    let components = relative
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();
    let top_level_ignored = components.first().is_some_and(|component| {
        matches!(
            component.as_ref(),
            ".git" | ".ruvyxa" | "target" | "dist" | ".npm-pack" | ".npm-smoke"
        ) || component.starts_with(".ruvyxa-")
    });
    top_level_ignored
        || components
            .iter()
            .any(|component| matches!(component.as_ref(), ".ruvyxa" | "node_modules"))
}

fn format_update_elapsed(elapsed: Duration) -> String {
    if elapsed >= Duration::from_millis(1) {
        return format!("{}ms", elapsed.as_millis());
    }
    let tenths = elapsed.as_micros().div_ceil(100).max(1);
    format!("{}.{:01}ms", tenths / 10, tenths % 10)
}

fn print_field(name: &str, value: String) {
    let padding = " ".repeat(20usize.saturating_sub(name.len()));
    println!("  {}{} {}", dim(name), padding, value);
}

fn current_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn enabled_text(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

fn middleware_summary(config: &MiddlewareConfig) -> String {
    let mut enabled = Vec::new();

    if config.builtin.timing {
        enabled.push("timing");
    }
    if config.builtin.logging {
        enabled.push("logging");
    }
    if config.builtin.cors.is_some() {
        enabled.push("cors");
    }
    if config.builtin.rate_limit.is_some() {
        enabled.push("rate-limit");
    }
    if !config.builtin.headers.is_empty() {
        enabled.push("headers");
    }
    if !config.layers.is_empty() {
        enabled.push("layers");
    }
    if !config.plugins.is_empty() {
        enabled.push("plugins");
    }

    if enabled.is_empty() {
        "none".to_string()
    } else {
        enabled.join(", ")
    }
}

fn heading(value: impl AsRef<str>) -> String {
    paint(value, "1;35")
}

fn accent(value: impl AsRef<str>) -> String {
    paint(value, "36")
}

fn dim(value: impl AsRef<str>) -> String {
    paint(value, "90")
}

fn ok(value: impl AsRef<str>) -> String {
    paint(value, "32")
}

fn warn_text(value: impl AsRef<str>) -> String {
    paint(value, "33")
}

fn link(value: impl AsRef<str>) -> String {
    paint(value, "34")
}

fn path_text(path: &Path) -> String {
    paint(path.display().to_string(), "34")
}

fn paint(value: impl AsRef<str>, code: &str) -> String {
    let value = value.as_ref();
    if !std::io::stdout().is_terminal()
        || std::env::var_os("NO_COLOR").is_some()
        || std::env::var("TERM")
            .map(|term| term.eq_ignore_ascii_case("dumb"))
            .unwrap_or(false)
    {
        return value.to_string();
    }

    format!("\x1b[{code}m{value}\x1b[0m")
}

async fn hmr_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        let mut reload_rx = state.reload_tx.subscribe();

        while let Ok(payload) = reload_rx.recv().await {
            if socket.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    })
}

#[derive(Debug, Deserialize)]
struct ClientBundleQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
struct ActionQuery {
    path: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct TraceQuery {
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeTrace {
    path: String,
    matched: bool,
    route: Option<RouteEntry>,
    params: BTreeMap<String, String>,
    runtime: &'static str,
    assets: TraceAssets,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceAssets {
    public_dir: String,
    app_dir: String,
}

async fn client_bundle(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ClientBundleQuery>,
) -> Response {
    let response = match render_client_bundle_pooled(&state, &query.path).await {
        Ok(script) => {
            let mut response = script.into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript; charset=utf-8"),
            );
            response
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("console.error({:?});", error.to_string()),
        )
            .into_response(),
    };
    with_security_headers(response)
}

async fn action_endpoint(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    Query(query): Query<ActionQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = validate_action_request(&headers, body.len(), &state.config) {
        return with_security_headers(response);
    }

    let rate_key = action_rate_limit_key(peer, &headers, &query);
    let retry_after = {
        let mut limiter = state
            .action_limiter
            .lock()
            .expect("action limiter mutex poisoned");
        (!limiter.allow(&rate_key)).then(|| limiter.retry_after_seconds(&rate_key))
    };
    if let Some(retry_after) = retry_after {
        return with_security_headers(
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after.to_string())],
                "Action rate limit exceeded",
            )
                .into_response(),
        );
    }

    let response = match render_server_action_pooled(
        &state,
        &query.path,
        &query.name,
        std::str::from_utf8(&body).unwrap_or("{}"),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("console.error({:?});", error.to_string()),
        )
            .into_response(),
    };
    with_security_headers(response)
}

async fn trace_endpoint(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TraceQuery>,
) -> Response {
    if !debug_traces_enabled(&state.config) {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }
    let response =
        match runtime_trace_cached(&state.config, &state.runtime_cache, &query.path).await {
            Ok(trace) => json_response(StatusCode::OK, &trace),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("console.error({:?});", error.to_string()),
            )
                .into_response(),
        };
    with_security_headers(response)
}

fn debug_traces_enabled(config: &ServerConfig) -> bool {
    config.watch && config.debug_traces
}

async fn handle_request(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
) -> impl IntoResponse {
    let started = Instant::now();
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    let mut method = parts.method.as_str().to_string();
    let mut request_path = parts.uri.path().to_string();
    let mut request_body = if request_method_allows_body(&method) {
        match to_bytes(body, state.config.api_body_limit_bytes).await {
            Ok(bytes) if bytes.is_empty() => None,
            Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Err(error) => {
                return with_security_headers(
                    (
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!(
                            "Request body exceeded the API body limit or could not be read: {error}"
                        ),
                    )
                        .into_response(),
                );
            }
        }
    } else {
        None
    };

    let mut plugin_request = PluginRequest {
        method: method.clone(),
        path: request_path.clone(),
        headers: headers_to_plugin_pairs(&headers),
        body: request_body.as_ref().map(|body| body.as_bytes().to_vec()),
    };
    match apply_request_plugins(&state, &mut plugin_request).await {
        Ok(Some(response)) => return response,
        Ok(None) => {
            method = plugin_request.method.clone();
            request_path = plugin_request.path.clone();
            headers = plugin_headers(&plugin_request.headers);
            request_body = plugin_request
                .body
                .as_deref()
                .map(|body| String::from_utf8_lossy(body).into_owned());
        }
        Err(error) => {
            return with_security_headers(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Middleware plugin request phase failed: {error}"),
                )
                    .into_response(),
            );
        }
    }

    let render_result = render_request_pooled(
        &state,
        &request_path,
        &method,
        &headers,
        request_body.as_deref(),
    )
    .await;
    let response = match render_result {
        Ok(response) => response,
        Err(error) => {
            let is_dev = state.config.watch && state.config.error_overlay;
            match &error {
                RuvyxaError::Diagnostic(diag) => {
                    error_response(StatusCode::INTERNAL_SERVER_ERROR, diag, is_dev)
                }
                _ => {
                    let body = if is_dev {
                        dev_error_overlay(&error.to_string(), None, None, None)
                    } else {
                        plain_error_page(&error.to_string())
                    };
                    html_response(StatusCode::INTERNAL_SERVER_ERROR, body)
                }
            }
        }
    };
    let response = match apply_response_plugins(&state, &plugin_request, response).await {
        Ok(response) => response,
        Err(error) => with_security_headers(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Middleware plugin response phase failed: {error}"),
            )
                .into_response(),
        ),
    };
    if state.config.watch && should_log_dev_request(&request_path) {
        println!(
            "{}",
            dev_page_request_log(&method, &request_path, response.status(), started.elapsed())
        );
    }
    response
}

async fn apply_request_plugins(
    state: &AppState,
    request: &mut PluginRequest,
) -> Result<Option<Response>> {
    let Some(runtime) = &state.plugin_runtime else {
        return Ok(None);
    };
    let Some(result) = runtime.execute_request_plugins(request).await? else {
        return Ok(None);
    };
    match result.action.as_str() {
        "respond" => Ok(Some(plugin_response_into_response(
            result.response.ok_or_else(|| {
                RuvyxaError::Message("Plugin returned respond without a response".to_string())
            })?,
        )?)),
        "modify-request" => {
            let replacement = result.request.ok_or_else(|| {
                RuvyxaError::Message("Plugin returned modify-request without a request".to_string())
            })?;
            *request = replacement;
            Ok(None)
        }
        action => Err(RuvyxaError::Message(format!(
            "Plugin returned unsupported request-phase action '{action}'"
        ))),
    }
}

async fn apply_response_plugins(
    state: &AppState,
    request: &PluginRequest,
    response: Response,
) -> Result<Response> {
    let Some(runtime) = &state.plugin_runtime else {
        return Ok(response);
    };
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX).await.map_err(|error| {
        RuvyxaError::Message(format!("Failed to read response for plugin: {error}"))
    })?;
    let plugin_response = PluginResponse {
        status: parts.status.as_u16(),
        headers: headers_to_plugin_pairs(&parts.headers),
        body: Some(body.to_vec()),
    };
    let original = plugin_response.clone();
    let Some(result) = runtime
        .execute_response_plugins(request, &plugin_response)
        .await?
    else {
        return plugin_response_into_response(original);
    };
    match result.action.as_str() {
        "respond" | "modify-response" => {
            plugin_response_into_response(result.response.ok_or_else(|| {
                RuvyxaError::Message(
                    "Plugin returned a response action without a response".to_string(),
                )
            })?)
        }
        action => Err(RuvyxaError::Message(format!(
            "Plugin returned unsupported response-phase action '{action}'"
        ))),
    }
}

fn plugin_response_into_response(response: PluginResponse) -> Result<Response> {
    let status = StatusCode::from_u16(response.status).map_err(|error| {
        RuvyxaError::Message(format!("Plugin returned invalid status: {error}"))
    })?;
    let mut output = (status, response.body.unwrap_or_default()).into_response();
    for (name, value) in response.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            RuvyxaError::Message(format!("Plugin returned invalid header name: {error}"))
        })?;
        let value = HeaderValue::from_str(&value).map_err(|error| {
            RuvyxaError::Message(format!("Plugin returned invalid header value: {error}"))
        })?;
        output.headers_mut().insert(name, value);
    }
    Ok(output)
}

fn headers_to_plugin_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.to_string(), value.to_string()))
        })
        .collect()
}

fn plugin_headers(headers: &[(String, String)]) -> HeaderMap {
    headers
        .iter()
        .filter_map(|(name, value)| {
            Some((
                HeaderName::from_bytes(name.as_bytes()).ok()?,
                HeaderValue::from_str(value).ok()?,
            ))
        })
        .collect()
}

fn request_method_allows_body(method: &str) -> bool {
    !method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD")
}

fn should_log_dev_request(request_path: &str) -> bool {
    if request_path.starts_with("/__ruvyxa/") {
        return false;
    }
    if request_path == "/api" || request_path.starts_with("/api/") {
        return true;
    }
    Path::new(request_path).extension().is_none()
}

fn dev_page_request_log(
    method: &str,
    request_path: &str,
    status: StatusCode,
    elapsed: Duration,
) -> String {
    format!(
        "{} {} {} {} {} {} {}",
        paint("◌", "1;32"),
        paint(method, "1;32"),
        paint(request_path, "1;37"),
        dim("→"),
        status_text(status),
        dim("·"),
        accent(format_update_elapsed(elapsed))
    )
}

fn status_text(status: StatusCode) -> String {
    let color = if status.is_success() {
        "1;32"
    } else if status.is_redirection() {
        "1;36"
    } else if status.is_client_error() {
        "1;33"
    } else {
        "1;31"
    };
    paint(status.as_u16().to_string(), color)
}

fn worker_request_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

pub fn render_request(config: &ServerConfig, request_path: &str, method: &str) -> Result<Response> {
    render_request_cached(config, request_path, method)
}

fn render_request_cached(
    config: &ServerConfig,
    request_path: &str,
    method: &str,
) -> Result<Response> {
    if let Some(client_response) = serve_client_file_sync(&config.client_dir, request_path)? {
        return Ok(client_response);
    }

    if let Some(public_response) = serve_public_file_sync(&config.public_dir, request_path)? {
        return Ok(public_response);
    }

    let manifest = discover_routes(DiscoverOptions::new(&config.app_dir))?;
    let Some(route_match) = find_route(&manifest, request_path) else {
        return Ok(html_response(
            StatusCode::NOT_FOUND,
            error_page("Route not found", config.watch && config.error_overlay),
        ));
    };

    match route_match.route.kind {
        RouteKind::Page => {
            let styles = collect_styles(&config.root, &config.app_dir, &config.style_entries)?.css;
            let html = render_page(
                config,
                route_match.route,
                request_path,
                &route_match.params,
                &styles,
            )?;
            Ok(html_response(StatusCode::OK, html))
        }
        RouteKind::Api => render_api(
            config,
            route_match.route,
            request_path,
            method,
            &route_match.params,
        ),
    }
}

/// Sync fallback for static file serving (used by render_request test/bench path).
fn serve_public_file_sync(public_dir: &Path, request_path: &str) -> Result<Option<Response>> {
    let trimmed = request_path.trim_start_matches('/');
    if !is_safe_relative_path(trimmed) {
        return Ok(None);
    }
    let Some((file, _)) = resolve_public_asset(public_dir, trimmed, None) else {
        return Ok(None);
    };
    let bytes = fs::read(&file)?;
    let content_type = content_type_for(&file);
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    apply_security_headers(&mut response);
    Ok(Some(response))
}

/// Sync fallback for client file serving (used by render_request test/bench path).
fn serve_client_file_sync(client_dir: &Path, request_path: &str) -> Result<Option<Response>> {
    let Some(file_name) = request_path.strip_prefix("/__ruvyxa/client/") else {
        return Ok(None);
    };
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Ok(None);
    }
    let file = client_dir.join(file_name);
    if !file.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&file)?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    apply_security_headers(&mut response);
    Ok(Some(response))
}

// --- Worker-pool-based async render functions ---

async fn render_request_pooled(
    state: &AppState,
    request_path: &str,
    method: &str,
    request_headers: &HeaderMap,
    request_body: Option<&str>,
) -> Result<Response> {
    if let Some(client_response) = serve_client_file(
        &state.config.client_dir,
        request_path,
        Some(request_headers),
    )
    .await?
    {
        return Ok(client_response);
    }

    if let Some(public_response) = serve_public_file(
        &state.config.public_dir,
        request_path,
        Some(request_headers),
    )
    .await?
    {
        return Ok(public_response);
    }

    let (manifest, router) = state.runtime_cache.router(&state.config).await?;
    let Some(route_match) = router.find(&manifest, request_path) else {
        return Ok(html_response(
            StatusCode::NOT_FOUND,
            error_page(
                "Route not found",
                state.config.watch && state.config.error_overlay,
            ),
        ));
    };

    match route_match.route.kind {
        RouteKind::Page => {
            let styles = state.runtime_cache.styles(&state.config).await?;
            let html = render_page_by_strategy(
                state,
                route_match.route,
                request_path,
                &route_match.params,
                &styles,
            )
            .await?;
            Ok(html_response(StatusCode::OK, html))
        }
        RouteKind::Api => {
            let headers = worker_request_headers(request_headers);
            render_api_pooled(
                state,
                route_match.route,
                request_path,
                method,
                &headers,
                request_body,
                &route_match.params,
            )
            .await
        }
    }
}

/// Dispatch page rendering based on the route's declared rendering strategy.
async fn render_page_by_strategy(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
) -> Result<String> {
    match route.render.strategy {
        RenderStrategy::Ssr => render_page_pooled(state, route, request_path, params, styles).await,
        RenderStrategy::Ssg => {
            // In dev mode, SSG pages are rendered on-demand like SSR but cached indefinitely.
            render_page_ssg(state, route, request_path, params, styles).await
        }
        RenderStrategy::Isr => render_page_isr(state, route, request_path, params, styles).await,
        RenderStrategy::Csr => render_page_csr(state, route, request_path, params, styles).await,
        RenderStrategy::Ppr => render_page_ppr(state, route, request_path, params, styles).await,
    }
}

/// SSG in dev mode: render once and cache (no TTL eviction).
/// In production: serve pre-rendered HTML directly from disk.
async fn render_page_ssg(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
) -> Result<String> {
    // In production, try to serve the pre-rendered HTML file directly
    if !state.config.watch
        && let Some(html) = serve_prerendered_html(&state.config.prerender_dir, request_path)
    {
        return Ok(html);
    }

    let cache_key = format!("ssg:{}", render_cache::ssr_cache_key(request_path, params));
    if let Some(cached) = state.render_cache.get(&cache_key).await {
        return Ok(cached);
    }

    // Render via worker pool (same as SSR but with the SSG bundle type)
    let response = state
        .worker_pool
        .render_ssg(
            &state.config.root,
            &state.config.app_dir,
            &route.file,
            request_path,
            params,
            "full",
        )
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1500".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "SSG render failed".to_string());
        return Err(Diagnostic::new("RUV1500", "SSG render failed")
            .explain(format!("{code}: {message}"))
            .at_file(&route.file)
            .into());
    }

    let rendered = response
        .html
        .ok_or_else(|| RuvyxaError::Message("SSG render produced no HTML".to_string()))?;

    let asset_links = public_asset_links(&state.config.public_dir);
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);
    let head_content = format!(r#"{asset_links}<style data-ruvyxa-css>{styles}</style>"#);
    let html = compose_document(&rendered, &head_content, &format!("{client_script}{hmr}"));

    state.render_cache.put(cache_key, html.clone()).await;
    Ok(html)
}

/// ISR: serve from cache if available (stale-while-revalidate), trigger
/// background revalidation when the entry is older than the revalidate interval.
/// In production: serve pre-rendered HTML and schedule background revalidation.
async fn render_page_isr(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
) -> Result<String> {
    let cache_key = format!("isr:{}", render_cache::ssr_cache_key(request_path, params));

    // Try to serve stale content immediately (from cache or prerendered file)
    if let Some(cached) = state.render_cache.get(&cache_key).await {
        // Trigger background revalidation
        spawn_isr_revalidation(state, route, request_path, params, styles, &cache_key);
        return Ok(cached);
    }

    // In production, try the pre-rendered HTML file
    if !state.config.watch
        && let Some(html) = serve_prerendered_html(&state.config.prerender_dir, request_path)
    {
        // Store in cache and schedule revalidation
        state
            .render_cache
            .put(cache_key.clone(), html.clone())
            .await;
        spawn_isr_revalidation(state, route, request_path, params, styles, &cache_key);
        return Ok(html);
    }

    // No cached version — render synchronously (blocking fallback)
    let html = render_isr_background(state, route, request_path, params, styles).await?;
    state.render_cache.put(cache_key, html.clone()).await;
    Ok(html)
}

/// ISR background render (used both for first render and revalidation).
async fn render_isr_background(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
) -> Result<String> {
    let response = state
        .worker_pool
        .render_ssg(
            &state.config.root,
            &state.config.app_dir,
            &route.file,
            request_path,
            params,
            "full",
        )
        .await?;

    if !response.ok {
        let message = response.message.unwrap_or_default();
        return Err(RuvyxaError::Message(format!(
            "ISR revalidation failed: {message}"
        )));
    }

    let rendered = response
        .html
        .ok_or_else(|| RuvyxaError::Message("ISR render produced no HTML".to_string()))?;

    let asset_links = public_asset_links(&state.config.public_dir);
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);
    let head_content = format!(r#"{asset_links}<style data-ruvyxa-css>{styles}</style>"#);
    Ok(compose_document(
        &rendered,
        &head_content,
        &format!("{client_script}{hmr}"),
    ))
}

/// Spawn a background task to revalidate an ISR page.
fn spawn_isr_revalidation(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
    cache_key: &str,
) {
    let revalidate_state = state.clone();
    let revalidate_route = route.clone();
    let revalidate_path = request_path.to_string();
    let revalidate_params = params.clone();
    let revalidate_styles = styles.to_string();
    let revalidate_key = cache_key.to_string();

    tokio::spawn(async move {
        if let Ok(html) = render_isr_background(
            &revalidate_state,
            &revalidate_route,
            &revalidate_path,
            &revalidate_params,
            &revalidate_styles,
        )
        .await
        {
            revalidate_state
                .render_cache
                .put(revalidate_key, html)
                .await;
        }
    });
}

/// Try to serve a pre-rendered HTML file from the prerender directory.
/// Returns `Some(html)` if the file exists, `None` otherwise.
fn serve_prerendered_html(prerender_dir: &Path, request_path: &str) -> Option<String> {
    let sanitized = request_path.trim_start_matches('/');
    let html_path = if sanitized.is_empty() {
        prerender_dir.join("index.html")
    } else {
        prerender_dir.join(sanitized).join("index.html")
    };

    fs::read_to_string(&html_path).ok()
}

/// CSR: emit a minimal HTML shell with no server-rendered content.
/// The page loads entirely in the browser via the client bundle.
/// In production: serve the pre-built CSR shell HTML.
async fn render_page_csr(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
) -> Result<String> {
    // In production, serve the pre-rendered CSR shell
    if !state.config.watch
        && let Some(html) = serve_prerendered_html(&state.config.prerender_dir, request_path)
    {
        return Ok(html);
    }

    let asset_links = public_asset_links(&state.config.public_dir);
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);

    let params_json = serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string());
    let path_json = serde_json::to_string(request_path).unwrap_or_else(|_| "\"\"".to_string());

    let shell = format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  {asset_links}
  <style data-ruvyxa-css>{styles}</style>
  <script>
    window.__RUVYXA_ROUTE_PARAMS__ = {params_json};
    window.__RUVYXA_REQUEST_PATH__ = {path_json};
  </script>
</head>
<body>
  <div id="__ruvyxa"></div>
  {client_script}
  {hmr}
</body>
</html>"#
    );

    Ok(shell)
}

/// PPR: render the static shell (Suspense fallbacks) and stream dynamic slots.
/// In dev mode, we render with onShellReady to get the shell quickly, then
/// the remaining content streams in via the client hydration.
/// In production: serve the pre-rendered shell from disk.
async fn render_page_ppr(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
) -> Result<String> {
    // In production, serve the pre-rendered PPR shell
    if !state.config.watch
        && let Some(html) = serve_prerendered_html(&state.config.prerender_dir, request_path)
    {
        return Ok(html);
    }

    let cache_key = format!("ppr:{}", render_cache::ssr_cache_key(request_path, params));
    if let Some(cached) = state.render_cache.get(&cache_key).await {
        return Ok(cached);
    }

    // PPR mode: render with onShellReady (Suspense boundaries show fallback)
    let response = state
        .worker_pool
        .render_ssg(
            &state.config.root,
            &state.config.app_dir,
            &route.file,
            request_path,
            params,
            "ppr",
        )
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1550".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "PPR render failed".to_string());
        return Err(Diagnostic::new("RUV1550", "PPR render failed")
            .explain(format!("{code}: {message}"))
            .at_file(&route.file)
            .into());
    }

    let rendered = response
        .html
        .ok_or_else(|| RuvyxaError::Message("PPR render produced no HTML".to_string()))?;

    let asset_links = public_asset_links(&state.config.public_dir);
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);
    let head_content = format!(r#"{asset_links}<style data-ruvyxa-css>{styles}</style>"#);
    let html = compose_document(&rendered, &head_content, &format!("{client_script}{hmr}"));

    state.render_cache.put(cache_key, html.clone()).await;
    Ok(html)
}

async fn render_page_pooled(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
) -> Result<String> {
    // Check render cache first
    let cache_key = render_cache::ssr_cache_key(request_path, params);
    if let Some(cached) = state.render_cache.get(&cache_key).await {
        return Ok(cached);
    }

    let source_fut = {
        let file = route.file.clone();
        tokio::task::spawn_blocking(move || {
            fs::read_to_string(&file).map_err(|source| RuvyxaError::Io {
                message: format!("Failed to read page module {}", file.display()),
                source,
            })
        })
    };

    let source = source_fut
        .await
        .map_err(|e| RuvyxaError::Message(format!("Page read task panicked: {e}")))??;

    if !page_has_default_export(&route.file, &source) {
        return Err(
            Diagnostic::new("RUV1004", "Page is missing a default export")
                .explain("Every TypeScript/JavaScript page must export a default component. Markdown and MDX pages receive one from the content compiler.")
                .at_file(&route.file)
                .suggest("Add `export default function Page() { return <main /> }`.")
                .into(),
        );
    }

    let response = state
        .worker_pool
        .render_ssr(
            &state.config.root,
            &state.config.app_dir,
            &route.file,
            request_path,
            params,
        )
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1100".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "React SSR failed without an error message".to_string());
        let explanation = if let Some(stack) = response.stack {
            format!("{message}\n\n{stack}")
        } else {
            message
        };
        return Err(Diagnostic::new("RUV1100", "React SSR failed")
            .explain(format!("{code}: {explanation}"))
            .at_file(&route.file)
            .suggest("Check the page component, its imports, and whether React dependencies are installed.")
            .into());
    }

    let rendered = response
        .html
        .ok_or_else(|| RuvyxaError::Message("React SSR completed without HTML".to_string()))?;

    let asset_links = public_asset_links(&state.config.public_dir);
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);
    let head_content = format!(r#"{asset_links}<style data-ruvyxa-css>{styles}</style>"#);

    let html = compose_document(&rendered, &head_content, &format!("{client_script}{hmr}"));

    // Cache the fully rendered page for subsequent requests
    state.render_cache.put(cache_key, html.clone()).await;

    Ok(html)
}
async fn render_api_pooled(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    method: &str,
    headers: &BTreeMap<String, String>,
    body: Option<&str>,
    params: &BTreeMap<String, String>,
) -> Result<Response> {
    let response = state
        .worker_pool
        .render_api(RenderApiRequest {
            project_root: &state.config.root,
            route_file: &route.file,
            method,
            request_path,
            headers,
            body,
            params,
        })
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1200".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "API route failed without an error message".to_string());
        let explanation = if let Some(stack) = response.stack {
            format!("{message}\n\n{stack}")
        } else {
            message
        };
        return Err(Diagnostic::new("RUV1200", "API route execution failed")
            .explain(format!("{code}: {explanation}"))
            .at_file(&route.file)
            .suggest("Check the route handler export and its imports.")
            .into());
    }

    let status = response.status.unwrap_or(200);
    let status = StatusCode::from_u16(status)
        .map_err(|error| RuvyxaError::Message(format!("Invalid API response status: {error}")))?;
    let body = response.body.unwrap_or_default();
    let mut http_response = (status, body).into_response();

    if let Some(headers) = response.headers {
        for (name, value) in headers {
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_str(&value) else {
                continue;
            };
            http_response.headers_mut().insert(name, value);
        }
    }

    Ok(with_security_headers(http_response))
}

async fn render_client_bundle_pooled(state: &AppState, request_path: &str) -> Result<String> {
    let (manifest, router) = state.runtime_cache.router(&state.config).await?;
    let Some(route_match) = router.find(&manifest, request_path) else {
        return Err(Diagnostic::new("RUV1303", "Client route was not found")
            .explain("The browser requested a hydration bundle for a route that does not exist.")
            .suggest("Reload the page so the client bundle URL matches the current route.")
            .into());
    };

    if route_match.route.kind != RouteKind::Page {
        return Err(
            Diagnostic::new("RUV1304", "Client bundle requested for a non-page route")
                .explain("Only page routes can produce a hydration bundle.")
                .at_file(&route_match.route.file)
                .suggest("Request a client bundle for a page route instead.")
                .into(),
        );
    }

    // Check render cache for client bundles
    let cache_key = render_cache::client_cache_key(request_path, &route_match.params);
    if let Some(cached) = state.render_cache.get(&cache_key).await {
        return Ok(cached);
    }

    let response = state
        .worker_pool
        .render_client(
            &state.config.root,
            &state.config.app_dir,
            &route_match.route.file,
            request_path,
            &route_match.params,
        )
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1300".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "Client bundling failed without an error message".to_string());
        let explanation = if let Some(stack) = response.stack {
            format!("{message}\n\n{stack}")
        } else {
            message
        };
        return Err(
            Diagnostic::new("RUV1300", "Client hydration bundling failed")
                .explain(format!("{code}: {explanation}"))
                .suggest(
                    "Check the page component, its browser-safe imports, and React dependencies.",
                )
                .into(),
        );
    }

    let script = response.script.ok_or_else(|| {
        RuvyxaError::Message("Client renderer completed without script output".to_string())
    })?;

    // Cache the bundled client script
    state.render_cache.put(cache_key, script.clone()).await;

    Ok(script)
}

async fn render_server_action_pooled(
    state: &AppState,
    request_path: &str,
    action_name: &str,
    payload_json: &str,
) -> Result<Response> {
    let (manifest, router) = state.runtime_cache.router(&state.config).await?;
    let Some(route_match) = router.find(&manifest, request_path) else {
        return Ok((StatusCode::NOT_FOUND, "Route not found for action").into_response());
    };

    if route_match.route.kind != RouteKind::Page {
        return Ok((
            StatusCode::METHOD_NOT_ALLOWED,
            "Actions can only target page routes",
        )
            .into_response());
    }

    let action_file = action_file_for(route_match.route).ok_or_else(|| {
        Diagnostic::new("RUV1501", "Route action file was not found")
            .explain(
                "Server actions are resolved from action.ts or action.js next to the page route.",
            )
            .at_file(&route_match.route.file)
            .suggest(
                "Create action.ts beside the page and export the action handler you want to call.",
            )
    })?;

    let response = state
        .worker_pool
        .render_action(
            &state.config.root,
            &action_file,
            action_name,
            payload_json,
            request_path,
        )
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1500".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "Unknown server action error".to_string());
        let mut diagnostic = Diagnostic::new(
            action_error_code(Some(&code)),
            "Server action execution failed",
        )
        .explain(message)
        .at_file(&route_match.route.file);

        if let Some(stack) = response.stack {
            diagnostic = diagnostic.suggest(stack);
        }

        return Err(diagnostic.into());
    }

    let status = StatusCode::from_u16(response.status.unwrap_or(200)).unwrap_or(StatusCode::OK);
    let mut http_response = (status, response.body.unwrap_or_default()).into_response();

    if let Some(headers) = response.headers {
        for (key, value) in headers {
            let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_str(&value) else {
                continue;
            };
            http_response.headers_mut().insert(name, value);
        }
    }

    Ok(with_security_headers(http_response))
}

async fn runtime_trace_cached(
    config: &ServerConfig,
    runtime_cache: &RuntimeCache,
    request_path: &str,
) -> Result<RuntimeTrace> {
    let manifest = runtime_cache.manifest(config).await?;
    let route_match = find_route(&manifest, request_path);
    let (route, params) = match route_match {
        Some(route_match) => (Some(route_match.route.clone()), route_match.params),
        None => (None, BTreeMap::new()),
    };

    Ok(RuntimeTrace {
        path: request_path.to_string(),
        matched: route.is_some(),
        route,
        params,
        runtime: if config.watch { "dev" } else { "production" },
        assets: TraceAssets {
            public_dir: config.public_dir.display().to_string(),
            app_dir: config.app_dir.display().to_string(),
        },
    })
}

async fn serve_public_file(
    public_dir: &Path,
    request_path: &str,
    request_headers: Option<&HeaderMap>,
) -> Result<Option<Response>> {
    let trimmed = request_path.trim_start_matches('/');
    if !is_safe_relative_path(trimmed) {
        return Ok(None);
    }

    let Some((file, vary_accept)) = resolve_public_asset(public_dir, trimmed, request_headers)
    else {
        return Ok(None);
    };
    let metadata = match tokio::fs::metadata(&file).await {
        Ok(meta) if meta.is_file() => meta,
        _ => return Ok(None),
    };

    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read public file {}", file.display()),
            source,
        })?;

    // Compute ETag using blake3 hash
    let etag = compute_etag(&bytes);

    // Check If-None-Match for conditional response
    if let Some(headers) = request_headers
        && let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && let Ok(client_etag) = if_none_match.to_str()
        && client_etag.trim_matches('"') == etag.trim_matches('"')
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if vary_accept {
            response
                .headers_mut()
                .insert(header::VARY, HeaderValue::from_static("Accept"));
        }
        apply_security_headers(&mut response);
        return Ok(Some(response));
    }

    let content_type = content_type_for(&file);
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600, must-revalidate"),
    );
    if vary_accept {
        headers.insert(header::VARY, HeaderValue::from_static("Accept"));
    }

    let _ = metadata; // used for existence check
    apply_security_headers(&mut response);
    Ok(Some(response))
}

fn resolve_public_asset(
    public_dir: &Path,
    request_path: &str,
    _request_headers: Option<&HeaderMap>,
) -> Option<(PathBuf, bool)> {
    let requested = public_dir.join(request_path);
    if requested.is_file() {
        return contained_public_asset(public_dir, &requested).map(|file| (file, false));
    }

    // Development keeps source images untouched while the React component
    // points at the production `.webp` URL. Resolve that URL to exactly one
    // source format; ambiguity matches the build-time collision guard.
    if requested.extension().and_then(|value| value.to_str()) == Some("webp") {
        let mut candidates = ["png", "jpg", "jpeg", "PNG", "JPG", "JPEG"]
            .map(|extension| requested.with_extension(extension))
            .into_iter()
            .filter_map(|path| {
                path.is_file()
                    .then(|| contained_public_asset(public_dir, &path))
                    .flatten()
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        if candidates.len() == 1 {
            return Some((candidates.into_iter().next()?, false));
        }
    }

    // Keep server deployments compatible with plain `<img src="hero.png">`
    // while the build output stores only `hero.webp`.
    if is_convertible_image_url(&requested) {
        let webp = requested.with_extension("webp");
        if webp.is_file() {
            return contained_public_asset(public_dir, &webp).map(|file| (file, false));
        }
    }
    None
}

/// Canonicalize asset paths before serving them so public-directory symlinks
/// cannot expose files outside the configured root.
fn contained_public_asset(public_dir: &Path, candidate: &Path) -> Option<PathBuf> {
    let public_root = public_dir.canonicalize().ok()?;
    let candidate = candidate.canonicalize().ok()?;
    candidate.starts_with(&public_root).then_some(candidate)
}

fn is_convertible_image_url(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
}

fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') {
        return false;
    }

    Path::new(path).components().all(|component| {
        matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    })
}

/// Compute a strong ETag using blake3 hash of file content.
fn compute_etag(bytes: &[u8]) -> String {
    let hash = blake3::hash(bytes);
    format!("\"{}\"", &hash.to_hex()[..16])
}

async fn serve_client_file(
    client_dir: &Path,
    request_path: &str,
    request_headers: Option<&HeaderMap>,
) -> Result<Option<Response>> {
    let Some(file_name) = request_path.strip_prefix("/__ruvyxa/client/") else {
        return Ok(None);
    };

    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Ok(None);
    }

    let file = client_dir.join(file_name);
    match tokio::fs::metadata(&file).await {
        Ok(meta) if meta.is_file() => {}
        _ => return Ok(None),
    }

    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read client file {}", file.display()),
            source,
        })?;

    // Client bundles are content-hashed, so use immutable caching with ETag
    let etag = compute_etag(&bytes);

    if let Some(headers) = request_headers
        && let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && let Ok(client_etag) = if_none_match.to_str()
        && client_etag.trim_matches('"') == etag.trim_matches('"')
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        apply_security_headers(&mut response);
        return Ok(Some(response));
    }

    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    apply_security_headers(&mut response);
    Ok(Some(response))
}

fn html_response(status: StatusCode, body: String) -> Response {
    let mut response = (status, Html(body)).into_response();
    apply_security_headers(&mut response);
    response
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_string(value) {
        Ok(body) => {
            let mut response = (status, body).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            );
            apply_security_headers(&mut response);
            response
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize JSON response: {error}"),
        )
            .into_response(),
    }
}

fn apply_security_headers(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
}

fn finalize_security_headers(mut response: Response, enabled: bool) -> Response {
    if enabled {
        apply_security_headers(&mut response);
    } else {
        let headers = response.headers_mut();
        headers.remove(header::X_CONTENT_TYPE_OPTIONS);
        headers.remove("referrer-policy");
        headers.remove("permissions-policy");
        headers.remove("cross-origin-opener-policy");
    }
    response
}

fn with_security_headers(mut response: Response) -> Response {
    apply_security_headers(&mut response);
    response
}

fn render_page(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
    styles: &str,
) -> Result<String> {
    let source = fs::read_to_string(&route.file).map_err(|source| RuvyxaError::Io {
        message: format!("Failed to read page module {}", route.file.display()),
        source,
    })?;

    if !page_has_default_export(&route.file, &source) {
        return Err(
            Diagnostic::new("RUV1004", "Page is missing a default export")
                .explain("Every TypeScript/JavaScript page must export a default component. Markdown and MDX pages receive one from the content compiler.")
                .at_file(&route.file)
                .suggest("Add `export default function Page() { return <main /> }`.")
                .into(),
        );
    }

    let rendered = render_react_page(config, route, request_path, params)?;
    let asset_links = public_asset_links(&config.public_dir);
    let hmr = if config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(config, route, request_path, params);
    let head_content = format!(r#"{asset_links}<style data-ruvyxa-css>{styles}</style>"#);

    Ok(compose_document(
        &rendered,
        &head_content,
        &format!("{client_script}{hmr}"),
    ))
}

fn page_has_default_export(file: &Path, source: &str) -> bool {
    matches!(
        file.extension().and_then(|extension| extension.to_str()),
        Some("md" | "mdx")
    ) || source.contains("export default")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SsrRenderResult {
    ok: bool,
    html: Option<String>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiRenderResult {
    ok: bool,
    status: Option<u16>,
    headers: Option<BTreeMap<String, String>>,
    body: Option<String>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

fn render_react_page(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let renderer = find_ssr_renderer(&config.root).ok_or_else(|| {
        Diagnostic::new("RUV1102", "SSR renderer was not found")
            .explain("Ruvyxa could not find the Node SSR renderer used to transform TSX and render React.")
            .suggest("Run pnpm install from the monorepo root, or install the ruvyxa package in the app.")
    })?;

    let output = node_command(&config.root)?
        .arg(&renderer)
        .arg(&config.root)
        .arg(&config.app_dir)
        .arg(&route.file)
        .arg(request_path)
        .arg(
            serde_json::to_string(params)
                .map_err(|error| RuvyxaError::Message(error.to_string()))?,
        )
        .output()
        .map_err(|source| RuvyxaError::Io {
            message: "Failed to start Node for React SSR".to_string(),
            source,
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let result: SsrRenderResult =
        serde_json::from_str(&stdout).map_err(|error| {
            RuvyxaError::Message(format!(
                "React SSR returned invalid renderer output: {error}\nstdout:\n{stdout}\nstderr:\n{stderr}"
            ))
        })?;

    if output.status.success() && result.ok {
        return result
            .html
            .ok_or_else(|| RuvyxaError::Message("React SSR completed without HTML".to_string()));
    }

    let code = result.code.unwrap_or_else(|| "RUV1100".to_string());
    let message = result
        .message
        .unwrap_or_else(|| "React SSR failed without an error message".to_string());
    let explanation = if let Some(stack) = result.stack {
        format!("{message}\n\n{stack}")
    } else {
        message
    };

    Err(Diagnostic::new("RUV1100", "React SSR failed")
        .explain(format!("{code}: {explanation}"))
        .at_file(&route.file)
        .suggest(
            "Check the page component, its imports, and whether React dependencies are installed.",
        )
        .into())
}

fn find_ssr_renderer(root: &Path) -> Option<PathBuf> {
    find_runtime_script(root, "ssr-renderer.mjs")
}

fn find_api_renderer(root: &Path) -> Option<PathBuf> {
    find_runtime_script(root, "api-renderer.mjs")
}

fn find_runtime_script(root: &Path, file_name: &str) -> Option<PathBuf> {
    if let Ok(renderer) = std::env::var("RUVYXA_SSR_RENDERER") {
        let path = PathBuf::from(renderer);
        if file_name == "ssr-renderer.mjs" && path.is_file() {
            return Some(path);
        }
    }

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

fn node_command(root: &Path) -> Result<Command> {
    let mut command = Command::new("node");
    command.envs(project_env(root)?);
    Ok(command)
}

fn project_env(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();

    for file_name in [".env", ".env.local"] {
        let file = root.join(file_name);
        if !file.exists() {
            continue;
        }

        let source = fs::read_to_string(&file).map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read {}", file.display()),
            source,
        })?;

        for (key, value) in parse_env_source(&source) {
            values.insert(key, value);
        }
    }

    Ok(values)
}

fn parse_env_source(source: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();

    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        if key.is_empty() {
            continue;
        }

        values.insert(key.to_string(), unquote_env_value(value.trim()));
    }

    values
}

fn unquote_env_value(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn render_api(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    method: &str,
    params: &BTreeMap<String, String>,
) -> Result<Response> {
    let renderer = find_api_renderer(&config.root).ok_or_else(|| {
        Diagnostic::new("RUV1202", "API renderer was not found")
            .explain("Ruvyxa could not find the Node API renderer used to transform and execute route handlers.")
            .suggest("Run pnpm install from the monorepo root, or install the ruvyxa package in the app.")
    })?;

    let output = node_command(&config.root)?
        .arg(&renderer)
        .arg(&config.root)
        .arg(&route.file)
        .arg(method)
        .arg(request_path)
        .arg(
            serde_json::to_string(params)
                .map_err(|error| RuvyxaError::Message(error.to_string()))?,
        )
        .output()
        .map_err(|source| RuvyxaError::Io {
            message: "Failed to start Node for API route rendering".to_string(),
            source,
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let result: ApiRenderResult =
        serde_json::from_str(&stdout).map_err(|error| {
            RuvyxaError::Message(format!(
                "API route returned invalid renderer output: {error}\nstdout:\n{stdout}\nstderr:\n{stderr}"
            ))
        })?;

    if !output.status.success() || !result.ok {
        let code = result.code.unwrap_or_else(|| "RUV1200".to_string());
        let message = result
            .message
            .unwrap_or_else(|| "API route failed without an error message".to_string());
        let explanation = if let Some(stack) = result.stack {
            format!("{message}\n\n{stack}")
        } else {
            message
        };

        return Err(Diagnostic::new("RUV1200", "API route execution failed")
            .explain(format!("{code}: {explanation}"))
            .at_file(&route.file)
            .suggest("Check the route handler export and its imports.")
            .into());
    }

    let status = result.status.unwrap_or(200);
    let status = StatusCode::from_u16(status)
        .map_err(|error| RuvyxaError::Message(format!("Invalid API response status: {error}")))?;
    let body = result.body.unwrap_or_default();
    let mut response = (status, body).into_response();

    if let Some(headers) = result.headers {
        for (name, value) in headers {
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_str(&value) else {
                continue;
            };
            response.headers_mut().insert(name, value);
        }
    }

    Ok(with_security_headers(response))
}

fn action_error_code(code: Option<&str>) -> &'static str {
    match code {
        Some("RUV1501") => "RUV1501",
        Some("RUV1502") => "RUV1502",
        Some("RUV1503") => "RUV1503",
        _ => "RUV1500",
    }
}

fn action_file_for(route: &RouteEntry) -> Option<PathBuf> {
    let route_dir = route.file.parent()?;
    ["action.ts", "action.js"]
        .into_iter()
        .map(|name| route_dir.join(name))
        .find(|path| path.is_file())
}

fn find_route<'a>(
    manifest: &'a RouteManifest,
    request_path: &str,
) -> Option<router::RouteMatch<'a>> {
    RadixRouter::compile(manifest).find(manifest, request_path)
}

fn compose_document(rendered: &str, head_content: &str, hmr: &str) -> String {
    if contains_ascii_case(rendered, "<html") {
        let with_head = if contains_ascii_case(rendered, "<head") {
            insert_before_ascii_case(rendered, "</head>", head_content)
        } else if let Some(body_index) = find_ascii_case(rendered, "<body") {
            let mut document = String::with_capacity(rendered.len() + head_content.len() + 32);
            document.push_str(&rendered[..body_index]);
            document.push_str("<head>");
            document.push_str(head_content);
            document.push_str("</head>");
            document.push_str(&rendered[body_index..]);
            document
        } else {
            insert_after_opening_html(rendered, head_content)
        };

        return insert_before_ascii_case(&with_head, "</body>", hmr);
    }

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{head_content}</head><body>{rendered}{hmr}</body></html>"
    )
}

fn insert_after_opening_html(rendered: &str, head_content: &str) -> String {
    let Some(html_index) = find_ascii_case(rendered, "<html") else {
        return rendered.to_string();
    };
    let Some(close_index) = rendered[html_index..].find('>') else {
        return rendered.to_string();
    };
    let insert_index = html_index + close_index + 1;
    let mut document = String::with_capacity(rendered.len() + head_content.len() + 16);
    document.push_str(&rendered[..insert_index]);
    document.push_str("<head>");
    document.push_str(head_content);
    document.push_str("</head>");
    document.push_str(&rendered[insert_index..]);
    document
}

fn insert_before_ascii_case(input: &str, needle: &str, insertion: &str) -> String {
    let Some(index) = find_ascii_case(input, needle) else {
        let mut output = input.to_string();
        output.push_str(insertion);
        return output;
    };

    let mut output = String::with_capacity(input.len() + insertion.len());
    output.push_str(&input[..index]);
    output.push_str(insertion);
    output.push_str(&input[index..]);
    output
}

fn contains_ascii_case(input: &str, needle: &str) -> bool {
    find_ascii_case(input, needle).is_some()
}

fn find_ascii_case(input: &str, needle: &str) -> Option<usize> {
    input
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

#[derive(Debug, Deserialize)]
struct ClientAssetManifest {
    routes: Vec<ClientAssetRoute>,
}

#[derive(Debug, Deserialize)]
struct ClientAssetRoute {
    path: String,
    src: String,
    #[serde(default, rename = "sharedChunks")]
    shared_chunks: Vec<ClientSharedChunk>,
}

#[derive(Debug, Deserialize)]
struct ClientSharedChunk {
    src: String,
}

struct ClientAssets {
    src: String,
    preloads: Vec<String>,
}

fn client_hydration_script(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
) -> String {
    let params_json = serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string());
    let params_json = safe_json_for_script(&params_json);
    let request_path_json = safe_json_for_script(
        &serde_json::to_string(request_path).unwrap_or_else(|_| "\"/\"".to_string()),
    );
    let assets = if config.watch {
        ClientAssets {
            src: format!(
                "/__ruvyxa/client?path={}",
                url_encode_component(request_path)
            ),
            preloads: Vec::new(),
        }
    } else {
        prebuilt_client_assets(config, &route.path).unwrap_or_else(|| ClientAssets {
            src: format!(
                "/__ruvyxa/client?path={}",
                url_encode_component(request_path)
            ),
            preloads: Vec::new(),
        })
    };
    let preload_links = assets
        .preloads
        .iter()
        .map(|src| {
            let src = escape_html(src);
            format!(r#"<link rel="modulepreload" href="{src}">"#)
        })
        .collect::<String>();
    let src = escape_html(&assets.src);

    format!(
        r#"{preload_links}<script>globalThis.__RUVYXA_ROUTE_PARAMS__ = {params_json};globalThis.__RUVYXA_REQUEST_PATH__ = {request_path_json};</script><script type="module" src="{src}"></script>"#,
    )
}

fn prebuilt_client_assets(config: &ServerConfig, route_path: &str) -> Option<ClientAssets> {
    let source = fs::read_to_string(config.client_dir.join("manifest.json")).ok()?;
    let manifest: ClientAssetManifest = serde_json::from_str(&source).ok()?;
    manifest
        .routes
        .into_iter()
        .find(|route| route.path == route_path)
        .map(|route| ClientAssets {
            src: route.src,
            preloads: route
                .shared_chunks
                .into_iter()
                .map(|chunk| chunk.src)
                .collect(),
        })
}

fn safe_json_for_script(json: &str) -> String {
    json.replace("</", "<\\/")
}

fn hmr_client_script() -> &'static str {
    r#"<script>
(() => {
  const protocol = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${protocol}://${location.host}/__ruvyxa/hmr`);
  socket.addEventListener("message", (event) => {
    // A clean page load keeps the browser's ESM module graph and React root in sync.
    // This also covers route, CSS, and imported-module changes consistently.
    JSON.parse(event.data);
    location.reload();
  });
})();
</script>"#
}

#[cfg(test)]
fn classify_hmr_event(paths: &[PathBuf]) -> &'static str {
    if paths.is_empty() {
        return "full-reload";
    }

    if paths.iter().all(|path| extension_is(path, "css")) {
        return "css-update";
    }

    let has_component = paths.iter().any(|path| {
        ["tsx", "jsx", "ts", "js", "md", "mdx"]
            .into_iter()
            .any(|extension| extension_is(path, extension))
            && path.components().any(|component| {
                let segment = component.as_os_str().to_string_lossy();
                segment == "app" || segment == "components"
            })
    });

    if has_component {
        "component-update"
    } else {
        "full-reload"
    }
}

#[cfg(test)]
fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

struct ActionRateLimiter {
    hits: HashMap<String, Vec<Instant>>,
    max_hits: usize,
    window: Duration,
    max_keys: usize,
}

impl ActionRateLimiter {
    fn new(max_hits: usize, window: Duration) -> Self {
        Self {
            hits: HashMap::new(),
            max_hits,
            window,
            max_keys: MAX_TRACKED_ACTION_RATE_LIMIT_KEYS,
        }
    }

    fn allow(&mut self, key: &str) -> bool {
        let now = Instant::now();
        self.hits.retain(|_, hits| {
            hits.retain(|hit| now.duration_since(*hit) <= self.window);
            !hits.is_empty()
        });
        if !self.hits.contains_key(key) && self.hits.len() >= self.max_keys {
            return false;
        }
        let hits = self.hits.entry(key.to_string()).or_default();

        if hits.len() >= self.max_hits {
            return false;
        }

        hits.push(now);
        true
    }

    fn retry_after_seconds(&self, key: &str) -> u64 {
        self.hits
            .get(key)
            .and_then(|hits| hits.first())
            .map(|first| self.window.saturating_sub(first.elapsed()).as_secs().max(1))
            .unwrap_or(1)
    }
}

fn validate_action_request(
    headers: &HeaderMap,
    body_len: usize,
    config: &ServerConfig,
) -> Option<Response> {
    if body_len > config.action_body_limit_bytes {
        return Some(
            (StatusCode::PAYLOAD_TOO_LARGE, "Action payload is too large").into_response(),
        );
    }

    if !action_content_type_is_supported(headers) {
        return Some(
            (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Action payload must be JSON or URL-encoded form data",
            )
                .into_response(),
        );
    }

    if config.same_origin_actions && action_origin_is_cross_site(headers) {
        return Some(
            (StatusCode::FORBIDDEN, "Cross-origin action request blocked").into_response(),
        );
    }

    if config.fetch_metadata_actions && action_fetch_site_is_cross_site(headers) {
        return Some((StatusCode::FORBIDDEN, "Cross-site action request blocked").into_response());
    }

    None
}

fn action_content_type_is_supported(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };

    let content_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    matches!(
        content_type.as_str(),
        "application/json" | "application/x-www-form-urlencoded"
    )
}

fn action_origin_is_cross_site(headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    let Some((origin_scheme, origin_host)) = origin
        .split_once("://")
        .filter(|(_, value)| !value.contains('/') && !value.is_empty())
    else {
        return true;
    };

    let expected_scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| matches!(*value, "http" | "https"))
        .unwrap_or("http");

    !origin_host.eq_ignore_ascii_case(host) || !origin_scheme.eq_ignore_ascii_case(expected_scheme)
}

fn action_fetch_site_is_cross_site(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
}

fn action_rate_limit_key(peer: SocketAddr, headers: &HeaderMap, query: &ActionQuery) -> String {
    let peer_ip = peer.ip();

    // Only trust forwarded headers when the direct peer is a loopback or
    // private address (i.e., a trusted reverse proxy). This prevents clients
    // from spoofing X-Forwarded-For to bypass rate limiting.
    let client: String = if is_trusted_proxy_ip(peer_ip) {
        headers
            .get("x-forwarded-for")
            .or_else(|| headers.get("x-real-ip"))
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(|s| s.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| peer_ip.to_string())
    } else {
        peer_ip.to_string()
    };

    format!("{client}:{}:{}", query.path, query.name)
}

/// Returns true if the IP is a loopback or private network address,
/// indicating the connection comes from a local reverse proxy.
fn is_trusted_proxy_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    }
}

fn url_encode_component(input: &str) -> String {
    let mut output = String::new();

    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                output.push(byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }

    output
}

fn extract_code_frame(file: &Path, line: Option<u32>) -> Option<String> {
    let line = line?;
    let source = fs::read_to_string(file).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    let total = lines.len();
    let idx = line.saturating_sub(1) as usize;
    if idx >= total {
        return None;
    }
    let start = idx.saturating_sub(2);
    let end = (idx + 3).min(total);
    let mut frame = String::new();
    let max_digits = end.to_string().len().max(2);
    for (i, line_text) in lines[start..end].iter().enumerate() {
        let i = start + i;
        let num = i + 1;
        let prefix = if i == idx { ">" } else { " " };
        let marker = if i == idx { "  ← error" } else { "" };
        frame.push_str(&format!(
            " {prefix} {:>width$} │ {}{}\n",
            num,
            line_text,
            marker,
            width = max_digits
        ));
    }
    Some(frame)
}

fn error_response(status: StatusCode, diagnostics: &Diagnostic, is_dev: bool) -> Response {
    if !is_dev {
        return html_response(status, plain_error_page(&diagnostics.to_string()));
    }
    let code_frame = diagnostics
        .span
        .as_ref()
        .and_then(|span| extract_code_frame(&span.file, span.line));
    let body = dev_diagnostic_overlay(diagnostics, code_frame.as_deref());
    html_response(status, body)
}

fn error_page(message: &str, show_overlay: bool) -> String {
    if show_overlay {
        dev_error_overlay(message, None, None, None)
    } else {
        plain_error_page(message)
    }
}

fn plain_error_page(message: &str) -> String {
    let not_found = message.contains("Route not found");
    let code = if not_found { "404" } else { "500" };
    let title = if not_found {
        "This page could not be found."
    } else {
        "Ruvyxa hit an unexpected error."
    };

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>Ruvyxa Error - {code}</title>
<style>
  :root {{ color-scheme: light; --bg: #18181c; --ink: #4c1d95; --muted: #6d4b8f; --accent: #7c3aed; --line: rgba(124,58,237,.28); }}
  *, *::before, *::after {{ box-sizing: border-box; }}
  html, body {{ min-height: 100%; }}
  body {{ display: grid; min-height: 100vh; place-items: center; margin: 0; padding: 28px; color: var(--ink); background: radial-gradient(circle at 50% 38%, rgba(111, 65, 143, .18), transparent 34rem), var(--bg); font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  .error-card {{ width: min(760px, 100%); padding: clamp(30px, 6vw, 66px); border: 1px solid rgba(124,58,237,.16); border-radius: 24px; background: #fff; box-shadow: 0 28px 90px rgba(0,0,0,.38), 0 0 0 1px rgba(255,255,255,.7) inset; text-align: center; }}
  .logo {{ display: block; width: clamp(82px, 15vw, 132px); height: clamp(82px, 15vw, 132px); margin: 0 auto 28px; object-fit: contain; filter: drop-shadow(0 12px 22px rgba(123, 62, 226, .3)); }}
  .status {{ display: inline-flex; align-items: center; justify-content: center; gap: clamp(14px, 3vw, 34px); margin: 0 auto 18px; }}
  .code {{ color: var(--accent); font: 800 clamp(36px, 7vw, 58px)/1 ui-monospace, SFMono-Regular, Consolas, monospace; letter-spacing: -.06em; }}
  .divider {{ width: 1px; height: 62px; background: var(--line); }}
  h1 {{ margin: 0; color: var(--ink); font-size: clamp(22px, 4vw, 34px); font-weight: 520; letter-spacing: -.035em; }}
  .message {{ max-width: 620px; margin: 18px auto 0; color: var(--muted); font: 15px/1.7 ui-monospace, SFMono-Regular, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }}
  .path-label {{ display: inline-block; margin-top: 20px; padding: 6px 12px; border: 1px solid rgba(124,58,237,.2); border-radius: 999px; color: #6d28d9; background: #f4efff; font-size: clamp(13px, 1.8vw, 16px); font-weight: 700; letter-spacing: .06em; text-transform: uppercase; text-shadow: 0 1px 0 rgba(255,255,255,.8); }}
  @media (max-width: 560px) {{ body {{ padding: 16px; }} .error-card {{ padding: 34px 22px; border-radius: 18px; }} .status {{ flex-direction: column; gap: 12px; }} .code {{ font-size: clamp(42px, 14vw, 54px); }} .divider {{ width: 64px; height: 1px; }} h1 {{ max-width: 260px; text-align: center; }} }}
</style>
</head>
<body>
<main class="error-card" aria-labelledby="error-title">
  <img class="logo" src="/ruvyxa.png" alt="Ruvyxa">
  <div class="status" aria-label="Error status">
    <span class="code">{code}</span>
    <span class="divider" aria-hidden="true"></span>
    <h1 id="error-title">{title}</h1>
  </div>
  <pre class="message">{}</pre>
  <div class="path-label">Ruvyxa Error</div>
</main>
</body>
</html>"##,
        escape_html(message)
    )
}

fn dev_error_overlay(
    message: &str,
    code_frame: Option<&str>,
    stack: Option<&str>,
    suggestion: Option<&str>,
) -> String {
    let mut lines = message.lines();
    let title = lines.next().unwrap_or("Unhandled Runtime Error");
    let detail = lines.collect::<Vec<_>>().join("\n");
    render_error_overlay(ErrorOverlayView {
        code: "RUV_RUNTIME",
        title,
        detail: if detail.trim().is_empty() {
            message
        } else {
            &detail
        },
        location: None,
        code_frame,
        stack,
        suggestion,
        import_chain: &[],
        affected_routes: &[],
    })
}

fn dev_diagnostic_overlay(diagnostic: &Diagnostic, code_frame: Option<&str>) -> String {
    let location = diagnostic
        .span
        .as_ref()
        .map(|span| match (span.line, span.column) {
            (Some(line), Some(column)) => format!("{}:{line}:{column}", span.file.display()),
            (Some(line), None) => format!("{}:{line}", span.file.display()),
            _ => span.file.display().to_string(),
        });
    render_error_overlay(ErrorOverlayView {
        code: diagnostic.code,
        title: &diagnostic.title,
        detail: &diagnostic.explanation,
        location: location.as_deref(),
        code_frame,
        stack: None,
        suggestion: diagnostic.suggested_fix.as_deref(),
        import_chain: &diagnostic.import_chain,
        affected_routes: &diagnostic.affected_routes,
    })
}

struct ErrorOverlayView<'a> {
    code: &'a str,
    title: &'a str,
    detail: &'a str,
    location: Option<&'a str>,
    code_frame: Option<&'a str>,
    stack: Option<&'a str>,
    suggestion: Option<&'a str>,
    import_chain: &'a [PathBuf],
    affected_routes: &'a [String],
}

fn render_error_overlay(view: ErrorOverlayView<'_>) -> String {
    let ErrorOverlayView {
        code,
        title,
        detail,
        location,
        code_frame,
        stack,
        suggestion,
        import_chain,
        affected_routes,
    } = view;
    let frame_html = code_frame
        .map(|f| {
            format!(
                r#"<section class="source"><div class="source-head"><span>Source</span><code>{}</code></div><pre>{}</pre></section>"#,
                escape_html(location.unwrap_or("source unavailable")),
                escape_html(f)
            )
        })
        .unwrap_or_default();
    let stack_html = stack
        .map(|s| {
            format!(
                r#"<details><summary>Stack trace</summary><pre>{}</pre></details>"#,
                escape_html(s)
            )
        })
        .unwrap_or_default();
    let suggestion_html = suggestion
        .map(|s| {
            format!(
                r#"<section class="hint"><strong>Suggested fix</strong><p>{}</p></section>"#,
                escape_html(s)
            )
        })
        .unwrap_or_default();
    let location_html = location
        .map(|location| format!(r#"<div class="location">{}</div>"#, escape_html(location)))
        .unwrap_or_default();
    let import_chain_html = if import_chain.is_empty() {
        String::new()
    } else {
        format!(
            r#"<details open><summary>Import chain ({})</summary><ol>{}</ol></details>"#,
            import_chain.len(),
            import_chain
                .iter()
                .map(|path| format!(
                    "<li><code>{}</code></li>",
                    escape_html(&path.display().to_string())
                ))
                .collect::<String>()
        )
    };
    let routes_html = if affected_routes.is_empty() {
        String::new()
    } else {
        format!(
            r#"<details open><summary>Affected routes ({})</summary><ul>{}</ul></details>"#,
            affected_routes.len(),
            affected_routes
                .iter()
                .map(|route| format!("<li><code>{}</code></li>", escape_html(route)))
                .collect::<String>()
        )
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Ruvyxa Error - {title}</title>
<style>
  *, *::before, *::after {{ box-sizing: border-box; }}
  :root {{ color-scheme: light; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  body {{
    margin: 0;
    min-height: 100vh;
    color: #171717;
    background: linear-gradient(135deg, #f1f1f1, #d9d9d9);
  }}
  .backdrop {{
    min-height: 100vh;
    padding: clamp(16px, 5vw, 64px);
    background: rgba(245, 245, 245, .76);
    backdrop-filter: blur(9px);
  }}
  .dialog {{
    width: min(920px, 100%);
    margin: 0 auto;
    background: #fff;
    border: 1px solid #d7d7d7;
    border-top: 3px solid #ef5b5b;
    border-radius: 8px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, .2);
    overflow: hidden;
  }}
  .toolbar {{
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 46px;
    padding: 0 14px;
    border-bottom: 1px solid #ececec;
    color: #6b6b6b;
    font-size: 12px;
  }}
  .toolbar button {{ border: 0; background: transparent; color: #707070; font-size: 22px; cursor: pointer; padding: 4px 8px; }}
  .content {{ padding: clamp(20px, 4vw, 40px); }}
  .eyebrow {{ color: #d53535; font: 700 12px/1.4 ui-monospace, SFMono-Regular, Consolas, monospace; letter-spacing: .06em; }}
  h1 {{ margin: 8px 0 6px; font-size: clamp(20px, 3vw, 28px); line-height: 1.25; }}
  .location {{ color: #b4232d; font: 500 13px/1.5 ui-monospace, SFMono-Regular, Consolas, monospace; overflow-wrap: anywhere; }}
  .detail {{ margin: 18px 0 24px; color: #424242; white-space: pre-wrap; overflow-wrap: anywhere; }}
  .source {{ margin: 20px 0; border: 1px solid #222; border-radius: 6px; overflow: hidden; background: #101010; color: #f5f5f5; }}
  .source-head {{ display: flex; justify-content: space-between; gap: 16px; padding: 8px 12px; border-bottom: 1px solid #333; color: #d7d7d7; font-size: 12px; }}
  .source-head code {{ color: #a8a8a8; overflow-wrap: anywhere; text-align: right; }}
  .source pre {{ margin: 0; padding: 16px; overflow: auto; color: #f3f3f3; font: 13px/1.6 ui-monospace, SFMono-Regular, Consolas, monospace; tab-size: 2; }}
  .hint {{ margin: 18px 0; padding: 14px 16px; border: 1px solid #9dd5ab; border-left: 4px solid #2f9e44; border-radius: 6px; background: #f3fbf5; }}
  .hint strong {{ color: #176b2c; }}
  .hint p {{ margin: 5px 0 0; color: #285b35; white-space: pre-wrap; }}
  details {{ margin-top: 12px; border: 1px solid #e2e2e2; border-radius: 6px; padding: 10px 12px; }}
  summary {{ cursor: pointer; font-weight: 650; }}
  details pre {{ overflow: auto; white-space: pre-wrap; color: #454545; font: 12px/1.55 ui-monospace, SFMono-Regular, Consolas, monospace; }}
  details ol, details ul {{ margin-bottom: 0; padding-left: 24px; }}
  details li {{ margin: 5px 0; overflow-wrap: anywhere; }}
  .footer {{ padding: 12px 20px; border-top: 1px solid #ececec; background: #fafafa; color: #777; font-size: 12px; text-align: center; }}
  @media (max-width: 600px) {{
    .backdrop {{ padding: 0; }}
    .dialog {{ min-height: 100vh; border-radius: 0; border-left: 0; border-right: 0; }}
    .source-head {{ flex-direction: column; }}
    .source-head code {{ text-align: left; }}
  }}
</style>
</head>
<body>
<main class="backdrop">
  <section class="dialog" id="ruvyxa-error-overlay" role="dialog" aria-modal="true" aria-labelledby="ruvyxa-error-title">
    <div class="toolbar"><span>‹ &nbsp; 1 of 1 unhandled error &nbsp; ›</span><button type="button" aria-label="Close error overlay" onclick="document.getElementById('ruvyxa-error-overlay').hidden=true">×</button></div>
    <div class="content">
      <div class="eyebrow">{code}</div>
      <h1 id="ruvyxa-error-title">{title}</h1>
      {location_html}
      <div class="detail">{detail}</div>
      {frame_html}
      {suggestion_html}
      {import_chain_html}
      {routes_html}
      {stack_html}
    </div>
    <div class="footer">Ruvyxa Dev Server — fix the error and save to hot-reload</div>
  </section>
</main>
</body>
</html>"#,
        code = escape_html(code),
        title = escape_html(title),
        detail = escape_html(detail),
    )
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("webmanifest") => "application/manifest+json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        _ => "application/octet-stream",
    }
}

fn public_asset_links(public_dir: &Path) -> String {
    let mut links = Vec::new();

    if public_dir.join("ruvyxa.png").exists() {
        links.push(r#"<link rel="icon" type="image/png" href="/ruvyxa.png">"#.to_string());
    }

    links.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composes_react_rendered_html_documents() {
        let rendered = r#"<!doctype html><html lang="en"><body><main>Hello</main></body></html>"#;
        let html = compose_document(
            rendered,
            r#"<link rel="icon" href="/ruvyxa.png">"#,
            "<script />",
        );

        assert!(html.contains(r#"<head><link rel="icon" href="/ruvyxa.png"></head>"#));
        assert!(html.contains("<script /></body>"));
    }

    #[test]
    fn diagnostic_overlay_renders_complete_escaped_context() {
        let mut diagnostic = Diagnostic::new("RUV1300", "Compile <error>")
            .explain("Unexpected </script> token")
            .at_file_with_span("app/page.tsx", 8, 15)
            .suggest("Close the JSX element");
        diagnostic.import_chain = vec![PathBuf::from("app/layout.tsx")];
        diagnostic.affected_routes = vec!["/docs?<unsafe>".to_string()];

        let html = dev_diagnostic_overlay(
            &diagnostic,
            Some("   8 │ return <main>\n     │              ^"),
        );

        assert!(html.contains("RUV1300"));
        assert!(html.contains("app/page.tsx:8:15"));
        assert!(html.contains("Suggested fix"));
        assert!(html.contains("Import chain (1)"));
        assert!(html.contains("Affected routes (1)"));
        assert!(html.contains("return &lt;main&gt;"));
        assert!(!html.contains("<script> token"));
        assert!(html.contains("&lt;/script&gt; token"));
        assert!(html.contains("/docs?&lt;unsafe&gt;"));
    }

    #[test]
    fn runtime_overlay_matches_modal_error_interaction() {
        let html = dev_error_overlay(
            "Unhandled Runtime Error\nFailed to load script",
            None,
            Some("at Page (page.tsx:2:1)"),
            None,
        );
        assert!(html.contains("1 of 1 unhandled error"));
        assert!(html.contains("role=\"dialog\""));
        assert!(html.contains("RUV_RUNTIME"));
        assert!(html.contains("Stack trace"));
        assert!(html.contains("Close error overlay"));
    }

    #[test]
    fn plain_error_page_escapes_message() {
        let html = plain_error_page("<script>alert(1)</script>");

        assert!(html.contains("<main class=\"error-card\""));
        assert!(html.contains("src=\"/ruvyxa.png\""));
        assert!(html.contains("500"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }

    #[test]
    fn plain_error_page_uses_centered_404_state_and_logo() {
        let html = plain_error_page("Route not found");

        assert!(html.contains("<main class=\"error-card\""));
        assert!(html.contains("<span class=\"code\">404</span>"));
        assert!(html.contains("src=\"/ruvyxa.png\""));
        assert!(html.contains("This page could not be found."));
    }

    #[tokio::test]
    async fn builds_runtime_trace_for_matched_routes() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app/blog/[slug]");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("page.tsx"),
            "export default function BlogPost() { return <main /> }",
        )
        .unwrap();
        std::fs::write(app.join("action.ts"), "export const save = {}").unwrap();

        let config = ServerConfig::dev(temp.path(), "localhost", 3000);
        let trace = runtime_trace_cached(&config, &RuntimeCache::default(), "/blog/hello")
            .await
            .unwrap();

        assert!(trace.matched);
        assert_eq!(trace.params.get("slug"), Some(&"hello".to_string()));
        assert_eq!(trace.runtime, "dev");
        assert!(trace.route.unwrap().server_modules[0].ends_with("action.ts"));
    }

    #[test]
    fn parses_env_sources() {
        let env = parse_env_source(
            r#"
            # ignored
            RUVYXA_PUBLIC_APP_NAME="Ruvyxa"
            DATABASE_URL='postgres://localhost/db'
            EMPTY=
            INVALID
            "#,
        );

        assert_eq!(
            env.get("RUVYXA_PUBLIC_APP_NAME"),
            Some(&"Ruvyxa".to_string())
        );
        assert_eq!(
            env.get("DATABASE_URL"),
            Some(&"postgres://localhost/db".to_string())
        );
        assert_eq!(env.get("EMPTY"), Some(&"".to_string()));
        assert!(!env.contains_key("INVALID"));
    }

    #[test]
    fn classifies_hmr_events_by_changed_file_type() {
        assert_eq!(
            classify_hmr_event(&[PathBuf::from("app/global.css")]),
            "css-update"
        );
        assert_eq!(
            classify_hmr_event(&[PathBuf::from("components/Nav.tsx")]),
            "component-update"
        );
        assert_eq!(
            classify_hmr_event(&[PathBuf::from("server/db.ts")]),
            "full-reload"
        );
        assert_eq!(
            classify_hmr_event(&[PathBuf::from("app/docs/page.mdx")]),
            "component-update"
        );
        assert!(page_has_default_export(
            Path::new("app/docs/page.mdx"),
            "# Content"
        ));
        assert!(!page_has_default_export(
            Path::new("app/docs/page.tsx"),
            "export const title = 'Missing'"
        ));
    }

    #[test]
    fn blocks_cross_origin_action_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );

        assert!(action_origin_is_cross_site(&headers));
    }

    #[test]
    fn accepts_same_origin_action_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:3000"),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        assert!(!action_origin_is_cross_site(&headers));
        assert!(action_content_type_is_supported(&headers));
        assert!(
            validate_action_request(&headers, 128, &ServerConfig::dev(".", "localhost", 3000))
                .is_none()
        );
    }

    #[test]
    fn blocks_cross_scheme_action_requests_without_trusted_proxy_protocol() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://localhost:3000"),
        );

        assert!(action_origin_is_cross_site(&headers));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(!action_origin_is_cross_site(&headers));
    }

    #[test]
    fn action_rate_limiter_bounds_tracked_keys() {
        let mut limiter = ActionRateLimiter::new(1, Duration::from_secs(60));
        limiter.max_keys = 2;
        assert!(limiter.allow("first"));
        assert!(limiter.allow("second"));
        assert!(!limiter.allow("third"));
        assert!(limiter.retry_after_seconds("first") >= 59);
    }

    #[test]
    fn plugin_responses_reject_invalid_headers() {
        let response = PluginResponse {
            status: 200,
            headers: vec![("bad header".to_string(), "value".to_string())],
            body: Some(b"body".to_vec()),
        };
        assert!(plugin_response_into_response(response).is_err());
    }

    #[test]
    fn action_security_options_control_request_validation() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));

        let mut config = ServerConfig::dev(".", "localhost", 3000);
        config.action_body_limit_bytes = 8;
        assert!(validate_action_request(&headers, 9, &config).is_some());

        config.action_body_limit_bytes = MAX_ACTION_BODY_BYTES;
        config.same_origin_actions = false;
        config.fetch_metadata_actions = false;
        assert!(validate_action_request(&headers, 8, &config).is_none());
    }

    #[test]
    fn rejects_unsafe_public_asset_paths() {
        assert!(is_safe_relative_path("images/logo.png"));
        assert!(is_safe_relative_path("./images/logo.png"));
        assert!(!is_safe_relative_path(""));
        assert!(!is_safe_relative_path("../secret.txt"));
        assert!(!is_safe_relative_path("images\\logo.png"));
    }

    #[test]
    fn resolves_single_webp_outputs_and_development_sources() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("hero.png"), b"png").unwrap();
        let (fallback, vary) = resolve_public_asset(temp.path(), "hero.webp", None).unwrap();
        assert!(fallback.ends_with("hero.png"));
        assert!(!vary);

        fs::remove_file(temp.path().join("hero.png")).unwrap();
        fs::write(temp.path().join("hero.webp"), b"webp").unwrap();
        let (selected, vary) = resolve_public_asset(temp.path(), "hero.png", None).unwrap();
        assert!(selected.ends_with("hero.webp"));
        assert!(!vary);
    }

    #[test]
    fn rejects_ambiguous_development_image_sources() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("hero.png"), b"png").unwrap();
        fs::write(temp.path().join("hero.jpg"), b"jpg").unwrap();
        assert!(resolve_public_asset(temp.path(), "hero.webp", None).is_none());
    }

    #[test]
    fn resolves_uppercase_development_image_extensions() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("hero.PNG"), b"png").unwrap();
        let (source, _) = resolve_public_asset(temp.path(), "hero.webp", None).unwrap();
        assert!(source.ends_with("hero.PNG"));
    }

    #[test]
    fn rejects_public_assets_outside_the_configured_root() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        fs::create_dir_all(&public).unwrap();
        fs::write(temp.path().join("secret.txt"), b"secret").unwrap();

        assert!(resolve_public_asset(&public, "../secret.txt", None).is_none());
    }

    #[test]
    fn applies_default_security_headers() {
        let response = html_response(StatusCode::OK, "<main />".to_string());

        assert_eq!(
            response.headers().get(header::X_CONTENT_TYPE_OPTIONS),
            Some(&HeaderValue::from_static("nosniff"))
        );
        assert_eq!(
            response.headers().get("referrer-policy"),
            Some(&HeaderValue::from_static("strict-origin-when-cross-origin"))
        );
    }

    #[test]
    fn can_disable_default_security_headers() {
        let response = finalize_security_headers(StatusCode::OK.into_response(), false);

        assert!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .is_none()
        );
        assert!(response.headers().get("referrer-policy").is_none());
    }

    #[test]
    fn default_security_headers_preserve_websocket_upgrade_headers() {
        let mut response = StatusCode::SWITCHING_PROTOCOLS.into_response();
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        response
            .headers_mut()
            .insert(header::UPGRADE, HeaderValue::from_static("websocket"));

        let response = finalize_security_headers(response, true);

        assert_eq!(
            response.headers().get(header::CONNECTION),
            Some(&HeaderValue::from_static("Upgrade"))
        );
        assert_eq!(
            response.headers().get(header::UPGRADE),
            Some(&HeaderValue::from_static("websocket"))
        );
    }

    #[test]
    fn blocks_cross_site_fetch_metadata_for_actions() {
        let mut headers = HeaderMap::new();
        headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));

        assert!(action_fetch_site_is_cross_site(&headers));
        assert!(
            validate_action_request(&headers, 128, &ServerConfig::dev(".", "localhost", 3000))
                .is_some()
        );
    }

    #[test]
    fn rate_limits_action_keys() {
        let mut limiter = ActionRateLimiter::new(ACTION_RATE_LIMIT_MAX, ACTION_RATE_LIMIT_WINDOW);

        for _ in 0..ACTION_RATE_LIMIT_MAX {
            assert!(limiter.allow("local:/todos:createTodo"));
        }

        assert!(!limiter.allow("local:/todos:createTodo"));
        assert!(limiter.allow("local:/other:createTodo"));
    }

    #[test]
    fn reads_prebuilt_client_assets_from_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            client_dir.join("manifest.json"),
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.123.js"}]}]}"#,
        )
        .unwrap();

        let config = ServerConfig::production(temp.path(), "localhost", 3000);

        let assets = prebuilt_client_assets(&config, "/").unwrap();
        assert_eq!(assets.src, "/__ruvyxa/client/home.js");
        assert_eq!(assets.preloads, vec!["/__ruvyxa/client/shared.123.js"]);

        std::fs::write(
            client_dir.join("manifest.json"),
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/legacy.js"}]}"#,
        )
        .unwrap();
        let legacy_assets = prebuilt_client_assets(&config, "/").unwrap();
        assert_eq!(legacy_assets.src, "/__ruvyxa/client/legacy.js");
        assert!(legacy_assets.preloads.is_empty());
    }

    #[test]
    fn hydration_script_preloads_route_shared_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            client_dir.join("manifest.json"),
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.123.js"}]}]}"#,
        )
        .unwrap();
        let config = ServerConfig::production(temp.path(), "localhost", 3000);
        let route = RouteEntry {
            id: "page:index".to_string(),
            path: "/".to_string(),
            file: temp.path().join("app/page.tsx"),
            kind: ruvyxa_graph::RouteKind::Page,
            layout_chain: Vec::new(),
            server_modules: Vec::new(),
            client_modules: Vec::new(),
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: Default::default(),
        };

        let script = client_hydration_script(&config, &route, "/", &BTreeMap::new());

        assert!(
            script.contains(r#"<link rel="modulepreload" href="/__ruvyxa/client/shared.123.js">"#)
        );
        assert!(script.contains(r#"<script type="module" src="/__ruvyxa/client/home.js">"#));
    }

    #[tokio::test]
    async fn runtime_cache_reuses_manifest_until_invalidated() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("page.tsx"),
            "export default function Home() { return <main /> }",
        )
        .unwrap();

        let config = ServerConfig::dev(temp.path(), "localhost", 3000);
        let cache = RuntimeCache::default();

        assert_eq!(cache.manifest(&config).await.unwrap().routes.len(), 1);

        let about = app.join("about");
        std::fs::create_dir_all(&about).unwrap();
        std::fs::write(
            about.join("page.tsx"),
            "export default function About() { return <main /> }",
        )
        .unwrap();

        assert_eq!(cache.manifest(&config).await.unwrap().routes.len(), 1);
        cache.invalidate_async().await;
        assert_eq!(cache.manifest(&config).await.unwrap().routes.len(), 2);
    }

    #[test]
    fn dependency_prebundle_plan_includes_pages_only_when_enabled_in_dev() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        std::fs::create_dir_all(app.join("api/health")).unwrap();
        std::fs::write(
            app.join("page.tsx"),
            "export default function Home() { return <main /> }",
        )
        .unwrap();
        std::fs::write(
            app.join("api/health/route.ts"),
            "export function GET() { return Response.json({ ok: true }) }",
        )
        .unwrap();
        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();

        let mut dev = ServerConfig::dev(temp.path(), "localhost", 3000);
        let routes = dependency_warmup_routes(&dev, &manifest);
        assert_eq!(routes.len(), 1);
        assert!(routes[0].page_file.ends_with("page.tsx"));
        assert_eq!(routes[0].app_dir, app.display().to_string());

        dev.prebundle_dependencies = false;
        assert!(dependency_warmup_routes(&dev, &manifest).is_empty());

        let production = ServerConfig::production(temp.path(), "localhost", 3000);
        assert!(dependency_warmup_routes(&production, &manifest).is_empty());
    }

    #[test]
    fn local_display_url_prefers_localhost_for_loopback() {
        let config = ServerConfig::dev(".", "localhost", 3001);
        let address = "[::1]:3001".parse().unwrap();

        assert_eq!(local_display_url(&config, address), "http://localhost:3001");
    }

    #[test]
    fn runtime_traces_require_both_dev_mode_and_debug_flag() {
        let mut dev = ServerConfig::dev(".", "localhost", 3000);
        assert!(!debug_traces_enabled(&dev));
        dev.debug_traces = true;
        assert!(debug_traces_enabled(&dev));

        let mut production = ServerConfig::production(".", "localhost", 3000);
        production.debug_traces = true;
        assert!(!debug_traces_enabled(&production));
    }

    #[tokio::test]
    async fn bind_listener_uses_next_available_port_when_requested_port_is_busy() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_address = occupied.local_addr().unwrap();
        if occupied_address.port() == u16::MAX {
            return;
        }

        let config = ServerConfig::dev(".", "127.0.0.1", occupied_address.port());
        let (_listener, bound_address) = bind_listener(&config, occupied_address).await.unwrap();

        assert!(bound_address.port() > occupied_address.port());
        assert!(
            bound_address.port()
                <= occupied_address
                    .port()
                    .saturating_add(PORT_FALLBACK_SCAN_LIMIT)
        );
    }

    #[test]
    fn port_conflict_diagnostic_reports_scanned_range() {
        let config = ServerConfig::dev(".", "localhost", 3000);
        let address = "127.0.0.1:3000".parse().unwrap();
        let error = std::io::Error::new(std::io::ErrorKind::AddrInUse, "in use");
        let diagnostic = port_conflict_diagnostic(&config, address, &error);

        assert_eq!(diagnostic.code, "RUV1201");
        assert!(diagnostic.explanation.contains("localhost:3000"));
        assert!(diagnostic.explanation.contains("3100"));
        assert!(
            diagnostic
                .suggested_fix
                .as_deref()
                .unwrap()
                .contains("3000-3100")
        );
    }

    #[test]
    fn watches_the_project_root_for_imported_modules_and_styles() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let styles = temp.path().join("styles");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(app.join("page.tsx"), "import '../styles/site.css'").unwrap();
        std::fs::write(styles.join("site.css"), "body { color: green; }").unwrap();
        let config = ServerConfig::dev(temp.path(), "localhost", 3000);

        assert_eq!(watch_paths(&config), vec![temp.path().to_path_buf()]);
        assert!(!ignored_watch_path(temp.path(), &styles.join("site.css")));
        assert!(!ignored_watch_path(
            temp.path(),
            &temp.path().join("lib/utils.ts")
        ));
        assert!(ignored_watch_path(
            temp.path(),
            &temp.path().join("node_modules/react/index.js")
        ));
        assert!(ignored_watch_path(
            temp.path(),
            &temp.path().join(".ruvyxa/cache/client.js")
        ));
        assert!(ignored_watch_path(
            temp.path(),
            &Path::new(".")
                .join(".ruvyxa")
                .join("cache")
                .join("ssr")
                .join("page.mjs")
        ));
        assert!(ignored_watch_path(
            temp.path(),
            &temp
                .path()
                .join(".ruvyxa-action-test-BW9IHB")
                .join("app/todos/action.ts")
        ));
        assert!(!ignored_watch_path(
            temp.path(),
            &temp.path().join("app/.ruvyxa-action-test-helper.ts")
        ));
    }

    #[test]
    fn dev_hmr_logs_keep_submillisecond_timing_visible() {
        assert_eq!(format_update_elapsed(Duration::from_micros(42)), "0.1ms");
        assert_eq!(format_update_elapsed(Duration::from_millis(1)), "1ms");
    }

    #[test]
    fn dev_request_logs_include_route_methods_without_asset_noise() {
        // Disable ANSI colors so the assertion compares plain text regardless of
        // whether the test runner's stdout is detected as a terminal.
        // SAFETY: This test is not run in parallel with others that depend on NO_COLOR.
        unsafe { std::env::set_var("NO_COLOR", "1") };

        assert!(should_log_dev_request("/"));
        assert!(should_log_dev_request("/api/echo"));
        assert!(!should_log_dev_request("/app.js"));
        assert!(!should_log_dev_request("/images/logo.webp"));
        assert!(!should_log_dev_request("/__ruvyxa/client"));

        assert_eq!(
            dev_page_request_log("GET", "/about", StatusCode::OK, Duration::from_micros(420),),
            "◌ GET /about → 200 · 0.5ms"
        );

        unsafe { std::env::remove_var("NO_COLOR") };
    }

    #[test]
    fn hmr_client_reloads_for_every_update() {
        let script = hmr_client_script();
        assert!(script.contains("JSON.parse(event.data);"));
        assert!(script.contains("location.reload();"));
        assert!(!script.contains("document.createElement(\"script\")"));
    }
}
