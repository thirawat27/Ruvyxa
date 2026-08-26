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
    apply_revalidations, client_artifact_version, render_client_bundle_pooled,
    render_server_action_pooled, runtime_trace_cached, stamp_client_artifact,
};
use crate::response::{json_response, with_security_headers};
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

/// Which shared browser module `/__ruvyxa/client/vendor` should answer with.
#[derive(Debug, Deserialize)]
pub(crate) struct ClientVendorQuery {
    name: String,
}

/// The URL a soft navigation into a server-components route asks for.
#[derive(Debug, Deserialize)]
pub(crate) struct RscPayloadQuery {
    path: String,
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
/// A build publishes this file to `client/route-manifest.json`, and that copy is
/// served verbatim when it exists. It has to be: its entries point at the
/// content-addressed bundles the build emitted, which link React out of the
/// shared chunk the served document already loaded. The synthesized table below
/// points every route at `/__ruvyxa/client?path=…` instead — a bundle compiled
/// on demand, carrying its own React — so a soft navigation rendered a
/// component from one React copy into a root owned by another, and every hook
/// in it threw. The router caught the failure and fell back to a document load,
/// which is why the pages still worked and the client router quietly did
/// nothing in production.
///
/// `ruvyxa dev` has no such file, so it keeps the synthesized table.
pub(crate) async fn client_manifest(State(state): State<Arc<AppState>>) -> Response {
    if let Some(prebuilt) = prebuilt_route_manifest(&state.config).await {
        let mut response = prebuilt.into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return with_security_headers(response);
    }

    let body = synthesized_route_manifest(&state).await;
    let mut response = body.to_string().into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    // Never cache in the browser: routes appear and disappear as files are
    // added during dev, and the server's own copy is dropped on every watcher
    // event.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    with_security_headers(response)
}

/// The dev route table, rebuilt only when something on disk has changed.
///
/// The work below is proportional to the number of page routes and asks the
/// worker pool for each one's browser bundle, because the hash of that bundle is
/// the `artifactVersion` the client router compares against. The browser fetches
/// this table on every document load, so doing it per request made the endpoint
/// the slowest thing `ruvyxa dev` served — two orders of magnitude above a page
/// render, and seconds on the first request of a session.
async fn synthesized_route_manifest(state: &Arc<AppState>) -> Arc<str> {
    loop {
        let generation = match state.runtime_cache.cached_client_routes().await {
            Ok(body) => return body,
            Err(generation) => generation,
        };
        let body: Arc<str> = Arc::from(build_route_manifest_body(state).await.as_str());
        if let Some(stored) = state
            .runtime_cache
            .store_client_routes(generation, body)
            .await
        {
            return stored;
        }
    }
}

async fn build_route_manifest_body(state: &Arc<AppState>) -> String {
    let Ok((manifest, _)) = state
        .runtime_cache
        .router(&state.config)
        .await
        .inspect_err(|error| {
            error!(%error, "client manifest request failed");
        })
    else {
        return serde_json::json!({ "routes": [] }).to_string();
    };
    let eligible = manifest
        .routes
        .iter()
        .filter(|route| route.kind == RouteKind::Page && route.render.ships_client_bundle())
        .cloned()
        .collect::<Vec<_>>();

    // One route's entry needs its bundle compiled, and the routes are
    // independent, so they run across the pool rather than one after another. In
    // sequence this was the largest stall of a dev session: the first request of
    // the day compiled every page's browser bundle end to end before answering.
    // Bounded by the pool, because more in flight than there are workers only
    // queues on one of them while keeping more compilation output alive.
    let mut pending = tokio::task::JoinSet::new();
    let mut entries = vec![serde_json::Value::Null; eligible.len()];
    let mut next = 0;
    let concurrency = state.worker_pool.size().min(eligible.len().max(1));
    loop {
        while pending.len() < concurrency && next < eligible.len() {
            let state = Arc::clone(state);
            let route = eligible[next].clone();
            let index = next;
            next += 1;
            pending.spawn(async move { (index, route_manifest_entry(&state, &route).await) });
        }
        let Some(joined) = pending.join_next().await else {
            break;
        };
        match joined {
            Ok((index, Some(entry))) => entries[index] = entry,
            Ok((_, None)) => {}
            Err(error) => error!(%error, "client manifest entry task failed"),
        }
    }

    // Order is the manifest's, not the order the compiles finished, so the same
    // project always answers with the same bytes.
    let routes = entries
        .into_iter()
        .filter(|entry| !entry.is_null())
        .collect::<Vec<_>>();
    serde_json::json!({ "routes": routes }).to_string()
}

/// What a route's own source says about how the browser should talk to it.
///
/// Both facts come from one read and one parse because both callers need the
/// read and one of them needed both facts; asking twice re-parsed the module to
/// answer a second question about bytes already in hand.
struct RouteModuleFacts {
    /// `export function flight(context)` — the route produces a payload the
    /// client router can fetch instead of a document.
    flight: bool,
    /// The `'use cache'` module directive.
    uses_cache: bool,
}

/// Read a route's source and answer both questions about it.
///
/// The error is deliberately not folded into `Ok(RouteModuleFacts::default())`.
/// `false` is a legitimate answer — most routes export no `flight` — so a
/// defaulted read made an unreadable file indistinguishable from a route that
/// genuinely has neither, and both callers then reported the wrong thing with
/// nothing logged. `a_route_source_read_is_never_defaulted` holds that.
async fn route_module_facts(file: &std::path::Path) -> std::io::Result<RouteModuleFacts> {
    let source = tokio::fs::read_to_string(file).await?;
    let module = ruvyxa_bundler::ast::parse_module(&source);
    Ok(RouteModuleFacts {
        flight: ruvyxa_bundler::ast::has_named_runtime_export(&source, &module, "flight"),
        uses_cache: ruvyxa_bundler::reference_manifest::has_module_directive(&source, "use cache"),
    })
}

/// One route's line in the dev route table, or `None` when its bundle could not
/// be produced — a route the browser cannot navigate to is left out rather than
/// advertised.
async fn route_manifest_entry(
    state: &Arc<AppState>,
    route: &ruvyxa_graph::RouteEntry,
) -> Option<serde_json::Value> {
    let bundle = render_client_bundle_pooled(state, &route.path).await.ok()?;
    // Hashed unstamped, exactly as `client_bundle` hashes it before appending
    // the stamp, so the version this manifest advertises is the one the browser
    // reads back out of the bundle.
    let artifact = client_artifact_version(&bundle.document.html);
    // A route whose source cannot be read is left out, exactly as one whose
    // bundle could not be produced is. Defaulting to an empty source instead
    // advertised the route with `flight: false` and `cache: false`, so the
    // browser router stopped asking for payloads a route does in fact produce
    // and nothing recorded why.
    let facts = match route_module_facts(&route.file).await {
        Ok(facts) => facts,
        Err(error) => {
            error!(%error, file = %route.file.display(), "route source read failed");
            return None;
        }
    };
    let mut entry = serde_json::json!({
        "path": route.path,
        "src": format!("/__ruvyxa/client?path={}", url_encode_component(&route.path)),
        "artifactVersion": artifact,
        "flight": facts.flight,
        "cache": facts.uses_cache,
    });
    // Held to the same shape the build writes, in `write_client_route_manifest`:
    // present only when true.
    if route.render.server_components {
        entry["serverComponents"] = serde_json::Value::Bool(true);
    }
    Some(entry)
}

/// The route table a build published, or `None` when there is not one.
///
/// Read per request rather than cached: the file is written once by a build and
/// then never again, the router asks for it at most once per document, and a
/// cache here would be a third copy of the same bytes after the OS page cache
/// and the browser's.
async fn prebuilt_route_manifest(config: &ServerConfig) -> Option<String> {
    if config.watch {
        return None;
    }
    tokio::fs::read_to_string(config.client_dir.join("route-manifest.json"))
        .await
        .ok()
}

/// Everything a Flight request must satisfy before any route is looked up, or
/// the response that refuses it.
///
/// Split out as one section rather than piecemeal: these four checks read the
/// request and nothing else, they all answer with a status and a sentence, and
/// keeping them beside the routing and rendering they gate is what pushed
/// `flight_endpoint` past the workspace cognitive-complexity gate. Returns the
/// canonical request path, which is the only thing the rest of the endpoint
/// needs from them.
fn validate_flight_request(
    headers: &HeaderMap,
    query: &FlightQuery,
) -> Result<String, (StatusCode, &'static str)> {
    // A Flight payload is cacheable and shared; private request state must not
    // be able to vary it.
    if headers.contains_key(header::AUTHORIZATION) || headers.contains_key(header::COOKIE) {
        return Err((
            StatusCode::FORBIDDEN,
            "Flight requests must not include private request state",
        ));
    }
    // A header a cross-origin form cannot set, which is what makes this a
    // navigation endpoint rather than a public data endpoint.
    if headers
        .get("x-ruvyxa-flight")
        .and_then(|value| value.to_str().ok())
        != Some("1")
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Flight requests require the Ruvyxa navigation header",
        ));
    }
    if query.artifact.len() != 16
        || !query
            .artifact
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Flight request has an invalid artifact",
        ));
    }
    canonical_request_path(&query.path).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "Flight request has an invalid route",
        )
    })
}

/// Serve the same public Flight contract in dev/start that deployment adapters expose.
pub(crate) async fn flight_endpoint(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FlightQuery>,
    headers: HeaderMap,
) -> Response {
    let request_path = match validate_flight_request(&headers, &query) {
        Ok(path) => path,
        Err(rejection) => return with_security_headers(rejection.into_response()),
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
    // 501 here means "this route exports no `flight`", which an empty source
    // also produces — so defaulting the read reported a missing export as the
    // cause of an I/O failure, told the browser to stop asking, and logged
    // nothing. A read that fails is a server fault and is answered as one.
    let facts = match route_module_facts(&route_match.route.file).await {
        Ok(facts) => facts,
        Err(error) => {
            error!(%error, file = %route_match.route.file.display(), "Flight route source read failed");
            return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    if !facts.flight {
        return with_security_headers(
            (
                StatusCode::NOT_IMPLEMENTED,
                "This route does not expose a Flight payload",
            )
                .into_response(),
        );
    }
    let bundle = match render_client_bundle_pooled(&state, &route_match.route.path).await {
        Ok(bundle) => bundle,
        Err(error) => {
            error!(%error, path = %request_path, "Flight client artifact failed");
            return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    let current_artifact = client_artifact_version(&bundle.document.html);
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
            artifact_version: &current_artifact,
        })
        .await;
    flight_render_response(response, &request_path)
}

/// Turn the worker's answer into the response the browser router reads.
///
/// The three ways a render ends — a payload, a render the worker refused, and a
/// worker that failed — are one decision, and separating them from the request
/// checks and the route lookup above is what keeps `flight_endpoint` inside the
/// workspace cognitive-complexity gate. Every failure is reported to the log and
/// answered as a bare 500: the detail belongs to the operator, not the page.
fn flight_render_response(
    response: ruvyxa_diagnostics::Result<crate::worker_protocol::WorkerResponse>,
    request_path: &str,
) -> Response {
    let payload = match response {
        Ok(response) if response.ok => response.flight,
        Ok(response) => {
            error!(code = ?response.code, message = ?response.message, path = %request_path, "Flight render failed");
            return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
        Err(error) => {
            error!(%error, path = %request_path, "Flight worker failed");
            return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    // `ok` without a payload is the worker contradicting itself; there is
    // nothing to send and nothing the browser could do with a 200.
    let Some(payload) = payload else {
        error!(path = %request_path, "Flight render reported success with no payload");
        return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
    };
    let mut response = payload.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/vnd.ruvyxa.flight+json; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("x-ruvyxa-flight"));
    with_security_headers(response)
}

/// Header that keeps `/__ruvyxa/rsc` out of reach of a cross-origin page.
///
/// Four spellings, not two, and this comment claimed two until the count was
/// taken: here, in `createHandler` (`serverless-handler.mjs`, the host every
/// deployed build runs), and in both browser halves —
/// `rsc-client-runtime.mjs` and `@ruvyxa/react`'s router. They cannot share a
/// module across the language boundary or into a function bundle, so
/// `requiredHeaders` on `/__ruvyxa/rsc` in
/// `tests/fixtures/framework-endpoint-conformance.json` is the table both
/// request hosts replay instead.
const RSC_REQUEST_HEADER: &str = "x-ruvyxa-rsc";

/// One server-components route, resolved and owned.
///
/// Owned rather than borrowed because the router snapshot it came from is a
/// temporary: the two endpoints below hold this across an await on the worker,
/// which a `RouteMatch` borrowing the manifest could not survive.
struct ServerComponentsRoute {
    file: std::path::PathBuf,
    /// The canonical request path, after normalisation.
    path: String,
    /// The route pattern, which is what keys every registry the browser holds.
    route_path: String,
    params: RouteParams,
}

/// Resolve `/__ruvyxa/rsc`'s route, or the response explaining why it cannot.
///
/// The same-origin header check lives here rather than in each endpoint so the
/// `GET` and the `POST` cannot come to differ about it. A cross-origin page
/// cannot set a custom header without a preflight, and nothing here answers one,
/// so a third-party site cannot reach either verb even with credentials
/// attached.
///
/// The refusal is boxed. An `axum` `Response` is far larger than the resolved
/// route, so an unboxed `Err` would make every caller of this function carry a
/// response-sized value on the path that succeeds — which is every request that
/// works.
async fn resolve_server_components_route(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
) -> Result<ServerComponentsRoute, Box<Response>> {
    if headers
        .get(RSC_REQUEST_HEADER)
        .and_then(|value| value.to_str().ok())
        != Some("1")
    {
        return Err(Box::new(with_security_headers(
            (
                StatusCode::BAD_REQUEST,
                "Server-components requests require the Ruvyxa navigation header",
            )
                .into_response(),
        )));
    }
    let Ok(request_path) = canonical_request_path(path) else {
        return Err(Box::new(with_security_headers(
            (
                StatusCode::BAD_REQUEST,
                "Server-components request has an invalid route",
            )
                .into_response(),
        )));
    };
    let (manifest, router) = match state.runtime_cache.router(&state.config).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            error!(%error, "Server-components route snapshot failed");
            return Err(Box::new(with_security_headers(
                StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            )));
        }
    };
    let Some(route_match) = router.find(&manifest, &request_path) else {
        return Err(Box::new(with_security_headers(
            StatusCode::NOT_FOUND.into_response(),
        )));
    };
    // A route that never opted in has no payload to give and no server function
    // to run, and answering either would mean going through a pipeline it was
    // not written for.
    if route_match.route.kind != RouteKind::Page || !route_match.route.render.server_components {
        return Err(Box::new(with_security_headers(
            StatusCode::NOT_FOUND.into_response(),
        )));
    }
    Ok(ServerComponentsRoute {
        file: route_match.route.file.clone(),
        path: request_path,
        route_path: route_match.route.path.clone(),
        params: route_match.params.clone(),
    })
}

/// The visitor's headers, in the shape the worker protocol carries them.
fn forwarded_header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
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

/// Wrap a Flight payload in the response both `/__ruvyxa/rsc` verbs return.
///
/// A render, not a cacheable document: it may have read this visitor's cookies,
/// so no shared cache may hold it. `Vary` names the header that decides whether
/// the path answers at all.
fn flight_payload_response(payload: String) -> Response {
    let mut response = payload.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/x-component; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static(RSC_REQUEST_HEADER));
    response
}

/// Serve one server-components route's Flight payload for a soft navigation.
///
/// Deliberately *not* the same contract as [`flight_endpoint`] above, which
/// serves Ruvyxa's own public per-route JSON and therefore refuses a request
/// carrying credentials. This payload is a render: a server component may read
/// `cookies()` and `headers()` exactly as it does on a full document request,
/// so the visitor's headers are forwarded and the response is marked private
/// and uncacheable.
///
/// The custom header is what keeps it same-origin. A cross-origin page cannot
/// set it without a preflight, and nothing here answers one — so a third-party
/// site cannot read a visitor's rendered page even with credentials attached.
pub(crate) async fn rsc_payload_endpoint(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RscPayloadQuery>,
    headers: HeaderMap,
) -> Response {
    let route_match = match resolve_server_components_route(&state, &headers, &query.path).await {
        Ok(resolved) => resolved,
        Err(response) => return *response,
    };
    let request_path = route_match.path.clone();
    let header_pairs = forwarded_header_pairs(&headers);

    let response = state
        .worker_pool
        .render_rsc_payload(crate::worker_pool::RenderSsrRequest {
            project_root: &state.config.root,
            app_dir: &state.config.app_dir,
            page_file: &route_match.file,
            request_path: &request_path,
            request_target: &request_path,
            route_path: &route_match.route_path,
            params: &route_match.params,
            headers: &header_pairs,
            method: "GET",
            server_components: true,
            // A payload render is a read. The no-JavaScript form post is the
            // only page request that carries one, and it never comes here.
            form_action: None,
        })
        .await;

    match response {
        Ok(response) if response.ok => {
            let Some(payload) = response.rsc_payload else {
                return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
            };
            with_security_headers(flight_payload_response(payload))
        }
        Ok(response) => {
            error!(code = ?response.code, message = ?response.message, path = %request_path, "Server-components payload render failed");
            with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(error) => {
            error!(%error, path = %request_path, "Server-components payload worker failed");
            with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// Header naming the server function a `POST` to `/__ruvyxa/rsc` should call.
///
/// Spelled once here and once in `rsc-client-runtime.mjs`, which is the browser
/// half of the same request. There is no third reader.
const SERVER_ACTION_HEADER: &str = "x-ruvyxa-action";

/// Largest server-function call body this host will read into memory.
///
/// A call is arguments, not an upload: React encodes them as text unless one is
/// a file, and a file large enough to matter belongs in a route handler that can
/// stream it. The bound exists because the body is buffered before the worker
/// sees it, so without one a single request could size the process.
const MAX_SERVER_ACTION_BODY: usize = 4 * 1024 * 1024;

/// Run one of a server-components route's server functions.
///
/// The same path that serves a route's payload, because it is the same question
/// asked twice: `GET` renders the route, `POST` runs one of the functions it
/// exposes and returns what that function produced. A second path would mean a
/// second reserved route and a second place the same-origin header is checked.
///
/// The reply is itself a Flight payload, so a server function may return an
/// element tree — including client components — and not only data.
pub(crate) async fn rsc_action_endpoint(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RscPayloadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(reference) = headers
        .get(SERVER_ACTION_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    else {
        return with_security_headers(
            (
                StatusCode::BAD_REQUEST,
                "Server-function calls must name a reference",
            )
                .into_response(),
        );
    };
    if body.len() > MAX_SERVER_ACTION_BODY {
        return with_security_headers(
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "Server-function call too large",
            )
                .into_response(),
        );
    }
    let route_match = match resolve_server_components_route(&state, &headers, &query.path).await {
        Ok(resolved) => resolved,
        Err(response) => return *response,
    };

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("text/plain;charset=UTF-8")
        .to_string();
    let header_pairs = forwarded_header_pairs(&headers);

    let response = state
        .worker_pool
        .render_rsc_action(
            crate::worker_pool::RenderSsrRequest {
                project_root: &state.config.root,
                app_dir: &state.config.app_dir,
                page_file: &route_match.file,
                request_path: &route_match.path,
                request_target: &route_match.path,
                route_path: &route_match.route_path,
                params: &route_match.params,
                headers: &header_pairs,
                method: "POST",
                server_components: true,
                form_action: None,
            },
            reference,
            &content_type,
            &body,
        )
        .await;

    match response {
        Ok(worker) if worker.ok => {
            let Some(payload) = worker.rsc_payload else {
                return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
            };
            apply_revalidations(&state, worker.revalidate).await;
            with_security_headers(flight_payload_response(payload))
        }
        Ok(worker) => {
            error!(code = ?worker.code, message = ?worker.message, reference = %reference, "server function failed");
            with_security_headers(
                (StatusCode::INTERNAL_SERVER_ERROR, "Server function failed").into_response(),
            )
        }
        Err(error) => {
            error!(%error, reference = %reference, "server function worker failed");
            with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// Serve one shared browser module for bundles this host compiles on demand.
///
/// A build gives every route a shared chunk, so a page holds one React however
/// many route bundles it loads. A bundle compiled per request has no such
/// analysis behind it and used to inline its own copy — so a soft navigation
/// rendered a component from one React into a root owned by another, every hook
/// in it threw, and the router fell back to a document load. Every such bundle
/// imports these modules by URL instead; the names are decided by
/// `CLIENT_VENDOR_MODULES` in `packages/ruvyxa/runtime/compiler.mjs`, and an
/// unknown one is rejected there rather than compiled.
pub(crate) async fn client_vendor(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ClientVendorQuery>,
) -> Response {
    let response = state
        .worker_pool
        .render_client_vendor(&state.config.root, &query.name)
        .await;
    match response {
        Ok(response) if response.ok => {
            let Some(script) = response.script else {
                return with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response());
            };
            let mut response = script.into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript; charset=utf-8"),
            );
            // Never cached: the module is recompiled when its package changes,
            // and a stale copy would be a second React on the page — the exact
            // failure this endpoint exists to prevent.
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            with_security_headers(response)
        }
        Ok(response) => {
            error!(code = ?response.code, message = ?response.message, name = %query.name, "shared browser module failed");
            with_security_headers(
                (StatusCode::NOT_FOUND, "Unknown shared browser module").into_response(),
            )
        }
        Err(error) => {
            error!(%error, name = %query.name, "shared browser module worker failed");
            with_security_headers(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

pub(crate) async fn client_bundle(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ClientBundleQuery>,
) -> Response {
    let response = match render_client_bundle_pooled(&state, &query.path).await {
        Ok(bundle) => {
            if state.config.watch {
                state
                    .devtools
                    .record_bundle(&query.path, bundle.document.html.len());
            }
            // Stamped here rather than cached stamped: the version is a hash of
            // the unstamped script, and the route manifest and the Flight
            // endpoint both have to hash the same text this one does.
            let stamped = stamp_client_artifact(&bundle.document.html, &bundle.route_path);
            let mut response = stamped.into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/javascript; charset=utf-8"),
            );
            response
        }
        Err(error) => {
            error!(%error, path = %query.path, "client bundle request failed");
            let message = public_internal_error(&state.config, &error);
            // Tell the page, not just the terminal.
            //
            // A browser reports a script that answered 500 as a load failure
            // and nothing else, so the document around it — already
            // server-rendered and already 200 — sat there looking finished
            // while nothing on it worked. The overlay listens on the HMR
            // channel, and this is the only place that knows the bundle failed.
            if state.config.watch && state.config.error_overlay {
                let _ = state.reload_tx.send(crate::watcher::hmr_issue_payload(
                    "RUV1300",
                    &message,
                    &query.path,
                ));
            }
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
    if query.quality.is_some_and(|quality| {
        !(dynamic_image::MIN_QUALITY..=dynamic_image::MAX_QUALITY).contains(&quality)
    }) {
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

    /// A route source that cannot be read must not read as a route without a
    /// `flight` export.
    ///
    /// `has_named_runtime_export` answers `false` for an empty source, and that
    /// is a legitimate answer — most routes export no `flight`. So
    /// `read_to_string(..).unwrap_or_default()` on a route file collapsed two
    /// different facts into one: the Flight endpoint answered `501 this route
    /// does not expose a Flight payload` for an I/O failure and logged nothing,
    /// and the dev route table advertised `flight: false`, which tells the
    /// browser router to stop asking for payloads the route does produce.
    ///
    /// Asserted over the source because behaviour cannot reach it: both call
    /// sites need a live `AppState`, and the failure is a file becoming
    /// unreadable between route discovery and the request — a race no unit test
    /// can stage through the public path. The first assertion below is what
    /// makes the rest necessary, so it is asserted rather than assumed.
    ///
    /// The scan stops at this test module and looks ahead rather than matching
    /// one line: the shape that shipped was spelled across three lines, and a
    /// scan that reads its own assertions finds itself.
    #[test]
    fn a_route_source_read_is_never_defaulted() {
        let empty = "";
        let module = ruvyxa_bundler::ast::parse_module(empty);
        assert!(
            !ruvyxa_bundler::ast::has_named_runtime_export(empty, &module, "flight"),
            "an empty source reports no flight export, so a defaulted read is \
             indistinguishable from a route that genuinely has none"
        );

        let whole = include_str!("framework_endpoints.rs");
        // Split on the test module specifically. This file carries an earlier
        // `#[cfg(test)]` on a single item, and stopping there left the scan
        // covering the first few lines and finding nothing to check.
        let production = whole
            .split_once("\n#[cfg(test)]\nmod tests {")
            .map_or(whole, |(before, _)| before);
        let lines: Vec<&str> = production.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains("read_to_string") {
                continue;
            }
            // The call and its terminator, however the formatter broke them up.
            let statement = lines[index..lines.len().min(index + 4)].join(" ");
            assert!(
                !statement.contains("unwrap_or_default"),
                "framework_endpoints.rs:{} defaults a source read; an unreadable \
                 route file would be reported as a missing export again",
                index + 1
            );
        }
        assert!(
            production.matches("read_to_string").count() >= 2,
            "this guard covers the route-source reads; it stopped finding them"
        );
    }

    /// The `/__ruvyxa/rsc` gate is one rule with two implementations.
    ///
    /// This host spells the header names as the constants above; `createHandler`
    /// in `packages/ruvyxa/runtime/serverless-handler.mjs` spells them again for
    /// the deployed half, and `@ruvyxa/react`'s router and
    /// `rsc-client-runtime.mjs` are the browser that sends them. Nothing held
    /// the set together, and the endpoint contract could not express it: its
    /// probes sent no headers, so the 400 a missing header produces satisfied
    /// "the endpoint is dispatched" and a host that dropped the check would have
    /// stayed green while answering a cross-origin server-function call. The
    /// contract carries `requiredHeaders` now; this is the native replay of it,
    /// and `framework-endpoints.test.mjs` is the serverless one.
    #[test]
    fn the_rsc_gate_headers_match_the_shared_endpoint_contract() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/framework-endpoint-conformance.json"
        ))
        .expect("the framework endpoint contract must be valid JSON");

        let endpoint = contract["endpoints"]
            .as_array()
            .expect("endpoints must be an array")
            .iter()
            .find(|endpoint| endpoint["path"] == "/__ruvyxa/rsc")
            .expect("the contract must describe /__ruvyxa/rsc");

        // Collected across every probe: `GET` requires the navigation header
        // alone and `POST` requires the reference header beside it, so asserting
        // the union keeps a probe that loses its headers from silently reducing
        // what this test covers.
        let mut required: std::collections::BTreeMap<&str, &str> =
            std::collections::BTreeMap::new();
        for probe in endpoint["probes"]
            .as_array()
            .expect("/__ruvyxa/rsc answers more than one verb, so it lists probes")
        {
            for (name, value) in probe["requiredHeaders"]
                .as_object()
                .expect("every /__ruvyxa/rsc probe must declare the headers it needs")
            {
                required.insert(
                    name.as_str(),
                    value.as_str().expect("a required header value is a string"),
                );
            }
        }

        assert_eq!(
            required.get(RSC_REQUEST_HEADER).copied(),
            Some("1"),
            "{RSC_REQUEST_HEADER} is this host's cross-origin gate for /__ruvyxa/rsc; \
             the contract must require it so the serverless host is held to it too"
        );
        assert!(
            required.contains_key(SERVER_ACTION_HEADER),
            "{SERVER_ACTION_HEADER} names the server function a POST runs; \
             the contract must require it"
        );
        assert_eq!(
            required.len(),
            2,
            "the contract requires a header on /__ruvyxa/rsc that this host does not check: \
             {required:?}"
        );
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
