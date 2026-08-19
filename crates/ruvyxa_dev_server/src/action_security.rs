//! Server-action request validation: origin/fetch-metadata checks, payload
//! parsing, and the per-key rate limiter.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

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

/// One trusted reverse-proxy address, expressed as a network prefix.
///
/// A bare address is stored as a host route (`/32` for IPv4, `/128` for IPv6),
/// so exact addresses and ranges share one matching path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpPrefix {
    network: IpAddr,
    prefix_len: u8,
}

impl IpPrefix {
    /// Parse `10.0.0.0/8`, `2001:db8::/32`, or a bare `10.0.0.9`.
    ///
    /// Host bits outside the prefix are masked off rather than rejected, so
    /// `10.1.2.3/8` and `10.0.0.0/8` describe the same range instead of one of
    /// them failing a deployment at startup over a cosmetic difference.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        let (address, prefix_len) = match value.split_once('/') {
            Some((address, length)) => {
                let address = address.trim().parse::<IpAddr>().ok()?;
                let prefix_len = length.trim().parse::<u8>().ok()?;
                (address, prefix_len)
            }
            None => {
                let address = value.parse::<IpAddr>().ok()?;
                (address, host_prefix_len(address))
            }
        };
        if prefix_len > host_prefix_len(address) {
            return None;
        }
        Some(Self {
            network: mask_address(address, prefix_len),
            prefix_len,
        })
    }

    /// Whether `candidate` falls inside this prefix.
    pub fn contains(&self, candidate: IpAddr) -> bool {
        // A dual-stack listener reports an IPv4 peer as `::ffff:10.0.0.9`.
        // Comparing that against an IPv4 prefix byte-wise would never match, so
        // a proxy allowlist written in IPv4 would silently stop working the
        // moment the server bound an IPv6 socket.
        let candidate = unmap_v4(candidate);
        let network = unmap_v4(self.network);
        match (network, candidate) {
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {
                mask_address(candidate, self.prefix_len) == network
            }
            _ => false,
        }
    }
}

/// Reverse proxies allowed to supply forwarded client and protocol headers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies {
    prefixes: Vec<IpPrefix>,
}

impl TrustedProxies {
    /// Parse every configured entry, naming the first invalid one.
    pub fn parse_all<'a>(
        values: impl IntoIterator<Item = &'a str>,
    ) -> std::result::Result<Self, String> {
        let mut prefixes = Vec::new();
        for value in values {
            let prefix = IpPrefix::parse(value)
                .ok_or_else(|| format!("invalid IP or CIDR range `{value}`"))?;
            prefixes.push(prefix);
        }
        Ok(Self { prefixes })
    }

    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty()
    }

    /// Whether `ip` is a configured proxy. Loopback is handled by the caller.
    pub fn contains(&self, ip: IpAddr) -> bool {
        self.prefixes.iter().any(|prefix| prefix.contains(ip))
    }
}

impl FromIterator<IpPrefix> for TrustedProxies {
    fn from_iter<T: IntoIterator<Item = IpPrefix>>(iter: T) -> Self {
        Self {
            prefixes: iter.into_iter().collect(),
        }
    }
}

fn host_prefix_len(address: IpAddr) -> u8 {
    match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    }
}

/// Zero every bit below `prefix_len`.
fn mask_address(address: IpAddr, prefix_len: u8) -> IpAddr {
    match address {
        IpAddr::V4(address) => {
            let bits = u32::from(address);
            let mask = if prefix_len == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix_len))
            };
            IpAddr::V4(Ipv4Addr::from(bits & mask))
        }
        IpAddr::V6(address) => {
            let bits = u128::from(address);
            let mask = if prefix_len == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(prefix_len))
            };
            IpAddr::V6(std::net::Ipv6Addr::from(bits & mask))
        }
    }
}

/// Collapse an IPv4-mapped IPv6 address to its IPv4 form.
fn unmap_v4(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => match address.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(address),
        },
        address => address,
    }
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
    match forwarded_scheme(headers, config, peer) {
        Some(scheme) => !origin_scheme.eq_ignore_ascii_case(scheme),
        None => false,
    }
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
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .and_then(|value| match value {
            value if value.eq_ignore_ascii_case("https") => Some("https"),
            value if value.eq_ignore_ascii_case("http") => Some("http"),
            _ => None,
        })
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

/// The address an action request is attributed to.
///
/// Forwarded identity is untrusted unless the direct peer is loopback or
/// explicitly allowlisted. Private ranges alone are not a trust boundary:
/// a LAN client can otherwise forge X-Forwarded-For and bypass the limiter.
///
/// Shared by the rate limiter and the replay guard's per-client quota, so the
/// two cannot disagree about who a request belongs to.
pub(crate) fn action_client_ip(
    peer: SocketAddr,
    headers: &HeaderMap,
    config: &ServerConfig,
) -> IpAddr {
    let peer_ip = peer.ip();
    if is_trusted_proxy_ip(config, peer_ip) {
        forwarded_client_ip(config, headers).unwrap_or(peer_ip)
    } else {
        peer_ip
    }
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
    // `unmap_v4` so a dual-stack listener's `::ffff:127.0.0.1` peer is still
    // recognized as loopback.
    let ip = unmap_v4(ip);
    ip.is_loopback() || config.trusted_proxies.contains(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("test address must parse")
    }

    #[test]
    fn parses_bare_addresses_as_host_routes() {
        let prefix = IpPrefix::parse("10.0.0.9").expect("a bare IPv4 address is valid");
        assert!(prefix.contains(ip("10.0.0.9")));
        assert!(!prefix.contains(ip("10.0.0.10")));

        let prefix = IpPrefix::parse("2001:db8::2").expect("a bare IPv6 address is valid");
        assert!(prefix.contains(ip("2001:db8::2")));
        assert!(!prefix.contains(ip("2001:db8::3")));
    }

    #[test]
    fn matches_documented_cidr_ranges() {
        // These are the exact ranges the server-actions guide tells users to
        // configure; before CIDR support they failed validation at startup.
        let proxies = TrustedProxies::parse_all(["10.0.0.0/8", "172.16.0.0/12"])
            .expect("documented ranges must parse");

        assert!(proxies.contains(ip("10.255.255.254")));
        assert!(proxies.contains(ip("172.18.0.4")));
        assert!(!proxies.contains(ip("172.32.0.1")), "outside the /12");
        assert!(!proxies.contains(ip("11.0.0.1")), "outside the /8");
    }

    #[test]
    fn masks_host_bits_instead_of_rejecting_them() {
        let sloppy = IpPrefix::parse("10.1.2.3/8").expect("host bits are masked, not rejected");
        let canonical = IpPrefix::parse("10.0.0.0/8").expect("canonical form must parse");
        assert_eq!(sloppy, canonical);
    }

    #[test]
    fn rejects_malformed_and_oversized_prefixes() {
        for value in [
            "not-an-ip",
            "10.0.0.0/33",
            "2001:db8::/129",
            "10.0.0.0/",
            "10.0.0.0/8/8",
            "",
        ] {
            assert!(IpPrefix::parse(value).is_none(), "{value} must not parse");
        }
        let error = TrustedProxies::parse_all(["10.0.0.0/8", "10.0.0.0/33"])
            .expect_err("an invalid entry must be reported");
        assert!(error.contains("10.0.0.0/33"), "{error}");
    }

    #[test]
    fn a_zero_length_prefix_matches_every_address_of_its_family() {
        let all_v4 = IpPrefix::parse("0.0.0.0/0").expect("/0 is a valid prefix");
        assert!(all_v4.contains(ip("203.0.113.8")));
        assert!(
            !all_v4.contains(ip("2001:db8::2")),
            "an IPv4 prefix must not swallow IPv6 peers"
        );
    }

    #[test]
    fn ipv4_prefixes_match_dual_stack_mapped_peers() {
        // A server bound to an IPv6 wildcard socket reports an IPv4 client as
        // `::ffff:a.b.c.d`. Without unmapping, an IPv4 proxy allowlist would
        // silently stop matching the moment the listener became dual-stack.
        let proxies = TrustedProxies::parse_all(["10.0.0.0/8"]).expect("range must parse");
        assert!(proxies.contains(ip("::ffff:10.0.0.9")));
        assert!(!proxies.contains(ip("::ffff:11.0.0.9")));
    }

    #[test]
    fn families_never_cross_match() {
        let v6 = TrustedProxies::parse_all(["2001:db8::/32"]).expect("range must parse");
        assert!(v6.contains(ip("2001:db8::dead")));
        assert!(!v6.contains(ip("10.0.0.9")));
    }

    #[test]
    fn an_empty_allowlist_trusts_nothing_beyond_loopback() {
        let proxies = TrustedProxies::default();
        assert!(proxies.is_empty());
        assert!(!proxies.contains(ip("10.0.0.9")));
        // Loopback trust lives in `is_trusted_proxy_ip`, not in the allowlist,
        // so the empty allowlist must not claim it.
        assert!(!proxies.contains(ip("127.0.0.1")));
    }
}
