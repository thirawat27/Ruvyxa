//! Built-in middleware implementations using Tower layers.
//!
//! These are the standard middleware that ship with Ruvyxa, configurable
//! via `ruvyxa.config.ts` under `middleware.builtin`.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, Response, StatusCode, header};
use tower::{Layer, Service};
use tracing::info;

use crate::config::RateLimitConfig;

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
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let start = Instant::now();
            let response = inner.call(request).await?;
            let elapsed = start.elapsed();
            let status = response.status().as_u16();
            info!(
                method = %method,
                path = %path,
                status = status,
                duration_ms = elapsed.as_millis() as u64,
                "request"
            );
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
        let is_preflight = request.method() == axum::http::Method::OPTIONS;
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
            max_requests: config.max_requests,
            window: Duration::from_secs(config.window_secs),
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
        // Default: use peer IP from extensions or fallback
        request
            .extensions()
            .get::<std::net::SocketAddr>()
            .map(|addr| addr.ip().to_string())
            .or_else(|| {
                request
                    .headers()
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.split(',').next())
                    .map(|s| s.trim().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn allow(&self, key: &str) -> bool {
        let mut state = self.state.lock().expect("rate limiter mutex poisoned");
        let now = Instant::now();
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
        let mut inner = self.inner.clone();

        Box::pin(async move {
            if !allowed {
                let response = Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .header("content-type", "text/plain; charset=utf-8")
                    .header("retry-after", "60")
                    .body(Body::from("Rate limit exceeded"))
                    .unwrap();
                return Ok(response);
            }
            inner.call(request).await
        })
    }
}
