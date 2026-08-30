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
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, header};
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

    /// The decision this layer applies, as a value another layer can hold.
    ///
    /// See [`CorsPolicy`] for why that is worth having separately.
    pub fn policy(&self) -> CorsPolicy {
        CorsPolicy {
            origins: self.origins.clone(),
            methods: self.methods.join(", "),
            headers: self.headers.join(", "),
            credentials: self.credentials,
            max_age: self.max_age.to_string(),
        }
    }
}

/// The CORS answer for one request, as a value rather than as a layer.
///
/// Whether an origin is allowed, and what a response then owes it, is a pure
/// function of the configured allowlist and the request's `Origin`. Nothing
/// about it needs the response to have come from the application, so it does not
/// have to be reached through [`CorsService`] — and a short-circuit produced
/// *above* that service cannot reach it that way anyway.
///
/// That is the whole reason this is split out. The rate limiter sits outside
/// CORS so a preflight spends a token, which leaves its 429 with no path back
/// out through the CORS layer. Holding the decision as a value lets
/// [`RateLimitLayerWithKey::with_cors`] attach exactly what the CORS layer would
/// have attached, while the two layers stay separate: the limiter asks this
/// policy a question, it does not answer one.
#[derive(Debug, Clone)]
pub struct CorsPolicy {
    origins: Vec<String>,
    methods: String,
    headers: String,
    credentials: bool,
    max_age: String,
}

impl CorsPolicy {
    /// The `Origin` this request is entitled to have echoed back, if any.
    ///
    /// Returned as an owned value so a caller can compute it before the request
    /// is consumed and use it after — which is what a short-circuit does.
    pub fn allowed_origin(&self, headers: &HeaderMap) -> Option<String> {
        let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
        self.origins
            .iter()
            .any(|allowed| allowed == "*" || allowed == origin)
            .then(|| origin.to_string())
    }

    /// Answer a preflight this policy allows.
    fn preflight_response<B: Default>(&self, origin: &str) -> Response<B> {
        let mut response = Response::new(B::default());
        *response.status_mut() = StatusCode::NO_CONTENT;
        self.apply(&mut response, Some(origin), true);
        response
    }

    /// Attach what a response that is **not** a preflight answer owes its
    /// origin, given the decision [`Self::allowed_origin`] already made.
    ///
    /// Called both by [`CorsService`] on the way back out of the application and
    /// directly by a layer that refused the request before it ever got there.
    pub fn decorate_actual<B>(&self, allowed_origin: Option<&str>, response: &mut Response<B>) {
        match allowed_origin {
            Some(origin) => self.apply(response, Some(origin), false),
            // The response body is identical, but its CORS headers depend on the
            // request Origin. Without `Vary: Origin` on the rejected path a
            // shared cache can store this header-less response and replay it to
            // an allowed origin (and the reverse), which reads to the browser as
            // a random CORS failure.
            None => append_vary_origin(response.headers_mut()),
        }
    }

    /// Attach the CORS headers this response is entitled to.
    ///
    /// `Allow-Methods`, `Allow-Headers`, and `Max-Age` answer a preflight
    /// question, and the Fetch standard has the browser read them only from a
    /// preflight response. Sending them on every actual response is not merely
    /// redundant: it advertises the whole method and header allowlist to any
    /// origin that gets a response at all, and it invites a proxy to cache a
    /// `Max-Age` that was never negotiated. `Allow-Origin`, `Allow-Credentials`,
    /// and `Vary` do belong on both, because the browser checks those on the
    /// actual response too.
    ///
    /// Mirrored by `withCorsHeaders` in
    /// `packages/ruvyxa/runtime/serverless-handler.mjs`. The two hosts serve the
    /// same applications, so a split that held in one and not the other would
    /// make a project's CORS behavior depend on how it was deployed.
    fn apply<B>(&self, response: &mut Response<B>, origin: Option<&str>, preflight: bool) {
        let h = response.headers_mut();
        if let Some(origin) = origin
            && let Ok(value) = HeaderValue::from_str(origin)
        {
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
            append_vary_origin(h);
        }
        if self.credentials {
            h.insert(
                header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
                HeaderValue::from_static("true"),
            );
        }
        if !preflight {
            return;
        }
        if !self.methods.is_empty()
            && let Ok(value) = HeaderValue::from_str(&self.methods)
        {
            h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, value);
        }
        if !self.headers.is_empty()
            && let Ok(value) = HeaderValue::from_str(&self.headers)
        {
            h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
        }
        if let Ok(value) = HeaderValue::from_str(&self.max_age) {
            h.insert(header::ACCESS_CONTROL_MAX_AGE, value);
        }
    }
}

impl<S> Layer<S> for CorsLayer {
    type Service = CorsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CorsService {
            inner,
            policy: self.policy(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CorsService<S> {
    inner: S,
    policy: CorsPolicy,
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
        // The decision is made here, before the request is consumed, so the same
        // answer is available on both sides of the inner call.
        let allowed_origin = self.policy.allowed_origin(request.headers());

        let mut inner = self.inner.clone();
        let policy = self.policy.clone();

        Box::pin(async move {
            // Handle preflight
            if is_preflight && let Some(origin) = &allowed_origin {
                return Ok(policy.preflight_response(origin));
            }

            let mut response = inner.call(request).await?;
            policy.decorate_actual(allowed_origin.as_deref(), &mut response);
            Ok(response)
        })
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

/// The fixed-width map key one client identity is tracked under.
///
/// The identity itself is not bounded and is not ours. `key: "header:x-api-key"`
/// takes the header verbatim, and `stack.rs` accepts any valid header name for
/// `key:`, so the only limit on a tracked key was the server's header size
/// limit — ten thousand of those retain tens of megabytes. The crate docs'
/// "bounded memory" promise was true of the *count* and not of the *size*.
/// Hashing makes it true of both: every key is sixty-four bytes whatever the
/// caller wrote.
///
/// The trade is that a 429 can no longer name the client. Nothing reads a key
/// back out today — `allow` and `retry_after_seconds` only ever look one up —
/// and anything that ever wants to log the client wants the identity, which is
/// the argument to this function and not its result.
fn bounded_key(identity: &str) -> String {
    blake3::hash(identity.as_bytes()).to_hex().to_string()
}

/// In-memory fixed-window rate limiter, keyed by transport peer or by a
/// configured header.
///
/// The general-purpose one of the four limiters catalogued in this crate's
/// module docs, and the one `rateLimit` in `ruvyxa.config.ts` configures.
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
            return bounded_key(value);
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
            return bounded_key("unknown");
        };
        bounded_key(&client_ip(peer.ip(), request.headers(), trusted).to_string())
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
            // The sweep frees only a bucket whose *whole* window has elapsed,
            // so inside one window it can free nothing at all — which is
            // exactly the state one client produces by sending a distinct
            // `X-Api-Key` per request. Refusing here would hand that one
            // client the whole service: every visitor the map has not already
            // seen gets a 429 until the window rolls.
            //
            // "Fail closed" is the right answer when a limiter cannot answer.
            // This one can: it is not out of answers, it is out of slots, and
            // a slot can be taken back. Evict the least recently refilled
            // bucket — the client that has gone quietest — and admit the new
            // one. The evicted client is re-admitted with a full allowance the
            // moment it returns, so the cost of the flood falls on the
            // strictness of the limit rather than on availability. That is the
            // same direction the action limiter's fixed-slot array already
            // accepts, and the opposite of denying a page to everyone.
            while state.len() >= MAX_TRACKED_RATE_LIMIT_KEYS {
                let Some(oldest) = state
                    .iter()
                    .min_by_key(|(_, bucket)| bucket.last_refill)
                    .map(|(tracked, _)| tracked.clone())
                else {
                    break;
                };
                state.remove(&oldest);
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
            cors: None,
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
    /// The CORS decision this layer's own refusal has to answer for itself.
    cors: Option<CorsPolicy>,
}

impl RateLimitLayerWithKey {
    pub fn from_config(config: &RateLimitConfig, trusted: TrustedProxies) -> Self {
        Self {
            limiter: RateLimitLayer::from_config(config),
            key_by: config.key_by.clone(),
            trusted,
            cors: None,
        }
    }

    /// Lend the limiter the CORS decision, so its 429 is one a browser can read.
    ///
    /// [`CorsLayer`] is installed *inside* this one so a preflight spends a
    /// token, which means it never runs on the way back out of a refusal. A 429
    /// with no `Access-Control-Allow-Origin` is not a 429 as far as a
    /// cross-origin caller is concerned — the browser reports an opaque CORS
    /// failure, and the client cannot tell "you are rate limited, retry in N
    /// seconds" from "the network broke", even though `Retry-After` is right
    /// there in the response.
    ///
    /// This does not make the limiter a CORS implementation. It holds the same
    /// [`CorsPolicy`] value the layer applies and asks it the same two
    /// questions, so there is one set of rules and one place to change them.
    /// With no `cors` block configured there is no policy to ask, and the 429
    /// carries no CORS headers at all.
    ///
    /// A rate-limited *preflight* is still a failed preflight: the Fetch
    /// standard requires an ok status for one and 429 is not ok. What this
    /// recovers is the refused **actual** request, which is the case a client
    /// can act on.
    pub fn with_cors(mut self, cors: Option<CorsPolicy>) -> Self {
        self.cors = cors;
        self
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
            cors: self.cors.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: RateLimitLayer,
    key_by: String,
    trusted: TrustedProxies,
    cors: Option<CorsPolicy>,
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
        // The CORS layer is installed inside this one, so it will never see this
        // refusal. Ask its policy the same question here, while the request is
        // still in hand, and let the refusal answer for itself.
        let refusal_cors = (!allowed)
            .then_some(self.cors.as_ref())
            .flatten()
            .map(|policy| (policy.clone(), policy.allowed_origin(request.headers())));
        let mut inner = self.inner.clone();

        Box::pin(async move {
            if !allowed {
                let mut response = Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .header("content-type", "text/plain; charset=utf-8")
                    .header("retry-after", retry_after.unwrap_or(1).to_string())
                    .body(Body::from("Rate limit exceeded"))
                    .unwrap();
                if let Some((policy, allowed_origin)) = refusal_cors {
                    policy.decorate_actual(allowed_origin.as_deref(), &mut response);
                }
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

    /// The limiter's refusal is produced *above* `CorsService` and can never
    /// pass back out through it, so the limiter has to answer the origin itself
    /// from the policy it was lent. Asserted at the layer as well as through the
    /// assembled stack, because the stack is not the only way these compose.
    #[tokio::test]
    async fn a_rate_limited_refusal_carries_the_cors_headers_it_was_lent() {
        let inner = tower::service_fn(|_request: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::from("handled")))
        });
        let mut service = RateLimitLayerWithKey::from_config(
            &RateLimitConfig {
                max_requests: 1,
                window_secs: 60,
                key_by: "ip".to_string(),
            },
            TrustedProxies::default(),
        )
        .with_cors(Some(test_cors_layer().policy()))
        .layer(inner);
        let cross_origin = || {
            Request::builder()
                .header(header::ORIGIN, "https://app.example")
                .body(Body::empty())
                .unwrap()
        };

        assert_eq!(
            service.call(cross_origin()).await.unwrap().status(),
            StatusCode::OK
        );

        let limited = service.call(cross_origin()).await.unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static("https://app.example"))
        );
        assert!(
            limited
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
        );
        assert!(limited.headers().contains_key(header::RETRY_AFTER));
        assert!(
            !limited
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_METHODS),
            "a refusal is not a preflight answer, so it owes no negotiation headers"
        );
    }

    /// The shared preflight table, replayed against the layered stack.
    ///
    /// `tests/packages/ruvyxa/serverless-handler.test.mjs` replays the same
    /// cases against `createHandler`. The two hosts disagreed about this: here
    /// the limiter sits outside CORS so an `OPTIONS` is charged, and the
    /// deployed handler answered preflights before the limiter saw them, so the
    /// same `rateLimit.max` bought a different number of real requests
    /// depending on where a project was deployed.
    #[tokio::test]
    async fn replays_the_shared_preflight_rate_limit_table() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/rate-limit-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("the rate-limit fixture is valid JSON");

        let cases = fixture["preflightCases"]["cases"]
            .as_array()
            .expect("the fixture carries preflight cases");
        assert!(!cases.is_empty(), "an empty table asserts nothing");

        for case in cases {
            let name = case["name"].as_str().expect("each case is named");
            let preflight = case["preflight"].as_bool().unwrap_or(false);
            let inner = tower::service_fn(|_request: Request<Body>| async {
                Ok::<_, Infallible>(Response::new(Body::from("handled")))
            });
            let mut service = RateLimitLayerWithKey::from_config(
                &RateLimitConfig {
                    max_requests: case["max"].as_u64().expect("max") as usize,
                    window_secs: case["windowSeconds"].as_u64().expect("windowSeconds"),
                    key_by: "ip".to_string(),
                },
                TrustedProxies::default(),
            )
            .with_cors(Some(test_cors_layer().policy()))
            .layer(inner);

            let request = || {
                let mut builder = Request::builder().header(header::ORIGIN, "https://app.example");
                if preflight {
                    builder = builder
                        .method(axum::http::Method::OPTIONS)
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST");
                }
                builder.body(Body::empty()).unwrap()
            };

            let refused_at = case["expectRefusedAt"].as_u64().expect("expectRefusedAt") as usize;
            let total = case["requests"].as_u64().expect("requests") as usize;
            let mut refusal = None;
            for attempt in 1..=total {
                let response = service.call(request()).await.unwrap();
                if attempt < refused_at {
                    assert_ne!(
                        response.status(),
                        StatusCode::TOO_MANY_REQUESTS,
                        "{name}: request {attempt} was refused early",
                    );
                } else if attempt == refused_at {
                    assert_eq!(
                        response.status(),
                        StatusCode::TOO_MANY_REQUESTS,
                        "{name}: request {attempt} was not refused, so a preflight cost nothing",
                    );
                    refusal = Some(response);
                }
            }

            let refusal = refusal.expect("the table names a refusal");
            if case["expectAllowOrigin"].as_bool().unwrap_or(false) {
                assert_eq!(
                    refusal.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                    Some(&HeaderValue::from_static("https://app.example")),
                    "{name}: a refusal the browser cannot read is an opaque failure",
                );
            }
            if case["expectNegotiationHeaders"].as_bool() == Some(false) {
                assert!(
                    !refusal
                        .headers()
                        .contains_key(header::ACCESS_CONTROL_ALLOW_METHODS),
                    "{name}: a refusal is not a preflight answer",
                );
            }
        }
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
    ///
    /// The expectations run through [`bounded_key`] because the key is hashed
    /// before it is tracked. What is asserted is unchanged — *which identity*
    /// the limiter chose. Two identities that must stay apart are checked apart
    /// first, so a degenerate hash cannot make the rest of this pass.
    #[test]
    fn the_default_key_believes_a_forwarded_header_only_from_a_trusted_peer() {
        assert_ne!(bounded_key("198.51.100.7"), bounded_key("203.0.113.8"));
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
            bounded_key("198.51.100.7")
        );
        // The configured proxy is believed, and so is loopback.
        assert_eq!(
            RateLimitLayer::extract_key(&forged("10.0.0.9:44321"), "ip", &trusted),
            bounded_key("203.0.113.8")
        );
        assert_eq!(
            RateLimitLayer::extract_key(&forged("127.0.0.1:44321"), "ip", &trusted),
            bounded_key("203.0.113.8")
        );
        // With no allowlist configured, only loopback is a proxy.
        assert_eq!(
            RateLimitLayer::extract_key(
                &forged("10.0.0.9:44321"),
                "ip",
                &TrustedProxies::default()
            ),
            bounded_key("10.0.0.9")
        );

        // `header:` is the application's own key: it is taken from the header
        // and not from the peer, whatever the forwarding rules would say.
        let request = Request::builder()
            .header("x-forwarded-for", "203.0.113.8")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            RateLimitLayer::extract_key(&request, "header:x-forwarded-for", &trusted),
            bounded_key("203.0.113.8")
        );
        // No peer and no configured header leaves nothing to attribute.
        assert_eq!(
            RateLimitLayer::extract_key(&request, "ip", &trusted),
            bounded_key("unknown")
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
                bounded_key("198.51.100.7")
            );
        }

        // Only a peer that cannot be determined either is unattributable.
        let anonymous = Request::builder().body(Body::empty()).unwrap();
        assert_eq!(
            RateLimitLayer::extract_key(&anonymous, "header:x-api-key", &TrustedProxies::default()),
            bounded_key("unknown")
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

    /// Capacity pressure must cost one bucket, never the whole service.
    ///
    /// The sweep above only frees a bucket whose *whole* window has elapsed,
    /// so inside one window it can free nothing — which is precisely the state
    /// one client produces by sending a distinct `X-Api-Key` per request. If
    /// the limiter answers that by refusing, that single client has denied
    /// service to every visitor the map has not already seen.
    #[test]
    fn a_new_client_is_admitted_when_no_tracked_bucket_can_be_swept() {
        let limiter = RateLimitLayer::from_config(&RateLimitConfig {
            max_requests: 1,
            window_secs: 60,
            key_by: "ip".to_string(),
        });
        let now = Instant::now();
        {
            let mut state = limiter.state.lock().unwrap();
            for index in 0..MAX_TRACKED_RATE_LIMIT_KEYS {
                state.insert(
                    format!("flood-{index:05}"),
                    RateBucket {
                        tokens: 0,
                        // Staggered well inside the sixty-second window, so
                        // nothing is sweepable and "least recently refilled"
                        // names exactly one bucket.
                        last_refill: now
                            - Duration::from_millis((MAX_TRACKED_RATE_LIMIT_KEYS - index) as u64),
                    },
                );
            }
        }

        assert!(
            limiter.allow("a-visitor-the-map-has-never-seen"),
            "a full map of unexpired buckets refused a brand new client"
        );

        let state = limiter.state.lock().unwrap();
        assert!(state.contains_key("a-visitor-the-map-has-never-seen"));
        assert_eq!(
            state.len(),
            MAX_TRACKED_RATE_LIMIT_KEYS,
            "admitting a new client must cost exactly one evicted bucket"
        );
        assert!(
            !state.contains_key("flood-00000"),
            "the evicted bucket must be the least recently refilled one"
        );
        assert!(
            state.contains_key(&format!("flood-{:05}", MAX_TRACKED_RATE_LIMIT_KEYS - 1)),
            "the most recently active client must not be the one evicted"
        );
    }

    /// The map key is derived from a value the caller writes.
    ///
    /// `key: "header:x-api-key"` takes the header verbatim and `stack.rs`
    /// accepts any valid header name, so the only bound on a tracked key is the
    /// server's header size limit. Ten thousand of those retain tens of
    /// megabytes, which is the second half of `RUV-H4`.
    #[test]
    fn an_attacker_chosen_header_value_cannot_grow_the_key_it_is_tracked_under() {
        let peer: std::net::SocketAddr = "198.51.100.7:44321".parse().unwrap();
        let huge = "k".repeat(16 * 1024);
        let tracked_key = |value: &str| {
            let mut request = Request::builder()
                .header("x-api-key", value)
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(peer);
            RateLimitLayer::extract_key(&request, "header:x-api-key", &TrustedProxies::default())
        };

        let key = tracked_key(&huge);
        assert!(
            key.len() <= 64,
            "a 16 KB header value produced a {}-byte map key",
            key.len()
        );

        // Bounded, and still one bucket per client: two values a hash must
        // tell apart cannot collapse into one key, or either client limits
        // the other.
        assert_ne!(key, tracked_key(&format!("{huge}x")));
    }

    /// Expand one fixture value: a string, or `{ repeat, times, suffix }`.
    ///
    /// The second form is how a 16 KB header value is written without spelling
    /// it out in the fixture.
    fn fixture_value(spec: &serde_json::Value) -> String {
        if let Some(literal) = spec.as_str() {
            return literal.to_string();
        }
        let unit = spec["repeat"].as_str().unwrap();
        let times = spec["times"].as_u64().unwrap() as usize;
        let suffix = spec["suffix"].as_str().unwrap_or("");
        format!("{}{suffix}", unit.repeat(times))
    }

    /// A count the fixture writes as a number or as the literal `"capacity"`.
    ///
    /// Neither replay may hardcode the cap, or the fixture stops describing the
    /// implementations the day one of them changes it.
    fn fixture_count(spec: &serde_json::Value) -> usize {
        if spec.as_str() == Some("capacity") {
            return MAX_TRACKED_RATE_LIMIT_KEYS;
        }
        spec.as_u64().unwrap() as usize
    }

    fn flood_key(index: usize) -> String {
        format!("flood-{index:05}")
    }

    /// Both languages replay `tests/fixtures/rate-limit-conformance.json`.
    ///
    /// The deployed half is `rate limiter conformance with the native
    /// middleware` in `tests/packages/ruvyxa/serverless-handler.test.mjs`, over
    /// `rateLimitKey` and `consumeFixedWindow` in
    /// `packages/ruvyxa/runtime/serverless-handler.mjs`. One
    /// `middleware.builtin.rate` block is enforced by both hosts and nothing
    /// held them to one answer, so `RUV-H4` was fixed here while every deployed
    /// build still refused each unseen key once its map filled with buckets
    /// that no sweep could free.
    #[test]
    fn the_shared_rate_limit_conformance_table_is_answered_the_same_way() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/rate-limit-conformance.json"
        ))
        .unwrap();
        let max_key_length = fixture["maxKeyLength"].as_u64().unwrap() as usize;

        for case in fixture["keyCases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let key_by = case["keyBy"].as_str().unwrap();
            let mut builder = Request::builder();
            if let Some(header_name) = key_by.strip_prefix("header:")
                && !case["keyHeader"].is_null()
            {
                builder = builder.header(header_name, fixture_value(&case["keyHeader"]));
            }
            let mut request = builder.body(Body::empty()).unwrap();
            // How the client is attributed is host-local. This host reads the
            // transport peer; a deployed function has none and reads the
            // forwarded chain instead.
            if let Some(client) = case["client"].as_str() {
                let peer: std::net::SocketAddr = format!("{client}:44321").parse().unwrap();
                request.extensions_mut().insert(peer);
            }

            let key = RateLimitLayer::extract_key(&request, key_by, &TrustedProxies::default());
            assert!(
                key.len() <= max_key_length,
                "a tracked key of {} bytes exceeds the shared bound: {name}",
                key.len()
            );
            assert_eq!(
                key,
                bounded_key(&fixture_value(&case["identity"])),
                "key identity case disagrees with the shared fixture: {name}"
            );
            if !case["distinctFrom"].is_null() {
                // Two clients that share a bucket can each limit the other, so
                // an identity the limiter must tell apart may never collapse
                // into one key.
                assert_ne!(
                    key,
                    bounded_key(&fixture_value(&case["distinctFrom"])),
                    "two identities collapsed into one bucket: {name}"
                );
            }
        }

        for case in fixture["admissionCases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let window_secs = case["windowSeconds"].as_u64().unwrap();
            let limiter = RateLimitLayer::from_config(&RateLimitConfig {
                max_requests: case["max"].as_u64().unwrap() as usize,
                window_secs,
                key_by: "ip".to_string(),
            });
            let now = Instant::now();
            let prefill = &case["prefill"];
            if !prefill.is_null() {
                let count = fixture_count(&prefill["count"]);
                let expired = prefill["state"].as_str() == Some("expired");
                let mut state = limiter.state.lock().unwrap();
                for index in 0..count {
                    state.insert(
                        flood_key(index),
                        RateBucket {
                            tokens: 0,
                            // Staggered well inside the window, oldest first,
                            // so "the least recently refilled bucket" names
                            // exactly one of them and no tie is involved.
                            last_refill: if expired {
                                now - Duration::from_secs(window_secs + 1)
                            } else {
                                now - Duration::from_millis((count - index) as u64)
                            },
                        },
                    );
                }
            }

            for (index, request) in case["requests"].as_array().unwrap().iter().enumerate() {
                let identity = request["identity"].as_str().unwrap();
                assert_eq!(
                    limiter.allow(identity),
                    request["allowed"].as_bool().unwrap(),
                    "request {index} for {identity} disagrees with the shared fixture: {name}"
                );
            }

            let state = limiter.state.lock().unwrap();
            assert_eq!(
                state.len(),
                fixture_count(&case["expectTracked"]),
                "tracked bucket count disagrees with the shared fixture: {name}"
            );
            for request in case["requests"].as_array().unwrap() {
                let identity = request["identity"].as_str().unwrap();
                assert!(
                    state.contains_key(identity),
                    "{identity} was not tracked: {name}"
                );
            }
            if case["expectEvicted"].as_str() == Some("oldest") {
                assert!(
                    !state.contains_key(&flood_key(0)),
                    "the evicted bucket must be the oldest one: {name}"
                );
            }
            if case["expectRetained"].as_str() == Some("newest") {
                assert!(
                    state.contains_key(&flood_key(fixture_count(&prefill["count"]) - 1)),
                    "the most recently active client must not be the one evicted: {name}"
                );
            }
        }
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
