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
                );
            }
            Ok(response)
        })
    }
}

fn apply_cors_headers<B>(
    response: &mut Response<B>,
    origin: Option<&str>,
    methods: &str,
    headers_str: &str,
    credentials: bool,
    max_age: &str,
) {
    let h = response.headers_mut();
    if let Some(origin) = origin
        && let Ok(value) = HeaderValue::from_str(origin)
    {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
        append_vary_origin(h);
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
    if credentials {
        h.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
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

/// Token-bucket rate limiter state shared across clones.
#[derive(Debug)]
struct RateBucket {
    tokens: usize,
    last_refill: Instant,
}

/// In-memory sliding-window rate limiter keyed by client IP.
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

    fn extract_key(request: &Request<Body>, key_by: &str) -> String {
        if let Some(header_name) = key_by.strip_prefix("header:") {
            return request
                .headers()
                .get(header_name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();
        }
        // Default: use the transport peer only. Forwarded headers are client
        // supplied unless a deployment explicitly selects them with
        // `key: "header:x-forwarded-for"`.
        request
            .extensions()
            .get::<std::net::SocketAddr>()
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn allow(&self, key: &str) -> bool {
        let Ok(mut state) = self.state.lock() else {
            tracing::error!("rate limiter mutex poisoned; rejecting request");
            return false;
        };
        let now = Instant::now();
        let expired_current_key = state
            .get(key)
            .is_some_and(|bucket| now.duration_since(bucket.last_refill) >= self.window);
        if expired_current_key {
            state.remove(key);
        }

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

        // Refill tokens if window has elapsed
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
        }
    }
}

/// Wraps the `RateLimitLayer` with a specific key extraction strategy.
#[derive(Clone)]
pub struct RateLimitLayerWithKey {
    pub limiter: RateLimitLayer,
    pub key_by: String,
}

impl RateLimitLayerWithKey {
    pub fn from_config(config: &RateLimitConfig) -> Self {
        Self {
            limiter: RateLimitLayer::from_config(config),
            key_by: config.key_by.clone(),
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
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: RateLimitLayer,
    key_by: String,
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
        let key = RateLimitLayer::extract_key(&request, &self.key_by);
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

    #[test]
    fn default_rate_limit_key_does_not_trust_forwarded_headers() {
        let request = Request::builder()
            .header("x-forwarded-for", "203.0.113.8")
            .body(Body::empty())
            .unwrap();

        assert_eq!(RateLimitLayer::extract_key(&request, "ip"), "unknown");
        assert_eq!(
            RateLimitLayer::extract_key(&request, "header:x-forwarded-for"),
            "203.0.113.8"
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
