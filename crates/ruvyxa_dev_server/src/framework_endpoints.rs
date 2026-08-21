//! The handlers behind the reserved `/__ruvyxa/*` paths.
//!
//! These are the framework's own endpoints rather than any route the project
//! wrote: the client manifest and bundle, the Flight payload a soft navigation
//! fetches, the hydration loader, on-demand image optimization, the server
//! action endpoint, the DevTools dashboard and its data feed, and the edit
//! traces. `tests/fixtures/framework-endpoint-conformance.json` is the list
//! they answer, and `lib.rs` replays it against the router that mounts them.
//!
//! They sat in the crate root interleaved with server assembly and the file
//! watcher, which is why the fixture had a list and the file did not. Grouping
//! them makes the module and the contract describe the same surface.
//!
//! Route matching for project pages and API routes is not here: `handle_request`
//! in `lib.rs` owns that path and reaches it only by having missed these.

use std::net::SocketAddr;
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ruvyxa_graph::{RouteEntry, RouteKind, RouteParams};
use serde::{Deserialize, Serialize};
use tracing::error;

#[cfg(test)]
use crate::RuntimeCache;
use crate::action_security::{
    action_client_ip, action_rate_limit_key, action_reference_id, hmr_origin_is_cross_site,
    validate_action_payload, validate_action_request,
};
use crate::devtools::dashboard_html;
use crate::dynamic_image::{self, DynamicImageError};
use crate::html_document::{hydration_loader_source, public_internal_error, url_encode_component};
use crate::plugin_bridge::canonical_request_path;
use crate::render_pipeline::{
    render_client_bundle_pooled, render_server_action_pooled, runtime_trace_cached,
};
use crate::response::{json_response, shared_text_body, with_security_headers};
use crate::worker_pool::RenderFlightRequest;
use crate::{AppState, ServerConfig, render_pipeline, static_assets, trace};

#[derive(Debug, Deserialize)]
pub(crate) struct ClientBundleQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FlightQuery {
    path: String,
    artifact: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DynamicImageQuery {
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
pub(crate) struct TraceQuery {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceAck {
    trace_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditTraceResponse {
    contract: &'static str,
    schema_version: u32,
    traces: Vec<trace::EditTrace>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeTrace {
    pub(crate) path: String,
    pub(crate) matched: bool,
    pub(crate) route: Option<RouteEntry>,
    pub(crate) params: RouteParams,
    pub(crate) runtime: &'static str,
    pub(crate) assets: TraceAssets,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraceAssets {
    pub(crate) public_dir: String,
    pub(crate) app_dir: String,
}

/// Serve the client route table so the browser router can match and load
/// routes without a document load, the same as a production build.
///
/// Production publishes this file to `/__ruvyxa/client/route-manifest.json`;
/// the dev server has no such static file, so it synthesizes an equivalent from
/// the live route manifest. Each page route points at the on-demand bundle
/// endpoint keyed by its pattern — the generated bundle registers itself under
/// that pattern, which is what `@ruvyxa/react`'s router looks up.
pub(crate) async fn client_manifest(State(state): State<Arc<AppState>>) -> Response {
    let routes = match state.runtime_cache.router(&state.config).await {
        Ok((manifest, _)) => {
            let eligible = manifest
                .routes
                .iter()
                .filter(|route| route.kind == RouteKind::Page && route.render.ships_client_bundle())
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
pub(crate) async fn flight_endpoint(
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

pub(crate) async fn client_bundle(
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

pub(crate) async fn hydration_loader() -> Response {
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

pub(crate) async fn dynamic_image_endpoint(
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

pub(crate) async fn action_endpoint(
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
        // A poisoned lock is recovered rather than refused. `ActionReplayGuard`
        // records a nonce in `seen` before `order`, so a panic under this lock
        // cannot leave state that accepts a replay — and `std::sync` poisoning
        // never clears, so failing on it took every versioned action out for the
        // life of the process. This matches how `collab.rs` treats its own room
        // lock.
        let replay = state
            .action_replays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .consume(
                &headers,
                provided_reference,
                action_client_ip(peer, &headers, &state.config),
            );
        if let Err(rejection) = replay {
            return with_security_headers(
                (rejection.status(), rejection.message()).into_response(),
            );
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

pub(crate) async fn devtools_dashboard(State(state): State<Arc<AppState>>) -> Response {
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

pub(crate) async fn devtools_data(
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

pub(crate) async fn trace_endpoint(
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

pub(crate) async fn trace_ack_endpoint(
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
