//! Conversion layer between axum HTTP types and the plugin middleware wire
//! format, plus the request/response plugin application entry points.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Body, BodyDataStream, Bytes, HttpBody};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::Response;
use futures_core::Stream;
use ruvyxa_diagnostics::{Result, RuvyxaError};
use ruvyxa_middleware::{PluginHttpRequest, PluginHttpRequestResult, PluginHttpResponse};

use crate::AppState;

pub(crate) async fn apply_request_plugins(
    state: &AppState,
    request: PluginHttpRequest,
) -> Result<(Option<Response>, PluginHttpRequest)> {
    let Some(runtime) = &state.plugin_runtime else {
        return Ok((None, request));
    };
    match runtime.execute_request(&request).await? {
        PluginHttpRequestResult::Response { response } => {
            Ok((Some(plugin_response_into_response(response)?), request))
        }
        PluginHttpRequestResult::Request { request } => Ok((None, request)),
    }
}

pub(crate) async fn apply_response_plugins(
    state: &AppState,
    request: &PluginHttpRequest,
    response: Response,
) -> Result<Response> {
    let Some(runtime) = &state.plugin_runtime else {
        return Ok(response);
    };
    if runtime.descriptor().http.response == 0 {
        return Ok(response);
    }
    let limit_bytes = state.config.plugin_response_body_limit_bytes;
    let (parts, body) = response.into_parts();
    // A body over the buffer limit used to surface as a 500. Pass it through
    // untouched instead: serving the response beats failing it, and only this
    // response skips the middleware. Sized bodies are detected up front;
    // unsized (streaming) bodies buffer chunk-by-chunk and reassemble on
    // overflow so nothing read so far is lost.
    if body_exceeds_plugin_limit(&body, limit_bytes) {
        warn_plugin_limit_skip(limit_bytes, &request.path);
        return Ok(Response::from_parts(parts, body));
    }
    let body = match buffer_plugin_response_body(body, limit_bytes).await? {
        BufferedPluginBody::Buffered(bytes) => bytes,
        BufferedPluginBody::Oversized(body) => {
            warn_plugin_limit_skip(limit_bytes, &request.path);
            return Ok(Response::from_parts(parts, body));
        }
    };
    let plugin_response = PluginHttpResponse {
        status: parts.status.as_u16(),
        headers: headers_to_plugin_pairs(&parts.headers),
        // `None` rather than an empty string: a zero-length body still *is* a
        // body once it crosses the bridge, and `new Response(<empty>, { status:
        // 204 })` throws — so every 204, 205, and 304 answered by a project
        // with any response plugin became a 500 saying
        // "Invalid response status code 204".
        body_base64: (!body.is_empty()).then(|| encode_plugin_body(&body)),
    };
    let result = runtime.execute_response(request, &plugin_response).await?;
    plugin_response_into_response(result)
}

pub(crate) fn body_exceeds_plugin_limit(body: &Body, limit_bytes: usize) -> bool {
    body.size_hint()
        .exact()
        .is_some_and(|size| size > limit_bytes as u64)
}

fn warn_plugin_limit_skip(limit_bytes: usize, path: &str) {
    tracing::warn!(
        limit_bytes,
        path = %path,
        "response body exceeds the plugin buffer limit; skipping response middleware for this response"
    );
}

/// Outcome of buffering a response body for the plugin round-trip.
#[derive(Debug)]
pub(crate) enum BufferedPluginBody {
    /// The whole body fit within the limit and is ready for base64 transport.
    Buffered(Bytes),
    /// The body overflowed the limit. Carries a reassembled body — the chunks
    /// read so far replayed in front of the untouched remainder — so the
    /// response can be served as-is instead of failing.
    Oversized(Body),
}

pub(crate) async fn buffer_plugin_response_body(
    body: Body,
    limit_bytes: usize,
) -> Result<BufferedPluginBody> {
    let mut stream = body.into_data_stream();
    let mut chunks: VecDeque<Bytes> = VecDeque::new();
    let mut total = 0_usize;
    loop {
        let next = std::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await;
        match next {
            None => break,
            Some(Err(error)) => {
                return Err(RuvyxaError::Message(format!(
                    "Failed to read the response body for response plugins: {error}"
                )));
            }
            Some(Ok(chunk)) => {
                total = total.saturating_add(chunk.len());
                chunks.push_back(chunk);
                if total > limit_bytes {
                    return Ok(BufferedPluginBody::Oversized(Body::from_stream(
                        ReplayThenStream {
                            replay: chunks,
                            rest: Some(stream),
                        },
                    )));
                }
            }
        }
    }
    if chunks.len() == 1 {
        return Ok(BufferedPluginBody::Buffered(
            chunks.pop_front().unwrap_or_default(),
        ));
    }
    let mut buffered = Vec::with_capacity(total);
    for chunk in chunks {
        buffered.extend_from_slice(&chunk);
    }
    Ok(BufferedPluginBody::Buffered(Bytes::from(buffered)))
}

/// Replays already-buffered chunks, then continues with the remaining stream.
struct ReplayThenStream {
    replay: VecDeque<Bytes>,
    rest: Option<BodyDataStream>,
}

impl Stream for ReplayThenStream {
    type Item = std::result::Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(chunk) = self.replay.pop_front() {
            return Poll::Ready(Some(Ok(chunk)));
        }
        match self.rest.as_mut() {
            Some(rest) => match Pin::new(rest).poll_next(cx) {
                Poll::Ready(None) => {
                    self.rest = None;
                    Poll::Ready(None)
                }
                other => other,
            },
            None => Poll::Ready(None),
        }
    }
}

pub(crate) fn plugin_response_into_response(response: PluginHttpResponse) -> Result<Response> {
    let status = StatusCode::from_u16(response.status).map_err(|error| {
        RuvyxaError::Message(format!("Plugin returned invalid status: {error}"))
    })?;
    let body = decode_plugin_body(response.body_base64.as_deref())?.unwrap_or_default();
    // Construct the body directly so Axum does not inject a synthetic
    // content-type that would be duplicated when the plugin's header pairs
    // are appended below.
    let mut output = Response::new(Body::from(body));
    *output.status_mut() = status;
    for (name, value) in response.headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
            RuvyxaError::Message(format!("Plugin returned invalid header name: {error}"))
        })?;
        let value = HeaderValue::from_str(&value).map_err(|error| {
            RuvyxaError::Message(format!("Plugin returned invalid header value: {error}"))
        })?;
        // Preserve repeated response fields such as `Set-Cookie`. The wire
        // format is a pair list, so replacing earlier values here would lose
        // valid HTTP semantics at the JavaScript/Rust boundary.
        output.headers_mut().append(name, value);
    }
    Ok(output)
}

pub(crate) fn headers_to_plugin_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
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

pub(crate) fn encode_plugin_body(body: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(body)
}

pub(crate) fn decode_plugin_body(value: Option<&str>) -> Result<Option<Vec<u8>>> {
    use base64::Engine;
    value
        .map(|value| {
            base64::engine::general_purpose::STANDARD
                .decode(value)
                .map_err(|error| {
                    RuvyxaError::Message(format!("RUV1701 invalid plugin body: {error}"))
                })
        })
        .transpose()
}

pub(crate) fn split_plugin_target(method: &str, target: &str) -> Result<(String, String)> {
    let method = method.parse::<Method>().map_err(|error| {
        RuvyxaError::Message(format!("RUV1701 plugin returned invalid method: {error}"))
    })?;
    let uri = target.parse::<Uri>().map_err(|error| {
        RuvyxaError::Message(format!(
            "RUV1701 plugin returned an invalid request target: {error}"
        ))
    })?;
    let Some(path_and_query) = uri.path_and_query() else {
        return Err(RuvyxaError::Message(
            "RUV1701 plugin returned a request target without a path.".to_string(),
        ));
    };
    if path_and_query.as_str() != target {
        return Err(RuvyxaError::Message(
            "RUV1701 plugin returned a request target that is not an absolute application path."
                .to_string(),
        ));
    }
    let path = canonical_request_path(uri.path()).map_err(|error| {
        RuvyxaError::Message(format!(
            "RUV1701 plugin returned an unsafe request path: {error}"
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

pub(crate) fn plugin_headers(headers: &[(String, String)]) -> HeaderMap {
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

#[cfg(test)]
mod tests {
    use super::split_plugin_target;

    #[test]
    fn plugin_request_rewrites_use_the_same_canonical_path_contract_as_routing() {
        assert_eq!(
            split_plugin_target("PATCH", "/caf%C3%A9?from=plugin").unwrap(),
            ("PATCH".to_string(), "/café?from=plugin".to_string())
        );

        for target in [
            "https://example.test/admin",
            "/admin/%2Fprivate",
            "/admin/%5Cprivate",
            "/admin/%2E%2E",
            "/admin/%",
        ] {
            assert!(
                split_plugin_target("GET", target).is_err(),
                "{target} must be rejected"
            );
        }
    }
}
