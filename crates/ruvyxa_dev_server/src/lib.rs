#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::{BTreeSet, HashSet};
#[cfg(test)]
use std::fs;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
#[cfg(test)]
use axum::body::Bytes;
use axum::body::{Body, to_bytes};
use axum::extract::{DefaultBodyLimit, State};
#[cfg(test)]
use axum::http::{HeaderMap, HeaderValue, header};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use ruvyxa_bundler::JsxRuntime;
#[cfg(test)]
use ruvyxa_diagnostics::Diagnostic;
use ruvyxa_diagnostics::{Result, RuvyxaError};
#[cfg(test)]
use ruvyxa_graph::RouteEntry;
use ruvyxa_graph::{
    DiscoverOptions, I18nRouting, RenderStrategy, RouteKind, RouteManifest, discover_routes,
};
#[cfg(test)]
use ruvyxa_middleware::PluginHttpResponse;
use ruvyxa_middleware::{
    MiddlewareConfig, MiddlewareStack, PluginEnvironment, PluginHost, PluginHttpRequest,
};
use serde::Deserialize;
#[cfg(test)]
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

mod collab;
use collab::CollabRegistry;
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
pub mod image_decode;
#[cfg(test)]
use env_file::parse_env_source;
pub use env_file::project_env;

mod action_security;
use action_security::{ActionRateLimiter, ActionReplayGuard};
pub use action_security::{IpPrefix, TrustedProxies, action_reference_id};
#[cfg(test)]
use action_security::{
    action_content_type_is_supported, action_fetch_site_is_cross_site, action_origin_is_cross_site,
};
#[cfg(test)]
use action_security::{action_rate_limit_key, validate_action_payload, validate_action_request};
use devtools::DevToolsMetrics;
use dynamic_image::DynamicImageCache;
pub use dynamic_image::DynamicImageConfig;

mod cli_output;
use cli_output::{
    accent, dim, enabled_text, info, link, middleware_summary, note, number, ok, paint, path_text,
    print_field, print_header, warn_text,
};

mod port_binding;
use port_binding::bind_listeners;
#[cfg(test)]
use port_binding::{PORT_FALLBACK_SCAN_LIMIT, port_conflict_diagnostic};

mod document_stream;
mod html_document;
mod i18n;
mod trace;
pub use html_document::{
    BOOTSTRAP_ELEMENT_ID, bootstrap_data_block, escape_html, hydration_loader_source,
    hydration_loader_url, localize_document, rsc_payload_block, safe_json_for_script,
};
#[cfg(test)]
use html_document::{
    client_hydration_script, compose_document, dev_diagnostic_overlay, prebuilt_client_assets,
};
use html_document::{dev_error_overlay, error_response, plain_error_page, public_internal_error};

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
pub use static_assets::{
    DEFAULT_VIEWPORT_META, document_head_defaults, public_asset_links, style_head_tag,
};
#[cfg(test)]
use static_assets::{is_safe_relative_path, resolve_public_asset};

mod worker_protocol;

mod worker_pool;
pub use worker_pool::{NodeWorkerPool, RenderSsgRequest};
pub use worker_protocol::{
    ServerReferenceSource, StaticParamSegment, StaticParamsRoute, WarmupRoute,
};

mod render_pipeline;
#[cfg(test)]
use render_pipeline::serve_prerendered_html;
pub use render_pipeline::{
    RenderContext, apply_production_node_env, find_runtime_script, render_request_with_context,
};
use render_pipeline::{render_request_pooled, runtime_env};

mod router;
pub use router::RadixRouter;

mod render_cache;
mod response;
// Re-exported so every existing `crate::html_response`-style path keeps
// resolving: moving these into a module is a relocation, not an interface change.
pub use render_cache::RenderCache;
pub(crate) use response::{
    apply_security_headers, cached_html_response, finalize_security_headers, html_response,
    streamed_html_response, uncacheable, with_security_headers,
};

mod hmr_tracker;
pub use hmr_tracker::{HmrEventType, HmrTracker, HmrUpdate};

mod watcher;
use watcher::{WatcherRuntime, format_update_elapsed, start_watcher, watch_paths};

mod realtime_endpoints;
use realtime_endpoints::{hmr_ws, presence_ws, realtime_ws};

mod framework_endpoints;
// `ActionQuery` and the runtime-trace shapes are re-exported at the crate root
// because `action_security` and `render_pipeline` name them through `crate::`.
pub(crate) use framework_endpoints::{ActionQuery, RuntimeTrace, TraceAssets};
use framework_endpoints::{
    action_endpoint, client_bundle, client_manifest, client_vendor, devtools_dashboard,
    devtools_data, dynamic_image_endpoint, flight_endpoint, health_endpoint, hydration_loader,
    rsc_action_endpoint, rsc_payload_endpoint, trace_ack_endpoint, trace_endpoint,
};

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
/// How long in-flight work may finish after a shutdown signal, before the
/// remaining connections are dropped.
///
/// The standalone host's default, read from the same `RUVYXA_SHUTDOWN_GRACE`,
/// so one project is bounded the same way under `ruvyxa start` as it is under
/// its own build. It was 5 s here and 25 s there, which cut a six-second render
/// off on one host and let it finish on the other. Platforms send SIGTERM and
/// then SIGKILL after their own grace period — commonly 30 s — so this stays
/// under the usual floor.
const SERVER_SHUTDOWN_GRACE: Duration = Duration::from_secs(25);

/// How long the host keeps accepting, and answering, after a shutdown signal
/// before it stops accepting.
///
/// Closing the listener on the tick the drain flag is set makes the draining
/// status unreachable: a readiness probe is by definition a fresh connection,
/// so it is refused rather than told to stop routing here, and everything the
/// orchestrator sends while it is still deregistering fails instead of being
/// retried against another instance. `standalone-server.ts` fixed exactly this
/// and the fix landed only on the JavaScript side.
const SERVER_DRAIN_DELAY: Duration = Duration::from_secs(5);

/// Waiters allowed per admitted request before the host sheds load.
///
/// The standalone host's ratio, for the standalone host's reason: four waiters
/// per slot absorbs an ordinary burst, and past that a caller is told to come
/// back rather than being parked on memory this process would have to keep.
const ADMISSION_QUEUE_PER_SLOT: usize = 4;

/// The floor and ceiling on the default admission width.
///
/// A render is CPU-bound, so admitting more than the machine can run only slows
/// down the ones already going; the same two bounds the standalone host uses.
const ADMISSION_DEFAULT_BOUNDS: std::ops::RangeInclusive<usize> = 2..=8;

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
    ///
    /// After that comes the directory each runtime's own installer writes to.
    /// Both add it to `PATH`, and a shell that has not been restarted since —
    /// or a tool launched from one that never had it — sees neither the
    /// executable nor the shim. On the machine this was written on, Deno 2.9.5
    /// was installed at `~/.deno/bin/deno.exe` and `ruvyxa doctor` reported it
    /// missing, so `--runtime deno` could not be selected at all.
    #[must_use]
    pub fn executable(self) -> std::path::PathBuf {
        match self {
            Self::Node => std::path::PathBuf::from(self.command()),
            Self::Bun => {
                #[cfg(windows)]
                if let Some(executable) = bun_executable_from_path() {
                    return executable;
                }
                installer_home_executable(self)
                    .unwrap_or_else(|| std::path::PathBuf::from(self.command()))
            }
            Self::Deno => {
                #[cfg(windows)]
                if let Some(executable) = deno_executable_from_path() {
                    return executable;
                }
                installer_home_executable(self)
                    .unwrap_or_else(|| std::path::PathBuf::from(self.command()))
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
    ///
    /// Answered once per process. Every probe is a `--version` process spawn,
    /// and a single build asks this question from route discovery, style
    /// collection, bundling, and pre-rendering; the set of installed runtimes
    /// cannot change underneath one command, so asking again only costs
    /// spawns. A build of the demo used to spend a fifth of its warm total
    /// here.
    #[must_use]
    pub fn detect() -> Self {
        static DETECTED: std::sync::OnceLock<JavaScriptRuntime> = std::sync::OnceLock::new();
        *DETECTED.get_or_init(|| Self::detect_by(|runtime| runtime.is_available()))
    }

    /// The detection rule, asking `available` about one runtime at a time.
    ///
    /// The probe is consulted in preference order and stops at the first
    /// runtime that answers, so the ordinary case — Node installed — costs one
    /// spawn instead of three. Taking the probe as an argument is also what
    /// lets the rule be tested without three runtimes on the machine.
    #[must_use]
    pub fn detect_by(mut available: impl FnMut(Self) -> bool) -> Self {
        Self::DETECTION_ORDER
            .into_iter()
            .find(|runtime| available(*runtime))
            .unwrap_or(Self::Node)
    }

    #[must_use]
    pub fn from_availability(
        node_available: bool,
        bun_available: bool,
        deno_available: bool,
    ) -> Self {
        Self::detect_by(|runtime| match runtime {
            Self::Node => node_available,
            Self::Bun => bun_available,
            Self::Deno => deno_available,
        })
    }

    /// Preference order for auto-detection, stated once so the eager and lazy
    /// callers cannot come to disagree about it.
    const DETECTION_ORDER: [Self; 3] = [Self::Node, Self::Bun, Self::Deno];
}

/// Where a runtime's own installer puts it, under a home directory.
///
/// Split from the lookup so the rule can be tested without a home directory to
/// stand in for the real one. Bun installs to `~/.bun/bin` and Deno to
/// `~/.deno/bin`; both are named after the runtime, so the directory follows
/// from [`JavaScriptRuntime::command`] rather than from a second list that
/// could disagree with it.
fn runtime_home_executable(
    runtime: JavaScriptRuntime,
    home: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let file = if cfg!(windows) {
        format!("{}.exe", runtime.command())
    } else {
        runtime.command().to_string()
    };
    let candidate = home
        .join(format!(".{}", runtime.command()))
        .join("bin")
        .join(file);
    candidate.is_file().then_some(candidate)
}

fn installer_home_executable(runtime: JavaScriptRuntime) -> Option<std::path::PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    runtime_home_executable(runtime, std::path::Path::new(&home))
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

    /// Hand a freshly discovered manifest to the observer.
    ///
    /// Public because an observer composes: `ruvyxa dev` installs one that
    /// regenerates both the typed-routes file and the discovery documents, and
    /// the composed observer forwards to the one it wraps.
    pub fn notify(&self, manifest: &RouteManifest) {
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
    /// Where generated discovery files (`sitemap.xml`, `robots.txt`) are
    /// written for `ruvyxa dev`, consulted after `public/`.
    ///
    /// A build writes them into the published assets directory, so
    /// `ruvyxa start` and every deployment serve them like any other file.
    /// Development had no such directory and answered both with 404, so the one
    /// command a project runs while working on its SEO output was the one
    /// command that could not show it.
    pub discovery_dir: Option<PathBuf>,
    pub client_dir: PathBuf,
    /// Directory containing pre-rendered HTML files from the build step.
    pub prerender_dir: PathBuf,
    pub host: String,
    pub port: u16,
    pub watch: bool,
    pub cache_route_manifest: bool,
    pub cache_css: bool,
    /// `cache.maxEntries` — entries the in-memory `cache()` tier may hold.
    ///
    /// Carried here for the same reason `es_target` is: the value is spent
    /// inside a JavaScript worker and decided by `ruvyxa.config.ts`, so the
    /// only way it reaches the worker is through the environment
    /// [`crate::render_pipeline::runtime_env`] builds. Until it did, the two
    /// bounds were read by `documentCacheHandlerPrelude` alone — the deployed
    /// build's registry — so a project that shrank or disabled the tier got the
    /// bound it asked for on every platform except the two hosts this crate
    /// serves, `ruvyxa dev` and `ruvyxa start`, whose worker pool is
    /// long-lived and is exactly where an unbounded tier grows.
    pub data_cache_max_entries: Option<u32>,
    /// `cache.maxBytes` — the memory ceiling the entry count cannot express.
    pub data_cache_max_bytes: Option<u64>,
    /// `cache.handler` — the project module this host reads and writes shared
    /// cache state through, already resolved to an absolute path.
    ///
    /// Absolute because the worker resolves it as a module URL from a working
    /// directory this crate does not choose, and validated by the caller before
    /// it lands here: a handler that names no file is a configuration error the
    /// operator should see once at startup rather than once per render worker.
    ///
    /// The other half of the same setting. `data_cache_max_entries` above
    /// arrived first and carried only the numbers, so the handler itself
    /// reached every deployed platform and neither of the two hosts this crate
    /// serves — an application running several `ruvyxa start` instances behind
    /// one load balancer declared a shared store, was told by the build that it
    /// had one, and got per-instance caching.
    pub data_cache_handler: Option<PathBuf>,
    /// Prepended to every key this server hands the shared store.
    ///
    /// `<build id>:`, the same shape `documentCacheHandlerPrelude` writes into a
    /// deployed build's registry, so one project deployed two ways addresses one
    /// store the same way. Two deployments pointed at one managed Redis
    /// otherwise both write `cache('user:1')` and read each other's answer.
    ///
    /// Only meaningful beside `data_cache_handler`, and required whenever that
    /// is set: an unprefixed key in a shared store is the collision this exists
    /// to prevent, so the caller fails rather than sending one.
    pub data_cache_key_prefix: Option<String>,
    /// Additional project-relative global stylesheet files or directories.
    pub style_entries: Vec<PathBuf>,
    /// Precompile route modules and load their dependencies in dev workers.
    pub prebundle_dependencies: bool,
    /// JavaScript runtime used by every renderer and worker.
    pub runtime: JavaScriptRuntime,
    /// JSX transform runtime passed to every JavaScript renderer and worker.
    pub jsx_runtime: JsxRuntime,
    /// JavaScript language level `compiler.mjs` writes its modules down to.
    ///
    /// Carried here so a dev render and a built bundle apply the same
    /// `build.target`; the value is handed to the worker through
    /// `RUVYXA_ES_TARGET`, the way the JSX runtime already is.
    pub es_target: ruvyxa_bundler::EsTarget,
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
    /// Which environment the plugin host serves.
    ///
    /// Deliberately explicit rather than inferred from `watch`: a development
    /// server with watching disabled is still a development server, and a
    /// plugin that decided otherwise would withhold behaviour the developer
    /// asked for.
    pub plugin_environment: PluginEnvironment,
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
            discovery_dir: None,
            client_dir: root.join(".ruvyxa/client"),
            prerender_dir: root.join(".ruvyxa/prerender"),
            root,
            host: host.into(),
            port,
            watch: true,
            cache_route_manifest: true,
            cache_css: true,
            data_cache_max_entries: None,
            data_cache_max_bytes: None,
            data_cache_handler: None,
            data_cache_key_prefix: None,
            style_entries: Vec::new(),
            prebundle_dependencies: true,
            runtime: JavaScriptRuntime::detect(),
            jsx_runtime: JsxRuntime::Automatic,
            es_target: ruvyxa_bundler::EsTarget::EsNext,
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
            plugin_environment: PluginEnvironment::Development,
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
            discovery_dir: None,
            client_dir: root.join(".ruvyxa/client"),
            prerender_dir: root.join(".ruvyxa/prerender"),
            root,
            host: host.into(),
            port,
            watch: false,
            cache_route_manifest: true,
            cache_css: true,
            data_cache_max_entries: None,
            data_cache_max_bytes: None,
            data_cache_handler: None,
            data_cache_key_prefix: None,
            style_entries: Vec::new(),
            prebundle_dependencies: false,
            runtime: JavaScriptRuntime::detect(),
            jsx_runtime: JsxRuntime::Automatic,
            es_target: ruvyxa_bundler::EsTarget::EsNext,
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
            plugin_environment: PluginEnvironment::Production,
            plugin_head: Vec::new(),
            default_render_strategy: None,
            default_revalidate: None,
            i18n: None,
            dynamic_images: DynamicImageConfig::default(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppState {
    config: ServerConfig,
    reload_tx: broadcast::Sender<String>,
    runtime_cache: Arc<RuntimeCache>,
    action_limiter: Arc<Mutex<ActionRateLimiter>>,
    action_replays: Arc<Mutex<ActionReplayGuard>>,
    worker_pool: Arc<NodeWorkerPool>,
    render_cache: Arc<RenderCache>,
    isr_revalidating: render_pipeline::IsrRevalidationSet,
    /// One render per cache key, however many requests are asking for it.
    single_flight: Arc<render_pipeline::RenderSingleFlight>,
    hmr_tracker: Arc<HmrTracker>,
    plugin_runtime: Option<Arc<PluginHost>>,
    realtime: Option<RealtimeRuntime>,
    presence: Option<PresenceRuntime>,
    devtools: Arc<DevToolsMetrics>,
    dynamic_image_cache: Arc<DynamicImageCache>,
    edit_traces: Arc<trace::TraceStore>,
    /// Set once a termination signal has arrived and the drain has begun.
    ///
    /// Read by `/__ruvyxa/health` and nothing else. An orchestrator that is
    /// still routing to a process which has stopped accepting sends it work it
    /// can only refuse, so the readiness answer has to change before the socket
    /// does.
    draining: Arc<std::sync::atomic::AtomicBool>,
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
/// route. Registering a transport on one of these panics axum with
/// `Overlapping method route`, before the server can report anything.
///
/// This has to name every path [`build_app_router`] registers, and for a long
/// time it named eight of ten: `/__ruvyxa/hydration-loader.js` and
/// `/__ruvyxa/client/route-manifest.json` were registered and not listed, so a
/// plugin declaring a transport there passed `validate_socket_path` and killed
/// the server at startup instead of getting RUV1701. The comment claiming the
/// two stayed in sync was the only thing holding them together;
/// `every_registered_route_is_reserved` reads the route chain now.
const RESERVED_FRAMEWORK_ROUTES: [&str; 13] = [
    "/__ruvyxa/hmr",
    "/__ruvyxa/client",
    "/__ruvyxa/action",
    "/__ruvyxa/flight",
    "/__ruvyxa/rsc",
    "/__ruvyxa/trace",
    "/__ruvyxa/devtools",
    "/__ruvyxa/devtools/data",
    "/__ruvyxa/image",
    "/__ruvyxa/hydration-loader.js",
    "/__ruvyxa/client/route-manifest.json",
    "/__ruvyxa/client/vendor",
    "/__ruvyxa/health",
];

/// Process-wide startup hooks recognised by the JavaScript compiler/runtime.
/// Kept under a cross-language conformance test because changing one host alone
/// makes development and deployed instrumentation disagree.
pub(crate) const INSTRUMENTATION_FILES: [&str; 3] = [
    "instrumentation.ts",
    "instrumentation.js",
    "instrumentation.mjs",
];

#[derive(Default)]
pub(crate) struct RuntimeCache {
    routes: tokio::sync::RwLock<CacheSlot<RouteCacheEntry>>,
    styles: tokio::sync::RwLock<CacheSlot<StyleCacheEntry>>,
    /// The stylesheet URL a production build recorded, or `None` when there is
    /// none — `ruvyxa dev` never has one. Cached beside the links below for the
    /// same reason: it is a filesystem read whose answer changes only when the
    /// build writes a new manifest.
    style_asset: tokio::sync::RwLock<CacheSlot<Option<Arc<str>>>>,
    /// `<link>` tags derived from the public directory's contents.
    ///
    /// Resolved once and reused. `public_asset_links` stats the public directory
    /// to decide which tags to emit, and every page render called it — a
    /// blocking filesystem syscall on a Tokio worker thread, per request, for an
    /// answer that only changes when the watcher invalidates this cache.
    asset_links: tokio::sync::RwLock<CacheSlot<Arc<str>>>,
    /// The synthesized `route-manifest.json` body `ruvyxa dev` serves.
    ///
    /// Rebuilt only when something on disk changed. Building it asks the worker
    /// pool for every page route's browser bundle — the bundle's hash is the
    /// `artifactVersion` the client router compares against — and the browser
    /// fetches this table on every document load, so the answer was being
    /// recomputed across every route of the project for each one. On the demo
    /// that made a manifest request cost more than sixty milliseconds against a
    /// page's two tenths of one.
    client_routes: tokio::sync::RwLock<CacheSlot<Arc<str>>>,
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
            style_asset: tokio::sync::RwLock::new(CacheSlot::default()),
            asset_links: tokio::sync::RwLock::new(CacheSlot::default()),
            client_routes: tokio::sync::RwLock::new(CacheSlot::default()),
        }
    }

    /// The cached client route table, or the generation a rebuild must install
    /// against.
    ///
    /// Split in two rather than taking a builder closure because building the
    /// table needs the whole [`AppState`] this cache is a field of. The
    /// generation is what stops a build that started before a watcher event
    /// from installing its stale answer afterwards.
    pub(crate) async fn cached_client_routes(&self) -> std::result::Result<Arc<str>, u64> {
        let cached = self.client_routes.read().await;
        match cached.value.as_ref() {
            Some(body) => Ok(Arc::clone(body)),
            None => Err(cached.generation),
        }
    }

    /// Install a table built against `generation`.
    ///
    /// `None` means the generation moved while it was being built, so the table
    /// describes a tree that has already changed and the caller has to build
    /// again rather than serve it.
    pub(crate) async fn store_client_routes(
        &self,
        generation: u64,
        body: Arc<str>,
    ) -> Option<Arc<str>> {
        let mut cached = self.client_routes.write().await;
        if let Some(existing) = cached.value.as_ref() {
            return Some(Arc::clone(existing));
        }
        cached
            .insert_if_current(generation, Arc::clone(&body))
            .then_some(body)
    }

    /// Drop the client route table alone.
    ///
    /// Called for *every* watcher event, including the selective ones that keep
    /// the route manifest and the collected CSS. Those are selective because a
    /// component edit changes neither — but it does change the bundle that
    /// component is in, and the table advertises that bundle's hash. Keeping it
    /// across such an edit tells the router the bundle it already has is
    /// current, and the soft navigation renders the code from before the save.
    pub(crate) fn invalidate_client_routes(&self) {
        self.client_routes.blocking_write().invalidate();
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

    /// The head fragment that gives a document its project stylesheet.
    ///
    /// A production build writes the compiled CSS as a client asset, and every
    /// host links it: the browser caches one file instead of receiving the
    /// whole stylesheet inside each document, and a deployed function — which
    /// has no `app/` to compile from — can serve a request-time render with the
    /// same stylesheet a pre-rendered page got. `ruvyxa dev` has no such asset
    /// and inlines the collection, because HMR replaces the rule text in place.
    async fn style_tag(&self, config: &ServerConfig) -> Result<String> {
        if !config.watch
            && let Some(url) = self.built_style_asset(config).await
        {
            return Ok(style_head_tag(Some(&url), ""));
        }
        let css = self.styles(config).await?;
        Ok(style_head_tag(None, &css))
    }

    /// The stylesheet URL `ruvyxa build` recorded, read once per cache
    /// generation rather than per request.
    ///
    /// The generation is captured in the read-lock block, before the manifest
    /// is read, exactly as `asset_links` does. It used to be read from inside
    /// the write lock it was about to write through, which made
    /// `insert_if_current` a tautology — a guard that cannot refuse anything.
    async fn built_style_asset(&self, config: &ServerConfig) -> Option<Arc<str>> {
        loop {
            let generation = {
                let cached = self.style_asset.read().await;
                if let Some(value) = cached.value.as_ref() {
                    return value.clone();
                }
                cached.generation
            };

            let manifest_path = config.client_dir.join("route-manifest.json");
            let discovered = tokio::task::spawn_blocking(move || {
                let source = std::fs::read(&manifest_path).ok()?;
                let manifest: serde_json::Value = serde_json::from_slice(&source).ok()?;
                let url = manifest.get("styles")?.as_array()?.first()?.as_str()?;
                Some(Arc::<str>::from(url))
            })
            .await
            .ok()
            .flatten();

            let mut cached = self.style_asset.write().await;
            if let Some(value) = cached.value.as_ref() {
                return value.clone();
            }
            if cached.insert_if_current(generation, discovered.clone()) {
                return discovered;
            }
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
    ///
    /// An empty slot is not the same question. The file set is only known once
    /// a collection has finished, so before that this cannot decide whether a
    /// change matters — and answering "it does not" is what lets a collection
    /// already in flight install CSS it read before the save. The generation is
    /// exactly the mechanism for revoking that right, so an empty slot is
    /// bumped rather than left alone.
    fn invalidate_styles_for_paths(&self, paths: &[PathBuf]) -> bool {
        let changed = paths
            .iter()
            .map(|path| normalize_cache_path(path))
            .collect::<BTreeSet<_>>();
        let mut styles = self.styles.blocking_write();
        let Some(cached) = styles.value.as_ref() else {
            styles.invalidate();
            return true;
        };
        let intersects = !cached.files.is_disjoint(&changed);
        if intersects {
            styles.invalidate();
        }
        intersects
    }

    /// Every slot, including `style_asset`.
    ///
    /// `style_asset` was left out, so a `ruvyxa start` that was redeployed in
    /// place went on linking the stylesheet URL from the manifest the previous
    /// build wrote — and its generation, never moving off zero, hid that
    /// `built_style_asset`'s staleness guard could not refuse anything either.
    fn invalidate(&self) {
        // Use blocking_write for sync context (file watcher callback)
        self.routes.blocking_write().invalidate();
        self.styles.blocking_write().invalidate();
        self.style_asset.blocking_write().invalidate();
        self.asset_links.blocking_write().invalidate();
        self.client_routes.blocking_write().invalidate();
    }

    #[cfg(test)]
    async fn invalidate_async(&self) {
        self.routes.write().await.invalidate();
        self.styles.write().await.invalidate();
        self.style_asset.write().await.invalidate();
        self.asset_links.write().await.invalidate();
        self.client_routes.write().await.invalidate();
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
    let runtime = config.runtime;
    tokio::task::spawn_blocking(move || collect_styles(&root, &app_dir, &entries, runtime))
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

pub(crate) fn discover_options(config: &ServerConfig) -> DiscoverOptions {
    DiscoverOptions::new(&config.app_dir)
        .with_rendering_defaults(config.default_render_strategy, config.default_revalidate)
        .with_i18n(config.i18n.clone())
}

/// Pre-bundle the dependencies the first requests will need, in the background.
///
/// Fire-and-forget on purpose: warming is an optimization, and blocking startup
/// on it would trade a slow first request for a slow `ruvyxa dev`.
fn spawn_dependency_warmup(
    config: &ServerConfig,
    manifest: &RouteManifest,
    worker_pool: &Arc<NodeWorkerPool>,
) {
    let warmup_routes = dependency_warmup_routes(config, manifest);
    if warmup_routes.is_empty() {
        return;
    }
    let warmup_pool = worker_pool.clone();
    let warmup_root = config.root.display().to_string();
    tokio::spawn(async move {
        let warmed = warmup_pool.warmup(&warmup_root, warmup_routes).await;
        info!(warmed, "dependency pre-bundling complete");
    });
}

/// Start the TypeScript plugin host pool, unless plugins are disabled.
async fn start_plugin_runtime(config: &ServerConfig) -> Result<Option<Arc<PluginHost>>> {
    if !config.plugins_enabled {
        return Ok(None);
    }
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
        config.plugin_environment,
    )
    .await?;
    if host.pool_size() > 1 {
        info!(
            workers = host.pool_size(),
            "TypeScript plugin middleware pool ready"
        );
    }
    Ok(Some(Arc::new(host)))
}

/// Reject a websocket path the router cannot serve.
///
/// Registering over a reserved framework route would panic inside axum's
/// router, so a bad descriptor has to become a diagnostic before the route
/// table is built.
///
/// The alphabet is an allowlist, not a denylist, because a denylist is only
/// correct while it tracks the router's own syntax and nothing makes it. This
/// rejected `?`, `#`, and `*` — the axum 0.7 wildcard set — long after the
/// workspace moved to axum 0.8, where a capture is `{name}` and a catch-all
/// `{*rest}`: `/{room}` passed and registered a single-segment wildcard that
/// shadowed every one-segment project page, and `/{` passed and panicked
/// `matchit` inside `Router::route`, which is precisely the outcome this
/// function exists to prevent. One or more `/`-prefixed segments of RFC 3986
/// unreserved characters is a literal path in every router version and can
/// never acquire a meaning.
///
/// `packages/ruvyxa/runtime/plugin-http.mjs` decides the same question first,
/// inside the plugin host, and the two are held level by `transportPaths` in
/// `tests/fixtures/framework-endpoint-conformance.json`.
fn validate_socket_path(path: &str, kind: &str) -> Result<()> {
    if !is_literal_transport_path(path) {
        return Err(RuvyxaError::Message(format!(
            "RUV1701 TypeScript plugin host returned invalid {kind} configuration: \
             path {path:?} must be one or more `/`-prefixed segments of letters, \
             digits, `-`, `.`, `_`, or `~`"
        )));
    }
    if RESERVED_FRAMEWORK_ROUTES.contains(&path) {
        return Err(RuvyxaError::Message(format!(
            "RUV1701 {kind} path {path} collides with a reserved framework route"
        )));
    }
    Ok(())
}

/// One or more `/`-prefixed segments of RFC 3986 unreserved characters.
///
/// The twin of `isLiteralTransportPath` in
/// `packages/ruvyxa/runtime/plugin-http.mjs`.
fn is_literal_transport_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    rest.split('/').all(|segment| {
        !segment.is_empty()
            && segment.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
            })
    })
}

/// Build the realtime transport a plugin declared, if any.
fn realtime_runtime(plugin_runtime: Option<&Arc<PluginHost>>) -> Result<Option<RealtimeRuntime>> {
    let Some(descriptor) = plugin_runtime.and_then(|runtime| runtime.descriptor().realtime())
    else {
        return Ok(None);
    };
    if !(5_000..=120_000).contains(&descriptor.heartbeat_ms)
        || !(16..=4_096).contains(&descriptor.capacity)
    {
        return Err(RuvyxaError::Message(
            "RUV1701 TypeScript plugin host returned invalid realtime configuration".into(),
        ));
    }
    validate_socket_path(&descriptor.path, "realtime")?;
    let (tx, _) = broadcast::channel(descriptor.capacity);
    Ok(Some(RealtimeRuntime {
        path: descriptor.path.clone(),
        heartbeat: Duration::from_millis(descriptor.heartbeat_ms),
        tx,
    }))
}

/// Build the presence transport a plugin declared, if any.
fn presence_runtime(plugin_runtime: Option<&Arc<PluginHost>>) -> Result<Option<PresenceRuntime>> {
    let Some(descriptor) = plugin_runtime.and_then(|runtime| runtime.descriptor().presence())
    else {
        return Ok(None);
    };
    if !(5_000..=120_000).contains(&descriptor.heartbeat_ms) {
        return Err(RuvyxaError::Message(
            "RUV1701 TypeScript plugin host returned invalid presence configuration".into(),
        ));
    }
    validate_socket_path(&descriptor.path, "presence")?;
    Ok(Some(PresenceRuntime {
        path: descriptor.path.clone(),
        heartbeat: Duration::from_millis(descriptor.heartbeat_ms),
        registry: CollabRegistry::new(),
    }))
}

/// Capabilities this host serves that no deployed build can.
///
/// `ruvyxa build` reports them with `RUV2205` and `ruvyxa test:parity` reports
/// them, but both arrive after the application has been written around the
/// transport — and replacing a transport is not a small change. Only `dev`
/// says it: `ruvyxa start` is the long-lived host that *does* serve these, so
/// the same line there would be false.
fn native_only_capability_notes(
    config: &ServerConfig,
    realtime: Option<&RealtimeRuntime>,
    presence: Option<&PresenceRuntime>,
) -> Vec<String> {
    if !config.watch {
        return Vec::new();
    }
    let mut notes = Vec::new();
    if let Some(realtime) = realtime {
        notes.push(format!("realtime@1 {}", realtime.path));
    }
    if let Some(presence) = presence {
        notes.push(format!("presence@1 {}", presence.path));
    }
    notes
}

fn print_native_only_capabilities(notes: &[String]) {
    if notes.is_empty() {
        return;
    }
    for note in notes {
        println!("{} {note} is served by this process only.", warn_text("!"));
        println!(
            "  no build artifact serves it; `ruvyxa build` reports RUV2205 and deployments need `ruvyxa start`"
        );
    }
    println!();
}

/// Reject a project that points both websocket transports at one path.
///
/// Registering the same path twice panics inside axum's router, so this has to
/// fail before the route table is built.
fn assert_transport_paths_distinct(
    realtime: Option<&RealtimeRuntime>,
    presence: Option<&PresenceRuntime>,
) -> Result<()> {
    if let (Some(realtime), Some(presence)) = (realtime, presence)
        && realtime.path == presence.path
    {
        return Err(RuvyxaError::Message(format!(
            "RUV1701 presence path {} collides with the realtime transport",
            presence.path
        )));
    }
    Ok(())
}

/// How many page requests may run at once, and how many may wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AdmissionLimits {
    concurrency: usize,
    queue: usize,
}

/// The semaphore and waiter count one host shares across every connection.
///
/// Nothing bounded this. Every request that arrived got a render started for
/// it, so a burst larger than the machine turned into unbounded queueing:
/// latency grew without limit, nothing was refused, and `/__ruvyxa/health` kept
/// answering `200` because it does not read queue depth. The same application
/// deployed through a self-hosted adapter sheds load correctly — this is that
/// controller, on the host that was missing it.
#[derive(Clone)]
struct AdmissionControl {
    permits: Arc<tokio::sync::Semaphore>,
    waiting: Arc<std::sync::atomic::AtomicUsize>,
    limits: AdmissionLimits,
}

impl AdmissionControl {
    fn new(limits: AdmissionLimits) -> Self {
        Self {
            permits: Arc::new(tokio::sync::Semaphore::new(limits.concurrency)),
            waiting: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            limits,
        }
    }

    /// Take a slot, or refuse. `None` means the queue is full.
    ///
    /// The waiter count is raised before the wait and lowered after it, so the
    /// queue bounds requests parked on memory this process would have to keep
    /// rather than requests it is actually working on.
    async fn acquire(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        use std::sync::atomic::Ordering;

        if let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() {
            return Some(permit);
        }
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= self.limits.queue {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        let permit = Arc::clone(&self.permits).acquire_owned().await.ok();
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        permit
    }
}

/// The answer to a request this host has decided not to start.
///
/// `503` and not `500`: nothing failed, the server declined to begin, and a
/// caller that retries may well be served. `Retry-After` says so in the one
/// place a proxy reads. The same body and headers the standalone host sends,
/// so a client sees one answer whichever host is in front of it.
fn admission_refused() -> Response {
    use axum::http::{HeaderValue, header};

    let mut response = (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Take a slot, run the request, and give the slot back.
///
/// The slot is released when the **response** exists, not when its body has
/// finished — the same boundary the standalone host uses, and for the same
/// reason: a server-sent-event stream holds its body open for hours, and a slot
/// held that long would take the pool down to nothing after a handful of
/// subscribers. What is being bounded is the render, which is the part that
/// competes for the CPU.
async fn admit(
    control: AdmissionControl,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(_permit) = control.acquire().await else {
        warn!(
            concurrency = control.limits.concurrency,
            queue = control.limits.queue,
            path = %request.uri().path(),
            "refused: admission queue is full"
        );
        return admission_refused();
    };
    next.run(request).await
}

/// Read a positive count from an environment value, mirroring the standalone
/// host's `positiveNumber`: anything absent, unparseable, or non-positive falls
/// back rather than turning a typo into a limit of zero.
fn positive_count(raw: Option<&str>) -> Option<usize> {
    let value = raw?.trim().parse::<f64>().ok()?;
    (value.is_finite() && value > 0.0).then(|| value.trunc() as usize)
}

/// The machine's share of cores, bounded, as the default admission width.
fn default_admission_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(*ADMISSION_DEFAULT_BOUNDS.start())
        .clamp(
            *ADMISSION_DEFAULT_BOUNDS.start(),
            *ADMISSION_DEFAULT_BOUNDS.end(),
        )
}

/// Resolve `RUVYXA_MAX_CONCURRENCY` / `RUVYXA_MAX_QUEUE`, or `None` for "off".
///
/// `RUVYXA_MAX_CONCURRENCY=0` turns admission off for a deployment that has
/// something else in front of it doing this, and `ruvyxa dev` defaults to off:
/// one developer with a browser is not a load event, and a refused request in
/// the middle of an edit loop looks like a broken framework.
fn resolve_admission_limits(
    concurrency: Option<&str>,
    queue: Option<&str>,
    watch: bool,
) -> Option<AdmissionLimits> {
    let default_concurrency = if watch {
        0
    } else {
        default_admission_concurrency()
    };
    let concurrency = match concurrency.map(str::trim) {
        Some("0") => 0,
        other => positive_count(other).unwrap_or(default_concurrency),
    };
    if concurrency == 0 {
        return None;
    }
    let queue = positive_count(queue).unwrap_or(concurrency * ADMISSION_QUEUE_PER_SLOT);
    Some(AdmissionLimits { concurrency, queue })
}

fn admission_control(config: &ServerConfig) -> Option<AdmissionControl> {
    resolve_admission_limits(
        std::env::var("RUVYXA_MAX_CONCURRENCY").ok().as_deref(),
        std::env::var("RUVYXA_MAX_QUEUE").ok().as_deref(),
        config.watch,
    )
    .map(AdmissionControl::new)
}

/// Assemble the route table: framework endpoints, the optional transports, then
/// the page fallback.
///
/// The fallback is registered last on purpose. Everything under `/__ruvyxa/` is
/// framework surface, and a project route must never shadow it.
fn build_app_router(config: &ServerConfig, state: Arc<AppState>) -> Router {
    let realtime_path = state.realtime.as_ref().map(|runtime| runtime.path.clone());
    let presence_path = state.presence.as_ref().map(|runtime| runtime.path.clone());
    let mut app = Router::new()
        .route("/__ruvyxa/client", get(client_bundle))
        .route("/__ruvyxa/hydration-loader.js", get(hydration_loader))
        .route("/__ruvyxa/client/route-manifest.json", get(client_manifest))
        .route("/__ruvyxa/client/vendor", get(client_vendor))
        .route("/__ruvyxa/flight", get(flight_endpoint))
        .route(
            "/__ruvyxa/rsc",
            get(rsc_payload_endpoint).post(rsc_action_endpoint),
        )
        .route("/__ruvyxa/image", get(dynamic_image_endpoint))
        .route(
            "/__ruvyxa/health",
            get(health_endpoint).head(health_endpoint),
        )
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
        // Dev-only surface, gated at registration. `/__ruvyxa/hmr` was
        // registered here unconditionally while the endpoint contract recorded
        // it `"native": "dev"`, so a production `ruvyxa start` let any
        // unauthenticated client open unbounded WebSockets that carry nothing,
        // are never heartbeated, and are never timed out. It stays in
        // `RESERVED_FRAMEWORK_ROUTES`, so `validate_socket_path` keeps refusing
        // a plugin transport on the path in both modes.
        app = app
            .route("/__ruvyxa/hmr", get(hmr_ws))
            .route("/__ruvyxa/devtools", get(devtools_dashboard))
            .route("/__ruvyxa/devtools/data", get(devtools_data));
    }
    if let Some(path) = realtime_path {
        app = app.route(&path, get(realtime_ws));
    }
    if let Some(path) = presence_path {
        app = app.route(&path, get(presence_ws));
    }

    let page = Router::new()
        .fallback(handle_request)
        .with_state(Arc::clone(&state));
    compose_app_router(app, page, admission_control(config)).with_state(state)
}

/// Put the page fallback behind admission and the framework routes in front.
///
/// Admission stands in front of the page fallback and nothing else, so
/// `/__ruvyxa/health` is answered before it. A probe that queues behind the
/// renders it exists to report on says "unhealthy" exactly when the process is
/// merely busy, and the orchestrator restarts something that was working.
///
/// This is a function rather than four lines inside [`build_app_router`] so the
/// saturation test can drive the composition this host actually runs. A test
/// that assembles its own router asserts a shape nothing holds the real one to,
/// and the one line that matters here is which of the two routers the layer
/// lands on.
fn compose_app_router<S>(
    framework: Router<S>,
    page: Router,
    admission: Option<AdmissionControl>,
) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let page = match admission {
        Some(control) => page.layer(axum::middleware::from_fn(move |request, next| {
            admit(control.clone(), request, next)
        })),
        None => page,
    };
    framework.fallback_service(page)
}

/// Resolve the configured host and port into one socket address.
fn resolve_bind_address(config: &ServerConfig) -> Result<SocketAddr> {
    format!("{}:{}", config.host, config.port)
        .to_socket_addrs()
        .map_err(|error| RuvyxaError::Message(format!("Invalid server address: {error}")))?
        .next()
        .ok_or_else(|| RuvyxaError::Message("Server address did not resolve".to_string()))
}

pub async fn serve(config: ServerConfig) -> Result<()> {
    config.validate_limits()?;
    let startup_started = Instant::now();
    let manifest = discover_routes(discover_options(&config))?;
    observe_manifest(&config, &manifest);
    info!(routes = manifest.routes.len(), "discovered routes");

    let (reload_tx, _) = broadcast::channel(64);
    let runtime_cache = Arc::new(RuntimeCache::with_manifest(manifest.clone()));

    // Validated before anything is spawned: a rejected middleware stack is a
    // configuration error, and paying for two JavaScript runtimes to start
    // before reporting it only makes the report slower.
    //
    // The built-in rate limiter reads `security.trustedProxyIps` for the same
    // reason the action limiter does: behind a reverse proxy the transport peer
    // is the proxy, so keying on it alone gives every caller one shared bucket.
    let middleware_stack = MiddlewareStack::new(config.middleware.clone())
        .with_trusted_proxies(config.trusted_proxies.clone());
    middleware_stack.validate().map_err(RuvyxaError::Message)?;

    // Both start a JavaScript runtime and neither needs anything from the
    // other, so they come up together. In sequence they were the whole of a dev
    // server's startup — the render workers and then the plugin host, each
    // waiting for a process that had nothing to say to it.
    let env = runtime_env(&config)?;
    let (worker_pool, plugin_runtime) = tokio::try_join!(
        NodeWorkerPool::start_with_runtime(&config.root, env, config.runtime),
        start_plugin_runtime(&config),
    )?;
    let worker_pool = Arc::new(worker_pool);
    info!(
        runtime = config.runtime.command(),
        "JavaScript worker pool ready"
    );

    spawn_dependency_warmup(&config, &manifest, &worker_pool);

    let render_cache = Arc::new(if config.watch {
        RenderCache::default_dev()
    } else {
        RenderCache::default_production()
    });

    let watcher_pool = worker_pool.clone();
    let watcher_render_cache = render_cache.clone();
    let hmr_tracker = Arc::new(HmrTracker::new());
    hmr_tracker.populate_from_manifest(&manifest.routes);
    let realtime = realtime_runtime(plugin_runtime.as_ref())?;
    let presence = presence_runtime(plugin_runtime.as_ref())?;
    assert_transport_paths_distinct(realtime.as_ref(), presence.as_ref())?;
    let native_only = native_only_capability_notes(&config, realtime.as_ref(), presence.as_ref());
    let draining = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let state = AppState {
        config: config.clone(),
        draining: Arc::clone(&draining),
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
        single_flight: Arc::default(),
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

    let app = build_app_router(&config, Arc::new(state));

    // Apply middleware stack from config (compression, CORS, timing, logging, custom headers)
    let app = middleware_stack.apply(app).map_err(RuvyxaError::Message)?;
    let security_headers = config.security_headers;
    let app =
        app.layer(axum::middleware::map_response(
            move |response: Response| async move {
                finalize_security_headers(response, security_headers)
            },
        ));

    let address = resolve_bind_address(&config)?;
    let (listeners, bound_address) = bind_listeners(&config, address).await?;

    for listener in &listeners {
        if let Ok(bound) = listener.local_addr() {
            info!("Ruvyxa server listening on http://{bound}");
        }
    }
    print_server_ready(&config, &manifest, bound_address, startup_started.elapsed());
    print_native_only_capabilities(&native_only);
    let server_result = serve_until_shutdown(
        listeners,
        app,
        draining,
        ShutdownTiming::from_env(config.watch),
    )
    .await;

    worker_pool.shutdown().await;
    server_result?;
    Ok(())
}

/// The two shutdown windows this host observes, resolved once at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShutdownTiming {
    /// How long in-flight work may finish once the listeners stop accepting.
    grace: Duration,
    /// How long the listeners keep accepting after the drain flag is raised.
    drain_delay: Duration,
}

impl ShutdownTiming {
    fn from_env(watch: bool) -> Self {
        Self::resolve(
            std::env::var("RUVYXA_SHUTDOWN_GRACE").ok().as_deref(),
            std::env::var("RUVYXA_DRAIN_DELAY").ok().as_deref(),
            watch,
        )
    }

    /// Both windows are milliseconds, spelled the way `standalone-server.ts`
    /// spells them, because they are the same two variables on the same
    /// deployment. The delay is capped at half the grace so in-flight work
    /// keeps a budget of its own however the two are configured, and
    /// `RUVYXA_DRAIN_DELAY=0` closes straight away — which is right where
    /// nothing is load-balancing this process, and is the default under
    /// `ruvyxa dev`, where a five-second wait on Ctrl-C reads as a hang.
    fn resolve(grace: Option<&str>, drain: Option<&str>, watch: bool) -> Self {
        let grace = positive_millis(grace).unwrap_or(SERVER_SHUTDOWN_GRACE);
        let default_delay = if watch {
            Duration::ZERO
        } else {
            SERVER_DRAIN_DELAY
        };
        let drain_delay = match drain.map(str::trim) {
            Some("0") => Duration::ZERO,
            other => positive_millis(other).unwrap_or(default_delay),
        };
        Self {
            grace,
            drain_delay: drain_delay.min(grace / 2),
        }
    }
}

/// Read a positive millisecond duration from an environment value.
fn positive_millis(raw: Option<&str>) -> Option<Duration> {
    let value = raw?.trim().parse::<f64>().ok()?;
    (value.is_finite() && value > 0.0).then(|| Duration::from_secs_f64(value / 1_000.0))
}

/// Accept connections until a termination signal arrives, then drain.
///
/// The grace window is bounded: a client holding a streaming response open
/// would otherwise keep `ruvyxa dev` alive indefinitely after Ctrl-C, so the
/// remaining connections are dropped once it expires.
async fn serve_until_shutdown(
    listeners: Vec<tokio::net::TcpListener>,
    app: Router,
    draining: Arc<std::sync::atomic::AtomicBool>,
    timing: ShutdownTiming,
) -> std::io::Result<()> {
    serve_with_signals(listeners, app, draining, timing, shutdown_signal).await
}

/// The body of [`serve_until_shutdown`], with the signal source as a parameter.
///
/// `signal` is called once per signal awaited: the first ends the serving
/// phase, and a second — an operator pressing Ctrl-C twice, or a platform
/// escalating — means now, and must not be held for a window that exists for a
/// load balancer. A test can hand this an ordinary channel; nothing else about
/// the sequence changes.
async fn serve_with_signals<Signal, Wait>(
    listeners: Vec<tokio::net::TcpListener>,
    app: Router,
    draining: Arc<std::sync::atomic::AtomicBool>,
    timing: ShutdownTiming,
    mut signal: Signal,
) -> std::io::Result<()>
where
    Signal: FnMut() -> Wait,
    Wait: std::future::Future<Output = &'static str>,
{
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    // One server per address the host answers to, over one router. They share
    // the shutdown channel, so a signal drains all of them together rather than
    // leaving the loopback family nobody signalled still accepting.
    let mut servers = tokio::task::JoinSet::new();
    for listener in listeners {
        let mut shutdown_rx = shutdown_tx.subscribe();
        let service = server_make_service(app.clone());
        servers.spawn(async move {
            axum::serve(listener, service)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await
        });
    }
    let servers = drain_servers(servers);
    tokio::pin!(servers);

    let first = tokio::select! {
        result = &mut servers => return result,
        signal = signal() => signal,
    };
    info!(
        signal = first,
        drain_delay_ms = timing.drain_delay.as_millis() as u64,
        "draining Ruvyxa server connections"
    );
    // Before the listeners stop accepting, so a probe on a connection that is
    // already open learns this too rather than only the ones that fail to
    // connect afterwards.
    draining.store(true, std::sync::atomic::Ordering::Relaxed);

    let forced = await_drain_window(timing.drain_delay, &mut signal).await;
    let _ = shutdown_tx.send(true);
    if forced {
        return Ok(());
    }
    tokio::select! {
        result = tokio::time::timeout(timing.grace, &mut servers) => match result {
            Ok(result) => result,
            Err(_) => {
                warn!("server shutdown timed out; closing remaining connections");
                Ok(())
            }
        },
        second = signal() => {
            warn!(signal = second, "shutdown forced; closing remaining connections");
            Ok(())
        }
    }
}

/// Keep serving for the drain window. `true` means a second signal cut it short.
///
/// The host is still listening, and still answering, for this whole window: the
/// readiness probe opens a fresh connection, reads the `503`, and stops routing
/// here before the socket goes away. Raising the drain flag and closing the
/// listener on the same tick made that answer unreachable — the probe got
/// `ECONNREFUSED`, and everything the orchestrator was still sending failed in a
/// browser instead of being retried against another instance.
async fn await_drain_window<Signal, Wait>(delay: Duration, signal: &mut Signal) -> bool
where
    Signal: FnMut() -> Wait,
    Wait: std::future::Future<Output = &'static str>,
{
    if delay.is_zero() {
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        second = signal() => {
            warn!(signal = second, "shutdown forced; ending the drain window");
            true
        }
    }
}

/// Wait for every listener's server to finish, reporting the first failure.
///
/// Dropping this future aborts whatever is still running, which is what the
/// grace timeout above relies on to close remaining connections.
async fn drain_servers(
    mut servers: tokio::task::JoinSet<std::io::Result<()>>,
) -> std::io::Result<()> {
    let mut first_failure = None;
    while let Some(joined) = servers.join_next().await {
        let failure = match joined {
            Ok(Ok(())) => continue,
            Ok(Err(error)) => error,
            Err(error) if error.is_panic() => {
                std::io::Error::other(format!("server task panicked: {error}"))
            }
            // Cancelled: this future was dropped, so there is nothing to report.
            Err(_) => continue,
        };
        first_failure.get_or_insert(failure);
    }
    match first_failure {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
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
) -> Vec<worker_protocol::WarmupRoute> {
    if !config.watch || !config.prebundle_dependencies {
        return Vec::new();
    }

    manifest
        .routes
        .iter()
        .filter(|route| route.kind == RouteKind::Page)
        .map(|route| worker_protocol::WarmupRoute {
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

    // Drawn by `ruvyxa_tui::banner`, exactly as every CLI command draws it.
    // The dev server used to print its own copy of these four lines, which is
    // why it was for a long time the one surface with no command badge — and
    // why a change to the header shape had to be made twice to be visible
    // everywhere. The badge resolves from the title's first word, so `Dev
    // Server` and `Dev` are the same badge.
    print_header(if config.watch { "Dev Server" } else { "Server" });
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

/// The answer to a request whose body exceeded `security.apiLimit`.
///
/// `Connection: close` is the load-bearing part. The limit exists so the rest of
/// the upload is *not* read, which means it is still in flight on this socket
/// when the answer goes out — and a client that reuses the connection has those
/// bytes read as the beginning of its next request. hyper closes the connection
/// for us because the body was never drained, so without the header the client
/// is told it may reuse a socket that is already gone, and a later, unrelated
/// request dies with a connection reset naming nothing to do with the upload.
/// RFC 9112 §9.6 is explicit: a response without the `close` option is one the
/// client may reuse.
///
/// The standalone server the adapters emit answers the same way for the same
/// reason — see the 413 branch in
/// `packages/@ruvyxa/core/src/standalone-server.ts`. Draining megabytes to keep
/// the connection warm is the cost the limit exists to avoid, on both hosts.
fn request_body_too_large(error: impl std::fmt::Display) -> Response {
    let mut response = (
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("Request body exceeded the API body limit or could not be read: {error}"),
    )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::CONNECTION,
        axum::http::HeaderValue::from_static("close"),
    );
    response
}

/// The request target the plugin stage is handed.
///
/// A plugin hook is scoped by path, and the JavaScript registry answers "does
/// this hook apply?" against whatever path string arrives here. The router
/// answers the same question against the canonical segment form, so handing
/// over the raw request line gave the two stages different answers: `//api/x`
/// routed to `/api/x` and read as out of scope for `['/api/*']`, which is the
/// default scope of `originGuard()`. A cross-site form POST to that address
/// reached the handler with the session cookie and no guard ran.
///
/// The query string is carried over untouched. It is not part of the scoping
/// decision, but this target becomes the request target for the rest of the
/// pipeline once a plugin has run, so dropping it would drop every query
/// parameter for any project with request middleware.
///
/// Held to `tests/fixtures/plugin-path-scope-conformance.json` together with
/// the deployed host, which makes the same decision inside
/// `packages/ruvyxa/runtime/plugin-http.mjs`.
fn plugin_request_target(request_path: &str, request_target: &str) -> String {
    match request_target.split_once('?') {
        Some((_, query)) => format!("{request_path}?{query}"),
        None => request_path.to_string(),
    }
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
                return with_security_headers(request_body_too_large(error));
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
            path: plugin_request_target(&request_path, &request_target),
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
                        plain_error_page(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
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
            path: plugin_request_target(&request_path, &request_target),
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
mod tests {
    use super::*;
    use axum::http::HeaderName;
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
            // `none` is the one value that says the Axum host deliberately does
            // not serve this path. Any other string, including a misspelling of
            // that one, still has to be registered — the skip is explicit so a
            // typo fails closed rather than quietly exempting an endpoint.
            match endpoint["native"].as_str() {
                None | Some("none") => continue,
                Some(_) => {}
            }
            assert!(
                source.contains(&format!(".route(\"{path}\""))
                    || source.contains(&format!("\"{path}\",")),
                "{path} is in the endpoint contract but is not registered by serve()"
            );
        }
    }

    /// Every path `build_app_router` registers is reserved against plugins.
    ///
    /// The two existing checks read the contract outwards: contract to
    /// `RESERVED_FRAMEWORK_ROUTES`, and contract to the route chain. Neither
    /// read the chain inwards, which is the direction `validate_socket_path`
    /// depends on — a route registered and never listed is one a plugin may
    /// take, and axum's answer to that is `Overlapping method route`, a panic
    /// during startup rather than the RUV1701 the guard exists to produce.
    /// `/__ruvyxa/hydration-loader.js` and
    /// `/__ruvyxa/client/route-manifest.json` were both in that state.
    ///
    /// Read from the source for the same reason
    /// `every_contract_endpoint_is_registered_on_the_native_router` does: axum
    /// cannot enumerate its own paths, and a `ServerConfig` needs a project on
    /// disk. The route chain is a literal list, so reading it is exact.
    #[test]
    fn every_registered_route_is_reserved() {
        let source = include_str!("lib.rs");
        let (_, body) = source
            .split_once("fn build_app_router(")
            .expect("build_app_router must exist");
        let body = body
            .split_once("\nfn ")
            .map_or(body, |(function, _)| function);

        let mut registered = Vec::new();
        for fragment in body.split(".route(").skip(1) {
            // Both spellings the chain uses: a literal, and `&path` for the
            // plugin transports, which are the paths being guarded rather than
            // guarding paths.
            let Some(rest) = fragment.trim_start().strip_prefix('"') else {
                continue;
            };
            let path = rest
                .split_once('"')
                .expect("a route literal must be closed")
                .0;
            registered.push(path.to_string());
        }

        assert!(
            registered.len() >= RESERVED_FRAMEWORK_ROUTES.len(),
            "only {} routes were read out of the chain; the parse missed some",
            registered.len()
        );
        for path in &registered {
            assert!(
                RESERVED_FRAMEWORK_ROUTES.contains(&path.as_str()),
                "{path} is registered on the router but is not in \
                 RESERVED_FRAMEWORK_ROUTES, so a plugin transport may claim it \
                 and panic axum at startup; add it there and to \
                 tests/fixtures/framework-endpoint-conformance.json"
            );
        }
        for reserved in RESERVED_FRAMEWORK_ROUTES {
            assert!(
                registered.iter().any(|path| path == reserved),
                "{reserved} is reserved against plugins but nothing registers it"
            );
        }
    }

    /// Both hosts answer `transportPaths` the same way.
    ///
    /// The JavaScript half is replayed by
    /// `tests/packages/ruvyxa/framework-endpoints.test.mjs`. This half matters
    /// more, because a path the plugin host let through does not produce a
    /// diagnostic here: it panics `matchit` inside `Router::route`, before the
    /// server can report anything.
    #[test]
    fn a_transport_path_is_accepted_or_refused_as_the_contract_says() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/framework-endpoint-conformance.json"
        ))
        .expect("the framework endpoint contract must be valid JSON");

        let cases = contract["transportPaths"]
            .as_array()
            .expect("transportPaths must be an array");
        assert!(
            !cases.is_empty(),
            "the transport path table must not be empty"
        );

        for case in cases {
            let path = case["path"].as_str().expect("every case must have a path");
            let valid = case["valid"]
                .as_bool()
                .expect("every case must state whether it is valid");
            let why = case["why"].as_str().unwrap_or("");

            for kind in ["realtime", "presence"] {
                let accepted = validate_socket_path(path, kind).is_ok();
                assert_eq!(
                    accepted, valid,
                    "validate_socket_path({path:?}, {kind:?}) answered {accepted}, \
                     the contract says {valid}: {why}"
                );
            }
        }
    }

    /// The crate sources this test family reads, keyed by top-level function.
    ///
    /// Read from the source for the same reason
    /// `every_contract_endpoint_is_registered_on_the_native_router` does: axum
    /// cannot enumerate its own paths, and a `ServerConfig` needs a project on
    /// disk. Every item here is at column zero in rustfmt's output, so a
    /// signature line starts with no indentation and the body ends at the
    /// first line that is exactly `}`.
    fn crate_function_bodies() -> std::collections::HashMap<String, String> {
        const SOURCES: [&str; 4] = [
            include_str!("lib.rs"),
            include_str!("framework_endpoints.rs"),
            include_str!("realtime_endpoints.rs"),
            include_str!("action_security.rs"),
        ];

        let mut bodies = std::collections::HashMap::new();
        for source in SOURCES {
            let mut current: Option<(String, String)> = None;
            for line in source.lines() {
                if let Some((_, body)) = current.as_mut() {
                    body.push_str(line);
                    body.push('\n');
                    if line == "}" {
                        let (name, body) = current.take().expect("a function was being read");
                        bodies.insert(name, body);
                    }
                    continue;
                }
                if line.starts_with(char::is_whitespace) {
                    continue;
                }
                let Some((_, rest)) = line.split_once("fn ") else {
                    continue;
                };
                let name = rest
                    .split(['(', '<'])
                    .next()
                    .expect("split always yields one element");
                if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                current = Some((name.to_string(), format!("{line}\n")));
            }
        }
        bodies
    }

    /// Functions that decide something on `config.watch`, and their callers.
    ///
    /// `/__ruvyxa/trace` is gated in its handler rather than at registration,
    /// and the guard is one call away — `debug_traces_enabled`. Closing over
    /// callers rather than callees is what makes that count and keeps a plain
    /// helper like `with_security_headers` out: a function is guarded when it
    /// *calls* a guarded one, never when a guarded one calls it.
    fn watch_guarded_functions(
        bodies: &std::collections::HashMap<String, String>,
    ) -> std::collections::HashSet<String> {
        let mut guarded = bodies
            .iter()
            .filter(|(_, body)| body.contains("config.watch"))
            .map(|(name, _)| name.clone())
            .collect::<std::collections::HashSet<_>>();
        loop {
            let grown = bodies
                .iter()
                .filter(|(name, _)| !guarded.contains(*name))
                .filter(|(_, body)| {
                    guarded
                        .iter()
                        .any(|name| body.contains(&format!("{name}(")))
                })
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            if grown.is_empty() {
                return guarded;
            }
            guarded.extend(grown);
        }
    }

    /// The `.route(…)` call whose argument list contains `at`.
    ///
    /// A route path never contains a parenthesis, so balancing them is exact
    /// here and stays exact when rustfmt breaks an entry across lines.
    fn route_call_around(router: &str, at: usize) -> &str {
        let opened = router[..at]
            .rfind(".route(")
            .expect("the path must sit inside a .route( call")
            + ".route".len();
        let mut depth = 0usize;
        for (offset, character) in router[opened..].char_indices() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return &router[opened..=opened + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("the .route( call around byte {at} is never closed");
    }

    /// No endpoint the contract marks `dev` is served by a production host.
    ///
    /// `native: "dev"` was honoured three different ways — devtools at
    /// registration, `/__ruvyxa/trace` in the handler, and `/__ruvyxa/hmr`
    /// nowhere. `every_contract_endpoint_is_registered_on_the_native_router`
    /// asserts only that a contract endpoint *is* registered, so the one
    /// endpoint that was never gated at all was invisible to the gate that
    /// existed: `ruvyxa start` accepted unauthenticated WebSocket upgrades on
    /// `/__ruvyxa/hmr` that carry nothing, are never heartbeated, and are never
    /// timed out. This is the other direction — a `dev` endpoint has to be
    /// registered under `if config.watch {`, or handled by a function that
    /// reaches a `config.watch` guard.
    #[test]
    fn no_dev_only_endpoint_is_served_in_production() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/framework-endpoint-conformance.json"
        ))
        .expect("the framework endpoint contract must be valid JSON");
        let source = include_str!("lib.rs");
        let (_, router) = source
            .split_once("fn build_app_router(")
            .expect("build_app_router must exist");
        let router = router
            .split_once("\nfn ")
            .map_or(router, |(function, _)| function);
        let gate = router
            .find("if config.watch {")
            .expect("build_app_router must gate its dev-only routes on config.watch");

        let bodies = crate_function_bodies();
        let guarded = watch_guarded_functions(&bodies);

        for endpoint in contract["endpoints"]
            .as_array()
            .expect("endpoints must be an array")
        {
            if endpoint["native"].as_str() != Some("dev") {
                continue;
            }
            let path = endpoint["path"].as_str().expect("path must be a string");
            let quoted = format!("\"{path}\"");
            let at = router.find(&quoted).unwrap_or_else(|| {
                panic!("{path} is marked native: dev but build_app_router never registers it")
            });
            if at > gate {
                continue;
            }

            // Registered in every mode, so the handlers have to carry the gate.
            // The entry is the `.route(…)` call around the path, read by
            // balancing its parentheses — a window ending at the next `.route(`
            // swallows the comments after the last entry in the chain.
            let entry = route_call_around(router, at);
            let handlers = entry
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .filter(|word| bodies.contains_key(*word))
                .collect::<Vec<_>>();
            assert!(
                !handlers.is_empty(),
                "{path} is registered outside the config.watch block and no handler \
                 for it could be read out of the route chain"
            );
            for handler in handlers {
                assert!(
                    guarded.contains(handler),
                    "{path} is marked native: dev, is registered outside the \
                     `if config.watch {{` block, and {handler} has no config.watch \
                     guard, so a production `ruvyxa start` serves a dev endpoint"
                );
            }
        }
    }

    /// The two shutdown windows, and what each environment value means.
    ///
    /// Milliseconds, and the same two variable names, because these are the
    /// same two knobs on the same deployment as `standalone-server.ts`.
    #[test]
    fn shutdown_windows_resolve_the_way_the_standalone_host_resolves_them() {
        let production = ShutdownTiming::resolve(None, None, false);
        assert_eq!(production.grace, Duration::from_secs(25));
        assert_eq!(production.drain_delay, Duration::from_secs(5));

        // A five-second wait on Ctrl-C reads as a hung dev server.
        assert_eq!(
            ShutdownTiming::resolve(None, None, true).drain_delay,
            Duration::ZERO
        );
        // Explicit still wins in dev.
        assert_eq!(
            ShutdownTiming::resolve(None, Some("2000"), true).drain_delay,
            Duration::from_secs(2)
        );

        assert_eq!(
            ShutdownTiming::resolve(Some("30000"), Some("0"), false),
            ShutdownTiming {
                grace: Duration::from_secs(30),
                drain_delay: Duration::ZERO,
            }
        );
        // In-flight work keeps a budget of its own however the two are set.
        assert_eq!(
            ShutdownTiming::resolve(Some("4000"), Some("9000"), false).drain_delay,
            Duration::from_secs(2)
        );
        // Unparseable, negative, and empty all fall back rather than turning a
        // typo into "close immediately".
        for raw in ["", "  ", "later", "-1"] {
            assert_eq!(
                ShutdownTiming::resolve(Some(raw), Some(raw), false),
                production,
                "{raw:?} must fall back to the defaults"
            );
        }
    }

    /// One awaited signal, boxed so the source is a plain `FnMut`.
    type TestSignal = std::pin::Pin<Box<dyn std::future::Future<Output = &'static str> + Send>>;

    /// A signal source a test can fire, one signal per call.
    ///
    /// `serve_with_signals` calls its source again to watch for a second
    /// signal, so a latch would resolve that one immediately and no drain
    /// window would ever be observed. A queue consumes one message per await,
    /// which is what an operator pressing Ctrl-C twice actually is.
    fn test_signals() -> (
        tokio::sync::mpsc::UnboundedSender<()>,
        impl FnMut() -> TestSignal,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let source = move || {
            let rx = Arc::clone(&rx);
            Box::pin(async move {
                let _ = rx.lock().await.recv().await;
                "TEST"
            }) as TestSignal
        };
        (tx, source)
    }

    /// A readiness probe is a fresh connection, so the drain has to outlive the
    /// signal that starts it.
    ///
    /// `draining.store(true, …)` and `shutdown_tx.send(true)` were adjacent
    /// statements, so `axum::serve` stopped accepting on the tick the flag was
    /// set: the probe got `ECONNREFUSED` and the `503 {"status":"draining"}`
    /// the health handler builds, `Retry-After` and all, was unreachable code.
    /// Everything the orchestrator sent while it was still deregistering then
    /// failed in a browser instead of being retried against another instance.
    ///
    /// Read off a socket rather than through a `Router`, for the reason
    /// `the_connection_close_on_a_413_reaches_the_client` does it: what is
    /// under test is whether a new TCP connection is still accepted, which a
    /// response object cannot show. Not gated on platform — the 2026-08-28
    /// drain defect was missed because the equivalent test was Windows-skipped.
    #[tokio::test]
    async fn a_fresh_connection_reads_the_drain_status_before_the_socket_closes() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let draining = Arc::new(AtomicBool::new(false));
        // Stands in for `health_endpoint`, whose draining branch reads this
        // same flag out of `AppState` — a real one needs a worker pool and a
        // project on disk. What this test owns is the window, not the body.
        let flag = Arc::clone(&draining);
        let health = move || {
            let flag = Arc::clone(&flag);
            async move {
                if flag.load(Ordering::Relaxed) {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        [(header::RETRY_AFTER, "1")],
                        "{\"status\":\"draining\",\"host\":\"native\"}",
                    )
                        .into_response()
                } else {
                    (StatusCode::OK, "{\"status\":\"ok\",\"host\":\"native\"}").into_response()
                }
            }
        };
        let app = Router::new().route("/__ruvyxa/health", get(health));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        let (signals, source) = test_signals();
        let timing = ShutdownTiming {
            grace: Duration::from_secs(10),
            drain_delay: Duration::from_secs(5),
        };
        let served = tokio::spawn(serve_with_signals(
            vec![listener],
            app,
            Arc::clone(&draining),
            timing,
            source,
        ));

        assert!(
            probe_path(address, "/__ruvyxa/health")
                .await
                .starts_with("HTTP/1.1 200"),
            "the host must answer 200 before any signal arrives"
        );

        signals.send(()).unwrap();
        // The flag is raised before the drain window opens, so waiting on it
        // times the probe without racing a fixed sleep against a scheduler.
        for _ in 0..200 {
            if draining.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            draining.load(Ordering::Relaxed),
            "the drain flag must be raised as soon as the signal arrives"
        );

        let response = probe_path(address, "/__ruvyxa/health").await;
        let shown = single_log_line(&response);
        assert!(
            response.starts_with("HTTP/1.1 503"),
            "a probe connecting during the drain window must be accepted and told \
             this process is draining: {shown}"
        );
        assert!(
            response
                .lines()
                .any(|line| line.eq_ignore_ascii_case("retry-after: 1")),
            "the draining answer carries Retry-After: {shown}"
        );

        // A second signal means now, so the drain window is not a five-second
        // wait on the operator's second Ctrl-C.
        signals.send(()).unwrap();
        let stopped = tokio::time::timeout(Duration::from_secs(5), served).await;
        assert!(
            stopped.is_ok(),
            "a second signal must end the drain window immediately"
        );
        stopped.unwrap().unwrap().unwrap();
    }

    /// One request over one fresh connection, returned as raw bytes.
    async fn probe_path(address: SocketAddr, path: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut client = tokio::net::TcpStream::connect(address)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "a fresh connection to {path} was refused ({error}); the host \
                     stopped accepting before anything could read its answer"
                )
            });
        client
            .write_all(
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        tokio::time::timeout(
            Duration::from_secs(10),
            client.read_to_string(&mut response),
        )
        .await
        .expect("the host must answer a probe rather than hanging")
        .unwrap();
        response
    }

    /// `RUVYXA_MAX_CONCURRENCY` / `RUVYXA_MAX_QUEUE`, resolved the standalone
    /// host's way — including the two values that mean "off".
    #[test]
    fn admission_limits_resolve_the_way_the_standalone_host_resolves_them() {
        assert_eq!(
            resolve_admission_limits(Some("4"), None, false),
            Some(AdmissionLimits {
                concurrency: 4,
                queue: 16,
            })
        );
        assert_eq!(
            resolve_admission_limits(Some("2"), Some("3"), false),
            Some(AdmissionLimits {
                concurrency: 2,
                queue: 3,
            })
        );
        // Off, both ways: explicitly, and because one developer with a browser
        // is not a load event.
        assert_eq!(resolve_admission_limits(Some("0"), None, false), None);
        assert_eq!(resolve_admission_limits(None, None, true), None);
        // Explicit still wins in dev.
        assert_eq!(
            resolve_admission_limits(Some("1"), Some("1"), true),
            Some(AdmissionLimits {
                concurrency: 1,
                queue: 1,
            })
        );
        // Bounded by the machine, never zero, so a production host always has a
        // limit rather than inheriting one from `available_parallelism`.
        let default = resolve_admission_limits(None, None, false).expect("production admits");
        assert!(ADMISSION_DEFAULT_BOUNDS.contains(&default.concurrency));
        assert_eq!(
            default.queue,
            default.concurrency * ADMISSION_QUEUE_PER_SLOT
        );
        // A typo is not a limit of zero.
        assert_eq!(
            resolve_admission_limits(Some("plenty"), Some("plenty"), false),
            Some(default)
        );
    }

    /// Past the limit and the queue, the host refuses — and health still answers.
    ///
    /// There was no concurrency limit, no queue cap, and no overload answer on
    /// this host, while the standalone server — the same long-lived self-hosted
    /// shape — has `WorkerAdmissionController` and answers `503`. Under a cheap
    /// unauthenticated flood `ruvyxa start` degraded to unbounded queueing:
    /// every request waited, none was refused, and `/__ruvyxa/health` kept
    /// answering `200` because it does not read queue depth.
    ///
    /// The framework routes and the page fallback are composed by
    /// `compose_app_router`, the same function `build_app_router` calls, so
    /// "admission wraps the page fallback only" is asserted about the host
    /// rather than about a router this test assembled to match it. Moving the
    /// layer onto the framework router fails here.
    #[tokio::test]
    async fn a_saturated_host_sheds_load_and_still_answers_health() {
        use tokio::io::AsyncWriteExt;

        let control = AdmissionControl::new(AdmissionLimits {
            concurrency: 1,
            queue: 1,
        });
        let release = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let holding = Arc::clone(&release);
        let counter = Arc::clone(&started);
        let page = Router::new().fallback(move || {
            let release = Arc::clone(&holding);
            let started = Arc::clone(&counter);
            async move {
                started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                release.notified().await;
                "page"
            }
        });
        let admitted = Arc::clone(&control.waiting);
        let app = compose_app_router(
            Router::new().route("/__ruvyxa/health", get(async || "ok")),
            page,
            Some(control),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, server_make_service(app)).await.ok();
        });

        // One request in the single slot, held inside the handler.
        let mut running = tokio::net::TcpStream::connect(address).await.unwrap();
        running
            .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        wait_for("the first request never reached the handler", || {
            started.load(std::sync::atomic::Ordering::SeqCst) == 1
        })
        .await;

        // One more filling the single queue slot.
        let mut queued = tokio::net::TcpStream::connect(address).await.unwrap();
        queued
            .write_all(b"GET /queued HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        wait_for(
            "the second request was never queued, so either nothing bounds how many \
             requests this host starts at once, or admission is not in front of \
             the page fallback",
            || admitted.load(std::sync::atomic::Ordering::SeqCst) == 1,
        )
        .await;

        let refused = probe_path(address, "/refused").await;
        let shown = single_log_line(&refused);
        assert!(
            refused.starts_with("HTTP/1.1 503"),
            "past the limit and the queue the host must refuse rather than park \
             the caller on memory it has to keep: {shown}"
        );
        assert!(
            refused
                .lines()
                .any(|line| line.eq_ignore_ascii_case("retry-after: 1")),
            "a refusal a caller can act on carries Retry-After: {shown}"
        );

        let health = probe_path(address, "/__ruvyxa/health").await;
        assert!(
            health.starts_with("HTTP/1.1 200"),
            "readiness is answered before admission, or an orchestrator restarts \
             a process that was merely busy: {}",
            single_log_line(&health)
        );

        release.notify_waiters();
        server.abort();
        let _ = server.await;
    }

    /// Poll a condition rather than sleeping a fixed span against a scheduler.
    async fn wait_for(expected: &str, mut ready: impl FnMut() -> bool) {
        for _ in 0..500 {
            if ready() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{expected}");
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
        let html = plain_error_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "<script>alert(1)</script>",
        );

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
        let html = plain_error_page(StatusCode::NOT_FOUND, "Route not found");

        assert!(html.contains("<main class=\"error-card\""));
        assert!(html.contains("<span class=\"code\">404</span>"));
        assert!(html.contains("src=\"/ruvyxa.png\""));
        assert!(html.contains("This page could not be found."));
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

    /// A quoted value that spans lines is one value, not a truncation plus
    /// junk.
    ///
    /// A PEM key in `.env` is routine for the auth and deploy integrations this
    /// framework ships. A line-based parser gave `PRIVATE_KEY` the opening
    /// fence with its quote still attached, and then read the base64 body — it
    /// contains `=` — as further variables, which went into every worker
    /// process and into `build_dependency_hash`.
    #[test]
    fn parses_multi_line_and_commented_env_values() {
        let env = parse_env_source(
            "PRIVATE_KEY=\"-----BEGIN PRIVATE KEY-----\n\
             MIIBVgIBADAN+Bg==\n\
             -----END PRIVATE KEY-----\"\n\
             SINGLE='first\nsecond'\n\
             PORT=3000 # dev only\n\
             HASH=abc#def\n\
             QUOTED_HASH=\"a # b\"\n\
             AFTER=tail\n",
        );

        assert_eq!(
            env.get("PRIVATE_KEY"),
            Some(
                &"-----BEGIN PRIVATE KEY-----\nMIIBVgIBADAN+Bg==\n-----END PRIVATE KEY-----"
                    .to_string()
            )
        );
        // The base64 body carries `=`, so a line-based parser assigned it.
        assert!(!env.contains_key("MIIBVgIBADAN+Bg"));
        assert_eq!(env.get("SINGLE"), Some(&"first\nsecond".to_string()));
        // An unquoted trailing comment is not part of the value.
        assert_eq!(env.get("PORT"), Some(&"3000".to_string()));
        // A `#` with no whitespace before it is an ordinary character.
        assert_eq!(env.get("HASH"), Some(&"abc#def".to_string()));
        assert_eq!(env.get("QUOTED_HASH"), Some(&"a # b".to_string()));
        // Parsing resumed at the line after the multi-line value ended.
        assert_eq!(env.get("AFTER"), Some(&"tail".to_string()));
    }

    /// An unterminated quote takes one line, never the rest of the file.
    ///
    /// Consuming to end-of-file would turn one typo into "every variable below
    /// it disappeared", which is worse than the truncation it replaces.
    #[test]
    fn an_unterminated_quote_does_not_swallow_the_rest_of_the_env_file() {
        let env = parse_env_source("BROKEN=\"open\nNEXT=kept\n");

        assert_eq!(env.get("BROKEN"), Some(&"\"open".to_string()));
        assert_eq!(env.get("NEXT"), Some(&"kept".to_string()));
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

    /// The 413 retires the connection, and says so.
    ///
    /// Without the header a pooling client — every browser, and `fetch` itself —
    /// is entitled to reuse a socket hyper has already closed, so a later and
    /// unrelated request dies with a connection reset naming nothing to do with
    /// the upload that caused it.
    #[test]
    fn an_over_limit_body_is_refused_with_the_connection_retired() {
        let response = request_body_too_large("length limit exceeded");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response.headers()[axum::http::header::CONNECTION],
            "close",
            "the rest of the upload is still in flight on this socket"
        );
    }

    /// And hyper puts it on the wire.
    ///
    /// A header a handler sets is not a header the client receives: hyper owns
    /// the connection options and rewrites some of them. Asserting the response
    /// object alone would pass on a fix that never reaches anybody, so this
    /// reads the bytes.
    #[tokio::test]
    async fn the_connection_close_on_a_413_reaches_the_client() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn over_limit() -> Response {
            request_body_too_large("length limit exceeded")
        }

        let app = Router::new().route("/", get(over_limit));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, server_make_service(app))
                .await
                .unwrap();
        });

        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                b"GET / HTTP/1.1
Host: localhost

",
            )
            .await
            .unwrap();
        // Bounded: `read_to_string` ends when the server closes, which is the
        // behaviour under test. Without the bound a missing `close` option makes
        // this hang instead of fail, and a test that hangs reports nothing.
        let mut response = String::new();
        let read =
            tokio::time::timeout(Duration::from_secs(5), client.read_to_string(&mut response))
                .await;
        server.abort();
        let _ = server.await;
        let shown = single_log_line(&response);
        assert!(
            read.is_ok(),
            "the server kept the connection open, so the client was never told to \
             stop reading: {shown}"
        );
        read.unwrap().unwrap();

        assert!(response.starts_with("HTTP/1.1 413"), "{shown}");
        assert!(
            response
                .lines()
                .any(|line| line.eq_ignore_ascii_case("connection: close")),
            "hyper must forward the close option, not strip it: {shown}"
        );
    }

    /// Bytes read off a socket, rendered safe to put in a failure message.
    ///
    /// A panic message is a log line, and this text arrived over the network, so
    /// it is remote-controlled as far as anything reading the code can tell. A
    /// response carrying a carriage return splices whatever follows it into the
    /// log as a line of its own — a forged entry in the output a person or a CI
    /// job reads to decide what happened. That is `rust/log-injection`, and it
    /// is also just unreadable: an HTTP response is many lines and only the
    /// first few say anything.
    ///
    /// Rebuilt from an allowlist rather than escaped: every character that is
    /// not printable ASCII becomes a dot, so there is no escape syntax to get
    /// wrong and nothing to reason about. Bounded too, because a failure message
    /// carrying a megabyte of body helps nobody.
    fn single_log_line(response: &str) -> String {
        const LIMIT: usize = 240;
        let mut rendered = String::with_capacity(LIMIT);
        for character in response.chars() {
            if rendered.len() >= LIMIT {
                rendered.push('…');
                break;
            }
            rendered.push(if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                '.'
            });
        }
        rendered
    }

    /// The property the helper above exists for: no forged line, ever.
    #[test]
    fn a_socket_response_cannot_forge_a_line_in_a_failure_message() {
        let forged = "HTTP/1.1 200 OK\r\n\r\nthread 'main' panicked at: everything is fine";
        let rendered = single_log_line(forged);
        assert!(
            !rendered.contains('\n') && !rendered.contains('\r'),
            "a carriage return splices the rest into the log as its own entry: {rendered}"
        );
        // The text is still there to read, which is the point of showing it.
        assert!(rendered.contains("HTTP/1.1 200 OK"), "{rendered}");

        // Bounded, so a response body cannot bury the assertion that failed.
        let long = single_log_line(&"x".repeat(10_000));
        assert!(long.chars().count() <= 241, "{}", long.chars().count());
        assert!(long.ends_with('…'));
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

    /// `cache.maxEntries` and `cache.maxBytes` have to reach the render worker,
    /// because `@ruvyxa/core`'s `cache()` store lives inside it and this is the
    /// only channel it has. Both were read by the deployed build's registry
    /// alone, so the long-lived pool `ruvyxa dev` and `ruvyxa start` run — the
    /// one host where an unbounded in-memory tier has time to grow — silently
    /// used the store's defaults instead of the configured bound.
    ///
    /// Zero is asserted separately from "some number" because it is the value
    /// that carries the decision: `maxEntries: 0` turns the tier off and
    /// `maxBytes: 0` removes the memory ceiling, and a carrier that dropped it
    /// would answer both with the default while looking wired.
    /// `installDataCacheBounds` in `packages/ruvyxa/runtime/worker-pool.mjs` is
    /// the half that reads these.
    #[test]
    fn runtime_env_carries_the_configured_data_cache_bounds() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::production(temp.path(), "localhost", 3000);

        let unset = runtime_env(&config).unwrap();
        assert_eq!(unset.get("RUVYXA_DATA_CACHE_MAX_ENTRIES"), None);
        assert_eq!(unset.get("RUVYXA_DATA_CACHE_MAX_BYTES"), None);

        config.data_cache_max_entries = Some(0);
        config.data_cache_max_bytes = Some(0);
        let off = runtime_env(&config).unwrap();
        assert_eq!(
            off.get("RUVYXA_DATA_CACHE_MAX_ENTRIES").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            off.get("RUVYXA_DATA_CACHE_MAX_BYTES").map(String::as_str),
            Some("0")
        );

        config.data_cache_max_entries = Some(64);
        config.data_cache_max_bytes = Some(1_048_576);
        let bounded = runtime_env(&config).unwrap();
        assert_eq!(
            bounded
                .get("RUVYXA_DATA_CACHE_MAX_ENTRIES")
                .map(String::as_str),
            Some("64")
        );
        assert_eq!(
            bounded
                .get("RUVYXA_DATA_CACHE_MAX_BYTES")
                .map(String::as_str),
            Some("1048576")
        );
    }

    /// The other half of the same setting, and the half that decides whether
    /// two instances agree at all.
    ///
    /// The bounds above bound one worker's own tier. This names the store every
    /// worker in every instance shares, and until it travelled here it reached
    /// every deployed platform and neither host this crate serves — so several
    /// `ruvyxa start` instances behind one load balancer declared a shared
    /// store and cached per instance. `loadDataCacheHandler` in
    /// `packages/ruvyxa/runtime/worker-pool.mjs` is the half that reads it.
    ///
    /// The prefix travels beside it because a key without one is the collision
    /// the prefix exists to prevent: two deployments pointed at one managed
    /// store would both write `cache('user:1')` and read each other's answer.
    #[test]
    fn runtime_env_carries_the_configured_data_cache_handler() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::production(temp.path(), "localhost", 3000);

        let unset = runtime_env(&config).unwrap();
        assert_eq!(unset.get("RUVYXA_DATA_CACHE_HANDLER"), None);
        assert_eq!(unset.get("RUVYXA_DATA_CACHE_KEY_PREFIX"), None);

        let handler = temp.path().join("cache-handler.mjs");
        config.data_cache_handler = Some(handler.clone());
        config.data_cache_key_prefix = Some("build-id:".to_string());
        let wired = runtime_env(&config).unwrap();
        assert_eq!(
            wired.get("RUVYXA_DATA_CACHE_HANDLER").map(String::as_str),
            Some(handler.to_string_lossy().as_ref())
        );
        assert_eq!(
            wired
                .get("RUVYXA_DATA_CACHE_KEY_PREFIX")
                .map(String::as_str),
            Some("build-id:")
        );
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

    /// Each probe is a `<runtime> --version` process spawn, so the number of
    /// them is the cost of detection, not an implementation detail. Node
    /// answering has to end the question: the eager form asked all three on
    /// every call, and a warm build asks six times.
    #[test]
    fn detection_stops_probing_once_a_runtime_answers() {
        let mut probed = Vec::new();
        let selected = JavaScriptRuntime::detect_by(|runtime| {
            probed.push(runtime);
            runtime == JavaScriptRuntime::Node
        });
        assert_eq!(selected, JavaScriptRuntime::Node);
        assert_eq!(probed, vec![JavaScriptRuntime::Node]);

        let mut probed = Vec::new();
        let selected = JavaScriptRuntime::detect_by(|runtime| {
            probed.push(runtime);
            runtime == JavaScriptRuntime::Deno
        });
        assert_eq!(selected, JavaScriptRuntime::Deno);
        assert_eq!(
            probed,
            vec![
                JavaScriptRuntime::Node,
                JavaScriptRuntime::Bun,
                JavaScriptRuntime::Deno
            ]
        );
    }

    /// The answer is cached for the life of the process, so a second call must
    /// not spawn anything. Asserted through the public entry point because the
    /// cache is what callers actually get.
    #[test]
    fn repeated_detection_answers_from_the_first_probe() {
        let first = JavaScriptRuntime::detect();
        let second = JavaScriptRuntime::detect();
        assert_eq!(first, second);
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
        assert!(!is_safe_relative_path(""));
        assert!(!is_safe_relative_path("../secret.txt"));
        assert!(!is_safe_relative_path("images\\logo.png"));
        // A `.` segment used to be accepted here and rejected by the deployed
        // handler's `isUnsafeSegment`, so one URL resolved differently under
        // `ruvyxa start` than in a deployed build. Browsers normalize `.` away
        // before sending, and no route the build emits contains one, so the two
        // now agree on rejecting it. See
        // `tests/fixtures/prerender-path-conformance.json`.
        assert!(!is_safe_relative_path("./images/logo.png"));
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

    /// The traversal rule is textual, and a text rule can overreach.
    ///
    /// `resolve_public_asset` refuses `../` on the string as well as by segment,
    /// because a substring test is the only traversal check a static analyser
    /// follows. Widening that literal to a bare `".."` — the other spelling the
    /// analyser accepts — would pass the same scan and 404 a legitimate file,
    /// silently, for as long as nobody happened to ship one. So the file this
    /// asserts on is exactly that: a name with two dots in it and no traversal.
    #[test]
    fn serves_a_public_file_whose_name_merely_contains_two_dots() {
        let temp = tempfile::tempdir().unwrap();
        let public = temp.path().join("public");
        fs::create_dir_all(&public).unwrap();
        fs::write(public.join("sprite..png"), b"png").unwrap();

        assert!(resolve_public_asset(&public, "sprite..png").is_some());
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

    /// Where `ruvyxa build` actually writes the client build report.
    ///
    /// Beside `client/`, not inside it: that directory is public by contract —
    /// every file in it is served and copied to a CDN — while the report
    /// carries the build machine's absolute source paths and the module graph
    /// of every chunk. See `client_build_report_path` in `html_document.rs`.
    ///
    /// A helper rather than a literal per test because these fixtures kept
    /// naming `client/manifest.json` for two moves after the report left it.
    /// The suite stayed green the whole time while `ruvyxa start` served every
    /// page with no client script at all, because a manifest that is not there
    /// means "this project ships no client bundle".
    fn client_report_path(client_dir: &Path) -> PathBuf {
        client_dir
            .parent()
            .expect("a client directory sits inside a build directory")
            .join("client-report.json")
    }

    #[test]
    fn reads_prebuilt_client_assets_from_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            client_report_path(&client_dir),
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.123.js"}]}]}"#,
        )
        .unwrap();

        let config = ServerConfig::production(temp.path(), "localhost", 3000);

        let assets = prebuilt_client_assets(&config, "/").unwrap();
        assert_eq!(assets.src, "/__ruvyxa/client/home.js");
        assert_eq!(assets.preloads, vec!["/__ruvyxa/client/shared.123.js"]);

        std::fs::write(
            client_report_path(&client_dir),
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
            client_report_path(&client_dir),
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
    fn client_manifest_cache_evicts_oldest_roots_past_its_bound() {
        use crate::html_document::{MAX_CACHED_MANIFEST_ROOTS, cached_client_manifest_roots};

        // The cache is process-global and keyed by manifest path, so a process
        // that sees many roots used to retain every parse it ever made. Walk
        // past the bound and confirm the cache stops growing while still
        // answering correctly for the roots it is asked about.
        let temps: Vec<_> = (0..MAX_CACHED_MANIFEST_ROOTS + 8)
            .map(|index| {
                let temp = tempfile::tempdir().unwrap();
                let client_dir = temp.path().join(".ruvyxa/client");
                std::fs::create_dir_all(&client_dir).unwrap();
                std::fs::write(
                    client_report_path(&client_dir),
                    format!(
                        r#"{{"routes":[{{"path":"/","src":"/__ruvyxa/client/home.{index}.js","sharedChunks":[]}}]}}"#
                    ),
                )
                .unwrap();
                (temp, index)
            })
            .collect();

        for (temp, index) in &temps {
            let config = ServerConfig::production(temp.path(), "localhost", 3000);
            let assets = prebuilt_client_assets(&config, "/").unwrap();
            assert_eq!(assets.src, format!("/__ruvyxa/client/home.{index}.js"));
        }

        assert!(
            cached_client_manifest_roots() <= MAX_CACHED_MANIFEST_ROOTS,
            "manifest cache grew past its bound: {} entries",
            cached_client_manifest_roots(),
        );

        // An evicted root must still resolve — eviction costs a re-parse, never
        // a wrong answer.
        let (first_temp, first_index) = &temps[0];
        let config = ServerConfig::production(first_temp.path(), "localhost", 3000);
        assert_eq!(
            prebuilt_client_assets(&config, "/").unwrap().src,
            format!("/__ruvyxa/client/home.{first_index}.js"),
        );
    }

    #[test]
    fn client_manifest_cache_refreshes_after_rebuild() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        let manifest = client_report_path(&client_dir);
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
        let manifest = client_report_path(&client_dir);

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
        let manifest = client_report_path(&client_dir);
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
            client_report_path(&client_dir),
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
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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
        no_hydrate.render.hydration = ruvyxa_graph::HydrationMode::None;
        assert_eq!(
            client_hydration_script(&config, &no_hydrate, "/", &BTreeMap::new()),
            ""
        );
    }

    /// The Flight payload is a data block, quoted as a JSON string.
    ///
    /// Quoted, because the payload is line-delimited and is not itself a JSON
    /// document — quoting is what lets one escaping rule cover it and the
    /// bootstrap block alike. A data block rather than an executable script,
    /// because a `Content-Security-Policy` without `'unsafe-inline'` blocks
    /// inline script and a per-request payload cannot be covered by a hash.
    #[test]
    fn the_rsc_payload_rides_in_an_escaped_data_block() {
        let open = r#"<script type="application/json" id="__ruvyxa-rsc">"#;

        // A payload React produced from user data can contain anything. The
        // property asserted is the one that matters and does not depend on
        // which escape spelling is used: nothing inside the element can be read
        // as markup, and nothing can end a JavaScript line early.
        let hostile = html_document::rsc_payload_block("</script><img src=x>&\u{2028}\u{2029}");
        assert!(hostile.starts_with(open), "{hostile}");
        assert!(hostile.ends_with("</script>"), "{hostile}");
        let inner = &hostile[open.len()..hostile.len() - "</script>".len()];
        for forbidden in ['<', '>', '&', '\u{2028}', '\u{2029}'] {
            assert!(
                !inner.contains(forbidden),
                "{forbidden:?} survived unescaped in {inner}",
            );
        }

        // And the round trip still yields the exact bytes React wrote.
        let payload =
            "0:[\"$\",\"main\",null,{}]\n5:I[\"ruv:m_0123456789abcdef\",[],\"default\"]\n";
        let block = html_document::rsc_payload_block(payload);
        let inner = &block[open.len()..block.len() - "</script>".len()];
        let decoded: String = serde_json::from_str(
            &inner
                .replace("\\u003c", "<")
                .replace("\\u003e", ">")
                .replace("\\u0026", "&"),
        )
        .unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn deferred_hydration_uses_loader_without_preloading_route_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            client_report_path(&client_dir),
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
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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

    /// The built stylesheet URL is a cache slot like the other four.
    ///
    /// It was the one slot `invalidate` skipped, so its generation never moved
    /// off zero — and `built_style_asset` read the generation from inside the
    /// write lock it was about to write through, which made
    /// `insert_if_current` a tautology. The two halves hid each other: the
    /// guard could not refuse anything, and nothing ever asked it to.
    ///
    /// This asserts both. The first half is behavioural — an invalidation has
    /// to reach this slot, which is what makes the guard reachable at all. The
    /// second drives the two halves of `built_style_asset` by hand, the way
    /// `a_stylesheet_saved_during_a_collection_is_not_installed_stale` drives
    /// `styles()`, because the window it describes is between a filesystem read
    /// and a lock acquisition and cannot be scheduled deterministically from
    /// outside.
    #[tokio::test]
    async fn a_stylesheet_url_read_before_an_invalidation_is_not_installed_stale() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::production(temp.path(), "localhost", 3000);
        config.client_dir = temp.path().join("client");
        std::fs::create_dir_all(&config.client_dir).unwrap();
        let manifest = config.client_dir.join("route-manifest.json");
        std::fs::write(&manifest, r#"{"styles":["/assets/stale.css"]}"#).unwrap();

        let cache = RuntimeCache::default();

        assert_eq!(
            cache.built_style_asset(&config).await.as_deref(),
            Some("/assets/stale.css")
        );

        // Rewriting the manifest must not change the cached answer: reading it
        // once per generation instead of once per render is the point of the
        // slot.
        std::fs::write(&manifest, r#"{"styles":["/assets/fresh.css"]}"#).unwrap();
        assert_eq!(
            cache.built_style_asset(&config).await.as_deref(),
            Some("/assets/stale.css")
        );

        // An invalidation has to reach this slot, or every render after an
        // in-place redeploy links a stylesheet the build no longer wrote.
        cache.invalidate_async().await;
        assert_eq!(
            cache.built_style_asset(&config).await.as_deref(),
            Some("/assets/fresh.css"),
            "invalidate() must reach style_asset like every other cache slot"
        );

        // First half of `built_style_asset`: the slot is empty, so remember the
        // generation the manifest read is being made against.
        cache.invalidate_async().await;
        let generation = {
            let cached = cache.style_asset.read().await;
            assert!(cached.value.is_none(), "the slot must start empty");
            cached.generation
        };

        // A redeploy lands while that read is in flight.
        cache.invalidate_async().await;

        // Second half: the answer read against the old generation is refused,
        // and the slot stays empty so the next render reads again.
        let stale: Option<Arc<str>> = Some(Arc::from("/assets/stale.css"));
        assert!(
            !cache
                .style_asset
                .write()
                .await
                .insert_if_current(generation, stale),
            "a manifest read that started before the invalidation must not install"
        );
        assert!(cache.style_asset.read().await.value.is_none());
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

    /// The favicon link is derived from a filesystem stat, and every page render
    /// used to redo it. Caching it means the answer is computed once and only
    /// recomputed when the watcher invalidates the runtime cache.
    /// A runtime installed by its own installer has to be findable.
    ///
    /// Both installers write to `~/.<runtime>/bin` and add it to `PATH`, so a
    /// shell that has not been restarted since — or a tool launched from one
    /// that never had it — sees nothing on `PATH`. Deno 2.9.5 was installed
    /// exactly that way on the machine this was written on and `doctor`
    /// reported it missing, which also made `--runtime deno` unselectable.
    #[test]
    fn a_runtime_is_found_where_its_own_installer_puts_it() {
        let home = tempfile::tempdir().expect("temp dir");
        for runtime in [JavaScriptRuntime::Bun, JavaScriptRuntime::Deno] {
            assert_eq!(
                runtime_home_executable(runtime, home.path()),
                None,
                "{runtime:?} must not be claimed before it is installed"
            );

            let bin = home
                .path()
                .join(format!(".{}", runtime.command()))
                .join("bin");
            std::fs::create_dir_all(&bin).expect("bin dir");
            let file = if cfg!(windows) {
                format!("{}.exe", runtime.command())
            } else {
                runtime.command().to_string()
            };
            std::fs::write(bin.join(&file), []).expect("write");

            assert_eq!(
                runtime_home_executable(runtime, home.path()),
                Some(bin.join(&file)),
                "{runtime:?} must be found where its installer put it"
            );
        }
        // Node is not installed this way, and asking would only find a
        // directory nobody writes.
        assert_eq!(
            runtime_home_executable(JavaScriptRuntime::Node, home.path()),
            None
        );
    }

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

    /// A save during the first style collection is not silently discarded.
    ///
    /// `styles()` reads the generation, drops the lock, collects off-thread,
    /// and installs only if the generation still matches — which is what makes
    /// a watcher event during a collection safe. `invalidate_styles_for_paths`
    /// used to skip the bump whenever the slot held no value, and an in-flight
    /// collection is precisely the state where it holds none. The collection
    /// then installed CSS it had read before the save, and the dev server
    /// served the previous stylesheet until the *next* CSS change — the shape
    /// of "my CSS edit did not show up, so I saved again and then it did".
    ///
    /// The interleaving is reproduced directly rather than raced: the two
    /// halves of `styles()` around its off-thread collection are the two
    /// `styles` accesses below, with the watcher event in between.
    #[tokio::test]
    async fn a_stylesheet_saved_during_a_collection_is_not_installed_stale() {
        let cache = Arc::new(RuntimeCache::default());
        let stylesheet = PathBuf::from("/project/styles/site.css");

        // First half of `styles()`: no cached value, so remember the generation
        // and start collecting.
        let generation = {
            let cached = cache.styles.read().await;
            assert!(cached.value.is_none(), "the slot must start empty");
            cached.generation
        };

        // The save lands while that collection is running.
        let watcher_cache = Arc::clone(&cache);
        let watched = stylesheet.clone();
        let invalidated = tokio::task::spawn_blocking(move || {
            watcher_cache.invalidate_styles_for_paths(&[watched])
        })
        .await
        .unwrap();
        assert!(
            invalidated,
            "a change that cannot be checked against a file set has to be treated as relevant"
        );

        // Second half of `styles()`: the collection finishes holding the bytes
        // it read before the save, and must not become the cached answer.
        let stale = StyleCacheEntry {
            css: "body { color: navy; }".to_string(),
            files: BTreeSet::from([normalize_cache_path(&stylesheet)]),
        };
        let installed = cache
            .styles
            .write()
            .await
            .insert_if_current(generation, stale);
        assert!(
            !installed,
            "a collection that started before the save installed its stale CSS"
        );
        assert!(
            cache.styles.read().await.value.is_none(),
            "the slot must stay empty so the next request collects again"
        );
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

    #[tokio::test]
    async fn bind_listeners_use_next_available_port_when_requested_port_is_busy() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_address = occupied.local_addr().unwrap();
        if occupied_address.port() == u16::MAX {
            return;
        }

        let config = ServerConfig::dev(".", "127.0.0.1", occupied_address.port());
        let (_listeners, bound_address) = bind_listeners(&config, occupied_address).await.unwrap();

        assert!(bound_address.port() > occupied_address.port());
        assert!(
            bound_address.port()
                <= occupied_address
                    .port()
                    .saturating_add(PORT_FALLBACK_SCAN_LIMIT)
        );
    }

    /// The default host must answer on both loopback families.
    ///
    /// `localhost` resolves to `::1` and `127.0.0.1`, and the server used to
    /// take whichever one the resolver returned first — `::1` on Windows. A
    /// browser falls back to the other family and never notices; `proxy_pass
    /// http://127.0.0.1:3000`, a container health probe, and `curl 127.0.0.1`
    /// do not, and got "connection refused" from a server that was serving
    /// perfectly well.
    ///
    /// Connections are opened rather than listeners inspected, because the
    /// claim is about reachability, and both have to be on one port: two
    /// families on two ports is the same failure wearing a different shape.
    #[tokio::test]
    async fn the_default_host_accepts_connections_on_every_loopback_family() {
        use tokio::net::TcpStream;

        // Nothing to prove on a host that cannot serve IPv6 loopback at all.
        let Ok(probe) = TcpListener::bind("[::1]:0").await else {
            return;
        };
        drop(probe);

        let config = ServerConfig::dev(".", "localhost", 0);
        let requested = resolve_bind_address(&config).unwrap();
        let (listeners, bound_address) = bind_listeners(&config, requested).await.unwrap();
        let port = bound_address.port();

        let bound: Vec<SocketAddr> = listeners
            .iter()
            .map(|listener| listener.local_addr().unwrap())
            .collect();
        assert!(
            bound.iter().all(|address| address.port() == port),
            "every loopback family must answer on one port, got {bound:?}"
        );

        for address in [
            SocketAddr::from(([127, 0, 0, 1], port)),
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
        ] {
            assert!(
                TcpStream::connect(address).await.is_ok(),
                "the default host must be reachable at {address}"
            );
        }
    }

    /// The reported bug: another project holds `127.0.0.1:3000`, Ruvyxa binds
    /// `[::1]:3000` because that is what `localhost` resolved to first, both
    /// succeed, and `http://localhost:3000` reaches whichever server the
    /// browser's resolver happens to pick. A port is only free when it is free
    /// on every address the host answers to.
    #[tokio::test]
    async fn bind_listeners_move_on_when_the_other_loopback_family_is_taken() {
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
        let (_listeners, bound_address) = bind_listeners(&config, requested).await.unwrap();

        assert_ne!(
            bound_address.port(),
            port,
            "the IPv4 loopback holder must push the server to another port"
        );
    }

    /// A production host takes the port it was told to take, or fails.
    ///
    /// `ruvyxa dev` scanning forward is a convenience for a developer reading
    /// the terminal. `ruvyxa start` has no such reader: a container routes to
    /// the configured port, so a process that quietly bound the next one is
    /// healthy-looking and unreachable, and the orchestrator reports a restart
    /// loop with the real cause — usually the previous instance still holding
    /// the port — only as a line on stdout. The other self-hosted host, the
    /// generated standalone server, has always let `EADDRINUSE` surface.
    #[tokio::test]
    async fn bind_listeners_refuse_a_busy_port_in_production_instead_of_moving() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_address = occupied.local_addr().unwrap();

        let config = ServerConfig::production(".", "127.0.0.1", occupied_address.port());
        let error = bind_listeners(&config, occupied_address)
            .await
            .expect_err("a production server must not bind a port nothing routes to");

        let RuvyxaError::Diagnostic(diagnostic) = error else {
            panic!("a port conflict must be reported as a diagnostic, got {error:?}");
        };
        assert_eq!(diagnostic.code, "RUV1201");
        assert!(
            diagnostic
                .explanation
                .contains(&occupied_address.port().to_string()),
            "the diagnostic must name the port that could not be taken: {}",
            diagnostic.explanation
        );
        assert!(
            !diagnostic
                .explanation
                .contains("could not find a free port"),
            "production scans no range, so the message must not claim one: {}",
            diagnostic.explanation
        );

        // The dev host keeps the fallback, and this is the same call: only
        // `config.watch` separates them.
        if occupied_address.port() < u16::MAX {
            let dev = ServerConfig::dev(".", "127.0.0.1", occupied_address.port());
            let (_listeners, bound) = bind_listeners(&dev, occupied_address).await.unwrap();
            assert!(bound.port() > occupied_address.port());
        }
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
}
