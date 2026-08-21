//! Server-action request validation: origin/fetch-metadata checks, payload
//! parsing, and the per-key rate limiter.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use ruvyxa_middleware::client_ip;
pub use ruvyxa_middleware::client_ip::{IpPrefix, TrustedProxies};

use crate::{ActionQuery, ServerConfig};

const ACTION_NONCE_TTL: Duration = Duration::from_secs(600);
const ACTION_NONCE_MAX_ENTRIES: usize = 10_000;

/// Live nonces one client address may hold, a tenth of the whole pool.
///
/// The rate limiter in front of this guard is keyed per client *and per path
/// and action*, so a client that spreads its requests over two actions gets two
/// fresh buckets while the nonce pool stays one — enough to reach global
/// saturation alone, which refuses every other client's actions for a TTL. The
/// quota bounds one address to a share, so saturating the pool now takes ten
/// distinct addresses rather than one.
///
/// A tenth is a deliberate trade. A NAT'd office shares one address, and at
/// this bound they collectively get 1,000 in-flight versioned actions per TTL
/// before the guard starts refusing them; the alternative was letting any one
/// address refuse everyone.
const ACTION_NONCE_MAX_PER_CLIENT: usize = ACTION_NONCE_MAX_ENTRIES / 10;

/// Process-local replay protection for version-bound action requests.
///
/// Two structures over one set of keys: `seen` answers the replay question,
/// and `order` holds the same keys in expiry order. Every nonce is stored with
/// the same TTL, so insertion order *is* expiry order and the sweep stops at
/// the first live entry. A single map keyed by nonce could not do that — the
/// previous `retain` walked all `ACTION_NONCE_MAX_ENTRIES` on every action, and
/// eviction then scanned again for the minimum.
///
/// Mirrored by `consumeActionNonce` in
/// `packages/ruvyxa/runtime/serverless-handler.mjs`; both replay
/// `tests/fixtures/action-contract.json`.
#[derive(Debug, Default)]
pub(crate) struct ActionReplayGuard {
    seen: HashSet<String>,
    order: VecDeque<NonceEntry>,
    /// Live entry count per client, kept level with `order` by the same sweep.
    /// An address drops out of the map when its last nonce expires, so this
    /// cannot outgrow the pool it accounts for.
    per_client: HashMap<IpAddr, usize>,
}

#[derive(Debug)]
struct NonceEntry {
    expires: Instant,
    key: String,
    client: IpAddr,
}

/// Why the replay guard refused a versioned action request.
///
/// The status travels with the rejection instead of being re-derived at the
/// call site. It used to be recovered by comparing the message text against
/// string literals repeated in `handle_action`, so rewording a message here
/// silently answered `400` where `tests/fixtures/action-contract.json` pins
/// `503` — a drift no test could catch, because the fixture's `status` was
/// replayed by the serverless handler's suite and by nothing on this side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionReplayRejection {
    InvalidNonce,
    Replayed,
    ClientSaturated,
    Saturated,
}

impl ActionReplayRejection {
    pub(crate) fn status(self) -> StatusCode {
        match self {
            Self::InvalidNonce => StatusCode::BAD_REQUEST,
            Self::Replayed => StatusCode::CONFLICT,
            // The client's own quota, not the service's capacity: 429 says the
            // caller should slow down, where 503 would claim the service is
            // degraded for everyone when only this address is over its share.
            Self::ClientSaturated => StatusCode::TOO_MANY_REQUESTS,
            Self::Saturated => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::InvalidNonce => "Versioned action requests require a valid replay nonce",
            Self::Replayed => "Action request replayed",
            Self::ClientSaturated => "Action replay protection is saturated for this client",
            Self::Saturated => "Action replay protection is saturated",
        }
    }
}

impl ActionReplayGuard {
    pub(crate) fn consume(
        &mut self,
        headers: &HeaderMap,
        action_reference: &str,
        client: IpAddr,
    ) -> Result<(), ActionReplayRejection> {
        let nonce = headers
            .get("x-ruvyxa-action-nonce")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !(16..=128).contains(&nonce.len())
            || !nonce.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-')
            })
        {
            return Err(ActionReplayRejection::InvalidNonce);
        }
        let now = Instant::now();
        while self.order.front().is_some_and(|entry| entry.expires <= now) {
            if let Some(expired) = self.order.pop_front() {
                self.seen.remove(&expired.key);
                self.release_client(expired.client);
            }
        }

        let key = format!("{action_reference}:{nonce}");
        if self.seen.contains(&key) {
            return Err(ActionReplayRejection::Replayed);
        }

        // This address is over its share of the pool. Checked before the global
        // bound so the address that filled the guard is the one refused, rather
        // than whichever request happens to arrive next.
        if self
            .per_client
            .get(&client)
            .is_some_and(|held| *held >= ACTION_NONCE_MAX_PER_CLIENT)
        {
            return Err(ActionReplayRejection::ClientSaturated);
        }

        // Full, with nothing expired left to drop. Dropping the oldest live
        // nonce to make room would accept that nonce's replay — the one thing
        // this guard exists to refuse -- and an attacker reaches this state by
        // sending fresh nonces. Saturation fails closed instead.
        if self.seen.len() >= ACTION_NONCE_MAX_ENTRIES {
            return Err(ActionReplayRejection::Saturated);
        }

        // `seen` is written before `order`, so the guard's structures can only
        // ever disagree in the direction that refuses: an interruption between
        // these lines strands one key that never expires, where the reverse
        // order would leave a nonce that `seen` does not know about and whose
        // replay the guard would then accept. That is what lets the caller
        // recover a poisoned lock rather than answering 503 forever. The
        // per-client count is raised last for the same reason — overcounting
        // refuses, undercounting admits.
        self.seen.insert(key.clone());
        self.order.push_back(NonceEntry {
            expires: now + ACTION_NONCE_TTL,
            key,
            client,
        });
        *self.per_client.entry(client).or_insert(0) += 1;
        Ok(())
    }

    /// Drop one live entry from a client's count, forgetting the address when
    /// its last nonce expires so `per_client` stays bounded by the pool.
    fn release_client(&mut self, client: IpAddr) {
        if let Some(held) = self.per_client.get_mut(&client) {
            *held -= 1;
            if *held == 0 {
                self.per_client.remove(&client);
            }
        }
    }
}

pub fn action_reference_id(route_id: &str, source: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in route_id
        .as_bytes()
        .iter()
        .copied()
        .chain(std::iter::once(0))
        .chain(source.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("a_{hash:016x}")
}

/// Number of counter slots the action rate limiter keeps.
///
/// Fixed at construction, so the limiter's memory does not depend on how many
/// distinct clients it has seen. 8192 slots cost roughly 200 KiB.
const ACTION_RATE_LIMIT_SLOTS: usize = 8192;

/// One slot's sliding-window counters.
///
/// Two adjacent fixed windows approximate a sliding window in constant space:
/// the previous window's count is weighted by how much of it still falls inside
/// the trailing `window`, which bounds a burst to about `max_hits` over any
/// window without storing one timestamp per request.
#[derive(Debug, Clone, Copy)]
struct RateSlot {
    window_start: Instant,
    current: u32,
    previous: u32,
}

/// Per-client action rate limiter with memory independent of client count.
///
/// The previous design tracked a `HashMap<String, Vec<Instant>>` capped at
/// 10,000 keys and **denied any key it could not admit** once that cap was
/// reached. That made the limiter an amplifier: an attacker rotating source
/// addresses — trivial with an IPv6 /64 — filled the key set and every
/// first-time client was then rejected for the rest of the window, including
/// clients the attacker never touched. It also allowed the map itself to grow to
/// `max_keys * max_hits` timestamps, roughly 96 MiB at the default limits.
///
/// Hashing each key into a fixed slot array removes both problems. Admission is
/// never refused for lack of room, so no client can be denied on another
/// client's behalf. Slot collisions make two clients share a budget, which can
/// only ever limit a client *earlier* than its own traffic warrants — the same
/// direction the limiter already errs in, and never a bypass. The slot array is
/// seeded per process, so keys cannot be crafted to collide with a chosen
/// victim.
pub(crate) struct ActionRateLimiter {
    slots: Vec<Option<RateSlot>>,
    hasher: std::collections::hash_map::RandomState,
    max_hits: usize,
    window: Duration,
}

impl ActionRateLimiter {
    pub(crate) fn new(max_hits: usize, window: Duration) -> Self {
        Self {
            slots: vec![None; ACTION_RATE_LIMIT_SLOTS],
            hasher: std::collections::hash_map::RandomState::new(),
            max_hits,
            window,
        }
    }

    fn slot_index(&self, key: &str) -> usize {
        use std::hash::BuildHasher;
        (self.hasher.hash_one(key) % ACTION_RATE_LIMIT_SLOTS as u64) as usize
    }

    pub(crate) fn allow(&mut self, key: &str) -> bool {
        let now = Instant::now();
        let window = self.window;
        let max_hits = self.max_hits;
        let index = self.slot_index(key);

        let slot = self.slots[index].get_or_insert(RateSlot {
            window_start: now,
            current: 0,
            previous: 0,
        });
        advance_window(slot, now, window);

        if estimated_hits(slot, now, window) >= max_hits as f64 {
            return false;
        }
        slot.current = slot.current.saturating_add(1);
        true
    }

    /// Seconds a limited client should wait, from the current window's end.
    pub(crate) fn retry_after_seconds(&self, key: &str) -> u64 {
        let index = self.slot_index(key);
        let Some(slot) = self.slots[index] else {
            return 1;
        };
        let elapsed = slot.window_start.elapsed();
        self.window.saturating_sub(elapsed).as_secs().max(1)
    }

    /// Live slots, for tests asserting the memory bound.
    #[cfg(test)]
    pub(crate) fn occupied_slots(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }
}

/// Roll the slot forward so `window_start` is always within one window of `now`.
fn advance_window(slot: &mut RateSlot, now: Instant, window: Duration) {
    let elapsed = now.duration_since(slot.window_start);
    if window.is_zero() || elapsed >= window.saturating_mul(2) {
        // Both windows are stale (or the window is degenerate): start clean.
        slot.previous = 0;
        slot.current = 0;
        slot.window_start = now;
    } else if elapsed >= window {
        slot.previous = slot.current;
        slot.current = 0;
        // Advance by exactly one window rather than jumping to `now`, so the
        // remaining overlap with the previous window stays accurate.
        slot.window_start += window;
    }
}

/// Weighted request count over the trailing window.
fn estimated_hits(slot: &RateSlot, now: Instant, window: Duration) -> f64 {
    if window.is_zero() {
        return f64::from(slot.current);
    }
    let elapsed = now.duration_since(slot.window_start).as_secs_f64();
    let overlap = (1.0 - elapsed / window.as_secs_f64()).clamp(0.0, 1.0);
    f64::from(slot.previous) * overlap + f64::from(slot.current)
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
    // The trust policy is this host's alone; the decision that follows it is
    // shared. Only a proxy we trust can vouch for the scheme a browser used.
    origin_is_cross_site(headers, forwarded_scheme(headers, config, peer))
}

/// Whether the request is not provably same-origin.
///
/// The shared half of the decision, with the trusted scheme already resolved by
/// the caller. That split is what lets this be held to
/// `tests/fixtures/origin-policy-conformance.json` together with
/// `packages/@ruvyxa/core/src/origin-policy.ts`: a deployed function has no
/// transport peer to weigh `X-Forwarded-Proto` against, and the `originGuard`
/// plugin has no trust policy at all, so the three hosts legitimately differ on
/// that one input and on nothing after it.
///
/// Three ports of these rules used to be kept in step by a comment saying they
/// mirrored each other — the arrangement that let `STATIC_CONTENT_TYPES` and
/// `DEFAULT_SECURITY_HEADERS` drift apart in production.
pub(crate) fn origin_is_cross_site(headers: &HeaderMap, trusted_scheme: Option<&str>) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
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
        .filter(|value| !value.is_empty())
    else {
        return true;
    };
    let Some((origin_scheme, origin_host)) = origin
        .split_once("://")
        .filter(|(_, value)| !value.contains('/') && !value.is_empty())
    else {
        return true;
    };

    if !origin_host.eq_ignore_ascii_case(host) {
        return true;
    }

    // Assert the scheme only when something trustworthy actually stated it.
    //
    // Ruvyxa never terminates TLS itself, so the only evidence of the scheme a
    // browser used is `X-Forwarded-Proto` from a proxy we trust. Treating the
    // absence of that evidence as proof of `http` rejected every deployment
    // whose TLS-terminating proxy is not loopback and not listed in
    // `security.trustedProxyIps` — Docker Compose, Kubernetes, and managed
    // platform edges — with `403 Cross-origin action request blocked` on every
    // server action.
    //
    // The host comparison above is the load-bearing CSRF check: a browser sets
    // `Origin` itself and a cross-site page cannot forge it, so a matching host
    // already establishes same-origin intent. Comparing against a scheme we
    // cannot observe adds no protection and only produces false rejections.
    match trusted_scheme {
        Some(scheme) => !origin_scheme.eq_ignore_ascii_case(scheme),
        None => false,
    }
}

/// Read `X-Forwarded-Proto` into a scheme, taking the leftmost entry.
///
/// Callers apply their own trust policy before calling this. Shared with
/// `parseForwardedScheme` in `@ruvyxa/core/origin-policy` and replayed from the
/// same fixture.
pub(crate) fn parse_forwarded_scheme(value: Option<&str>) -> Option<&'static str> {
    value
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .and_then(|value| match value {
            value if value.eq_ignore_ascii_case("https") => Some("https"),
            value if value.eq_ignore_ascii_case("http") => Some("http"),
            _ => None,
        })
}

/// Whether this request's transport peer may state who the client is.
///
/// The trust decision is this host's own — a deployed function has no peer to
/// weigh — which is why `tests/fixtures/client-ip-conformance.json` and
/// `tests/fixtures/origin-policy-conformance.json` both start after it.
fn is_trusted_proxy_ip(config: &ServerConfig, ip: IpAddr) -> bool {
    client_ip::is_trusted_proxy_ip(&config.trusted_proxies, ip)
}

/// The address an action request is attributed to.
///
/// Shared by the action rate limiter and the replay guard's per-client quota,
/// and — through [`client_ip::client_ip`] — with the built-in `rate`
/// middleware, so the three cannot disagree about who a request belongs to.
pub(crate) fn action_client_ip(
    peer: SocketAddr,
    headers: &HeaderMap,
    config: &ServerConfig,
) -> IpAddr {
    client_ip::client_ip(peer.ip(), headers, &config.trusted_proxies)
}

/// The request scheme as reported by a trusted proxy, when one vouched for it.
fn forwarded_scheme(
    headers: &HeaderMap,
    config: &ServerConfig,
    peer: IpAddr,
) -> Option<&'static str> {
    if !is_trusted_proxy_ip(config, peer) {
        return None;
    }
    parse_forwarded_scheme(
        headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok()),
    )
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
    let client = action_client_ip(peer, headers, config);
    format!("{client}:{}:{}", query.path, query.name)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    /// One nonce header, so the guard tests read as the request they describe.
    fn nonce_headers(nonce: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-ruvyxa-action-nonce", nonce.parse().unwrap());
        headers
    }

    /// Distinct client addresses, one per index, for filling the pool.
    fn client(index: usize) -> IpAddr {
        let octets = u32::try_from(index).expect("test client index fits an IPv4 address");
        IpAddr::V4(Ipv4Addr::from(octets))
    }

    /// Both languages replay `tests/fixtures/origin-policy-conformance.json`.
    ///
    /// The JavaScript side is `tests/packages/core/origin-policy-contract.test.ts`
    /// over `@ruvyxa/core/origin-policy`, which the action endpoint and the
    /// `originGuard` plugin both read. Three ports of this decision used to be
    /// kept in step by a comment saying they mirrored each other.
    #[test]
    fn origin_policy_matches_the_shared_conformance_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/origin-policy-conformance.json"
        ))
        .unwrap();

        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let mut headers = HeaderMap::new();
            for (field, value) in case["headers"].as_object().unwrap() {
                headers.insert(
                    HeaderName::from_bytes(field.as_bytes()).unwrap(),
                    HeaderValue::from_str(value.as_str().unwrap()).unwrap(),
                );
            }
            let trusted = case["trustedScheme"].as_str();
            assert_eq!(
                origin_is_cross_site(&headers, trusted),
                case["crossSite"].as_bool().unwrap(),
                "origin policy case disagrees with the shared fixture: {name}"
            );
        }

        for case in fixture["forwardedScheme"]["cases"].as_array().unwrap() {
            let header = case["header"].as_str().unwrap();
            assert_eq!(
                parse_forwarded_scheme(Some(header)),
                case["scheme"].as_str(),
                "forwarded scheme case disagrees with the shared fixture: {header:?}"
            );
        }
        assert_eq!(parse_forwarded_scheme(None), None);
    }

    /// The trusted-proxy gate is this host's alone, and is the reason the shared
    /// table takes the scheme as an input rather than deriving it.
    #[test]
    fn forwarded_scheme_is_ignored_when_the_peer_is_not_a_trusted_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("app.test"));
        headers.insert(header::ORIGIN, HeaderValue::from_static("http://app.test"));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));

        let config = ServerConfig::dev("D:/app", "127.0.0.1", 3000);
        let untrusted = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));
        // Loopback is trusted by default, so it is the contrast case.
        let trusted = IpAddr::V4(Ipv4Addr::LOCALHOST);

        assert!(
            !action_origin_is_cross_site(&headers, &config, untrusted),
            "an untrusted peer's X-Forwarded-Proto must not reject a matching host"
        );
        assert!(
            action_origin_is_cross_site(&headers, &config, trusted),
            "a trusted proxy stating https must reject an http origin"
        );
    }

    #[test]
    fn action_reference_and_nonce_replay_contract_is_stable() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/action-contract.json"))
                .unwrap();
        assert_eq!(
            action_reference_id(
                fixture["routeId"].as_str().unwrap(),
                fixture["source"].as_str().unwrap(),
            ),
            fixture["expected"].as_str().unwrap()
        );

        let mut guard = ActionReplayGuard::default();
        let headers = nonce_headers("0123456789abcdef");
        assert!(
            guard
                .consume(&headers, "a_0123456789abcdef", client(1))
                .is_ok()
        );
        assert_eq!(
            guard.consume(&headers, "a_0123456789abcdef", client(1)),
            Err(ActionReplayRejection::Replayed)
        );

        // A replay from a different address is still a replay: the client is an
        // accounting dimension, never part of the nonce's identity.
        assert_eq!(
            guard.consume(&headers, "a_0123456789abcdef", client(2)),
            Err(ActionReplayRejection::Replayed)
        );

        let nonce = &fixture["nonce"];
        assert_eq!(
            nonce["ttlSeconds"].as_u64().unwrap(),
            ACTION_NONCE_TTL.as_secs()
        );
        assert_eq!(
            nonce["maxEntries"].as_u64().unwrap() as usize,
            ACTION_NONCE_MAX_ENTRIES
        );
        assert_eq!(
            nonce["perClientMaxEntries"].as_u64().unwrap() as usize,
            ACTION_NONCE_MAX_PER_CLIENT
        );
    }

    /// A guard with no expired entry to reclaim refuses the request rather than
    /// dropping a live nonce, whose replay it would then accept. Held with the
    /// serverless handler by `action-contract.json`.
    #[test]
    fn a_saturated_replay_guard_refuses_rather_than_dropping_a_live_nonce() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/action-contract.json"))
                .unwrap();
        let saturation = &fixture["nonce"]["saturation"];
        assert_eq!(saturation["behavior"].as_str().unwrap(), "reject");

        let mut guard = ActionReplayGuard::default();
        let first = "n0000000000000000";
        // Spread across addresses, because no single one may fill the pool any
        // more — that is what `a_single_client_cannot_saturate_the_pool` holds.
        for index in 0..ACTION_NONCE_MAX_ENTRIES {
            let headers = nonce_headers(&format!("n{index:016}"));
            assert!(
                guard
                    .consume(
                        &headers,
                        "a_0123456789abcdef",
                        client(index / ACTION_NONCE_MAX_PER_CLIENT)
                    )
                    .is_ok()
            );
        }

        let headers = nonce_headers("fedcba9876543210");
        let rejection = guard
            .consume(&headers, "a_0123456789abcdef", client(9999))
            .expect_err("a saturated guard must refuse");
        assert_eq!(rejection, ActionReplayRejection::Saturated);

        // Both halves of the fixture's saturation clause, replayed here as the
        // serverless handler's suite replays them. The status was previously
        // derived in `handle_action` by comparing the message against a copy of
        // this literal, so a reworded message answered 400 with nothing failing.
        assert_eq!(rejection.message(), saturation["message"].as_str().unwrap());
        assert_eq!(
            rejection.status().as_u16(),
            u16::try_from(saturation["status"].as_u64().unwrap()).unwrap()
        );

        // The nonce that filling the guard would previously have evicted is
        // still refused as a replay, which is the point of failing closed.
        let headers = nonce_headers(first);
        assert_eq!(
            guard.consume(&headers, "a_0123456789abcdef", client(0)),
            Err(ActionReplayRejection::Replayed)
        );
    }

    /// One address may hold only its share, so it cannot refuse everyone else's
    /// actions by filling the pool alone. The rate limiter in front of the guard
    /// does not bound this on its own: it is keyed per path and action too, so
    /// the same client earns a fresh bucket per action while the pool stays one.
    #[test]
    fn a_single_client_cannot_saturate_the_pool() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/action-contract.json"))
                .unwrap();
        let clause = &fixture["nonce"]["clientSaturation"];
        assert_eq!(clause["behavior"].as_str().unwrap(), "reject");

        let mut guard = ActionReplayGuard::default();
        let noisy = client(7);
        for index in 0..ACTION_NONCE_MAX_PER_CLIENT {
            let headers = nonce_headers(&format!("n{index:016}"));
            assert!(guard.consume(&headers, "a_0123456789abcdef", noisy).is_ok());
        }

        let headers = nonce_headers("fedcba9876543210");
        let rejection = guard
            .consume(&headers, "a_0123456789abcdef", noisy)
            .expect_err("a client over its quota must be refused");
        assert_eq!(rejection, ActionReplayRejection::ClientSaturated);
        assert_eq!(rejection.message(), clause["message"].as_str().unwrap());
        assert_eq!(
            rejection.status().as_u16(),
            u16::try_from(clause["status"].as_u64().unwrap()).unwrap()
        );

        // The pool is a tenth full, so every other address is unaffected — the
        // whole point of the quota.
        assert!(
            guard
                .consume(&headers, "a_0123456789abcdef", client(8))
                .is_ok()
        );
    }

    /// Every rejection carries its own status, and nothing else in the crate
    /// pins them now that `handle_action` no longer restates them.
    #[test]
    fn every_replay_rejection_carries_its_own_status() {
        let mut guard = ActionReplayGuard::default();
        let headers = nonce_headers("short");
        assert_eq!(
            guard.consume(&headers, "a_0123456789abcdef", client(1)),
            Err(ActionReplayRejection::InvalidNonce)
        );

        assert_eq!(
            ActionReplayRejection::ClientSaturated.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            ActionReplayRejection::InvalidNonce.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ActionReplayRejection::Replayed.status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ActionReplayRejection::Saturated.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
