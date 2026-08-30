//! Middleware stack builder.
//!
//! Compiles a `MiddlewareConfig` into an axum-compatible layer stack
//! that can be applied to a Router.

use axum::{Router, body::HttpBody};
use tower_http::compression::{
    CompressionLayer,
    predicate::{DefaultPredicate, Predicate},
};
use tracing::{info, warn};

use crate::builtin::{
    CorsLayer, CorsPolicy, CustomHeadersLayer, RateLimitLayerWithKey, RequestLoggingLayer,
    TimingLayer,
};
use crate::client_ip::TrustedProxies;
use crate::config::MiddlewareConfig;

/// Compress only response bodies whose complete size is already known.
///
/// Axum bodies backed by a live stream have no exact size hint. Running those
/// bodies through the asynchronous compression adapter can terminate the
/// encoded body before the HTTP/1 chunked response is finalized, which clients
/// report as an incomplete chunked encoding. Buffered responses keep the normal
/// tower-http content-type and minimum-size compression rules.
#[derive(Clone, Default)]
struct CompleteBodyCompressionPredicate {
    default: DefaultPredicate,
}

impl Predicate for CompleteBodyCompressionPredicate {
    fn should_compress<B>(&self, response: &axum::http::Response<B>) -> bool
    where
        B: HttpBody,
    {
        response.body().size_hint().exact().is_some() && self.default.should_compress(response)
    }
}

/// Rate-limit key selectors that hand the bucket key to the caller.
///
/// `header:` mode is verbatim by design — an API key is not an address and
/// must not be parsed as one — so a forwarding header selected here is used
/// exactly as sent. These are the names for which that is a bypass rather than
/// a choice.
const FORWARDING_HEADER_KEYS: [&str; 5] = [
    "header:x-forwarded-for",
    "header:x-real-ip",
    "header:cf-connecting-ip",
    "header:x-vercel-forwarded-for",
    "header:true-client-ip",
];

/// A compiled middleware stack ready to be applied to an axum Router.
#[derive(Default)]
pub struct MiddlewareStack {
    config: MiddlewareConfig,
    trusted_proxies: TrustedProxies,
}

impl MiddlewareStack {
    /// Create a new middleware stack from configuration.
    pub fn new(config: MiddlewareConfig) -> Self {
        Self {
            config,
            trusted_proxies: TrustedProxies::default(),
        }
    }

    /// Reverse proxies whose forwarded client header the rate limiter believes.
    ///
    /// This is `security.trustedProxyIps`, the same list the server-action
    /// limiter reads. It is a separate builder step rather than a field on
    /// `MiddlewareConfig` because it is a security policy the host owns, not
    /// part of the `middleware` block a project writes.
    pub fn with_trusted_proxies(mut self, trusted_proxies: TrustedProxies) -> Self {
        self.trusted_proxies = trusted_proxies;
        self
    }

    /// Apply the middleware stack to an axum Router.
    ///
    /// `Router::layer` wraps outermost-**last**, so this reads bottom-up: the
    /// runtime order is
    /// 1. Compression (gzip + brotli)
    /// 2. Request Logging (X-Request-Id)
    /// 3. Custom Headers
    /// 4. Timing (X-Response-Time header)
    /// 5. Rate Limiting (if configured)
    /// 6. CORS (if configured)
    ///
    /// **Observability sits outside the two short-circuits on purpose.** The
    /// rate limiter's 429 and the CORS preflight's 204 are produced by a layer
    /// rather than by the application, so every layer *below* them never sees
    /// them. With logging, custom headers, and timing inside the limiter, a 429
    /// carried no `x-request-id`, no `x-response-time`, and none of the
    /// project's `middleware.builtin.headers`, and no `"request"` line was
    /// logged for it at all — rate limiting was invisible in the request log,
    /// which is the one signal an operator needs when a limiter is
    /// misconfigured. The framework's own security headers were never affected:
    /// `ruvyxa_dev_server` applies those as a `map_response` outside this whole
    /// stack.
    ///
    /// **Two consequences of that order, decided deliberately.**
    ///
    /// `CustomHeadersLayer` uses `insert`, so a configured header now overrides
    /// one an inner handler set under the same name; previously the handler won
    /// because the layer ran first. Configuration is the more specific
    /// statement of intent — a project that writes `headers` in
    /// `ruvyxa.config.ts` means them — and a header the handler owns is not
    /// something a project should be silently unable to set. Request logging is
    /// applied outermost of the two so `x-request-id` still names the request
    /// the log line names, whatever a project configures.
    ///
    /// CORS sits *inside* the rate limiter so a preflight spends a token: an
    /// `OPTIONS` flood from an allowed origin used to be free to send. That
    /// order means the CORS layer never runs on the way back out of a 429, so
    /// the limiter is lent the same [`CorsPolicy`] the layer applies and
    /// decorates its own refusal with it. A rate-limited cross-origin request
    /// carries `Access-Control-Allow-Origin` — and
    /// `Access-Control-Allow-Credentials` when configured — so the browser
    /// surfaces the status and the `Retry-After` the limiter already set instead
    /// of an opaque CORS failure. The two concerns stay separable: the limiter
    /// asks the policy a question rather than implementing CORS, an origin
    /// outside the allowlist is refused the same way on both paths, and with no
    /// `cors` block configured there is no policy to ask and a 429 carries no
    /// CORS headers at all.
    ///
    /// A rate-limited *preflight* is still a failed preflight, because the Fetch
    /// standard requires an ok status for one and 429 is not ok. And a `max`
    /// sized for page loads can be too low for legitimate preflight traffic; a
    /// browser client that fires many needs the limit raised to account for
    /// them.
    ///
    /// Fails when the configuration is invalid: installing a layer that
    /// `validate()` rejects (for example credentialed wildcard CORS) would
    /// silently weaken security, so an invalid config must never produce a
    /// running stack.
    pub fn apply<S: Clone + Send + Sync + 'static>(
        &self,
        router: Router<S>,
    ) -> std::result::Result<Router<S>, String> {
        self.validate()?;
        let mut app = router;

        // Apply CORS (innermost, so its preflight short-circuit is still
        // counted by the rate limiter above it)
        let mut cors_policy = None;
        if let Some(ref cors_config) = self.config.builtin.cors {
            let cors = CorsLayer::from_config(cors_config);
            // The limiter's 429 is produced above this layer and can never pass
            // back out through it, so it is handed the layer's own decision to
            // apply to that one response itself.
            cors_policy = Some(cors.policy());
            app = app.layer(cors);
            info!(
                origins = cors_config.origins.len(),
                "CORS middleware enabled"
            );
        }

        // Apply rate limiting
        app = self.apply_rate_limit(app, cors_policy);

        // Apply timing
        if self.config.builtin.timing {
            app = app.layer(TimingLayer);
        }

        // Apply custom headers if any
        if !self.config.builtin.headers.is_empty() {
            app = app.layer(CustomHeadersLayer::new(&self.config.builtin.headers));
            info!(
                count = self.config.builtin.headers.len(),
                "custom response headers configured"
            );
        }

        // Apply request logging outside every short-circuit, so a refused
        // request is logged and correlatable like any other.
        if self.config.builtin.logging {
            app = app.layer(RequestLoggingLayer);
        }

        // Compression is always applied to complete, sized bodies (outermost).
        // Unknown-size bodies are live streams and must reach HTTP framing
        // without an asynchronous compression adapter in between.
        app = app.layer(
            CompressionLayer::new().compress_when(CompleteBodyCompressionPredicate::default()),
        );

        info!(
            builtin_layers = self.count_builtin_layers(),
            "middleware stack applied"
        );

        Ok(app)
    }

    /// Validate the middleware configuration before applying it.
    ///
    /// Returns an error when builtin middleware values are invalid.
    pub fn validate(&self) -> std::result::Result<(), String> {
        self.config.plugin_workers()?;
        self.config.plugin_timeout()?;

        for (name, value) in &self.config.builtin.headers {
            if axum::http::HeaderName::from_bytes(name.as_bytes()).is_err()
                || axum::http::HeaderValue::from_str(value).is_err()
            {
                return Err(format!("Invalid custom response header '{name}'"));
            }
        }

        if let Some(cors) = &self.config.builtin.cors {
            if cors.credentials && cors.origins.iter().any(|origin| origin == "*") {
                return Err(
                    "CORS credentials cannot be enabled with the wildcard origin '*'; use an explicit origin allowlist"
                        .to_string(),
                );
            }
            for method in &cors.methods {
                if axum::http::Method::from_bytes(method.as_bytes()).is_err() {
                    return Err(format!("Invalid CORS method '{method}'"));
                }
            }
            for allowed_header in &cors.headers {
                if axum::http::HeaderName::from_bytes(allowed_header.as_bytes()).is_err() {
                    return Err(format!("Invalid CORS header '{allowed_header}'"));
                }
            }
        }

        if let Some(rate) = &self.config.builtin.rate_limit {
            if rate.max_requests == 0 {
                return Err("Rate limit 'max' must be greater than 0".to_string());
            }
            if rate.window_secs == 0 {
                return Err("Rate limit 'window' must be greater than 0".to_string());
            }
            if rate.key_by == "ip" {
                // `ip` asks `crate::client_ip`: the transport peer, unless that
                // peer is loopback or listed in `security.trustedProxyIps`, in
                // which case the forwarded chain names the client. A client
                // that is not a proxy still cannot rename itself.
            } else if let Some(header_name) = rate.key_by.strip_prefix("header:") {
                if header_name.is_empty()
                    || axum::http::HeaderName::from_bytes(header_name.as_bytes()).is_err()
                {
                    return Err(format!(
                        "Rate limit 'key' must be 'ip' or 'header:<valid-header-name>', got '{}'",
                        rate.key_by
                    ));
                }
            } else {
                return Err(format!(
                    "Rate limit 'key' must be 'ip' or 'header:<valid-header-name>', got '{}'",
                    rate.key_by
                ));
            }
        }

        Ok(())
    }

    /// Install the rate limiter, and say so when its key is one a caller writes.
    ///
    /// `cors` is the decision the CORS layer below would have applied; the
    /// limiter needs it because its refusal never reaches that layer.
    fn apply_rate_limit<S: Clone + Send + Sync + 'static>(
        &self,
        app: Router<S>,
        cors: Option<CorsPolicy>,
    ) -> Router<S> {
        let Some(rate_config) = &self.config.builtin.rate_limit else {
            return app;
        };
        // `header:` is the escape hatch for an application-defined identity
        // such as an API key, and it is used verbatim — no parsing, no
        // trusted-hop scan. Pointing it at a forwarding header hands the bucket
        // key to the caller: one client rotating the value collects a fresh
        // allowance every request. `ip` is the proxy-aware mode and is what a
        // deployment behind a proxy wants.
        if FORWARDING_HEADER_KEYS
            .iter()
            .any(|name| rate_config.key_by.eq_ignore_ascii_case(name))
        {
            warn!(
                key = %rate_config.key_by,
                "rate limit key uses a client-writable forwarding header verbatim, which one caller can rotate to bypass the limit; use key: \"ip\" with security.trustedProxyIps instead"
            );
        }
        info!(
            max = rate_config.max_requests,
            window_secs = rate_config.window_secs,
            key = %rate_config.key_by,
            "rate limiting enabled"
        );
        app.layer(
            RateLimitLayerWithKey::from_config(rate_config, self.trusted_proxies.clone())
                .with_cors(cors),
        )
    }

    fn count_builtin_layers(&self) -> usize {
        let mut count = 1; // compression always on
        if self.config.builtin.cors.is_some() {
            count += 1;
        }
        if self.config.builtin.rate_limit.is_some() {
            count += 1;
        }
        if self.config.builtin.timing {
            count += 1;
        }
        if self.config.builtin.logging {
            count += 1;
        }
        if !self.config.builtin.headers.is_empty() {
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RateLimitConfig;
    use axum::{
        body::{Body, Bytes, to_bytes},
        http::{Method, Request, Response, StatusCode, header},
        routing::get,
    };
    use futures_core::Stream;
    use std::{
        convert::Infallible,
        pin::Pin,
        task::{Context, Poll},
    };
    use tower::ServiceExt;

    struct OneChunk(Option<Bytes>);

    impl Stream for OneChunk {
        type Item = Result<Bytes, Infallible>;

        fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.0.take().map(Ok))
        }
    }

    async fn streamed_response() -> Response<Body> {
        Response::new(Body::from_stream(OneChunk(Some(Bytes::from_static(
            b"streamed response that is deliberately larger than thirty-two bytes",
        )))))
    }

    async fn buffered_response() -> &'static str {
        "buffered response that is deliberately larger than thirty-two bytes"
    }

    /// One configured response header, and a limiter with room for exactly one
    /// request, so the second request through the stack is a short-circuit.
    fn short_circuit_probe_config() -> MiddlewareConfig {
        let mut config = MiddlewareConfig::default();
        config
            .builtin
            .headers
            .insert("x-ruvyxa-probe".to_string(), "configured".to_string());
        config.builtin.rate_limit = Some(RateLimitConfig {
            max_requests: 1,
            window_secs: 60,
            key_by: "ip".to_string(),
        });
        config
    }

    fn plain_request(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    /// A 429 is produced by a layer, not by the application, so it only carries
    /// the request id and the project's configured headers when those layers sit
    /// **outside** the limiter. They did not, which is why rate limiting was
    /// invisible in the request log — the one signal an operator needs when a
    /// limiter is misconfigured.
    #[tokio::test]
    async fn a_rate_limited_response_still_carries_the_request_id_and_configured_headers() {
        let app = MiddlewareStack::new(short_circuit_probe_config())
            .apply(Router::new().route("/", get(buffered_response)))
            .expect("probe config is valid");

        let first = app.clone().oneshot(plain_request("/")).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let limited = app.oneshot(plain_request("/")).await.unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited.headers().get("x-ruvyxa-probe"),
            Some(&header::HeaderValue::from_static("configured")),
            "a configured response header must survive a short-circuited response"
        );
        assert!(
            limited.headers().contains_key("x-request-id"),
            "a 429 the operator cannot correlate with a log line is the defect"
        );
        assert!(
            limited.headers().contains_key("x-response-time"),
            "x-response-time is the other half of the same signal"
        );
    }

    /// A preflight is a request the browser sends and a server answers, so it
    /// costs the same to serve as any other. Answering it above the limiter made
    /// an `OPTIONS` flood from an allowed origin free to send.
    #[tokio::test]
    async fn a_cors_preflight_spends_a_rate_limit_token_and_carries_configured_headers() {
        let mut config = short_circuit_probe_config();
        config.builtin.cors = Some(probe_cors_config(false));
        let app = MiddlewareStack::new(config)
            .apply(Router::new().route("/", get(buffered_response)))
            .expect("probe config is valid");

        let preflight = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/")
                    .header(header::ORIGIN, "https://app.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            preflight.headers().get("x-ruvyxa-probe"),
            Some(&header::HeaderValue::from_static("configured")),
            "the other short-circuit loses the same configured headers"
        );
        assert!(preflight.headers().contains_key("x-request-id"));

        // The preflight spent the single token, so the next request is refused.
        let next = app.oneshot(plain_request("/")).await.unwrap();
        assert_eq!(next.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    fn probe_cors_config(credentials: bool) -> crate::config::CorsConfig {
        crate::config::CorsConfig {
            origins: vec!["https://app.example".to_string()],
            methods: vec!["POST".to_string()],
            headers: Vec::new(),
            credentials,
            max_age: 60,
        }
    }

    fn cross_origin_request(origin: &str) -> Request<Body> {
        Request::builder()
            .uri("/")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap()
    }

    /// A refusal the browser cannot read is not a signal either. The CORS layer
    /// sits *inside* the limiter so a preflight spends a token, which means it
    /// never runs on the way back out of a 429 — and without
    /// `Access-Control-Allow-Origin` the browser reports an opaque CORS failure
    /// instead of the status and the `Retry-After` the limiter already set. A
    /// cross-origin API consumer cannot tell "you are rate limited, retry in N
    /// seconds" from "the network broke".
    #[tokio::test]
    async fn a_rate_limited_cross_origin_request_is_readable_by_the_browser() {
        let mut config = short_circuit_probe_config();
        config.builtin.cors = Some(probe_cors_config(true));
        let app = MiddlewareStack::new(config)
            .apply(Router::new().route("/", get(buffered_response)))
            .expect("probe config is valid");

        let first = app
            .clone()
            .oneshot(cross_origin_request("https://app.example"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(
            first.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&header::HeaderValue::from_static("https://app.example"))
        );

        let limited = app
            .oneshot(cross_origin_request("https://app.example"))
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&header::HeaderValue::from_static("https://app.example")),
            "a 429 without Allow-Origin reads to the browser as a network failure"
        );
        assert_eq!(
            limited
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
            Some(&header::HeaderValue::from_static("true")),
            "a credentialed client cannot read the refusal without Allow-Credentials"
        );
        assert!(
            limited.headers().contains_key(header::RETRY_AFTER),
            "Retry-After is the payload the Allow-Origin header exists to expose"
        );
        assert!(
            limited
                .headers()
                .get(header::VARY)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.to_ascii_lowercase().contains("origin")),
            "the refusal is origin-dependent, so a shared cache must not replay it"
        );
    }

    /// The limiter borrows the CORS *decision*, not a blanket permission to echo
    /// whatever `Origin` arrived. A refusal to an origin outside the allowlist
    /// must stay unreadable, and must still say it varies by origin so a shared
    /// cache cannot replay it to an allowed one.
    #[tokio::test]
    async fn a_rate_limited_disallowed_origin_gets_no_allow_origin_header() {
        let mut config = short_circuit_probe_config();
        config.builtin.cors = Some(probe_cors_config(false));
        let app = MiddlewareStack::new(config)
            .apply(Router::new().route("/", get(buffered_response)))
            .expect("probe config is valid");

        let first = app
            .clone()
            .oneshot(cross_origin_request("https://evil.example"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let limited = app
            .oneshot(cross_origin_request("https://evil.example"))
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            !limited
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            "a disallowed origin must not be echoed on a short-circuit either"
        );
        assert!(
            limited.headers().contains_key(header::VARY),
            "the header-less refusal is still origin-dependent"
        );
    }

    /// With no CORS configured there is no decision to borrow, so a 429 carries
    /// no CORS headers at all — the limiter must not invent a policy.
    #[tokio::test]
    async fn a_rate_limited_request_without_cors_configured_carries_no_cors_headers() {
        let app = MiddlewareStack::new(short_circuit_probe_config())
            .apply(Router::new().route("/", get(buffered_response)))
            .expect("probe config is valid");

        let _ = app
            .clone()
            .oneshot(cross_origin_request("https://app.example"))
            .await
            .unwrap();
        let limited = app
            .oneshot(cross_origin_request("https://app.example"))
            .await
            .unwrap();

        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            !limited
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
    }

    /// `ruvyxa_dev_server` layers `finalize_security_headers` as a
    /// `map_response` **after** calling `apply`, and `Router::layer` wraps
    /// outermost-last, so the framework's own security headers sit outside this
    /// whole stack and reach both short-circuits. That is the reason `GMDT-08`
    /// was scoped to a project's *configured* headers only, and the reason
    /// reordering layers in here cannot silently strip a 429 of them. Asserted
    /// with a stand-in finalizer rather than taken on trust; the crate boundary
    /// is why it is a stand-in.
    #[tokio::test]
    async fn a_response_finalizer_layered_outside_apply_reaches_both_short_circuits() {
        let mut config = short_circuit_probe_config();
        config.builtin.cors = Some(probe_cors_config(false));
        let app = MiddlewareStack::new(config)
            .apply(Router::new().route("/", get(buffered_response)))
            .expect("probe config is valid");
        // The same shape as the dev server's security-header finalizer.
        let app = app.layer(axum::middleware::map_response(
            |mut response: Response<Body>| async move {
                response.headers_mut().insert(
                    "x-content-type-options",
                    header::HeaderValue::from_static("nosniff"),
                );
                response
            },
        ));

        let preflight = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/")
                    .header(header::ORIGIN, "https://app.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert!(preflight.headers().contains_key("x-content-type-options"));

        let limited = app.oneshot(plain_request("/")).await.unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(limited.headers().contains_key("x-content-type-options"));
    }

    #[test]
    fn rejects_rate_limits_that_could_disable_protection() {
        for (max_requests, window_secs) in [(0, 60), (10, 0)] {
            let mut config = MiddlewareConfig::default();
            config.builtin.rate_limit = Some(RateLimitConfig {
                max_requests,
                window_secs,
                key_by: "ip".to_string(),
            });

            assert!(MiddlewareStack::new(config).validate().is_err());
        }
    }

    #[test]
    fn rejects_invalid_plugin_runtime_limits_during_stack_validation() {
        let config = MiddlewareConfig {
            workers: Some(0),
            ..MiddlewareConfig::default()
        };
        assert!(MiddlewareStack::new(config).validate().is_err());

        let config = MiddlewareConfig {
            timeout_ms: Some(0),
            ..MiddlewareConfig::default()
        };
        assert!(MiddlewareStack::new(config).validate().is_err());
    }

    #[test]
    fn apply_refuses_invalid_configuration() {
        let mut config = MiddlewareConfig::default();
        config.builtin.cors = Some(crate::config::CorsConfig {
            origins: vec!["*".to_string()],
            methods: vec!["POST".to_string()],
            headers: Vec::new(),
            credentials: true,
            max_age: 60,
        });

        // An invalid config must fail to build a stack, not degrade to a
        // warning while installing credentialed wildcard CORS.
        assert!(
            MiddlewareStack::new(config)
                .apply(Router::<()>::new())
                .is_err()
        );
    }

    #[test]
    fn rejects_unknown_rate_limit_key_selectors() {
        for key_by in ["forwarded", "header:", "header:invalid header"] {
            let mut config = MiddlewareConfig::default();
            config.builtin.rate_limit = Some(RateLimitConfig {
                max_requests: 10,
                window_secs: 60,
                key_by: key_by.to_string(),
            });

            assert!(MiddlewareStack::new(config).validate().is_err(), "{key_by}");
        }
    }

    #[test]
    fn accepts_ip_and_header_rate_limit_keys() {
        for key_by in ["ip", "header:x-api-key"] {
            let mut config = MiddlewareConfig::default();
            config.builtin.rate_limit = Some(RateLimitConfig {
                max_requests: 10,
                window_secs: 60,
                key_by: key_by.to_string(),
            });

            assert!(MiddlewareStack::new(config).validate().is_ok(), "{key_by}");
        }
    }

    #[test]
    fn rejects_credentialed_wildcard_cors_and_invalid_allowlists() {
        let mut config = MiddlewareConfig::default();
        config.builtin.cors = Some(crate::config::CorsConfig {
            origins: vec!["*".to_string()],
            methods: vec!["POST".to_string()],
            headers: Vec::new(),
            credentials: true,
            max_age: 60,
        });
        assert!(MiddlewareStack::new(config).validate().is_err());

        let mut config = MiddlewareConfig::default();
        config.builtin.cors = Some(crate::config::CorsConfig {
            origins: vec!["https://app.example".to_string()],
            methods: vec!["NOT A METHOD".to_string()],
            headers: Vec::new(),
            credentials: false,
            max_age: 60,
        });
        assert!(MiddlewareStack::new(config).validate().is_err());
    }

    #[tokio::test]
    async fn leaves_unknown_size_streams_uncompressed_and_complete() {
        let app = MiddlewareStack::new(MiddlewareConfig::default())
            .apply(Router::new().route("/stream", get(streamed_response)))
            .expect("default middleware config is valid");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/stream")
                    .header(header::ACCEPT_ENCODING, "gzip, br")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            body,
            &b"streamed response that is deliberately larger than thirty-two bytes"[..]
        );
    }

    #[tokio::test]
    async fn still_compresses_complete_sized_responses() {
        let app = MiddlewareStack::new(MiddlewareConfig::default())
            .apply(Router::new().route("/buffered", get(buffered_response)))
            .expect("default middleware config is valid");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/buffered")
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get(header::CONTENT_ENCODING).unwrap(),
            "gzip"
        );
        assert!(
            !to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
