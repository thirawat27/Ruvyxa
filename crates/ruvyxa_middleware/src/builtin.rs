//! Built-in middleware implementations using Tower layers.
//!
//! These are the standard middleware that ship with Ruvyxa, configurable
//! via `ruvyxa.config.ts` under `middleware.builtin`.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, Response, StatusCode, header};
use tower::{Layer, Service};
use tracing::info;

use crate::client_ip::{TrustedProxies, client_ip};
use crate::config::RateLimitConfig;

const MAX_TRACKED_RATE_LIMIT_KEYS: usize = 10_000;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

// ─── Timing Layer ──────────────────────────────────────────────────────────────

/// Adds `X-Response-Time` header to all responses.
#[derive(Debug, Clone)]
pub struct TimingLayer;

impl<S> Layer<S> for TimingLayer {
    type Service = TimingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TimingService { inner }
    }
}

#[derive(Debug, Clone)]
pub struct TimingService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for TimingService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let start = Instant::now();
            let mut response = inner.call(request).await?;
            let elapsed = start.elapsed();
            let timing = format!("{}ms", elapsed.as_millis());
            if let Ok(value) = HeaderValue::from_str(&timing) {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static("x-response-time"), value);
            }
            Ok(response)
        })
    }
}

// ─── Request Logging Layer ─────────────────────────────────────────────────────

/// Logs request method, path, status, and duration.
#[derive(Debug, Clone)]
pub struct RequestLoggingLayer;

impl<S> Layer<S> for RequestLoggingLayer {
    type Service = RequestLoggingService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestLoggingService { inner }
    }
}

#[derive(Debug, Clone)]
pub struct RequestLoggingService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for RequestLoggingService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let request_id = request
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "ruvyxa-{:x}",
                    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
                )
            });
        let mut request = request;
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            request.headers_mut().insert("x-request-id", value);
        }
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let start = Instant::now();
            let mut response = inner.call(request).await?;
            let elapsed = start.elapsed();
            let status = response.status().as_u16();
            info!(
                request_id = %request_id,
                method = %method,
                path = %path,
                status = status,
                duration_ms = elapsed.as_millis() as u64,
                "request"
            );
            if let Ok(value) = HeaderValue::from_str(&request_id) {
                response.headers_mut().insert("x-request-id", value);
            }
            Ok(response)
        })
    }
}

// ─── Custom Headers Layer ──────────────────────────────────────────────────────

/// Applies custom response headers from configuration.
#[derive(Debug, Clone)]
pub struct CustomHeadersLayer {
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl CustomHeadersLayer {
    pub fn new(headers: &BTreeMap<String, String>) -> Self {
        let parsed = headers
            .iter()
            .filter_map(|(key, value)| {
                let name = HeaderName::from_bytes(key.as_bytes()).ok()?;
                let value = HeaderValue::from_str(value).ok()?;
                Some((name, value))
            })
            .collect();
        Self { headers: parsed }
    }
}

impl<S> Layer<S> for CustomHeadersLayer {
    type Service = CustomHeadersService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CustomHeadersService {
            inner,
            headers: self.headers.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CustomHeadersService<S> {
    inner: S,
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for CustomHeadersService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let mut inner = self.inner.clone();
        let headers = self.headers.clone();

        Box::pin(async move {
            let mut response = inner.call(request).await?;
            for (name, value) in headers {
                response.headers_mut().insert(name, value);
            }
            Ok(response)
        })
    }
}

// ─── CORS Layer ────────────────────────────────────────────────────────────────

/// Simple CORS middleware.
#[derive(Debug, Clone)]
pub struct CorsLayer {
    pub origins: Vec<String>,
    pub methods: Vec<String>,
    pub headers: Vec<String>,
    pub credentials: bool,
    pub max_age: u64,
}

impl CorsLayer {
    pub fn from_config(config: &super::config::CorsConfig) -> Self {
        Self {
            origins: config.origins.clone(),
            methods: config.methods.clone(),
            headers: config.headers.clone(),
            credentials: config.credentials,
            max_age: config.max_age,
        }
    }
}

impl<S> Layer<S> for CorsLayer {
    type Service = CorsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CorsService {
            inner,
            origins: self.origins.clone(),
            methods: self.methods.join(", "),
            headers: self.headers.join(", "),
            credentials: self.credentials,
            max_age: self.max_age.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CorsService<S> {
    inner: S,
    origins: Vec<String>,
    methods: String,
    headers: String,
    credentials: bool,
    max_age: String,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for CorsService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let is_preflight = request.method() == axum::http::Method::OPTIONS
            && request
                .headers()
                .get(header::ACCESS_CONTROL_REQUEST_METHOD)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| axum::http::Method::from_bytes(value.as_bytes()).ok())
                .is_some();
        let origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let mut inner = self.inner.clone();
        let allowed_origins = self.origins.clone();
        let methods = self.methods.clone();
        let headers = self.headers.clone();
        let credentials = self.credentials;
        let max_age = self.max_age.clone();

        Box::pin(async move {
            let origin_allowed = match &origin {
                Some(origin) => {
                    allowed_origins.contains(&"*".to_string()) || allowed_origins.contains(origin)
                }
                None => false,
            };

            // Handle preflight
            if is_preflight && origin_allowed {
                let mut response = Response::new(ResBody::default());
                *response.status_mut() = StatusCode::NO_CONTENT;
                apply_cors_headers(
                    &mut response,
                    origin.as_deref(),
                    &methods,
                    &headers,
                    credentials,
                    &max_age,
                    true,
                );
                return Ok(response);
            }

            let mut response = inner.call(request).await?;
            if origin_allowed {
                apply_cors_headers(
                    &mut response,
                    origin.as_deref(),
                    &methods,
                    &headers,
                    credentials,
                    &max_age,
                    false,
                );
            } else {
                // The response body is identical, but its CORS headers depend on
                // the request Origin. Without `Vary: Origin` on the rejected
                // path a shared cache can store this header-less response and
                // replay it to an allowed origin (and the reverse), which reads
                // to the browser as a random CORS failure.
                append_vary_origin(response.headers_mut());
            }
            Ok(response)
        })
    }
}

/// Attach the CORS headers this response is entitled to.
///
/// `Allow-Methods`, `Allow-Headers`, and `Max-Age` answer a preflight question,
/// and the Fetch standard has the browser read them only from a preflight
/// response. Sending them on every actual response is not merely redundant: it
/// advertises the whole method and header allowlist to any origin that gets a
/// response at all, and it invites a proxy to cache a `Max-Age` that was never
/// negotiated. `Allow-Origin`, `Allow-Credentials`, and `Vary` do belong on
/// both, because the browser checks those on the actual response too.
///
/// Mirrored by `withCorsHeaders` in
/// `packages/ruvyxa/runtime/serverless-handler.mjs`. The two hosts serve the
/// same applications, so a split that held in one and not the other would make
/// a project's CORS behavior depend on how it was deployed.
fn apply_cors_headers<B>(
    response: &mut Response<B>,
    origin: Option<&str>,
    methods: &str,
    headers_str: &str,
    credentials: bool,
    max_age: &str,
    preflight: bool,
) {
    let h = response.headers_mut();
    if let Some(origin) = origin
        && let Ok(value) = HeaderValue::from_str(origin)
    {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        append_vary_origin(h);
    }
    if credentials {
        h.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }
    if !preflight {
        return;
    }
    if !methods.is_empty()
        && let Ok(value) = HeaderValue::from_str(methods)
    {
        h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, value);
    }
    if !headers_str.is_empty()
        && let Ok(value) = HeaderValue::from_str(headers_str)
    {
        h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
    }
    if let Ok(value) = HeaderValue::from_str(max_age) {
        h.insert(header::ACCESS_CONTROL_MAX_AGE, value);
    }
}

fn append_vary_origin(headers: &mut axum::http::HeaderMap) {
    let mut values = headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !values
        .iter()
        .any(|value| value.eq_ignore_ascii_case("origin"))
    {
        values.push("Origin".to_string());
    }
    if let Ok(value) = HeaderValue::from_str(&values.join(", ")) {
        headers.insert(header::VARY, value);
    }
}

// ─── Rate Limiting Layer ───────────────────────────────────────────────────────

/// One client's allowance for the window it is currently inside.
///
/// `last_refill` is the start of that window, not a continuously advancing
/// refill clock: it is stamped when the bucket is created or rolled over and
/// then left alone until the whole window elapses. That makes this a **fixed
/// window**, and the consequence is worth stating rather than discovering — a
/// client can spend its full allowance at the end of one window and again at
/// the start of the next, so a short burst of up to `2 * max_requests` is
/// reachable across a window boundary. The limit this enforces is a sustained
/// rate, not an instantaneous one.
#[derive(Debug)]
struct RateBucket {
    tokens: usize,
    last_refill: Instant,
}

/// In-memory fixed-window rate limiter, keyed by transport peer or by a
/// configured header.
#[derive(Debug, Clone)]
pub struct RateLimitLayer {
    max_requests: usize,
    window: Duration,
    state: Arc<Mutex<BTreeMap<String, RateBucket>>>,
}

impl RateLimitLayer {
    pub fn from_config(config: &RateLimitConfig) -> Self {
        Self {
            // MiddlewareStack rejects zero values at startup. Keep this public
            // constructor safe for direct Tower users that bypass that stack.
            max_requests: config.max_requests.max(1),
            window: Duration::from_secs(config.window_secs.max(1)),
            state: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn extract_key(request: &Request<Body>, key_by: &str, trusted: &TrustedProxies) -> String {
        if let Some(header_name) = key_by.strip_prefix("header:")
            && let Some(value) = request
                .headers()
                .get(header_name)
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.is_empty())
        {
            return value.to_string();
        }
        // Falling back to the client address, not to a shared literal. A
        // request that is missing the configured header is not the *same*
        // client as every other request missing it, and bucketing them together
        // turns the limiter into an outage: one caller that never sends the
        // header drains a bucket every other such caller has to share, so the
        // control meant to protect the service is what denies it.
        //
        // Who the client *is* comes from [`crate::client_ip`], which is also
        // what the server-action limiter and the deployed handler ask. This
        // used to be the transport peer and nothing else, so behind any reverse
        // proxy every caller shared one bucket here while the same
        // configuration limited per real client once deployed. A forwarded
        // header is still believed only when the peer that sent it is loopback
        // or listed in `security.trustedProxyIps`.
        let Some(peer) = request.extensions().get::<std::net::SocketAddr>() else {
            return "unknown".to_string();
        };
        client_ip(peer.ip(), request.headers(), trusted).to_string()
    }

    fn allow(&self, key: &str) -> bool {
        let Ok(mut state) = self.state.lock() else {
            tracing::error!("rate limiter mutex poisoned; rejecting request");
            return false;
        };
        let now = Instant::now();
        // A key whose window has rolled over is refilled in place below, not
        // removed and re-inserted. Removing it first made an already-tracked
        // client indistinguishable from a brand new one, so at capacity the
        // guard below would answer a returning client — whose bucket the map
        // still had room for — with 429 until the sweep happened to free a
        // slot. It also made the refill branch unreachable, which is why the
        // window rollover had only one real implementation despite two.
        if !state.contains_key(key) && state.len() >= MAX_TRACKED_RATE_LIMIT_KEYS {
            // The ordinary path only examines the current key. A full sweep is
            // reserved for capacity pressure so high-cardinality traffic cannot
            // make every request scan the whole map while holding this mutex.
            state.retain(|_, bucket| now.duration_since(bucket.last_refill) < self.window);
            if state.len() >= MAX_TRACKED_RATE_LIMIT_KEYS {
                return false;
            }
        }
        let bucket = state.entry(key.to_string()).or_insert(RateBucket {
            tokens: self.max_requests,
            last_refill: now,
        });

        // Refill tokens if the window has elapsed. This is the single place a
        // window rollover happens, for a bucket that was already tracked and
        // for one just created alike.
        let elapsed = now.duration_since(bucket.last_refill);
        if elapsed >= self.window {
            bucket.tokens = self.max_requests;
            bucket.last_refill = now;
        }

        if bucket.tokens > 0 {
            bucket.tokens -= 1;
            true
        } else {
            false
        }
    }

    fn retry_after_seconds(&self, key: &str) -> u64 {
        let Ok(state) = self.state.lock() else {
            return 1;
        };
        state
            .get(key)
            .map(|bucket| {
                self.window
                    .saturating_sub(bucket.last_refill.elapsed())
                    .as_secs()
                    .max(1)
            })
            .unwrap_or(1)
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: self.clone(),
            key_by: "ip".to_string(),
            // A direct Tower user configured no proxy allowlist, so only a
            // loopback peer may state a forwarded identity.
            trusted: TrustedProxies::default(),
        }
    }
}

/// Wraps the `RateLimitLayer` with a specific key extraction strategy.
#[derive(Clone)]
pub struct RateLimitLayerWithKey {
    pub limiter: RateLimitLayer,
    pub key_by: String,
    /// Reverse proxies whose forwarded client header may be believed.
    pub trusted: TrustedProxies,
}

impl RateLimitLayerWithKey {
    pub fn from_config(config: &RateLimitConfig, trusted: TrustedProxies) -> Self {
        Self {
            limiter: RateLimitLayer::from_config(config),
            key_by: config.key_by.clone(),
            trusted,
        }
    }
}

impl<S> Layer<S> for RateLimitLayerWithKey {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: self.limiter.clone(),
            key_by: self.key_by.clone(),
            trusted: self.trusted.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: RateLimitLayer,
    key_by: String,
    trusted: TrustedProxies,
}

impl<S> Service<Request<Body>> for RateLimitService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let key = RateLimitLayer::extract_key(&request, &self.key_by, &self.trusted);
        let allowed = self.limiter.allow(&key);
        let retry_after = (!allowed).then(|| self.limiter.retry_after_seconds(&key));
        let mut inner = self.inner.clone();

        Box::pin(async move {
            if !allowed {
                let response = Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .header("content-type", "text/plain; charset=utf-8")
                    .header("retry-after", retry_after.unwrap_or(1).to_string())
                    .body(Body::from("Rate limit exceeded"))
                    .unwrap();
                return Ok(response);
            }
            inner.call(request).await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;

    fn test_cors_layer() -> CorsLayer {
        CorsLayer {
            origins: vec!["https://app.example".to_string()],
            methods: vec!["GET".to_string(), "POST".to_string(), "OPTIONS".to_string()],
            headers: vec!["Content-Type".to_string()],
            credentials: true,
            max_age: 3600,
        }
    }

    #[tokio::test]
    async fn ordinary_options_requests_reach_the_inner_service() {
        let inner = tower::service_fn(|_request: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::from("handled")))
        });
        let mut service = test_cors_layer().layer(inner);
        let request = Request::builder()
            .method(axum::http::Method::OPTIONS)
            .header(header::ORIGIN, "https://app.example")
            .body(Body::empty())
            .unwrap();

        let response = service.call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn cors_preflight_requests_are_short_circuited() {
        let inner = tower::service_fn(|_request: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::from("handled")))
        });
        let mut service = test_cors_layer().layer(inner);
        let request = Request::builder()
            .method(axum::http::Method::OPTIONS)
            .header(header::ORIGIN, "https://app.example")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .body(Body::empty())
            .unwrap();

        let response = service.call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://app.example"))
        );
    }

    /// The preflight-only headers are the ones a browser reads from a preflight
    /// response and nowhere else. This asserts both halves in one place so a
    /// change that moves a header across the line has to move this test too.
    #[tokio::test]
    async fn cors_sends_negotiation_headers_on_preflight_responses_only() {
        let preflight_only = [
            header::ACCESS_CONTROL_ALLOW_METHODS,
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            header::ACCESS_CONTROL_MAX_AGE,
        ];
        let both = [
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        ];

        let inner = tower::service_fn(|_request: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::from("handled")))
        });
        let mut service = test_cors_layer().layer(inner);
        let preflight = service
            .call(
                Request::builder()
                    .method(axum::http::Method::OPTIONS)
                    .header(header::ORIGIN, "https://app.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        for name in preflight_only.iter().chain(both.iter()) {
            assert!(
                preflight.headers().contains_key(name),
                "preflight response is missing {name}"
            );
        }

        let inner = tower::service_fn(|_request: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::from("handled")))
        });
        let mut service = test_cors_layer().layer(inner);
        let actual = service
            .call(
                Request::builder()
                    .header(header::ORIGIN, "https://app.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        for name in &preflight_only {
            assert!(
                !actual.headers().contains_key(name),
                "actual response should not carry {name}"
            );
        }
        for name in &both {
            assert!(
                actual.headers().contains_key(name),
                "actual response is missing {name}"
            );
        }
        assert!(actual.headers().contains_key(header::VARY));
    }

    #[tokio::test]
    async fn cors_preserves_every_existing_vary_field_value() {
        let inner = tower::service_fn(|_request: Request<Body>| async {
            let mut response = Response::new(Body::empty());
            response
                .headers_mut()
                .append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
            response
                .headers_mut()
                .append(header::VARY, HeaderValue::from_static("Accept-Language"));
            Ok::<_, Infallible>(response)
        });
        let mut service = test_cors_layer().layer(inner);
        let request = Request::builder()
            .header(header::ORIGIN, "https://app.example")
            .body(Body::empty())
            .unwrap();

        let response = service.call(request).await.unwrap();
        let vary = response
            .headers()
            .get_all(header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .collect::<Vec<_>>();

        assert!(
            vary.iter()
                .any(|value| value.eq_ignore_ascii_case("accept-encoding"))
        );
        assert!(
            vary.iter()
                .any(|value| value.eq_ignore_ascii_case("accept-language"))
        );
        assert!(
            vary.iter()
                .any(|value| value.eq_ignore_ascii_case("origin"))
        );
    }

    #[tokio::test]
    async fn cors_marks_responses_as_origin_dependent_even_when_rejected() {
        let inner = tower::service_fn(|_request: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::from("handled")))
        });
        let mut service = test_cors_layer().layer(inner);
        let request = Request::builder()
            .header(header::ORIGIN, "https://attacker.example")
            .body(Body::empty())
            .unwrap();

        let response = service.call(request).await.unwrap();

        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
        let vary = response
            .headers()
            .get_all(header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .collect::<Vec<_>>();
        assert!(
            vary.iter()
                .any(|value| value.eq_ignore_ascii_case("origin")),
            "a shared cache must not reuse this response for an allowed origin"
        );
    }

    /// A client cannot rename itself, and a proxy can.
    ///
    /// `ip` used to mean the transport peer and nothing else, which is safe
    /// against forgery and useless behind a reverse proxy: every caller arrives
    /// with the proxy's address, so one bucket serves the whole internet while
    /// the same configuration limited per real client once deployed. It now
    /// asks [`crate::client_ip`], the rule the action limiter and the deployed
    /// handler already used.
    #[test]
    fn the_default_key_believes_a_forwarded_header_only_from_a_trusted_peer() {
        let trusted = TrustedProxies::parse_all(["10.0.0.9"]).unwrap();
        let forged = |peer: &str| {
            let mut request = Request::builder()
                .header("x-forwarded-for", "203.0.113.8")
                .body(Body::empty())
                .unwrap();
            request
                .extensions_mut()
                .insert(peer.parse::<std::net::SocketAddr>().unwrap());
            request
        };

        // An ordinary client claiming to be someone else stays itself.
        assert_eq!(
            RateLimitLayer::extract_key(&forged("198.51.100.7:44321"), "ip", &trusted),
            "198.51.100.7"
        );
        // The configured proxy is believed, and so is loopback.
        assert_eq!(
            RateLimitLayer::extract_key(&forged("10.0.0.9:44321"), "ip", &trusted),
            "203.0.113.8"
        );
        assert_eq!(
            RateLimitLayer::extract_key(&forged("127.0.0.1:44321"), "ip", &trusted),
            "203.0.113.8"
        );
        // With no allowlist configured, only loopback is a proxy.
        assert_eq!(
            RateLimitLayer::extract_key(
                &forged("10.0.0.9:44321"),
                "ip",
                &TrustedProxies::default()
            ),
            "10.0.0.9"
        );

        // `header:` is the application's own key and stays verbatim.
        let request = Request::builder()
            .header("x-forwarded-for", "203.0.113.8")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            RateLimitLayer::extract_key(&request, "header:x-forwarded-for", &trusted),
            "203.0.113.8"
        );
        // No peer and no configured header leaves nothing to attribute.
        assert_eq!(
            RateLimitLayer::extract_key(&request, "ip", &trusted),
            "unknown"
        );
    }

    #[test]
    fn a_missing_key_header_falls_back_to_the_peer_not_a_shared_bucket() {
        let peer: std::net::SocketAddr = "198.51.100.7:44321".parse().unwrap();
        let mut absent = Request::builder().body(Body::empty()).unwrap();
        absent.extensions_mut().insert(peer);
        let mut empty = Request::builder()
            .header("x-api-key", "")
            .body(Body::empty())
            .unwrap();
        empty.extensions_mut().insert(peer);

        // Two clients that both fail to identify themselves must not land in
        // one bucket, or either of them can rate-limit the other.
        for request in [&absent, &empty] {
            assert_eq!(
                RateLimitLayer::extract_key(
                    request,
                    "header:x-api-key",
                    &TrustedProxies::default()
                ),
                "198.51.100.7"
            );
        }

        // Only a peer that cannot be determined either is unattributable.
        let anonymous = Request::builder().body(Body::empty()).unwrap();
        assert_eq!(
            RateLimitLayer::extract_key(&anonymous, "header:x-api-key", &TrustedProxies::default()),
            "unknown"
        );
    }

    #[test]
    fn evicts_expired_buckets_only_when_capacity_is_reached() {
        let limiter = RateLimitLayer::from_config(&RateLimitConfig {
            max_requests: 1,
            window_secs: 1,
            key_by: "ip".to_string(),
        });
        let expired = Instant::now() - Duration::from_secs(2);
        {
            let mut state = limiter.state.lock().unwrap();
            for index in 0..MAX_TRACKED_RATE_LIMIT_KEYS {
                state.insert(
                    format!("expired-{index}"),
                    RateBucket {
                        tokens: 0,
                        last_refill: expired,
                    },
                );
            }
        }

        assert!(limiter.allow("new-client"));
        let state = limiter.state.lock().unwrap();
        assert_eq!(state.len(), 1);
        assert!(state.contains_key("new-client"));
    }

    #[test]
    fn a_tracked_client_is_refilled_rather_than_rejected_at_capacity() {
        let limiter = RateLimitLayer::from_config(&RateLimitConfig {
            max_requests: 1,
            window_secs: 1,
            key_by: "ip".to_string(),
        });
        let fresh = Instant::now();
        let expired = fresh - Duration::from_secs(2);
        {
            let mut state = limiter.state.lock().unwrap();
            // The map is full and nothing in it is sweepable, so the capacity
            // guard cannot make room. A key already in the map must not need it
            // to: its bucket is already allocated.
            for index in 0..MAX_TRACKED_RATE_LIMIT_KEYS - 1 {
                state.insert(
                    format!("active-{index}"),
                    RateBucket {
                        tokens: 1,
                        last_refill: fresh,
                    },
                );
            }
            state.insert(
                "returning".to_string(),
                RateBucket {
                    tokens: 0,
                    last_refill: expired,
                },
            );
        }

        assert!(
            limiter.allow("returning"),
            "a tracked client whose window rolled over must be refilled, not rejected"
        );
        assert!(
            !limiter.allow("returning"),
            "the refill must hand out exactly one window's worth of tokens"
        );
    }

    #[test]
    fn direct_layer_construction_does_not_disable_limits_for_zero_values() {
        let limiter = RateLimitLayer::from_config(&RateLimitConfig {
            max_requests: 0,
            window_secs: 0,
            key_by: "ip".to_string(),
        });

        assert!(limiter.allow("client"));
        assert!(!limiter.allow("client"));
    }
}
