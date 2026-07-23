//! Server-action request validation: origin/fetch-metadata checks, payload
//! parsing, and the per-key rate limiter.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::{ActionQuery, ServerConfig};

/// Highest number of distinct rate-limit keys tracked before pruning expired ones.
const MAX_TRACKED_ACTION_RATE_LIMIT_KEYS: usize = 10_000;

pub(crate) struct ActionRateLimiter {
    pub(crate) hits: HashMap<String, Vec<Instant>>,
    max_hits: usize,
    window: Duration,
    pub(crate) max_keys: usize,
}

impl ActionRateLimiter {
    pub(crate) fn new(max_hits: usize, window: Duration) -> Self {
        Self {
            hits: HashMap::new(),
            max_hits,
            window,
            max_keys: MAX_TRACKED_ACTION_RATE_LIMIT_KEYS,
        }
    }

    pub(crate) fn allow(&mut self, key: &str) -> bool {
        let now = Instant::now();
        if let Some(hits) = self.hits.get_mut(key) {
            hits.retain(|hit| now.duration_since(*hit) <= self.window);
            if !hits.is_empty() {
                if hits.len() >= self.max_hits {
                    return false;
                }
                hits.push(now);
                return true;
            }
        }
        // The current key has no active requests. Remove its empty bucket
        // before considering the bounded set of client keys.
        self.hits.remove(key);

        if self.hits.len() >= self.max_keys {
            // A full sweep is only necessary when admitting a new key at
            // capacity. Keeping it off the normal request path avoids an
            // O(tracked keys) scan for every action request.
            self.remove_expired_keys(now);
        }
        if self.hits.len() >= self.max_keys {
            return false;
        }

        self.hits.insert(key.to_string(), vec![now]);
        true
    }

    fn remove_expired_keys(&mut self, now: Instant) {
        self.hits.retain(|_, hits| {
            hits.retain(|hit| now.duration_since(*hit) <= self.window);
            !hits.is_empty()
        });
    }

    pub(crate) fn retry_after_seconds(&self, key: &str) -> u64 {
        self.hits
            .get(key)
            .and_then(|hits| hits.first())
            .map(|first| self.window.saturating_sub(first.elapsed()).as_secs().max(1))
            .unwrap_or(1)
    }
}

pub(crate) fn validate_action_request(
    headers: &HeaderMap,
    body_len: usize,
    config: &ServerConfig,
    peer: SocketAddr,
) -> Option<Response> {
    if body_len > config.action_body_limit_bytes {
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

    if config.same_origin_actions && action_origin_is_cross_site(headers, config, peer.ip()) {
        return Some(
            (StatusCode::FORBIDDEN, "Cross-origin action request blocked").into_response(),
        );
    }

    if config.fetch_metadata_actions && action_fetch_site_is_cross_site(headers) {
        return Some((StatusCode::FORBIDDEN, "Cross-site action request blocked").into_response());
    }

    None
}

pub(crate) fn action_content_type_is_supported(headers: &HeaderMap) -> bool {
    action_content_type(headers).is_some()
}

fn action_content_type(headers: &HeaderMap) -> Option<&'static str> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())?
        .trim();

    if content_type.eq_ignore_ascii_case("application/json") {
        Some("application/json")
    } else if content_type.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        Some("application/x-www-form-urlencoded")
    } else {
        None
    }
}

pub(crate) fn validate_action_payload(
    headers: &HeaderMap,
    body: &[u8],
) -> std::result::Result<(&'static str, String), Box<Response>> {
    let Some(content_type) = action_content_type(headers) else {
        return Err(Box::new(
            (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Action payload must declare JSON or URL-encoded form data",
            )
                .into_response(),
        ));
    };
    let payload = std::str::from_utf8(body).map_err(|_| {
        Box::new(
            (
                StatusCode::BAD_REQUEST,
                "Action payload must be valid UTF-8",
            )
                .into_response(),
        )
    })?;
    let payload = if payload.is_empty() && content_type == "application/json" {
        "{}".to_string()
    } else {
        payload.to_string()
    };

    if content_type == "application/json"
        && let Err(error) = serde_json::from_str::<serde_json::Value>(&payload)
    {
        return Err(Box::new(
            (
                StatusCode::BAD_REQUEST,
                format!("Action JSON payload is malformed: {error}"),
            )
                .into_response(),
        ));
    }

    Ok((content_type, payload))
}

pub(crate) fn action_origin_is_cross_site(
    headers: &HeaderMap,
    config: &ServerConfig,
    peer: IpAddr,
) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        // Modern browsers send either Origin or Fetch Metadata. Fail closed
        // when both are absent; otherwise a stripped-origin cross-site form can
        // reach a mutation endpoint with no same-origin evidence.
        return !headers
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("same-origin"));
    };
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    let Some((origin_scheme, origin_host)) = origin
        .split_once("://")
        .filter(|(_, value)| !value.contains('/') && !value.is_empty())
    else {
        return true;
    };

    let expected_scheme = if is_trusted_proxy_ip(config, peer) {
        headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| matches!(*value, "http" | "https"))
            .unwrap_or("http")
    } else {
        "http"
    };

    !origin_host.eq_ignore_ascii_case(host) || !origin_scheme.eq_ignore_ascii_case(expected_scheme)
}

/// Cross-site check for the HMR WebSocket handshake.
///
/// Browsers always send `Origin` on WebSocket upgrades, so a missing header
/// means a non-browser client (curl, tooling) and is allowed; a present
/// header must match the request host exactly like the action endpoint.
/// Without this, any web page open in the developer's browser can connect to
/// the HMR socket and read changed file paths and route patterns
/// (cross-site WebSocket hijacking).
pub(crate) fn hmr_origin_is_cross_site(
    headers: &HeaderMap,
    config: &ServerConfig,
    peer: IpAddr,
) -> bool {
    if headers.get(header::ORIGIN).is_none() {
        return action_fetch_site_is_cross_site(headers);
    }
    action_origin_is_cross_site(headers, config, peer)
}

pub(crate) fn action_fetch_site_is_cross_site(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
}

pub(crate) fn action_rate_limit_key(
    peer: SocketAddr,
    headers: &HeaderMap,
    query: &ActionQuery,
    config: &ServerConfig,
) -> String {
    let peer_ip = peer.ip();

    // Forwarded identity is untrusted unless the direct peer is loopback or
    // explicitly allowlisted. Private ranges alone are not a trust boundary:
    // a LAN client can otherwise forge X-Forwarded-For and bypass the limiter.
    let client = if is_trusted_proxy_ip(config, peer_ip) {
        forwarded_client_ip(config, headers).unwrap_or(peer_ip)
    } else {
        peer_ip
    };

    format!("{client}:{}:{}", query.path, query.name)
}

/// Pick the client IP from forwarded headers, scanning from the right.
///
/// Each proxy appends the peer it actually saw, so rightmost entries are
/// proxy-written while leftmost entries arrive from the client and are
/// forgeable. Taking the leftmost entry would let a client behind a trusted
/// proxy rotate fabricated addresses through the rate limiter; instead, skip
/// trusted proxy addresses from the right and use the first address that is
/// not one of ours.
fn forwarded_client_ip(config: &ServerConfig, headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|value| value.to_str().ok())
        .into_iter()
        .flat_map(|value| value.split(',').rev())
        .filter_map(|value| value.trim().parse::<IpAddr>().ok())
        .find(|candidate| !is_trusted_proxy_ip(config, *candidate))
}

fn is_trusted_proxy_ip(config: &ServerConfig, ip: IpAddr) -> bool {
    ip.is_loopback() || config.trusted_proxy_ips.contains(&ip)
}
