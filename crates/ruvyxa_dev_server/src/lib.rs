use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use ruvyxa_graph::{discover_routes, DiscoverOptions, RouteEntry, RouteKind, RouteManifest};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{info, warn};
use walkdir::WalkDir;

const MAX_ACTION_BODY_BYTES: usize = 64 * 1024;
const ACTION_RATE_LIMIT_MAX: usize = 60;
const ACTION_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub root: PathBuf,
    pub app_dir: PathBuf,
    pub public_dir: PathBuf,
    pub client_dir: PathBuf,
    pub host: String,
    pub port: u16,
    pub watch: bool,
}

impl ServerConfig {
    pub fn dev(root: impl Into<PathBuf>, host: impl Into<String>, port: u16) -> Self {
        let root = root.into();
        Self {
            app_dir: root.join("app"),
            public_dir: root.join("public"),
            client_dir: root.join(".ruvyxa/client"),
            root,
            host: host.into(),
            port,
            watch: true,
        }
    }

    pub fn production(root: impl Into<PathBuf>, host: impl Into<String>, port: u16) -> Self {
        let root = root.into();
        Self {
            app_dir: root.join(".ruvyxa/server/app"),
            public_dir: root.join(".ruvyxa/assets"),
            client_dir: root.join(".ruvyxa/client"),
            root,
            host: host.into(),
            port,
            watch: false,
        }
    }
}

#[derive(Clone)]
struct AppState {
    config: ServerConfig,
    reload_tx: broadcast::Sender<String>,
    action_limiter: Arc<Mutex<ActionRateLimiter>>,
}

pub async fn serve(config: ServerConfig) -> Result<()> {
    let manifest = discover_routes(DiscoverOptions::new(&config.app_dir))?;
    info!(routes = manifest.routes.len(), "discovered routes");

    let (reload_tx, _) = broadcast::channel(64);
    let state = AppState {
        config: config.clone(),
        reload_tx,
        action_limiter: Arc::new(Mutex::new(ActionRateLimiter::default())),
    };

    let _watcher = if config.watch {
        Some(start_watcher(
            &watch_paths(&config),
            state.reload_tx.clone(),
        )?)
    } else {
        None
    };

    let app = Router::new()
        .route("/__ruvyxa/hmr", get(hmr_ws))
        .route("/__ruvyxa/client", get(client_bundle))
        .route(
            "/__ruvyxa/action",
            post(action_endpoint).layer(DefaultBodyLimit::max(MAX_ACTION_BODY_BYTES)),
        )
        .route("/__ruvyxa/trace", get(trace_endpoint))
        .fallback(handle_request)
        .with_state(Arc::new(state));

    let address: SocketAddr = format!("{}:{}", config.host, config.port)
        .to_socket_addrs()
        .map_err(|error| RuvyxaError::Message(format!("Invalid server address: {error}")))?
        .next()
        .ok_or_else(|| RuvyxaError::Message("Server address did not resolve".to_string()))?;
    let listener = TcpListener::bind(address).await?;

    info!("Ruvyxa server listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn start_watcher(
    watch_paths: &[PathBuf],
    reload_tx: broadcast::Sender<String>,
) -> Result<RecommendedWatcher> {
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                let paths = event.paths;
                let payload = serde_json::json!({
                    "type": classify_hmr_event(&paths),
                    "paths": paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>(),
                })
                .to_string();
                let _ = reload_tx.send(payload);
            }
            Err(error) => warn!(%error, "file watcher error"),
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
    [
        config.app_dir.clone(),
        config.root.join("components"),
        config.root.join("server"),
        config.public_dir.clone(),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

async fn hmr_ws(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        let mut reload_rx = state.reload_tx.subscribe();

        while let Ok(payload) = reload_rx.recv().await {
            if socket
                .send(Message::Text(payload.into()))
                .await
                .is_err()
            {
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
    let response = match render_client_bundle(&state.config, &query.path) {
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
    Query(query): Query<ActionQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = validate_action_request(&headers, body.len()) {
        return with_security_headers(response);
    }

    let rate_key = action_rate_limit_key(&headers, &query);
    if !state
        .action_limiter
        .lock()
        .expect("action limiter mutex poisoned")
        .allow(&rate_key)
    {
        return with_security_headers(
            (StatusCode::TOO_MANY_REQUESTS, "Action rate limit exceeded").into_response(),
        );
    }

    let response = match render_server_action(
        &state.config,
        &query.path,
        &query.name,
        std::str::from_utf8(&body).unwrap_or("{}"),
    ) {
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
    let response = match runtime_trace(&state.config, &query.path) {
        Ok(trace) => json_response(StatusCode::OK, &trace),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("console.error({:?});", error.to_string()),
        )
            .into_response(),
    };
    with_security_headers(response)
}

async fn handle_request<B>(
    State(state): State<Arc<AppState>>,
    request: Request<B>,
) -> impl IntoResponse {
    match render_request(
        &state.config,
        request.uri().path(),
        request.method().as_str(),
    ) {
        Ok(response) => response,
        Err(error) => {
            let body = error_page(&error.to_string());
            html_response(StatusCode::INTERNAL_SERVER_ERROR, body)
        }
    }
}

pub fn render_request(config: &ServerConfig, request_path: &str, method: &str) -> Result<Response> {
    if let Some(client_response) = serve_client_file(&config.client_dir, request_path)? {
        return Ok(client_response);
    }

    if let Some(public_response) = serve_public_file(&config.public_dir, request_path)? {
        return Ok(public_response);
    }

    let manifest = discover_routes(DiscoverOptions::new(&config.app_dir))?;
    let Some(route_match) = find_route(&manifest, request_path) else {
        return Ok(html_response(
            StatusCode::NOT_FOUND,
            error_page("Route not found"),
        ));
    };

    match route_match.route.kind {
        RouteKind::Page => {
            let html = render_page(config, route_match.route, request_path, &route_match.params)?;
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

fn runtime_trace(config: &ServerConfig, request_path: &str) -> Result<RuntimeTrace> {
    let manifest = discover_routes(DiscoverOptions::new(&config.app_dir))?;
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

fn serve_public_file(public_dir: &Path, request_path: &str) -> Result<Option<Response>> {
    let trimmed = request_path.trim_start_matches('/');
    if !is_safe_relative_path(trimmed) {
        return Ok(None);
    }

    let file = public_dir.join(trimmed);
    if !file.is_file() {
        return Ok(None);
    }

    let bytes = fs::read(&file)?;
    let content_type = content_type_for(&file);
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    apply_security_headers(&mut response);
    Ok(Some(response))
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

fn serve_client_file(client_dir: &Path, request_path: &str) -> Result<Option<Response>> {
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

fn with_security_headers(mut response: Response) -> Response {
    apply_security_headers(&mut response);
    response
}

fn render_page(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let source = fs::read_to_string(&route.file).map_err(|source| RuvyxaError::Io {
        message: format!("Failed to read page module {}", route.file.display()),
        source,
    })?;

    if !source.contains("export default") {
        return Err(
            Diagnostic::new("RUV1004", "Page is missing a default export")
                .explain("Every page.tsx file must export a default component.")
                .at_file(&route.file)
                .suggest("Add `export default function Page() { return <main /> }`.")
                .into(),
        );
    }

    let rendered = render_react_page(config, route, request_path, params)?;
    let styles = collect_css(&config.root, &config.app_dir)?;
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientRenderResult {
    ok: bool,
    script: Option<String>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionRenderResult {
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

fn find_client_renderer(root: &Path) -> Option<PathBuf> {
    find_runtime_script(root, "client-renderer.mjs")
}

fn find_action_renderer(root: &Path) -> Option<PathBuf> {
    find_runtime_script(root, "action-renderer.mjs")
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
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(&value),
            ) {
                response.headers_mut().insert(name, value);
            }
        }
    }

    Ok(with_security_headers(response))
}

fn render_client_bundle(config: &ServerConfig, request_path: &str) -> Result<String> {
    let manifest = discover_routes(DiscoverOptions::new(&config.app_dir))?;
    let Some(route_match) = find_route(&manifest, request_path) else {
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

    let renderer = find_client_renderer(&config.root).ok_or_else(|| {
        Diagnostic::new("RUV1302", "Client renderer was not found")
            .explain("Ruvyxa could not find the Node client renderer used to bundle browser hydration code.")
            .suggest("Run pnpm install from the monorepo root, or install the ruvyxa package in the app.")
    })?;

    let output = node_command(&config.root)?
        .arg(&renderer)
        .arg(&config.root)
        .arg(&config.app_dir)
        .arg(&route_match.route.file)
        .arg(request_path)
        .arg(
            serde_json::to_string(&route_match.params)
                .map_err(|error| RuvyxaError::Message(error.to_string()))?,
        )
        .output()
        .map_err(|source| RuvyxaError::Io {
            message: "Failed to start Node for client hydration bundling".to_string(),
            source,
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let result: ClientRenderResult = serde_json::from_str(&stdout).map_err(|error| {
        RuvyxaError::Message(format!(
            "Client renderer returned invalid output: {error}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        ))
    })?;

    if output.status.success() && result.ok {
        return result.script.ok_or_else(|| {
            RuvyxaError::Message("Client renderer completed without script output".to_string())
        });
    }

    let code = result.code.unwrap_or_else(|| "RUV1300".to_string());
    let message = result
        .message
        .unwrap_or_else(|| "Client bundling failed without an error message".to_string());
    let explanation = if let Some(stack) = result.stack {
        format!("{message}\n\n{stack}")
    } else {
        message
    };

    Err(
        Diagnostic::new("RUV1300", "Client hydration bundling failed")
            .explain(format!("{code}: {explanation}"))
            .suggest("Check the page component, its browser-safe imports, and React dependencies.")
            .into(),
    )
}

fn render_server_action(
    config: &ServerConfig,
    request_path: &str,
    action_name: &str,
    payload_json: &str,
) -> Result<Response> {
    let manifest = discover_routes(DiscoverOptions::new(&config.app_dir))?;
    let Some(route_match) = find_route(&manifest, request_path) else {
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

    let renderer = find_action_renderer(&config.root).ok_or_else(|| {
        Diagnostic::new("RUV1502", "Action renderer was not found")
            .explain(
                "Ruvyxa could not find the Node action renderer used to execute server actions.",
            )
            .suggest("Run from the monorepo root or install the ruvyxa package into the app.")
    })?;

    let output = node_command(&config.root)?
        .arg(renderer)
        .arg(&config.root)
        .arg(action_file)
        .arg(action_name)
        .arg(payload_json)
        .arg(request_path)
        .output()
        .map_err(|source| RuvyxaError::Io {
            message: "Failed to start Node for server action execution".to_string(),
            source,
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: ActionRenderResult =
        serde_json::from_str(&stdout).map_err(|error| RuvyxaError::Message(error.to_string()))?;

    if !result.ok {
        let mut diagnostic = Diagnostic::new(
            action_error_code(result.code.as_deref()),
            "Server action execution failed",
        )
        .explain(
            result
                .message
                .unwrap_or_else(|| "Unknown server action error".to_string()),
        )
        .at_file(&route_match.route.file);

        if let Some(stack) = result.stack {
            diagnostic = diagnostic.suggest(stack);
        }

        return Err(diagnostic.into());
    }

    let status = StatusCode::from_u16(result.status.unwrap_or(200)).unwrap_or(StatusCode::OK);
    let mut response = (status, result.body.unwrap_or_default()).into_response();

    if let Some(headers) = result.headers {
        for (key, value) in headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(key.as_bytes()),
                HeaderValue::from_str(&value),
            ) {
                response.headers_mut().insert(name, value);
            }
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

struct MatchedRoute<'a> {
    route: &'a RouteEntry,
    params: BTreeMap<String, String>,
}

fn find_route<'a>(manifest: &'a RouteManifest, request_path: &str) -> Option<MatchedRoute<'a>> {
    manifest.routes.iter().find_map(|route| {
        route_params(&route.path, request_path).map(|params| MatchedRoute { route, params })
    })
}

fn route_params(pattern: &str, request_path: &str) -> Option<BTreeMap<String, String>> {
    let pattern_parts = split_path(pattern);
    let request_parts = split_path(request_path);
    let mut params = BTreeMap::new();

    let mut pattern_index = 0;
    let mut request_index = 0;

    while pattern_index < pattern_parts.len() {
        let pattern_part = pattern_parts[pattern_index];

        if pattern_part.starts_with('*') {
            let name = pattern_part.trim_start_matches('*');
            params.insert(name.to_string(), request_parts[request_index..].join("/"));
            return Some(params);
        }

        if pattern_part.ends_with('?') && pattern_part.starts_with(':') {
            let name = pattern_part
                .trim_start_matches(':')
                .trim_end_matches('?')
                .to_string();
            pattern_index += 1;
            if request_index < request_parts.len() {
                params.insert(name, request_parts[request_index].to_string());
                request_index += 1;
            }
            continue;
        }

        let request_part = request_parts.get(request_index)?;

        if !pattern_part.starts_with(':') && pattern_part != *request_part {
            return None;
        }

        if pattern_part.starts_with(':') {
            params.insert(
                pattern_part.trim_start_matches(':').to_string(),
                (*request_part).to_string(),
            );
        }

        pattern_index += 1;
        request_index += 1;
    }

    if request_index == request_parts.len() {
        Some(params)
    } else {
        None
    }
}

fn split_path(path: &str) -> Vec<&str> {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn collect_css(root: &Path, app_dir: &Path) -> Result<String> {
    let mut css = String::new();

    for entry in WalkDir::new(app_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "css")
        {
            let source = fs::read_to_string(entry.path())?;
            if source.contains("@import \"tailwindcss\"")
                || source.contains("@import 'tailwindcss'")
            {
                css.push_str(&compile_tailwind_css(root, entry.path())?);
            } else {
                css.push_str(&source);
            }
            css.push('\n');
        }
    }

    Ok(css)
}

fn compile_tailwind_css(root: &Path, input: &Path) -> Result<String> {
    let tailwind = find_tailwind_cli(root).ok_or_else(|| {
        Diagnostic::new("RUV1401", "Tailwind CSS CLI was not found")
            .explain("A CSS file imports `tailwindcss`, but Ruvyxa could not find `@tailwindcss/cli` in node_modules.")
            .at_file(input)
            .suggest("Install Tailwind support with `pnpm add tailwindcss && pnpm add -D @tailwindcss/cli`.")
    })?;
    let input_arg = input.strip_prefix(root).unwrap_or(input);

    let output = Command::new(tailwind)
        .current_dir(root)
        .arg("-i")
        .arg(input_arg)
        .arg("--minify")
        .output()
        .map_err(|source| RuvyxaError::Io {
            message: "Failed to run Tailwind CSS CLI".to_string(),
            source,
        })?;

    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map_err(|error| RuvyxaError::Message(error.to_string()));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Diagnostic::new("RUV1400", "Tailwind CSS compilation failed")
        .explain(stderr.trim())
        .at_file(input)
        .suggest("Check Tailwind directives, content sources, and installed Tailwind package versions.")
        .into())
}

fn find_tailwind_cli(root: &Path) -> Option<PathBuf> {
    let binary = if cfg!(windows) {
        "tailwindcss.cmd"
    } else {
        "tailwindcss"
    };

    [
        root.join("node_modules/.bin").join(binary),
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join("node_modules/.bin").join(binary))
            .unwrap_or_default(),
    ]
    .into_iter()
    .find(|path| path.is_file())
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
    let src = if config.watch {
        format!(
            "/__ruvyxa/client?path={}",
            url_encode_component(request_path)
        )
    } else {
        prebuilt_client_src(config, &route.path).unwrap_or_else(|| {
            format!(
                "/__ruvyxa/client?path={}",
                url_encode_component(request_path)
            )
        })
    };

    format!(
        r#"<script>globalThis.__RUVYXA_ROUTE_PARAMS__ = {params_json};globalThis.__RUVYXA_REQUEST_PATH__ = {request_path_json};</script><script type="module" src="{src}"></script>"#
    )
}

fn prebuilt_client_src(config: &ServerConfig, route_path: &str) -> Option<String> {
    let source = fs::read_to_string(config.client_dir.join("manifest.json")).ok()?;
    let manifest: ClientAssetManifest = serde_json::from_str(&source).ok()?;
    manifest
        .routes
        .into_iter()
        .find(|route| route.path == route_path)
        .map(|route| route.src)
}

fn safe_json_for_script(json: &str) -> String {
    json.replace("</", "<\\/")
}

fn hmr_client_script() -> &'static str {
    r#"<script>
(() => {
  const protocol = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${protocol}://${location.host}/__ruvyxa/hmr`);
  const refreshCss = async () => {
    const html = await fetch(location.href, { headers: { accept: "text/html" } }).then((res) => res.text());
    const next = new DOMParser().parseFromString(html, "text/html").querySelector("style[data-ruvyxa-css]");
    const current = document.querySelector("style[data-ruvyxa-css]");
    if (next && current) current.replaceWith(next);
    else location.reload();
  };
  const refreshComponent = () => {
    const script = document.createElement("script");
    script.type = "module";
    script.src = `/__ruvyxa/client?path=${encodeURIComponent(location.pathname)}&t=${Date.now()}`;
    script.onerror = () => location.reload();
    document.body.appendChild(script);
  };
  socket.addEventListener("message", (event) => {
    const update = JSON.parse(event.data);
    if (update.type === "css-update") refreshCss().catch(() => location.reload());
    else if (update.type === "component-update") refreshComponent();
    else location.reload();
  });
})();
</script>"#
}

fn classify_hmr_event(paths: &[PathBuf]) -> &'static str {
    if paths.is_empty() {
        return "full-reload";
    }

    if paths.iter().all(|path| extension_is(path, "css")) {
        return "css-update";
    }

    let has_component = paths.iter().any(|path| {
        ["tsx", "jsx", "ts", "js"]
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

fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

#[derive(Default)]
struct ActionRateLimiter {
    hits: HashMap<String, Vec<Instant>>,
}

impl ActionRateLimiter {
    fn allow(&mut self, key: &str) -> bool {
        let now = Instant::now();
        let hits = self.hits.entry(key.to_string()).or_default();
        hits.retain(|hit| now.duration_since(*hit) <= ACTION_RATE_LIMIT_WINDOW);

        if hits.len() >= ACTION_RATE_LIMIT_MAX {
            return false;
        }

        hits.push(now);
        true
    }
}

fn validate_action_request(headers: &HeaderMap, body_len: usize) -> Option<Response> {
    if body_len > MAX_ACTION_BODY_BYTES {
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

    if action_origin_is_cross_site(headers) {
        return Some(
            (StatusCode::FORBIDDEN, "Cross-origin action request blocked").into_response(),
        );
    }

    if action_fetch_site_is_cross_site(headers) {
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
    let Some(origin_host) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .and_then(|value| value.split('/').next())
    else {
        return true;
    };

    !origin_host.eq_ignore_ascii_case(host)
}

fn action_fetch_site_is_cross_site(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
}

fn action_rate_limit_key(headers: &HeaderMap, query: &ActionQuery) -> String {
    let client = headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("local");

    format!("{client}:{}:{}", query.path, query.name)
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

fn error_page(message: &str) -> String {
    format!(
        "<!doctype html><html><body><main><h1>Ruvyxa Error</h1><pre>{}</pre></main></body></html>",
        escape_html(message)
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
    fn matches_static_and_dynamic_routes() {
        assert!(route_params("/", "/").is_some());
        assert!(route_params("/blog/:slug", "/blog/hello").is_some());
        assert!(route_params("/docs/*slug", "/docs/a/b/c").is_some());
        assert!(route_params("/shop/:category?", "/shop").is_some());
        assert!(route_params("/shop/:category?", "/shop/books").is_some());
        assert!(route_params("/blog/:slug", "/blog").is_none());
    }

    #[test]
    fn extracts_route_params() {
        assert_eq!(
            route_params("/blog/:slug", "/blog/hello")
                .unwrap()
                .get("slug"),
            Some(&"hello".to_string())
        );
        assert_eq!(
            route_params("/docs/*slug", "/docs/a/b/c")
                .unwrap()
                .get("slug"),
            Some(&"a/b/c".to_string())
        );
        assert!(route_params("/shop/:category?", "/shop")
            .unwrap()
            .is_empty());
        assert_eq!(
            route_params("/shop/:category?", "/shop/books")
                .unwrap()
                .get("category"),
            Some(&"books".to_string())
        );
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
    fn builds_runtime_trace_for_matched_routes() {
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
        let trace = runtime_trace(&config, "/blog/hello").unwrap();

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
        assert!(validate_action_request(&headers, 128).is_none());
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
    fn blocks_cross_site_fetch_metadata_for_actions() {
        let mut headers = HeaderMap::new();
        headers.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));

        assert!(action_fetch_site_is_cross_site(&headers));
        assert!(validate_action_request(&headers, 128).is_some());
    }

    #[test]
    fn rate_limits_action_keys() {
        let mut limiter = ActionRateLimiter::default();

        for _ in 0..ACTION_RATE_LIMIT_MAX {
            assert!(limiter.allow("local:/todos:createTodo"));
        }

        assert!(!limiter.allow("local:/todos:createTodo"));
        assert!(limiter.allow("local:/other:createTodo"));
    }

    #[test]
    fn reads_prebuilt_client_src_from_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path().join(".ruvyxa/client");
        std::fs::create_dir_all(&client_dir).unwrap();
        std::fs::write(
            client_dir.join("manifest.json"),
            r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.js"}]}"#,
        )
        .unwrap();

        let config = ServerConfig::production(temp.path(), "localhost", 3000);

        assert_eq!(
            prebuilt_client_src(&config, "/"),
            Some("/__ruvyxa/client/home.js".to_string())
        );
    }
}
