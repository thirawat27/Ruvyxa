//! Serve a Ruvyxa static build from an [`axum`] application.
//!
//! `ruvyxa build` with `@ruvyxa/adapter-static` writes a publish directory:
//! one `index.html` per pre-rendered route, the hashed client bundles under
//! `__ruvyxa/client/`, everything from `public/`, a `404.html`, and a
//! `_headers` file carrying the cache and security policy every response is
//! meant to leave with. A static host reads that directory as-is. This crate
//! lets a Rust binary do the same, so an API written in axum and the React
//! front end it serves ship as one executable with nothing else installed on
//! the machine — no Node, no second process, no reverse proxy joining the two.
//!
//! ```no_run
//! use axum::{Router, routing::get};
//! use ruvyxa_axum::StaticSite;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let site = StaticSite::open(".ruvyxa/static")?;
//! let app: Router = Router::new()
//!     .route("/api/health", get(|| async { "ok" }))
//!     // The `_headers` policy the published files carry, on your own routes too.
//!     .layer(site.headers_layer())
//!     // Every path the API does not answer is the site: pages, assets, its 404.
//!     .fallback_service(site.router());
//! # let _ = app;
//! # Ok(()) }
//! ```
//!
//! ## What is served, and what is not
//!
//! The static target holds only routes that are fully materialized at build
//! time — SSG and CSR pages. `ruvyxa build --adapter static` refuses a
//! server-rendered, ISR, PPR, or API route with a per-route error before it
//! writes anything, so the directory this crate opens never needs a renderer.
//! A project that needs those routes runs `ruvyxa start` or one of the deploy
//! adapters instead; this crate does not start a JavaScript runtime and has no
//! way to render.
//!
//! ## What the directory decides
//!
//! The policy is the file's. `_headers` is read as Netlify and Cloudflare Pages
//! read it — every rule whose pattern matches the path, in file order, a later
//! rule overriding an earlier one for the same header — so a site deployed
//! there and a site served from this crate answer alike. Two things are
//! decided here, because a rule names a published file and not every response
//! at its path is one:
//!
//! - A caching header (`Cache-Control`, `Expires`, the `CDN-Cache-Control`
//!   family) reaches only a 2xx or 304 response. A 404 for a bundle hash that
//!   has not shipped yet is not that bundle, and `immutable` on it is a year of
//!   cached misses.
//! - A caching header never replaces one the application's own route set, so
//!   `Cache-Control: private` on a per-user response stays private behind
//!   [`StaticSite::headers_layer`].
//!
//! Every other header — the security policy — is on every response, errors
//! included, replacing whatever the route set. `_headers` and `_redirects`
//! themselves configure the host and are not served: they are a 404, as they
//! are on those platforms.
//!
//! A path that names a directory holding an `index.html` is served as that
//! document at the URL asked for, without a redirect, because that is what the
//! Ruvyxa hosts and every static platform do with a pre-rendered route. A miss
//! is the project's own `404.html` with a 404 status.
//!
//! Content types come from the same table `ruvyxa start` and every deployed
//! server use, replayed here from the shared conformance fixture rather than
//! guessed from the file name by a MIME database that disagrees with it about
//! `.js`, `.map`, `.wasm`, and the charset on text.

use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{
    CONTENT_TYPE, HeaderName, HeaderValue, IF_MATCH, IF_MODIFIED_SINCE, IF_NONE_MATCH, IF_RANGE,
    IF_UNMODIFIED_SINCE, RANGE,
};
use axum::http::{Method, Response, StatusCode, Uri};
use axum::middleware::{self, Next};
use tower::{Layer, Service};
use tower_http::services::{ServeDir, ServeFile};

/// The file the static adapter writes its response-header policy to.
const HEADERS_FILE: &str = "_headers";

/// The document a static publish directory answers a miss with.
const NOT_FOUND_PAGE: &str = "404.html";

/// The document a directory is served as when a route pre-rendered into it.
const INDEX_PAGE: &str = "index.html";

/// A publish directory written by `ruvyxa build --adapter static`.
///
/// Opening it reads `_headers` once and checks for `404.html`; nothing else is
/// touched until a request arrives. Cheap to clone — the parsed policy is
/// shared — so one site can hand out its [`router`](Self::router) and its
/// [`headers_layer`](Self::headers_layer) to different parts of an application.
#[derive(Clone, Debug)]
pub struct StaticSite {
    root: PathBuf,
    rules: Arc<[HeaderRule]>,
    has_not_found_page: bool,
}

impl StaticSite {
    /// Open the publish directory at `publish_dir`.
    ///
    /// Fails when the directory does not exist, when `_headers` cannot be
    /// read, or when a line in it is not a header HTTP can send — a policy
    /// file that is half applied is worse than one that is refused at startup.
    pub fn open(publish_dir: impl Into<PathBuf>) -> Result<Self, OpenError> {
        let root: PathBuf = publish_dir.into();
        match std::fs::metadata(&root) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(OpenError::NotADirectory(root)),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(OpenError::MissingDirectory(root));
            }
            Err(source) => return Err(OpenError::Io { path: root, source }),
        }
        let headers_path = root.join(HEADERS_FILE);
        let rules = match std::fs::read_to_string(&headers_path) {
            Ok(text) => parse_headers_file(&text)?,
            // A directory with no `_headers` is a legitimate publish directory;
            // it simply carries no policy. Any other failure is reported, not
            // treated as an empty file.
            Err(source) if source.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                return Err(OpenError::Io {
                    path: headers_path,
                    source,
                });
            }
        };
        let has_not_found_page = root.join(NOT_FOUND_PAGE).is_file();
        Ok(Self {
            root,
            rules: rules.into(),
            has_not_found_page,
        })
    }

    /// The directory this site serves.
    pub fn publish_dir(&self) -> &Path {
        &self.root
    }

    /// A [`tower::Layer`] applying the `_headers` policy to any response.
    ///
    /// The router returned by [`router`](Self::router) already carries it. Add
    /// it to the routes an application defines itself so an API answers with
    /// the same security headers as the pages beside it — otherwise the policy
    /// stops exactly where the published files do.
    pub fn headers_layer(&self) -> HeadersLayer {
        HeadersLayer {
            rules: Arc::clone(&self.rules),
        }
    }

    /// A router serving every file in the publish directory.
    ///
    /// Mount it as a fallback (`Router::fallback_service`) so the application's
    /// own routes win, or nest it under a prefix. `GET` and `HEAD` only; any
    /// other method answers `405` with an `Allow` header. Conditional and range
    /// requests are honoured, and a file reaches the socket as a stream.
    pub fn router(&self) -> Router {
        let resolver = Arc::new(Resolver {
            root: self.root.clone(),
            not_found_page: self
                .has_not_found_page
                .then(|| self.root.join(NOT_FOUND_PAGE)),
        });
        // Resolution is done in `serve_resolved`: `ServeDir` is handed only a
        // file the resolver found, so it must not redirect `/about` to
        // `/about/` either — a pre-rendered route is served at the URL the
        // document links to. Its own not-found service is for a file that
        // went away between the two.
        let files = ServeDir::new(&self.root).append_index_html_on_directories(false);
        let router = if self.has_not_found_page {
            Router::new().fallback_service(
                files.not_found_service(ServeFile::new(self.root.join(NOT_FOUND_PAGE))),
            )
        } else {
            Router::new().fallback_service(files)
        };
        router
            .layer(middleware::from_fn_with_state(resolver, serve_resolved))
            .layer(self.headers_layer())
    }
}

/// Open `publish_dir` and return its router in one call.
///
/// The short form of [`StaticSite::open`] followed by [`StaticSite::router`]
/// for an application that does not need the headers layer separately.
pub fn router(publish_dir: impl Into<PathBuf>) -> Result<Router, OpenError> {
    Ok(StaticSite::open(publish_dir)?.router())
}

/// Why a publish directory could not be opened.
#[derive(Debug)]
pub enum OpenError {
    /// The path does not exist. Usually `ruvyxa build --adapter static` has
    /// not run, or ran into a different `outDir`.
    MissingDirectory(PathBuf),
    /// The path exists and is a file.
    NotADirectory(PathBuf),
    /// Reading the directory or its `_headers` file failed.
    Io {
        /// What was being read.
        path: PathBuf,
        /// The failure.
        source: io::Error,
    },
    /// A line of `_headers` is not a header HTTP can send.
    InvalidHeadersLine {
        /// One-based line number in `_headers`.
        line: usize,
        /// What was wrong with it.
        reason: String,
    },
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDirectory(path) => write!(
                f,
                "publish directory {} does not exist; run `ruvyxa build --adapter static` first",
                path.display()
            ),
            Self::NotADirectory(path) => {
                write!(f, "publish directory {} is not a directory", path.display())
            }
            Self::Io { path, source } => write!(f, "failed to read {}: {source}", path.display()),
            Self::InvalidHeadersLine { line, reason } => {
                write!(f, "{HEADERS_FILE} line {line}: {reason}")
            }
        }
    }
}

impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// `_headers`
// ---------------------------------------------------------------------------

/// One block of `_headers`: a path pattern and the headers it attaches.
#[derive(Clone, Debug)]
struct HeaderRule {
    pattern: String,
    headers: Vec<(HeaderName, HeaderValue)>,
}

/// Parse a `_headers` file as Netlify and Cloudflare Pages read it.
///
/// An unindented line starts a rule with a path pattern; every indented
/// `Name: value` line below it belongs to that rule. Blank lines and `#`
/// comments are skipped. The only wildcard is `*`, which matches any run of
/// characters including `/` — the one form Ruvyxa's adapter writes.
fn parse_headers_file(text: &str) -> Result<Vec<HeaderRule>, OpenError> {
    let mut rules: Vec<HeaderRule> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indented = raw.starts_with(' ') || raw.starts_with('\t');
        if !indented {
            rules.push(HeaderRule {
                pattern: trimmed.to_owned(),
                headers: Vec::new(),
            });
            continue;
        }
        let Some(rule) = rules.last_mut() else {
            return Err(OpenError::InvalidHeadersLine {
                line,
                reason: "a header line appears before any path pattern".to_owned(),
            });
        };
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(OpenError::InvalidHeadersLine {
                line,
                reason: format!("expected `Name: value`, found `{trimmed}`"),
            });
        };
        let name = HeaderName::from_bytes(name.trim().as_bytes()).map_err(|error| {
            OpenError::InvalidHeadersLine {
                line,
                reason: format!("invalid header name `{}`: {error}", name.trim()),
            }
        })?;
        let value =
            HeaderValue::from_str(value.trim()).map_err(|error| OpenError::InvalidHeadersLine {
                line,
                reason: format!("invalid value for `{name}`: {error}"),
            })?;
        rule.headers.push((name, value));
    }
    Ok(rules)
}

/// Whether `pattern` matches `path`, with `*` standing for any run of bytes.
///
/// Anchored at both ends. Byte-wise is sound on UTF-8 because a literal
/// multi-byte character in the pattern is the same byte sequence in the path.
fn glob_matches(pattern: &str, path: &str) -> bool {
    let (pattern, path) = (pattern.as_bytes(), path.as_bytes());
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < path.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            backtrack = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == path[t] {
            p += 1;
            t += 1;
        } else if let Some((star, matched)) = backtrack {
            p = star + 1;
            t = matched + 1;
            backtrack = Some((star, t));
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// The headers every matching rule attaches to `path`, in file order.
fn headers_for(rules: &[HeaderRule], path: &str) -> Vec<(HeaderName, HeaderValue)> {
    rules
        .iter()
        .filter(|rule| glob_matches(&rule.pattern, path))
        .flat_map(|rule| rule.headers.iter().cloned())
        .collect()
}

/// Whether `name` tells a cache how long it may keep the response.
///
/// `Cache-Control` and `Expires` for every cache, and the CDN-targeted
/// variants a shared cache obeys *instead of* `Cache-Control` — which is why a
/// service's own `Cache-Control` has to hold back all of them, not just its
/// namesake: `private, no-store` beside a rule's `CDN-Cache-Control: max-age`
/// is a per-user response the CDN keeps.
fn is_caching_header(name: &HeaderName) -> bool {
    let name = name.as_str();
    matches!(
        name,
        "cache-control" | "expires" | "cdn-cache-control" | "surrogate-control"
    ) || name.ends_with("-cdn-cache-control")
}

/// Whether a response may leave with a caching rule from `_headers`.
///
/// A rule names the published file at that path. A 404 or a 500 is not that
/// file, and a lifetime on it is a lifetime on the failure — `immutable` on
/// the miss for a bundle hash that ships a minute later is a year of 404s
/// from every cache in between. A 304 refreshes the stored 200, so it keeps
/// the rule.
fn may_carry_caching_rules(status: StatusCode) -> bool {
    status.is_success() || status == StatusCode::NOT_MODIFIED
}

/// Applies a site's `_headers` policy to every response of the wrapped service.
///
/// Obtained from [`StaticSite::headers_layer`]. Rules are matched against the
/// request path as the client sent it, and a header a later rule repeats
/// replaces the earlier value. What happens next depends on the header:
///
/// - **Caching headers** (`Cache-Control`, `Expires`, `CDN-Cache-Control`,
///   `Surrogate-Control`, and the `*-CDN-Cache-Control` family) are added only
///   to a 2xx or 304 response, and only when the inner service set none of
///   them. A miss is not the asset its URL names, and a route that answers
///   `Cache-Control: private` knows something about its response a path
///   pattern written for published files cannot.
/// - **Every other header** — the security policy — is set on every response,
///   errors included, replacing a value the inner service set, because the
///   file is the policy.
#[derive(Clone, Debug)]
pub struct HeadersLayer {
    rules: Arc<[HeaderRule]>,
}

impl<S> Layer<S> for HeadersLayer {
    type Service = Headers<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Headers {
            inner,
            rules: Arc::clone(&self.rules),
        }
    }
}

/// The service [`HeadersLayer`] wraps an inner service in.
#[derive(Clone, Debug)]
pub struct Headers<S> {
    inner: S,
    rules: Arc<[HeaderRule]>,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for Headers<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    S::Future: Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let matched = headers_for(&self.rules, request.uri().path());
        let future = self.inner.call(request);
        Box::pin(async move {
            let mut response = future.await?;
            let cacheable = may_carry_caching_rules(response.status());
            let headers = response.headers_mut();
            // Read before anything is inserted, so a rule's own `Expires`
            // does not count as the service having stated a policy.
            let service_stated_caching = headers.keys().any(is_caching_header);
            for (name, value) in matched {
                if is_caching_header(&name) && (!cacheable || service_stated_caching) {
                    continue;
                }
                headers.insert(name, value);
            }
            Ok(response)
        })
    }
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// Files in the publish directory that configure the host rather than belong
/// to the site. Netlify and Cloudflare Pages read them and never serve them;
/// served, `_headers` hands out the policy file and `_redirects` the routing
/// table, neither of which the project published.
const HOST_CONFIG_FILES: [&str; 2] = [HEADERS_FILE, "_redirects"];

/// What `serve_resolved` needs from the site, behind one `Arc`.
struct Resolver {
    root: PathBuf,
    /// `404.html`, when the publish directory has one.
    not_found_page: Option<PathBuf>,
}

/// Which published file a request path names, if any.
struct Resolved {
    /// The request path to hand `ServeDir`, still percent-encoded.
    uri_path: String,
    /// The file relative to the publish directory, for its content type.
    file: PathBuf,
}

impl Resolver {
    /// Resolve `uri_path` the way a static host does: the file itself, or the
    /// `index.html` of the directory it names. `None` is a miss.
    async fn resolve(&self, uri_path: &str) -> Option<Resolved> {
        let relative = safe_relative_path(uri_path)?;
        let candidate = self.root.join(&relative);
        if is_file(&candidate).await {
            if self.is_host_config_file(&relative, &candidate).await {
                return None;
            }
            return Some(Resolved {
                uri_path: uri_path.to_owned(),
                file: relative,
            });
        }
        let index = relative.join(INDEX_PAGE);
        if is_file(&self.root.join(&index)).await {
            let separator = if uri_path.ends_with('/') { "" } else { "/" };
            return Some(Resolved {
                uri_path: format!("{uri_path}{separator}{INDEX_PAGE}"),
                file: index,
            });
        }
        None
    }

    /// Whether `candidate` opens one of [`HOST_CONFIG_FILES`].
    ///
    /// Decided by which file the path opens, not by how it is spelled: on a
    /// case-insensitive filesystem `/_HEADERS` opens `_headers`, and Windows
    /// also opens it as `_headers.`, `_headers ` (trailing dots and spaces are
    /// dropped), `_headers::$DATA` (its default stream), and an 8.3 short
    /// name. A list of spellings would be one Windows quirk short. Only a
    /// file directly in the publish directory can be one of these, so a
    /// nested asset pays nothing.
    async fn is_host_config_file(&self, relative: &Path, candidate: &Path) -> bool {
        if relative.components().count() != 1 {
            return false;
        }
        let Some(identity) = file_identity(candidate).await else {
            return false;
        };
        for name in HOST_CONFIG_FILES {
            if file_identity(&self.root.join(name)).await.as_ref() == Some(&identity) {
                return true;
            }
        }
        false
    }

    /// The answer to a path that names no published file.
    ///
    /// Answered here rather than by handing the request on to `ServeDir`, so
    /// that `resolve` is the one place that decides what is published: a path
    /// it refused is not looked up a second time by a resolver that does not
    /// know which files to withhold.
    async fn miss(&self, request: Request, next: Next) -> Response<Body> {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            // `ServeDir` answers `405` with `Allow` before it touches the disk.
            return next.run(request).await;
        }
        let Some(page) = &self.not_found_page else {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NOT_FOUND;
            return response;
        };
        // The page is the answer to the miss, not a resource the client holds
        // a copy or a range of: a `304` or a `206` of it would be a 404 with
        // no document, or with a fragment of one.
        let (mut parts, body) = request.into_parts();
        for name in [
            RANGE,
            IF_RANGE,
            IF_MATCH,
            IF_NONE_MATCH,
            IF_MODIFIED_SINCE,
            IF_UNMODIFIED_SINCE,
        ] {
            parts.headers.remove(name);
        }
        let mut service = ServeFile::new(page);
        // Both are infallible: an unreadable page is a 404 from `ServeFile`.
        let Ok(()) =
            std::future::poll_fn(|cx| Service::<Request>::poll_ready(&mut service, cx)).await;
        let Ok(response) = service.call(Request::from_parts(parts, body)).await;
        let mut response = response.map(Body::new);
        *response.status_mut() = StatusCode::NOT_FOUND;
        response
    }
}

/// What tells two paths apart as files on this platform.
///
/// A device and inode pair on Unix — exact, and indifferent to case folding,
/// symlinks, and Unicode normalisation alike. Windows has no stable inode in
/// `std`, so there it is the canonical path, which `GetFinalPathNameByHandle`
/// answers with the file's real name whatever spelling opened it.
#[cfg(unix)]
async fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
async fn file_identity(path: &Path) -> Option<PathBuf> {
    tokio::fs::canonicalize(path).await.ok()
}

async fn is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .ok()
        .is_some_and(|metadata| metadata.is_file())
}

/// The publish-directory-relative path a request names, or `None` when the
/// path decodes to something that must not be looked up.
///
/// Every component has to be an ordinary name: `..`, a root, a Windows drive
/// prefix, and a NUL byte are all refused rather than normalised, because the
/// file a normalised path would reach is not one the site published.
fn safe_relative_path(uri_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(uri_path)?;
    if decoded.contains('\0') {
        return None;
    }
    let trimmed = decoded.trim_start_matches('/');
    let mut relative = PathBuf::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(name) => relative.push(name),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(relative)
}

/// Decode `%XX` escapes. `None` when an escape is malformed or the bytes are
/// not UTF-8 — a path that cannot be spelled cannot name a published file.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3)?;
            let high = hex_digit(hex[0])?;
            let low = hex_digit(hex[1])?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Resolve the request to a published file, hand it to `ServeDir`, and label
/// the response with the shared content-type table.
async fn serve_resolved(
    State(resolver): State<Arc<Resolver>>,
    mut request: Request,
    next: Next,
) -> Response<Body> {
    let mut response = match resolver.resolve(request.uri().path()).await {
        Some(resolved) => {
            if resolved.uri_path != request.uri().path()
                && let Some(uri) = with_path(request.uri(), &resolved.uri_path)
            {
                *request.uri_mut() = uri;
            }
            let mut response = next.run(request).await;
            let status = response.status();
            if status.is_success() || status == StatusCode::NOT_MODIFIED {
                response.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static(content_type_for(&resolved.file)),
                );
            }
            response
        }
        None => resolver.miss(request, next).await,
    };
    // A miss answered here, or one `ServeDir` found when the file it was
    // handed went away in between.
    if response.status() == StatusCode::NOT_FOUND && resolver.not_found_page.is_some() {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static(content_type_for(Path::new(NOT_FOUND_PAGE))),
        );
    }
    response
}

/// `uri` with its path replaced and its query kept.
fn with_path(uri: &Uri, path: &str) -> Option<Uri> {
    let path_and_query = match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    };
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query.parse().ok()?);
    Uri::from_parts(parts).ok()
}

/// The content type a published file is served with.
///
/// The same table as `content_type_for` in the Rust host and
/// `STATIC_CONTENT_TYPES` in `@ruvyxa/core/utils`, which this crate cannot
/// import; `tests/fixtures/static-asset-conformance.json` holds the three
/// together, and `serves_the_shared_content_type_table` below replays it.
fn content_type_for(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("webmanifest") => "application/manifest+json; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("wasm") => "application/wasm",
        Some("apng") => "image/apng",
        Some("bmp") => "image/bmp",
        Some("eot") => "application/vnd.ms-fontobject",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("mp4") => "video/mp4",
        Some("ogg") => "audio/ogg",
        Some("otf") => "font/otf",
        Some("ttf") => "font/ttf",
        Some("wav") => "audio/wav",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::to_bytes;
    use axum::http::{Method, header};
    use axum::routing::get;
    use tower::ServiceExt;

    /// The `_headers` the static adapter writes, built from the same fixture
    /// `headersFileContents()` is held to, so a header added there is asked
    /// for here without this file being edited.
    fn security_headers_fixture() -> Vec<(String, String)> {
        let fixture = fixture("security-headers-conformance.json");
        fixture["headers"]
            .as_object()
            .expect("fixture declares headers")
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str().unwrap().to_owned()))
            .collect()
    }

    fn fixture(name: &str) -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name);
        serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
        )
        .expect("conformance fixture is valid JSON")
    }

    /// A publish directory shaped like `ruvyxa build --adapter static` output.
    fn publish_dir() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let site = temp.path().join("site");
        std::fs::create_dir_all(site.join("about")).unwrap();
        std::fs::create_dir_all(site.join("__ruvyxa/client")).unwrap();
        std::fs::create_dir_all(site.join("empty")).unwrap();
        std::fs::write(site.join("index.html"), "<h1>home</h1>").unwrap();
        std::fs::write(site.join("about/index.html"), "<h1>about</h1>").unwrap();
        std::fs::write(site.join("404.html"), "<h1>SMOKE-NOT-FOUND-MARKER</h1>").unwrap();
        std::fs::write(site.join("smoke.svg"), "<svg/>").unwrap();
        std::fs::write(site.join("LICENSE"), "Apache-2.0").unwrap();
        std::fs::write(site.join("__ruvyxa/client/abc123.js"), "export {}").unwrap();
        let mut headers = String::from("/*\n");
        for (name, value) in security_headers_fixture() {
            headers.push_str(&format!("  {name}: {value}\n"));
        }
        headers
            .push_str("/__ruvyxa/client/*\n  Cache-Control: public, max-age=31536000, immutable\n");
        headers.push_str("/*.svg\n  Cache-Control: public, max-age=3600, must-revalidate\n");
        std::fs::write(site.join("_headers"), headers).unwrap();
        // Outside the publish directory: a traversal that reaches it is the bug.
        std::fs::write(temp.path().join("secret.txt"), "not published").unwrap();
        temp
    }

    fn site(temp: &tempfile::TempDir) -> StaticSite {
        StaticSite::open(temp.path().join("site")).unwrap()
    }

    async fn get_path(router: &Router, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
        request(router, Method::GET, path).await
    }

    async fn request(
        router: &Router,
        method: Method,
        path: &str,
    ) -> (StatusCode, axum::http::HeaderMap, String) {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, String::from_utf8(body.to_vec()).unwrap())
    }

    fn header<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).and_then(|value| value.to_str().ok())
    }

    #[tokio::test]
    async fn serves_the_pre_rendered_document_at_the_route_url() {
        let temp = publish_dir();
        let router = site(&temp).router();

        let (status, headers, body) = get_path(&router, "/").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<h1>home</h1>");
        assert_eq!(
            header(&headers, "content-type"),
            Some("text/html; charset=utf-8")
        );

        // A directory route is a document at its own URL, never a redirect —
        // the document links to `/about`, and `ruvyxa start` answers it there.
        for path in ["/about", "/about/", "/about/index.html", "/about?x=1"] {
            let (status, headers, body) = get_path(&router, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(body, "<h1>about</h1>", "{path}");
            assert_eq!(
                header(&headers, "content-type"),
                Some("text/html; charset=utf-8"),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn every_response_carries_the_headers_file_policy() {
        let temp = publish_dir();
        let router = site(&temp).router();
        let expected = security_headers_fixture();

        for path in ["/", "/about", "/smoke.svg", "/nope"] {
            let (_, headers, _) = get_path(&router, path).await;
            for (name, value) in &expected {
                assert_eq!(
                    header(&headers, name),
                    Some(value.as_str()),
                    "{name} on {path}"
                );
            }
        }

        let (status, headers, _) = get_path(&router, "/__ruvyxa/client/abc123.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            header(&headers, "cache-control"),
            Some("public, max-age=31536000, immutable")
        );
        assert_eq!(
            header(&headers, "content-type"),
            Some("text/javascript; charset=utf-8")
        );

        let (_, headers, _) = get_path(&router, "/smoke.svg").await;
        assert_eq!(
            header(&headers, "cache-control"),
            Some("public, max-age=3600, must-revalidate")
        );
        assert_eq!(header(&headers, "content-type"), Some("image/svg+xml"));

        // A document is not an asset: no cache rule names it.
        let (_, headers, _) = get_path(&router, "/").await;
        assert_eq!(header(&headers, "cache-control"), None);
    }

    #[tokio::test]
    async fn a_miss_is_the_projects_own_404_page() {
        let temp = publish_dir();
        let router = site(&temp).router();

        for path in ["/nope", "/nope/", "/empty", "/empty/", "/about/nope.html"] {
            let (status, headers, body) = get_path(&router, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert!(body.contains("SMOKE-NOT-FOUND-MARKER"), "{path}: {body}");
            assert_eq!(
                header(&headers, "content-type"),
                Some("text/html; charset=utf-8"),
                "{path}"
            );
        }

        // The page answers the miss whole: a range or a validator the client
        // sends is about the URL it asked for, not about the 404 document.
        let (_, page_headers, _) = get_path(&router, "/404.html").await;
        let last_modified = header(&page_headers, "last-modified").unwrap().to_owned();
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/nope")
                    .header(header::RANGE, "bytes=0-2")
                    .header(header::IF_MODIFIED_SINCE, last_modified)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"<h1>SMOKE-NOT-FOUND-MARKER</h1>");
    }

    #[tokio::test]
    async fn without_a_404_page_a_miss_is_a_plain_404() {
        let temp = publish_dir();
        std::fs::remove_file(temp.path().join("site/404.html")).unwrap();
        let router = site(&temp).router();
        let (status, headers, body) = get_path(&router, "/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.is_empty());
        assert_eq!(header(&headers, "content-type"), None);
    }

    #[tokio::test]
    async fn traversal_cannot_leave_the_publish_directory() {
        let temp = publish_dir();
        let router = site(&temp).router();
        for path in [
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/about/../../secret.txt",
            "/about/%2e%2e/%2e%2e/secret.txt",
            "/%00",
            "/%zz",
        ] {
            let (status, _, body) = get_path(&router, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert!(!body.contains("not published"), "{path} left the directory");
        }
    }

    #[tokio::test]
    async fn a_file_without_an_extension_is_a_binary_download() {
        let temp = publish_dir();
        let router = site(&temp).router();
        let (status, headers, body) = get_path(&router, "/LICENSE").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "Apache-2.0");
        assert_eq!(
            header(&headers, "content-type"),
            Some("application/octet-stream")
        );
    }

    #[tokio::test]
    async fn head_range_and_conditional_requests_reach_the_file_service() {
        let temp = publish_dir();
        let router = site(&temp).router();

        let (status, headers, body) = request(&router, Method::HEAD, "/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty());
        assert_eq!(
            header(&headers, "content-type"),
            Some("text/html; charset=utf-8")
        );

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/smoke.svg")
                    .header(header::RANGE, "bytes=0-2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header(response.headers(), "content-type"),
            Some("image/svg+xml")
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"<sv");

        let (status, headers, _) = get_path(&router, "/").await;
        let last_modified = header(&headers, "last-modified").unwrap().to_owned();
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::IF_MODIFIED_SINCE, last_modified)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn other_methods_are_refused_with_allow() {
        let temp = publish_dir();
        let router = site(&temp).router();
        let (status, headers, _) = request(&router, Method::POST, "/").await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(header(&headers, "allow"), Some("GET,HEAD"));
    }

    #[tokio::test]
    async fn the_headers_layer_covers_an_applications_own_routes() {
        let temp = publish_dir();
        let site = site(&temp);
        let app = Router::new()
            .route("/api/health", get(|| async { "ok" }))
            .layer(site.headers_layer())
            .fallback_service(site.router());

        let (status, headers, body) = get_path(&app, "/api/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
        for (name, value) in security_headers_fixture() {
            assert_eq!(header(&headers, &name), Some(value.as_str()), "{name}");
        }
        // Not an asset, so no cache rule applies to it either.
        assert_eq!(header(&headers, "cache-control"), None);

        let (status, _, body) = get_path(&app, "/about").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<h1>about</h1>");
    }

    /// A miss is not the asset it names. A CDN in front of this server reads
    /// `immutable` on a 404 for a not-yet-deployed bundle hash as "this URL is
    /// a 404 for a year", and serves that miss after the bundle ships.
    #[tokio::test]
    async fn a_caching_rule_reaches_only_a_response_that_can_be_cached() {
        let temp = publish_dir();
        let site = site(&temp);
        let app = Router::new()
            .route(
                "/__ruvyxa/client/broken.js",
                get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
            )
            .layer(site.headers_layer())
            .fallback_service(site.router());

        for path in [
            "/__ruvyxa/client/missing-hash.js",
            "/missing.svg",
            "/__ruvyxa/client/broken.js",
        ] {
            let (status, headers, _) = get_path(&app, path).await;
            assert!(!status.is_success(), "{path}: {status}");
            assert_eq!(header(&headers, "cache-control"), None, "{path}");
            // The security policy is not a caching rule: every response has it.
            for (name, value) in security_headers_fixture() {
                assert_eq!(
                    header(&headers, &name),
                    Some(value.as_str()),
                    "{name} on {path}"
                );
            }
        }

        // A 304 refreshes the stored response, so it carries the policy.
        let (_, headers, _) = get_path(&app, "/__ruvyxa/client/abc123.js").await;
        let last_modified = header(&headers, "last-modified").unwrap().to_owned();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/__ruvyxa/client/abc123.js")
                    .header(header::IF_MODIFIED_SINCE, last_modified)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            header(response.headers(), "cache-control"),
            Some("public, max-age=31536000, immutable")
        );
    }

    /// A route that answers per user says so with its own caching headers; a
    /// `/*.png` rule meant for published images must not make that response
    /// shared-cacheable — not by replacing `Cache-Control`, and not by adding a
    /// `CDN-Cache-Control` or `Expires` a shared cache would obey instead.
    #[tokio::test]
    async fn a_cache_policy_the_application_set_is_not_replaced() {
        let temp = publish_dir();
        let mut headers = String::from("/*\n  X-Frame-Options: DENY\n");
        headers.push_str(
            "/*.png\n  Cache-Control: public, max-age=3600\n  CDN-Cache-Control: max-age=86400\n  Expires: Thu, 01 Dec 2099 16:00:00 GMT\n",
        );
        std::fs::write(temp.path().join("site/_headers"), headers).unwrap();
        let site = site(&temp);
        let app = Router::new()
            .route(
                "/api/avatar.png",
                get(|| async {
                    (
                        [
                            (header::CACHE_CONTROL, "private, no-store"),
                            (header::X_FRAME_OPTIONS, "SAMEORIGIN"),
                        ],
                        "per-user",
                    )
                }),
            )
            .route("/api/logo.png", get(|| async { "shared" }))
            .layer(site.headers_layer())
            .fallback_service(site.router());

        let (status, headers, _) = get_path(&app, "/api/avatar.png").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(header(&headers, "cache-control"), Some("private, no-store"));
        assert_eq!(header(&headers, "cdn-cache-control"), None);
        assert_eq!(header(&headers, "expires"), None);
        // Security headers keep their rule: the file is the policy for them.
        assert_eq!(header(&headers, "x-frame-options"), Some("DENY"));

        // A response that states no caching policy of its own gets the file's.
        let (status, headers, _) = get_path(&app, "/api/logo.png").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            header(&headers, "cache-control"),
            Some("public, max-age=3600")
        );
        assert_eq!(header(&headers, "cdn-cache-control"), Some("max-age=86400"));
        assert_eq!(
            header(&headers, "expires"),
            Some("Thu, 01 Dec 2099 16:00:00 GMT")
        );
    }

    /// `_headers` and `_redirects` configure the host; no static platform
    /// publishes them, and neither does this one — under any spelling the
    /// filesystem would open as the same file.
    #[tokio::test]
    async fn the_hosts_own_config_files_are_not_published() {
        let temp = publish_dir();
        std::fs::write(temp.path().join("site/_redirects"), "/old /new 301\n").unwrap();
        let router = site(&temp).router();
        for path in [
            "/_headers",
            "/_redirects",
            "/./_headers",
            "/_headers/",
            "/%5Fheaders",
            "/_HEADERS",
            "/_Redirects",
            "/_headers.",
            "/_headers%20",
            "/_headers::$DATA",
        ] {
            for method in [Method::GET, Method::HEAD] {
                let (status, headers, body) = request(&router, method.clone(), path).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
                assert!(!body.contains("Cache-Control"), "{method} {path}: {body}");
                assert!(!body.contains("/old"), "{method} {path}: {body}");
                assert_eq!(header(&headers, "cache-control"), None, "{method} {path}");
                if method == Method::GET {
                    assert!(body.contains("SMOKE-NOT-FOUND-MARKER"), "{path}: {body}");
                }
            }
        }
        // The policy still applies, so the file was read — just not served.
        let (_, headers, _) = get_path(&router, "/").await;
        assert_eq!(header(&headers, "x-frame-options"), Some("DENY"));
    }

    #[tokio::test]
    async fn a_directory_without_a_headers_file_carries_no_policy() {
        let temp = publish_dir();
        std::fs::remove_file(temp.path().join("site/_headers")).unwrap();
        let router = site(&temp).router();
        let (status, headers, _) = get_path(&router, "/").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(header(&headers, "x-frame-options"), None);
    }

    #[test]
    fn opening_reports_what_is_wrong() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("nope");
        assert!(matches!(
            StaticSite::open(&missing),
            Err(OpenError::MissingDirectory(path)) if path == missing
        ));
        let file = temp.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(matches!(
            StaticSite::open(&file),
            Err(OpenError::NotADirectory(path)) if path == file
        ));
        let site = temp.path().join("site");
        std::fs::create_dir(&site).unwrap();
        std::fs::write(site.join("_headers"), "  X-Frame-Options: DENY\n").unwrap();
        let error = StaticSite::open(&site).unwrap_err();
        assert!(
            matches!(&error, OpenError::InvalidHeadersLine { line: 1, .. }),
            "{error}"
        );
        std::fs::write(site.join("_headers"), "/*\n  Bad Name: x\n").unwrap();
        let error = StaticSite::open(&site).unwrap_err();
        assert!(
            matches!(&error, OpenError::InvalidHeadersLine { line: 2, .. }),
            "{error}"
        );
        assert!(error.to_string().contains("_headers line 2"));
    }

    #[test]
    fn headers_file_parsing_matches_how_static_hosts_read_it() {
        let rules = parse_headers_file(
            "# comment\n\n/*\n  X-Frame-Options: DENY\n\t Cache-Control: no-store\n/a/*\n  Cache-Control: public\n",
        )
        .unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].pattern, "/*");
        assert_eq!(rules[0].headers.len(), 2);
        assert_eq!(rules[1].pattern, "/a/*");

        // Later rule wins for the same header; unrelated headers accumulate.
        let applied = headers_for(&rules, "/a/b");
        let mut map = axum::http::HeaderMap::new();
        for (name, value) in applied {
            map.insert(name, value);
        }
        assert_eq!(map.get("cache-control").unwrap(), "public");
        assert_eq!(map.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(headers_for(&rules, "/z").len(), 2);
    }

    #[test]
    fn glob_star_crosses_path_separators_and_is_anchored() {
        assert!(glob_matches("/*", "/"));
        assert!(glob_matches("/*", "/a/b/c.js"));
        assert!(glob_matches(
            "/__ruvyxa/client/*",
            "/__ruvyxa/client/x/y.js"
        ));
        assert!(!glob_matches("/__ruvyxa/client/*", "/__ruvyxa/clients.js"));
        assert!(glob_matches("/*.png", "/a/b.png"));
        assert!(!glob_matches("/*.png", "/a/b.pngx"));
        assert!(!glob_matches("/*.png", "/a/b.jpg"));
        assert!(glob_matches("/a*b*c", "/aXXbYYc"));
        assert!(!glob_matches("/a*b*c", "/aXXbYY"));
        assert!(glob_matches("/exact", "/exact"));
        assert!(!glob_matches("/exact", "/exact/"));
        assert!(glob_matches("/*/ü.svg", "/ä/ü.svg"));
    }

    #[test]
    fn unsafe_paths_never_resolve() {
        assert_eq!(safe_relative_path("/"), Some(PathBuf::new()));
        assert_eq!(
            safe_relative_path("/a/./b"),
            Some(PathBuf::from("a").join("b"))
        );
        assert_eq!(safe_relative_path("/a%20b"), Some(PathBuf::from("a b")));
        assert_eq!(safe_relative_path("/../x"), None);
        assert_eq!(safe_relative_path("/%2e%2e/x"), None);
        assert_eq!(safe_relative_path("/a/%2E%2E/x"), None);
        assert_eq!(safe_relative_path("/%00"), None);
        assert_eq!(safe_relative_path("/%e0%80"), None);
        assert_eq!(safe_relative_path("/%4"), None);
    }

    /// Replays `tests/fixtures/static-asset-conformance.json`, the table the
    /// Rust host and the JavaScript servers are held to; a type changed there
    /// and not here fails here rather than as a mislabelled download.
    #[test]
    fn serves_the_shared_content_type_table() {
        let fixture = fixture("static-asset-conformance.json");
        let content_types = fixture["contentTypes"]
            .as_object()
            .expect("fixture declares contentTypes");
        for (extension, expected) in content_types {
            let expected = expected.as_str().expect("content type is a string");
            assert_eq!(
                content_type_for(Path::new(&format!("asset.{extension}"))),
                expected,
                "content type for .{extension}"
            );
        }
        let fallback = fixture["fallbackContentType"]
            .as_str()
            .expect("fixture declares a fallback");
        assert_eq!(content_type_for(Path::new("archive.bin")), fallback);
        assert_eq!(content_type_for(Path::new("LICENSE")), fallback);
        for (spelling, expected) in fixture["caseInsensitiveExamples"]
            .as_object()
            .expect("fixture declares caseInsensitiveExamples")
        {
            assert_eq!(
                content_type_for(Path::new(&format!("asset.{spelling}"))),
                expected.as_str().unwrap(),
                "content type for .{spelling}"
            );
        }
    }
}
