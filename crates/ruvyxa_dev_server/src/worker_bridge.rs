//! Conversion layer between axum HTTP types and the project worker's wire
//! format, plus the request stages that cross to the worker: `proxy.handler`
//! and the content engine's live artifacts.

use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use ruvyxa_diagnostics::{Result, RuvyxaError};
use ruvyxa_middleware::{WireRequest, WireRequestResult, WireResponse};

use crate::AppState;

pub(crate) fn wire_response_into_response(response: WireResponse) -> Result<Response> {
    let status = StatusCode::from_u16(response.status).map_err(|error| {
        RuvyxaError::Message(format!("proxy.handler returned an invalid status: {error}"))
    })?;
    let body = decode_body(response.body_base64.as_deref())?.unwrap_or_default();
    // Construct the body directly so Axum does not inject a synthetic
    // content-type that would be duplicated when the handler's header pairs
    // are appended below.
    let mut output = Response::new(Body::from(body));
    *output.status_mut() = status;
    for (name, value) in response.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            RuvyxaError::Message(format!(
                "proxy.handler returned an invalid header name: {error}"
            ))
        })?;
        let value = HeaderValue::from_str(&value).map_err(|error| {
            RuvyxaError::Message(format!(
                "proxy.handler returned an invalid header value: {error}"
            ))
        })?;
        // Preserve repeated response fields such as `Set-Cookie`. The wire
        // format is a pair list, so replacing earlier values here would lose
        // valid HTTP semantics at the JavaScript/Rust boundary.
        output.headers_mut().append(name, value);
    }
    Ok(output)
}

pub(crate) fn encode_body(body: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(body)
}

pub(crate) fn decode_body(value: Option<&str>) -> Result<Option<Vec<u8>>> {
    use base64::Engine;
    value
        .map(|value| {
            base64::engine::general_purpose::STANDARD
                .decode(value)
                .map_err(|error| {
                    RuvyxaError::Message(format!("RUV1701 invalid wire body: {error}"))
                })
        })
        .transpose()
}

/// Validate the method and request target `proxy.handler` forwarded, and
/// canonicalize the path the way routing does.
pub(crate) fn split_wire_target(method: &str, target: &str) -> Result<(String, String)> {
    let method = method.parse::<Method>().map_err(|error| {
        RuvyxaError::Message(format!(
            "RUV1701 proxy.handler returned an invalid method: {error}"
        ))
    })?;
    let uri = target.parse::<Uri>().map_err(|error| {
        RuvyxaError::Message(format!(
            "RUV1701 proxy.handler returned an invalid request target: {error}"
        ))
    })?;
    let Some(path_and_query) = uri.path_and_query() else {
        return Err(RuvyxaError::Message(
            "RUV1701 proxy.handler returned a request target without a path.".to_string(),
        ));
    };
    if path_and_query.as_str() != target {
        return Err(RuvyxaError::Message(
            "RUV1701 proxy.handler returned a request target that is not an absolute application path."
                .to_string(),
        ));
    }
    let path = canonical_request_path(uri.path()).map_err(|error| {
        RuvyxaError::Message(format!(
            "RUV1701 proxy.handler returned an unsafe request path: {error}"
        ))
    })?;
    let target = match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    Ok((method.to_string(), target))
}

/// Decode each URI path segment without allowing encoded bytes to introduce a
/// new path boundary or filesystem traversal component.
pub(crate) fn canonical_request_path(raw_path: &str) -> Result<String> {
    if !raw_path.starts_with('/') {
        return Err(RuvyxaError::Message(
            "request path must start with '/'.".to_string(),
        ));
    }

    let mut segments = Vec::new();
    for segment in raw_path.split('/').filter(|segment| !segment.is_empty()) {
        let decoded = decode_path_segment(segment)?;
        if decoded.is_empty()
            || matches!(decoded.as_str(), "." | "..")
            || decoded.contains(['/', '\\'])
            || decoded.chars().any(char::is_control)
        {
            return Err(RuvyxaError::Message(
                "request path contains an unsafe segment.".to_string(),
            ));
        }
        segments.push(decoded);
    }

    Ok(if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    })
}

fn decode_path_segment(segment: &str) -> Result<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }

        let Some(high) = bytes.get(index + 1).and_then(|byte| hex_value(*byte)) else {
            return Err(RuvyxaError::Message(
                "request path contains malformed percent encoding.".to_string(),
            ));
        };
        let Some(low) = bytes.get(index + 2).and_then(|byte| hex_value(*byte)) else {
            return Err(RuvyxaError::Message(
                "request path contains malformed percent encoding.".to_string(),
            ));
        };
        decoded.push((high << 4) | low);
        index += 3;
    }

    String::from_utf8(decoded).map_err(|_| {
        RuvyxaError::Message("request path contains invalid UTF-8 encoding.".to_string())
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn wire_headers_to_map(headers: &[(String, String)]) -> HeaderMap {
    let mut output = HeaderMap::new();
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            output.append(name, value);
        }
    }
    output
}

pub(crate) fn request_method_allows_body(method: &str) -> bool {
    !method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD")
}

/// The request target handed to the worker: the canonical path routing
/// resolved, with the request's own query carried along.
///
/// `handle_request` routes on the canonical form and hands over the same one,
/// so `//api/x` cannot slip past a `proxy.matcher` written for `/api/x`.
pub(crate) fn wire_target(request_path: &str, request_target: &str) -> String {
    match request_target.split_once('?') {
        Some((_, query)) => format!("{request_path}?{query}"),
        None => request_path.to_string(),
    }
}

/// A request as it stands between the stages `handle_request` runs: the pieces
/// routing needs, owned, so a stage can hand back a changed one.
pub(crate) struct ForwardedRequest {
    pub(crate) method: String,
    /// Canonical path, no query.
    pub(crate) request_path: String,
    /// Path with its query, as an API handler's `Request` sees it.
    pub(crate) request_target: String,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Option<Vec<u8>>,
    /// `headers` as the route-rule evaluator reads them, built at most once.
    ///
    /// Three stages of one request ask for the same list — `headers()` and
    /// `redirects()`, the `proxy.matcher`, and the `beforeFiles` rewrite — and
    /// each used to build its own `Vec<(String, String)>` over every header.
    ///
    /// It lives *on* the request rather than beside it so it cannot describe
    /// headers that are no longer the request's: `proxy.handler` answering
    /// with a different `Request` builds a whole new `ForwardedRequest`, whose
    /// list starts empty. A rewrite that changes only the path leaves it valid,
    /// because the path is not what it holds.
    ///
    /// `OnceLock` rather than `OnceCell` because this is borrowed across the
    /// `await` on the worker, and rather than an eager build because a project
    /// that configures no rule and no proxy asks for the list zero times and
    /// must not pay for it.
    rule_headers: OnceLock<Vec<(String, String)>>,
}

impl ForwardedRequest {
    pub(crate) fn new(
        method: String,
        request_path: String,
        request_target: String,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
    ) -> Self {
        Self {
            method,
            request_path,
            request_target,
            headers,
            body,
            rule_headers: OnceLock::new(),
        }
    }

    /// The header pairs every rule and matcher on this request reads.
    pub(crate) fn rule_headers(&self) -> &[(String, String)] {
        self.rule_headers
            .get_or_init(|| ruvyxa_middleware::route_rules::header_pairs(&self.headers))
    }

    /// This request as a rule, a matcher, or a rewrite reads it.
    ///
    /// One constructor rather than the same four fields wired up at each of
    /// the three call sites: `query` is taken from the target and `path` from
    /// the canonical path, and a stage that mixed the two would match rules
    /// against a string routing never resolved.
    pub(crate) fn rule_request(&self) -> ruvyxa_middleware::RuleRequest<'_> {
        ruvyxa_middleware::RuleRequest {
            path: &self.request_path,
            query: self.request_target.split_once('?').map(|(_, query)| query),
            headers: self.rule_headers(),
            host: ruvyxa_middleware::route_rules::host_header(&self.headers),
        }
    }
}

/// Run the config's `proxy.handler` on a request its matcher names.
///
/// The matcher is evaluated natively first, so a request it does not name
/// never crosses to the worker. A handler that returns a `Response` answers
/// the request; one that returns a `Request` forwards it — a different path is
/// a rewrite, different headers are carried — and one that returns nothing
/// forwards the request unchanged.
///
/// Answers with `Some(response)`, or with `None` after leaving `current` as the
/// request the remaining stages continue with: untouched, or replaced whole by
/// the one the handler built. The request is borrowed rather than moved through
/// a two-variant outcome, because carrying it back by value made "continue" the
/// large half of an enum every request passes through, and boxing that half
/// would have put a heap allocation on the path of every request that
/// configures no proxy at all.
pub(crate) async fn run_proxy_stage(
    state: &AppState,
    current: &mut ForwardedRequest,
) -> Option<Box<Response>> {
    let (Some(proxy), Some(worker)) = (state.config.proxy.as_ref(), state.worker.as_deref()) else {
        return None;
    };
    if !proxy.wants(&current.rule_request()) {
        return None;
    }
    let wire = WireRequest {
        method: current.method.clone(),
        path: wire_target(&current.request_path, &current.request_target),
        // Copied rather than moved: the list belongs to the request, and the
        // request outlives this call whenever the handler forwards it.
        headers: current.rule_headers().to_vec(),
        body_base64: current.body.as_deref().map(encode_body),
    };
    match worker.execute_proxy(&wire).await {
        Err(error) => {
            tracing::error!(%error, path = %current.request_path, "proxy.handler failed");
            Some(Box::new(internal_error(state, &error)))
        }
        Ok(WireRequestResult::Response { response }) => match wire_response_into_response(response)
        {
            Ok(response) => Some(Box::new(response)),
            Err(error) => {
                tracing::error!(%error, "proxy.handler returned an unusable response");
                Some(Box::new(internal_error(state, &error)))
            }
        },
        Ok(WireRequestResult::Request { request }) => match forwarded_request(request) {
            Ok(next) => {
                *current = next;
                None
            }
            Err(response) => Some(response),
        },
    }
}

/// Answer a content-engine path live, in development.
///
/// `ruvyxa build` writes `/content.json` and its siblings under `assets/`, and
/// a production server serves those files. Under `ruvyxa dev` nothing has
/// been built, so the worker derives the artifact from the source tree on
/// request — behind `no-cache`, because the developer is editing that tree.
pub(crate) async fn run_content_stage(state: &AppState, request_path: &str) -> Option<Response> {
    if !state.config.watch {
        return None;
    }
    let worker = state.worker.as_deref()?;
    if !worker.descriptor().serves_content(request_path) {
        return None;
    }
    match worker.content_artifact(request_path).await {
        Ok(Some(artifact)) => {
            let mut response = Response::new(Body::from(artifact.body));
            if let Ok(value) = HeaderValue::from_str(&artifact.content_type) {
                response
                    .headers_mut()
                    .insert(axum::http::header::CONTENT_TYPE, value);
            }
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            );
            Some(response)
        }
        Ok(None) => None,
        Err(error) => {
            tracing::error!(%error, path = %request_path, "content engine failed");
            Some(internal_error(state, &error))
        }
    }
}

fn internal_error(state: &AppState, error: &RuvyxaError) -> Response {
    let message = crate::html_document::public_internal_error(&state.config, error);
    crate::response::with_security_headers(
        (StatusCode::INTERNAL_SERVER_ERROR, message).into_response(),
    )
}

/// Decode the request `proxy.handler` forwarded back into the pieces routing
/// needs.
fn forwarded_request(next: WireRequest) -> std::result::Result<ForwardedRequest, Box<Response>> {
    let refuse = |error: RuvyxaError| {
        Box::new(crate::response::with_security_headers(
            (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
        ))
    };
    let (method, request_target) = split_wire_target(&next.method, &next.path).map_err(refuse)?;
    let request_path = request_target
        .split_once('?')
        .map_or_else(|| request_target.clone(), |(path, _)| path.to_string());
    let body = decode_body(next.body_base64.as_deref()).map_err(refuse)?;
    Ok(ForwardedRequest::new(
        method,
        request_path,
        request_target,
        wire_headers_to_map(&next.headers),
        body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path handed to the worker is the canonical one routing resolved,
    /// with the query carried along — so a matcher written for `/api/x` sees
    /// `//api/x` and `/api/%78` as what they are.
    #[test]
    fn the_worker_is_handed_the_same_path_the_router_resolves() {
        assert!(
            canonical_request_path("/a/./b").is_err(),
            "a dot segment must be refused before the worker sees it"
        );
        for (target, expected) in [("//api/x", "/api/x"), ("/api/%78", "/api/x")] {
            let canonical = canonical_request_path(target).unwrap();
            assert_eq!(canonical, expected);
            assert_eq!(wire_target(&canonical, target), expected);
            assert_eq!(
                wire_target(&canonical, &format!("{target}?page=2&q=a%20b")),
                format!("{expected}?page=2&q=a%20b")
            );
        }
    }

    #[test]
    fn forwarded_rewrites_use_the_same_canonical_path_contract_as_routing() {
        assert_eq!(
            split_wire_target("PATCH", "/caf%C3%A9?from=proxy").unwrap(),
            ("PATCH".to_string(), "/café?from=proxy".to_string())
        );

        for target in [
            "https://example.test/admin",
            "/admin/%2Fprivate",
            "/admin/%5Cprivate",
            "/admin/%2E%2E",
            "/admin/%",
        ] {
            assert!(
                split_wire_target("GET", target).is_err(),
                "{target} must be rejected"
            );
        }
    }

    /// One request builds one header list, and a request the proxy replaced
    /// builds its own.
    ///
    /// Three stages read this list on every request that configures a rule or
    /// a proxy — `headers()`/`redirects()`, the matcher, and the `beforeFiles`
    /// rewrite — and each used to build its own copy over every header. Sharing
    /// it is only safe because it lives on the request: a handler that answers
    /// with a different `Request` produces a different `ForwardedRequest`, so
    /// there is no list to go stale.
    #[test]
    fn the_rule_header_list_is_built_once_and_belongs_to_its_request() {
        let mut headers = HeaderMap::new();
        headers.insert("x-team", HeaderValue::from_static("blue"));
        let request = ForwardedRequest::new(
            "GET".to_string(),
            "/a".to_string(),
            "/a?q=1".to_string(),
            headers,
            None,
        );

        let first = request.rule_headers();
        assert_eq!(first, [("x-team".to_string(), "blue".to_string())]);
        assert!(
            std::ptr::eq(first, request.rule_headers()),
            "asking twice must not build the list twice"
        );

        let rule = request.rule_request();
        assert_eq!(rule.path, "/a");
        assert_eq!(rule.query, Some("q=1"));
        assert_eq!(rule.headers, first);

        let replaced = forwarded_request(WireRequest {
            method: "GET".to_string(),
            path: "/b".to_string(),
            headers: vec![("x-team".to_string(), "red".to_string())],
            body_base64: None,
        })
        .expect("a forwarded request with a valid target");
        assert_eq!(
            replaced.rule_headers(),
            [("x-team".to_string(), "red".to_string())],
            "the replacement reads its own headers, not the ones it replaced"
        );
    }

    #[test]
    fn wire_responses_reject_invalid_headers_and_keep_repeated_ones() {
        let broken = WireResponse {
            status: 200,
            headers: vec![("bad header".to_string(), "value".to_string())],
            body_base64: Some(encode_body(b"body")),
        };
        assert!(wire_response_into_response(broken).is_err());

        let response = wire_response_into_response(WireResponse {
            status: 200,
            headers: vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("set-cookie".to_string(), "session=one; Path=/".to_string()),
                ("set-cookie".to_string(), "theme=dark; Path=/".to_string()),
            ],
            body_base64: None,
        })
        .unwrap();
        let cookies = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(cookies, vec!["session=one; Path=/", "theme=dark; Path=/"]);
        assert_eq!(response.headers().get_all("content-type").iter().count(), 1);
    }
}
