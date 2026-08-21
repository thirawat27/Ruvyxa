//! Who a request belongs to, for every control that counts per client.
//!
//! Three places in this repository answer that question: the server-action rate
//! limiter and replay quota in
//! `crates/ruvyxa_dev_server/src/action_security.rs`, the built-in `rate`
//! middleware in [`crate::builtin`], and `clientAddress` in
//! `packages/ruvyxa/runtime/serverless-handler.mjs`. The first and third
//! already scanned `X-Forwarded-For` from the right against
//! `security.trustedProxyIps`; the second read the transport peer and nothing
//! else, so one project with one `middleware.builtin.rate` block was limited
//! per real client once deployed and as a single shared bucket when the native
//! server ran behind a reverse proxy — the control meant to protect the service
//! became the thing that denied it.
//!
//! This module is that rule, once. Both Rust hosts call it directly; the
//! JavaScript host cannot, so the two languages are held to
//! `tests/fixtures/client-ip-conformance.json`.
//!
//! What stays outside the shared table is deliberate, and mirrors
//! `ForwardedScheme` in `@ruvyxa/core/origin-policy`: whether this request's
//! upstream hop may be believed at all. The native server weighs the transport
//! peer against the trusted list; a deployed function has no peer and treats
//! its platform ingress as trusted by construction. Everything after that
//! decision is identical.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use axum::http::HeaderMap;

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

    /// Whether `ip` is a configured proxy. Loopback is handled by
    /// [`is_trusted_proxy_ip`].
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

/// Whether an address may state who the client is.
///
/// Loopback is trusted without configuration — a proxy terminating on the same
/// host is the ordinary deployment — and other private ranges are not, because
/// a LAN client would otherwise forge `X-Forwarded-For` and walk past every
/// per-client control in the process.
pub fn is_trusted_proxy_ip(trusted: &TrustedProxies, ip: IpAddr) -> bool {
    // `unmap_v4` so a dual-stack listener's `::ffff:127.0.0.1` peer is still
    // recognized as loopback.
    let ip = unmap_v4(ip);
    ip.is_loopback() || trusted.contains(ip)
}

/// Pick the client address out of forwarded headers, scanning from the right.
///
/// Each proxy appends the peer it actually saw, so rightmost entries are
/// proxy-written while leftmost entries arrive from the client and are
/// forgeable. Taking the leftmost entry would let a client behind a trusted
/// proxy rotate fabricated addresses through a rate limiter and collect a fresh
/// bucket every request.
///
/// Only hops that parse as an address are considered: the header is
/// client-writable, and treating raw text as an identity lets one caller rotate
/// arbitrary junk through a limiter that then counts to one, forever.
pub fn forwarded_client_ip(trusted: &TrustedProxies, headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|value| value.to_str().ok())
        .into_iter()
        .flat_map(|value| value.split(',').rev())
        .filter_map(|value| value.trim().parse::<IpAddr>().ok())
        .find(|candidate| !is_trusted_proxy_ip(trusted, *candidate))
}

/// The address a request is attributed to on a host that has a transport peer.
///
/// Forwarded identity is untrusted unless the direct peer is itself trusted.
/// Shared by the action rate limiter, the action replay guard's per-client
/// quota, and the built-in `rate` middleware, so the three cannot disagree
/// about who a request belongs to.
pub fn client_ip(peer: IpAddr, headers: &HeaderMap, trusted: &TrustedProxies) -> IpAddr {
    if is_trusted_proxy_ip(trusted, peer) {
        forwarded_client_ip(trusted, headers).unwrap_or(peer)
    } else {
        peer
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
            IpAddr::V6(Ipv6Addr::from(bits & mask))
        }
    }
}

/// Collapse an IPv4-mapped IPv6 address to its IPv4 form.
pub fn unmap_v4(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => match address.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(address),
        },
        address => address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn headers_from(pairs: &serde_json::Map<String, serde_json::Value>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (field, value) in pairs {
            headers.insert(
                HeaderName::from_bytes(field.as_bytes()).unwrap(),
                HeaderValue::from_str(value.as_str().unwrap()).unwrap(),
            );
        }
        headers
    }

    /// Both languages replay `tests/fixtures/client-ip-conformance.json`.
    ///
    /// The JavaScript side is
    /// `tests/packages/ruvyxa/client-ip-contract.test.mjs` over `clientAddress`
    /// in `serverless-handler.mjs`. The two used to disagree: this host read
    /// the transport peer and never looked at a forwarded header, so the same
    /// `middleware.builtin.rate` block limited per real client once deployed
    /// and as one shared bucket when the native server ran behind a proxy.
    #[test]
    fn forwarded_scan_matches_the_shared_conformance_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/client-ip-conformance.json"
        ))
        .unwrap();

        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let headers = headers_from(case["headers"].as_object().unwrap());
            let trusted = TrustedProxies::parse_all(
                case["trustedProxyIps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap()),
            )
            .unwrap();
            let actual = forwarded_client_ip(&trusted, &headers).map(|ip| ip.to_string());
            assert_eq!(
                actual.as_deref(),
                case["client"].as_str(),
                "client identity case disagrees with the shared fixture: {name}"
            );
        }
    }

    /// The peer gate is this host's alone, and is why the shared table starts
    /// after it.
    #[test]
    fn forwarded_identity_is_ignored_when_the_peer_is_not_trusted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9"));
        let trusted = TrustedProxies::parse_all(["10.0.0.0/8"]).unwrap();

        let untrusted_peer = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 4));
        assert_eq!(
            client_ip(untrusted_peer, &headers, &trusted),
            untrusted_peer,
            "a client that is not a proxy must not be able to rename itself"
        );

        let proxy_peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9));
        assert_eq!(
            client_ip(proxy_peer, &headers, &trusted).to_string(),
            "203.0.113.9"
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

    /// Loopback is trusted without configuration; other private ranges are not.
    #[test]
    fn loopback_is_trusted_and_other_private_ranges_are_not() {
        let empty = TrustedProxies::default();
        assert!(is_trusted_proxy_ip(&empty, IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_trusted_proxy_ip(
            &empty,
            "::ffff:127.0.0.1".parse().unwrap()
        ));
        assert!(!is_trusted_proxy_ip(
            &empty,
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 4))
        ));
    }
}
