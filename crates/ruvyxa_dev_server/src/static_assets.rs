//! Public-directory and client-bundle static file serving: path safety,
//! image format fallback, ETag/conditional responses, and content types.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ruvyxa_diagnostics::{Result, RuvyxaError};

use crate::apply_security_headers;

/// Maximum number of asset fingerprints kept in memory.
const ASSET_ETAG_CACHE_LIMIT: usize = 1024;

/// How long a file's modification time must be in the past before its ETag is
/// eligible for caching.
///
/// ETags are content hashes, but the cache is keyed by `(len, mtime)`. Several
/// filesystems record mtime with one-second granularity, so two writes inside
/// the same second can leave identical `(len, mtime)` for different bytes.
/// Only fingerprinting files whose mtime is already older than this window
/// removes that ambiguity: any later write necessarily lands in a newer second
/// and therefore misses the cache.
const ASSET_ETAG_SETTLE: Duration = Duration::from_secs(2);

/// Identity of the file a cached ETag was computed from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AssetIdentity {
    len: u64,
    modified: SystemTime,
}

/// Bounded `path -> (identity, etag)` index over settled public assets.
///
/// Public assets ship with `must-revalidate`, so browsers re-ask for files they
/// already hold and the conditional request is the steady-state path, not an
/// edge case. Without this index every one of those revalidations reads the
/// whole file off disk and blake3-hashes it only to answer `304 Not Modified`
/// with an empty body — the exact work a 304 exists to avoid.
///
/// Eviction is insertion-ordered, matching `ContentModuleCache` in the bundler.
#[derive(Default)]
struct AssetEtagCache {
    entries: HashMap<PathBuf, (AssetIdentity, String)>,
    insertion_order: VecDeque<PathBuf>,
}

static ASSET_ETAG_CACHE: LazyLock<Mutex<AssetEtagCache>> =
    LazyLock::new(|| Mutex::new(AssetEtagCache::default()));

/// Current identity of a file, or `None` when its mtime is unreadable.
fn asset_identity(metadata: &std::fs::Metadata) -> Option<AssetIdentity> {
    Some(AssetIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok()?,
    })
}

/// ETag previously computed for exactly these bytes, if still valid.
///
/// A mismatch on either length or mtime is treated as a miss, so a rewritten
/// file never reuses its predecessor's ETag.
fn cached_asset_etag(file: &Path, identity: &AssetIdentity) -> Option<String> {
    let cache = ASSET_ETAG_CACHE.lock().ok()?;
    let (cached_identity, etag) = cache.entries.get(file)?;
    (cached_identity == identity).then(|| etag.clone())
}

/// Record the ETag of a file that has stopped changing.
///
/// Files modified within [`ASSET_ETAG_SETTLE`] are deliberately not recorded:
/// see that constant for why a fresh mtime cannot identify content.
fn store_asset_etag(file: &Path, identity: &AssetIdentity, etag: &str) {
    if !is_settled(identity, SystemTime::now()) {
        return;
    }

    let Ok(mut cache) = ASSET_ETAG_CACHE.lock() else {
        return;
    };
    if cache
        .entries
        .insert(file.to_path_buf(), (identity.clone(), etag.to_string()))
        .is_none()
    {
        cache.insertion_order.push_back(file.to_path_buf());
    }
    while cache.insertion_order.len() > ASSET_ETAG_CACHE_LIMIT {
        let Some(oldest) = cache.insertion_order.pop_front() else {
            break;
        };
        cache.entries.remove(&oldest);
    }
}

/// Whether a file has stopped changing long enough for `(len, mtime)` to
/// identify its bytes.
///
/// An mtime in the future is never settled: clock skew and timestamp-preserving
/// copies both produce one, and neither lets us bound how recently the content
/// was written.
fn is_settled(identity: &AssetIdentity, now: SystemTime) -> bool {
    now.duration_since(identity.modified)
        .is_ok_and(|age| age >= ASSET_ETAG_SETTLE)
}

/// True when the request already holds the version identified by `etag`.
fn request_matches_etag(request_headers: Option<&HeaderMap>, etag: &str) -> bool {
    request_headers
        .and_then(|headers| headers.get(header::IF_NONE_MATCH))
        .is_some_and(|value| etag_matches(value, etag))
}

fn not_modified_response() -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    apply_security_headers(&mut response);
    response
}

pub(crate) async fn serve_public_file(
    public_dir: &Path,
    request_path: &str,
    request_headers: Option<&HeaderMap>,
) -> Result<Option<Response>> {
    let trimmed = request_path.trim_start_matches('/');
    if !is_safe_relative_path(trimmed) {
        return Ok(None);
    }

    let Some(file) = resolve_public_asset(public_dir, trimmed) else {
        return Ok(None);
    };
    let metadata = match tokio::fs::metadata(&file).await {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return Ok(None),
    };
    let identity = asset_identity(&metadata);

    // Answer a revalidation from the fingerprint index, before touching the
    // file. This is the whole point of the index: a 304 carries no body, so
    // reading and hashing the file to produce one is pure waste.
    if let Some(identity) = &identity
        && let Some(etag) = cached_asset_etag(&file, identity)
        && request_matches_etag(request_headers, &etag)
    {
        return Ok(Some(not_modified_response()));
    }

    let content_type = content_type_for(&file);

    // Above the threshold the file reaches the socket as a stream. Reading it in
    // full first makes peak memory the sum of every large asset being served at
    // once — one 200 MB video and a handful of clients is enough to end the
    // process — and none of that memory buys anything, because the bytes are
    // written out and dropped immediately.
    if metadata.len() > streamed_asset_threshold() {
        let identity = identity.as_ref().and_then(weak_validator);
        if let Some(etag) = &identity
            && request_matches_etag(request_headers, etag)
        {
            return Ok(Some(not_modified_response()));
        }
        return Ok(Some(
            streamed_file_response(&file, &metadata, content_type, identity.as_deref()).await?,
        ));
    }

    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read public file {}", file.display()),
            source,
        })?;

    // Always hash the bytes actually being served rather than trusting the
    // index here. The file could have been rewritten between the metadata read
    // and this one, and an ETag that does not describe the response body would
    // stay wrong in every downstream cache until the file changed again.
    let etag = compute_etag(&bytes);
    if let Some(identity) = &identity {
        store_asset_etag(&file, identity, &etag);
    }

    // Check If-None-Match for conditional response
    if request_matches_etag(request_headers, &etag) {
        return Ok(Some(not_modified_response()));
    }

    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600, must-revalidate"),
    );
    apply_security_headers(&mut response);
    Ok(Some(response))
}

/// Size above which a public asset is streamed rather than buffered.
///
/// 8 MiB is chosen so nothing a page actually waits on changes behaviour:
/// scripts, stylesheets, fonts and ordinary images sit far below it, while the
/// files above it — video, archives, large downloads — are already-compressed
/// formats that gain nothing from the compression a buffered body would allow.
const DEFAULT_STREAMED_ASSET_THRESHOLD: u64 = 8 * 1024 * 1024;

const STREAMED_ASSET_THRESHOLD_ENV: &str = "RUVYXA_STREAM_ASSET_THRESHOLD_BYTES";

fn streamed_asset_threshold() -> u64 {
    std::env::var(STREAMED_ASSET_THRESHOLD_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_STREAMED_ASSET_THRESHOLD)
}

/// A weak validator built from the file's size and modification time.
///
/// A streamed response never holds all of its bytes at once, so it cannot carry
/// the content hash the buffered path uses. `W/` marks the difference honestly:
/// the validator identifies a version of the file rather than its exact bytes,
/// which is what HTTP defines weak validators for and what every general-purpose
/// static server sends for a large file.
fn weak_validator(identity: &AssetIdentity) -> Option<String> {
    let modified = identity
        .modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?;
    Some(format!("W/\"{:x}-{:x}\"", identity.len, modified.as_secs()))
}

/// Send a file as a stream, with `Content-Length` so the response is still sized.
async fn streamed_file_response(
    file: &Path,
    metadata: &std::fs::Metadata,
    content_type: &'static str,
    etag: Option<&str>,
) -> Result<Response> {
    let handle = tokio::fs::File::open(file)
        .await
        .map_err(|source| RuvyxaError::Io {
            message: format!("Failed to open public file {}", file.display()),
            source,
        })?;
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(handle));

    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    // Axum cannot infer a length for a stream, and a large download with no
    // length gives the client no progress and forces chunked framing.
    if let Ok(length) = HeaderValue::from_str(&metadata.len().to_string()) {
        headers.insert(header::CONTENT_LENGTH, length);
    }
    if let Some(etag) = etag
        && let Ok(value) = HeaderValue::from_str(etag)
    {
        headers.insert(header::ETAG, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600, must-revalidate"),
    );
    apply_security_headers(&mut response);
    Ok(response)
}

/// Sync fallback for static file serving (used by render_request test/bench path).
pub(crate) fn serve_public_file_sync(
    public_dir: &Path,
    request_path: &str,
) -> Result<Option<Response>> {
    let trimmed = request_path.trim_start_matches('/');
    if !is_safe_relative_path(trimmed) {
        return Ok(None);
    }
    let Some(file) = resolve_public_asset(public_dir, trimmed) else {
        return Ok(None);
    };
    let bytes = fs::read(&file)?;
    let content_type = content_type_for(&file);
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    apply_security_headers(&mut response);
    Ok(Some(response))
}

/// Resolve `/__ruvyxa/client/<name>` to a file inside `client_dir`, or refuse.
///
/// The client bundle directory is flat, so a served name may not carry a
/// separator or a parent segment at all — a stricter rule than
/// `is_safe_relative_path`, which exists to accept nested public assets.
///
/// This is the one place that rule lives. It was written out twice, once in
/// each of the two callers below, which is a copy of a security guard whose
/// halves nothing kept level: `render_pipeline` reaches the sync caller while
/// live requests reach the async one, so a rule added to one of them would have
/// left the other answering for paths it had already been taught to refuse.
fn resolve_client_file(client_dir: &Path, request_path: &str) -> Option<PathBuf> {
    let file_name = request_path.strip_prefix("/__ruvyxa/client/")?;
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return None;
    }
    contained_public_asset(client_dir, &client_dir.join(file_name))
}

/// Sync fallback for client file serving (used by render_request test/bench path).
pub(crate) fn serve_client_file_sync(
    client_dir: &Path,
    request_path: &str,
) -> Result<Option<Response>> {
    let Some(file) = resolve_client_file(client_dir, request_path) else {
        return Ok(None);
    };
    if !file.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&file)?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    apply_security_headers(&mut response);
    Ok(Some(response))
}

pub(crate) async fn serve_client_file(
    client_dir: &Path,
    request_path: &str,
    request_headers: Option<&HeaderMap>,
) -> Result<Option<Response>> {
    let Some(file) = resolve_client_file(client_dir, request_path) else {
        return Ok(None);
    };
    let metadata = match tokio::fs::metadata(&file).await {
        Ok(meta) if meta.is_file() => meta,
        _ => return Ok(None),
    };
    let identity = asset_identity(&metadata);

    // Answer a revalidation from the fingerprint index, before touching the
    // file — the same index `serve_public_file` uses, which this path had never
    // consulted. A 304 carries no body, so reading the bundle and hashing every
    // byte of it only to send an empty response is pure waste.
    if let Some(identity) = &identity
        && let Some(etag) = cached_asset_etag(&file, identity)
        && request_matches_etag(request_headers, &etag)
    {
        return Ok(Some(not_modified_response()));
    }

    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read client file {}", file.display()),
            source,
        })?;

    // Hash the bytes actually being served rather than trusting the index: the
    // file could have been rewritten between the metadata read and this one, and
    // an ETag that does not describe the body stays wrong in every downstream
    // cache until the file changes again.
    let etag = compute_etag(&bytes);
    if let Some(identity) = &identity {
        store_asset_etag(&file, identity, &etag);
    }

    if request_matches_etag(request_headers, &etag) {
        return Ok(Some(not_modified_response()));
    }

    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    apply_security_headers(&mut response);
    Ok(Some(response))
}

/// Map a public URL path to the file that should answer it.
///
/// Resolution is driven entirely by the requested URL extension, never by the
/// `Accept` header, so responses are not content-negotiated and need no `Vary`.
pub(crate) fn resolve_public_asset(public_dir: &Path, request_path: &str) -> Option<PathBuf> {
    let requested = public_dir.join(request_path);
    if requested.is_file() {
        return contained_public_asset(public_dir, &requested);
    }

    // Development keeps source images untouched while the React component
    // points at the production `.webp` URL. Resolve that URL to exactly one
    // source format; ambiguity matches the build-time collision guard.
    if requested.extension().and_then(|value| value.to_str()) == Some("webp") {
        let mut candidates = ["png", "jpg", "jpeg", "PNG", "JPG", "JPEG"]
            .map(|extension| requested.with_extension(extension))
            .into_iter()
            .filter_map(|path| {
                path.is_file()
                    .then(|| contained_public_asset(public_dir, &path))
                    .flatten()
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        if candidates.len() == 1 {
            return candidates.into_iter().next();
        }
    }

    // Keep server deployments compatible with plain `<img src="hero.png">`
    // while the build output stores only `hero.webp`.
    if is_convertible_image_url(&requested) {
        let webp = requested.with_extension("webp");
        if webp.is_file() {
            return contained_public_asset(public_dir, &webp);
        }
    }
    None
}

/// Canonicalize asset paths before serving them so public-directory symlinks
/// cannot expose files outside the configured root.
pub(crate) fn contained_public_asset(public_dir: &Path, candidate: &Path) -> Option<PathBuf> {
    if !public_dir.exists() || !candidate.exists() {
        return None;
    }
    let public_root = ruvyxa_diagnostics::normalized_canonical_path(public_dir);
    let candidate = ruvyxa_diagnostics::normalized_canonical_path(candidate);
    candidate.starts_with(&public_root).then_some(candidate)
}

/// Extensions that only ever name a build or public asset.
///
/// Restricted to images, fonts, media, and emitted web assets: none of these
/// is a plausible value for a dynamic route parameter, so refusing them cannot
/// swallow a real page. Mirrors `STATIC_ASSET_EXTENSIONS` in
/// `packages/ruvyxa/runtime/serverless-handler.mjs`.
const STATIC_ASSET_EXTENSIONS: [&str; 25] = [
    "apng", "avif", "bmp", "css", "eot", "gif", "ico", "jpeg", "jpg", "js", "map", "mjs", "mov",
    "mp3", "mp4", "ogg", "otf", "png", "svg", "ttf", "wav", "webm", "webp", "woff", "woff2",
];

/// True when the last path segment names a static asset file.
///
/// A request that reaches routing with this shape has already missed both the
/// client bundle directory and the public directory, so the file genuinely
/// does not exist and a dynamic route must not render a page for it.
pub(crate) fn is_static_asset_request(request_path: &str) -> bool {
    if is_crawler_discovery_path(request_path) {
        return true;
    }
    let segment = request_path.rsplit('/').next().unwrap_or_default();
    let Some((name, extension)) = segment.rsplit_once('.') else {
        return false;
    };
    if name.is_empty() || extension.is_empty() {
        return false;
    }
    let extension = extension.to_ascii_lowercase();
    STATIC_ASSET_EXTENSIONS.contains(&extension.as_str())
}

/// Well-known crawler files that are never a page.
///
/// `.txt` and `.xml` are deliberately absent from `STATIC_ASSET_EXTENSIONS` —
/// a route may legitimately end in either — but these exact paths are fixed by
/// convention. Letting `/[lang]` answer `/robots.txt` returns 200 with an HTML
/// body, which is exactly what Lighthouse's `robots-txt` audit fails on. The
/// build emits both files by default, so this only decides what a project that
/// turned generation off serves. Mirrors `isCrawlerDiscoveryPath()` in
/// `packages/ruvyxa/runtime/serverless-handler.mjs`.
fn is_crawler_discovery_path(request_path: &str) -> bool {
    matches!(
        request_path.trim_end_matches('/'),
        "/robots.txt" | "/sitemap.xml" | "/sitemap_index.xml"
    )
}

pub(crate) fn is_convertible_image_url(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
}

pub(crate) fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') {
        return false;
    }
    if !Path::new(path)
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return false;
    }

    // Segment rules, checked explicitly rather than left to `Path::components`.
    //
    // The serverless handler decides the same thing in `isUnsafeSegment`, and
    // the two disagreed: `Components` accepted `foo:bar` (only a single-letter
    // `a:` parses as a Windows prefix), accepted control characters, and folded
    // `.` away as `CurDir`, while the deployed handler rejected all three. That
    // made one URL resolve differently under `ruvyxa start` than in a deployed
    // build — and this rule guards a path that is written as well as read, so on
    // Windows `foo:bar` names an NTFS alternate data stream. Both halves are now
    // held to `tests/fixtures/prerender-path-conformance.json`.
    path.split('/').all(|segment| {
        segment.is_empty()
            || (segment != "."
                && segment != ".."
                && !segment
                    .chars()
                    .any(|character| character == ':' || character.is_control()))
    })
}

/// Compute a strong ETag using blake3 hash of file content.
pub(crate) fn compute_etag(bytes: &[u8]) -> String {
    let hash = blake3::hash(bytes);
    format!("\"{}\"", &hash.to_hex()[..16])
}

/// An entity tag reduced to the part that identifies the representation.
///
/// `W/"abc"` and `"abc"` name the same version; the weak marker says only that
/// the two are semantically rather than byte-for-byte equivalent, which is not
/// what an `If-None-Match` comparison is deciding.
fn normalized_etag(value: &str) -> &str {
    value
        .trim()
        .strip_prefix("W/")
        .unwrap_or(value.trim())
        .trim_matches('"')
}

pub(crate) fn etag_matches(value: &HeaderValue, etag: &str) -> bool {
    let Ok(value) = value.to_str() else {
        return false;
    };
    // Normalize both sides the same way. The candidate was already stripped of a
    // `W/` prefix while the target was only unquoted, which was invisible while
    // every validator this server produced was strong — a weak one never matched
    // itself, so a large streamed asset revalidated into a full 200 every time.
    let target = normalized_etag(etag);
    value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || normalized_etag(candidate) == target
    })
}

pub(crate) fn content_type_for(path: &Path) -> &'static str {
    // File-system extensions are case-preserving, and `resolve_public_asset`
    // deliberately resolves upper-case image sources such as `hero.PNG`.
    // Matching case-sensitively here would serve those as a binary download.
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        // `.map` is JSON. Left to the fallback it was served as a binary
        // download, and a browser will not attach a source map it is handed as
        // `application/octet-stream`.
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("webmanifest") => "application/manifest+json; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        // RFC 9309 requires robots.txt to use text/plain. Sitemap XML is
        // likewise served as XML instead of the binary fallback, while the
        // explicit UTF-8 charset matches the generated declarations.
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
        // `WebAssembly.instantiateStreaming` rejects any response that is not
        // `application/wasm`, so the fallback did not merely mislabel the module
        // — it made streaming instantiation fail outright.
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

pub(crate) fn public_asset_links(public_dir: &Path) -> String {
    let mut links = Vec::new();

    if public_dir.join("ruvyxa.png").exists() {
        links.push(r#"<link rel="icon" type="image/png" href="/ruvyxa.png">"#.to_string());
    }

    links.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A large asset must not be read into memory in full before the first byte
    /// is written: peak memory was the sum of every large file being served at
    /// once, with nothing bounding it.
    #[tokio::test]
    async fn a_large_public_asset_is_streamed_rather_than_buffered() {
        let temp = tempfile::tempdir().expect("temp dir");
        let big = vec![b'x'; (DEFAULT_STREAMED_ASSET_THRESHOLD + 1) as usize];
        std::fs::write(temp.path().join("movie.webm"), &big).expect("write");

        let response = serve_public_file(temp.path(), "/movie.webm", None)
            .await
            .expect("serving must succeed")
            .expect("the file exists");

        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/webm");
        assert_eq!(
            response.headers()[header::CONTENT_LENGTH],
            big.len().to_string(),
            "a streamed download still needs its length"
        );
        assert!(
            response.headers()[header::ETAG]
                .to_str()
                .unwrap()
                .starts_with("W/"),
            "a streamed body cannot carry a content hash and must say so"
        );
        // The stream has to actually deliver the file, not just be shaped like one.
        let delivered = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a streamed body must read back");
        assert_eq!(delivered.len(), big.len());
        assert_eq!(&delivered[..], &big[..]);
    }

    /// Everything a page waits on stays on the buffered path, with the strong
    /// content-hash validator it had before.
    #[tokio::test]
    async fn an_ordinary_asset_keeps_its_strong_validator() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("hero.png"), b"png-bytes").expect("write");

        let response = serve_public_file(temp.path(), "/hero.png", None)
            .await
            .expect("serving must succeed")
            .expect("the file exists");

        let etag = response.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            !etag.starts_with("W/"),
            "{etag} should be a strong content-hash validator"
        );

        let delivered = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a buffered body must read back");
        assert_eq!(&delivered[..], b"png-bytes");
    }

    /// A streamed asset must still answer a revalidation without opening the file.
    #[tokio::test]
    async fn a_streamed_asset_answers_its_own_weak_validator() {
        let temp = tempfile::tempdir().expect("temp dir");
        let big = vec![b'x'; (DEFAULT_STREAMED_ASSET_THRESHOLD + 1) as usize];
        std::fs::write(temp.path().join("movie.webm"), &big).expect("write");

        let first = serve_public_file(temp.path(), "/movie.webm", None)
            .await
            .unwrap()
            .unwrap();
        let etag = first.headers()[header::ETAG].clone();

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag);
        let second = serve_public_file(temp.path(), "/movie.webm", Some(&headers))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    }

    /// Replay the shared cross-language path-safety table.
    ///
    /// The deployed handler decides this in `isUnsafeSegment`, and the two had
    /// drifted: `Path::components` accepted `foo:bar`, accepted control
    /// characters, and folded `.` away, while the handler rejected all three —
    /// so one URL resolved differently under `ruvyxa start` than in a deployed
    /// build. This rule also guards a path that is written, not only read.
    /// `tests/packages/ruvyxa/prerender-path.test.mjs` replays the same file.
    #[test]
    fn matches_the_shared_cross_language_path_safety_table() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/prerender-path-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("conformance fixture is valid JSON");

        let cases = fixture["cases"].as_array().expect("fixture declares cases");
        assert!(!cases.is_empty(), "the fixture must carry cases");
        for case in cases {
            let path = case["path"].as_str().expect("case path");
            let expected = case["safe"].as_bool().expect("case verdict");
            let why = case["why"].as_str().unwrap_or("");
            assert_eq!(is_safe_relative_path(path), expected, "{path:?} — {why}");
        }
    }

    /// Replay the shared cross-language static-asset table.
    ///
    /// The JavaScript servers read `STATIC_CONTENT_TYPES` from
    /// `@ruvyxa/core/utils`; this handler cannot, so the two are held together
    /// by `tests/fixtures/static-asset-conformance.json` instead.
    /// `tests/packages/core/static-asset-contract.test.mjs` drives the same file
    /// through the JavaScript table. A change made in one language and not the
    /// other fails here rather than after deployment.
    #[test]
    fn serves_the_shared_cross_language_content_type_table() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/static-asset-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("conformance fixture is valid JSON");

        let content_types = fixture["contentTypes"]
            .as_object()
            .expect("fixture declares contentTypes");
        for (extension, expected) in content_types {
            let expected = expected.as_str().expect("content type is a string");
            let path = PathBuf::from(format!("public/asset.{extension}"));
            assert_eq!(
                content_type_for(&path),
                expected,
                "content type for .{extension}"
            );
        }

        let fallback = fixture["fallbackContentType"]
            .as_str()
            .expect("fixture declares a fallback");
        assert_eq!(
            content_type_for(&PathBuf::from("public/archive.bin")),
            fallback
        );
        assert_eq!(content_type_for(&PathBuf::from("public/LICENSE")), fallback);

        for (spelling, expected) in fixture["caseInsensitiveExamples"]
            .as_object()
            .expect("fixture declares case examples")
        {
            let expected = expected.as_str().expect("content type is a string");
            let path = PathBuf::from(format!("public/asset.{spelling}"));
            assert_eq!(
                content_type_for(&path),
                expected,
                "content type for .{spelling}"
            );
        }

        let declared: Vec<&str> = fixture["staticAssetExtensions"]
            .as_array()
            .expect("fixture declares staticAssetExtensions")
            .iter()
            .map(|value| value.as_str().expect("extension is a string"))
            .collect();
        assert_eq!(
            declared, STATIC_ASSET_EXTENSIONS,
            "the asset-extension list is copied into three languages; the fixture is the one that decides it"
        );
        for extension in &declared {
            assert!(
                is_static_asset_request(&format!("/media/file.{extension}")),
                ".{extension} must be recognized as a static asset request"
            );
            // Recognising a URL as an asset and knowing how to serve the file are
            // two different lists, and they had different membership: a video or
            // font was routed as an asset and then handed over as an opaque
            // download.
            assert_ne!(
                content_type_for(&PathBuf::from(format!("file.{extension}"))),
                fallback,
                ".{extension} is routed as an asset but has no content type"
            );
        }
    }

    /// Serve `file` and report the status plus the body length actually sent.
    async fn serve(
        public_dir: &Path,
        request_path: &str,
        if_none_match: Option<&str>,
    ) -> (StatusCode, usize) {
        let mut headers = HeaderMap::new();
        if let Some(value) = if_none_match {
            headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(value).unwrap());
        }
        let response = serve_public_file(public_dir, request_path, Some(&headers))
            .await
            .expect("serving must not fail")
            .expect("asset must resolve");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body must be readable");
        (status, body.len())
    }

    async fn etag_of(public_dir: &Path, request_path: &str) -> String {
        let response = serve_public_file(public_dir, request_path, None)
            .await
            .expect("serving must not fail")
            .expect("asset must resolve");
        response
            .headers()
            .get(header::ETAG)
            .expect("a served asset carries an ETag")
            .to_str()
            .expect("ETags are ASCII")
            .to_string()
    }

    #[tokio::test]
    async fn conditional_request_for_unchanged_asset_returns_an_empty_304() {
        let temp = tempfile::tempdir().unwrap();
        let public_dir = temp.path();
        fs::write(public_dir.join("logo.png"), vec![7u8; 4096]).unwrap();

        let etag = etag_of(public_dir, "/logo.png").await;
        let (status, body_len) = serve(public_dir, "/logo.png", Some(&etag)).await;

        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert_eq!(body_len, 0, "a 304 must not carry a body");
    }

    #[tokio::test]
    async fn rewritten_asset_never_reuses_the_previous_etag() {
        // The correctness guarantee the fingerprint index must not break: an
        // ETag that outlived its content would stay wrong in every downstream
        // cache until the file changed again.
        let temp = tempfile::tempdir().unwrap();
        let public_dir = temp.path();
        let file = public_dir.join("app.css");

        fs::write(&file, "a{color:red}").unwrap();
        let first = etag_of(public_dir, "/app.css").await;

        fs::write(&file, "a{color:blue}").unwrap();
        let second = etag_of(public_dir, "/app.css").await;

        assert_ne!(first, second, "changed bytes must produce a new ETag");

        let (status, body_len) = serve(public_dir, "/app.css", Some(&first)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a stale ETag must not be answered with 304"
        );
        assert_eq!(body_len, "a{color:blue}".len());
    }

    #[tokio::test]
    async fn served_etag_always_describes_the_bytes_in_the_response() {
        let temp = tempfile::tempdir().unwrap();
        let public_dir = temp.path();
        fs::write(public_dir.join("data.txt"), "ruvyxa").unwrap();

        let response = serve_public_file(public_dir, "/data.txt", None)
            .await
            .unwrap()
            .unwrap();
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        assert_eq!(etag, compute_etag(&body));
    }

    #[test]
    fn only_files_that_stopped_changing_are_fingerprinted() {
        let now = SystemTime::now();
        let settled = AssetIdentity {
            len: 10,
            modified: now - ASSET_ETAG_SETTLE,
        };
        let fresh = AssetIdentity {
            len: 10,
            modified: now,
        };
        // A one-second-granularity filesystem can report this mtime for two
        // different writes, so it must not key a content hash.
        let borderline = AssetIdentity {
            len: 10,
            modified: now - Duration::from_millis(1500),
        };
        let future = AssetIdentity {
            len: 10,
            modified: now + Duration::from_secs(60),
        };

        assert!(is_settled(&settled, now));
        assert!(!is_settled(&fresh, now));
        assert!(!is_settled(&borderline, now));
        assert!(!is_settled(&future, now));
    }

    #[test]
    fn fingerprint_index_stays_bounded() {
        let identity = AssetIdentity {
            len: 1,
            modified: SystemTime::now() - ASSET_ETAG_SETTLE * 2,
        };
        for index in 0..(ASSET_ETAG_CACHE_LIMIT + 64) {
            store_asset_etag(
                &PathBuf::from(format!("/bounded-test/{index}.bin")),
                &identity,
                "\"deadbeefdeadbeef\"",
            );
        }

        let cache = ASSET_ETAG_CACHE.lock().unwrap();
        assert!(cache.entries.len() <= ASSET_ETAG_CACHE_LIMIT);
        assert_eq!(cache.entries.len(), cache.insertion_order.len());
    }

    #[test]
    fn a_changed_length_or_mtime_invalidates_the_fingerprint() {
        let file = PathBuf::from("/fingerprint-test/asset.bin");
        let modified = SystemTime::now() - ASSET_ETAG_SETTLE * 2;
        let identity = AssetIdentity { len: 32, modified };
        store_asset_etag(&file, &identity, "\"0123456789abcdef\"");

        assert_eq!(
            cached_asset_etag(&file, &identity).as_deref(),
            Some("\"0123456789abcdef\"")
        );
        assert_eq!(
            cached_asset_etag(&file, &AssetIdentity { len: 33, modified }),
            None,
            "a different length must miss"
        );
        assert_eq!(
            cached_asset_etag(
                &file,
                &AssetIdentity {
                    len: 32,
                    modified: modified + Duration::from_secs(1),
                }
            ),
            None,
            "a different mtime must miss"
        );
    }
}
