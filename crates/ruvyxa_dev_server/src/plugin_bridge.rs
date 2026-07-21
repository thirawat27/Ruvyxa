//! Conversion layer between axum HTTP types and the plugin middleware wire
//! format, plus the request/response plugin application entry points.

use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use ruvyxa_diagnostics::{Result, RuvyxaError};
use ruvyxa_middleware::{MiddlewareRequestResult, PluginHttpRequest, PluginHttpResponse};

use crate::AppState;

pub(crate) async fn apply_request_plugins(
    state: &AppState,
    request: PluginHttpRequest,
) -> Result<(Option<Response>, PluginHttpRequest)> {
    let Some(runtime) = &state.plugin_runtime else {
        return Ok((None, request));
    };
    match runtime.execute_request(&request).await? {
        MiddlewareRequestResult::Response { response } => {
            Ok((Some(plugin_response_into_response(response)?), request))
        }
        MiddlewareRequestResult::Request { request } => Ok((None, request)),
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
    if runtime.descriptor().middleware.response == 0 {
        return Ok(response);
    }
    let (parts, body) = response.into_parts();
    let body =
        read_plugin_response_body(body, state.config.plugin_response_body_limit_bytes).await?;
    let plugin_response = PluginHttpResponse {
        status: parts.status.as_u16(),
        headers: headers_to_plugin_pairs(&parts.headers),
        body_base64: Some(encode_plugin_body(&body)),
    };
    let result = runtime.execute_response(request, &plugin_response).await?;
    plugin_response_into_response(result)
}

pub(crate) async fn read_plugin_response_body(body: Body, limit_bytes: usize) -> Result<Bytes> {
    to_bytes(body, limit_bytes).await.map_err(|error| {
        RuvyxaError::Message(format!(
            "Response exceeds the {limit_bytes}-byte limit for response plugins: {error}"
        ))
    })
}

pub(crate) fn plugin_response_into_response(response: PluginHttpResponse) -> Result<Response> {
    let status = StatusCode::from_u16(response.status).map_err(|error| {
        RuvyxaError::Message(format!("Plugin returned invalid status: {error}"))
    })?;
    let body = decode_plugin_body(response.body_base64.as_deref())?.unwrap_or_default();
    let mut output = (status, body).into_response();
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
    if !target.starts_with('/') {
        return Err(RuvyxaError::Message(
            "RUV1701 plugin returned a path that does not start with '/'.".to_string(),
        ));
    }
    Ok((method.to_string(), target.to_string()))
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
