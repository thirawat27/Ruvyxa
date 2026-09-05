//! Page, API, action, and client-bundle rendering: strategy dispatch
//! (SSR/SSG/ISR/CSR/PPR), worker-pool render paths, ISR revalidation, and the
//! Node/Bun render-process fallback used by `render_request`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use ruvyxa_bundler::JsxRuntime;
use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use ruvyxa_graph::{
    DiscoverOptions, RenderStrategy, RouteEntry, RouteKind, RouteManifest, RouteParams,
    discover_routes, document_cache_control, document_has_validator,
};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::document_stream::StreamedDocument;
use crate::html_document::{
    bootstrap_data_block, client_hydration_script, compose_localized_document, error_page,
    hmr_client_script,
};
use crate::plugin_head::render_plugin_head;
use crate::render_cache::{CachedDocument, ForcedRevalidationClaim, RenderCache};
use crate::router::RadixRouter;
use crate::static_assets::{
    contained_public_asset, is_safe_relative_path, is_static_asset_request, public_asset_links,
    request_matches_etag, serve_client_file, serve_public_file, weak_content_etag,
};
use crate::worker_pool::{PostedForm, RenderActionRequest, RenderApiRequest, WorkerApiResponse};
use crate::worker_protocol::{self, WorkerRequest, WorkerResponse};
use crate::{
    AppState, RuntimeCache, RuntimeTrace, ServerConfig, TraceAssets, cached_html_response,
    html_response, project_env, streamed_html_response, uncacheable, with_security_headers,
};
use crate::{render_cache, style::collect_styles};
use futures_util::StreamExt;
use tracing::warn;

/// Send the caching contract this route's strategy names.
///
/// The deployed function has always done this — `documentCacheControl` in the
/// serverless handler — while the Axum host sent a rendered document with **no**
/// `cache-control` at all, and a `200` with no explicit freshness is exactly the
/// response RFC 9111 lets a browser or an intermediary cache on a heuristic. So
/// the same page carried opposite caching rules depending on which host answered
/// it, and the host that got it wrong is the one `ruvyxa start` runs.
///
/// Not applied to a streamed document or to a response marked `uncacheable`:
/// both already say `no-store`, which is stricter than anything the table names.
///
/// `request_scoped` outranks the table. `document_cache_control` describes the
/// *route*; the flag describes *this response*, and a render that read a cookie,
/// a header, or draft mode produced one visitor's document. Offering that to a
/// shared cache for the strategy's window is how one visitor's page reaches
/// everybody else — the same reason the render cache refuses to hold it, applied
/// to the other cache the response can reach.
///
/// The strategies that can be stored are the ones this matters for: `Ssr` and
/// `Ppr` already say `no-store`. Today no request-scoped document reaches this
/// function under any other strategy, because only `render_page_pooled` renders
/// inside a request context and only `Ssr` routes it. The guard is written down
/// anyway, so this host and `handlePage` in `serverless-handler.mjs` state the
/// same rule rather than one of them relying on a dispatch table staying as it
/// is.
fn insert_document_cache_control(
    response: &mut Response,
    route: &RouteEntry,
    request_scoped: bool,
) {
    let value = if request_scoped {
        // The spelling `document_cache_control` itself uses for `no-store`, so
        // the two hosts emit the same bytes for the same decision.
        "no-store".to_string()
    } else {
        document_cache_control(route.render.strategy, route.render.revalidate)
    };
    let header = HeaderValue::from_str(&value)
        .expect("document cache-control is built from ASCII literals and a number");
    response.headers_mut().insert(header::CACHE_CONTROL, header);
}

/// Answer a revalidation of a stored document with the version it already holds.
///
/// Carries the validator and the strategy's own lifetime, which is what a cache
/// needs to know when to ask again, and `Vary` for the same reason the `200`
/// does: the document is served identity-encoded or compressed depending on that
/// header. Nothing that describes a body, because it is not sending one.
///
/// `request_scoped` travels with it so the `304` and the `200` it answers for
/// cannot disagree about who may reuse this document.
fn not_modified_document(etag: &str, route: &RouteEntry, request_scoped: bool) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    insert_document_cache_control(&mut response, route, request_scoped);
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    response
}

fn worker_request_headers(headers: &HeaderMap) -> Vec<(String, String)> {
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

fn register_server_hmr_inputs(
    state: &AppState,
    route_path: &str,
    inputs_version: Option<&str>,
    inputs: Option<&[PathBuf]>,
) {
    if state.config.watch
        && let Some(inputs) = inputs
    {
        if let Some(version) = inputs_version {
            state
                .hmr_tracker
                .register_versioned_route(route_path, version, inputs);
        } else {
            state.hmr_tracker.register_route(route_path, inputs);
        }
    }
}

fn register_client_hmr_inputs(
    state: &AppState,
    route_path: &str,
    inputs_version: Option<&str>,
    inputs: Option<&[PathBuf]>,
) {
    if state.config.watch
        && let Some(inputs) = inputs
    {
        if let Some(version) = inputs_version {
            state
                .hmr_tracker
                .register_versioned_client_route(route_path, version, inputs);
        } else {
            state.hmr_tracker.register_client_route(route_path, inputs);
        }
    }
}

fn register_action_hmr_inputs(
    state: &AppState,
    route_path: &str,
    inputs_version: Option<&str>,
    inputs: Option<&[PathBuf]>,
) {
    if state.config.watch
        && let (Some(version), Some(inputs)) = (inputs_version, inputs)
    {
        state
            .hmr_tracker
            .register_versioned_action_route(route_path, version, inputs);
    }
}

/// Project-wide state for the synchronous render path, built once and reused.
///
/// Rendering from a [`ServerConfig`] alone would make every call rediscover the
/// route graph, recompile the radix router, and re-collect every stylesheet
/// from disk. That is invisible for a single render, but the one caller that
/// renders more than one path — the dev/prod parity sweep in `ruvyxa check` —
/// would repeat the whole project scan twice per route.
///
/// Holding that state in a context makes the work per project instead of per
/// request.
pub struct RenderContext {
    manifest: RouteManifest,
    router: RadixRouter,
    /// Collected on the first page render and reused after. API-only sweeps
    /// never pay for it.
    styles: std::sync::OnceLock<String>,
}

impl RenderContext {
    /// Discover the route graph and compile the router for `config`.
    pub fn new(config: &ServerConfig) -> Result<Self> {
        let manifest = discover_routes(
            DiscoverOptions::new(&config.app_dir)
                .with_rendering_defaults(config.default_render_strategy, config.default_revalidate)
                .with_i18n(config.i18n.clone()),
        )?;
        let router = RadixRouter::compile(&manifest);
        Ok(Self {
            manifest,
            router,
            styles: std::sync::OnceLock::new(),
        })
    }

    /// The head fragment carrying this project's CSS.
    ///
    /// Holds the finished tag rather than the rule text, so this context writes
    /// the same thing every other host does: a link to the built stylesheet
    /// when there is one, and the collection inline when there is not.
    fn styles(&self, config: &ServerConfig) -> Result<&str> {
        if let Some(styles) = self.styles.get() {
            return Ok(styles);
        }
        let css = collect_styles(
            &config.root,
            &config.app_dir,
            &config.style_entries,
            config.runtime,
        )?
        .css;
        let tag = crate::style_head_tag(None, &css);
        // A concurrent caller may have won the race; either value is the same
        // collection of the same sources, so whichever lands first stands.
        Ok(self.styles.get_or_init(|| tag))
    }
}

/// Render one request against an already-built [`RenderContext`].
pub fn render_request_with_context(
    config: &ServerConfig,
    context: &RenderContext,
    request_path: &str,
    method: &str,
) -> Result<Response> {
    render_request_cached(config, context, request_path, method)
}

/// True when an asset-shaped request survived static serving only because a
/// dynamic route happened to capture it.
///
/// The client and public directories are consulted before routing, so a
/// request such as `/logo.png` that reaches the router has no file behind it.
/// Letting `/[lang]` answer it returns a 200 HTML document where the browser
/// expects image bytes — a broken image rather than a diagnosable 404. Routes
/// that spell the extension out (`/sitemap.xml`, `/api/data.json`) contain no
/// dynamic segment and keep matching. Deploy adapters apply the same rule in
/// `serverless-handler.mjs`, so `dev`, `start`, and every host agree.
fn is_missing_static_asset(request_path: &str, route_path: &str) -> bool {
    is_static_asset_request(request_path) && route_path.contains('[')
}

/// Drive one asynchronous serving call from a synchronous entry point.
///
/// `render_request_cached` is a synchronous function that now reaches the same
/// asynchronous serving rules a live request does, so something has to bridge
/// the two. Its callers are not all alike, and that is the whole difficulty:
/// `ruvyxa build`'s prerenderer and `ruvyxa bench` call it from an ordinary
/// thread, while `ruvyxa test:parity` calls it from inside a Tokio runtime.
/// `Runtime::block_on` panics when a runtime is already active on the thread,
/// so blocking in place is correct for the first group and fatal for the
/// second.
///
/// A thread of our own is therefore used whenever a runtime is already running:
/// the future moves to a thread that has no runtime context, blocks there, and
/// the result comes back. `std::thread::scope` is what makes that safe to do
/// with a future borrowing the caller's paths. Outside a runtime the extra
/// thread buys nothing, so the common path -- one call per route through the
/// whole prerender -- still blocks in place.
fn block_on_serving<F>(future: F) -> Result<Option<Response>>
where
    F: Future<Output = Result<Option<Response>>> + Send,
{
    fn block_here<F>(future: F) -> Result<Option<Response>>
    where
        F: Future<Output = Result<Option<Response>>>,
    {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|source| RuvyxaError::Io {
                message: "Failed to start a runtime to serve a static file".to_string(),
                source,
            })?
            .block_on(future)
    }

    if tokio::runtime::Handle::try_current().is_err() {
        return block_here(future);
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| block_here(future))
            .join()
            .unwrap_or_else(|_| {
                Err(RuvyxaError::Io {
                    message: "The thread serving a static file panicked".to_string(),
                    source: std::io::Error::other("static file worker panicked"),
                })
            })
    })
}

pub(crate) fn render_request_cached(
    config: &ServerConfig,
    context: &RenderContext,
    request_path: &str,
    method: &str,
) -> Result<Response> {
    // The same functions a live request reaches, not a second copy of them.
    // There used to be a `serve_client_file_sync` and a `serve_public_file_sync`
    // that set no ETag, no `Cache-Control` and no `Accept-Ranges`, answered no
    // conditional or range request, never streamed, and returned `Err` for a
    // missing file. Their only callers were `ruvyxa bench` and the parity
    // command -- so `test:parity`, whose whole job is to prove a route answers
    // the same under both hosts, was driving the copy that answered none of the
    // questions the host answers.
    if let Some(client_response) =
        block_on_serving(serve_client_file(&config.client_dir, request_path, None))?
    {
        return Ok(client_response);
    }

    if let Some(public_response) =
        block_on_serving(serve_public_file(&config.public_dir, request_path, None))?
    {
        return Ok(public_response);
    }

    let route_match = match context.router.find(&context.manifest, request_path) {
        Some(route_match) => route_match,
        None => {
            let headers = HeaderMap::new();
            if let Some(location) = crate::i18n::locale_redirect_path(
                config.i18n.as_ref(),
                &context.manifest,
                &context.router,
                request_path,
                // This path renders from a route table alone — the build's
                // prerenderer and `ruvyxa bench` are its only callers — so
                // there is no request URI here and no query to carry.
                None,
                method,
                &headers,
            ) {
                return Ok(with_security_headers(
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(header::LOCATION, location)],
                    )
                        .into_response(),
                ));
            }
            return Ok(html_response(
                StatusCode::NOT_FOUND,
                error_page(
                    StatusCode::NOT_FOUND,
                    "Route not found",
                    config.watch && config.error_overlay,
                ),
            ));
        }
    };
    if is_missing_static_asset(request_path, &route_match.route.path) {
        return Ok(html_response(
            StatusCode::NOT_FOUND,
            error_page(
                StatusCode::NOT_FOUND,
                "Asset not found",
                config.watch && config.error_overlay,
            ),
        ));
    }

    match route_match.route.kind {
        RouteKind::Page => {
            let styles = context.styles(config)?;
            let html = render_page(
                config,
                route_match.route,
                request_path,
                &route_match.params,
                styles,
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

// --- Worker-pool-based async render functions ---

pub(crate) async fn render_request_pooled(
    state: &AppState,
    request_path: &str,
    request_target: &str,
    method: &str,
    request_headers: &HeaderMap,
    request_body: Option<&[u8]>,
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

    // Generated discovery files, after the project's own: a `public/robots.txt`
    // the application ships still wins.
    if let Some(discovery_dir) = &state.config.discovery_dir
        && let Some(discovery_response) =
            serve_public_file(discovery_dir, request_path, Some(request_headers)).await?
    {
        return Ok(discovery_response);
    }

    let (manifest, router) = state.runtime_cache.router(&state.config).await?;
    let route_match = match router.find(&manifest, request_path) {
        Some(route_match) => route_match,
        None => {
            if let Some(location) = crate::i18n::locale_redirect_path(
                state.config.i18n.as_ref(),
                &manifest,
                &router,
                request_path,
                // `request_path` is the canonical path and carries no query by
                // construction; `request_target` is the value that does. A 307
                // preserves the method and the body and says nothing about the
                // query, so the redirect has to reproduce it explicitly.
                request_target.split_once('?').map(|(_, query)| query),
                method,
                request_headers,
            ) {
                return Ok(with_security_headers(
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(header::LOCATION, location)],
                    )
                        .into_response(),
                ));
            }
            // The application's own not-found page, if it has one.
            //
            // `app/not-found.tsx` is the file every reader coming from another
            // framework writes for this, and it was rendered for nothing: only
            // a `notFound()` call inside a matched route reached it, so an
            // unmatched URL got the framework's own error card — which also
            // links `/ruvyxa.png`, an image most projects do not publish.
            if let Some(html) = render_root_not_found(state, request_path, request_headers).await {
                return Ok(html_response(StatusCode::NOT_FOUND, html));
            }
            return Ok(html_response(
                StatusCode::NOT_FOUND,
                error_page(
                    StatusCode::NOT_FOUND,
                    "Route not found",
                    state.config.watch && state.config.error_overlay,
                ),
            ));
        }
    };
    if is_missing_static_asset(request_path, &route_match.route.path) {
        return Ok(html_response(
            StatusCode::NOT_FOUND,
            error_page(
                StatusCode::NOT_FOUND,
                "Asset not found",
                state.config.watch && state.config.error_overlay,
            ),
        ));
    }

    match route_match.route.kind {
        RouteKind::Page => {
            let styles = state.runtime_cache.style_tag(&state.config).await?;
            let page_headers = worker_request_headers(request_headers);
            let form_action = posted_form(route_match.route, method, request_headers, request_body);
            let page_request = PageRequestContext {
                path: request_path,
                target: request_target,
                headers: &page_headers,
                method,
                form_action,
            };
            // A submitted form is answered by the route's own render, not by its
            // strategy: whatever the strategy would have served is a document
            // produced before this action ran. Every other request goes through
            // the strategy exactly as before.
            if form_action.is_some() {
                return render_page_form_action(
                    state,
                    route_match.route,
                    &page_request,
                    &route_match.params,
                    &styles,
                )
                .await;
            }
            if streams_document(route_match.route) {
                return render_page_streamed(
                    state,
                    route_match.route,
                    &page_request,
                    &route_match.params,
                    &styles,
                )
                .await;
            }
            let page = render_page_by_strategy(
                state,
                route_match.route,
                &page_request,
                &route_match.params,
                &styles,
            )
            .await?;
            let html = page.document;
            // A stored document — ssg, csr, isr — is the same bytes for every
            // reader, so a reader who already holds it can be told so instead of
            // being sent it again. `document_cache_control` asks the browser to
            // revalidate before every reuse, and until now that revalidation had
            // nothing to be answered with.
            let validator = document_has_validator(route_match.route.render.strategy)
                .then(|| weak_content_etag(html.html.as_bytes()));
            if let Some(etag) = &validator
                && request_matches_etag(Some(request_headers), etag)
            {
                return Ok(not_modified_document(
                    etag,
                    route_match.route,
                    page.request_scoped,
                ));
            }
            let mut response = cached_html_response(StatusCode::OK, &html, Some(request_headers));
            insert_document_cache_control(&mut response, route_match.route, page.request_scoped);
            if let Some(etag) = &validator
                && let Ok(value) = HeaderValue::from_str(etag)
            {
                response.headers_mut().insert(header::ETAG, value);
            }
            Ok(response)
        }
        RouteKind::Api => {
            let headers = worker_request_headers(request_headers);
            render_api_pooled(
                state,
                route_match.route,
                request_target,
                method,
                &headers,
                request_body,
                &route_match.params,
            )
            .await
        }
    }
}

/// Whether this route's document is produced per request and can therefore stream.
///
/// Server components and `Ssr` together. Every other strategy has to produce a
/// string — a pre-render writes one to disk, an ISR or SSG entry puts one in a
/// cache — and a stream is the wrong shape for that. An ordinary SSR route
/// could stream too, but its render already resolves in one step: there is no
/// `Suspense` boundary for the server to fill in later, so there would be
/// nothing to stream except the loss of its document cache.
pub(crate) fn streams_document(route: &RouteEntry) -> bool {
    route.render.server_components && route.render.strategy == RenderStrategy::Ssr
}

/// Answer a `<form action={fn}>` submitted by a browser with no JavaScript.
///
/// The form posted to the page it is on, so the answer is that page — rendered
/// after its server function ran, which is what makes the result visible
/// without a single byte of JavaScript having executed. It is the same document
/// a `GET` would produce; only the state it reads has changed.
///
/// Streamed when the route streams anyway, and buffered otherwise, because the
/// choice belongs to the route rather than to the method. Either way the
/// response is `no-store`: it is one visitor's answer to one submission, and the
/// route's own strategy — which may be `Ssg`, and may have a file on disk — has
/// nothing to say about a POST.
async fn render_page_form_action(
    state: &AppState,
    route: &RouteEntry,
    request: &PageRequestContext<'_>,
    params: &RouteParams,
    styles: &str,
) -> Result<Response> {
    if streams_document(route) {
        return render_page_streamed(state, route, request, params, styles).await;
    }
    // `uncacheable` regardless of what the render read: a submission's answer
    // belongs to the visitor who sent it, which is why `render_page_pooled`
    // reports every form render as request-scoped in the first place.
    let page = render_page_pooled(state, route, request, params, styles).await?;
    Ok(uncacheable(cached_html_response(
        StatusCode::OK,
        &page.document,
        None,
    )))
}

/// Serve a server-components document as it is rendered.
///
/// The shell leaves as soon as React has it, and each `Suspense` boundary
/// follows when the server resolves it — so a slow server component delays the
/// part of the page waiting on it and nothing else. The buffered path a few
/// functions up holds the first byte until the last one is ready.
///
/// Nothing is cached, and that is inherent rather than an omission: the document
/// never exists as a string this process could store, and a route that declared
/// itself `Ssr` said its document is not reusable anyway.
///
/// The head is injected once `</head>` has been seen and the tail at the end,
/// both by [`StreamedDocument`]. The Flight payload is part of that tail: it is
/// complete only when the render is, and it arrives on the frame that ends the
/// body.
async fn render_page_streamed(
    state: &AppState,
    route: &RouteEntry,
    request: &PageRequestContext<'_>,
    params: &RouteParams,
    styles: &str,
) -> Result<Response> {
    let mut streamed = state
        .worker_pool
        .render_rsc_document(crate::worker_pool::RenderSsrRequest {
            project_root: &state.config.root,
            app_dir: &state.config.app_dir,
            page_file: &route.file,
            request_path: request.path,
            request_target: request.target,
            route_path: &route.path,
            params,
            headers: request.headers,
            method: request.method,
            server_components: true,
            form_action: request.form_action,
        })
        .await?;

    if !streamed.response.ok {
        let code = streamed
            .response
            .code
            .clone()
            .unwrap_or_else(|| "RUV1500".to_string());
        let message = streamed
            .response
            .message
            .clone()
            .unwrap_or_else(|| "Server-components render failed".to_string());
        return Err(
            Diagnostic::new("RUV1500", "Server-components render failed")
                .explain(ruvyxa_diagnostics::label_with_code(&code, &message))
                .at_file(&route.file)
                .into(),
        );
    }

    register_server_hmr_inputs(
        state,
        &route.path,
        streamed.response.inputs_version.as_deref(),
        streamed.response.inputs.as_deref(),
    );

    // A submitted form's server function has already run by the time the first
    // frame arrives, so anything it revalidated is known now rather than at the
    // end of the body.
    if request.form_action.is_some() {
        apply_revalidations(state, streamed.response.revalidate.take()).await;
    }

    let body = streamed.body.ok_or_else(|| {
        RuvyxaError::Message("Server-components stream started without a body".to_string())
    })?;

    let asset_links = state.runtime_cache.asset_links(&state.config).await;
    let plugin_head = render_plugin_head(&state.config.plugin_head);
    // Composed per stream rather than once: the framework's defaults stand down
    // for a document that declares its own, and the prefix — the first frame,
    // which carries the shell's `<head>` — is the only place to read that from.
    let head_tail = format!("{plugin_head}{styles}");
    // Resolved before the stream starts: the request is out of reach by the time
    // the head prefix arrives, and the answer is the same for every chunk.
    let locale = crate::i18n::localized_head(
        state.config.i18n.as_ref(),
        &route.path,
        request.path,
        params,
    );
    let compose = move |prefix: &str| {
        let head_content = format!(
            "{}{head_tail}",
            crate::document_head_defaults(prefix, &asset_links)
        );
        crate::html_document::compose_document_head(
            prefix,
            &head_content,
            locale
                .as_ref()
                .map(|(locale, head)| (locale.as_str(), head.as_str())),
        )
    };

    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request.path, params);
    let trailer = streamed.trailer;
    let tail = move || {
        // The payload the SSR pass rendered from, so the browser hydrates the
        // tree that is already on screen. Absent only if the worker ended the
        // stream without it, which leaves the page server-rendered and inert
        // rather than blank — the browser entry declines to hydrate nothing.
        let payload = trailer
            .get()
            .and_then(|frame| frame.rsc_payload.as_deref())
            .map(crate::html_document::rsc_payload_block)
            .unwrap_or_default();
        format!("{payload}{client_script}{hmr}")
    };

    // `Body::into_data_stream` reports `axum::Error`; the composer works in
    // `io::Error` because that is what the worker body stream produces. One
    // conversion here keeps both sides in the error type they already use.
    let source = body
        .into_data_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    Ok(streamed_html_response(Body::from_stream(
        StreamedDocument::new(source, compose, tail),
    )))
}

/// Dispatch page rendering based on the route's declared rendering strategy.
///
/// Returns the document as an `Arc<str>` so a cache hit — the common case in
/// production — shares the stored allocation instead of copying the whole page
/// on its way into the response body.
struct PageRequestContext<'a> {
    path: &'a str,
    target: &'a str,
    headers: &'a [(String, String)],
    method: &'a str,
    /// Set when this request is a `<form action={fn}>` submitted without
    /// JavaScript. The render then runs that server function first.
    form_action: Option<PostedForm<'a>>,
}

/// The bytes of a no-JavaScript `<form action={fn}>` submission, if this is one.
///
/// Three questions, all answerable from the request line and one header, and
/// none of them "does the body actually name an action" — that needs a
/// multipart parser and the server function registry, both of which live in the
/// worker. A POST that turns out to name nothing renders the page exactly as a
/// POST always did here, so guessing wide costs a body forwarded and nothing
/// else.
///
/// Server components are required because they are what a server function
/// needs: the reference in the form's hidden field is resolved against the
/// route's `react-server` graph, and a route without one has no such graph.
///
/// A hydrated page never reaches this. Its form posts to `/__ruvyxa/rsc`
/// instead, and gets a payload back to patch itself with rather than a whole
/// new document.
fn posted_form<'a>(
    route: &RouteEntry,
    method: &str,
    headers: &'a HeaderMap,
    body: Option<&'a [u8]>,
) -> Option<PostedForm<'a>> {
    if !route.render.server_components || !method.eq_ignore_ascii_case("POST") {
        return None;
    }
    let body = body.filter(|bytes| !bytes.is_empty())?;
    let content_type = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    // The two encodings a `<form>` can be submitted with. React asks for the
    // first when it writes the hidden fields; the second is what a form that
    // overrode `encType` sends, and both decode to `FormData` on the far side.
    if essence != "multipart/form-data" && essence != "application/x-www-form-urlencoded" {
        return None;
    }
    Some(PostedForm { content_type, body })
}

/// A rendered page document, and whether producing it read request state.
///
/// The two travel together because the second decides what may be said about
/// the first. A render that called `cookies()`, `headers()`, or `draftMode()`
/// produced one visitor's document, and both caches it can reach — the render
/// cache inside this process and whatever shared cache the `cache-control`
/// speaks to — have to be told so. Keeping the flag beside the bytes is what
/// stops a later caller from having only half the answer.
///
/// Only [`render_page_pooled`] can report `true`: every other strategy renders
/// through `handleSsg` in `worker-pool.mjs`, which installs no request context,
/// so those accessors throw there instead of answering.
struct RenderedPage {
    document: CachedDocument,
    request_scoped: bool,
}

impl RenderedPage {
    /// A document produced without reading request state.
    fn shared(document: CachedDocument) -> Self {
        Self {
            document,
            request_scoped: false,
        }
    }
}

async fn render_page_by_strategy(
    state: &AppState,
    route: &RouteEntry,
    request: &PageRequestContext<'_>,
    params: &RouteParams,
    styles: &str,
) -> Result<RenderedPage> {
    match route.render.strategy {
        RenderStrategy::Ssr => {
            let forced = state.render_cache.forced_claim(request.path).await;
            let page = render_page_pooled(state, route, request, params, styles).await?;
            if let Some(claim) = forced {
                state
                    .render_cache
                    .acknowledge_forced(request.path, claim)
                    .await;
            }
            Ok(page)
        }
        RenderStrategy::Ssg => {
            // In dev mode, SSG pages are rendered on-demand like SSR but cached indefinitely.
            render_page_ssg(state, route, request.path, params, styles)
                .await
                .map(RenderedPage::shared)
        }
        RenderStrategy::Isr => render_page_isr(state, route, request.path, params, styles)
            .await
            .map(RenderedPage::shared),
        RenderStrategy::Csr => render_page_csr(state, route, request.path, params, styles)
            .await
            .map(RenderedPage::shared),
        RenderStrategy::Ppr => render_page_ppr(state, route, request.path, params, styles)
            .await
            .map(RenderedPage::shared),
    }
}

/// SSG in dev mode: render once and cache (no TTL eviction).
/// In production: serve pre-rendered HTML directly from disk.
async fn render_page_ssg(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
) -> Result<CachedDocument> {
    let cache_key = render_cache::page_cache_key("ssg:", request_path, params);
    if let Some(cached) = state.render_cache.get_document(&cache_key).await {
        return Ok(cached);
    }

    // `revalidatePath()` named this URL. The build's HTML on disk is exactly
    // what it asked to replace, so this one request skips it and renders fresh.
    //
    // Claimed before the single flight and never inside it: a forced request is
    // replacing content the application declared out of date, and answering it
    // with a render that began before that declaration would serve exactly what
    // it invalidated. Only one request holds the claim, so the rest of a burst
    // still coalesces.
    let forced = state.render_cache.forced_claim(request_path).await;
    if forced.is_some() {
        return render_page_ssg_fresh(
            state,
            route,
            request_path,
            params,
            styles,
            &cache_key,
            forced,
        )
        .await;
    }

    single_flight_document(
        &state.single_flight,
        &state.render_cache,
        &cache_key,
        || render_page_ssg_fresh(state, route, request_path, params, styles, &cache_key, None),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn render_page_ssg_fresh(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
    cache_key: &str,
    forced: Option<ForcedRevalidationClaim>,
) -> Result<CachedDocument> {
    // In production, serve the pre-rendered HTML file. Read it from disk once
    // and serve subsequent requests from the in-memory render cache — a
    // synchronous file open per request otherwise dominates the hot path.
    if forced.is_none()
        && !state.config.watch
        && let Some((html, _)) =
            store_document(state, request_path, cache_key, RenderStrategy::Ssg, None).await
    {
        return Ok(html);
    }

    // Render via worker pool (same as SSR but with the SSG bundle type)
    let response = state
        .worker_pool
        .render_ssg(crate::worker_pool::RenderSsgRequest {
            project_root: &state.config.root,
            app_dir: &state.config.app_dir,
            page_file: &route.file,
            request_path,
            route_path: &route.path,
            params,
            mode: "full",
            server_components: route.render.server_components,
        })
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1500".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "SSG render failed".to_string());
        return Err(Diagnostic::new("RUV1500", "SSG render failed")
            .explain(ruvyxa_diagnostics::label_with_code(&code, &message))
            .at_file(&route.file)
            .into());
    }

    register_server_hmr_inputs(
        state,
        &route.path,
        response.inputs_version.as_deref(),
        response.inputs.as_deref(),
    );

    let rendered = response
        .html
        .ok_or_else(|| RuvyxaError::Message("SSG render produced no HTML".to_string()))?;

    let asset_links = state.runtime_cache.asset_links(&state.config).await;
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);
    let rsc_payload = rsc_payload_block(route, response.rsc_payload.as_deref());
    let plugin_head = render_plugin_head(&state.config.plugin_head);
    let head_content = format!(
        "{}{plugin_head}{styles}",
        crate::document_head_defaults(&rendered, &asset_links)
    );
    let html = compose_localized_document(
        &rendered,
        &head_content,
        &format!("{rsc_payload}{client_script}{hmr}"),
        state.config.i18n.as_ref(),
        route,
        request_path,
        params,
    );

    let document = state.render_cache.put(cache_key.to_string(), html).await;
    settle_forced_revalidation(state, request_path, forced, &document.html).await;
    Ok(document)
}

/// ISR: serve from cache if available (stale-while-revalidate), trigger
/// background revalidation when the entry is older than the revalidate interval.
/// In production: serve pre-rendered HTML and schedule background revalidation.
async fn render_page_isr(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
) -> Result<CachedDocument> {
    let cache_key = render_cache::page_cache_key("isr:", request_path, params);

    let revalidate_after = Duration::from_secs(route.render.revalidate.unwrap_or(60));

    // Serve stale content immediately. Only revalidate after the route's
    // configured interval, and coalesce concurrent requests for the same key.
    if let Some((cached, age)) = state.render_cache.get_stale_with_age(&cache_key).await {
        if age >= revalidate_after {
            spawn_isr_revalidation(state, route, request_path, params, styles, &cache_key);
        }
        return Ok(cached);
    }

    // A path `revalidatePath()` named renders on its own, for the reason given
    // in `render_page_ssg`.
    let forced = state.render_cache.forced_claim(request_path).await;
    if forced.is_some() {
        return render_page_isr_fresh(
            state,
            route,
            request_path,
            params,
            styles,
            &cache_key,
            forced,
        )
        .await;
    }

    single_flight_document(
        &state.single_flight,
        &state.render_cache,
        &cache_key,
        || render_page_isr_fresh(state, route, request_path, params, styles, &cache_key, None),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn render_page_isr_fresh(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
    cache_key: &str,
    forced: Option<ForcedRevalidationClaim>,
) -> Result<CachedDocument> {
    // In production, try the pre-rendered HTML file. Storing it means the first
    // background revalidation waits until the route's declared interval instead
    // of firing once per request. A path `revalidatePath()` named skips it: the
    // build's document is the stale one the caller is replacing.
    if forced.is_none()
        && !state.config.watch
        && let Some((html, refresh)) = store_document(
            state,
            request_path,
            cache_key,
            RenderStrategy::Isr,
            route.render.revalidate,
        )
        .await
    {
        // The store said this document is past its window. Serve it now and
        // refresh it behind the response, exactly as `render_page_isr` does for
        // an entry its own memory has aged out, and as the deployed hosts do in
        // `serveIncremental`. The refresh publishes through the handler, so the
        // next instance to ask the store gets a fresh answer.
        if refresh {
            spawn_isr_revalidation(state, route, request_path, params, styles, cache_key);
        }
        return Ok(html);
    }

    // No cached version — render synchronously (blocking fallback)
    let html = render_isr_background(state, route, request_path, params, styles).await?;
    let document = state.render_cache.put(cache_key.to_string(), html).await;
    settle_forced_revalidation(state, request_path, forced, &document.html).await;
    Ok(document)
}

/// ISR background render (used both for first render and revalidation).
async fn render_isr_background(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
) -> Result<String> {
    let response = state
        .worker_pool
        .render_ssg(crate::worker_pool::RenderSsgRequest {
            project_root: &state.config.root,
            app_dir: &state.config.app_dir,
            page_file: &route.file,
            request_path,
            route_path: &route.path,
            params,
            mode: "full",
            server_components: route.render.server_components,
        })
        .await?;

    if !response.ok {
        let message = response.message.unwrap_or_default();
        return Err(RuvyxaError::Message(format!(
            "ISR revalidation failed: {message}"
        )));
    }

    register_server_hmr_inputs(
        state,
        &route.path,
        response.inputs_version.as_deref(),
        response.inputs.as_deref(),
    );

    let rendered = response
        .html
        .ok_or_else(|| RuvyxaError::Message("ISR render produced no HTML".to_string()))?;

    let asset_links = state.runtime_cache.asset_links(&state.config).await;
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);
    let rsc_payload = rsc_payload_block(route, response.rsc_payload.as_deref());
    let plugin_head = render_plugin_head(&state.config.plugin_head);
    let head_content = format!(
        "{}{plugin_head}{styles}",
        crate::document_head_defaults(&rendered, &asset_links)
    );
    Ok(compose_localized_document(
        &rendered,
        &head_content,
        &format!("{rsc_payload}{client_script}{hmr}"),
        state.config.i18n.as_ref(),
        route,
        request_path,
        params,
    ))
}

/// Set of ISR cache keys that currently have a background revalidation running.
///
/// A plain [`std::sync::Mutex`] rather than the async one: the critical section
/// is a single `HashSet` insert or remove, so it never needs to hold the lock
/// across an await, and a synchronous lock is what lets the slot be released
/// from [`Drop`].
pub(crate) type IsrRevalidationSet = Arc<Mutex<HashSet<String>>>;

/// Exclusive claim on one ISR cache key's revalidation slot, released on drop.
///
/// The slot used to be freed by a `remove` call at the tail of the spawned
/// task's happy path. Anything that stopped the task before that line — a panic
/// inside the render, a cancelled runtime, or an early `return` added later —
/// left the key in the set permanently, and the route then never revalidated
/// again for the life of the process while still reporting cache hits. Tying
/// the release to `Drop` makes the claim last exactly as long as the task that
/// owns it, whatever way that task ends.
pub(crate) struct IsrRevalidationSlot {
    keys: IsrRevalidationSet,
    key: String,
}

impl IsrRevalidationSlot {
    /// Claim the slot for `key`, or `None` when a revalidation is already in
    /// flight for it.
    ///
    /// A poisoned lock is recovered rather than propagated: the guarded value is
    /// a key set with no invariant a panicking holder could have broken, and
    /// treating poison as fatal would disable ISR revalidation process-wide.
    pub(crate) fn claim(keys: &IsrRevalidationSet, key: &str) -> Option<Self> {
        let mut in_flight = keys.lock().unwrap_or_else(PoisonError::into_inner);
        if !in_flight.insert(key.to_string()) {
            return None;
        }
        drop(in_flight);
        Some(Self {
            keys: Arc::clone(keys),
            key: key.to_string(),
        })
    }
}

impl Drop for IsrRevalidationSlot {
    fn drop(&mut self) {
        self.keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// The renders currently in flight, keyed by the cache key each will fill.
///
/// A cacheable render is a check, a render, and a store, with nothing claiming
/// the key in between — so N requests that arrive for one cold URL each found a
/// miss and each rendered it. The worker process does coalesce identical renders
/// it sees together, but `select_worker` hands each racing request to a
/// different idle worker precisely so they do not queue behind one another, so
/// no worker ever saw more than one of them. Single-flight has to live here,
/// above the pool: the Rust host is the only side that knows the cache key.
///
/// Waiters receive the winner's `CachedDocument` — the same `Arc<str>` and the
/// same lazily-compressed copy the cache stores, not a second encoding of equal
/// bytes.
#[derive(Default)]
pub(crate) struct RenderSingleFlight {
    /// Sender per in-flight key. Dropped when the render ends, which is what
    /// tells waiters to answer for themselves.
    in_flight: Mutex<HashMap<String, broadcast::Sender<CachedDocument>>>,
    /// Route patterns whose renders have been observed to read request state.
    ///
    /// Whether a document is one visitor's is only knowable after the render:
    /// the worker reports it, having watched for a `cookies()`, `headers()`, or
    /// `draftMode()` call. Such a document may never be published to waiters, so
    /// coalescing those routes would make every burst wait out one render and
    /// then still render for itself — slower than not coalescing at all. Keyed
    /// by route pattern rather than URL because it is a property of the route's
    /// code: the set is bounded by the manifest, and a route that stops reading
    /// request state is coalesced again after the next restart.
    request_scoped_routes: Mutex<HashSet<String>>,
}

/// Whether this caller renders the key or waits for the caller that is.
pub(crate) enum RenderClaim {
    Leader(RenderLeader),
    Follower(broadcast::Receiver<CachedDocument>),
}

/// The claim on one cache key, released on drop.
///
/// Release is tied to `Drop` for the same reason [`IsrRevalidationSlot`]'s is: a
/// render that panics, or a request whose future is dropped when the client goes
/// away, must not leave the key claimed. Dropping the sender closes the channel,
/// so waiters are woken with an error rather than left waiting for a render that
/// is no longer running — a single-flight that turns one failure into a stall
/// for every concurrent request is worse than the duplicate renders it replaced.
pub(crate) struct RenderLeader {
    flight: Arc<RenderSingleFlight>,
    key: String,
    updates: broadcast::Sender<CachedDocument>,
}

impl RenderSingleFlight {
    /// Claim `key`, or subscribe to the claim someone else holds.
    pub(crate) fn claim(self: &Arc<Self>, key: &str) -> RenderClaim {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(updates) = in_flight.get(key) {
            return RenderClaim::Follower(updates.subscribe());
        }
        // One value is ever sent on this channel, so one slot is enough however
        // many waiters there are.
        let (updates, _) = broadcast::channel(1);
        in_flight.insert(key.to_string(), updates.clone());
        RenderClaim::Leader(RenderLeader {
            flight: Arc::clone(self),
            key: key.to_string(),
            updates,
        })
    }

    /// Whether this route's documents have been seen to depend on the request.
    pub(crate) fn renders_request_scoped(&self, route_path: &str) -> bool {
        self.request_scoped_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(route_path)
    }

    pub(crate) fn mark_request_scoped(&self, route_path: &str) {
        self.request_scoped_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(route_path.to_string());
    }

    /// How many callers are waiting on `key`.
    #[cfg(test)]
    fn waiting(&self, key: &str) -> usize {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .map_or(0, broadcast::Sender::receiver_count)
    }
}

impl RenderLeader {
    /// Hand the finished document to everyone waiting on this key.
    ///
    /// Only ever called with a document that was also allowed into the render
    /// cache. That is the same question — is this document shareable — and
    /// answering it twice is how one visitor's page would reach another's.
    fn publish(&self, document: &CachedDocument) {
        let _ = self.updates.send(document.clone());
    }
}

impl Drop for RenderLeader {
    fn drop(&mut self) {
        let mut in_flight = self
            .flight
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Only this leader's own channel: a later render of the same key has
        // its own claim, and removing that would let a third caller start a
        // duplicate render of what is still in flight.
        if in_flight
            .get(&self.key)
            .is_some_and(|updates| updates.same_channel(&self.updates))
        {
            in_flight.remove(&self.key);
        }
    }
}

/// Render `cache_key` once for however many callers are asking for it at once.
///
/// The losers wait for the winner's document. A winner that fails, is cancelled,
/// or produces something unshareable publishes nothing, and each waiter then
/// answers for itself — which is exactly what all of them did before this
/// existed, so the worst case is the old behaviour rather than a stall.
async fn single_flight_document<F, Fut>(
    flight: &Arc<RenderSingleFlight>,
    cache: &RenderCache,
    cache_key: &str,
    render: F,
) -> Result<CachedDocument>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<CachedDocument>>,
{
    match flight.claim(cache_key) {
        RenderClaim::Follower(updates) => {
            if let Some(document) = await_shared_document(updates, cache, cache_key).await {
                return Ok(document);
            }
            render().await
        }
        RenderClaim::Leader(leader) => {
            let rendered = render().await;
            if let Ok(document) = &rendered {
                leader.publish(document);
            }
            rendered
        }
    }
}

/// Wait for the render that claimed this key, or `None` if it produced nothing
/// this caller may use.
async fn await_shared_document(
    mut updates: broadcast::Receiver<CachedDocument>,
    cache: &RenderCache,
    cache_key: &str,
) -> Option<CachedDocument> {
    match updates.recv().await {
        Ok(document) => Some(document),
        // The leader published and then released its claim before this caller
        // subscribed, so the document it stored is already in the cache.
        Err(_) => cache.get_document(cache_key).await,
    }
}

/// Spawn a background task to revalidate an ISR page.
fn spawn_isr_revalidation(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
    cache_key: &str,
) {
    let Some(slot) = IsrRevalidationSlot::claim(&state.isr_revalidating, cache_key) else {
        return;
    };

    let revalidate_state = state.clone();
    let revalidate_route = route.clone();
    let revalidate_path = request_path.to_string();
    let revalidate_params = params.clone();
    let revalidate_styles = styles.to_string();
    let revalidate_key = cache_key.to_string();

    tokio::spawn(async move {
        // Held for the whole task so the slot is released however the task
        // ends. Never dropped early.
        let _slot = slot;
        if let Ok(html) = render_isr_background(
            &revalidate_state,
            &revalidate_route,
            &revalidate_path,
            &revalidate_params,
            &revalidate_styles,
        )
        .await
        {
            // Published before the local cache is updated, not after, and only
            // when the project declared a store. An ordinary revalidation used
            // to end at `render_cache`, which is this process's memory — so a
            // deployment running several instances refreshed the one that
            // happened to receive the request and left every other one serving
            // the document this render replaced. That is the arrangement
            // `cache.handler` exists to fix, and the forced path was the only
            // one that ever wrote anything another instance could read.
            if revalidate_state.config.data_cache_handler.is_some() {
                write_document_through_handler(
                    &revalidate_state,
                    &revalidate_path,
                    &html,
                    revalidate_route.render.revalidate,
                    false,
                )
                .await;
            }
            revalidate_state
                .render_cache
                .put(revalidate_key, html)
                .await;
        }
    });
}

/// Try to serve a pre-rendered HTML file from the prerender directory.
/// Returns `Some(html)` if the file exists, `None` otherwise.
pub(crate) fn serve_prerendered_html(prerender_dir: &Path, request_path: &str) -> Option<String> {
    let html_path = prerendered_document_path(prerender_dir, request_path)?;
    let html_path = contained_public_asset(prerender_dir, &html_path)?;
    fs::read_to_string(html_path).ok()
}

/// Where the prerendered document for `request_path` lives, before any
/// existence or containment check.
///
/// The reader and the writer below derive this path from one place. A layout
/// rule copied into both is a rule that can drift, and a writer that publishes
/// to a file the reader never opens would retire a revalidation claim while the
/// stale document it was supposed to replace stayed on disk.
fn prerendered_document_path(prerender_dir: &Path, request_path: &str) -> Option<PathBuf> {
    let sanitized = request_path.trim_start_matches('/');
    if !sanitized.is_empty() && !is_safe_relative_path(sanitized) {
        return None;
    }
    // Again, inline, beside the join it guards. See `is_safe_relative_path`.
    if sanitized.contains("../") {
        return None;
    }
    Some(if sanitized.is_empty() {
        prerender_dir.join("index.html")
    } else {
        prerender_dir.join(sanitized).join("index.html")
    })
}

/// Whether no stale prerendered document remains for `request_path`.
///
/// Replaces the build's document with `html` when one exists. Only an artifact
/// the build already published is replaced: creating one for a path the build
/// never prerendered would add a disk fallback where none existed, and for ISR
/// it would also restart the age its stale-while-revalidate window is measured
/// from. A path with no artifact is already answered by a fresh render, so it
/// reports `true` with nothing written.
///
/// `false` means a document is still on disk and still stale — a read-only
/// filesystem, a permission error, a full disk.
fn settle_prerendered_artifact(prerender_dir: &Path, request_path: &str, html: &str) -> bool {
    let Some(candidate) = prerendered_document_path(prerender_dir, request_path) else {
        // The reader rejects this path for the same reason, so it can never
        // serve a document for it.
        return true;
    };
    let Some(existing) = contained_public_asset(prerender_dir, &candidate) else {
        return true;
    };
    // Unlike the content-addressed callers of `write_atomic`, two concurrent
    // forced renders of one path can carry different bytes. The rename is still
    // atomic, so a reader sees one complete document or the other, and both are
    // fresh renders of the same URL.
    ruvyxa_bundler::atomic_file::write_atomic(&existing, html.as_bytes()).is_ok()
}

/// Retire a `revalidatePath()` claim once the document behind it can no longer
/// be served.
///
/// A claim exists to stop a request from answering with the build's HTML for a
/// URL the application declared out of date, so it may only be retired when
/// that HTML cannot come back. Two situations allow it:
///
///   - Watch mode never reads the prerender directory, so no artifact of it can
///     reach a response.
///   - The artifact was replaced with this render, or never existed.
///
/// Anything else leaves the claim pending, which is exactly what happened
/// before this function existed. That is the safe direction — a claim that
/// outlives its cause costs cache efficiency, while one retired too early
/// serves content the application already invalidated.
///
/// There is deliberately no `requestScoped` guard here, unlike the equivalent
/// write in `serverless-handler.mjs`. That host renders every strategy inside a
/// request context, so an SSG page there can read a cookie and produce one
/// visitor's document. `handleSsg` in `worker-pool.mjs` installs no context, so
/// `cookies()`, `headers()`, and `draftMode()` throw instead of returning a
/// value: a document that reaches this function cannot contain request state.
/// Should that ever change, this write needs the same guard the serverless
/// handler has, and so does the `render_cache.put` above every call site.
async fn settle_forced_revalidation(
    state: &AppState,
    request_path: &str,
    claim: Option<ForcedRevalidationClaim>,
    html: &Arc<str>,
) {
    let Some(claim) = claim else {
        return;
    };

    // The project's store first, and awaited: `revalidatePath()` answered a
    // request that told its caller the old document is gone, so losing this
    // write leaves every other instance serving exactly what was invalidated.
    // The local artifact below is still replaced, because the build's directory
    // stays the fallback this host reads when the store has nothing.
    if !state.config.watch && state.config.data_cache_handler.is_some() {
        write_document_through_handler(state, request_path, html, None, true).await;
    }

    if !state.config.watch {
        let prerender_dir = state.config.prerender_dir.clone();
        let path = request_path.to_string();
        let html = Arc::clone(html);
        // The publish canonicalizes a path and writes a whole document, the
        // same blocking work `read_prerendered_html` already keeps off the
        // async worker threads.
        let settled = tokio::task::spawn_blocking(move || {
            settle_prerendered_artifact(&prerender_dir, &path, &html)
        })
        .await
        .unwrap_or(false);

        if !settled {
            tracing::debug!(
                path = request_path,
                "prerendered document could not be replaced; its revalidation claim stays pending"
            );
            return;
        }
    }

    state
        .render_cache
        .acknowledge_forced(request_path, claim)
        .await;
}

/// Async wrapper around [`serve_prerendered_html`].
///
/// The lookup canonicalizes paths and reads the whole document, so running it
/// inline on an async request handler blocks a Tokio worker thread for the
/// duration of the file I/O. `render_page_pooled` already reads page modules
/// through `spawn_blocking`; prerendered documents follow the same rule.
async fn read_prerendered_html(prerender_dir: &Path, request_path: &str) -> Option<String> {
    let prerender_dir = prerender_dir.to_path_buf();
    let request_path = request_path.to_string();
    tokio::task::spawn_blocking(move || serve_prerendered_html(&prerender_dir, &request_path))
        .await
        .ok()
        .flatten()
}

/// Read a prerendered document once and store it under `cache_key`.
///
/// Every prerendered strategy funnels its disk read through here so the rule
/// stays in one place: a prerender directory does not change while a
/// production server runs, and re-opening the same document per request is the
/// dominant cost on an otherwise trivial response. Callers consult the render
/// cache with their own getter first — SSG/CSR/PPR use the TTL getter, ISR
/// deliberately uses the stale-tolerant one.
async fn store_prerendered_html(
    render_cache: &RenderCache,
    prerender_dir: &Path,
    request_path: &str,
    cache_key: &str,
) -> Option<CachedDocument> {
    let html = read_prerendered_html(prerender_dir, request_path).await?;
    Some(render_cache.put(cache_key.to_string(), html).await)
}

/// The stored document for this path: the project's store first, then the build.
///
/// One seam for every strategy that serves a stored document — SSG, CSR, ISR,
/// and PPR all reach it through here — because the question "where does a
/// stored document come from" has one answer and four callers, and a rule
/// written four times is a rule three of them will miss.
///
/// **The project's store is consulted first, and the build's own directory is
/// the fallback rather than being replaced.** The deployed hosts substitute
/// `cache.handler`'s `read` for the platform reader outright, which is right
/// there: a function bundle's copy of the documents is the platform store. Here
/// they are two different things — `.ruvyxa/prerender` is what this build
/// produced, and it is still the correct answer for a path the shared store has
/// never held. Replacing it would make every SSG page re-render once per
/// instance against a cold store, for no gain.
///
/// A document the store reports **stale** is still a document the store holds,
/// and it is served. The returned flag says so, and the ISR caller refreshes it
/// behind the response — the same stale-while-revalidate the deployed hosts
/// run in `serveIncremental`, and the same rule this host already applies to
/// its own memory in `render_page_isr`. It used to be treated as a miss, and a
/// miss falls through to the build's directory: the document served was then
/// the one the build produced, which is *older* than the stale one the store
/// had just answered with, and it was stored with a fresh timestamp, so no
/// refresh ran for a whole window either. A stale document is never older than
/// the fallback, so serving it is the correct direction, and the refresh is
/// what makes it temporary.
///
/// The rule for what an answer means and what each strategy does with it is
/// `tests/fixtures/stored-document-conformance.json`, which the worker and the
/// deployed handler replay too.
async fn store_document(
    state: &AppState,
    request_path: &str,
    cache_key: &str,
    strategy: RenderStrategy,
    revalidate: Option<u64>,
) -> Option<(CachedDocument, bool)> {
    if state.config.data_cache_handler.is_some()
        && let Some((html, refresh)) = serve_stored_document(
            read_document_through_handler(state, request_path, revalidate).await,
            strategy,
        )
    {
        let document = state.render_cache.put(cache_key.to_string(), html).await;
        return Some((document, refresh));
    }
    store_prerendered_html(
        &state.render_cache,
        &state.config.prerender_dir,
        request_path,
        cache_key,
    )
    .await
    .map(|document| (document, false))
}

/// What a strategy does with the store's answer: the document to serve, and
/// whether to refresh it behind the response.
///
/// A held document is served whatever its freshness, because the alternative
/// on this host is the build's own copy, which is never newer. Only ISR has a
/// window to refresh against, so only ISR refreshes a stale one — the deployed
/// hosts draw the same line between `serveIncremental` and `servePrerendered`.
/// A miss serves nothing; the caller falls back and then renders.
fn serve_stored_document(
    answer: StoredDocument,
    strategy: RenderStrategy,
) -> Option<(String, bool)> {
    match answer {
        StoredDocument::Miss => None,
        StoredDocument::Held { html, stale } => {
            Some((html, stale && strategy == RenderStrategy::Isr))
        }
    }
}

/// What the project's store answered for one path.
///
/// Three answers, not two: "the store has nothing" and "the store has this and
/// says it is expired" lead to different documents. A miss falls back to the
/// build's own directory; a held document is served whether or not it is
/// stale, because the build's copy is never newer than the store's.
#[derive(Debug, PartialEq, Eq)]
enum StoredDocument {
    Miss,
    Held { html: String, stale: bool },
}

/// Read a worker's `documentRead` answer.
///
/// Only the `ok` shape is interpreted here; a failed response is a store this
/// request could not reach, and the caller reports it before treating it as a
/// miss. `stale` is carried, not decided: `None` and `Some(false)` both mean
/// the store called the document fresh, because a worker that predates the
/// flag answers neither.
fn stored_document_from_response(response: WorkerResponse) -> StoredDocument {
    match response.html {
        Some(html) if response.ok => StoredDocument::Held {
            html,
            stale: response.stale == Some(true),
        },
        _ => StoredDocument::Miss,
    }
}

/// Ask a render worker for this path's document from the project's store.
///
/// Through the pool because the store is a JavaScript module the project wrote,
/// and a worker is the only thing in this process tree that can call it.
///
/// A worker that fails is a store this request cannot reach, not a request that
/// fails: the caller falls back to the build's own document and, failing that,
/// renders. Reported rather than swallowed, because a store that is quietly
/// unreachable is a deployment paying for one and getting per-instance caching.
async fn read_document_through_handler(
    state: &AppState,
    request_path: &str,
    revalidate: Option<u64>,
) -> StoredDocument {
    let request = WorkerRequest::DocumentRead {
        id: worker_protocol::next_request_id(),
        pathname: request_path.to_string(),
        revalidate,
    };
    match state.worker_pool.send(request).await {
        Ok(response) if response.ok => stored_document_from_response(response),
        Ok(response) => {
            warn!(
                path = request_path,
                message = response.message.unwrap_or_default(),
                "cache.handler read failed; falling back to this build's documents"
            );
            StoredDocument::Miss
        }
        Err(error) => {
            warn!(
                path = request_path,
                error = error.to_string(),
                "cache.handler read could not reach a worker"
            );
            StoredDocument::Miss
        }
    }
}

/// Publish one rendered document to the project's store.
///
/// Awaited by the caller that persists a revalidation, for the reason the
/// deployed hosts await theirs: the write is the point of the revalidation, and
/// a process that exits between the response and an unawaited write leaves
/// every other instance serving the document this one just replaced.
async fn write_document_through_handler(
    state: &AppState,
    request_path: &str,
    html: &str,
    revalidate: Option<u64>,
    forced: bool,
) {
    let request = WorkerRequest::DocumentWrite {
        id: worker_protocol::next_request_id(),
        pathname: request_path.to_string(),
        html: html.to_string(),
        revalidate,
        forced,
    };
    match state.worker_pool.send(request).await {
        Ok(response) if response.ok => {}
        Ok(response) => warn!(
            path = request_path,
            message = response.message.unwrap_or_default(),
            "cache.handler write failed; this document stayed on the local disk only"
        ),
        Err(error) => warn!(
            path = request_path,
            error = error.to_string(),
            "cache.handler write could not reach a worker"
        ),
    }
}

/// CSR: emit a minimal HTML shell with no server-rendered content.
/// The page loads entirely in the browser via the client bundle.
/// In production: serve the pre-built CSR shell HTML.
async fn render_page_csr(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
) -> Result<CachedDocument> {
    let cache_key = render_cache::page_cache_key("csr:", request_path, params);
    if let Some(cached) = state.render_cache.get_document(&cache_key).await {
        return Ok(cached);
    }
    let forced = state.render_cache.forced_claim(request_path).await;

    // In production, serve the pre-rendered CSR shell. The disk read is cached
    // like every other prerendered strategy: without it each request re-opens
    // and re-reads the same shell file for the life of the process.
    if forced.is_none()
        && !state.config.watch
        && let Some((html, _)) =
            store_document(state, request_path, &cache_key, RenderStrategy::Csr, None).await
    {
        return Ok(html);
    }

    let asset_links = state.runtime_cache.asset_links(&state.config).await;
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);

    // `csr: true` tells the client prelude to mount rather than hydrate: this
    // shell was not rendered from the route tree, so there is no markup to
    // hydrate against. See the bootstrap in `ruvyxa_bundler::output`.
    //
    // These parameters come from the request URL, and a segment containing
    // `</script>` would close the element and run whatever followed it. The
    // escaping lives inside `bootstrap_data_block` rather than here — this
    // writer is the one that once forgot it.
    let bootstrap = bootstrap_data_block(params, request_path, true);

    let shell = format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  {asset_links}
  {styles}
  {bootstrap}
</head>
<body>
  <div id="__ruvyxa"></div>
  {client_script}
  {hmr}
</body>
</html>"#
    );

    if forced.is_some() {
        // Cached rather than per-request: the claim is retired below only once
        // the build's shell has been replaced, and the fresh shell answers the
        // requests that arrive before then.
        let document = state.render_cache.put(cache_key, shell).await;
        settle_forced_revalidation(state, request_path, forced, &document.html).await;
        Ok(document)
    } else {
        // Not cache-backed: the shell is built for this request only, so there
        // is nowhere to keep an encoded copy and the layer compresses it as before.
        Ok(CachedDocument::uncached(Arc::from(shell)))
    }
}

/// PPR: render the static shell (Suspense fallbacks) and stream dynamic slots.
/// In dev mode, we render with onShellReady to get the shell quickly, then
/// the remaining content streams in via the client hydration.
/// In production: serve the pre-rendered shell from disk.
async fn render_page_ppr(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
) -> Result<CachedDocument> {
    let cache_key = render_cache::page_cache_key("ppr:", request_path, params);
    if let Some(cached) = state.render_cache.get_document(&cache_key).await {
        return Ok(cached);
    }

    // A path `revalidatePath()` named renders on its own, for the reason given
    // in `render_page_ssg`.
    let forced = state.render_cache.forced_claim(request_path).await;
    if forced.is_some() {
        return render_page_ppr_fresh(
            state,
            route,
            request_path,
            params,
            styles,
            &cache_key,
            forced,
        )
        .await;
    }

    single_flight_document(
        &state.single_flight,
        &state.render_cache,
        &cache_key,
        || render_page_ppr_fresh(state, route, request_path, params, styles, &cache_key, None),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn render_page_ppr_fresh(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
    cache_key: &str,
    forced: Option<ForcedRevalidationClaim>,
) -> Result<CachedDocument> {
    // In production, serve the pre-rendered PPR shell. The cache lookup above
    // must come first: reading the shell from disk before consulting the cache
    // made the cache unreachable and re-read the same file on every request.
    if forced.is_none()
        && !state.config.watch
        && let Some((html, _)) =
            store_document(state, request_path, cache_key, RenderStrategy::Ppr, None).await
    {
        return Ok(html);
    }

    // PPR mode: render with onShellReady (Suspense boundaries show fallback)
    let response = state
        .worker_pool
        .render_ssg(crate::worker_pool::RenderSsgRequest {
            project_root: &state.config.root,
            app_dir: &state.config.app_dir,
            page_file: &route.file,
            request_path,
            route_path: &route.path,
            params,
            mode: "ppr",
            // Partial pre-rendering streams a shell through a different entry
            // than the server-components pipeline builds, so the two are
            // refused together at discovery (RUV1019) rather than combined here.
            server_components: false,
        })
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1550".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "PPR render failed".to_string());
        return Err(Diagnostic::new("RUV1550", "PPR render failed")
            .explain(ruvyxa_diagnostics::label_with_code(&code, &message))
            .at_file(&route.file)
            .into());
    }

    register_server_hmr_inputs(
        state,
        &route.path,
        response.inputs_version.as_deref(),
        response.inputs.as_deref(),
    );

    let rendered = response
        .html
        .ok_or_else(|| RuvyxaError::Message("PPR render produced no HTML".to_string()))?;

    let asset_links = state.runtime_cache.asset_links(&state.config).await;
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request_path, params);
    let plugin_head = render_plugin_head(&state.config.plugin_head);
    let head_content = format!(
        "{}{plugin_head}{styles}",
        crate::document_head_defaults(&rendered, &asset_links)
    );
    let html = compose_localized_document(
        &rendered,
        &head_content,
        &format!("{client_script}{hmr}"),
        state.config.i18n.as_ref(),
        route,
        request_path,
        params,
    );

    let document = state.render_cache.put(cache_key.to_string(), html).await;
    settle_forced_revalidation(state, request_path, forced, &document.html).await;
    Ok(document)
}

/// The payload block a document carries, or nothing.
///
/// Only a route that ships a client bundle has anything to replay it: a page
/// with `export const hydrate = false` would otherwise carry a copy of its own
/// element tree that nothing on the page ever reads.
fn rsc_payload_block(route: &RouteEntry, payload: Option<&str>) -> String {
    if !route.render.ships_client_bundle() {
        return String::new();
    }
    payload
        .map(crate::html_document::rsc_payload_block)
        .unwrap_or_default()
}

/// The document `app/not-found.tsx` renders, or `None` when the project has no
/// such file — or when rendering it failed, which must not turn a 404 into a
/// 500.
///
/// Rendered as an ordinary page through the worker pool, with the root layout
/// around it, so it gets the same head, styles, and locale handling every other
/// document does. It ships no client bundle: the client router has no route for
/// this URL, and a script that tried to hydrate one would replace the document
/// with an empty tree.
async fn render_root_not_found(
    state: &AppState,
    request_path: &str,
    request_headers: &HeaderMap,
) -> Option<String> {
    let file = ["not-found.tsx", "not-found.jsx"]
        .into_iter()
        .map(|name| state.config.app_dir.join(name))
        .find(|candidate| candidate.is_file())?;

    let has_root_layout = ["layout.tsx", "layout.jsx"]
        .into_iter()
        .any(|name| state.config.app_dir.join(name).is_file());
    let layout_chain = if has_root_layout {
        vec!["app/layout".to_string()]
    } else {
        Vec::new()
    };

    let route = RouteEntry {
        id: "app/not-found".to_string(),
        path: "/__ruvyxa/not-found".to_string(),
        kind: RouteKind::Page,
        file,
        layout_chain,
        template_chain: Vec::new(),
        slots: Vec::new(),
        intercepts: Vec::new(),
        server_modules: Vec::new(),
        client_modules: Vec::new(),
        runtime: ruvyxa_graph::RuntimeTarget::Node,
        render: ruvyxa_graph::RenderMeta {
            hydration: ruvyxa_graph::HydrationMode::None,
            ..Default::default()
        },
    };

    let styles = state.runtime_cache.style_tag(&state.config).await.ok()?;
    let headers = worker_request_headers(request_headers);
    let request = PageRequestContext {
        path: request_path,
        target: request_path,
        headers: &headers,
        method: "GET",
        form_action: None,
    };
    render_page_pooled(state, &route, &request, &RouteParams::new(), &styles)
        .await
        .ok()
        .map(|page| page.document.html.to_string())
}
async fn render_page_pooled(
    state: &AppState,
    route: &RouteEntry,
    request: &PageRequestContext<'_>,
    params: &RouteParams,
    styles: &str,
) -> Result<RenderedPage> {
    // Check render cache first. Three things skip it. A submitted form skips it
    // in both directions: a stored document was rendered before this action ran,
    // and the one this produces answers a submission nobody else made. A route
    // that declared `export const dynamic = 'force-dynamic'` skips it because
    // that is what the declaration asks for — reading the export used to mean
    // only "do not pre-render this", so the page was rendered once per process
    // and every later visitor got that first document, timestamps and all.
    let cache_key = render_cache::ssr_cache_key(request.path, params);
    let cacheable = request.form_action.is_none() && !route.render.force_dynamic;
    if cacheable && let Some(cached) = state.render_cache.get_document(&cache_key).await {
        // A document the render cache holds was allowed in, which is the same
        // question this flag answers: nothing request-scoped is ever stored.
        return Ok(RenderedPage::shared(cached));
    }

    // A render nobody else can use is not worth coalescing, and a route already
    // seen to read request state would only make a burst wait out one render
    // before every caller rendered anyway.
    if !cacheable || state.single_flight.renders_request_scoped(&route.path) {
        return render_page_pooled_fresh(state, route, request, params, styles, &cache_key).await;
    }

    match state.single_flight.claim(&cache_key) {
        RenderClaim::Follower(updates) => {
            if let Some(document) =
                await_shared_document(updates, &state.render_cache, &cache_key).await
            {
                return Ok(RenderedPage::shared(document));
            }
            render_page_pooled_fresh(state, route, request, params, styles, &cache_key).await
        }
        RenderClaim::Leader(leader) => {
            let page =
                render_page_pooled_fresh(state, route, request, params, styles, &cache_key).await;
            match &page {
                // Published under exactly the condition the document was stored
                // under. A request-scoped render is one visitor's page.
                Ok(rendered) if !rendered.request_scoped => leader.publish(&rendered.document),
                Ok(_) => state.single_flight.mark_request_scoped(&route.path),
                Err(_) => {}
            }
            page
        }
    }
}

async fn render_page_pooled_fresh(
    state: &AppState,
    route: &RouteEntry,
    request: &PageRequestContext<'_>,
    params: &RouteParams,
    styles: &str,
    cache_key: &str,
) -> Result<RenderedPage> {
    // The page source is deliberately not read here. Confirming the default
    // export is route validation's job (`ruvyxa check`, and `validate_app` on
    // every build), and doing it again per request meant a full
    // `read_to_string` of the page module — through a `spawn_blocking` hop — on
    // every cache miss in production, to answer a question the worker's own
    // module load already answers. `missing_default_export_diagnostic` below
    // keeps the actionable RUV1004 message without the read.
    let mut response = state
        .worker_pool
        .render_ssr(crate::worker_pool::RenderSsrRequest {
            project_root: &state.config.root,
            app_dir: &state.config.app_dir,
            page_file: &route.file,
            request_path: request.path,
            request_target: request.target,
            route_path: &route.path,
            params,
            headers: request.headers,
            method: request.method,
            server_components: route.render.server_components,
            form_action: request.form_action,
        })
        .await?;

    if !response.ok {
        let code = response.code.unwrap_or_else(|| "RUV1100".to_string());
        let message = response
            .message
            .unwrap_or_else(|| "React SSR failed without an error message".to_string());
        if let Some(diagnostic) = missing_default_export_diagnostic(&route.file, &message) {
            return Err(diagnostic.into());
        }
        let explanation = if let Some(stack) = response.stack {
            format!("{message}\n\n{stack}")
        } else {
            message
        };
        return Err(Diagnostic::new("RUV1100", "React SSR failed")
            .explain(ruvyxa_diagnostics::label_with_code(&code, &explanation))
            .at_file(&route.file)
            .suggest("Check the page component, its imports, and whether React dependencies are installed.")
            .into());
    }

    register_server_hmr_inputs(
        state,
        &route.path,
        response.inputs_version.as_deref(),
        response.inputs.as_deref(),
    );

    // Reported by the worker when the render actually read a cookie, a header,
    // or draft mode. Such a document is one user's page: putting it in the
    // shared render cache would serve it to the next visitor of the same URL.
    let request_scoped = response.request_scoped.unwrap_or(false)
        || request.form_action.is_some()
        || route.render.force_dynamic;

    // A server function the submitted form ran may have called
    // `revalidatePath()`. Applied before the response is returned so a visitor
    // who follows a link on this page cannot beat the invalidation to the cache.
    if request.form_action.is_some() {
        apply_revalidations(state, response.revalidate.take()).await;
    }

    let rendered = response
        .html
        .ok_or_else(|| RuvyxaError::Message("React SSR completed without HTML".to_string()))?;

    let asset_links = state.runtime_cache.asset_links(&state.config).await;
    let hmr = if state.config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(&state.config, route, request.path, params);
    // The payload the SSR pass rendered from, so the browser hydrates the tree
    // that is already on screen instead of asking the server to build it again.
    // Written before the bundle that reads it: both are inert data until the
    // deferred module runs, but a reader that precedes its data reads as a bug.
    let rsc_payload = rsc_payload_block(route, response.rsc_payload.as_deref());
    let plugin_head = render_plugin_head(&state.config.plugin_head);
    let head_content = format!(
        "{}{plugin_head}{styles}",
        crate::document_head_defaults(&rendered, &asset_links)
    );

    let html = compose_localized_document(
        &rendered,
        &head_content,
        &format!("{rsc_payload}{client_script}{hmr}"),
        state.config.i18n.as_ref(),
        route,
        request.path,
        params,
    );

    if request_scoped {
        // Not stored, and the flag travels with it so the caller can keep it out
        // of the shared caches downstream too.
        return Ok(RenderedPage {
            document: CachedDocument::uncached(Arc::from(html)),
            request_scoped: true,
        });
    }

    // Cache the fully rendered page for subsequent requests, and serve the very
    // allocation that was stored.
    Ok(RenderedPage::shared(
        state.render_cache.put(cache_key.to_string(), html).await,
    ))
}

/// Act on the `revalidatePath()` calls an API route or server action made.
///
/// Applied after the handler succeeded and before its response is returned, so
/// a client that navigates on success cannot beat the invalidation to the
/// cache. Paths are bounded and validated here rather than trusted: they cross
/// a process boundary, and a handler that loops could otherwise ask the server
/// to walk an unbounded list on the request path.
pub(crate) async fn apply_revalidations(state: &AppState, paths: Option<Vec<String>>) {
    let Some(paths) = paths else { return };
    if paths.len() > MAX_REVALIDATED_PATHS {
        let dropped = state.render_cache.revalidate_all_paths().await;
        tracing::warn!(
            received = paths.len(),
            limit = MAX_REVALIDATED_PATHS,
            dropped,
            "oversized revalidation payload; bypassing all prerendered artifacts"
        );
        return;
    }
    for path in paths {
        if !path.starts_with('/') || path.encode_utf16().count() > MAX_REVALIDATED_PATH_LEN {
            tracing::warn!(path, "ignoring revalidatePath() for an unusable path");
            continue;
        }
        let dropped = state.render_cache.revalidate_path(&path).await;
        tracing::debug!(path, dropped, "revalidated path");
    }
}

/// Public `revalidatePath()` limit; the host also enforces it defensively for
/// payloads from older or untrusted workers without dropping a valid tail.
const MAX_REVALIDATED_PATHS: usize = 64;
/// Longest revalidated path accepted, matching the realtime metadata bound.
const MAX_REVALIDATED_PATH_LEN: usize = 2_048;

pub(crate) async fn render_api_pooled(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
    params: &RouteParams,
) -> Result<Response> {
    let known_inputs_version = state.hmr_tracker.server_graph_version(&route.path);
    let WorkerApiResponse {
        mut response,
        body: streamed_body,
        trailer: _,
    } = state
        .worker_pool
        .render_api(RenderApiRequest {
            project_root: &state.config.root,
            route_file: &route.file,
            method,
            request_path,
            headers,
            body,
            params,
            known_inputs_version: known_inputs_version.as_deref(),
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
            .explain(ruvyxa_diagnostics::label_with_code(&code, &explanation))
            .at_file(&route.file)
            .suggest("Check the route handler export and its imports.")
            .into());
    }

    register_server_hmr_inputs(
        state,
        &route.path,
        response.inputs_version.as_deref(),
        response.inputs.as_deref(),
    );

    apply_revalidations(state, response.revalidate.take()).await;

    let status = response.status.unwrap_or(200);
    let status = StatusCode::from_u16(status)
        .map_err(|error| RuvyxaError::Message(format!("Invalid API response status: {error}")))?;
    let body =
        streamed_body.unwrap_or_else(|| Body::from(response.body.take().unwrap_or_default()));
    let mut http_response = (status, body).into_response();

    if let Some(headers) = response.header_pairs.take().or_else(|| {
        response
            .headers
            .take()
            .map(|headers| headers.into_iter().collect::<Vec<_>>())
    }) {
        for (name, value) in headers {
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_str(&value) else {
                continue;
            };
            http_response.headers_mut().append(name, value);
        }
    }

    Ok(with_security_headers(http_response))
}

/// A development client bundle together with the route it registers itself under.
///
/// The route pattern travels with the document because the bundle has to be
/// stamped with that pattern before it is served, and the pattern is not
/// recoverable from the request path once a dynamic segment has been filled in.
pub(crate) struct ClientBundle {
    pub document: CachedDocument,
    /// The route the bundle registers itself under, dynamic segments and all.
    pub route_path: String,
}

/// The version a development client bundle is advertised and stamped with.
///
/// A build hashes the reference manifest and the bundler writes that hash into
/// the bundle it produced. Development has no build to hash, so the bundle text
/// is its own identity — computed over the *unstamped* script, because a script
/// cannot contain a hash of itself. Every caller therefore hashes before
/// `stamp_client_artifact` runs, and the route manifest and the served bundle
/// arrive at the same string without either having to tell the other.
pub(crate) fn client_artifact_version(script: &str) -> String {
    blake3::hash(script.as_bytes()).to_hex()[..16].to_string()
}

/// A client bundle with the line that tells the router which build it is.
///
/// `ruvyxa build` appends this in the bundler; development compiles browser
/// bundles in the Node worker, which never wrote it. The router imported a
/// route bundle, found no stamp under the route, read that as a stale artifact
/// and fell back to a document load — so every soft navigation in `ruvyxa dev`
/// was a full page load while the pages themselves looked fine.
///
/// Emitted at serve time rather than in the worker because the version is a
/// hash of the worker's own output, which the worker cannot know while it is
/// still producing it.
pub(crate) fn stamp_client_artifact(script: &str, route_path: &str) -> String {
    // JSON string syntax is a subset of JavaScript string syntax, so a JSON
    // literal is always a correctly escaped JS literal — the same reasoning the
    // bundler's `js_string` is built on.
    let route = serde_json::to_string(route_path).unwrap_or_else(|_| "\"\"".to_string());
    let version = serde_json::to_string(&client_artifact_version(script))
        .unwrap_or_else(|_| "\"\"".to_string());
    format!("{script}\n;(globalThis.__RUVYXA_ROUTE_ARTIFACTS__ ||= {{}})[{route}] = {version};\n")
}

pub(crate) async fn render_client_bundle_pooled(
    state: &AppState,
    request_path: &str,
) -> Result<ClientBundle> {
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
    if let Some(cached) = state.render_cache.get_document(&cache_key).await {
        return Ok(ClientBundle {
            document: cached,
            route_path: route_match.route.path.clone(),
        });
    }

    let response = state
        .worker_pool
        .render_client(
            &state.config.root,
            &state.config.app_dir,
            &route_match.route.file,
            request_path,
            &route_match.route.path,
            &route_match.params,
            route_match.route.render.server_components,
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
                .explain(ruvyxa_diagnostics::label_with_code(&code, &explanation))
                .suggest(
                    "Check the page component, its browser-safe imports, and React dependencies.",
                )
                .into(),
        );
    }

    register_client_hmr_inputs(
        state,
        &route_match.route.path,
        response.inputs_version.as_deref(),
        response.inputs.as_deref(),
    );

    let script = response.script.ok_or_else(|| {
        RuvyxaError::Message("Client renderer completed without script output".to_string())
    })?;

    // Cache the bundled client script
    let document = state.render_cache.put(cache_key, script).await;

    Ok(ClientBundle {
        document,
        route_path: route_match.route.path.clone(),
    })
}

pub(crate) async fn render_server_action_pooled(
    state: &AppState,
    request_path: &str,
    action_name: &str,
    payload_json: &str,
    content_type: &str,
    request_headers: &HeaderMap,
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

    let known_inputs_version = state
        .hmr_tracker
        .action_graph_version(&route_match.route.path);
    let mut response = state
        .worker_pool
        .render_action(RenderActionRequest {
            project_root: &state.config.root,
            action_file: &action_file,
            action_name,
            payload_json,
            content_type,
            request_path,
            headers: &worker_request_headers(request_headers),
            known_inputs_version: known_inputs_version.as_deref(),
        })
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

    register_action_hmr_inputs(
        state,
        &route_match.route.path,
        response.inputs_version.as_deref(),
        response.inputs.as_deref(),
    );

    apply_revalidations(state, response.revalidate.take()).await;

    let status = StatusCode::from_u16(response.status.unwrap_or(200)).unwrap_or(StatusCode::OK);
    let mut http_response = (status, response.body.take().unwrap_or_default()).into_response();
    let mut realtime_event = None;

    if let Some(headers) = response.header_pairs.or_else(|| {
        response
            .headers
            .map(|headers| headers.into_iter().collect::<Vec<_>>())
    }) {
        for (key, value) in headers {
            if key.eq_ignore_ascii_case("x-ruvyxa-realtime-event") {
                if realtime_event.is_some() {
                    return Err(RuvyxaError::Message(
                        "RUV1500 action returned duplicate realtime event metadata".into(),
                    ));
                }
                realtime_event = Some(decode_realtime_event(&value)?);
                continue;
            }
            let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_str(&value) else {
                continue;
            };
            http_response.headers_mut().append(name, value);
        }
    }

    if let (Some(runtime), Some(event)) = (&state.realtime, realtime_event) {
        let _ = runtime.tx.send(event);
    }

    Ok(with_security_headers(http_response))
}

pub(crate) fn decode_realtime_event(value: &str) -> Result<String> {
    if value.len() > 24 * 1024 {
        return Err(RuvyxaError::Message(
            "RUV1500 action realtime event metadata exceeds 24 KiB".into(),
        ));
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| {
            RuvyxaError::Message("RUV1500 action realtime event is not base64url".into())
        })?;
    let payload: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        RuvyxaError::Message("RUV1500 action realtime event is not valid JSON".into())
    })?;
    let channels = payload
        .get("channels")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            RuvyxaError::Message("RUV1500 action realtime event has no channels".into())
        })?;
    let action = payload.get("action").and_then(serde_json::Value::as_str);
    let path = payload.get("path").and_then(serde_json::Value::as_str);
    let invalidated = payload
        .get("invalidated")
        .and_then(serde_json::Value::as_array);
    let valid = payload.get("version").and_then(serde_json::Value::as_u64) == Some(1)
        && payload.get("type").and_then(serde_json::Value::as_str) == Some("action")
        && !channels.is_empty()
        && channels.len() <= 16
        && channels.iter().all(|channel| {
            channel
                .as_str()
                .is_some_and(crate::realtime_endpoints::valid_realtime_channel)
        })
        && action.is_some_and(|action| !action.is_empty() && action.len() <= 256)
        && path.is_some_and(|path| path.starts_with('/') && path.len() <= 2_048)
        && invalidated.is_some_and(|keys| {
            keys.len() <= 64
                && keys
                    .iter()
                    .all(|key| key.as_str().is_some_and(|key| key.len() <= 256))
        });
    if !valid {
        return Err(RuvyxaError::Message(
            "RUV1500 action realtime event has invalid metadata".into(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| RuvyxaError::Message("RUV1500 action realtime event is not UTF-8".into()))
}

pub(crate) async fn runtime_trace_cached(
    config: &ServerConfig,
    runtime_cache: &RuntimeCache,
    request_path: &str,
) -> Result<RuntimeTrace> {
    // Use the cached compiled router. Calling `find_route` here rebuilt the
    // whole radix trie on every trace request even though `runtime_cache`
    // already holds a compiled one for the same manifest.
    let (manifest, router) = runtime_cache.router(config).await?;
    let route_match = router.find(&manifest, request_path);
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

fn render_page(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
    styles: &str,
) -> Result<String> {
    // No source read here either: the one-shot renderer surfaces a missing
    // default export the same way the worker pool does, and route validation
    // catches it before a render is ever attempted.
    let rendered = render_react_page(config, route, request_path, params)?;
    let asset_links = public_asset_links(&config.public_dir);
    let hmr = if config.watch {
        hmr_client_script()
    } else {
        ""
    };
    let client_script = client_hydration_script(config, route, request_path, params);
    let plugin_head = render_plugin_head(&config.plugin_head);
    let head_content = format!(
        "{}{plugin_head}{styles}",
        crate::document_head_defaults(&rendered, &asset_links)
    );

    Ok(compose_localized_document(
        &rendered,
        &head_content,
        &format!("{client_script}{hmr}"),
        config.i18n.as_ref(),
        route,
        request_path,
        params,
    ))
}

/// Recover the actionable RUV1004 diagnostic from a worker module-load failure.
///
/// The generated SSR entry does `import Page from "<page module>"`, so a page
/// with no default export fails when the worker links that module. The runtime
/// used to pre-empt that by reading and scanning the page source on every render;
/// recognizing the loader's own message instead keeps the useful diagnostic and
/// leaves the source on disk.
fn missing_default_export_diagnostic(file: &Path, message: &str) -> Option<Diagnostic> {
    // Node phrases it as `... does not provide an export named 'default'`;
    // bundlers and Bun use `No matching export ... for "default"`. Match on the
    // stable parts of both rather than a single engine's exact wording.
    let lowered = message.to_ascii_lowercase();
    let mentions_default = lowered.contains("'default'")
        || lowered.contains("\"default\"")
        || lowered.contains("named default");
    let is_missing_export = lowered.contains("does not provide an export")
        || lowered.contains("no matching export")
        || lowered.contains("has no exported member");
    if !(mentions_default && is_missing_export) {
        return None;
    }
    Some(
        Diagnostic::new("RUV1004", "Page is missing a default export")
            .explain("Every TypeScript/JavaScript page must export a default component. Markdown and MDX pages receive one from the content compiler.")
            .at_file(file)
            .suggest("Add `export default function Page() { return <main /> }`."),
    )
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
    header_pairs: Option<Vec<(String, String)>>,
    body: Option<String>,
    code: Option<String>,
    message: Option<String>,
    stack: Option<String>,
}

fn render_react_page(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
) -> Result<String> {
    let renderer = find_ssr_renderer(&config.root).ok_or_else(|| {
        Diagnostic::new("RUV1102", "SSR renderer was not found")
            .explain("Ruvyxa could not find the Node SSR renderer used to transform TSX and render React.")
            .suggest("Run pnpm install from the monorepo root, or install the ruvyxa package in the app.")
    })?;

    let mut command = javascript_command(config)?;
    command
        .arg(&renderer)
        .arg(&config.root)
        .arg(&config.app_dir)
        .arg(&route.file)
        .arg(request_path)
        .arg(
            serde_json::to_string(params)
                .map_err(|error| RuvyxaError::Message(error.to_string()))?,
        );
    let output = run_renderer(&mut command, config, "React SSR")?;

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
    if let Some(diagnostic) = missing_default_export_diagnostic(&route.file, &message) {
        return Err(diagnostic.into());
    }
    let explanation = if let Some(stack) = result.stack {
        format!("{message}\n\n{stack}")
    } else {
        message
    };

    Err(Diagnostic::new("RUV1100", "React SSR failed")
        .explain(ruvyxa_diagnostics::label_with_code(&code, &explanation))
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

/// Locate one of the `ruvyxa` package runtime scripts.
///
/// Resolution order:
/// 1. `RUVYXA_SSR_RENDERER`, for `ssr-renderer.mjs` only.
/// 2. `packages/ruvyxa/runtime/` from the current directory upwards, so the
///    framework monorepo runs its own working tree.
/// 3. `node_modules/ruvyxa/runtime/` from the project root upwards. The upward
///    walk is required: package managers hoist dependencies to the workspace
///    root, so an app at `apps/web` in a user monorepo has no local
///    `node_modules/ruvyxa`.
pub fn find_runtime_script(root: &Path, file_name: &str) -> Option<PathBuf> {
    if file_name == "ssr-renderer.mjs"
        && let Ok(renderer) = std::env::var("RUVYXA_SSR_RENDERER")
    {
        let path = PathBuf::from(renderer);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Ok(cwd) = std::env::current_dir()
        && let Some(path) = find_upwards(&cwd, Path::new("packages/ruvyxa/runtime"), file_name)
    {
        return Some(path);
    }

    find_upwards(root, Path::new("node_modules/ruvyxa/runtime"), file_name)
}

/// Walk `start` and each of its ancestors looking for `<dir>/<relative>/<file_name>`.
fn find_upwards(start: &Path, relative: &Path, file_name: &str) -> Option<PathBuf> {
    let mut current = start;
    loop {
        let candidate = current.join(relative).join(file_name);
        if candidate.is_file() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

/// Run a one-shot renderer process under a bound.
///
/// These are the fallback render paths used when the worker pool is not
/// serving the request. They run while an HTTP request is open, so a renderer
/// that never exits — a page module that starts a timer or opens a handle at
/// import time — would hold the request thread indefinitely. The bound turns
/// that into an error the developer can see.
fn run_renderer(
    command: &mut Command,
    config: &ServerConfig,
    what: &str,
) -> Result<std::process::Output> {
    crate::process::output_with_timeout(command, crate::process::RENDER_TIMEOUT).map_err(|error| {
        match error {
            crate::process::ProcessError::Io(source) => RuvyxaError::Io {
                message: format!("Failed to start {} for {what}", config.runtime.command()),
                source,
            },
            timed_out => RuvyxaError::Message(format!(
                "{what} {timed_out}. A module imported by this route may be keeping the \
                 {} process alive after rendering.",
                config.runtime.command()
            )),
        }
    })
}

fn javascript_command(config: &ServerConfig) -> Result<Command> {
    let mut command = Command::new(config.runtime.executable());
    command.args(config.runtime.script_args());
    command.envs(runtime_env(config)?);
    Ok(command)
}

pub(crate) fn runtime_env(config: &ServerConfig) -> Result<BTreeMap<String, String>> {
    let mut env = project_env(&config.root)?;
    env.insert(
        "RUVYXA_JSX_RUNTIME".to_string(),
        jsx_runtime_name(config.jsx_runtime).to_string(),
    );
    env.insert(
        "RUVYXA_ES_TARGET".to_string(),
        config.es_target.as_str().to_string(),
    );
    env.insert(
        "RUVYXA_RUNTIME".to_string(),
        config.runtime.command().to_string(),
    );
    // The two `cache()` bounds, on the same channel and for the same reason as
    // `RUVYXA_ES_TARGET`: the number is decided by `ruvyxa.config.ts` and spent
    // by `@ruvyxa/core`'s store inside the worker, which has no other way to
    // learn it. Only inserted when the project configured one, so the absent
    // case stays the store's own default rather than a value this crate
    // restates — the second copy of a default is the one that drifts.
    if let Some(entries) = config.data_cache_max_entries {
        env.insert(
            "RUVYXA_DATA_CACHE_MAX_ENTRIES".to_string(),
            entries.to_string(),
        );
    }
    if let Some(bytes) = config.data_cache_max_bytes {
        env.insert("RUVYXA_DATA_CACHE_MAX_BYTES".to_string(), bytes.to_string());
    }
    // `cache.handler` travels the same way, and it is the half that decides
    // whether two instances agree at all: the numbers bound a per-instance tier,
    // this names the store they share. Absent unless the project configured one,
    // so a worker with no variable behaves exactly as it always has.
    if let Some(handler) = &config.data_cache_handler {
        env.insert(
            "RUVYXA_DATA_CACHE_HANDLER".to_string(),
            handler.to_string_lossy().into_owned(),
        );
    }
    if let Some(prefix) = &config.data_cache_key_prefix {
        env.insert("RUVYXA_DATA_CACHE_KEY_PREFIX".to_string(), prefix.clone());
    }
    apply_production_node_env(&mut env, !config.watch);
    Ok(env)
}

/// Tell a rendering worker it is serving production, unless the project said so.
///
/// React ships two builds of itself and picks between them by reading
/// `process.env.NODE_ENV` at load. A worker started without the variable gets
/// the development build: slower, noisier, and — once a route renders through
/// the server-components pipeline — one that writes absolute source paths into
/// the Flight payload the browser receives. `ruvyxa dev` wants exactly that
/// build; `ruvyxa start` and `ruvyxa build` want the other one.
///
/// Only set when the project has not: an app that puts `NODE_ENV` in its `.env`
/// has said which it wants, and overriding that would be this framework
/// deciding for it.
pub fn apply_production_node_env(env: &mut BTreeMap<String, String>, production: bool) {
    if production {
        env.entry("NODE_ENV".to_string())
            .or_insert_with(|| "production".to_string());
    }
}

fn jsx_runtime_name(runtime: JsxRuntime) -> &'static str {
    match runtime {
        JsxRuntime::Classic => "classic",
        JsxRuntime::Automatic => "automatic",
    }
}

/// Load project environment values for JavaScript runtime processes.
fn render_api(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    method: &str,
    params: &RouteParams,
) -> Result<Response> {
    let renderer = find_api_renderer(&config.root).ok_or_else(|| {
        Diagnostic::new("RUV1202", "API renderer was not found")
            .explain("Ruvyxa could not find the Node API renderer used to transform and execute route handlers.")
            .suggest("Run pnpm install from the monorepo root, or install the ruvyxa package in the app.")
    })?;

    let mut command = javascript_command(config)?;
    command
        .arg(&renderer)
        .arg(&config.root)
        .arg(&route.file)
        .arg(method)
        .arg(request_path)
        .arg(
            serde_json::to_string(params)
                .map_err(|error| RuvyxaError::Message(error.to_string()))?,
        );
    let output = run_renderer(&mut command, config, "API route rendering")?;

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
            .explain(ruvyxa_diagnostics::label_with_code(&code, &explanation))
            .at_file(&route.file)
            .suggest("Check the route handler export and its imports.")
            .into());
    }

    let status = result.status.unwrap_or(200);
    let status = StatusCode::from_u16(status)
        .map_err(|error| RuvyxaError::Message(format!("Invalid API response status: {error}")))?;
    let body = result.body.unwrap_or_default();
    let mut response = (status, body).into_response();

    if let Some(headers) = result
        .header_pairs
        .or_else(|| result.headers.map(|headers| headers.into_iter().collect()))
    {
        for (name, value) in headers {
            let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            let Ok(value) = HeaderValue::from_str(&value) else {
                continue;
            };
            response.headers_mut().append(name, value);
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

pub(crate) fn action_file_for(route: &RouteEntry) -> Option<PathBuf> {
    let route_dir = route.file.parent()?;
    ["action.ts", "action.js"]
        .into_iter()
        .map(|name| route_dir.join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rendered document carries the strategy's caching contract.
    ///
    /// This host used to send a `200` with no `cache-control` at all while the
    /// deployed function sent one, so the same page was cacheable-on-a-heuristic
    /// under `ruvyxa start` and explicit everywhere else.
    #[test]
    fn a_rendered_document_carries_the_strategy_cache_control() {
        let mut route = ruvyxa_graph::RouteEntry {
            id: "page:/x".to_string(),
            path: "/x".to_string(),
            kind: RouteKind::Page,
            file: std::path::PathBuf::from("app/x/page.tsx"),
            layout_chain: Vec::new(),
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
            server_modules: Vec::new(),
            client_modules: Vec::new(),
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: Default::default(),
        };

        for (strategy, revalidate, expected) in [
            (
                RenderStrategy::Ssg,
                None,
                "public, max-age=0, must-revalidate",
            ),
            (
                RenderStrategy::Csr,
                None,
                "public, max-age=0, must-revalidate",
            ),
            (RenderStrategy::Ssr, None, "no-store"),
            (
                RenderStrategy::Isr,
                Some(30),
                "public, max-age=0, s-maxage=30, stale-while-revalidate=31535970",
            ),
        ] {
            route.render.strategy = strategy;
            route.render.revalidate = revalidate;
            let mut response = html_response(StatusCode::OK, "<html></html>".to_string());
            insert_document_cache_control(&mut response, &route, false);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                expected,
                "{strategy:?}"
            );
        }
    }

    /// A render that read request state is never offered to a shared cache.
    ///
    /// `request_scoped` was computed by `render_page_pooled` and read only by
    /// the *store* decision — whether the render cache may hold the document.
    /// Nothing consulted it when the `cache-control` went on, so the answer to
    /// "may another process reuse this?" and the answer to "may a CDN?" came
    /// from different places. For `Isr` the header a personalised document left
    /// with was `public, max-age=0, s-maxage=…, stale-while-revalidate=…`, which
    /// is one visitor's page offered to every later reader of that URL.
    ///
    /// Held here for every strategy, not only the ones a request-scoped document
    /// can reach today, because what is being pinned is that the response's own
    /// answer outranks the route's table. `handlePage` in
    /// `serverless-handler.mjs` is the other half of the same rule.
    #[test]
    fn a_request_scoped_document_is_never_shared_cacheable() {
        let mut route = ruvyxa_graph::RouteEntry {
            id: "page:/x".to_string(),
            path: "/x".to_string(),
            kind: RouteKind::Page,
            file: std::path::PathBuf::from("app/x/page.tsx"),
            layout_chain: Vec::new(),
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
            server_modules: Vec::new(),
            client_modules: Vec::new(),
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: Default::default(),
        };

        for strategy in [
            RenderStrategy::Ssg,
            RenderStrategy::Csr,
            RenderStrategy::Ssr,
            RenderStrategy::Isr,
            RenderStrategy::Ppr,
        ] {
            route.render.strategy = strategy;
            route.render.revalidate = Some(30);

            let mut response = html_response(StatusCode::OK, "<html></html>".to_string());
            insert_document_cache_control(&mut response, &route, true);
            let value = response.headers()[header::CACHE_CONTROL]
                .to_str()
                .expect("cache-control is ASCII");
            assert_eq!(value, "no-store", "{strategy:?}");
            assert!(
                !value.contains("s-maxage"),
                "{strategy:?} must name no shared-cache lifetime"
            );

            // The 304 answering a conditional request for that document says the
            // same thing; a 200 and its 304 disagreeing about who may reuse a
            // document is the same disclosure in two steps.
            let not_modified = not_modified_document("W/\"abc\"", &route, true);
            assert_eq!(
                not_modified.headers()[header::CACHE_CONTROL],
                "no-store",
                "{strategy:?} 304"
            );
        }
    }

    /// A 304 for a stored document carries what a cache needs and no body.
    ///
    /// The `200` half is exercised end to end by the pipeline; what is pinned
    /// here is the shape of the answer, because a 304 that forgot its lifetime
    /// or its validator leaves the next revalidation with nothing to send.
    #[test]
    fn a_document_304_carries_its_validator_and_lifetime() {
        let mut route = ruvyxa_graph::RouteEntry {
            id: "page:/x".to_string(),
            path: "/x".to_string(),
            kind: RouteKind::Page,
            file: std::path::PathBuf::from("app/x/page.tsx"),
            layout_chain: Vec::new(),
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
            server_modules: Vec::new(),
            client_modules: Vec::new(),
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: Default::default(),
        };
        route.render.strategy = RenderStrategy::Isr;
        route.render.revalidate = Some(30);

        let etag = weak_content_etag(b"<html></html>");
        assert!(
            etag.starts_with("W/"),
            "documents are served in more than one              encoding, so the validator is weak"
        );
        let response = not_modified_document(&etag, &route, false);

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], etag);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=0, s-maxage=30, stale-while-revalidate=31535970"
        );
        assert_eq!(response.headers()[header::VARY], "accept-encoding");
        assert!(
            !response.headers().contains_key(header::CONTENT_LENGTH),
            "a length beside an absent body reads as a truncated response"
        );
    }

    /// A request holding the served version is recognised, whatever form it took.
    ///
    /// The weak marker says the two representations are semantically rather than
    /// byte-for-byte equivalent, which is not what `If-None-Match` is deciding —
    /// so a client echoing the tag back without it still matches.
    #[test]
    fn a_document_validator_is_compared_weakly() {
        let etag = weak_content_etag(b"<html></html>");
        let strong = etag.trim_start_matches("W/").to_string();
        for candidate in [etag.as_str(), strong.as_str(), "*"] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::IF_NONE_MATCH,
                HeaderValue::from_str(candidate).unwrap(),
            );
            assert!(
                request_matches_etag(Some(&headers), &etag),
                "{candidate} names the served version"
            );
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("W/\"stale\""),
        );
        assert!(!request_matches_etag(Some(&headers), &etag));
    }

    /// Which routes stream, spelled out.
    ///
    /// The dividing line is not "server components" but "produced per request".
    /// A pre-rendered, cached, or revalidated document has to become a string
    /// to be stored, and a route without server components has no `Suspense`
    /// boundary for the server to fill in later — so both would trade a real
    /// document cache for nothing.
    #[test]
    fn only_a_per_request_server_components_document_streams() {
        let mut route = ruvyxa_graph::RouteEntry {
            id: "page:/x".to_string(),
            path: "/x".to_string(),
            kind: RouteKind::Page,
            file: std::path::PathBuf::from("app/x/page.tsx"),
            layout_chain: Vec::new(),
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
            server_modules: Vec::new(),
            client_modules: Vec::new(),
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: Default::default(),
        };

        route.render.server_components = true;
        route.render.strategy = RenderStrategy::Ssr;
        assert!(streams_document(&route));

        for stored in [
            RenderStrategy::Ssg,
            RenderStrategy::Isr,
            RenderStrategy::Ppr,
            RenderStrategy::Csr,
        ] {
            route.render.strategy = stored;
            assert!(!streams_document(&route), "{stored:?}");
        }

        route.render.strategy = RenderStrategy::Ssr;
        route.render.server_components = false;
        assert!(!streams_document(&route));
    }

    /// Which requests are treated as a form submission, spelled out.
    ///
    /// Recognising one too eagerly costs a body forwarded to the worker and a
    /// `no-store` on a page that would have cached; missing one leaves the form
    /// silently inert for every visitor without JavaScript. The rule is
    /// deliberately wide within server-components routes and never leaves them,
    /// because a route with no `react-server` graph cannot resolve a reference
    /// at all.
    #[test]
    fn only_a_form_shaped_post_to_a_server_components_route_runs_an_action() {
        let mut route = ruvyxa_graph::RouteEntry {
            id: "page:/x".to_string(),
            path: "/x".to_string(),
            kind: RouteKind::Page,
            file: std::path::PathBuf::from("app/x/page.tsx"),
            layout_chain: Vec::new(),
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
            server_modules: Vec::new(),
            client_modules: Vec::new(),
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: Default::default(),
        };
        route.render.server_components = true;

        let headers = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(value).unwrap());
            headers
        };
        let body: &[u8] = b"--x--";

        for content_type in [
            "multipart/form-data; boundary=x",
            "MULTIPART/FORM-DATA; boundary=x",
            "application/x-www-form-urlencoded",
        ] {
            assert!(
                posted_form(&route, "POST", &headers(content_type), Some(body)).is_some(),
                "{content_type}"
            );
        }

        // A read, whatever it carries.
        assert!(
            posted_form(
                &route,
                "GET",
                &headers("multipart/form-data; boundary=x"),
                Some(body)
            )
            .is_none()
        );
        // A server function call from a hydrated page. It posts JSON-ish bodies
        // to `/__ruvyxa/rsc`, never a form encoding to the page itself.
        assert!(posted_form(&route, "POST", &headers("application/json"), Some(body)).is_none());
        // Nothing to decode.
        assert!(
            posted_form(
                &route,
                "POST",
                &headers("multipart/form-data; boundary=x"),
                Some(b"")
            )
            .is_none()
        );
        assert!(posted_form(&route, "POST", &headers("multipart/form-data"), None).is_none(),);
        // No `react-server` graph, so no reference the fields could name.
        route.render.server_components = false;
        assert!(
            posted_form(
                &route,
                "POST",
                &headers("multipart/form-data; boundary=x"),
                Some(body)
            )
            .is_none()
        );
    }

    /// The stamp a development bundle carries must be the version the route
    /// manifest advertises for it, or the router treats every freshly imported
    /// bundle as stale and falls back to a document load.
    #[test]
    fn a_stamped_bundle_registers_the_version_the_manifest_advertises() {
        let script = "console.log(1);";
        let version = client_artifact_version(script);
        let stamped = stamp_client_artifact(script, "/blog/[slug]");

        assert!(stamped.starts_with(script), "the script is served intact");
        assert!(
            stamped.contains(&format!(
                r#"(globalThis.__RUVYXA_ROUTE_ARTIFACTS__ ||= {{}})["/blog/[slug]"] = "{version}";"#
            )),
            "{stamped}"
        );
        // Hashed before the stamp is appended, so the manifest endpoint — which
        // only ever sees the unstamped script — reaches the same string.
        assert_ne!(version, client_artifact_version(&stamped));
    }

    /// A route path is interpolated into JavaScript, so it is emitted as a JSON
    /// literal rather than pasted between quotes.
    #[test]
    fn a_stamp_escapes_a_route_that_would_close_its_own_string() {
        let stamped = stamp_client_artifact("", r#"/a");globalThis.pwned=1;//"#);
        assert!(
            stamped.contains(r#"["/a\");globalThis.pwned=1;//"]"#),
            "{stamped}"
        );
    }

    #[test]
    fn revalidation_bounds_match_the_shared_host_contract() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/revalidation-conformance.json"
        ))
        .unwrap();
        assert_eq!(
            MAX_REVALIDATED_PATHS as u64,
            contract["maxPathsPerRequest"].as_u64().unwrap()
        );
        assert_eq!(
            MAX_REVALIDATED_PATH_LEN as u64,
            contract["maxPathLength"].as_u64().unwrap()
        );
    }

    /// The runtime no longer reads the page source to detect a missing default
    /// export, so the actionable RUV1004 has to be recovered from the module
    /// loader's own failure message instead.
    #[test]
    fn recovers_the_missing_default_export_diagnostic_from_loader_messages() {
        let page = Path::new("app/docs/page.tsx");
        for message in [
            "The requested module './page.tsx' does not provide an export named 'default'",
            "No matching export in \"app/docs/page.tsx\" for import \"default\"",
            "Module '\"./page\"' has no exported member 'default'",
        ] {
            let diagnostic = missing_default_export_diagnostic(page, message)
                .unwrap_or_else(|| panic!("should map: {message}"));
            assert_eq!(diagnostic.code, "RUV1004");
        }
    }

    /// Unrelated render failures must keep their own diagnostic. Mapping them to
    /// RUV1004 would send every SSR crash to the wrong fix.
    #[test]
    fn leaves_unrelated_render_failures_unmapped() {
        let page = Path::new("app/docs/page.tsx");
        for message in [
            "ReferenceError: window is not defined",
            "The requested module './db' does not provide an export named 'query'",
            "Cannot find module 'react-dom/server'",
            "default is not a function",
        ] {
            assert!(
                missing_default_export_diagnostic(page, message).is_none(),
                "should not map: {message}"
            );
        }
    }

    /// The context must own the route graph and the compiled router, not
    /// re-derive them per request: deleting the app directory after the context
    /// is built leaves routing intact, which cannot be true of a lookup that
    /// rescans the project.
    #[test]
    fn render_context_resolves_routes_without_rescanning_the_project() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        std::fs::create_dir_all(app.join("blog/[slug]")).unwrap();
        std::fs::write(
            app.join("page.tsx"),
            "export default function Home() { return <main /> }",
        )
        .unwrap();
        std::fs::write(
            app.join("blog/[slug]/page.tsx"),
            "export default function Post() { return <article /> }",
        )
        .unwrap();

        let config = ServerConfig::dev(temp.path(), "localhost", 3000);
        let context = RenderContext::new(&config).unwrap();

        std::fs::remove_dir_all(&app).unwrap();
        assert!(RenderContext::new(&config).is_err() || !app.exists());

        let matched = context
            .router
            .find(&context.manifest, "/blog/hello")
            .expect("the context keeps the graph it discovered");
        assert_eq!(matched.route.path, "/blog/[slug]");
        assert_eq!(
            matched.params.get("slug").and_then(|value| value.as_str()),
            Some("hello")
        );
    }

    /// A burst of concurrent requests for one cold URL.
    const BURST: usize = 8;
    const BURST_KEY: &str = "ssg:/pricing";

    /// Block until every caller in the burst has claimed or subscribed.
    ///
    /// Without it the leader could finish before the others arrive, and they
    /// would each start a render of their own — which is what the count is
    /// supposed to be measuring the absence of.
    async fn wait_for_waiters(flight: &Arc<RenderSingleFlight>, expected: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while flight.waiting(BURST_KEY) < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the burst never reached the in-flight render");
    }

    /// N concurrent requests for one cold page cost one render, not N.
    ///
    /// Every cacheable strategy was a check, a render, and a store with nothing
    /// claiming the key in between. The worker process coalesces identical
    /// renders it sees together, but `select_worker` deliberately routes racing
    /// requests to different idle workers, so no worker ever saw two of them.
    /// The first half of this test is that shape, and it is what the second half
    /// is measured against.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn one_cold_key_is_rendered_once_for_the_whole_burst() {
        let html = "<!doctype html><html><body>pricing</body></html>";

        // Check, render, store — with no claim between the miss and the store.
        let cache = Arc::new(RenderCache::new(16, 60));
        let uncoalesced = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let together = Arc::new(tokio::sync::Barrier::new(BURST));
        let mut burst = tokio::task::JoinSet::new();
        for _ in 0..BURST {
            let cache = Arc::clone(&cache);
            let renders = Arc::clone(&uncoalesced);
            let together = Arc::clone(&together);
            burst.spawn(async move {
                if cache.get_document(BURST_KEY).await.is_some() {
                    return;
                }
                renders.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                together.wait().await;
                cache.put(BURST_KEY.to_string(), html.to_string()).await;
            });
        }
        while burst.join_next().await.is_some() {}
        assert_eq!(
            uncoalesced.load(std::sync::atomic::Ordering::SeqCst),
            BURST,
            "without a claim every request in the burst renders the page itself"
        );

        // The same burst, coalesced. The leader cannot finish until the whole
        // burst is waiting on it, so the count is not a scheduling accident.
        let cache = Arc::new(RenderCache::new(16, 60));
        let flight = Arc::new(RenderSingleFlight::default());
        let renders = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let mut burst = tokio::task::JoinSet::new();
        for _ in 0..BURST {
            let cache = Arc::clone(&cache);
            let flight = Arc::clone(&flight);
            let renders = Arc::clone(&renders);
            let release = Arc::clone(&release);
            burst.spawn(async move {
                single_flight_document(&flight, &cache, BURST_KEY, || async {
                    renders.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _permit = release.acquire().await.unwrap();
                    Ok(cache.put(BURST_KEY.to_string(), html.to_string()).await)
                })
                .await
            });
        }

        wait_for_waiters(&flight, BURST - 1).await;
        release.add_permits(BURST);

        let mut documents = Vec::new();
        while let Some(joined) = burst.join_next().await {
            documents.push(joined.unwrap().expect("every caller must be answered"));
        }
        assert_eq!(
            renders.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the burst must cost one render"
        );
        assert_eq!(documents.len(), BURST);
        for document in &documents {
            assert!(
                Arc::ptr_eq(&document.html, &documents[0].html),
                "every caller must receive the winner's document, not a copy"
            );
        }
    }

    /// A render that fails must release its claim, not strand the burst.
    ///
    /// This is the failure mode a single-flight introduces if the claim is only
    /// released on success: one broken render would hold every concurrent
    /// request for that URL until they timed out. Each waiter answers for
    /// itself instead, which is exactly what all of them did before.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failing_shared_render_releases_its_claim() {
        let cache = Arc::new(RenderCache::new(16, 60));
        let flight = Arc::new(RenderSingleFlight::default());
        let renders = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));

        let mut burst = tokio::task::JoinSet::new();
        for _ in 0..BURST {
            let cache = Arc::clone(&cache);
            let flight = Arc::clone(&flight);
            let renders = Arc::clone(&renders);
            let release = Arc::clone(&release);
            burst.spawn(async move {
                single_flight_document(&flight, &cache, BURST_KEY, || async {
                    renders.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _permit = release.acquire().await.unwrap();
                    Err(RuvyxaError::Message("render failed".to_string()))
                })
                .await
            });
        }

        wait_for_waiters(&flight, BURST - 1).await;
        release.add_permits(BURST);

        let answered = tokio::time::timeout(Duration::from_secs(10), async {
            let mut answers = Vec::new();
            while let Some(joined) = burst.join_next().await {
                answers.push(joined.unwrap());
            }
            answers
        })
        .await
        .expect("a failed render must not leave the burst waiting");

        assert_eq!(answered.len(), BURST);
        assert!(answered.iter().all(std::result::Result::is_err));
        assert_eq!(
            renders.load(std::sync::atomic::Ordering::SeqCst),
            BURST,
            "each waiter falls back to rendering for itself"
        );
        assert_eq!(
            flight.waiting(BURST_KEY),
            0,
            "no claim may outlive the render that took it"
        );
    }

    /// A route whose renders read cookies, headers, or draft mode is never
    /// coalesced again.
    ///
    /// Its document is one visitor's, so it may not be published — and waiting
    /// out a render nobody can use, only to render anyway, is slower than never
    /// having claimed. The mark is per route pattern because request-scoping is
    /// a property of the route's code, not of the URL.
    #[test]
    fn a_route_seen_rendering_request_state_is_not_coalesced_again() {
        let flight = RenderSingleFlight::default();

        assert!(!flight.renders_request_scoped("/account"));
        flight.mark_request_scoped("/account");
        assert!(flight.renders_request_scoped("/account"));
        assert!(
            !flight.renders_request_scoped("/pricing"),
            "one route's request-scoping says nothing about another's"
        );
    }

    /// Only one revalidation may be in flight per ISR cache key.
    #[test]
    fn isr_slot_is_exclusive_per_key() {
        let keys: IsrRevalidationSet = Arc::new(Mutex::new(HashSet::new()));
        let held = IsrRevalidationSlot::claim(&keys, "isr:/blog").unwrap();

        assert!(
            IsrRevalidationSlot::claim(&keys, "isr:/blog").is_none(),
            "a second claim on a held key must be refused"
        );
        assert!(
            IsrRevalidationSlot::claim(&keys, "isr:/docs").is_some(),
            "a different key must still be claimable"
        );

        drop(held);
        assert!(IsrRevalidationSlot::claim(&keys, "isr:/blog").is_some());
    }

    /// A revalidation that panics must still release its slot. Before the slot
    /// was tied to `Drop`, the release ran only on the happy path, so one
    /// panicking render left the key claimed forever and that route never
    /// revalidated again for the life of the process.
    #[test]
    fn isr_slot_is_released_when_the_revalidation_panics() {
        let keys: IsrRevalidationSet = Arc::new(Mutex::new(HashSet::new()));

        let panicked = std::panic::catch_unwind({
            let keys = Arc::clone(&keys);
            move || {
                let _slot = IsrRevalidationSlot::claim(&keys, "isr:/blog").unwrap();
                panic!("render failed");
            }
        });
        assert!(panicked.is_err());

        assert!(
            IsrRevalidationSlot::claim(&keys, "isr:/blog").is_some(),
            "the slot must be reclaimable after a panicking revalidation"
        );
    }

    /// A panic inside the guarded section poisons the lock. ISR revalidation
    /// must keep working: the guarded value is a key set with no invariant to
    /// break, so poison is recovered rather than propagated.
    #[test]
    fn isr_slot_survives_a_poisoned_lock() {
        let keys: IsrRevalidationSet = Arc::new(Mutex::new(HashSet::new()));
        let poisoned = std::panic::catch_unwind({
            let keys = Arc::clone(&keys);
            move || {
                let _guard = keys.lock().unwrap();
                panic!("poison the lock");
            }
        });
        assert!(poisoned.is_err());
        assert!(keys.is_poisoned());

        let slot = IsrRevalidationSlot::claim(&keys, "isr:/blog");
        assert!(slot.is_some(), "a poisoned lock must not disable ISR");
        drop(slot);
        assert!(IsrRevalidationSlot::claim(&keys, "isr:/blog").is_some());
    }

    /// Every prerendered strategy owns a distinct cache namespace. A collision
    /// would let one strategy serve another strategy's document for the same
    /// request path.
    #[test]
    fn prerender_cache_namespaces_do_not_collide() {
        let params = RouteParams::new();
        let base = render_cache::ssr_cache_key("/pricing", &params);
        let keys = [
            format!("ssg:{base}"),
            format!("isr:{base}"),
            format!("csr:{base}"),
            format!("ppr:{base}"),
        ];

        let unique = keys.iter().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), keys.len(), "prerender cache keys must differ");
    }

    /// The prerendered document must be readable from the render cache after
    /// one disk read. Deleting the file proves no second read is required:
    /// before this was shared, CSR read the shell from disk on every request
    /// and PPR consulted its cache only after the read had already returned.
    #[tokio::test]
    async fn prerendered_html_is_read_from_disk_once_then_served_from_cache() {
        let temp = tempfile::tempdir().unwrap();
        let prerender_dir = temp.path();
        let page_dir = prerender_dir.join("pricing");
        std::fs::create_dir_all(&page_dir).unwrap();
        std::fs::write(page_dir.join("index.html"), "<h1>pricing</h1>").unwrap();

        let cache = RenderCache::new(8, 60);
        let cache_key = "csr:ssr:/pricing";

        let first = store_prerendered_html(&cache, prerender_dir, "/pricing", cache_key).await;
        assert_eq!(
            first.as_ref().map(|document| document.html.as_ref()),
            Some("<h1>pricing</h1>")
        );

        std::fs::remove_dir_all(&page_dir).unwrap();

        assert_eq!(
            cache.get(cache_key).await.as_deref(),
            Some("<h1>pricing</h1>"),
            "a served prerendered document must survive in the render cache"
        );
        assert_eq!(
            read_prerendered_html(prerender_dir, "/pricing").await,
            None,
            "the test removed the file, so a second disk read would have failed"
        );
    }

    /// The whole point of the write: a `revalidatePath()` claim can only be
    /// retired once the document it invalidated is gone from disk. The reader
    /// must see the fresh bytes afterwards, because that is what makes
    /// acknowledging the claim safe.
    #[test]
    fn settling_replaces_the_build_document_the_reader_would_serve() {
        let temp = tempfile::tempdir().unwrap();
        let page_dir = temp.path().join("blog/hello");
        std::fs::create_dir_all(&page_dir).unwrap();
        std::fs::write(page_dir.join("index.html"), "<h1>stale</h1>").unwrap();

        assert!(settle_prerendered_artifact(
            temp.path(),
            "/blog/hello",
            "<h1>fresh</h1>"
        ));
        assert_eq!(
            serve_prerendered_html(temp.path(), "/blog/hello").as_deref(),
            Some("<h1>fresh</h1>"),
            "the reader must see the replacement, not the build's document"
        );
    }

    /// A path the build never prerendered already falls through to a fresh
    /// render, so its claim is settled with nothing written. Creating the file
    /// here would add a disk fallback where none existed — and for ISR it would
    /// restart the age its stale-while-revalidate window is measured from.
    #[test]
    fn settling_a_path_with_no_artifact_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();

        assert!(settle_prerendered_artifact(
            temp.path(),
            "/never-built",
            "<h1>fresh</h1>"
        ));
        assert!(
            !temp.path().join("never-built").exists(),
            "settling must not publish an artifact the build did not produce"
        );
    }

    /// A failed write must report unsettled. Reporting success would retire the
    /// claim while the stale document is still the one the reader opens, which
    /// is the one outcome this whole path exists to prevent.
    #[test]
    fn a_write_that_cannot_happen_leaves_the_claim_unsettled() {
        let temp = tempfile::tempdir().unwrap();
        // A directory where the document belongs fails both the rename and the
        // direct write inside `write_atomic`.
        std::fs::create_dir_all(temp.path().join("pricing/index.html")).unwrap();

        assert!(!settle_prerendered_artifact(
            temp.path(),
            "/pricing",
            "<h1>fresh</h1>"
        ));
    }

    /// An escaping path is rejected by the reader too, so no document of it can
    /// ever be served and nothing may be written outside the directory.
    #[test]
    fn settling_refuses_to_write_outside_the_prerender_directory() {
        let temp = tempfile::tempdir().unwrap();
        let prerender_dir = temp.path().join("prerender");
        std::fs::create_dir_all(&prerender_dir).unwrap();
        std::fs::write(temp.path().join("index.html"), "<h1>outside</h1>").unwrap();

        assert!(settle_prerendered_artifact(
            &prerender_dir,
            "/../index.html",
            "<h1>fresh</h1>"
        ));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("index.html")).unwrap(),
            "<h1>outside</h1>",
            "a rejected path must leave every file outside the directory alone"
        );
    }

    /// A missing prerendered document must not poison the cache with an entry.
    #[tokio::test]
    async fn missing_prerendered_html_is_not_cached() {
        let temp = tempfile::tempdir().unwrap();
        let cache = RenderCache::new(8, 60);

        assert!(
            store_prerendered_html(&cache, temp.path(), "/absent", "ppr:ssr:/absent")
                .await
                .is_none()
        );
        assert_eq!(cache.get("ppr:ssr:/absent").await, None);
    }

    /// The prerender read must run off the async worker thread. `spawn_blocking`
    /// requires a multi-thread runtime handle; this asserts the wrapper works
    /// under the same runtime flavor the server uses.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prerendered_read_runs_off_the_async_worker_thread() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("index.html"), "<p>root</p>").unwrap();

        assert_eq!(
            read_prerendered_html(temp.path(), "/").await.as_deref(),
            Some("<p>root</p>")
        );
    }

    /// Reading a prerendered document obeys the same path table as writing one.
    ///
    /// `settling_refuses_to_write_outside_the_prerender_directory` covers the
    /// writer. The reader is the half an unauthenticated request reaches: it
    /// turns a URL into a file path and returns the bytes. Both derive that path
    /// from `prerendered_document_path`, and both are held to
    /// `tests/fixtures/prerender-path-conformance.json` — the same table the
    /// static-asset handler and the deployed handler replay, so a request the
    /// native server refuses is refused everywhere.
    #[test]
    fn serving_a_prerendered_document_replays_the_shared_path_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/prerender-path-conformance.json"
        ))
        .unwrap();

        let temp = tempfile::tempdir().unwrap();
        let prerender_dir = temp.path().join("prerender");
        std::fs::create_dir_all(&prerender_dir).unwrap();

        for case in fixture["cases"].as_array().unwrap() {
            let path = case["path"].as_str().unwrap();
            let safe = case["safe"].as_bool().unwrap();
            let why = case["why"].as_str().unwrap_or_default();

            let resolved = prerendered_document_path(&prerender_dir, &format!("/{path}"));
            assert_eq!(resolved.is_some(), safe, "{path}: {why}");
            if let Some(resolved) = resolved {
                assert!(
                    resolved.starts_with(&prerender_dir),
                    "{path} resolved outside the prerender directory: {resolved:?}"
                );
            }
        }
    }

    /// Create a directory symlink, or report that this host will not.
    ///
    /// Windows needs a privilege that an ordinary developer session does not
    /// have, so the symlink case is skipped there rather than failing. It still
    /// runs everywhere else, which is where the check it guards would otherwise
    /// have no test at all.
    fn link_dir(target: &Path, link: &Path) -> Option<()> {
        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_dir(target, link);
        #[cfg(not(any(unix, windows)))]
        let created: std::io::Result<()> = Err(std::io::Error::other("unsupported"));
        created.ok()
    }

    /// A refused path is refused before anything is read from disk.
    ///
    /// The containment check runs on a canonical path, so it needs the file to
    /// exist; a traversal that names a real file outside the directory is the
    /// case that matters, and it has to be stopped by the rule rather than by
    /// the file happening to be absent.
    #[test]
    fn a_traversal_never_reads_a_file_outside_the_prerender_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let prerender_dir = root.join("prerender");
        std::fs::create_dir_all(prerender_dir.join("blog")).unwrap();
        std::fs::write(prerender_dir.join("blog").join("index.html"), "<p>blog</p>").unwrap();

        // A real file the traversal is aiming at, next to the directory.
        std::fs::create_dir_all(root.join("secret")).unwrap();
        std::fs::write(root.join("secret").join("index.html"), "TOP SECRET").unwrap();

        assert_eq!(
            serve_prerendered_html(&prerender_dir, "/blog").as_deref(),
            Some("<p>blog</p>"),
            "an ordinary path must still be served"
        );

        // A segment the path rule has no reason to refuse, pointing somewhere
        // else. This is the case the containment check exists for: removing the
        // rule above is caught by the conformance table, but removing
        // `contained_public_asset` is caught by nothing without a symlink.
        if let Some(()) = link_dir(&root.join("secret"), &prerender_dir.join("linked")) {
            assert!(
                serve_prerendered_html(&prerender_dir, "/linked").as_deref() != Some("TOP SECRET"),
                "a symlinked segment escaped the prerender directory"
            );
        }

        for traversal in [
            "/../secret",
            "/blog/../../secret",
            "/./../secret",
            "/..%2Fsecret",
            "/\\..\\secret",
        ] {
            let served = serve_prerendered_html(&prerender_dir, traversal);
            assert!(
                served.as_deref() != Some("TOP SECRET"),
                "{traversal} escaped the prerender directory"
            );
        }
    }
}

#[cfg(test)]
mod document_store_protocol_tests {
    use super::{
        RenderStrategy, StoredDocument, WorkerRequest, WorkerResponse, serve_stored_document,
        stored_document_from_response,
    };

    /// The rule shared with both JavaScript hosts, replayed from the fixture
    /// they replay: what a worker's `documentRead` wire shape means, and what
    /// each strategy does with it. `tests/fixtures/stored-document-conformance.json`
    /// names the three implementations and the two ways they drifted before
    /// it existed.
    #[test]
    fn stored_document_conformance() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/stored-document-conformance.json"
        ))
        .unwrap();

        let expected_document = |expect: &serde_json::Value| match expect["kind"].as_str() {
            Some("miss") => StoredDocument::Miss,
            Some("held") => StoredDocument::Held {
                html: expect["html"].as_str().unwrap().to_string(),
                stale: expect["stale"].as_bool().unwrap(),
            },
            other => panic!("unknown document kind {other:?}"),
        };

        for case in fixture["answers"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            // The wire shape is what the worker wrote; `ok` and `id` are the
            // envelope every response carries.
            let mut wire = case["wire"].clone();
            wire["id"] = serde_json::Value::String("1".to_string());
            wire["ok"] = serde_json::Value::Bool(true);
            let response: WorkerResponse = serde_json::from_value(wire).unwrap();
            assert_eq!(
                stored_document_from_response(response),
                expected_document(&case["expect"]),
                "{name}"
            );
        }

        for case in fixture["serving"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let strategy: RenderStrategy =
                serde_json::from_value(case["strategy"].clone()).unwrap();
            let served = serve_stored_document(expected_document(&case["document"]), strategy);
            let expect = &case["expect"];
            assert_eq!(
                served.is_some(),
                expect["served"].as_bool().unwrap(),
                "{name}: served"
            );
            assert_eq!(
                served.as_ref().is_some_and(|(_, refresh)| *refresh),
                expect["refresh"].as_bool().unwrap(),
                "{name}: refresh"
            );
            if let (Some((html, _)), Some(expected_html)) =
                (served, case["document"]["html"].as_str())
            {
                assert_eq!(html, expected_html, "{name}: html");
            }
        }
    }

    fn answer(html: Option<&str>, stale: Option<bool>) -> WorkerResponse {
        WorkerResponse {
            ok: true,
            html: html.map(str::to_string),
            stale,
            ..WorkerResponse::default()
        }
    }

    /// A stale document is held, not missing.
    ///
    /// The host used to fold `stale` into "the store has nothing", and nothing
    /// falls through to the build's own directory — so a cold instance served
    /// the document the build produced, which is older than the stale one the
    /// store had just answered with, and stored it under a fresh timestamp so
    /// no refresh ran for a whole window. The deployed hosts serve a stale
    /// document and refresh behind it; this is the half that lets the native
    /// host do the same.
    #[test]
    fn a_stale_document_is_held_rather_than_missing() {
        assert_eq!(
            stored_document_from_response(answer(Some("<p>old</p>"), Some(true))),
            StoredDocument::Held {
                html: "<p>old</p>".to_string(),
                stale: true,
            }
        );
    }

    /// `None` and `Some(false)` both mean fresh: a worker that predates the
    /// flag answers neither, and its documents were always served.
    #[test]
    fn a_fresh_document_is_held_with_or_without_the_flag() {
        for stale in [None, Some(false)] {
            assert_eq!(
                stored_document_from_response(answer(Some("<p>new</p>"), stale)),
                StoredDocument::Held {
                    html: "<p>new</p>".to_string(),
                    stale: false,
                }
            );
        }
    }

    /// No document is a miss whatever the flag says, and a failed response is
    /// a miss even when it carries a document — the caller has already reported
    /// the failure, and a store that could not be read is not a store to serve
    /// from.
    #[test]
    fn no_document_or_a_failed_read_is_a_miss() {
        assert_eq!(
            stored_document_from_response(answer(None, Some(true))),
            StoredDocument::Miss
        );
        assert_eq!(
            stored_document_from_response(answer(None, None)),
            StoredDocument::Miss
        );
        let failed = WorkerResponse {
            ok: false,
            ..answer(Some("<p>partial</p>"), None)
        };
        assert_eq!(stored_document_from_response(failed), StoredDocument::Miss);
    }

    /// The field names `handleDocumentRead` and `handleDocumentWrite` read.
    ///
    /// This request is the whole document half of `cache.handler` on this host,
    /// and the two sides are in different languages: a rename here is a store
    /// that silently answers nothing, because the worker would read `undefined`
    /// for the path and hand the project's handler an empty lookup. Serializing
    /// the request and reading the keys back is what holds them together —
    /// `packages/ruvyxa/runtime/worker-pool.mjs` is the other half.
    #[test]
    fn a_document_request_serializes_the_fields_the_worker_reads() {
        let read = serde_json::to_value(WorkerRequest::DocumentRead {
            id: "1".to_string(),
            pathname: "/blog/hello".to_string(),
            revalidate: Some(60),
        })
        .unwrap();
        assert_eq!(read["type"], "documentRead");
        assert_eq!(read["pathname"], "/blog/hello");
        assert_eq!(read["revalidate"], 60);

        // A route with no window omits the key rather than sending a number the
        // project never configured; `readPrerendered` takes it as optional in
        // every deployed host too.
        let windowless = serde_json::to_value(WorkerRequest::DocumentRead {
            id: "2".to_string(),
            pathname: "/".to_string(),
            revalidate: None,
        })
        .unwrap();
        assert!(windowless.get("revalidate").is_none());

        let write = serde_json::to_value(WorkerRequest::DocumentWrite {
            id: "3".to_string(),
            pathname: "/blog/hello".to_string(),
            html: "<!doctype html>".to_string(),
            revalidate: Some(30),
            forced: true,
        })
        .unwrap();
        assert_eq!(write["type"], "documentWrite");
        assert_eq!(write["html"], "<!doctype html>");
        assert_eq!(write["forced"], true);
    }
}
