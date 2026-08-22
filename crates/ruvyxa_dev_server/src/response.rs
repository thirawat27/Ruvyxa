//! Response construction and the security headers every Ruvyxa response carries.
//!
//! ## One list of headers
//!
//! The defaults are declared once, in [`DEFAULT_SECURITY_HEADERS`], and both
//! directions read it: [`apply_security_headers`] inserts what is missing and
//! [`finalize_security_headers`] removes what it inserted when a project turns
//! the feature off. They used to be two hand-written sequences of the same seven
//! headers, so adding one meant remembering to add it twice — and forgetting the
//! removal half is silent: `security: false` would keep sending a header the
//! project asked not to send, which nothing tests for and no error reports.
//!
//! The same seven headers are also served by the JavaScript runtimes, from
//! `DEFAULT_SECURITY_HEADERS` in `packages/@ruvyxa/core/src/utils.ts`, which
//! generates the `_headers` file hosts read. That copy cannot import this one,
//! so `tests/fixtures/security-headers-conformance.json` holds both to the same
//! list: a header added to one language and not the other means the same site
//! is protected differently depending on where it is deployed.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use serde::Serialize;

/// Response headers Ruvyxa sends unless the application sets its own.
///
/// Names are lowercase because `HeaderName::from_static` requires it.
pub(crate) const DEFAULT_SECURITY_HEADERS: [(&str, &str); 7] = [
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    ),
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
    ("x-frame-options", "DENY"),
    ("x-permitted-cross-domain-policies", "none"),
];

/// Add every default header the response does not already set.
///
/// An application header always wins: these are defaults, not a policy imposed
/// over what a route deliberately chose.
pub(crate) fn apply_security_headers(response: &mut Response) {
    let headers = response.headers_mut();
    for (name, value) in DEFAULT_SECURITY_HEADERS {
        insert_default_header(
            headers,
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
}

fn insert_default_header(headers: &mut HeaderMap, name: HeaderName, value: HeaderValue) {
    if !headers.contains_key(&name) {
        headers.insert(name, value);
    }
}

/// Apply or strip the defaults according to the project's `security` setting.
///
/// Stripping only removes a header that still holds the exact default value, so
/// a header an application set deliberately survives either way.
pub(crate) fn finalize_security_headers(mut response: Response, enabled: bool) -> Response {
    if enabled {
        apply_security_headers(&mut response);
        return response;
    }
    let headers = response.headers_mut();
    for (name, value) in DEFAULT_SECURITY_HEADERS {
        remove_default_header(headers, HeaderName::from_static(name), value);
    }
    response
}

fn remove_default_header(headers: &mut HeaderMap, name: HeaderName, default_value: &str) {
    if headers
        .get(&name)
        .is_some_and(|value| value.as_bytes() == default_value.as_bytes())
    {
        headers.remove(name);
    }
}

pub(crate) fn with_security_headers(mut response: Response) -> Response {
    apply_security_headers(&mut response);
    response
}

/// Compression level for cached documents.
///
/// Encoding happens once per cached page rather than once per request, so this
/// can afford to be slower and smaller than the streaming default the response
/// layer uses. Brotli 5 is roughly the quality/throughput knee for HTML.
const BROTLI_QUALITY: u32 = 5;
const BROTLI_WINDOW: u32 = 22;
const GZIP_LEVEL: flate2::Compression = flate2::Compression::new(6);

/// Documents below this size are left to the response layer.
///
/// Compressing a few hundred bytes usually makes them bigger, and the header
/// overhead is a larger share of the transfer than anything saved.
const MIN_COMPRESSIBLE_BYTES: usize = 256;

/// Serve a cached document, reusing its stored compressed copy when the client
/// can take it.
///
/// A response that arrives at the outer `CompressionLayer` already carrying
/// `Content-Encoding` is passed through untouched, which is what turns a cache
/// hit into "write bytes we already have" instead of "compress the same page
/// again". Clients that accept neither encoding get the plain document and the
/// layer behaves exactly as it did before.
pub(crate) fn cached_html_response(
    status: StatusCode,
    document: &crate::render_cache::CachedDocument,
    request_headers: Option<&HeaderMap>,
) -> Response {
    let accept = request_headers
        .and_then(|headers| headers.get(header::ACCEPT_ENCODING))
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let encoded = if document.html.len() < MIN_COMPRESSIBLE_BYTES {
        None
    } else {
        document.compressed(
            |encoding| accept_encoding_allows(&accept, encoding),
            |html| encode_document(html, &accept),
        )
    };

    let Some(encoded) = encoded else {
        let mut response = shared_html_response(status, Arc::clone(&document.html));
        // Even an uncompressed answer varies by this header: a shared cache that
        // stored it without `Vary` would replay it to a client that could have
        // had the encoded copy, and vice versa.
        insert_default_header(
            response.headers_mut(),
            header::VARY,
            HeaderValue::from_static("accept-encoding"),
        );
        return response;
    };

    let mut response = html_response_from_body(
        status,
        Body::from(Bytes::from_owner(EncodedBytes(Arc::clone(&encoded.bytes)))),
    );
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static(encoded.encoding),
    );
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    response
}

/// Lets `Bytes` borrow the cached encoded copy instead of copying it per request.
struct EncodedBytes(Arc<[u8]>);

impl AsRef<[u8]> for EncodedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Whether an `Accept-Encoding` value permits `encoding`.
///
/// Deliberately conservative: an explicit `;q=0` is a refusal, and anything this
/// does not understand falls back to sending the document uncompressed, which is
/// always a valid answer.
fn accept_encoding_allows(accept: &str, encoding: &str) -> bool {
    accept.split(',').any(|part| {
        let mut pieces = part.split(';');
        let name = pieces.next().unwrap_or_default().trim();
        if name != encoding {
            return false;
        }
        !pieces.any(|parameter| parameter.trim().replace(' ', "") == "q=0")
    })
}

/// Encode once, preferring brotli, for storage in the document's shared slot.
fn encode_document(html: &str, accept: &str) -> Option<crate::render_cache::CompressedDocument> {
    use crate::render_cache::CompressedDocument;
    use std::io::Write;

    if accept_encoding_allows(accept, "br") {
        let mut encoded = Vec::with_capacity(html.len() / 3);
        let mut writer =
            brotli::CompressorWriter::new(&mut encoded, 4096, BROTLI_QUALITY, BROTLI_WINDOW);
        if writer.write_all(html.as_bytes()).is_ok() {
            drop(writer);
            return Some(CompressedDocument {
                encoding: "br",
                bytes: Arc::from(encoded.into_boxed_slice()),
            });
        }
    }

    if accept_encoding_allows(accept, "gzip") {
        let mut writer =
            flate2::write::GzEncoder::new(Vec::with_capacity(html.len() / 3), GZIP_LEVEL);
        if writer.write_all(html.as_bytes()).is_ok()
            && let Ok(encoded) = writer.finish()
        {
            return Some(CompressedDocument {
                encoding: "gzip",
                bytes: Arc::from(encoded.into_boxed_slice()),
            });
        }
    }

    None
}

pub(crate) fn html_response(status: StatusCode, body: String) -> Response {
    html_response_from_body(status, Body::from(body))
}

/// Serve an HTML document that is already stored behind an [`Arc<str>`].
///
/// The render cache hands out shared allocations, so a cache hit can build the
/// response body without copying the document. Building it from a `String`
/// instead meant one full copy of every cached page on every hit.
pub(crate) fn shared_html_response(status: StatusCode, body: Arc<str>) -> Response {
    html_response_from_body(status, shared_text_body(body))
}

/// Lets `Bytes` borrow an `Arc<str>` as its backing storage.
struct SharedText(Arc<str>);

impl AsRef<[u8]> for SharedText {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Build a response body from a shared string without copying it.
pub(crate) fn shared_text_body(text: Arc<str>) -> Body {
    Body::from(Bytes::from_owner(SharedText(text)))
}

/// Serve an HTML document that is still being produced.
///
/// `no-store` because there is nothing to store: the document is assembled per
/// request and never becomes a string this process holds. Marked explicitly
/// rather than left to default, so nothing between here and the browser decides
/// on its own that a `200` without a length is reusable.
pub(crate) fn streamed_html_response(body: Body) -> Response {
    let mut response = html_response_from_body(StatusCode::OK, body);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response
}

/// The same response, marked as belonging to this one request.
///
/// For an answer that is correct only for the request that asked: a page
/// rendered after a form post ran a server function, say. It overrides whatever
/// caching the route's strategy would otherwise have declared, because the
/// strategy describes the route and this describes one response to it.
pub(crate) fn uncacheable(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response
}

fn html_response_from_body(status: StatusCode, body: Body) -> Response {
    let mut response = (status, Html(body)).into_response();
    if status.is_client_error() || status.is_server_error() {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, max-age=0"),
        );
    }
    apply_security_headers(&mut response);
    response
}

pub(crate) fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_string(value) {
        Ok(body) => {
            let mut response = (status, body).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            );
            apply_security_headers(&mut response);
            response
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize JSON response: {error}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_response() -> Response {
        StatusCode::OK.into_response()
    }

    /// Whatever `apply` adds, `finalize(.., false)` has to be able to take back
    /// off. Two hand-maintained lists could not promise that; one list does.
    #[test]
    fn disabling_security_removes_exactly_what_enabling_adds() {
        let mut response = plain_response();
        apply_security_headers(&mut response);
        assert_eq!(
            response.headers().len(),
            DEFAULT_SECURITY_HEADERS.len(),
            "every default must be applied"
        );

        let stripped = finalize_security_headers(response, false);
        assert!(
            stripped.headers().is_empty(),
            "disabling security must leave no default behind: {:?}",
            stripped.headers()
        );
    }

    /// Defaults, not overrides: a route that set a header keeps its value, and
    /// disabling security must not delete it either.
    #[test]
    fn an_application_header_survives_both_directions() {
        let mut response = plain_response();
        response.headers_mut().insert(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("SAMEORIGIN"),
        );

        apply_security_headers(&mut response);
        assert_eq!(response.headers()["x-frame-options"], "SAMEORIGIN");

        let stripped = finalize_security_headers(response, false);
        assert_eq!(
            stripped.headers()["x-frame-options"],
            "SAMEORIGIN",
            "a deliberate application header is not a Ruvyxa default to strip"
        );
    }

    /// Held to the same list the JavaScript runtimes serve from
    /// `@ruvyxa/core/utils`, which no Rust code can import.
    #[test]
    fn matches_the_shared_cross_language_security_header_list() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/security-headers-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("conformance fixture is valid JSON");

        let declared = fixture["headers"]
            .as_object()
            .expect("fixture declares headers");
        let actual: std::collections::BTreeMap<String, String> = DEFAULT_SECURITY_HEADERS
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        let expected: std::collections::BTreeMap<String, String> = declared
            .iter()
            .map(|(name, value)| {
                (
                    name.to_ascii_lowercase(),
                    value
                        .as_str()
                        .expect("header value is a string")
                        .to_string(),
                )
            })
            .collect();

        assert_eq!(
            actual, expected,
            "the fixture decides the list; JavaScript replays the same file"
        );
    }

    fn accepting(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    fn page() -> crate::render_cache::CachedDocument {
        crate::render_cache::CachedDocument::uncached(Arc::from(
            format!("<html><body>{}</body></html>", "content ".repeat(200)).as_str(),
        ))
    }

    /// The point of the whole arrangement: a second hit must reuse the encoded
    /// bytes the first hit produced rather than compress the same page again.
    #[test]
    fn a_second_hit_reuses_the_stored_encoding_instead_of_re_encoding() {
        let document = page();
        let headers = accepting("br");

        let first = cached_html_response(StatusCode::OK, &document, Some(&headers));
        assert_eq!(first.headers()[header::CONTENT_ENCODING], "br");

        // `compressed` only ever calls its encoder when the slot is empty, so an
        // encoder that panics proves the second hit never reached one.
        let reused = document.compressed(
            |encoding| encoding == "br",
            |_| panic!("a stored encoding must not be rebuilt"),
        );
        assert!(reused.is_some());
    }

    /// A cached response must declare what it varies on, or a shared cache
    /// replays the encoded copy to a client that cannot read it.
    #[test]
    fn every_cached_document_response_varies_on_accept_encoding() {
        let document = page();
        for accept in ["br", "gzip", "identity", ""] {
            let response =
                cached_html_response(StatusCode::OK, &document, Some(&accepting(accept)));
            assert_eq!(
                response.headers()[header::VARY],
                "accept-encoding",
                "Accept-Encoding: {accept:?}"
            );
        }
    }

    /// A client that takes no encoding we store gets the plain document, and the
    /// outer compression layer is left to decide as it always did.
    #[test]
    fn a_client_that_accepts_nothing_gets_an_unencoded_body() {
        let document = page();
        let response =
            cached_html_response(StatusCode::OK, &document, Some(&accepting("identity")));
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));

        let no_header = cached_html_response(StatusCode::OK, &document, None);
        assert!(!no_header.headers().contains_key(header::CONTENT_ENCODING));
    }

    /// `br;q=0` is a refusal, not a request.
    #[test]
    fn a_zero_quality_encoding_is_refused() {
        assert!(!accept_encoding_allows("br;q=0", "br"));
        assert!(!accept_encoding_allows("gzip, br;q=0", "br"));
        assert!(accept_encoding_allows("gzip, br", "br"));
        assert!(accept_encoding_allows("br;q=1.0", "br"));
        assert!(!accept_encoding_allows("gzip", "br"));
    }

    /// Compressing a tiny document usually makes it larger; leave those alone.
    #[test]
    fn a_document_below_the_threshold_is_not_encoded() {
        let tiny = crate::render_cache::CachedDocument::uncached(Arc::from("<p>hi</p>"));
        let response = cached_html_response(StatusCode::OK, &tiny, Some(&accepting("br")));
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    }

    /// The stored bytes must actually be the document — a wrong encoder here
    /// would serve every cached page as garbage.
    #[test]
    fn the_stored_encoding_round_trips_to_the_original_document() {
        use std::io::Read;
        let document = page();
        let encoded = document
            .compressed(|_| true, |html| encode_document(html, "br"))
            .expect("brotli is acceptable");
        assert_eq!(encoded.encoding, "br");

        let mut decoded = String::new();
        brotli::Decompressor::new(&encoded.bytes[..], 4096)
            .read_to_string(&mut decoded)
            .expect("stored bytes must be valid brotli");
        assert_eq!(decoded, &*document.html);
        assert!(
            encoded.bytes.len() < document.html.len(),
            "an encoding that grows the page is not worth storing"
        );
    }

    #[test]
    fn gzip_is_used_when_brotli_is_not_accepted() {
        use std::io::Read;
        let document = page();
        let encoded = document
            .compressed(|_| true, |html| encode_document(html, "gzip, deflate"))
            .expect("gzip is acceptable");
        assert_eq!(encoded.encoding, "gzip");

        let mut decoded = String::new();
        flate2::read::GzDecoder::new(&encoded.bytes[..])
            .read_to_string(&mut decoded)
            .expect("stored bytes must be valid gzip");
        assert_eq!(decoded, &*document.html);
    }

    /// An error page must not be cached, or a transient 500 sticks in a shared
    /// cache long after the deploy that caused it.
    #[test]
    fn error_documents_are_not_cacheable() {
        let response = html_response(StatusCode::INTERNAL_SERVER_ERROR, "boom".into());
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-store, max-age=0"
        );

        let ok = html_response(StatusCode::OK, "fine".into());
        assert!(!ok.headers().contains_key(header::CACHE_CONTROL));
    }
}
