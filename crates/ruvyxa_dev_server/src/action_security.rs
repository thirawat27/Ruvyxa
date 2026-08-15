//! Server-action request validation: origin/fetch-metadata checks, payload
//! parsing, and the per-key rate limiter.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::{ActionQuery, ServerConfig};

const ACTION_NONCE_TTL: Duration = Duration::from_secs(600);
const ACTION_NONCE_MAX_ENTRIES: usize = 10_000;

/// Process-local replay protection for version-bound action requests.
#[derive(Debug, Default)]
pub(crate) struct ActionReplayGuard {
    entries: BTreeMap<String, Instant>,
}

impl ActionReplayGuard {
    pub(crate) fn consume(
        &mut self,
        headers: &HeaderMap,
        action_reference: &str,
    ) -> Result<(), &'static str> {
        let nonce = headers
            .get("x-ruvyxa-action-nonce")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !(16..=128).contains(&nonce.len())
            || !nonce.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-')
            })
        {
            return Err("Versioned action requests require a valid replay nonce");
        }
        let now = Instant::now();
        self.entries.retain(|_, expires| *expires > now);
        let key = format!("{action_reference}:{nonce}");
        if self.entries.contains_key(&key) {
            return Err("Action request replayed");
        }
        self.entries.insert(key, now + ACTION_NONCE_TTL);
        while self.entries.len() > ACTION_NONCE_MAX_ENTRIES {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, expires)| **expires)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        Ok(())
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
    // `unmap_v4` so a dual-stack listener's `::ffff:127.0.0.1` peer is still
    // recognized as loopback.
    let ip = unmap_v4(ip);
    ip.is_loopback() || config.trusted_proxies.contains(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut headers = HeaderMap::new();
        headers.insert("x-ruvyxa-action-nonce", "0123456789abcdef".parse().unwrap());
        assert!(guard.consume(&headers, "a_0123456789abcdef").is_ok());
        assert_eq!(
            guard.consume(&headers, "a_0123456789abcdef"),
            Err("Action request replayed")
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
