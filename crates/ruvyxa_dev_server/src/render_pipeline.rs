//! Page, API, action, and client-bundle rendering: strategy dispatch
//! (SSR/SSG/ISR/CSR/PPR), worker-pool render paths, ISR revalidation, and the
//! Node/Bun render-process fallback used by `render_request`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use ruvyxa_bundler::JsxRuntime;
use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use ruvyxa_graph::{
    DiscoverOptions, RenderStrategy, RouteEntry, RouteKind, RouteManifest, RouteParams,
    discover_routes,
};
use serde::Deserialize;

use crate::html_document::{
    client_hydration_script, compose_document, error_page, hmr_client_script,
};
use crate::router::{self, RadixRouter};
use crate::static_assets::{
    contained_public_asset, is_safe_relative_path, public_asset_links, serve_client_file,
    serve_client_file_sync, serve_public_file, serve_public_file_sync,
};
use crate::worker_pool::{RenderActionRequest, RenderApiRequest, WorkerApiResponse};
use crate::{
    AppState, RuntimeCache, RuntimeTrace, ServerConfig, TraceAssets, html_response, project_env,
    with_security_headers,
};
use crate::{render_cache, style::collect_styles};

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

pub fn render_request(config: &ServerConfig, request_path: &str, method: &str) -> Result<Response> {
    render_request_cached(config, request_path, method)
}

pub(crate) fn render_request_cached(
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

/// Dispatch page rendering based on the route's declared rendering strategy.
async fn render_page_by_strategy(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
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
    params: &RouteParams,
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
    params: &RouteParams,
    styles: &str,
) -> Result<String> {
    let cache_key = format!("isr:{}", render_cache::ssr_cache_key(request_path, params));

    let revalidate_after = Duration::from_secs(route.render.revalidate.unwrap_or(60));

    // Serve stale content immediately. Only revalidate after the route's
    // configured interval, and coalesce concurrent requests for the same key.
    if let Some((cached, age)) = state.render_cache.get_stale_with_age(&cache_key).await {
        if age >= revalidate_after {
            spawn_isr_revalidation(state, route, request_path, params, styles, &cache_key);
        }
        return Ok(cached);
    }

    // In production, try the pre-rendered HTML file
    if !state.config.watch
        && let Some(html) = serve_prerendered_html(&state.config.prerender_dir, request_path)
    {
        // Store in cache. The first background revalidation waits until the
        // route's declared interval instead of firing once per request.
        state
            .render_cache
            .put(cache_key.clone(), html.clone())
            .await;
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
    params: &RouteParams,
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
    params: &RouteParams,
    styles: &str,
    cache_key: &str,
) {
    let Ok(mut in_flight) = state.isr_revalidating.try_lock() else {
        return;
    };
    if !in_flight.insert(cache_key.to_string()) {
        return;
    }
    drop(in_flight);

    let revalidate_state = state.clone();
    let revalidate_route = route.clone();
    let revalidate_path = request_path.to_string();
    let revalidate_params = params.clone();
    let revalidate_styles = styles.to_string();
    let revalidate_key = cache_key.to_string();
    let revalidating = state.isr_revalidating.clone();

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
                .put(revalidate_key.clone(), html)
                .await;
        }
        revalidating.lock().await.remove(&revalidate_key);
    });
}

/// Try to serve a pre-rendered HTML file from the prerender directory.
/// Returns `Some(html)` if the file exists, `None` otherwise.
pub(crate) fn serve_prerendered_html(prerender_dir: &Path, request_path: &str) -> Option<String> {
    let sanitized = request_path.trim_start_matches('/');
    if !sanitized.is_empty() && !is_safe_relative_path(sanitized) {
        return None;
    }
    let html_path = if sanitized.is_empty() {
        prerender_dir.join("index.html")
    } else {
        prerender_dir.join(sanitized).join("index.html")
    };

    let html_path = contained_public_asset(prerender_dir, &html_path)?;
    fs::read_to_string(html_path).ok()
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
    params: &RouteParams,
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

pub(crate) async fn render_page_pooled(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
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
pub(crate) async fn render_api_pooled(
    state: &AppState,
    route: &RouteEntry,
    request_path: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
    params: &RouteParams,
) -> Result<Response> {
    let WorkerApiResponse {
        mut response,
        body: streamed_body,
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

pub(crate) async fn render_client_bundle_pooled(
    state: &AppState,
    request_path: &str,
) -> Result<String> {
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

    let response = state
        .worker_pool
        .render_action(RenderActionRequest {
            project_root: &state.config.root,
            action_file: &action_file,
            action_name,
            payload_json,
            content_type,
            request_path,
            headers: &worker_request_headers(request_headers),
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

    let status = StatusCode::from_u16(response.status.unwrap_or(200)).unwrap_or(StatusCode::OK);
    let mut http_response = (status, response.body.unwrap_or_default()).into_response();

    if let Some(headers) = response.header_pairs.or_else(|| {
        response
            .headers
            .map(|headers| headers.into_iter().collect::<Vec<_>>())
    }) {
        for (key, value) in headers {
            let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
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

pub(crate) async fn runtime_trace_cached(
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

fn render_page(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
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

pub(crate) fn page_has_default_export(file: &Path, source: &str) -> bool {
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
    params: &RouteParams,
) -> Result<String> {
    let renderer = find_ssr_renderer(&config.root).ok_or_else(|| {
        Diagnostic::new("RUV1102", "SSR renderer was not found")
            .explain("Ruvyxa could not find the Node SSR renderer used to transform TSX and render React.")
            .suggest("Run pnpm install from the monorepo root, or install the ruvyxa package in the app.")
    })?;

    let output = javascript_command(config)?
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
            message: format!("Failed to start {} for React SSR", config.runtime.command()),
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

pub(crate) fn find_runtime_script(root: &Path, file_name: &str) -> Option<PathBuf> {
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

fn javascript_command(config: &ServerConfig) -> Result<Command> {
    let mut command = Command::new(config.runtime.executable());
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
        "RUVYXA_RUNTIME".to_string(),
        config.runtime.command().to_string(),
    );
    Ok(env)
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

    let output = javascript_command(config)?
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
            message: format!(
                "Failed to start {} for API route rendering",
                config.runtime.command()
            ),
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

pub(crate) fn action_file_for(route: &RouteEntry) -> Option<PathBuf> {
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
