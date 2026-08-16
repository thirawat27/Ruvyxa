#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::{BTreeSet, HashSet};
#[cfg(test)]
use std::fs;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ruvyxa_bundler::JsxRuntime;
#[cfg(test)]
use ruvyxa_diagnostics::Diagnostic;
use ruvyxa_diagnostics::{Result, RuvyxaError};
use ruvyxa_graph::{
    DiscoverOptions, I18nRouting, RenderStrategy, RouteEntry, RouteKind, RouteManifest,
    RouteParams, discover_routes,
};
#[cfg(test)]
use ruvyxa_middleware::PluginHttpResponse;
use ruvyxa_middleware::{MiddlewareConfig, MiddlewareStack, PluginHost, PluginHttpRequest};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

mod collab;
use collab::{CollabRegistry, FrameRateLimiter, ParsedFrame, parse_client_frame};
mod devtools;
mod dynamic_image;
mod env_file;

/// Pixel conversion, SIMD resizing, and WebP encoding.
///
/// Public because the build-time optimizer in `ruvyxa_cli` runs the same
/// pipeline over the same formats. Two copies would be free to drift on the
/// details that decide output bytes — pixel layout, filter, encoder settings —
/// and a build/runtime mismatch there is invisible until someone compares two
/// renderings of the same asset.
pub mod image_codec;
#[cfg(test)]
use env_file::parse_env_source;
pub use env_file::project_env;

mod action_security;
use action_security::{
    ActionRateLimiter, ActionReplayGuard, action_rate_limit_key, hmr_origin_is_cross_site,
    validate_action_payload, validate_action_request,
};
pub use action_security::{IpPrefix, TrustedProxies, action_reference_id};
#[cfg(test)]
use action_security::{
    action_content_type_is_supported, action_fetch_site_is_cross_site, action_origin_is_cross_site,
};
use devtools::{DevToolsMetrics, dashboard_html};
pub use dynamic_image::DynamicImageConfig;
use dynamic_image::{DynamicImageCache, DynamicImageError};

mod cli_output;
use cli_output::{
    accent, badge, current_timestamp, dim, enabled_text, heading, info, link, middleware_summary,
    note, number, ok, paint, path_text, print_field,
};

mod port_binding;
use port_binding::bind_listener;
#[cfg(test)]
use port_binding::{PORT_FALLBACK_SCAN_LIMIT, port_conflict_diagnostic};

mod html_document;
mod i18n;
mod trace;
#[cfg(test)]
use html_document::{
    client_hydration_script, compose_document, dev_diagnostic_overlay, hmr_client_script,
    prebuilt_client_assets,
};
use html_document::{
    dev_error_overlay, error_response, plain_error_page, public_internal_error,
    url_encode_component,
};
pub use html_document::{
    hydration_loader_source, hydration_loader_url, localize_document, safe_json_for_script,
};

mod plugin_bridge;
#[cfg(test)]
use plugin_bridge::{
    BufferedPluginBody, body_exceeds_plugin_limit, buffer_plugin_response_body,
    plugin_response_into_response,
};
use plugin_bridge::{
    apply_request_plugins, apply_response_plugins, canonical_request_path, decode_plugin_body,
    encode_plugin_body, headers_to_plugin_pairs, plugin_headers, request_method_allows_body,
    split_plugin_target,
};

mod plugin_head;
pub use plugin_head::{PluginHeadEntry, render_plugin_head};
mod static_assets;
use static_assets::public_asset_links;
#[cfg(test)]
use static_assets::{is_safe_relative_path, resolve_public_asset};

mod worker_pool;
use worker_pool::RenderFlightRequest;
pub use worker_pool::{NodeWorkerPool, StaticParamSegment, StaticParamsRoute};

mod render_pipeline;
pub use render_pipeline::{RenderContext, find_runtime_script, render_request_with_context};
#[cfg(test)]
use render_pipeline::{decode_realtime_event, serve_prerendered_html};
use render_pipeline::{
    render_client_bundle_pooled, render_request_pooled, render_server_action_pooled, runtime_env,
    runtime_trace_cached,
};

mod router;
pub use router::RadixRouter;

mod render_cache;
mod response;
// Re-exported so every existing `crate::html_response`-style path keeps
// resolving: moving these into a module is a relocation, not an interface change.
pub use render_cache::RenderCache;
pub(crate) use response::{
    apply_security_headers, cached_html_response, finalize_security_headers, html_response,
    json_response, shared_text_body, with_security_headers,
};

mod hmr_tracker;
pub use hmr_tracker::{HmrEventType, HmrTracker, HmrUpdate};

mod postcss;
pub use postcss::PostcssRunner;

mod style;
pub use style::{StyleCollection, collect_styles, collect_styles_for_build, minify_css};

pub mod process;

const MAX_ACTION_BODY_BYTES: usize = 1024 * 1024;
const MAX_API_BODY_BYTES: usize = 10 * 1024 * 1024;
/// Absolute upper bound for action payload buffering, regardless of project config.
pub const MAX_ACTION_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;
/// Absolute upper bound for API payload buffering, regardless of project config.
pub const MAX_API_BODY_LIMIT_BYTES: usize = 256 * 1024 * 1024;
/// Default maximum response size buffered for a TypeScript response middleware.
pub const DEFAULT_PLUGIN_RESPONSE_BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
/// Largest response size a project may configure for TypeScript response middleware.
pub const MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES: usize = 256 * 1024 * 1024;
const ACTION_RATE_LIMIT_MAX: usize = 600;
const ACTION_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
pub const MAX_ACTION_RATE_LIMIT_REQUESTS: usize = 10_000;
pub const MAX_ACTION_RATE_LIMIT_WINDOW_SECS: u64 = 86_400;
const SERVER_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// JavaScript runtime used for Ruvyxa's config, render, and plugin processes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JavaScriptRuntime {
    #[default]
    Node,
    Bun,
    Deno,
}

impl JavaScriptRuntime {
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Bun => "bun",
            Self::Deno => "deno",
        }
    }

    /// Arguments that must precede a JavaScript entry point for this runtime.
    ///
    /// Deno is permission-secure by default while Node and Bun are not. Ruvyxa's
    /// local tool processes execute trusted project config and plugins and need
    /// filesystem, environment, process, network, and native-addon access, so
    /// local development deliberately uses the equivalent unrestricted mode.
    #[must_use]
    pub const fn script_args(self) -> &'static [&'static str] {
        match self {
            Self::Node | Self::Bun => &[],
            Self::Deno => &["run", "-A", "--no-prompt", "--node-modules-dir=manual"],
        }
    }

    /// Executable used to launch the runtime process.
    ///
    /// Windows package-manager shims commonly expose Bun as `bun.cmd` instead
    /// of `bun.exe`. Launching the shim through `cmd.exe` can corrupt JSON
    /// arguments, so resolve the Bun package executable behind the shim first.
    #[must_use]
    pub fn executable(self) -> std::path::PathBuf {
        match self {
            Self::Node => std::path::PathBuf::from(self.command()),
            Self::Bun => {
                #[cfg(windows)]
                if let Some(executable) = bun_executable_from_path() {
                    return executable;
                }
                std::path::PathBuf::from(self.command())
            }
            Self::Deno => {
                #[cfg(windows)]
                if let Some(executable) = deno_executable_from_path() {
                    return executable;
                }
                std::path::PathBuf::from(self.command())
            }
        }
    }

    #[must_use]
    pub fn is_available(self) -> bool {
        // Bounded: a runtime that cannot answer `--version` promptly is not a
        // usable runtime, and this probe runs during `doctor` and during
        // runtime auto-detection, where hanging would strand the whole command.
        let mut command = std::process::Command::new(self.executable());
        command.arg("--version");
        crate::process::output_with_timeout(&mut command, crate::process::PROBE_TIMEOUT)
            .is_ok_and(|output| output.status.success())
    }

    /// Select the default JavaScript runtime for an installation.
    ///
    /// Node remains the preferred runtime for compatibility. Bun is selected
    /// only when Node is unavailable and Bun can be executed. If neither
    /// runtime is installed, keep Node as the diagnostic target so the
    /// resulting process error names the conventional runtime.
    #[must_use]
    pub fn detect() -> Self {
        Self::from_availability(
            Self::Node.is_available(),
            Self::Bun.is_available(),
            Self::Deno.is_available(),
        )
    }

    #[must_use]
    pub const fn from_availability(
        node_available: bool,
        bun_available: bool,
        deno_available: bool,
    ) -> Self {
        if node_available {
            Self::Node
        } else if bun_available {
            Self::Bun
        } else if deno_available {
            Self::Deno
        } else {
            Self::Node
        }
    }
}

#[cfg(windows)]
fn bun_executable_from_path() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let direct = directory.join("bun.exe");
        if direct.is_file() {
            return Some(direct);
        }
        if directory.join("bun.cmd").is_file() {
            let package_executable = directory.join("node_modules/bun/bin/bun.exe");
            if package_executable.is_file() {
                return Some(package_executable);
            }
        }
    }
    None
}

#[cfg(windows)]
fn deno_executable_from_path() -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let direct = directory.join("deno.exe");
        if direct.is_file() {
            return Some(direct);
        }
        if directory.join("deno.cmd").is_file() {
            for candidate in [
                directory.join("node_modules/deno/deno.exe"),
                directory.join("node_modules/deno/node_modules/@deno/win32-x64/deno.exe"),
            ] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// A callback run with every freshly discovered route manifest.
///
/// The dev server owns route discovery and re-runs it when the watcher
/// invalidates its cache, which is exactly when generated artifacts derived
/// from the route set — today, the typed-routes declaration file — go stale.
/// Exposing the moment is cheaper and more correct than having the CLI watch
/// the same directory a second time and race the server's own scan.
///
/// A panic in the observer is caught and reported rather than allowed to take
/// the request down with it: this is a developer convenience running on the
/// request path, and a failure to write a `.d.ts` must not stop a page from
/// rendering.
#[derive(Clone)]
pub struct RouteManifestObserver(Arc<dyn Fn(&RouteManifest) + Send + Sync>);

impl RouteManifestObserver {
    pub fn new(observe: impl Fn(&RouteManifest) + Send + Sync + 'static) -> Self {
        Self(Arc::new(observe))
    }

    fn notify(&self, manifest: &RouteManifest) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.0)(manifest))).is_err() {
            tracing::warn!(
                "route manifest observer panicked; generated route artifacts may be stale"
            );
        }
    }
}

impl std::fmt::Debug for RouteManifestObserver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RouteManifestObserver")
    }
}

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
    /// JavaScript runtime used by every renderer and worker.
    pub runtime: JavaScriptRuntime,
    /// JSX transform runtime passed to every JavaScript renderer and worker.
    pub jsx_runtime: JsxRuntime,
    /// Render actionable source-aware error overlays in development.
    pub error_overlay: bool,
    /// Expose runtime route traces from the development diagnostics endpoint.
    pub debug_traces: bool,
    /// Maximum accepted action request payload size.
    pub action_body_limit_bytes: usize,
    /// Maximum accepted API route request payload size.
    pub api_body_limit_bytes: usize,
    /// Maximum response size buffered for TypeScript response middleware.
    pub plugin_response_body_limit_bytes: usize,
    /// Maximum action requests per client/action in the configured window.
    pub action_rate_limit_max: usize,
    /// Window used by the action rate limiter.
    pub action_rate_limit_window: Duration,
    /// Reject action requests whose Origin does not match the request Host.
    pub same_origin_actions: bool,
    /// Reject action requests initiated from a cross-site browser context.
    pub fetch_metadata_actions: bool,
    /// Non-loopback reverse proxies allowed to supply forwarded client and
    /// protocol headers, as exact addresses or CIDR ranges.
    pub trusted_proxies: TrustedProxies,
    /// Apply Ruvyxa's default security response headers.
    pub security_headers: bool,
    pub middleware: MiddlewareConfig,
    /// Notified whenever route discovery runs, so generated artifacts derived
    /// from the route set stay in step with it.
    pub route_manifest_observer: Option<RouteManifestObserver>,
    /// Start the TypeScript plugin host for this server.
    pub plugins_enabled: bool,
    /// Head elements plugins declared in `ruvyxa.config.ts`.
    pub plugin_head: Vec<PluginHeadEntry>,
    pub default_render_strategy: Option<RenderStrategy>,
    pub default_revalidate: Option<u64>,
    /// Validated file-system locale routing policy.
    pub i18n: Option<I18nRouting>,
    /// Same-origin runtime image resizing policy.
    pub dynamic_images: DynamicImageConfig,
}

impl ServerConfig {
    fn validate_limits(&self) -> Result<()> {
        if self.action_body_limit_bytes == 0
            || self.action_body_limit_bytes > MAX_ACTION_BODY_LIMIT_BYTES
        {
            return Err(RuvyxaError::Message(format!(
                "security.actionLimit must be between 1 and {MAX_ACTION_BODY_LIMIT_BYTES} bytes"
            )));
        }
        if self.api_body_limit_bytes == 0 || self.api_body_limit_bytes > MAX_API_BODY_LIMIT_BYTES {
            return Err(RuvyxaError::Message(format!(
                "security.apiLimit must be between 1 and {MAX_API_BODY_LIMIT_BYTES} bytes"
            )));
        }
        if self.action_rate_limit_max == 0
            || self.action_rate_limit_max > MAX_ACTION_RATE_LIMIT_REQUESTS
        {
            return Err(RuvyxaError::Message(format!(
                "security.actionRateLimit.max must be between 1 and {MAX_ACTION_RATE_LIMIT_REQUESTS}"
            )));
        }
        if self.action_rate_limit_window.is_zero()
            || self.action_rate_limit_window.as_secs() > MAX_ACTION_RATE_LIMIT_WINDOW_SECS
        {
            return Err(RuvyxaError::Message(format!(
                "security.actionRateLimit.window must be between 1 and {MAX_ACTION_RATE_LIMIT_WINDOW_SECS} seconds"
            )));
        }
        if self.plugin_response_body_limit_bytes == 0
            || self.plugin_response_body_limit_bytes > MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES
        {
            return Err(RuvyxaError::Message(format!(
                "security.pluginLimit must be between 1 and {MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES} bytes"
            )));
        }
        Ok(())
    }

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
            runtime: JavaScriptRuntime::detect(),
            jsx_runtime: JsxRuntime::Automatic,
            error_overlay: true,
            debug_traces: false,
            action_body_limit_bytes: MAX_ACTION_BODY_BYTES,
            api_body_limit_bytes: MAX_API_BODY_BYTES,
            plugin_response_body_limit_bytes: DEFAULT_PLUGIN_RESPONSE_BODY_LIMIT_BYTES,
            action_rate_limit_max: ACTION_RATE_LIMIT_MAX,
            action_rate_limit_window: ACTION_RATE_LIMIT_WINDOW,
            same_origin_actions: true,
            fetch_metadata_actions: true,
            trusted_proxies: TrustedProxies::default(),
            security_headers: true,
            middleware: MiddlewareConfig::default(),
            route_manifest_observer: None,
            plugins_enabled: false,
            plugin_head: Vec::new(),
            default_render_strategy: None,
            default_revalidate: None,
            i18n: None,
            dynamic_images: DynamicImageConfig::default(),
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
            runtime: JavaScriptRuntime::detect(),
            jsx_runtime: JsxRuntime::Automatic,
            error_overlay: false,
            debug_traces: false,
            action_body_limit_bytes: MAX_ACTION_BODY_BYTES,
            api_body_limit_bytes: MAX_API_BODY_BYTES,
            plugin_response_body_limit_bytes: DEFAULT_PLUGIN_RESPONSE_BODY_LIMIT_BYTES,
            action_rate_limit_max: ACTION_RATE_LIMIT_MAX,
            action_rate_limit_window: ACTION_RATE_LIMIT_WINDOW,
            same_origin_actions: true,
            fetch_metadata_actions: true,
            trusted_proxies: TrustedProxies::default(),
            security_headers: true,
            middleware: MiddlewareConfig::default(),
            route_manifest_observer: None,
            plugins_enabled: false,
            plugin_head: Vec::new(),
            default_render_strategy: None,
            default_revalidate: None,
            i18n: None,
            dynamic_images: DynamicImageConfig::default(),
        }
    }
}

#[derive(Clone)]
struct AppState {
    config: ServerConfig,
    reload_tx: broadcast::Sender<String>,
    runtime_cache: Arc<RuntimeCache>,
    action_limiter: Arc<Mutex<ActionRateLimiter>>,
    action_replays: Arc<Mutex<ActionReplayGuard>>,
    worker_pool: Arc<NodeWorkerPool>,
    render_cache: Arc<RenderCache>,
    isr_revalidating: render_pipeline::IsrRevalidationSet,
    hmr_tracker: Arc<HmrTracker>,
    plugin_runtime: Option<Arc<PluginHost>>,
    realtime: Option<RealtimeRuntime>,
    presence: Option<PresenceRuntime>,
    devtools: Arc<DevToolsMetrics>,
    dynamic_image_cache: Arc<DynamicImageCache>,
    edit_traces: Arc<trace::TraceStore>,
}

#[derive(Clone)]
struct RealtimeRuntime {
    path: String,
    heartbeat: Duration,
    tx: broadcast::Sender<String>,
}

/// Collaboration rooms live for the process, not for one connection, so the
/// registry is owned by the shared app state rather than the socket handler.
#[derive(Clone)]
struct PresenceRuntime {
    path: String,
    heartbeat: Duration,
    registry: CollabRegistry,
}

/// Framework endpoints registered on the router before the plugin realtime
/// route. Must stay in sync with the `Router::new()` chain in [`serve`];
/// registering a realtime transport on one of these would panic in axum.
const RESERVED_FRAMEWORK_ROUTES: [&str; 8] = [
    "/__ruvyxa/hmr",
    "/__ruvyxa/client",
    "/__ruvyxa/action",
    "/__ruvyxa/flight",
    "/__ruvyxa/trace",
    "/__ruvyxa/devtools",
    "/__ruvyxa/devtools/data",
    "/__ruvyxa/image",
];

/// Process-wide startup hooks recognised by the JavaScript compiler/runtime.
/// Kept under a cross-language conformance test because changing one host alone
/// makes development and deployed instrumentation disagree.
const INSTRUMENTATION_FILES: [&str; 3] = [
    "instrumentation.ts",
    "instrumentation.js",
    "instrumentation.mjs",
];

#[derive(Default)]
struct RuntimeCache {
    routes: tokio::sync::RwLock<CacheSlot<RouteCacheEntry>>,
    styles: tokio::sync::RwLock<CacheSlot<StyleCacheEntry>>,
    /// `<link>` tags derived from the public directory's contents.
    ///
    /// Resolved once and reused. `public_asset_links` stats the public directory
    /// to decide which tags to emit, and every page render called it — a
    /// blocking filesystem syscall on a Tokio worker thread, per request, for an
    /// answer that only changes when the watcher invalidates this cache.
    asset_links: tokio::sync::RwLock<CacheSlot<Arc<str>>>,
}

/// One independently invalidated cache generation.
///
/// Filesystem work intentionally runs without holding the lock. The generation
/// prevents a result that started before a watcher invalidation from becoming
/// the new cached value after that invalidation has already completed.
#[derive(Debug)]
struct CacheSlot<T> {
    generation: u64,
    value: Option<T>,
}

impl<T> Default for CacheSlot<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            value: None,
        }
    }
}

impl<T> CacheSlot<T> {
    fn with_value(value: T) -> Self {
        Self {
            generation: 0,
            value: Some(value),
        }
    }

    fn insert_if_current(&mut self, generation: u64, value: T) -> bool {
        if self.generation != generation || self.value.is_some() {
            return false;
        }
        self.value = Some(value);
        true
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.value = None;
    }
}

#[derive(Clone)]
struct RouteCacheEntry {
    manifest: Arc<RouteManifest>,
    router: Arc<RadixRouter>,
}

impl RouteCacheEntry {
    fn new(manifest: RouteManifest) -> Self {
        let manifest = Arc::new(manifest);
        let router = Arc::new(RadixRouter::compile(&manifest));
        Self { manifest, router }
    }

    fn pair(&self) -> (Arc<RouteManifest>, Arc<RadixRouter>) {
        (Arc::clone(&self.manifest), Arc::clone(&self.router))
    }
}

#[derive(Debug, Clone)]
struct StyleCacheEntry {
    css: String,
    files: BTreeSet<PathBuf>,
}

impl RuntimeCache {
    fn with_manifest(manifest: RouteManifest) -> Self {
        Self {
            routes: tokio::sync::RwLock::new(CacheSlot::with_value(RouteCacheEntry::new(manifest))),
            styles: tokio::sync::RwLock::new(CacheSlot::default()),
            asset_links: tokio::sync::RwLock::new(CacheSlot::default()),
        }
    }

    /// Public-directory `<link>` tags, resolved on first use.
    async fn asset_links(&self, config: &ServerConfig) -> Arc<str> {
        loop {
            let generation = {
                let cached = self.asset_links.read().await;
                if let Some(links) = cached.value.as_ref() {
                    return Arc::clone(links);
                }
                cached.generation
            };

            // The directory scan touches the filesystem, so keep it off the async
            // worker thread like every other blocking read on this path.
            let public_dir = config.public_dir.clone();
            let links: Arc<str> =
                tokio::task::spawn_blocking(move || Arc::from(public_asset_links(&public_dir)))
                    .await
                    .unwrap_or_else(|_| Arc::from(""));

            let mut cached = self.asset_links.write().await;
            if let Some(links) = cached.value.as_ref() {
                return Arc::clone(links);
            }
            if cached.insert_if_current(generation, Arc::clone(&links)) {
                return links;
            }
        }
    }

    async fn router(
        &self,
        config: &ServerConfig,
    ) -> Result<(Arc<RouteManifest>, Arc<RadixRouter>)> {
        self.route_snapshot(config).await
    }

    /// Return route discovery and matching as one generation. `RadixRouter`
    /// stores manifest indices, so either value is meaningless without the
    /// other and they must never be cached or refreshed independently.
    async fn route_snapshot(
        &self,
        config: &ServerConfig,
    ) -> Result<(Arc<RouteManifest>, Arc<RadixRouter>)> {
        if !config.cache_route_manifest {
            let entry = RouteCacheEntry::new(discover_routes_off_thread(config).await?);
            observe_manifest(config, &entry.manifest);
            return Ok(entry.pair());
        }

        loop {
            let generation = {
                let cached = self.routes.read().await;
                if let Some(entry) = cached.value.as_ref() {
                    return Ok(entry.pair());
                }
                cached.generation
            };

            let discovered = RouteCacheEntry::new(discover_routes_off_thread(config).await?);
            let entry = {
                let mut cached = self.routes.write().await;
                if let Some(entry) = cached.value.as_ref() {
                    return Ok(entry.pair());
                }
                if !cached.insert_if_current(generation, discovered.clone()) {
                    continue;
                }
                discovered
            };
            observe_manifest(config, &entry.manifest);
            return Ok(entry.pair());
        }
    }

    async fn styles(&self, config: &ServerConfig) -> Result<String> {
        if !config.cache_css {
            let css = collect_styles_off_thread(config).await?.css;
            return Ok(if config.watch {
                css
            } else {
                style::minify_css(&css)
            });
        }

        loop {
            let generation = {
                let cached = self.styles.read().await;
                if let Some(styles) = cached.value.as_ref() {
                    return Ok(styles.css.clone());
                }
                cached.generation
            };

            let collection = collect_styles_off_thread(config).await?;
            let mut css = collection.css;
            // Minify CSS in production mode to reduce inline style payload.
            if !config.watch {
                css = style::minify_css(&css);
            }
            let entry = StyleCacheEntry {
                css: css.clone(),
                files: collection
                    .files
                    .into_iter()
                    .map(|path| normalize_cache_path(&path))
                    .collect(),
            };
            let mut cached = self.styles.write().await;
            if let Some(styles) = cached.value.as_ref() {
                return Ok(styles.css.clone());
            }
            if cached.insert_if_current(generation, entry) {
                return Ok(css);
            }
        }
    }

    /// Invalidate cached CSS only when a watched event changed a CSS source
    /// collected for the current style graph. This preserves the style cache
    /// for component-only HMR updates.
    fn invalidate_styles_for_paths(&self, paths: &[PathBuf]) -> bool {
        let changed = paths
            .iter()
            .map(|path| normalize_cache_path(path))
            .collect::<BTreeSet<_>>();
        let mut styles = self.styles.blocking_write();
        let intersects = styles
            .value
            .as_ref()
            .is_some_and(|cached| !cached.files.is_disjoint(&changed));
        if intersects {
            styles.invalidate();
        }
        intersects
    }

    fn invalidate(&self) {
        // Use blocking_write for sync context (file watcher callback)
        self.routes.blocking_write().invalidate();
        self.styles.blocking_write().invalidate();
        self.asset_links.blocking_write().invalidate();
    }

    #[cfg(test)]
    async fn invalidate_async(&self) {
        self.routes.write().await.invalidate();
        self.styles.write().await.invalidate();
        self.asset_links.write().await.invalidate();
    }
}

async fn discover_routes_off_thread(config: &ServerConfig) -> Result<RouteManifest> {
    let options = discover_options(config);
    tokio::task::spawn_blocking(move || discover_routes(options))
        .await
        .map_err(|error| RuvyxaError::Message(format!("Route discovery task failed: {error}")))?
}

async fn collect_styles_off_thread(config: &ServerConfig) -> Result<StyleCollection> {
    let root = config.root.clone();
    let app_dir = config.app_dir.clone();
    let entries = config.style_entries.clone();
    tokio::task::spawn_blocking(move || collect_styles(&root, &app_dir, &entries))
        .await
        .map_err(|error| RuvyxaError::Message(format!("Style collection task failed: {error}")))?
}

fn normalize_cache_path(path: &Path) -> PathBuf {
    let absolute = if path.exists() {
        ruvyxa_diagnostics::normalized_canonical_path(path)
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };

    #[cfg(windows)]
    {
        PathBuf::from(absolute.to_string_lossy().to_ascii_lowercase())
    }
    #[cfg(not(windows))]
    {
        absolute
    }
}

/// Hand a freshly discovered manifest to the configured observer, if any.
fn observe_manifest(config: &ServerConfig, manifest: &RouteManifest) {
    if let Some(observer) = &config.route_manifest_observer {
        observer.notify(manifest);
    }
}

fn discover_options(config: &ServerConfig) -> DiscoverOptions {
    DiscoverOptions::new(&config.app_dir)
        .with_rendering_defaults(config.default_render_strategy, config.default_revalidate)
        .with_i18n(config.i18n.clone())
}

pub async fn serve(config: ServerConfig) -> Result<()> {
    config.validate_limits()?;
    let startup_started = Instant::now();
    let manifest = discover_routes(discover_options(&config))?;
    observe_manifest(&config, &manifest);
    info!(routes = manifest.routes.len(), "discovered routes");

    let (reload_tx, _) = broadcast::channel(64);
    let runtime_cache = Arc::new(RuntimeCache::with_manifest(manifest.clone()));

    let env = runtime_env(&config)?;
    let worker_pool =
        Arc::new(NodeWorkerPool::start_with_runtime(&config.root, env, config.runtime).await?);
    info!(
        runtime = config.runtime.command(),
        "JavaScript worker pool ready"
    );

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
    let plugin_runtime = if !config.plugins_enabled {
        None
    } else {
        let runtime_script = find_runtime_script(&config.root, "plugin-runtime.mjs")
            .ok_or_else(|| RuvyxaError::Message("RUV1701 plugin-runtime.mjs not found".into()))?;
        let executable = config.runtime.executable();
        let plugin_workers = config
            .middleware
            .plugin_workers()
            .map_err(RuvyxaError::Message)?;
        let plugin_timeout = config
            .middleware
            .plugin_timeout()
            .map_err(RuvyxaError::Message)?;
        let host = PluginHost::start_pool_with_timeout_and_args(
            &config.root,
            &runtime_script,
            &executable,
            config.runtime.script_args(),
            plugin_workers,
            plugin_timeout,
        )
        .await?;
        if host.pool_size() > 1 {
            info!(
                workers = host.pool_size(),
                "TypeScript plugin middleware pool ready"
            );
        }
        Some(Arc::new(host))
    };
    let realtime = plugin_runtime
        .as_ref()
        .and_then(|runtime| runtime.descriptor().realtime())
        .map(|descriptor| {
            if !descriptor.path.starts_with('/')
                || descriptor.path.contains(['?', '#', '*'])
                || !(5_000..=120_000).contains(&descriptor.heartbeat_ms)
                || !(16..=4_096).contains(&descriptor.capacity)
            {
                return Err(RuvyxaError::Message(
                    "RUV1701 TypeScript plugin host returned invalid realtime configuration".into(),
                ));
            }
            // Registering over a reserved framework route would panic inside
            // axum's router; fail with a diagnostic instead.
            if RESERVED_FRAMEWORK_ROUTES.contains(&descriptor.path.as_str()) {
                return Err(RuvyxaError::Message(format!(
                    "RUV1701 realtime path {} collides with a reserved framework route",
                    descriptor.path
                )));
            }
            let (tx, _) = broadcast::channel(descriptor.capacity);
            Ok(RealtimeRuntime {
                path: descriptor.path.clone(),
                heartbeat: Duration::from_millis(descriptor.heartbeat_ms),
                tx,
            })
        })
        .transpose()?;
    let presence = plugin_runtime
        .as_ref()
        .and_then(|runtime| runtime.descriptor().presence())
        .map(|descriptor| {
            if !descriptor.path.starts_with('/')
                || descriptor.path.contains(['?', '#', '*'])
                || !(5_000..=120_000).contains(&descriptor.heartbeat_ms)
            {
                return Err(RuvyxaError::Message(
                    "RUV1701 TypeScript plugin host returned invalid presence configuration".into(),
                ));
            }
            if RESERVED_FRAMEWORK_ROUTES.contains(&descriptor.path.as_str()) {
                return Err(RuvyxaError::Message(format!(
                    "RUV1701 presence path {} collides with a reserved framework route",
                    descriptor.path
                )));
            }
            Ok(PresenceRuntime {
                path: descriptor.path.clone(),
                heartbeat: Duration::from_millis(descriptor.heartbeat_ms),
                registry: CollabRegistry::new(),
            })
        })
        .transpose()?;
    // Realtime and presence are separate transports, so a project that claims
    // both must not point them at one path; axum would panic on the duplicate.
    if let (Some(realtime), Some(presence)) = (realtime.as_ref(), presence.as_ref())
        && realtime.path == presence.path
    {
        return Err(RuvyxaError::Message(format!(
            "RUV1701 presence path {} collides with the realtime transport",
            presence.path
        )));
    }
    let state = AppState {
        config: config.clone(),
        reload_tx,
        runtime_cache,
        action_limiter: Arc::new(Mutex::new(ActionRateLimiter::new(
            config.action_rate_limit_max,
            config.action_rate_limit_window,
        ))),
        action_replays: Arc::new(Mutex::new(ActionReplayGuard::default())),
        worker_pool: worker_pool.clone(),
        render_cache,
        isr_revalidating: Arc::new(std::sync::Mutex::new(HashSet::new())),
        hmr_tracker,
        plugin_runtime,
        realtime,
        presence,
        devtools: Arc::new(DevToolsMetrics::default()),
        dynamic_image_cache: Arc::new(DynamicImageCache::default()),
        edit_traces: Arc::new(trace::TraceStore::default()),
    };

    let _watcher = if config.watch {
        Some(start_watcher(
            &config.root,
            &watch_paths(&config),
            WatcherRuntime {
                config: config.clone(),
                reload_tx: state.reload_tx.clone(),
                runtime_cache: state.runtime_cache.clone(),
                worker_pool: watcher_pool,
                render_cache: watcher_render_cache,
                hmr_tracker: state.hmr_tracker.clone(),
                plugin_runtime: state.plugin_runtime.clone(),
                edit_traces: state.edit_traces.clone(),
                tokio_handle: tokio::runtime::Handle::current(),
            },
        )?)
    } else {
        None
    };

    let realtime_path = state.realtime.as_ref().map(|runtime| runtime.path.clone());
    let presence_path = state.presence.as_ref().map(|runtime| runtime.path.clone());
    let state = Arc::new(state);
    let mut app = Router::new()
        .route("/__ruvyxa/hmr", get(hmr_ws))
        .route("/__ruvyxa/client", get(client_bundle))
        .route("/__ruvyxa/hydration-loader.js", get(hydration_loader))
        .route("/__ruvyxa/client/route-manifest.json", get(client_manifest))
        .route("/__ruvyxa/flight", get(flight_endpoint))
        .route("/__ruvyxa/image", get(dynamic_image_endpoint))
        .route(
            "/__ruvyxa/action",
            post(action_endpoint).layer(DefaultBodyLimit::max(config.action_body_limit_bytes)),
        )
        .route(
            "/__ruvyxa/trace",
            get(trace_endpoint)
                .post(trace_ack_endpoint)
                .layer(DefaultBodyLimit::max(1_024)),
        );
    if config.watch {
        app = app
            .route("/__ruvyxa/devtools", get(devtools_dashboard))
            .route("/__ruvyxa/devtools/data", get(devtools_data));
    }
    if let Some(path) = realtime_path {
        app = app.route(&path, get(realtime_ws));
    }
    if let Some(path) = presence_path {
        app = app.route(&path, get(presence_ws));
    }
    let app = app.fallback(handle_request).with_state(state);

    // Apply middleware stack from config (compression, CORS, timing, logging, custom headers)
    let app = middleware_stack.apply(app).map_err(RuvyxaError::Message)?;
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
    let server = axum::serve(listener, server_make_service(app))
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

fn server_make_service(
    app: Router,
) -> axum::extract::connect_info::IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    app.into_make_service_with_connect_info::<SocketAddr>()
}

/// Wait for an interactive interrupt or the Unix termination signal.
async fn shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => {
                tracing::warn!(%error, "failed to register SIGTERM handler; falling back to Ctrl-C");
                let _ = tokio::signal::ctrl_c().await;
                return "CTRL_C";
            }
        };
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
            route_path: route.path.clone(),
        })
        .collect()
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

    // The same header shape every CLI command prints: title, badge, blank line.
    // The dev server used to print its own two-line variant, which is why it
    // was the one surface with no command badge.
    let (title, badge) = if config.watch {
        ("🦊 Ruvyxa Dev Server", badge("Dev"))
    } else {
        ("🦊 Ruvyxa Server", badge("Server"))
    };
    println!();
    println!("{}", heading(title));
    println!();
    println!("  {} {}", badge.icon, dim(badge.tagline));
    println!();
    print_field("time", dim(current_timestamp()));
    print_field("mode", accent(mode));
    print_field("local", link(&url));
    print_field("root", path_text(&config.root));
    print_field("app dir", path_text(&config.app_dir));
    print_field("public", path_text(&config.public_dir));
    print_field("client", path_text(&config.client_dir));
    print_field("routes", number(manifest.routes.len().to_string()));
    print_field("pages", info(page_routes.to_string()));
    print_field("api", note(api_routes.to_string()));
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
    print_field("watch paths", number(watch_paths(config).len().to_string()));
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

struct WatcherRuntime {
    config: ServerConfig,
    reload_tx: broadcast::Sender<String>,
    runtime_cache: Arc<RuntimeCache>,
    worker_pool: Arc<NodeWorkerPool>,
    render_cache: Arc<RenderCache>,
    hmr_tracker: Arc<HmrTracker>,
    plugin_runtime: Option<Arc<PluginHost>>,
    edit_traces: Arc<trace::TraceStore>,
    tokio_handle: tokio::runtime::Handle,
}

async fn refresh_hmr_manifest(
    config: &ServerConfig,
    runtime_cache: &RuntimeCache,
    hmr_tracker: &HmrTracker,
) -> Result<()> {
    let (manifest, _) = runtime_cache.route_snapshot(config).await?;
    hmr_tracker.populate_from_manifest(&manifest.routes);
    Ok(())
}

fn start_watcher(
    root: &Path,
    watch_paths: &[PathBuf],
    runtime: WatcherRuntime,
) -> Result<RecommendedWatcher> {
    let WatcherRuntime {
        config,
        reload_tx,
        runtime_cache,
        worker_pool,
        render_cache,
        hmr_tracker,
        plugin_runtime,
        edit_traces,
        tokio_handle,
    } = runtime;
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
                let instrumentation_changed = instrumentation_source_changed(&root, &paths);

                // Use HmrTracker for selective invalidation.
                let mut hmr_update = hmr_tracker.compute_update(&paths);
                if hmr_update.full_reload || instrumentation_changed {
                    hmr_update.full_reload = true;
                    hmr_update.event_type = HmrEventType::FullReload;
                }
                // Selective cache invalidation based on affected routes.
                if hmr_update.full_reload || hmr_update.affected_routes.is_empty() {
                    // Full invalidation: manifest may have changed (new/deleted routes).
                    runtime_cache.invalidate();
                    render_cache.invalidate_all_blocking();
                    let refresh_config = config.clone();
                    let refresh_cache = runtime_cache.clone();
                    let refresh_tracker = hmr_tracker.clone();
                    tokio_handle.spawn(async move {
                        if let Err(error) =
                            refresh_hmr_manifest(&refresh_config, &refresh_cache, &refresh_tracker)
                                .await
                        {
                            warn!(%error, "HMR route manifest refresh failed");
                        }
                    });
                } else {
                    // Selective invalidation: only evict affected route caches.
                    // Refresh styles only when the current CSS dependency graph
                    // intersects a changed path. Component-only updates retain it.
                    runtime_cache.invalidate_styles_for_paths(&paths);

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
                let hmr_paths = paths
                    .iter()
                    .map(|path| {
                        path.strip_prefix(&root)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .replace('\\', "/")
                    })
                    .collect::<Vec<_>>();
                let trace_id = trace::edit_id(&hmr_paths);
                let trace_kind = hmr_update_kind(&hmr_update, &hmr_paths);
                edit_traces.start(
                    &trace_id,
                    &hmr_paths,
                    &hmr_update.affected_routes,
                    trace_kind,
                );
                edit_traces.record(
                    &trace_id,
                    "cache",
                    if hmr_update.full_reload {
                        "all route and render caches invalidated"
                    } else {
                        "affected route and style caches invalidated"
                    },
                );
                info!(
                    trace_id = %trace_id,
                    files = hmr_paths.len(),
                    routes = hmr_update.affected_routes.len(),
                    "HMR edit accepted"
                );
                let worker_result = (!instrumentation_changed).then(|| {
                    worker_pool.invalidate_from_watcher(path_strings.clone(), Some(&trace_id))
                });
                match &worker_result {
                    Some(Ok(workers)) => {
                        edit_traces.record(
                            &trace_id,
                            "worker",
                            format!("queued invalidation for {workers} workers"),
                        );
                    }
                    Some(Err(error)) => {
                        edit_traces.record(
                            &trace_id,
                            "worker",
                            format!("invalidation failed: {error}"),
                        );
                    }
                    None => {
                        edit_traces.record(
                            &trace_id,
                            "worker",
                            "instrumentation change requires recycle",
                        );
                    }
                }
                if worker_result.as_ref().is_some_and(|result| result.is_err()) {
                    hmr_update.full_reload = true;
                    hmr_update.event_type = HmrEventType::FullReload;
                }

                if let Some(plugin_runtime) = plugin_runtime.clone() {
                    let plugin_paths = plugin_watch_paths(&root, &paths);
                    tokio_handle.spawn(async move {
                        if let Err(error) = plugin_runtime.notify_file_change(&plugin_paths).await {
                            warn!(%error, "plugin dev.fileChange hook failed");
                        }
                    });
                }

                if instrumentation_changed {
                    let recycle_pool = Arc::clone(&worker_pool);
                    let recycle_reload = reload_tx.clone();
                    let restart_payload = hmr_payload(
                        &hmr_update,
                        &hmr_paths,
                        &trace_id,
                        config.debug_traces,
                        None,
                    );
                    let issue_update = hmr_update.clone();
                    let issue_paths = hmr_paths.clone();
                    let issue_trace = trace_id.clone();
                    let trace_store = Arc::clone(&edit_traces);
                    let trace_ack = config.debug_traces;
                    tokio_handle.spawn(async move {
                        let payload = match recycle_pool.recycle().await {
                            Ok(workers) => {
                                info!(workers, "recycled workers after instrumentation change");
                                trace_store.record(
                                    &issue_trace,
                                    "worker",
                                    format!("recycled {workers} workers"),
                                );
                                restart_payload
                            }
                            Err(error) => {
                                warn!(%error, "worker recycle after instrumentation change failed");
                                trace_store.record(
                                    &issue_trace,
                                    "worker",
                                    format!("recycle failed: {error}"),
                                );
                                hmr_payload(
                                    &issue_update,
                                    &issue_paths,
                                    &issue_trace,
                                    trace_ack,
                                    Some((
                                        "RUV1707",
                                        "Worker restart failed; restart the development server.",
                                    )),
                                )
                            }
                        };
                        let _ = recycle_reload.send(payload);
                        trace_store.record(&issue_trace, "hmr", "message broadcast");
                    });
                } else if let Some(Err(error)) = worker_result {
                    warn!(%error, "worker invalidation failed; recycling workers");
                    let recycle_pool = Arc::clone(&worker_pool);
                    let recycle_reload = reload_tx.clone();
                    let issue_update = hmr_update.clone();
                    let issue_paths = hmr_paths.clone();
                    let issue_trace = trace_id.clone();
                    let trace_store = Arc::clone(&edit_traces);
                    let trace_ack = config.debug_traces;
                    tokio_handle.spawn(async move {
                        let issue = match recycle_pool.recycle().await {
                            Ok(workers) => {
                                info!(workers, "recycled workers after invalidation failure");
                                trace_store.record(
                                    &issue_trace,
                                    "worker",
                                    format!("recycled {workers} workers"),
                                );
                                (
                                    "RUV1706",
                                    "Worker cache invalidation failed; workers were restarted.",
                                )
                            }
                            Err(error) => {
                                warn!(%error, "worker recycle after invalidation failure failed");
                                trace_store.record(
                                    &issue_trace,
                                    "worker",
                                    format!("recycle failed: {error}"),
                                );
                                (
                                    "RUV1707",
                                    "Worker restart failed; restart the development server.",
                                )
                            }
                        };
                        let payload = hmr_payload(
                            &issue_update,
                            &issue_paths,
                            &issue_trace,
                            trace_ack,
                            Some(issue),
                        );
                        let _ = recycle_reload.send(payload);
                        trace_store.record(&issue_trace, "hmr", "message broadcast");
                    });
                } else {
                    // Send targeted HMR payload with affected routes.
                    let payload = hmr_payload(
                        &hmr_update,
                        &hmr_paths,
                        &trace_id,
                        config.debug_traces,
                        None,
                    );
                    let _ = reload_tx.send(payload);
                    edit_traces.record(&trace_id, "hmr", "message broadcast");
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

/// Serialize the HMR wire message in one place so the watcher, browser runtime,
/// and shared contract fixture cannot drift independently.
static NEXT_HMR_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn hmr_update_kind(update: &HmrUpdate, paths: &[String]) -> &'static str {
    if update.full_reload {
        return "restart";
    }
    match update.event_type {
        HmrEventType::CssUpdate => "css",
        HmrEventType::ComponentUpdate => {
            let server_route = paths.iter().any(|path| {
                path.split('/').any(|segment| segment == "server")
                    || path.ends_with("/action.ts")
                    || path.ends_with("/action.js")
                    || path.ends_with("/route.ts")
                    || path.ends_with("/route.js")
            });
            if server_route {
                "server-route"
            } else {
                "client-boundary"
            }
        }
        HmrEventType::FullReload => "restart",
    }
}

fn hmr_payload(
    update: &HmrUpdate,
    paths: &[String],
    trace_id: &str,
    trace_ack: bool,
    issue: Option<(&'static str, &'static str)>,
) -> String {
    let sequence = NEXT_HMR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let (message_type, kind) = if issue.is_some() {
        ("issues", "issues")
    } else {
        let kind = hmr_update_kind(update, paths);
        if kind == "restart" {
            ("restart", kind)
        } else {
            ("partial", kind)
        }
    };
    let mut payload = serde_json::json!({
        "protocol": "ruvyxa.hmr",
        "protocolVersion": 1,
        "sequence": sequence,
        "traceId": trace_id,
        "traceAck": trace_ack,
        "type": message_type,
        "kind": kind,
        "modules": paths,
        "paths": paths,
        "affectedRoutes": update.affected_routes,
        "fullReload": update.full_reload,
    });
    if let Some((code, message)) = issue {
        payload["issues"] = serde_json::json!([{ "code": code, "message": message }]);
    }
    payload.to_string()
}

fn watch_paths(config: &ServerConfig) -> Vec<PathBuf> {
    let mut paths = vec![config.root.clone()];
    paths.retain(|path| path.exists());
    paths.sort();
    paths.dedup();
    paths
}

fn ignored_watch_path(root: &Path, path: &Path) -> bool {
    let canonical_root = ruvyxa_diagnostics::normalized_canonical_path(root);
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

fn instrumentation_source_changed(root: &Path, paths: &[PathBuf]) -> bool {
    let root = ruvyxa_diagnostics::normalized_canonical_path(root);
    paths.iter().any(|path| {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        if !INSTRUMENTATION_FILES.contains(&file_name) {
            return false;
        }
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        absolute
            .parent()
            .map(ruvyxa_diagnostics::normalized_canonical_path)
            .as_deref()
            == Some(root.as_path())
    })
}

fn plugin_watch_paths(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.strip_prefix(root).unwrap_or(path))
        .map(|path| path.display().to_string().replace('\\', "/"))
        .collect()
}

fn format_update_elapsed(elapsed: Duration) -> String {
    if elapsed >= Duration::from_millis(1) {
        return format!("{}ms", elapsed.as_millis());
    }
    let tenths = elapsed.as_micros().div_ceil(100).max(1);
    format!("{}.{:01}ms", tenths / 10, tenths % 10)
}

async fn hmr_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    // Cross-site pages can open WebSockets to localhost and, unlike fetch,
    // read the messages. Reject handshakes without same-origin evidence so
    // project structure never leaks to other sites open in the browser.
    if hmr_origin_is_cross_site(&headers, &state.config, peer.ip()) {
        return with_security_headers(
            (StatusCode::FORBIDDEN, "Cross-origin HMR connection blocked").into_response(),
        );
    }
    ws.on_upgrade(move |mut socket| async move {
        let mut reload_rx = state.reload_tx.subscribe();

        while let Ok(payload) = reload_rx.recv().await {
            if socket.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    })
}

#[derive(Debug, Default, Deserialize)]
struct RealtimeQuery {
    channels: Option<String>,
}

async fn realtime_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<RealtimeQuery>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let Some(runtime) = state.realtime.clone() else {
        return (StatusCode::NOT_FOUND, "Realtime is not enabled").into_response();
    };
    if hmr_origin_is_cross_site(&headers, &state.config, peer.ip()) {
        return with_security_headers(
            (
                StatusCode::FORBIDDEN,
                "Cross-origin realtime connection blocked",
            )
                .into_response(),
        );
    }
    let channels = match parse_realtime_channels(query.channels.as_deref()) {
        Ok(channels) => channels,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    ws.on_upgrade(move |mut socket| async move {
        let mut receiver = runtime.tx.subscribe();
        let mut heartbeat = tokio::time::interval(runtime.heartbeat);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                received = receiver.recv() => {
                    match received {
                        Ok(payload) if realtime_payload_matches(&payload, &channels) => {
                            if socket.send(Message::Text(payload.into())).await.is_err() {
                                break;
                            }
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            if socket.send(Message::Text(
                                r#"{"version":1,"type":"resync","reason":"lagged"}"#.into(),
                            )).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = heartbeat.tick() => {
                    if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

#[derive(Deserialize)]
struct PresenceQuery {
    room: Option<String>,
}

/// Serve one collaboration room membership over a bidirectional socket.
///
/// Unlike the realtime transport, this socket reads. Everything a client sends
/// is validated, rate limited, and applied to process-local room state before
/// it is fanned out, so a peer can never make the server retain unbounded data
/// or forward a frame the room's own encoder did not produce.
async fn presence_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<PresenceQuery>,
    axum::extract::ConnectInfo(peer_addr): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let Some(runtime) = state.presence.clone() else {
        return (StatusCode::NOT_FOUND, "Presence is not enabled").into_response();
    };
    if hmr_origin_is_cross_site(&headers, &state.config, peer_addr.ip()) {
        return with_security_headers(
            (
                StatusCode::FORBIDDEN,
                "Cross-origin presence connection blocked",
            )
                .into_response(),
        );
    }
    let Some(room) = query.room.filter(|room| collab::valid_room_id(room)) else {
        return (
            StatusCode::BAD_REQUEST,
            "Presence requires a room of 1-128 letters, digits, colon, dot, underscore, slash, or dash",
        )
            .into_response();
    };
    ws.on_upgrade(move |socket| async move {
        use futures_util::{SinkExt, StreamExt};

        let registry = runtime.registry;
        let (mut sender, mut receiver) = socket.split();
        // Seat the peer only once the upgrade succeeded, so a handshake that
        // never completes cannot leave an occupant behind in the room.
        let seat = match registry.join(&room) {
            Ok(seat) => seat,
            Err(message) => {
                let _ = sender.send(Message::Text(error_frame(message).into())).await;
                let _ = sender.close().await;
                return;
            }
        };
        if sender
            .send(Message::Text(seat.welcome.clone().into()))
            .await
            .is_err()
        {
            registry.leave(&room, &seat.peer);
            return;
        }
        let mut feed = seat.receiver;
        let mut heartbeat = tokio::time::interval(runtime.heartbeat);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut limiter = FrameRateLimiter::new(Instant::now());

        loop {
            tokio::select! {
                broadcast = feed.recv() => {
                    match broadcast {
                        Ok(payload) => {
                            if sender.send(Message::Text(payload.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // The peer fell behind the room's frame buffer, so
                            // its view is now unreconstructable from the feed
                            // alone; tell it to reconnect for a fresh snapshot.
                            let _ = sender.send(Message::Text(
                                r#"{"version":1,"type":"resync","reason":"lagged"}"#.into(),
                            )).await;
                            break;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                inbound = receiver.next() => {
                    let Some(Ok(message)) = inbound else { break };
                    let Message::Text(payload) = message else { continue };
                    if !limiter.allow(Instant::now()) {
                        let _ = sender.send(Message::Text(
                            error_frame("Collaboration frame rate exceeded").into(),
                        )).await;
                        break;
                    }
                    let frame = match parse_client_frame(&payload) {
                        Ok(frame) => frame,
                        Err(message) => {
                            if sender.send(Message::Text(error_frame(message).into())).await.is_err() {
                                break;
                            }
                            continue;
                        }
                    };
                    match frame {
                        ParsedFrame::Presence(presence) => {
                            registry.update_presence(&room, &seat.peer, presence);
                        }
                        ParsedFrame::Set(entries) => {
                            if let Err(message) = registry.write_state(&room, &seat.peer, entries)
                                && sender.send(Message::Text(error_frame(message).into())).await.is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    if sender.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        registry.leave(&room, &seat.peer);
    })
}

fn error_frame(message: &str) -> String {
    serde_json::json!({ "version": 1, "type": "error", "message": message }).to_string()
}

fn parse_realtime_channels(
    value: Option<&str>,
) -> std::result::Result<HashSet<String>, &'static str> {
    let Some(value) = value else {
        return Err("Realtime requires at least one channel");
    };
    let requested = value.split(',').collect::<Vec<_>>();
    if requested.is_empty()
        || requested.len() > 16
        || requested.iter().any(|channel| channel.is_empty())
    {
        return Err("Realtime requires between 1 and 16 non-empty channels");
    }
    let channels = requested
        .into_iter()
        .map(str::to_string)
        .collect::<HashSet<_>>();
    if channels.len() > 16
        || channels
            .iter()
            .any(|channel| !valid_realtime_channel(channel))
    {
        return Err("Realtime accepts at most 16 channels of 128 bytes each");
    }
    Ok(channels)
}

fn valid_realtime_channel(channel: &str) -> bool {
    !channel.is_empty()
        && channel.len() <= 128
        && channel.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'/' | b'-')
        })
}

fn realtime_payload_matches(payload: &str, subscriptions: &HashSet<String>) -> bool {
    if subscriptions.is_empty() {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| {
            value
                .get("channels")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .is_some_and(|channels| {
            channels.iter().any(|channel| {
                channel
                    .as_str()
                    .is_some_and(|channel| subscriptions.contains(channel))
            })
        })
}

#[derive(Debug, Deserialize)]
struct ClientBundleQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
struct FlightQuery {
    path: String,
    artifact: String,
}

#[derive(Debug, Deserialize)]
struct DynamicImageQuery {
    src: String,
    #[serde(rename = "w")]
    width: u32,
    #[serde(rename = "q")]
    quality: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActionQuery {
    pub(crate) path: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TraceQuery {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct TraceAck {
    trace_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditTraceResponse {
    contract: &'static str,
    schema_version: u32,
    traces: Vec<trace::EditTrace>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeTrace {
    path: String,
    matched: bool,
    route: Option<RouteEntry>,
    params: RouteParams,
    runtime: &'static str,
    assets: TraceAssets,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceAssets {
    public_dir: String,
    app_dir: String,
}

/// Serve the client route table so the browser router can match and load
/// routes without a document load, the same as a production build.
///
/// Production publishes this file to `/__ruvyxa/client/route-manifest.json`;
/// the dev server has no such static file, so it synthesizes an equivalent from
/// the live route manifest. Each page route points at the on-demand bundle
/// endpoint keyed by its pattern — the generated bundle registers itself under
/// that pattern, which is what `@ruvyxa/react`'s router looks up.
async fn client_manifest(State(state): State<Arc<AppState>>) -> Response {
    let routes = match state.runtime_cache.router(&state.config).await {
        Ok((manifest, _)) => {
            let eligible = manifest
                .routes
                .iter()
                .filter(|route| route.kind == RouteKind::Page && route.render.hydrate)
                .cloned()
                .collect::<Vec<_>>();
            let mut entries = Vec::with_capacity(eligible.len());
            for route in eligible {
                let Ok(script) = render_client_bundle_pooled(&state, &route.path).await else {
                    continue;
                };
                let artifact = &blake3::hash(script.html.as_bytes()).to_hex()[..16];
                let source = tokio::fs::read_to_string(&route.file)
                    .await
                    .unwrap_or_default();
                let module = ruvyxa_bundler::ast::parse_module(&source);
                entries.push(serde_json::json!({
                    "path": route.path,
                    "src": format!(
                        "/__ruvyxa/client?path={}",
                        url_encode_component(&route.path)
                    ),
                    "artifactVersion": artifact,
                    "flight": ruvyxa_bundler::ast::has_named_runtime_export(&source, &module, "flight"),
                    "cache": ruvyxa_bundler::reference_manifest::has_module_directive(&source, "use cache"),
                }));
            }
            entries
        }
        Err(error) => {
            error!(%error, "client manifest request failed");
            Vec::new()
        }
    };

    let body = serde_json::json!({ "routes": routes }).to_string();
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    // Never cache: routes appear and disappear as files are added during dev.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_security_headers(response)
}

/// Serve the same public Flight contract in dev/start that deployment adapters expose.
async fn flight_endpoint(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FlightQuery>,
    headers: HeaderMap,
) -> Response {
    if headers.contains_key(header::AUTHORIZATION) || headers.contains_key(header::COOKIE) {
        return with_security_headers(
            (
                StatusCode::FORBIDDEN,
                "Flight requests must not include private request state",
            )
                .into_response(),
        );
    }
    if headers
        .get("x-ruvyxa-flight")
        .and_then(|value| value.to_str().ok())
        != Some("1")
    {
        return with_security_headers(
            (
                StatusCode::BAD_REQUEST,
                "Flight requests require the Ruvyxa navigation header",
            )
                .into_response(),
        );
    }
    if query.artifact.len() != 16
        || !query
            .artifact
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return with_security_headers(
            (
                StatusCode::BAD_REQUEST,
                "Flight request has an invalid artifact",
            )
                .into_response(),
        );
    }
    let request_path = match canonical_request_path(&query.path) {
        Ok(path) => path,
        Err(_) => {
            return with_security_headers(
                (
                    StatusCode::BAD_REQUEST,
                    "Flight request has an invalid route",
                )
                    .into_response(),
            );
        }
    };
    let (manifest, router) = match state.runtime_cache.router(&state.config).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            error!(%error, "Flight route snapshot failed");
            return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    let Some(route_match) = router.find(&manifest, &request_path) else {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    };
    if route_match.route.kind != RouteKind::Page {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }
    let source = tokio::fs::read_to_string(&route_match.route.file)
        .await
        .unwrap_or_default();
    let module = ruvyxa_bundler::ast::parse_module(&source);
    if !ruvyxa_bundler::ast::has_named_runtime_export(&source, &module, "flight") {
        return with_security_headers(
            (
                StatusCode::NOT_IMPLEMENTED,
                "This route does not expose a Flight payload",
            )
                .into_response(),
        );
    }
    let script = match render_client_bundle_pooled(&state, &route_match.route.path).await {
        Ok(script) => script,
        Err(error) => {
            error!(%error, path = %request_path, "Flight client artifact failed");
            return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    let current_artifact = &blake3::hash(script.html.as_bytes()).to_hex()[..16];
    if current_artifact != query.artifact {
        return with_security_headers(
            (StatusCode::CONFLICT, "Flight artifact is stale or invalid").into_response(),
        );
    }
    let response = state
        .worker_pool
        .render_flight(RenderFlightRequest {
            project_root: &state.config.root,
            app_dir: &state.config.app_dir,
            page_file: &route_match.route.file,
            request_path: &request_path,
            route_path: &route_match.route.path,
            params: &route_match.params,
            artifact_version: current_artifact,
        })
        .await;
    match response {
        Ok(response) if response.ok => {
            let Some(payload) = response.flight else {
                return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
            };
            let mut response = payload.into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/vnd.ruvyxa.flight+json; charset=utf-8"),
            );
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
            response
                .headers_mut()
                .insert(header::VARY, HeaderValue::from_static("x-ruvyxa-flight"));
            with_security_headers(response)
        }
        Ok(response) => {
            error!(code = ?response.code, message = ?response.message, path = %request_path, "Flight render failed");
            with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(error) => {
            error!(%error, path = %request_path, "Flight worker failed");
            with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

async fn client_bundle(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ClientBundleQuery>,
) -> Response {
    let response = match render_client_bundle_pooled(&state, &query.path).await {
        Ok(script) => {
            if state.config.watch {
                state.devtools.record_bundle(&query.path, script.html.len());
            }
            // The client bundle is cached behind an `Arc<str>`; serve that
            // allocation instead of copying the whole script per request.
            let mut response = shared_text_body(script.html).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript; charset=utf-8"),
            );
            response
        }
        Err(error) => {
            error!(%error, path = %query.path, "client bundle request failed");
            let message = public_internal_error(&state.config, &error);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("console.error({message:?});"),
            )
                .into_response()
        }
    };
    with_security_headers(response)
}

async fn hydration_loader() -> Response {
    let mut response = hydration_loader_source().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    with_security_headers(response)
}

async fn dynamic_image_endpoint(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DynamicImageQuery>,
    headers: HeaderMap,
) -> Response {
    if query
        .quality
        .is_some_and(|quality| !(1..=100).contains(&quality))
    {
        return with_security_headers(
            (
                StatusCode::BAD_REQUEST,
                "image quality must be between 1 and 100",
            )
                .into_response(),
        );
    }
    let bytes = match dynamic_image::optimize(
        &state.config.public_dir,
        &state.config.dynamic_images,
        &state.dynamic_image_cache,
        &query.src,
        query.width,
        query.quality,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(DynamicImageError::InvalidRequest(message)) => {
            return with_security_headers((StatusCode::BAD_REQUEST, message).into_response());
        }
        Err(DynamicImageError::NotFound) => {
            return with_security_headers(StatusCode::NOT_FOUND.into_response());
        }
        Err(DynamicImageError::TooLarge) => {
            return with_security_headers(
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "image exceeds runtime limits",
                )
                    .into_response(),
            );
        }
        Err(DynamicImageError::Decode) => {
            return with_security_headers(
                (
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "image could not be decoded",
                )
                    .into_response(),
            );
        }
        Err(DynamicImageError::Io(error)) => {
            error!(%error, src = %query.src, "dynamic image read failed");
            return with_security_headers(StatusCode::NOT_FOUND.into_response());
        }
        Err(DynamicImageError::Worker) => {
            error!(src = %query.src, "dynamic image worker failed");
            return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    let etag = static_assets::compute_etag(&bytes);
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|value| static_assets::etag_matches(value, &etag))
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=60, stale-while-revalidate=86400"),
        );
        response.headers_mut().insert(
            header::ETAG,
            HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        return with_security_headers(response);
    }
    let mut response = bytes.as_ref().to_vec().into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/webp"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=60, stale-while-revalidate=86400"),
    );
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    with_security_headers(response)
}

async fn action_endpoint(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    Query(query): Query<ActionQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = validate_action_request(&headers, body.len(), &state.config, peer) {
        return with_security_headers(response);
    }

    let (content_type, payload) = match validate_action_payload(&headers, &body) {
        Ok(payload) => payload,
        Err(response) => return with_security_headers(*response),
    };

    let rate_key = action_rate_limit_key(peer, &headers, &query, &state.config);
    let retry_after = {
        let Ok(mut limiter) = state.action_limiter.lock() else {
            error!("action rate limiter mutex poisoned; rejecting request");
            return with_security_headers(
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Service temporarily unavailable",
                )
                    .into_response(),
            );
        };
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

    if let Some(provided_reference) = query.id.as_deref() {
        let (manifest, router) = match state.runtime_cache.route_snapshot(&state.config).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                error!(%error, "action reference route snapshot failed");
                return with_security_headers(
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response(),
                );
            }
        };
        let Some(route_match) = router.find(&manifest, &query.path) else {
            return with_security_headers(
                (StatusCode::NOT_FOUND, "Route not found for action").into_response(),
            );
        };
        let Some(action_file) = render_pipeline::action_file_for(route_match.route) else {
            return with_security_headers(
                (StatusCode::NOT_FOUND, "Route action file was not found").into_response(),
            );
        };
        let source = match tokio::fs::read_to_string(&action_file).await {
            Ok(source) => source,
            Err(error) => {
                error!(%error, file = %action_file.display(), "action reference source read failed");
                return with_security_headers(
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response(),
                );
            }
        };
        if action_reference_id(&route_match.route.id, &source) != provided_reference {
            return with_security_headers(
                (StatusCode::CONFLICT, "Action reference is stale or invalid").into_response(),
            );
        }
        let replay = state
            .action_replays
            .lock()
            .map_err(|_| "Action replay protection is unavailable")
            .and_then(|mut guard| guard.consume(&headers, provided_reference));
        if let Err(message) = replay {
            let status = if message == "Action request replayed" {
                StatusCode::CONFLICT
            } else if message == "Action replay protection is unavailable" {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            };
            return with_security_headers((status, message).into_response());
        }
    }

    let action_started = Instant::now();
    let (response, action_error) = match render_server_action_pooled(
        &state,
        &query.path,
        &query.name,
        &payload,
        content_type,
        &headers,
    )
    .await
    {
        Ok(response) => (response, false),
        Err(error) => {
            error!(
                %error,
                path = %query.path,
                action = %query.name,
                "server action request failed"
            );
            let message = public_internal_error(&state.config, &error);
            (
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("console.error({message:?});"),
                )
                    .into_response(),
                true,
            )
        }
    };
    if state.config.watch {
        state.devtools.record_action(
            &query.path,
            &query.name,
            action_started.elapsed(),
            action_error,
        );
    }
    with_security_headers(response)
}

async fn devtools_dashboard(State(state): State<Arc<AppState>>) -> Response {
    if !state.config.watch {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }
    let mut response = dashboard_html().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_security_headers(response)
}

async fn devtools_data(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !state.config.watch || hmr_origin_is_cross_site(&headers, &state.config, peer.ip()) {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }
    let routes = match state.runtime_cache.router(&state.config).await {
        Ok((manifest, _)) => manifest
            .routes
            .iter()
            .map(|route| {
                serde_json::json!({
                    "path": route.path,
                    "kind": format!("{:?}", route.kind).to_lowercase(),
                    "strategy": format!("{:?}", route.render.strategy).to_lowercase(),
                    "file": route.file.strip_prefix(&state.config.root)
                        .unwrap_or(&route.file).display().to_string(),
                })
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            error!(%error, "devtools route snapshot failed");
            Vec::new()
        }
    };
    let snapshot = state.devtools.snapshot(
        serde_json::Value::Array(routes),
        state.render_cache.snapshot().await,
    );
    let mut response = json_response(StatusCode::OK, &snapshot);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_security_headers(response)
}

async fn trace_endpoint(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TraceQuery>,
) -> Response {
    if !debug_traces_enabled(&state.config) {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }
    if query.kind.as_deref() == Some("edits") {
        let snapshot = EditTraceResponse {
            contract: "ruvyxa.edit-trace",
            schema_version: 1,
            traces: state.edit_traces.snapshot(query.path.as_deref()),
        };
        let mut response = json_response(StatusCode::OK, &snapshot);
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return with_security_headers(response);
    }
    if query.kind.is_some() {
        return with_security_headers(
            (StatusCode::BAD_REQUEST, "Unknown trace kind").into_response(),
        );
    }
    let Some(path) = query.path.as_deref() else {
        return with_security_headers(
            (StatusCode::BAD_REQUEST, "Trace path is required").into_response(),
        );
    };
    let response = match runtime_trace_cached(&state.config, &state.runtime_cache, path).await {
        Ok(trace) => json_response(StatusCode::OK, &trace),
        Err(error) => {
            error!(%error, path, "runtime trace request failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("console.error({:?});", error.to_string()),
            )
                .into_response()
        }
    };
    with_security_headers(response)
}

async fn trace_ack_endpoint(
    State(state): State<Arc<AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !debug_traces_enabled(&state.config) {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }
    if hmr_origin_is_cross_site(&headers, &state.config, peer.ip()) {
        return with_security_headers(
            (
                StatusCode::FORBIDDEN,
                "Cross-origin trace acknowledgement blocked",
            )
                .into_response(),
        );
    }
    let acknowledgement = match serde_json::from_slice::<TraceAck>(&body) {
        Ok(value) if valid_trace_id(&value.trace_id) => value,
        _ => {
            return with_security_headers(
                (StatusCode::BAD_REQUEST, "Invalid trace acknowledgement").into_response(),
            );
        }
    };
    if !state
        .edit_traces
        .record(&acknowledgement.trace_id, "browser", "message received")
    {
        return with_security_headers(StatusCode::NOT_FOUND.into_response());
    }
    with_security_headers(StatusCode::NO_CONTENT.into_response())
}

fn valid_trace_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
    let mut request_path = match canonical_request_path(parts.uri.path()) {
        Ok(path) => path,
        Err(error) => {
            return with_security_headers(
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid request path: {error}"),
                )
                    .into_response(),
            );
        }
    };
    // Routing and static-file lookup must use only the path, while an API handler's
    // standard Request must retain the original query string.
    let mut request_target = parts
        .uri
        .path_and_query()
        .map(|target| target.as_str().to_string())
        .unwrap_or_else(|| request_path.clone());
    let mut request_body = if request_method_allows_body(&method) {
        match to_bytes(body, state.config.api_body_limit_bytes).await {
            Ok(bytes) if bytes.is_empty() => None,
            Ok(bytes) => Some(bytes.to_vec()),
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

    // The plugin round-trip serializes the request over stdio, so it only runs
    // when the registry declared request middleware whose routes can match.
    let mut plugin_request: Option<PluginHttpRequest> = None;
    if state
        .plugin_runtime
        .as_deref()
        .is_some_and(|runtime| runtime.wants_request(&request_path))
    {
        let initial_request = PluginHttpRequest {
            method: method.clone(),
            path: request_target.clone(),
            headers: headers_to_plugin_pairs(&headers),
            body_base64: request_body.as_deref().map(encode_plugin_body),
        };
        let (short_circuit, next_request) =
            match apply_request_plugins(&state, initial_request).await {
                Ok(result) => result,
                Err(error) => {
                    error!(%error, path = %request_path, "TypeScript request middleware failed");
                    let message = public_internal_error(&state.config, &error);
                    return with_security_headers(
                        (StatusCode::INTERNAL_SERVER_ERROR, message).into_response(),
                    );
                }
            };
        if let Some(response) = short_circuit {
            return response;
        }
        let (next_method, next_target) =
            match split_plugin_target(&next_request.method, &next_request.path) {
                Ok(value) => value,
                Err(error) => {
                    return with_security_headers(
                        (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
                    );
                }
            };
        method = next_method;
        request_target = next_target.clone();
        request_path = next_target
            .split_once('?')
            .map_or_else(|| next_target.clone(), |(path, _)| path.to_string());
        headers = plugin_headers(&next_request.headers);
        request_body = match decode_plugin_body(next_request.body_base64.as_deref()) {
            Ok(value) => value,
            Err(error) => {
                return with_security_headers(
                    (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
                );
            }
        };
        plugin_request = Some(next_request);
    }

    let render_result = render_request_pooled(
        &state,
        &request_path,
        &request_target,
        &method,
        &headers,
        request_body.as_deref(),
    )
    .await;
    let response = match render_result {
        Ok(response) => response,
        Err(error) => {
            error!(%error, path = %request_path, "request rendering failed");
            let is_dev = state.config.watch && state.config.error_overlay;
            match &error {
                RuvyxaError::Diagnostic(diag) => {
                    error_response(StatusCode::INTERNAL_SERVER_ERROR, diag, is_dev)
                }
                _ => {
                    let body = if is_dev {
                        dev_error_overlay(&error.to_string(), None, None, None)
                    } else {
                        plain_error_page("Internal server error")
                    };
                    html_response(StatusCode::INTERNAL_SERVER_ERROR, body)
                }
            }
        }
    };
    // Response middleware is gated on the (possibly rewritten) final path so
    // route-scoped plugins never force non-matching responses through the
    // buffering base64 round-trip.
    let response = if state
        .plugin_runtime
        .as_deref()
        .is_some_and(|runtime| runtime.wants_response(&request_path))
    {
        let request_payload = plugin_request.unwrap_or_else(|| PluginHttpRequest {
            method: method.clone(),
            path: request_target.clone(),
            headers: headers_to_plugin_pairs(&headers),
            body_base64: request_body.as_deref().map(encode_plugin_body),
        });
        match apply_response_plugins(&state, &request_payload, response).await {
            Ok(response) => response,
            Err(error) => {
                error!(%error, path = %request_path, "TypeScript response middleware failed");
                with_security_headers(
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        public_internal_error(&state.config, &error),
                    )
                        .into_response(),
                )
            }
        }
    } else {
        response
    };
    if state.config.watch && should_log_dev_request(&request_path) {
        println!(
            "{}",
            dev_page_request_log(&method, &request_path, response.status(), started.elapsed())
        );
    }
    response
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderName;
    use base64::Engine;
    use std::time::SystemTime;

    /// Both hosts answer to the same table of framework endpoints.
    ///
    /// The two request hosts drifted silently once already: `/__ruvyxa/action`
    /// lived in the axum router below and had no counterpart in
    /// `serverless-handler.mjs`, so every deployed server action answered 404
    /// while working under `ruvyxa dev`. Nothing compared the two lists.
    /// `tests/fixtures/framework-endpoint-conformance.json` is now that
    /// comparison, replayed here and by
    /// `tests/packages/ruvyxa/framework-endpoints.test.mjs`.
    #[test]
    fn reserved_routes_match_the_shared_framework_endpoint_contract() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/framework-endpoint-conformance.json"
        ))
        .expect("the framework endpoint contract must be valid JSON");

        let reserved = contract["endpoints"]
            .as_array()
            .expect("endpoints must be an array")
            .iter()
            .filter(|endpoint| endpoint["reserved"].as_bool() == Some(true))
            .map(|endpoint| {
                endpoint["path"]
                    .as_str()
                    .expect("every endpoint must have a path")
                    .to_string()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            RESERVED_FRAMEWORK_ROUTES.to_vec(),
            reserved,
            "RESERVED_FRAMEWORK_ROUTES and the endpoint contract disagree; \
             update tests/fixtures/framework-endpoint-conformance.json first"
        );
    }

    /// Every endpoint the contract marks `native` is registered on the router.
    ///
    /// Read from the source of `serve` rather than by building a router: axum
    /// exposes no way to enumerate registered paths, and a `ServerConfig` needs
    /// a project on disk. Checking the text still catches the failure that
    /// matters — an endpoint named by the contract that no host ever wired up.
    #[test]
    fn every_contract_endpoint_is_registered_on_the_native_router() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/framework-endpoint-conformance.json"
        ))
        .expect("the framework endpoint contract must be valid JSON");
        let source = include_str!("lib.rs");

        for endpoint in contract["endpoints"]
            .as_array()
            .expect("endpoints must be an array")
        {
            let path = endpoint["path"].as_str().expect("path must be a string");
            if endpoint["native"].as_str().is_none() {
                continue;
            }
            assert!(
                source.contains(&format!(".route(\"{path}\""))
                    || source.contains(&format!("\"{path}\",")),
                "{path} is in the endpoint contract but is not registered by serve()"
            );
        }
    }

    #[test]
    fn validates_realtime_event_metadata_and_channel_filters() {
        let payload = r#"{"version":1,"type":"action","channels":["todos"],"action":"save","path":"/todos","invalidated":[]}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        assert_eq!(decode_realtime_event(&encoded).unwrap(), payload);
        assert!(decode_realtime_event("not-base64!").is_err());

        let subscriptions = HashSet::from(["todos".to_string()]);
        assert!(realtime_payload_matches(payload, &subscriptions));
        assert!(!realtime_payload_matches(
            r#"{"version":1,"type":"action","channels":["users"]}"#,
            &subscriptions
        ));
        assert!(parse_realtime_channels(Some("todos,users")).is_ok());
        assert!(parse_realtime_channels(None).is_err());
        assert!(parse_realtime_channels(Some("todos,,users")).is_err());
        assert!(parse_realtime_channels(Some(&"a".repeat(129))).is_err());
    }

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

    #[tokio::test]
    async fn production_errors_do_not_expose_internal_details() {
        let config = ServerConfig::production(".", "127.0.0.1", 3000);
        let error =
            RuvyxaError::Message("database password from C:\\secrets\\production.env".to_string());

        assert_eq!(
            public_internal_error(&config, &error),
            "Internal server error"
        );
        assert_eq!(
            public_internal_error(&ServerConfig::dev(".", "127.0.0.1", 3000), &error),
            error.to_string()
        );

        let diagnostic = Diagnostic::new("RUV9999", "sensitive compiler detail")
            .explain("private path C:\\workspace\\secret.ts");
        let response = error_response(StatusCode::INTERNAL_SERVER_ERROR, &diagnostic, false);
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Internal server error"));
        assert!(!body.contains("sensitive compiler detail"));
        assert!(!body.contains("secret.ts"));
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
        assert_eq!(trace.params.get("slug"), Some(&serde_json::json!("hello")));
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
            export EXPORTED_TOKEN=shell-style
            export=literal-export-key
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
        // Shell-sourceable files prefix assignments with `export`; the prefix
        // is not part of the key.
        assert_eq!(env.get("EXPORTED_TOKEN"), Some(&"shell-style".to_string()));
        assert!(!env.contains_key("export EXPORTED_TOKEN"));
        assert_eq!(env.get("export"), Some(&"literal-export-key".to_string()));
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
    }

    #[test]
    fn hmr_payload_matches_the_shared_wire_contract() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/hmr-contract.json"))
                .unwrap();
        assert_eq!(fixture["protocol"], "ruvyxa.hmr");
        assert_eq!(fixture["protocolVersion"], 1);
        assert_eq!(fixture["fallback"], "reload");
        let required_fields = fixture["requiredFields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect::<BTreeSet<_>>();

        for event in fixture["messages"].as_array().unwrap() {
            let (event_type, full_reload, path) = match event["event"].as_str().unwrap() {
                "css" => (HmrEventType::CssUpdate, false, "app/global.css"),
                "client" => (HmrEventType::ComponentUpdate, false, "app/Button.tsx"),
                "server" => (HmrEventType::ComponentUpdate, false, "app/server/data.ts"),
                "structural" => (HmrEventType::FullReload, true, "app/layout.tsx"),
                "failure" => (HmrEventType::FullReload, true, "app/page.tsx"),
                kind => panic!("unknown fixture HMR event: {kind}"),
            };
            let update = HmrUpdate {
                affected_routes: vec!["/docs".to_string()],
                full_reload,
                changed_files: vec![PathBuf::from(path)],
                event_type,
            };
            let payload: serde_json::Value = serde_json::from_str(&hmr_payload(
                &update,
                &[path.to_string()],
                "0123456789abcdef0123456789abcdef",
                false,
                (event["event"] == "failure").then_some((
                    "RUV1706",
                    "Worker cache invalidation failed; workers were restarted.",
                )),
            ))
            .unwrap();
            let actual_fields: BTreeSet<&str> = payload
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert!(required_fields.is_subset(&actual_fields));
            assert_eq!(payload["protocol"], fixture["protocol"]);
            assert_eq!(payload["protocolVersion"], fixture["protocolVersion"]);
            assert_eq!(payload["traceId"], "0123456789abcdef0123456789abcdef");
            assert_eq!(payload["type"], event["type"]);
            assert_eq!(payload["kind"], event["kind"]);
            if event["event"] == "failure" {
                assert_eq!(payload["issues"][0]["code"], "RUV1706");
                assert_eq!(payload["fullReload"], true);
            }
            assert!(
                payload["sequence"]
                    .as_u64()
                    .is_some_and(|sequence| sequence > 0)
            );
        }
    }

    #[test]
    fn blocks_cross_origin_action_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );

        assert!(action_origin_is_cross_site(
            &headers,
            &ServerConfig::dev(".", "localhost", 3000),
            "127.0.0.1".parse().unwrap(),
        ));
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

        assert!(!action_origin_is_cross_site(
            &headers,
            &ServerConfig::dev(".", "localhost", 3000),
            "127.0.0.1".parse().unwrap(),
        ));
        assert!(action_content_type_is_supported(&headers));
        assert!(
            validate_action_request(
                &headers,
                128,
                &ServerConfig::dev(".", "localhost", 3000),
                "127.0.0.1:3000".parse().unwrap(),
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_actions_without_same_origin_evidence() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let config = ServerConfig::dev(".", "localhost", 3000);
        let peer = "127.0.0.1:3000".parse().unwrap();

        assert!(validate_action_request(&headers, 2, &config, peer).is_some());

        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(validate_action_request(&headers, 2, &config, peer).is_none());
    }

    #[test]
    fn rejects_missing_ambiguous_and_malformed_action_payloads() {
        let headers = HeaderMap::new();
        assert!(!action_content_type_is_supported(&headers));
        assert!(validate_action_payload(&headers, b"{}").is_err());

        let mut json_headers = HeaderMap::new();
        json_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(validate_action_payload(&json_headers, b"title=form").is_err());
        assert!(validate_action_payload(&json_headers, &[0xff, 0xfe]).is_err());
        assert_eq!(
            validate_action_payload(&json_headers, b"").unwrap(),
            ("application/json", "{}".to_string())
        );

        let mut form_headers = HeaderMap::new();
        form_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        assert_eq!(
            validate_action_payload(&form_headers, b"null").unwrap(),
            ("application/x-www-form-urlencoded", "null".to_string())
        );
    }

    /// The scheme is only compared when a trusted proxy actually reported it.
    ///
    /// This test previously asserted the opposite — that a `https` Origin with a
    /// matching Host is cross-site whenever no trusted proxy sent
    /// `X-Forwarded-Proto`. That encoded a false rejection: Ruvyxa never
    /// terminates TLS, so it cannot observe the browser's scheme on its own, and
    /// treating the missing evidence as proof of `http` returned 403 for every
    /// server action behind a TLS-terminating proxy that is not loopback and not
    /// listed in `security.trustedProxyIps`.
    #[test]
    fn compares_the_origin_scheme_only_when_a_trusted_proxy_reported_it() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://localhost:3000"),
        );

        let mut config = ServerConfig::dev(".", "localhost", 3000);

        // No trusted proxy vouched for a scheme: the matching host is the
        // same-origin evidence, and the request is accepted.
        assert!(!action_origin_is_cross_site(
            &headers,
            &config,
            "127.0.0.1".parse().unwrap(),
        ));
        assert!(!action_origin_is_cross_site(
            &headers,
            &config,
            "10.0.0.9".parse().unwrap(),
        ));

        // A trusted proxy reporting the same scheme keeps the request valid.
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(!action_origin_is_cross_site(
            &headers,
            &config,
            "127.0.0.1".parse().unwrap(),
        ));

        // A trusted proxy reporting a different scheme is a real mismatch.
        headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
        assert!(action_origin_is_cross_site(
            &headers,
            &config,
            "127.0.0.1".parse().unwrap(),
        ));

        // The same header from an untrusted peer carries no weight, so the
        // request falls back to the host comparison and is accepted.
        assert!(!action_origin_is_cross_site(
            &headers,
            &config,
            "10.0.0.9".parse().unwrap(),
        ));

        // Once that peer is trusted, its report is honored again.
        config.trusted_proxies = TrustedProxies::parse_all(["10.0.0.0/8"]).unwrap();
        assert!(action_origin_is_cross_site(
            &headers,
            &config,
            "10.0.0.9".parse().unwrap(),
        ));
    }

    /// A mismatched host stays cross-site no matter what the scheme says: this
    /// is the check that actually stops CSRF.
    #[test]
    fn blocks_cross_host_action_requests_regardless_of_scheme_evidence() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("app.example.com"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example.net"),
        );
        let mut config = ServerConfig::dev(".", "localhost", 3000);
        config.trusted_proxies = TrustedProxies::parse_all(["10.0.0.0/8"]).unwrap();

        assert!(action_origin_is_cross_site(
            &headers,
            &config,
            "10.0.0.9".parse().unwrap(),
        ));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(action_origin_is_cross_site(
            &headers,
            &config,
            "10.0.0.9".parse().unwrap(),
        ));
    }

    /// The deployment shape the old scheme assertion broke: a container-network
    /// proxy terminating TLS, reachable through a configured CIDR range.
    #[test]
    fn accepts_actions_behind_a_container_network_tls_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("app.example.com"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.example.com"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));

        let mut config = ServerConfig::production(".", "0.0.0.0", 3000);
        assert!(config.same_origin_actions, "default must stay fail-closed");

        // Unconfigured: accepted on the host match alone.
        assert!(!action_origin_is_cross_site(
            &headers,
            &config,
            "172.18.0.4".parse().unwrap(),
        ));

        // Configured as a CIDR range, which is what a container network needs.
        config.trusted_proxies = TrustedProxies::parse_all(["172.16.0.0/12"]).unwrap();
        assert!(!action_origin_is_cross_site(
            &headers,
            &config,
            "172.18.0.4".parse().unwrap(),
        ));
        assert_eq!(
            action_rate_limit_key(
                "172.18.0.4:5000".parse().unwrap(),
                &HeaderMap::from_iter([(
                    axum::http::HeaderName::from_static("x-forwarded-for"),
                    HeaderValue::from_static("203.0.113.8"),
                )]),
                &ActionQuery {
                    path: "/todos".to_string(),
                    name: "create".to_string(),
                    id: None,
                },
                &config,
            ),
            "203.0.113.8:/todos:create",
            "a proxy trusted by range must also be trusted for forwarded identity"
        );
    }

    #[test]
    fn accepts_forwarded_headers_only_from_trusted_proxies() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.8"));
        let query = ActionQuery {
            path: "/todos".to_string(),
            name: "create".to_string(),
            id: None,
        };
        let peer: SocketAddr = "10.0.0.9:5000".parse().unwrap();
        let mut config = ServerConfig::dev(".", "localhost", 3000);

        assert_eq!(
            action_rate_limit_key(peer, &headers, &query, &config),
            "10.0.0.9:/todos:create"
        );

        config.trusted_proxies = TrustedProxies::parse_all(["10.0.0.9"]).unwrap();
        assert_eq!(
            action_rate_limit_key(peer, &headers, &query, &config),
            "203.0.113.8:/todos:create"
        );
    }

    #[test]
    fn forwarded_client_ip_ignores_client_forged_prefix_entries() {
        // The client sent its own X-Forwarded-For value and the trusted proxy
        // appended the address it actually saw. The forged prefix must not
        // become the rate-limit identity, or rotating it defeats the limiter.
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.99, 203.0.113.8"),
        );
        let query = ActionQuery {
            path: "/todos".to_string(),
            name: "create".to_string(),
            id: None,
        };
        let peer: SocketAddr = "10.0.0.9:5000".parse().unwrap();
        let mut config = ServerConfig::dev(".", "localhost", 3000);
        config.trusted_proxies = TrustedProxies::parse_all(["10.0.0.9"]).unwrap();

        assert_eq!(
            action_rate_limit_key(peer, &headers, &query, &config),
            "203.0.113.8:/todos:create"
        );

        // A two-hop chain of trusted proxies: skip our own addresses from the
        // right and land on the first external one.
        let mut chained = HeaderMap::new();
        chained.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.99, 203.0.113.8, 10.0.0.9"),
        );
        assert_eq!(
            action_rate_limit_key(peer, &chained, &query, &config),
            "203.0.113.8:/todos:create"
        );
    }

    #[tokio::test]
    async fn server_make_service_attaches_tcp_peer_metadata() {
        async fn peer_handler(
            axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
        ) -> String {
            peer.ip().to_string()
        }

        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let app = Router::new().route("/", get(peer_handler));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, server_make_service(app))
                .await
                .unwrap();
        });

        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();
        server.abort();
        let _ = server.await;

        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.ends_with("127.0.0.1"));
    }

    #[test]
    fn action_rate_limiter_limits_each_key_within_its_window() {
        let mut limiter = ActionRateLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.allow("client:/todos:create"));
        assert!(limiter.allow("client:/todos:create"));
        assert!(!limiter.allow("client:/todos:create"));
        assert!(limiter.retry_after_seconds("client:/todos:create") >= 59);
        // A different key keeps its own budget.
        assert!(limiter.allow("other:/todos:create"));
    }

    #[test]
    fn action_rate_limiter_releases_a_key_after_its_window_passes() {
        let mut limiter = ActionRateLimiter::new(1, Duration::from_millis(20));
        assert!(limiter.allow("client:/todos:create"));
        assert!(!limiter.allow("client:/todos:create"));
        // Two windows clears both counters, so the client starts over.
        std::thread::sleep(Duration::from_millis(60));
        assert!(limiter.allow("client:/todos:create"));
    }

    /// The reason the key-set design was replaced: a client flooding the limiter
    /// with distinct keys must never cause an unrelated client's first request
    /// to be rejected. The old limiter denied every new key once its 10,000-key
    /// map filled, turning address rotation into a lockout for bystanders.
    #[test]
    fn action_rate_limiter_never_denies_a_bystander_because_of_a_key_flood() {
        let mut limiter = ActionRateLimiter::new(ACTION_RATE_LIMIT_MAX, ACTION_RATE_LIMIT_WINDOW);

        // Far more distinct keys than the old 10,000-key cap allowed.
        for index in 0..50_000u32 {
            limiter.allow(&format!("attacker-{index}:/todos:create"));
        }

        assert!(
            limiter.allow("victim:/todos:create"),
            "a first-time client must still be admitted after a key flood"
        );
    }

    /// Memory must not scale with the number of clients seen.
    #[test]
    fn action_rate_limiter_memory_is_independent_of_client_count() {
        let mut limiter = ActionRateLimiter::new(ACTION_RATE_LIMIT_MAX, ACTION_RATE_LIMIT_WINDOW);
        for index in 0..50_000u32 {
            limiter.allow(&format!("client-{index}:/todos:create"));
        }
        assert!(
            limiter.occupied_slots() <= 8192,
            "slot count must stay bounded, got {}",
            limiter.occupied_slots()
        );
    }

    /// Slot sharing may limit a client early, but must never hand one a larger
    /// budget than configured.
    #[test]
    fn action_rate_limiter_collisions_never_grant_extra_budget() {
        let mut limiter = ActionRateLimiter::new(4, Duration::from_secs(60));
        let mut allowed = 0;
        for index in 0..20_000u32 {
            if limiter.allow(&format!("client-{index}:/todos:create")) {
                allowed += 1;
            }
        }
        // 8192 slots x 4 hits is the absolute ceiling; exceeding it would mean a
        // slot handed out more than `max_hits`.
        assert!(
            allowed <= 8192 * 4,
            "no slot may exceed its budget, allowed {allowed}"
        );
    }

    #[test]
    fn plugin_responses_reject_invalid_headers() {
        let response = PluginHttpResponse {
            status: 200,
            headers: vec![("bad header".to_string(), "value".to_string())],
            body_base64: Some(encode_plugin_body(b"body")),
        };
        assert!(plugin_response_into_response(response).is_err());
    }

    #[test]
    fn plugin_responses_preserve_repeated_headers() {
        let response = PluginHttpResponse {
            status: 200,
            headers: vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("set-cookie".to_string(), "session=one; Path=/".to_string()),
                ("set-cookie".to_string(), "theme=dark; Path=/".to_string()),
            ],
            body_base64: None,
        };

        let response = plugin_response_into_response(response).unwrap();
        let cookies = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(cookies, vec!["session=one; Path=/", "theme=dark; Path=/"]);
        assert_eq!(response.headers().get_all("content-type").iter().count(), 1);
        assert_eq!(response.headers()["content-type"], "application/json");
    }

    /// Yields each chunk in turn; a `None` chunk injects a stream error.
    struct ChunkStream(std::collections::VecDeque<Option<Bytes>>);
    impl futures_core::Stream for ChunkStream {
        type Item = std::result::Result<Bytes, std::io::Error>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::task::Poll::Ready(
                self.0.pop_front().map(|chunk| {
                    chunk.ok_or_else(|| std::io::Error::other("stream failed mid-body"))
                }),
            )
        }
    }

    fn chunked_body(chunks: &[&'static [u8]]) -> Body {
        Body::from_stream(ChunkStream(
            chunks.iter().map(|c| Some(Bytes::from_static(c))).collect(),
        ))
    }

    #[tokio::test]
    async fn plugin_response_body_buffers_within_the_limit() {
        match buffer_plugin_response_body(Body::from(vec![0_u8; 8]), 8)
            .await
            .unwrap()
        {
            BufferedPluginBody::Buffered(bytes) => assert_eq!(bytes.len(), 8),
            BufferedPluginBody::Oversized(_) => panic!("body at the limit must buffer"),
        }

        match buffer_plugin_response_body(chunked_body(&[b"hel", b"lo"]), 8)
            .await
            .unwrap()
        {
            BufferedPluginBody::Buffered(bytes) => assert_eq!(&bytes[..], b"hello"),
            BufferedPluginBody::Oversized(_) => panic!("multi-chunk body within limit must buffer"),
        }
    }

    #[tokio::test]
    async fn oversized_unsized_bodies_pass_through_with_all_bytes_intact() {
        let body = chunked_body(&[b"abc", b"def", b"ghi"]);
        match buffer_plugin_response_body(body, 4).await.unwrap() {
            BufferedPluginBody::Buffered(_) => panic!("body over the limit must not buffer"),
            BufferedPluginBody::Oversized(body) => {
                let bytes = to_bytes(body, usize::MAX).await.unwrap();
                assert_eq!(&bytes[..], b"abcdefghi");
            }
        }
    }

    #[tokio::test]
    async fn plugin_response_body_read_errors_still_fail() {
        let body = Body::from_stream(ChunkStream(
            [Some(Bytes::from_static(b"ok")), None]
                .into_iter()
                .collect(),
        ));
        let error = buffer_plugin_response_body(body, 64).await.unwrap_err();
        assert!(error.to_string().contains("stream failed mid-body"));
    }

    #[test]
    fn oversized_sized_bodies_bypass_response_plugins() {
        assert!(body_exceeds_plugin_limit(&Body::from(vec![0_u8; 9]), 8));
        assert!(!body_exceeds_plugin_limit(&Body::from(vec![0_u8; 8]), 8));
        assert!(!body_exceeds_plugin_limit(&Body::empty(), 8));

        // Unsized bodies have no exact size hint, so the fast path never
        // triggers; they go through the chunked buffering path instead.
        let body = chunked_body(&[b"chunk"]);
        assert!(!body_exceeds_plugin_limit(&body, 4));
    }

    #[test]
    fn server_configs_default_to_the_plugin_response_limit() {
        for config in [
            ServerConfig::dev(".", "localhost", 3000),
            ServerConfig::production(".", "localhost", 3000),
        ] {
            assert_eq!(
                config.plugin_response_body_limit_bytes,
                DEFAULT_PLUGIN_RESPONSE_BODY_LIMIT_BYTES
            );
        }
    }

    #[test]
    fn server_config_rejects_unbounded_security_limits() {
        let mut config = ServerConfig::dev(".", "localhost", 3000);
        config.action_body_limit_bytes = MAX_ACTION_BODY_LIMIT_BYTES + 1;
        assert!(config.validate_limits().is_err());

        config.action_body_limit_bytes = MAX_ACTION_BODY_BYTES;
        config.action_rate_limit_window =
            Duration::from_secs(MAX_ACTION_RATE_LIMIT_WINDOW_SECS + 1);
        assert!(config.validate_limits().is_err());
    }

    #[test]
    fn runtime_env_uses_the_configured_jsx_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::dev(temp.path(), "localhost", 3000);
        config.jsx_runtime = JsxRuntime::Classic;

        assert_eq!(
            runtime_env(&config)
                .unwrap()
                .get("RUVYXA_JSX_RUNTIME")
                .map(String::as_str),
            Some("classic")
        );
    }

    #[test]
    fn runtime_env_exposes_the_configured_javascript_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::dev(temp.path(), "localhost", 3000);
        config.runtime = JavaScriptRuntime::Bun;

        assert_eq!(
            runtime_env(&config)
                .unwrap()
                .get("RUVYXA_RUNTIME")
                .map(String::as_str),
            Some("bun")
        );
        assert_eq!(config.runtime.command(), "bun");
    }

    #[test]
    fn runtime_detection_prefers_node_then_bun_then_deno() {
        assert_eq!(
            JavaScriptRuntime::from_availability(true, true, true),
            JavaScriptRuntime::Node
        );
        assert_eq!(
            JavaScriptRuntime::from_availability(true, false, true),
            JavaScriptRuntime::Node
        );
        assert_eq!(
            JavaScriptRuntime::from_availability(false, true, true),
            JavaScriptRuntime::Bun
        );
        assert_eq!(
            JavaScriptRuntime::from_availability(false, false, true),
            JavaScriptRuntime::Deno
        );
        assert_eq!(
            JavaScriptRuntime::from_availability(false, false, false),
            JavaScriptRuntime::Node
        );
    }

    #[test]
    fn action_security_options_control_request_validation() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));

        let mut config = ServerConfig::dev(".", "localhost", 3000);
        config.action_body_limit_bytes = 8;
        assert!(
            validate_action_request(&headers, 9, &config, "127.0.0.1:3000".parse().unwrap())
                .is_some()
        );

        config.action_body_limit_bytes = MAX_ACTION_BODY_BYTES;
        config.same_origin_actions = false;
        config.fetch_metadata_actions = false;
        assert!(
            validate_action_request(&headers, 8, &config, "127.0.0.1:3000".parse().unwrap())
                .is_none()
        );
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
    fn canonical_request_path_decodes_segments_for_routing_and_prerendering() {
        assert_eq!(
            canonical_request_path("/blog/hello%20world").unwrap(),
            "/blog/hello world"
        );
        assert_eq!(
            canonical_request_path("/%E0%B8%97%E0%B8%94%E0%B8%AA%E0%B8%AD%E0%B8%9A").unwrap(),
            "/ทดสอบ"
        );

        let temp = tempfile::tempdir().unwrap();
        let page_dir = temp.path().join("blog").join("hello world");
        fs::create_dir_all(&page_dir).unwrap();
        fs::write(page_dir.join("index.html"), "rendered").unwrap();
        let path = canonical_request_path("/blog/hello%20world").unwrap();

        assert_eq!(
            serve_prerendered_html(temp.path(), &path),
            Some("rendered".to_string())
        );
    }

    #[test]
    fn canonical_request_path_rejects_encoded_boundaries_and_malformed_values() {
        for raw_path in [
            "/blog/%2Fsecret",
            "/blog/%5Csecret",
            "/blog/%2E%2E",
            "/blog/%00",
            "/blog/%",
            "/blog/%GG",
            "/blog/%FF",
        ] {
            assert!(
                canonical_request_path(raw_path).is_err(),
                "{raw_path} must be rejected"
            );
        }
    }

    #[test]
    fn prerendered_html_rejects_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("index.html"), "safe").unwrap();
        fs::write(temp.path().parent().unwrap().join("secret.html"), "secret").unwrap();

        assert_eq!(
            serve_prerendered_html(temp.path(), "/"),
            Some("safe".to_string())
        );
        assert_eq!(serve_prerendered_html(temp.path(), "/../secret.html"), None);
    }

    #[test]
    fn resolves_single_webp_outputs_and_development_sources() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("hero.png"), b"png").unwrap();
        let fallback = resolve_public_asset(temp.path(), "hero.webp").unwrap();
        assert!(fallback.ends_with("hero.png"));

        fs::remove_file(temp.path().join("hero.png")).unwrap();
        fs::write(temp.path().join("hero.webp"), b"webp").unwrap();
        let selected = resolve_public_asset(temp.path(), "hero.png").unwrap();
        assert!(selected.ends_with("hero.webp"));
    }

    #[test]
    fn rejects_ambiguous_development_image_sources() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("hero.png"), b"png").unwrap();
        fs::write(temp.path().join("hero.jpg"), b"jpg").unwrap();
        assert!(resolve_public_asset(temp.path(), "hero.webp").is_none());
    }

    #[test]
    fn resolves_uppercase_development_image_extensions() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("hero.PNG"), b"png").unwrap();
        let source = resolve_public_asset(temp.path(), "hero.webp").unwrap();
        assert!(source.ends_with("hero.PNG"));

        // An upper-case source must still be served as an image, not as a
        // binary download the browser refuses to render.
        assert_eq!(
            static_assets::content_type_for(&source),
            "image/png",
            "case-sensitive extension matching would break upper-case sources"
        );
    }

    // A request that reaches routing has already missed the client and public
    // directories, so an asset-shaped path has no file behind it. Letting
    // `/[lang]` answer `/logo.png` returns a 200 HTML document where the
    // browser expects image bytes. `serverless-handler.mjs` applies the same
    // rule, so `dev`, `start`, and every deploy target agree.
    #[test]
    fn classifies_asset_shaped_request_paths() {
        for asset in [
            "/logo.png",
            "/favicon.ico",
            "/nested/app.CSS",
            "/fonts/inter.woff2",
            // Well-known crawler files: `.txt`/`.xml` are not asset extensions,
            // but these exact paths must 404 rather than let `/[lang]` answer
            // them with an HTML page.
            "/robots.txt",
            "/sitemap.xml",
            "/sitemap.xml/",
        ] {
            assert!(static_assets::is_static_asset_request(asset), "{asset}");
        }

        // A page that merely ends in the same extension keeps matching.
        for page in ["/docs/robots.txt.md", "/feed.xml", "/blog/sitemap.xml"] {
            assert!(!static_assets::is_static_asset_request(page), "{page}");
        }

        for page in ["/", "/en/docs", "/readme.md", "/blog/post.", "/.env"] {
            assert!(!static_assets::is_static_asset_request(page), "{page}");
        }
    }

    #[test]
    fn serves_crawler_discovery_files_with_protocol_content_types() {
        assert_eq!(
            static_assets::content_type_for(Path::new("robots.txt")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            static_assets::content_type_for(Path::new("sitemap.xml")),
            "application/xml; charset=utf-8"
        );
    }

    #[test]
    fn rejects_public_assets_outside_the_configured_root() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        fs::create_dir_all(&public).unwrap();
        fs::write(temp.path().join("secret.txt"), b"secret").unwrap();

        assert!(resolve_public_asset(&public, "../secret.txt").is_none());
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
        assert_eq!(
            response.headers().get("x-frame-options"),
            Some(&HeaderValue::from_static("DENY"))
        );
        assert_eq!(
            response.headers().get("cross-origin-resource-policy"),
            Some(&HeaderValue::from_static("same-origin"))
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
        assert!(response.headers().get("x-frame-options").is_none());
        assert!(
            response
                .headers()
                .get("cross-origin-resource-policy")
                .is_none()
        );
    }

    #[test]
    fn explicit_security_headers_override_framework_defaults() {
        let mut response = StatusCode::OK.into_response();
        response.headers_mut().insert(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(self)"),
        );

        let response = finalize_security_headers(response, true);

        assert_eq!(
            response.headers().get("permissions-policy"),
            Some(&HeaderValue::from_static("camera=(self)"))
        );
        assert_eq!(
            response.headers().get(header::X_CONTENT_TYPE_OPTIONS),
            Some(&HeaderValue::from_static("nosniff"))
        );
    }

    #[test]
    fn disabling_framework_defaults_preserves_explicit_security_headers() {
        let mut response = StatusCode::OK.into_response();
        response.headers_mut().insert(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(self)"),
        );

        let response = finalize_security_headers(response, false);

        assert_eq!(
            response.headers().get("permissions-policy"),
            Some(&HeaderValue::from_static("camera=(self)"))
        );
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
            validate_action_request(
                &headers,
                128,
                &ServerConfig::dev(".", "localhost", 3000),
                "127.0.0.1:3000".parse().unwrap(),
            )
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
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/incomplete.js"}]}"#,
        )
        .unwrap();
        assert!(prebuilt_client_assets(&config, "/").is_none());
    }

    #[test]
    fn client_manifest_cache_serves_repeated_reads() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            client_dir.join("manifest.json"),
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.123.js"}]}]}"#,
        )
        .unwrap();
        let config = ServerConfig::production(temp.path(), "localhost", 3000);

        // Two reads of an unchanged manifest must both resolve, exercising the
        // fingerprint-match cache-hit path on the second call.
        for _ in 0..2 {
            let assets = prebuilt_client_assets(&config, "/").unwrap();
            assert_eq!(assets.src, "/__ruvyxa/client/home.js");
            assert_eq!(assets.preloads, vec!["/__ruvyxa/client/shared.123.js"]);
        }
    }

    #[test]
    fn client_manifest_cache_refreshes_after_rebuild() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        let manifest = client_dir.join("manifest.json");
        std::fs::write(
            &manifest,
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/old.js","sharedChunks":[]}]}"#,
        )
        .unwrap();
        let config = ServerConfig::production(temp.path(), "localhost", 3000);
        assert_eq!(
            prebuilt_client_assets(&config, "/").unwrap().src,
            "/__ruvyxa/client/old.js"
        );

        // A rebuild rewrites the manifest, so the cache picks up the new asset
        // URLs instead of serving the previous build's bundles.
        std::fs::write(
            &manifest,
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/rebuilt.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.abc.js"}]}]}"#,
        )
        .unwrap();
        let assets = prebuilt_client_assets(&config, "/").unwrap();
        assert_eq!(assets.src, "/__ruvyxa/client/rebuilt.js");
        assert_eq!(assets.preloads, vec!["/__ruvyxa/client/shared.abc.js"]);
    }

    #[test]
    fn client_manifest_cache_refreshes_on_same_length_rebuild() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        let manifest = client_dir.join("manifest.json");

        // The realistic rebuild shape: only the content hash inside the bundle
        // URL changes, so the rewritten manifest has the exact same byte
        // length. A (mtime, len) fingerprint can therefore only detect this
        // rebuild when the filesystem's mtime resolution is finer than the gap
        // between the two writes, which is not guaranteed on FAT or on some
        // network and container mounts. Keying the cache on the content hash
        // makes the invalidation exact regardless of mtime resolution.
        let before = r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.a1b2c3.js","sharedChunks":[]}]}"#;
        let after = r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.d4e5f6.js","sharedChunks":[]}]}"#;
        assert_eq!(before.len(), after.len(), "test inputs must be same length");

        std::fs::write(&manifest, before).unwrap();
        let config = ServerConfig::production(temp.path(), "localhost", 3000);
        assert_eq!(
            prebuilt_client_assets(&config, "/").unwrap().src,
            "/__ruvyxa/client/home.a1b2c3.js"
        );

        // Restore the original mtime so the rewrite is indistinguishable from
        // the first write by metadata alone, emulating a coarse-resolution
        // filesystem without depending on the host's actual timestamp
        // granularity.
        let original = std::fs::metadata(&manifest).unwrap().modified().unwrap();
        std::fs::write(&manifest, after).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&manifest)
            .unwrap()
            .set_modified(original)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&manifest).unwrap().modified().unwrap(),
            original,
            "mtime must be restored for this test to exercise the hash path"
        );

        assert_eq!(
            prebuilt_client_assets(&config, "/").unwrap().src,
            "/__ruvyxa/client/home.d4e5f6.js"
        );
    }

    #[test]
    fn settled_client_manifest_is_served_without_rereading_it() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        let manifest = client_dir.join("manifest.json");
        let source =
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/settled.js","sharedChunks":[]}]}"#;
        std::fs::write(&manifest, source).unwrap();

        // Backdate past the settle window so the first load is allowed to
        // record `(len, mtime)`. A build writes this file and then exits, so an
        // old timestamp is the production steady state, not a contrivance.
        let settled = SystemTime::now() - Duration::from_secs(3600);
        let backdate = |at: SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(&manifest)
                .unwrap()
                .set_modified(at)
                .unwrap();
        };
        backdate(settled);

        let config = ServerConfig::production(temp.path(), "localhost", 3000);
        assert_eq!(
            prebuilt_client_assets(&config, "/").unwrap().src,
            "/__ruvyxa/client/settled.js"
        );

        // Replace the bytes with garbage of the same length and put the
        // timestamp back. Nothing outside a test can produce this — a real
        // rewrite moves mtime forward — so serving the cached parse here is the
        // proof that the second call never read or re-parsed the file.
        std::fs::write(&manifest, "x".repeat(source.len())).unwrap();
        backdate(settled);
        assert_eq!(
            prebuilt_client_assets(&config, "/").unwrap().src,
            "/__ruvyxa/client/settled.js",
            "a settled fingerprint must answer from the cache without a read"
        );

        // The moment the timestamp moves — which is what an actual rebuild
        // does — the fast path stops applying and the bytes decide again.
        backdate(SystemTime::now());
        assert!(
            prebuilt_client_assets(&config, "/").is_none(),
            "a changed timestamp must send the lookup back to the bytes"
        );
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

        // `export const hydrate = false` pages get no hydration payload at all.
        let mut no_hydrate = route.clone();
        no_hydrate.render.hydrate = false;
        assert_eq!(
            client_hydration_script(&config, &no_hydrate, "/", &BTreeMap::new()),
            ""
        );
    }

    #[test]
    fn deferred_hydration_uses_loader_without_preloading_route_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            client_dir.join("manifest.json"),
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.js"}],"hydration":"idle","hydrationLoader":"/__ruvyxa/client/hydration.js"}]}"#,
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
            render: ruvyxa_graph::RenderMeta {
                hydration: ruvyxa_graph::HydrationMode::Idle,
                ..Default::default()
            },
        };

        let script = client_hydration_script(&config, &route, "/", &BTreeMap::new());

        assert!(!script.contains("modulepreload"), "{script}");
        assert!(script.contains("hydration.js?strategy=idle&amp;src=/__ruvyxa/client/home.js"));
        assert!(!script.contains(r#"src="/__ruvyxa/client/home.js""#));
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

        assert_eq!(cache.router(&config).await.unwrap().0.routes.len(), 1);

        let about = app.join("about");
        std::fs::create_dir_all(&about).unwrap();
        std::fs::write(
            about.join("page.tsx"),
            "export default function About() { return <main /> }",
        )
        .unwrap();

        assert_eq!(cache.router(&config).await.unwrap().0.routes.len(), 1);
        cache.invalidate_async().await;
        assert_eq!(cache.router(&config).await.unwrap().0.routes.len(), 2);
    }

    #[test]
    fn cache_slot_rejects_work_started_before_invalidation() {
        let mut slot = CacheSlot::default();
        let stale_generation = slot.generation;

        slot.invalidate();

        assert!(!slot.insert_if_current(stale_generation, "stale"));
        assert!(slot.value.is_none());
        assert!(slot.insert_if_current(slot.generation, "fresh"));
        assert_eq!(slot.value, Some("fresh"));
    }

    #[tokio::test]
    async fn uncached_routes_compile_the_router_from_the_same_manifest_generation() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let z_route = app.join("z");
        std::fs::create_dir_all(&z_route).unwrap();
        std::fs::write(
            z_route.join("page.tsx"),
            "export default function Zed() { return <main /> }",
        )
        .unwrap();

        let mut config = ServerConfig::dev(temp.path(), "localhost", 3000);
        config.cache_route_manifest = false;
        let initial = discover_routes(discover_options(&config)).unwrap();
        let cache = RuntimeCache::with_manifest(initial);

        // `/a` sorts before `/z`, shifting the original router's route index.
        // A router compiled from the startup manifest would now return `/a`
        // when asked to resolve `/z` against the freshly discovered manifest.
        let a_route = app.join("a");
        std::fs::create_dir_all(&a_route).unwrap();
        std::fs::write(
            a_route.join("page.tsx"),
            "export default function A() { return <main /> }",
        )
        .unwrap();

        let (manifest, router) = cache.router(&config).await.unwrap();
        let matched = router.find(&manifest, "/z").unwrap();

        assert_eq!(manifest.routes.len(), 2);
        assert_eq!(matched.route.path, "/z");
        assert!(matched.route.file.ends_with("z/page.tsx"));
    }

    #[tokio::test]
    async fn refresh_hmr_manifest_reconciles_routes_without_losing_bundle_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        let home_page = app.join("page.tsx");
        std::fs::write(
            &home_page,
            "export default function Home() { return <main /> }",
        )
        .unwrap();

        let config = ServerConfig::dev(temp.path(), "localhost", 3000);
        let initial = discover_routes(discover_options(&config)).unwrap();
        let cache = RuntimeCache::with_manifest(initial.clone());
        let tracker = HmrTracker::new();
        tracker.populate_from_manifest(&initial.routes);

        let server_dependency = temp.path().join("lib").join("home-data.ts");
        tracker.register_route("/", std::slice::from_ref(&server_dependency));

        let about = app.join("about");
        std::fs::create_dir_all(&about).unwrap();
        let about_page = about.join("page.tsx");
        std::fs::write(
            &about_page,
            "export default function About() { return <main /> }",
        )
        .unwrap();

        cache.invalidate_async().await;
        refresh_hmr_manifest(&config, &cache, &tracker)
            .await
            .unwrap();

        assert_eq!(tracker.tracked_route_count(), 2);
        assert_eq!(
            tracker.compute_update(&[server_dependency]).affected_routes,
            vec!["/".to_string()],
            "refreshing route discovery must preserve a live route's worker graph"
        );
        assert_eq!(
            tracker.compute_update(&[about_page]).affected_routes,
            vec!["/about".to_string()],
            "new routes must become targetable immediately after manifest refresh"
        );
    }

    /// The favicon link is derived from a filesystem stat, and every page render
    /// used to redo it. Caching it means the answer is computed once and only
    /// recomputed when the watcher invalidates the runtime cache.
    #[tokio::test]
    async fn runtime_cache_resolves_public_asset_links_once_until_invalidated() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        std::fs::create_dir_all(&public).unwrap();

        let config = ServerConfig::dev(temp.path(), "localhost", 3000);
        let cache = RuntimeCache::default();

        assert_eq!(&*cache.asset_links(&config).await, "");

        // Adding the icon after the first resolution must not change the cached
        // answer, which is what proves the stat is not repeated per render.
        std::fs::write(public.join("ruvyxa.png"), [0u8; 4]).unwrap();
        assert_eq!(&*cache.asset_links(&config).await, "");

        cache.invalidate_async().await;
        assert!(
            cache.asset_links(&config).await.contains("/ruvyxa.png"),
            "invalidation must pick up the new asset"
        );

        // Repeat reads share the cached allocation.
        let first = cache.asset_links(&config).await;
        let second = cache.asset_links(&config).await;
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn runtime_cache_invalidates_styles_only_for_collected_dependencies() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let styles = temp.path().join("styles");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(app.join("page.tsx"), "import '../styles/site.css'").unwrap();
        let stylesheet = styles.join("site.css");
        std::fs::write(&stylesheet, "body { color: navy; }").unwrap();

        let config = ServerConfig::dev(temp.path(), "localhost", 3000);
        let cache = Arc::new(RuntimeCache::default());
        assert!(cache.styles(&config).await.unwrap().contains("navy"));

        let unrelated = app.join("page.tsx");
        let unchanged_cache = cache.clone();
        assert!(
            !tokio::task::spawn_blocking(move || {
                unchanged_cache.invalidate_styles_for_paths(&[unrelated])
            })
            .await
            .unwrap()
        );
        assert!(
            cache
                .styles
                .read()
                .await
                .value
                .as_ref()
                .is_some_and(|cached| cached.css.contains("navy"))
        );

        std::fs::write(&stylesheet, "body { color: teal; }").unwrap();
        let changed_cache = cache.clone();
        assert!(
            tokio::task::spawn_blocking(move || {
                changed_cache.invalidate_styles_for_paths(&[stylesheet])
            })
            .await
            .unwrap()
        );
        assert!(cache.styles(&config).await.unwrap().contains("teal"));

        cache.invalidate_async().await;
        assert!(cache.styles.read().await.value.is_none());
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

    #[test]
    fn trace_acknowledgements_require_lowercase_w3c_ids() {
        assert!(valid_trace_id("0123456789abcdef0123456789abcdef"));
        assert!(!valid_trace_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!valid_trace_id("0123456789abcdef"));
        assert!(!valid_trace_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));

        assert!(
            serde_json::from_str::<TraceAck>(r#"{"traceId":"0123456789abcdef0123456789abcdef"}"#)
                .is_ok()
        );
        assert!(
            serde_json::from_str::<TraceAck>(
                r#"{"traceId":"0123456789abcdef0123456789abcdef","extra":true}"#
            )
            .is_err()
        );
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

    /// The reported bug: another project holds `127.0.0.1:3000`, Ruvyxa binds
    /// `[::1]:3000` because that is what `localhost` resolved to first, both
    /// succeed, and `http://localhost:3000` reaches whichever server the
    /// browser's resolver happens to pick. A port is only free when it is free
    /// on every address the host answers to.
    #[tokio::test]
    async fn bind_listener_moves_on_when_the_other_loopback_family_is_taken() {
        let Ok(occupied) = TcpListener::bind("127.0.0.1:0").await else {
            return;
        };
        let port = occupied.local_addr().unwrap().port();
        if port == u16::MAX {
            return;
        }
        // Nothing to prove on a host that cannot serve IPv6 loopback at all.
        let Ok(probe) = TcpListener::bind(("::1".parse::<std::net::IpAddr>().unwrap(), 0)).await
        else {
            return;
        };
        drop(probe);

        let config = ServerConfig::dev(".", "localhost", port);
        let requested: SocketAddr = format!("[::1]:{port}").parse().unwrap();
        let (_listener, bound_address) = bind_listener(&config, requested).await.unwrap();

        assert_ne!(
            bound_address.port(),
            port,
            "the IPv4 loopback holder must push the server to another port"
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
    fn instrumentation_watcher_filenames_match_the_shared_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/instrumentation-files-conformance.json"
        ))
        .unwrap();
        let files = fixture["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(files, INSTRUMENTATION_FILES);

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for file in INSTRUMENTATION_FILES {
            assert!(instrumentation_source_changed(root, &[root.join(file)]));
        }
        assert!(!instrumentation_source_changed(
            root,
            &[root.join("app/instrumentation.ts")]
        ));
        assert!(!instrumentation_source_changed(
            root,
            &[root.join("instrumentation.ts.bak")]
        ));
    }

    #[test]
    fn plugin_file_change_paths_are_project_relative_and_portable() {
        let root = PathBuf::from("C:/workspace/app");
        let paths = vec![root.join("content/guide.md"), root.join("app/page.tsx")];

        assert_eq!(
            plugin_watch_paths(&root, &paths),
            vec!["content/guide.md", "app/page.tsx"]
        );
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
