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
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::warn;

use crate::apply_security_headers;

/// How long a hashed client bundle may be reused without asking again.
///
/// Named rather than written out at each site because the 200 and the 304 for
/// the same file both send it, and the 304 sent nothing at all while four
/// copies of the literal sat in this file — the drift a shared name removes.
/// `IMMUTABLE_CACHE_CONTROL` in `packages/@ruvyxa/core/src/utils.ts` is the
/// deployed half of the same fact.
pub(crate) const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// How long an unhashed `public/` asset may be reused without asking again.
///
/// The deployed half is `PUBLIC_ASSET_CACHE_CONTROL` in
/// `packages/@ruvyxa/core/src/utils.ts`.
pub(crate) const PUBLIC_ASSET_CACHE_CONTROL: &str = "public, max-age=3600, must-revalidate";

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
///
/// Shared with `dynamic_image`, which keys its on-demand transform cache on the
/// same `(len, mtime)` under the same settle rule rather than re-deriving one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssetIdentity {
    pub(crate) len: u64,
    pub(crate) modified: SystemTime,
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
pub(crate) fn asset_identity(metadata: &std::fs::Metadata) -> Option<AssetIdentity> {
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
pub(crate) fn is_settled(identity: &AssetIdentity, now: SystemTime) -> bool {
    now.duration_since(identity.modified)
        .is_ok_and(|age| age >= ASSET_ETAG_SETTLE)
}

/// Seconds since the Unix epoch, or `None` for a time before it.
fn epoch_seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|since| since.as_secs())
}

/// The civil date and time of an epoch second, as `(year, month, day, weekday)`
/// plus `(hour, minute, second)`.
///
/// Howard Hinnant's `civil_from_days`, which is exact for every day this can be
/// asked about and needs no table. The weekday falls out of the epoch day
/// directly: 1970-01-01 was a Thursday.
fn civil_from_epoch(seconds: u64) -> (i64, u32, u32, usize, u64, u64, u64) {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let weekday = ((days + 4) % 7) as usize;

    // Shift the era so the leap day lands at the end of the cycle.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // March-based month, so February's variable length is last rather than
    // in the middle where every branch would have to know about it.
    let march_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * march_month + 2) / 5 + 1) as u32;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    (
        year,
        month,
        day,
        weekday,
        time / 3_600,
        (time / 60) % 60,
        time % 60,
    )
}

/// Days since the Unix epoch for a civil date. The inverse of the above.
fn epoch_days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let march_month = if month > 2 { month - 3 } else { month + 9 } as i64;
    let day_of_year = (153 * march_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Weekday and month names, ASCII and fixed by the specification.
///
/// Deliberately not built from any locale-aware formatter: RFC 9110 §5.6.7
/// spells these names out, and a host whose locale renders them differently
/// would emit a header no client can parse. This is the same reason
/// `localeCompare` is banned repository-wide.
const HTTP_WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const HTTP_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// An `IMF-fixdate`, the one format RFC 9110 §5.6.7 requires a sender to use.
fn http_date(time: SystemTime) -> Option<String> {
    let seconds = epoch_seconds(time)?;
    let (year, month, day, weekday, hour, minute, second) = civil_from_epoch(seconds);
    let weekday = HTTP_WEEKDAYS[weekday];
    let month = HTTP_MONTHS[(month - 1) as usize];
    Some(format!(
        "{weekday}, {day:02} {month} {year:04} {hour:02}:{minute:02}:{second:02} GMT"
    ))
}

/// Parse an `IMF-fixdate` back to epoch seconds.
///
/// Only that format. The two obsolete forms RFC 9110 asks recipients to accept
/// are not produced by anything that has sent this server a header in practice,
/// and refusing one is safe in the direction that matters: an unparsed date
/// means the validator does not match, so the file is sent in full. Accepting
/// one wrongly would answer 304 to a client that does not hold the file.
fn parse_http_date(text: &str) -> Option<u64> {
    let text = text.trim();
    // `Sun, 06 Nov 1994 08:49:37 GMT` -- fixed width throughout.
    let bytes = text.as_bytes();
    if bytes.len() != 29 || !text.ends_with(" GMT") {
        return None;
    }
    if !HTTP_WEEKDAYS.contains(&&text[..3]) || bytes[3] != b',' || bytes[4] != b' ' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u64> {
        let digits = text.get(range)?;
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse::<u64>().ok())
            .flatten()
    };
    let day = number(5..7)?;
    let month = HTTP_MONTHS.iter().position(|name| *name == &text[8..11])? as u32 + 1;
    let year = number(12..16)? as i64;
    let hour = number(17..19)?;
    let minute = number(20..22)?;
    let second = number(23..25)?;
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = epoch_days_from_civil(year, month, day as u32);
    if days < 0 {
        return None;
    }
    Some(days as u64 * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Whether `If-Modified-Since` already covers this file's modification time.
///
/// Compared in whole seconds, because that is the resolution the header
/// carries: measuring a sub-second mtime against a second-resolution date makes
/// every file look newer than the copy the client was just given.
fn not_modified_since(header: Option<&HeaderMap>, modified: SystemTime) -> bool {
    let Some(value) = header
        .and_then(|headers| headers.get(header::IF_MODIFIED_SINCE))
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let (Some(since), Some(modified)) = (parse_http_date(value), epoch_seconds(modified)) else {
        return false;
    };
    modified <= since
}

/// Whether the client already holds this version, by either validator.
///
/// RFC 9110 §13.1.3: a recipient that understands `If-None-Match` must ignore
/// `If-Modified-Since` when both are present. That ordering is not cosmetic
/// here -- an entity tag is exact where a second-resolution date is not, so a
/// stale `If-None-Match` beside a date that happens to cover the mtime has to
/// answer 200, not 304. Mirrors `staticResponsePlan` in `standalone-server.ts`,
/// which is the other host serving the same `public/` directory.
fn request_is_fresh(
    request_headers: Option<&HeaderMap>,
    etag: &str,
    modified: Option<SystemTime>,
) -> bool {
    let none_match = request_headers
        .and_then(|headers| headers.get(header::IF_NONE_MATCH))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if none_match.is_some() {
        return request_matches_etag(request_headers, etag);
    }
    modified.is_some_and(|modified| not_modified_since(request_headers, modified))
}

/// True when the request already holds the version identified by `etag`.
pub(crate) fn request_matches_etag(request_headers: Option<&HeaderMap>, etag: &str) -> bool {
    request_headers
        .and_then(|headers| headers.get(header::IF_NONE_MATCH))
        .is_some_and(|value| etag_matches(value, etag))
}

/// Write `Last-Modified` for a file whose modification time is known.
///
/// Absent rather than invented when the platform cannot answer: a wrong date is
/// a validator clients will act on, where a missing one only costs a transfer.
fn insert_last_modified(headers: &mut HeaderMap, modified: Option<SystemTime>) {
    let Some(value) = modified.and_then(http_date) else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(&value) {
        headers.insert(header::LAST_MODIFIED, value);
    }
}

/// A 304 for a resource whose 200 would have carried `etag` and `cache_control`.
///
/// RFC 9110 §15.4.5 asks for both: a cache that revalidates and is told only
/// "unchanged" cannot refresh the freshness of what it stored, and cannot learn
/// the validator to send next time if it never held one. This sent neither,
/// while `dynamic_image_endpoint` one module over already sent both.
fn not_modified_response(
    etag: &str,
    cache_control: &'static str,
    modified: Option<SystemTime>,
) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    // Sent on the 304 as well as the 200. A client that revalidated with a date
    // and was told only "unchanged" cannot refresh what it stored, and one that
    // never held a `Last-Modified` cannot learn the value to send next time.
    insert_last_modified(headers, modified);
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    apply_security_headers(&mut response);
    response
}

/// One byte range, already resolved against the length of the file.
///
/// `end` is inclusive, as it is on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

impl ByteRange {
    fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// What a request's `Range` header asks this server to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeRequest {
    /// No range, or one this server declines to honour. Send the whole file.
    Whole,
    Partial(ByteRange),
    /// The client named a range that does not exist in this file. RFC 9110
    /// requires 416 rather than silently sending something else.
    Unsatisfiable,
}

/// Decide what to serve for a request that may carry `Range`.
///
/// A media element does not download a file and play it; it asks for bytes as
/// it needs them. Without ranges, dragging a video's scrubber restarts the
/// download from zero, and Safari refuses to play a resource whose server does
/// not answer its opening `Range: bytes=0-1` with a 206 at all — so the
/// streaming path added for exactly these files could not play them.
fn requested_range(
    headers: Option<&HeaderMap>,
    len: u64,
    validator: Option<&str>,
    modified: Option<SystemTime>,
) -> RangeRequest {
    let Some(headers) = headers else {
        return RangeRequest::Whole;
    };
    let Some(value) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    else {
        return RangeRequest::Whole;
    };

    // `If-Range` makes the range conditional on the client still holding the
    // version it is continuing. A mismatch is not an error: the whole
    // representation is the correct answer, and it is the only way a client
    // resuming a download notices the file changed underneath it. Both forms
    // are answered now: this server sends `Last-Modified`, so a date-formed
    // `If-Range` is one it gave the client itself. It previously sent none, so
    // every date-formed `If-Range` fell to the whole-file branch and a resumed
    // download restarted from zero.
    if let Some(if_range) = headers
        .get(header::IF_RANGE)
        .and_then(|value| value.to_str().ok())
    {
        // Either form is legal here. An entity tag is compared strongly --
        // RFC 9110 §13.1.5 -- because a partial response assembled from two
        // representations is a corrupt file rather than a stale one. A date is
        // compared for exact equality with the file's own `Last-Modified` for
        // the same reason: "not newer" is not enough, since a file rewritten
        // back to an older mtime is a different representation at the same date.
        let matched = if if_range.starts_with('"') || if_range.starts_with("W/") {
            validator
                .is_some_and(|validator| normalized_etag(if_range) == normalized_etag(validator))
        } else {
            match (parse_http_date(if_range), modified.and_then(epoch_seconds)) {
                (Some(sent), Some(current)) => sent == current,
                _ => false,
            }
        };
        if !matched {
            return RangeRequest::Whole;
        }
    }

    parse_single_byte_range(value, len)
}

/// One byte position from a range specifier: ASCII digits and nothing else.
///
/// Both languages had a permissive number parser and they were permissive
/// about different things: Rust's `u64::from_str` accepts a leading `+`, and
/// JavaScript's `Number()` accepts `1e1` and `0x2`. None of those is a range
/// specifier, so neither side takes any of them — a disagreement the shared
/// fixture now decides rather than each parser's host language.
fn byte_position(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // A position too large to represent is still a position, and it is past the
    // end of any real file — which is exactly what the caller needs to decide.
    // Reporting it as unparsable would send the whole file instead of a 416.
    Some(text.parse::<u64>().unwrap_or(u64::MAX))
}

/// Parse a single-range `bytes=` specifier against a known length.
///
/// Multi-range requests are answered whole. A `multipart/byteranges` body is
/// more machinery than any client of this server needs, and RFC 9110 lets a
/// server ignore a `Range` it does not wish to honour — which is why an
/// unparsable specifier also falls back to the whole file rather than to 416.
/// Only a syntactically valid range that this file cannot satisfy is a 416.
fn parse_single_byte_range(value: &str, len: u64) -> RangeRequest {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeRequest::Whole;
    };
    let spec = spec.trim();
    if spec.contains(',') {
        return RangeRequest::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return RangeRequest::Whole;
    };
    let (first, last) = (first.trim(), last.trim());

    if first.is_empty() {
        // `bytes=-N`: the final N bytes, clamped to the file.
        let Some(suffix) = byte_position(last) else {
            return RangeRequest::Whole;
        };
        // An empty file has no byte to name, and a zero-length suffix names
        // none either.
        if suffix == 0 || len == 0 {
            return RangeRequest::Unsatisfiable;
        }
        return RangeRequest::Partial(ByteRange {
            start: len.saturating_sub(suffix),
            end: len - 1,
        });
    }

    let Some(start) = byte_position(first) else {
        return RangeRequest::Whole;
    };
    if start >= len {
        return RangeRequest::Unsatisfiable;
    }
    let end = if last.is_empty() {
        len - 1
    } else {
        match byte_position(last) {
            // A last-byte position past the end is clamped, not refused: a
            // client that asks for one megabyte from here should get whatever
            // of it exists.
            Some(end) => end.min(len - 1),
            None => return RangeRequest::Whole,
        }
    };
    if end < start {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Partial(ByteRange { start, end })
}

/// Refuse a range this file cannot satisfy, telling the client the real length.
fn range_not_satisfiable_response(len: u64) -> Response {
    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    let headers = response.headers_mut();
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(value) = HeaderValue::from_str(&format!("bytes */{len}")) {
        headers.insert(header::CONTENT_RANGE, value);
    }
    apply_security_headers(&mut response);
    response
}

/// Whether a failed read of an already-resolved asset means "no such asset"
/// rather than a fault.
///
/// Every one of the four serving paths below made two adjacent observations of
/// the same file under two different policies: the existence check treats any
/// failure as a miss and falls through to routing, while the read mapped any
/// failure to `RuvyxaError::Io`, which `handle_request` turns into a 500. So a
/// file replaced between the two -- by a watcher-triggered rebuild, or by an
/// editor that saves atomically -- answered a transient 500, and an error
/// overlay in dev, where a moment earlier it would have answered 404.
///
/// `PermissionDenied` is a miss for the same reason: a file this process may not
/// read is not an asset it can serve, and 404 is both correct and less alarming
/// than 500. It is logged, so the misconfiguration stays visible in the status
/// code's absence. Every other kind is still a fault and still propagates.
fn read_failure_is_a_miss(error: &std::io::Error, file: &Path) -> bool {
    match error.kind() {
        std::io::ErrorKind::NotFound => true,
        std::io::ErrorKind::PermissionDenied => {
            warn!(file = %file.display(), "asset exists but could not be read; answering 404");
            true
        }
        _ => false,
    }
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
    let modified = identity.as_ref().map(|identity| identity.modified);

    // Answer a revalidation from the fingerprint index, before touching the
    // file. This is the whole point of the index: a 304 carries no body, so
    // reading and hashing the file to produce one is pure waste.
    if let Some(identity) = &identity
        && let Some(etag) = cached_asset_etag(&file, identity)
        && request_is_fresh(request_headers, &etag, modified)
    {
        return Ok(Some(not_modified_response(
            &etag,
            PUBLIC_ASSET_CACHE_CONTROL,
            modified,
        )));
    }

    let content_type = content_type_for(&file);

    // Above the threshold the file reaches the socket as a stream. Reading it in
    // full first makes peak memory the sum of every large asset being served at
    // once — one 200 MB video and a handful of clients is enough to end the
    // process — and none of that memory buys anything, because the bytes are
    // written out and dropped immediately.
    if metadata.len() > streamed_asset_threshold() {
        let validator = identity.as_ref().and_then(weak_validator);
        if let Some(etag) = &validator
            && request_is_fresh(request_headers, etag, modified)
        {
            return Ok(Some(not_modified_response(
                etag,
                PUBLIC_ASSET_CACHE_CONTROL,
                modified,
            )));
        }
        let range = match requested_range(
            request_headers,
            metadata.len(),
            validator.as_deref(),
            modified,
        ) {
            RangeRequest::Whole => None,
            RangeRequest::Partial(range) => Some(range),
            RangeRequest::Unsatisfiable => {
                return Ok(Some(range_not_satisfiable_response(metadata.len())));
            }
        };
        return Ok(Some(
            streamed_file_response(&file, &metadata, content_type, validator.as_deref(), range)
                .await?,
        ));
    }

    let bytes = match tokio::fs::read(&file).await {
        Ok(bytes) => bytes,
        Err(source) if read_failure_is_a_miss(&source, &file) => return Ok(None),
        Err(source) => {
            return Err(RuvyxaError::Io {
                message: format!("Failed to read public file {}", file.display()),
                source,
            });
        }
    };

    // Always hash the bytes actually being served rather than trusting the
    // index here. The file could have been rewritten between the metadata read
    // and this one, and an ETag that does not describe the response body would
    // stay wrong in every downstream cache until the file changed again.
    let etag = compute_etag(&bytes);
    if let Some(identity) = &identity {
        store_asset_etag(&file, identity, &etag);
    }

    // The conditional request, by whichever validator it carried.
    if request_is_fresh(request_headers, &etag, modified) {
        return Ok(Some(not_modified_response(
            &etag,
            PUBLIC_ASSET_CACHE_CONTROL,
            modified,
        )));
    }

    // Ranges are honoured here too, not only above the streaming threshold.
    // `Accept-Ranges` is a promise about the resource, and audio, small clips
    // and resumed downloads all sit below 8 MiB — advertising the header on
    // one branch and ignoring the request on the other would be a lie the
    // client cannot detect until playback breaks.
    let full_length = bytes.len() as u64;
    let range = match requested_range(request_headers, full_length, Some(&etag), modified) {
        RangeRequest::Whole => None,
        RangeRequest::Partial(range) => Some(range),
        RangeRequest::Unsatisfiable => {
            return Ok(Some(range_not_satisfiable_response(full_length)));
        }
    };
    let body = match range {
        Some(range) => bytes[range.start as usize..=range.end as usize].to_vec(),
        None => bytes,
    };

    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PUBLIC_ASSET_CACHE_CONTROL),
    );
    // On the 200 and the 206 both: a client that resumes a download compares
    // the two, and one that revalidates by date has to have been given one.
    insert_last_modified(headers, modified);
    if let Some(range) = range {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        if let Ok(value) = HeaderValue::from_str(&format!(
            "bytes {}-{}/{full_length}",
            range.start, range.end
        )) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
    }
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

/// Read once per process, not once per request.
///
/// This is a process-lifetime constant read from the environment, and it was
/// being re-read -- allocating a `String` and parsing it -- on every public
/// asset request. The environment cannot change under a running process in a
/// way this should honour, so the answer is settled at first use, the same way
/// `ASSET_ETAG_CACHE` above is.
static STREAMED_ASSET_THRESHOLD: LazyLock<u64> = LazyLock::new(|| {
    std::env::var(STREAMED_ASSET_THRESHOLD_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_STREAMED_ASSET_THRESHOLD)
});

fn streamed_asset_threshold() -> u64 {
    *STREAMED_ASSET_THRESHOLD
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

/// Send a file, or one range of it, as a stream with an accurate length.
///
/// Seeking and then bounding the reader is what keeps a range cheap: only the
/// requested bytes are ever read, which is the difference between scrubbing to
/// the end of a video and re-reading the whole file to reach it.
async fn streamed_file_response(
    file: &Path,
    metadata: &std::fs::Metadata,
    content_type: &'static str,
    etag: Option<&str>,
    range: Option<ByteRange>,
) -> Result<Response> {
    let mut handle = tokio::fs::File::open(file)
        .await
        .map_err(|source| RuvyxaError::Io {
            message: format!("Failed to open public file {}", file.display()),
            source,
        })?;

    // The length is measured from the *open handle*, not from the `Metadata`
    // captured before the open. Those are two observations of a file that an
    // in-progress `cp`/`rsync` into `public/` can change between, and
    // `Content-Length` has to describe the bytes this body actually streams:
    // advertising more truncates the response and poisons the keep-alive
    // connection, advertising fewer makes it an incomplete message.
    let full_length = match handle.metadata().await {
        Ok(open) => open.len(),
        // The handle is open, so a failure here is unexpected rather than
        // "the file went away". Fall back to what the caller measured.
        Err(_) => metadata.len(),
    };
    let body_length = match range {
        Some(range) => {
            handle
                .seek(std::io::SeekFrom::Start(range.start))
                .await
                .map_err(|source| RuvyxaError::Io {
                    message: format!("Failed to seek public file {}", file.display()),
                    source,
                })?;
            range.len().min(full_length.saturating_sub(range.start))
        }
        None => full_length,
    };
    // Both branches are bounded. The whole-file branch used to hand the reader
    // an unbounded handle, so a file that grew after it was measured streamed
    // more bytes than the header promised.
    let body =
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(handle.take(body_length)));

    let modified = metadata.modified().ok();
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    // Axum cannot infer a length for a stream, and a large download with no
    // length gives the client no progress and forces chunked framing.
    if let Ok(length) = HeaderValue::from_str(&body_length.to_string()) {
        headers.insert(header::CONTENT_LENGTH, length);
    }
    if let Some(etag) = etag
        && let Ok(value) = HeaderValue::from_str(etag)
    {
        headers.insert(header::ETAG, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PUBLIC_ASSET_CACHE_CONTROL),
    );
    // On the 200 and the 206 both: a client that resumes a download compares
    // the two, and one that revalidates by date has to have been given one.
    insert_last_modified(headers, modified);
    if let Some(range) = range {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        if let Ok(value) = HeaderValue::from_str(&format!(
            "bytes {}-{}/{full_length}",
            range.start, range.end
        )) {
            response.headers_mut().insert(header::CONTENT_RANGE, value);
        }
    }
    apply_security_headers(&mut response);
    Ok(response)
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

/// The content type an emitted client asset is served with.
///
/// The client directory held nothing but JavaScript until the build started
/// emitting the project's compiled stylesheet there, and a fixed
/// `text/javascript` on a `.css` file is refused by every browser: the
/// framework sends `X-Content-Type-Options: nosniff`, so a stylesheet served
/// under the wrong type is not applied at all.
fn client_asset_content_type(file: &Path) -> HeaderValue {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("css") => HeaderValue::from_static("text/css; charset=utf-8"),
        Some("map" | "json") => HeaderValue::from_static("application/json; charset=utf-8"),
        _ => HeaderValue::from_static("text/javascript; charset=utf-8"),
    }
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
    let modified = identity.as_ref().map(|identity| identity.modified);

    // Answer a revalidation from the fingerprint index, before touching the
    // file — the same index `serve_public_file` uses, which this path had never
    // consulted. A 304 carries no body, so reading the bundle and hashing every
    // byte of it only to send an empty response is pure waste.
    if let Some(identity) = &identity
        && let Some(etag) = cached_asset_etag(&file, identity)
        && request_is_fresh(request_headers, &etag, modified)
    {
        return Ok(Some(not_modified_response(
            &etag,
            IMMUTABLE_CACHE_CONTROL,
            modified,
        )));
    }

    let bytes = match tokio::fs::read(&file).await {
        Ok(bytes) => bytes,
        Err(source) if read_failure_is_a_miss(&source, &file) => return Ok(None),
        Err(source) => {
            return Err(RuvyxaError::Io {
                message: format!("Failed to read client file {}", file.display()),
                source,
            });
        }
    };

    // Hash the bytes actually being served rather than trusting the index: the
    // file could have been rewritten between the metadata read and this one, and
    // an ETag that does not describe the body stays wrong in every downstream
    // cache until the file changes again.
    let etag = compute_etag(&bytes);
    if let Some(identity) = &identity {
        store_asset_etag(&file, identity, &etag);
    }

    if request_is_fresh(request_headers, &etag, modified) {
        return Ok(Some(not_modified_response(
            &etag,
            IMMUTABLE_CACHE_CONTROL,
            modified,
        )));
    }

    let content_type = client_asset_content_type(&file);
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, content_type);
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(IMMUTABLE_CACHE_CONTROL),
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
///
/// `std::fs::canonicalize` directly rather than `normalized_canonical_path`,
/// whose documented fallback is to return the path *uncanonicalised* when
/// resolution fails. That fallback is right for the diagnostics this workspace
/// uses it for and wrong for a containment check: `Path::starts_with` compares
/// components, so an unresolved `public/../secret.txt` does start with
/// `public`, and a candidate that failed to canonicalise while the root
/// succeeded would have been accepted. Not reachable through today's callers --
/// each rejects `..` and a backslash first, and `.exists()` above already
/// required the path to resolve -- but the guarantee has to be local to this
/// function, because the next caller is what a guard like this is for. The
/// `resolve_client_file` duplication went wrong in exactly that way.
///
/// A path that exists yet cannot be canonicalised is now refused rather than
/// served. Given `.exists()` has already succeeded, the remaining causes are an
/// IO error, a filesystem returning `EIO`, or a path past `MAX_PATH` on a
/// Windows host without long-path support -- and in every one of those a
/// refusal is the answer this function's name promises.
pub(crate) fn contained_public_asset(public_dir: &Path, candidate: &Path) -> Option<PathBuf> {
    if !public_dir.exists() || !candidate.exists() {
        return None;
    }
    let public_root =
        ruvyxa_diagnostics::without_verbatim_prefix(&fs::canonicalize(public_dir).ok()?);
    let candidate = ruvyxa_diagnostics::without_verbatim_prefix(&fs::canonicalize(candidate).ok()?);
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

/// A weak validator over content that is served in more than one encoding.
///
/// Documents leave this host identity-encoded or brotli/gzip-encoded depending
/// on what the client accepts, and those are equivalent representations rather
/// than byte-identical ones — which is exactly what the `W/` marker states. A
/// strong tag here would let a shared cache hand one client's compressed copy to
/// a client that asked for neither encoding.
pub(crate) fn weak_content_etag(bytes: &[u8]) -> String {
    format!("W/{}", compute_etag(bytes))
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

/// The icon a document declares, in the order it is looked for, with the type it
/// is actually served as.
///
/// Both spellings are here because a build publishes only one of them:
/// `image.keepOriginal: false` converts the PNG and keeps the WebP, while the
/// project's own `public/` still holds the PNG that `ruvyxa dev` serves. The URL
/// of either resolves at run time — `resolve_public_asset` answers a PNG request
/// from the WebP — but an existence check does not know that, and asking it
/// about the PNG alone is how the icon link disappeared from production while
/// development kept emitting it.
const DOCUMENT_ICONS: &[(&str, &str)] =
    &[("ruvyxa.png", "image/png"), ("ruvyxa.webp", "image/webp")];

/// The viewport declaration a document gets when it declares none.
///
/// Without it a phone lays the page out at the legacy 980px viewport and scales
/// the result down, so every breakpoint in the application is evaluated against
/// a width no device has. `Meta` has no viewport field, so an author following
/// the documentation has nowhere to put one — the framework supplies the value
/// every comparable framework defaults to, and stands aside for a document that
/// writes its own.
pub const DEFAULT_VIEWPORT_META: &str =
    r#"<meta name="viewport" content="width=device-width, initial-scale=1">"#;

/// Head tags the framework contributes, minus anything the document already has.
///
/// Two of them are defaults rather than requirements: the viewport declaration
/// above, and the icon link derived from `public/`. An application that declares
/// either owns it — a project shipping `public/ruvyxa.png` alongside its own
/// icon used to get both links in the document, and the framework's won.
#[must_use]
/// The lowercased copy is deliberate, and it is the fast answer.
///
/// This runs with the whole document in hand on every buffered SSR response, so
/// the obvious objection is the copy: one allocation the size of the page, then
/// eight searches over it. Scanning the original in place with
/// `contains_ascii_case` removes that allocation and is **13-22x slower** on a
/// 200 KB page (1.05 ms against 55 us, measured on this crate) — `str::contains`
/// searches a single-cased haystack with a vectorised two-way searcher, while a
/// case-folding scan compares byte by byte and cannot use one. The copy plus
/// eight vectorised passes wins, and by an order of magnitude. Do not "optimise"
/// this back into `contains_ascii_case` without measuring it.
///
/// The needles are constants for the same reason: composing six of them with
/// `format!` allocated six `String`s per rendered page to search for six values
/// that never change.
pub fn document_head_defaults(document: &str, asset_links: &str) -> String {
    let lower = document.to_ascii_lowercase();
    let mut head = String::with_capacity(asset_links.len() + DEFAULT_VIEWPORT_META.len());
    if !lower.contains("name=\"viewport\"") && !lower.contains("name='viewport'") {
        head.push_str(DEFAULT_VIEWPORT_META);
    }
    // `asset_links` is the icon link and nothing else today; a document that
    // declares any icon keeps its own set whole rather than being merged with.
    if !declares_own_icon(&lower) {
        head.push_str(asset_links);
    }
    head
}

/// Every `rel` spelling that stands the framework's icon link down, in both
/// quote styles, already lowercased for the haystack above.
const OWN_ICON_DECLARATIONS: &[&str] = &[
    "rel=\"icon\"",
    "rel='icon'",
    "rel=\"shortcut icon\"",
    "rel='shortcut icon'",
    "rel=\"apple-touch-icon\"",
    "rel='apple-touch-icon'",
];

/// Whether a document already says what its icon is.
///
/// Both quote styles for each spelling. They were not symmetric before:
/// `rel='icon'` was recognised and `rel='shortcut icon'` was not, so a document
/// using single quotes for the longer spelling was given a second icon link
/// and the browser answered the framework's. The deployed writer in
/// `entry-templates.mjs` applies the same rule, and
/// `tests/fixtures/document-head-conformance.json` is what holds the two equal.
fn declares_own_icon(lowercased: &str) -> bool {
    OWN_ICON_DECLARATIONS
        .iter()
        .any(|declaration| lowercased.contains(declaration))
}

/// The head fragment that gives a document its project stylesheet.
///
/// One writer for all three hosts. A build emits the compiled CSS as a
/// content-addressed asset and every document links it; `ruvyxa dev` has no such
/// asset — HMR replaces the rule text in place — and inlines the collection
/// instead. Before this existed only the inline form was written, so a route
/// rendered at request time on a deployed build had no stylesheet at all: the
/// function that rendered it has no `app/` to compile from.
#[must_use]
pub fn style_head_tag(asset_url: Option<&str>, css: &str) -> String {
    match asset_url {
        Some(url) => format!(
            r#"<link rel="stylesheet" href="{}">"#,
            escape_attribute(url)
        ),
        None if css.is_empty() => String::new(),
        None => format!(r#"<style data-ruvyxa-css>{css}</style>"#),
    }
}

/// Minimal attribute escaping for a URL this crate emitted itself.
fn escape_attribute(value: &str) -> String {
    // One implementation, not two that happen to agree. This was a second copy
    // of `escape_html`'s rule with the same four replacements, so the character
    // added to that one would not have reached the attribute values written
    // here.
    crate::html_document::escape_html(value)
}

/// `<link>` tags for the files a project publishes that a document should
/// declare.
///
/// Public because a pre-rendered page has to end up with the same head this
/// server composes for a live render: `ruvyxa build` bakes the document and
/// `ruvyxa start` serves it from disk without ever running a renderer, so
/// nothing downstream is left to add these. When the icon link was missing from
/// baked pages, every browser fell back to requesting `/favicon.ico` and every
/// production page load logged a 404 that `ruvyxa dev` never showed.
///
/// What a document ends up with is decided by [`document_head_defaults`]: these
/// are defaults, and an application that declares its own icon keeps it.
pub fn public_asset_links(public_dir: &Path) -> String {
    let mut links = Vec::new();

    if let Some((file, content_type)) = DOCUMENT_ICONS
        .iter()
        .find(|(file, _)| public_dir.join(file).is_file())
    {
        links.push(format!(
            r#"<link rel="icon" type="{content_type}" href="/{file}">"#
        ));
    }

    links.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The framework's head defaults are defaults, and the document wins.
    ///
    /// Both of these shipped wrong in opposite directions: no document had a
    /// viewport declaration at all, so every breakpoint in every application was
    /// evaluated against a phone's legacy 980px layout width; and the icon link
    /// derived from `public/` was added even to a document that declared its
    /// own, so a project that shipped `ruvyxa.png` alongside its own mark got
    /// both links and the framework's answered.
    #[test]
    fn head_defaults_stand_down_for_a_document_that_declares_its_own() {
        let icon = r#"<link rel="icon" type="image/png" href="/ruvyxa.png">"#;

        let bare = document_head_defaults("<html><head><title>x</title></head></html>", icon);
        assert!(bare.contains("name=\"viewport\""), "{bare}");
        assert!(bare.contains("/ruvyxa.png"), "{bare}");

        let with_viewport = document_head_defaults(
            r#"<html><head><meta name="viewport" content="width=device-width"></head></html>"#,
            icon,
        );
        assert!(
            !with_viewport.contains("initial-scale=1"),
            "{with_viewport}"
        );
        assert!(with_viewport.contains("/ruvyxa.png"), "{with_viewport}");

        let with_icon = document_head_defaults(
            r#"<html><head><link rel="icon" href="/mark.svg"></head></html>"#,
            icon,
        );
        assert!(!with_icon.contains("/ruvyxa.png"), "{with_icon}");
        assert!(with_icon.contains("name=\"viewport\""), "{with_icon}");

        // Single quotes and a differing case are the same declaration.
        let quoted = document_head_defaults(
            "<html><HEAD><META NAME='viewport' CONTENT='width=device-width'></HEAD></html>",
            icon,
        );
        assert!(!quoted.contains("initial-scale=1"), "{quoted}");
    }

    /// The stand-down rule folds ASCII and nothing else.
    ///
    /// The six icon needles moved from `format!` to constants, and an audit
    /// proposed going further and scanning the document in place with
    /// `contains_ascii_case` — which measured 13-22x slower and was not taken
    /// (see [`document_head_defaults`]). Both forms are the same question only
    /// while the folding is byte-for-byte ASCII, so this states the equivalence
    /// against the shape that would break it: a document carrying non-ASCII
    /// letters whose Unicode lowercase is a *different* byte length (`İ`, `K`,
    /// `ẞ`) right beside the attributes being matched. A Unicode-aware fold
    /// would move those bytes and could match — or stop matching — where the
    /// ASCII fold does not. It also pins the in-place scanner as an answer-alike
    /// so the trade stays a performance decision rather than a behavioural one.
    #[test]
    fn the_stand_down_rule_folds_ascii_and_leaves_other_scripts_alone() {
        /// The rule spelled out per needle, `format!` and all, as the oracle.
        fn reference(document: &str, asset_links: &str) -> String {
            let lower = document.to_ascii_lowercase();
            let mut head = String::new();
            if !lower.contains("name=\"viewport\"") && !lower.contains("name='viewport'") {
                head.push_str(DEFAULT_VIEWPORT_META);
            }
            let declares_icon = ["icon", "shortcut icon", "apple-touch-icon"]
                .iter()
                .any(|rel| {
                    lower.contains(&format!("rel=\"{rel}\""))
                        || lower.contains(&format!("rel='{rel}'"))
                });
            if !declares_icon {
                head.push_str(asset_links);
            }
            head
        }

        let icon = r#"<link rel="icon" type="image/png" href="/ruvyxa.png">"#;
        let documents = [
            "<html><head><title>x</title></head></html>",
            r#"<html><head><meta name="viewport" content="width=1024"></head></html>"#,
            "<html><head><meta name='viewport' content='width=1024'></head></html>",
            r#"<html><head><META NAME="VIEWPORT" CONTENT="width=1024"></head></html>"#,
            "<html><head><META NAME='VIEWPORT'></head></html>",
            r#"<html><head><LINK REL="ICON" href="/mine.svg"></head></html>"#,
            "<html><head><LINK REL='ICON' href='/mine.svg'></head></html>",
            r#"<html><head><link rel="SHORTCUT ICON" href="/mine.ico"></head></html>"#,
            "<html><head><link rel='Apple-Touch-Icon' href='/t.png'></head></html>",
            r#"<html><head><link rel="stylesheet" href="/a.css"></head></html>"#,
            // Non-ASCII beside the attribute, and inside its value.
            "<html><head><title>İstanbul Kelvin K straße</title><LINK REL='ICON' href='/ı.svg'></head></html>",
            "<html><head><meta name='VIEWPORT' content='ẞ'><p>ǄǅǆΣσς</p></head></html>",
            // A needle-shaped run that differs only outside ASCII.
            "<html><head><link rel=\"ıcon\" href=\"/no.svg\"></head></html>",
            "<html><head><meta name=\"vıewport\"></head></html>",
            "<html><head><meta name=\"VİEWPORT\"></head></html>",
        ];

        for document in documents {
            for links in [icon, ""] {
                assert_eq!(
                    document_head_defaults(document, links),
                    reference(document, links),
                    "{document}"
                );
            }
        }
    }

    /// A stylesheet is linked when the build emitted one and inlined otherwise.
    #[test]
    fn the_style_tag_links_a_built_asset_and_inlines_a_collection() {
        assert_eq!(
            style_head_tag(Some("/__ruvyxa/client/styles.abc.css"), ""),
            r#"<link rel="stylesheet" href="/__ruvyxa/client/styles.abc.css">"#
        );
        assert_eq!(
            style_head_tag(None, "body{color:red}"),
            "<style data-ruvyxa-css>body{color:red}</style>"
        );
        assert_eq!(
            style_head_tag(None, ""),
            "",
            "no CSS declares no stylesheet"
        );
    }

    /// The icon link has to survive the build's own image optimization.
    ///
    /// `ruvyxa dev` publishes the project's `public/`, which holds the PNG;
    /// `ruvyxa start` publishes the staged assets, where `image.keepOriginal:
    /// false` left only the WebP. Asking about the PNG alone answered "no icon"
    /// for every production page, and every browser then fell back to
    /// `/favicon.ico` and logged a 404 that development never showed.
    #[test]
    fn the_icon_link_follows_whichever_form_the_build_published() {
        for (file, expected) in [
            (
                "ruvyxa.png",
                r#"<link rel="icon" type="image/png" href="/ruvyxa.png">"#,
            ),
            (
                "ruvyxa.webp",
                r#"<link rel="icon" type="image/webp" href="/ruvyxa.webp">"#,
            ),
        ] {
            let temp = tempfile::tempdir().expect("temp dir");
            assert_eq!(
                public_asset_links(temp.path()),
                "",
                "a directory with no icon declares none"
            );
            std::fs::write(temp.path().join(file), [0u8; 4]).expect("write");
            assert_eq!(public_asset_links(temp.path()), expected);
        }
    }

    /// The policy itself, stated once so the four serving paths can share it.
    ///
    /// A behavioural test would have to delete the file between the metadata
    /// check and the read, which is not schedulable from outside the process on
    /// either platform, so the rule is asserted directly and its call sites are
    /// pinned by the test below. Both halves are needed: the rule alone would
    /// still pass if a path stopped consulting it.
    #[test]
    fn a_vanished_or_unreadable_asset_is_a_miss_and_nothing_else_is() {
        let file = Path::new("public/logo.svg");
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied,
        ] {
            assert!(
                read_failure_is_a_miss(&std::io::Error::from(kind), file),
                "{kind:?} means the asset cannot be served, which is a 404, not a 500",
            );
        }
        for kind in [
            std::io::ErrorKind::InvalidData,
            std::io::ErrorKind::OutOfMemory,
            std::io::ErrorKind::Other,
        ] {
            assert!(
                !read_failure_is_a_miss(&std::io::Error::from(kind), file),
                "{kind:?} is a fault and must keep reaching the caller as one",
            );
        }
    }

    /// Every read of an already-resolved asset goes through the shared policy.
    ///
    /// The defect this closes was one read mapping every failure to a 500 while
    /// the existence check beside it treated every failure as a miss. That split
    /// existed in four serving paths -- public and client, async and sync. Two
    /// of them are gone: the `*_sync` pair were a weaker second copy of the
    /// serving rules and the only thing driving them was the parity command, so
    /// they were deleted rather than kept level. Two remain, and this is what
    /// says so out loud if a third appears.
    #[test]
    fn every_asset_read_consults_the_miss_policy() {
        // Split so this file's own scan does not match the needle it defines.
        let source = include_str!("static_assets.rs");
        let reads = source.matches(concat!("fs::read(&", "file)")).count();
        let guarded = source
            .matches(concat!("read_failure_is", "_a_miss(&source"))
            .count();
        assert_eq!(
            reads, 2,
            "a serving path was added or removed; the miss policy has to cover it too",
        );
        assert_eq!(
            guarded, reads,
            "a read of a resolved asset is answering 500 where the check in front              of it would have answered 404",
        );
    }

    /// The example date RFC 9110 §5.6.7 itself uses, plus the boundaries a
    /// hand-written civil-date conversion gets wrong: a leap day, the century
    /// that is not a leap year, the century that is, and both ends of a day.
    #[test]
    fn http_dates_round_trip_through_both_directions() {
        for (seconds, text) in [
            (0_u64, "Thu, 01 Jan 1970 00:00:00 GMT"),
            (784_111_777, "Sun, 06 Nov 1994 08:49:37 GMT"),
            (951_782_400, "Tue, 29 Feb 2000 00:00:00 GMT"),
            (4_107_456_000, "Sun, 28 Feb 2100 00:00:00 GMT"),
            (1_709_164_800, "Thu, 29 Feb 2024 00:00:00 GMT"),
            (1_735_689_599, "Tue, 31 Dec 2024 23:59:59 GMT"),
        ] {
            let time = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
            assert_eq!(http_date(time).as_deref(), Some(text));
            assert_eq!(parse_http_date(text), Some(seconds), "parsing {text}");
        }
    }

    /// A date this cannot read must not be read as one that matches: the whole
    /// file is the safe answer, a 304 is not.
    #[test]
    fn an_unreadable_date_is_not_a_match() {
        for text in [
            "",
            "not a date",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Xxx, 06 Nov 1994 08:49:37 GMT",
            "Sun, 06 Xxx 1994 08:49:37 GMT",
            "Sun, 6 Nov 1994 08:49:37 GMT",
        ] {
            assert_eq!(parse_http_date(text), None, "{text:?} must not parse");
        }
    }

    /// RFC 9110 §13.1.3. The date covers the file, so a server that read it
    /// alone would answer 304 -- but the entity tag says the client holds a
    /// different version, and the entity tag is the exact one.
    #[test]
    fn a_stale_if_none_match_beats_a_matching_if_modified_since() {
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);
        let covering = http_date(modified + Duration::from_secs(60)).expect("a date");

        let mut stale = HeaderMap::new();
        stale.insert(
            header::IF_NONE_MATCH,
            "\"0000000000000000\"".parse().unwrap(),
        );
        stale.insert(header::IF_MODIFIED_SINCE, covering.parse().unwrap());
        assert!(
            !request_is_fresh(Some(&stale), "\"abc\"", Some(modified)),
            "an entity tag that does not match must not be overruled by a date",
        );

        let mut matching = HeaderMap::new();
        matching.insert(header::IF_NONE_MATCH, "\"abc\"".parse().unwrap());
        assert!(request_is_fresh(Some(&matching), "\"abc\"", Some(modified)));

        let mut date_only = HeaderMap::new();
        date_only.insert(header::IF_MODIFIED_SINCE, covering.parse().unwrap());
        assert!(
            request_is_fresh(Some(&date_only), "\"abc\"", Some(modified)),
            "with no entity tag the date is what there is to go on",
        );

        let mut older = HeaderMap::new();
        let before = http_date(modified - Duration::from_secs(60)).expect("a date");
        older.insert(header::IF_MODIFIED_SINCE, before.parse().unwrap());
        assert!(!request_is_fresh(Some(&older), "\"abc\"", Some(modified)));
    }

    /// A directory is not a file, and a `public/ruvyxa.png/` would otherwise be
    /// declared as an icon that cannot load.
    #[test]
    fn a_directory_named_like_the_icon_is_not_one() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(temp.path().join("ruvyxa.png")).expect("mkdir");
        assert_eq!(public_asset_links(temp.path()), "");
    }

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

    /// `Content-Length` and the streamed bytes must come from one observation
    /// of one descriptor.
    ///
    /// The metadata that sizes the response is read before the file is opened.
    /// An in-progress `cp`/`rsync` into `public/` grows the file in between, and
    /// an unbounded reader then writes more bytes than the header promised —
    /// which poisons the whole keep-alive connection rather than failing one
    /// request.
    #[tokio::test]
    async fn a_streamed_asset_never_writes_more_bytes_than_it_declared() {
        let temp = tempfile::tempdir().expect("temp dir");
        let file = temp.path().join("archive.zip");
        std::fs::write(&file, vec![b'x'; 4096]).expect("write");
        let stale = std::fs::metadata(&file).expect("metadata");

        // The copy finishes between the metadata read and the open.
        std::fs::write(&file, vec![b'y'; 4096 + 8192]).expect("rewrite");

        let response = streamed_file_response(&file, &stale, "application/zip", None, None)
            .await
            .expect("streaming must succeed");
        let declared: usize = response.headers()[header::CONTENT_LENGTH]
            .to_str()
            .expect("a streamed download still needs its length")
            .parse()
            .expect("the length must be a number");
        let delivered = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a streamed body must read back");

        assert_eq!(
            delivered.len(),
            declared,
            "a body longer than Content-Length makes hyper truncate and error the connection"
        );
    }

    /// The shrink direction of the same invariant: a truncated file must not
    /// leave an incomplete message behind a longer declared length.
    #[tokio::test]
    async fn a_streamed_asset_declares_the_length_a_shrunk_file_can_deliver() {
        let temp = tempfile::tempdir().expect("temp dir");
        let file = temp.path().join("archive.zip");
        std::fs::write(&file, vec![b'x'; 16384]).expect("write");
        let stale = std::fs::metadata(&file).expect("metadata");

        std::fs::write(&file, vec![b'y'; 1024]).expect("truncate");

        let response = streamed_file_response(&file, &stale, "application/zip", None, None)
            .await
            .expect("streaming must succeed");
        let declared: usize = response.headers()[header::CONTENT_LENGTH]
            .to_str()
            .expect("a streamed download still needs its length")
            .parse()
            .expect("the length must be a number");
        let delivered = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a streamed body must read back");

        assert_eq!(
            delivered.len(),
            declared,
            "a body shorter than Content-Length is an incomplete message"
        );
        assert_eq!(delivered.len(), 1024);
    }

    /// The same invariant end to end: a file appended to after the response was
    /// built, but before its body was drained, still delivers exactly the length
    /// the headers promised.
    #[tokio::test]
    async fn a_streamed_asset_that_grows_mid_flight_still_matches_its_header() {
        let temp = tempfile::tempdir().expect("temp dir");
        let file = temp.path().join("movie.webm");
        let big = vec![b'x'; (DEFAULT_STREAMED_ASSET_THRESHOLD + 1) as usize];
        std::fs::write(&file, &big).expect("write");

        let response = serve_public_file(temp.path(), "/movie.webm", None)
            .await
            .expect("serving must succeed")
            .expect("the file exists");
        let declared: usize = response.headers()[header::CONTENT_LENGTH]
            .to_str()
            .expect("a streamed download still needs its length")
            .parse()
            .expect("the length must be a number");

        {
            use std::io::Write as _;
            let mut appender = std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .expect("append handle");
            appender.write_all(&[b'z'; 8192]).expect("append");
            appender.flush().expect("flush");
        }

        let delivered = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a streamed body must read back");
        assert_eq!(delivered.len(), declared);
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

    /// Replay the shared cross-language document-head table.
    ///
    /// A deployed function bundle composes its own head, in JavaScript, because
    /// by then this crate is not running: `entry-templates.mjs` writes the
    /// document for every route rendered at request time. That writer had the
    /// viewport half of this function and not the icon half, so a deployed
    /// build's pre-rendered pages carried an icon link and its request-time
    /// renders did not — visible only in production, and only as a
    /// `/favicon.ico` 404 in a log nobody reads.
    ///
    /// Containment is decided by resolved paths, never by component text.
    ///
    /// The escaping candidate here passed before this change too, but for the
    /// wrong reason: it was refused by the callers' own `..` check, while this
    /// function -- the one whose name promises containment -- would have
    /// accepted it had canonicalisation failed, because `Path::starts_with`
    /// compares components and `public/../secret.txt` does start with `public`.
    #[test]
    fn a_path_that_escapes_the_public_directory_is_refused_by_the_guard_itself() {
        let temp = tempfile::tempdir().expect("temp dir");
        let public = temp.path().join("public");
        std::fs::create_dir(&public).expect("mkdir");
        std::fs::write(temp.path().join("secret.txt"), b"private").expect("write");
        std::fs::write(public.join("logo.svg"), b"<svg/>").expect("write");

        // Exists, and is reachable by a component walk that starts with `public`.
        let escaping = public.join("..").join("secret.txt");
        assert!(
            escaping.exists(),
            "the test is meaningless if the file is absent"
        );
        assert_eq!(
            contained_public_asset(&public, &escaping),
            None,
            "a path that resolves outside the public directory is not a public asset",
        );

        // A file genuinely inside it still resolves, or the guard would be
        // refusing everything and this test would pass for a new wrong reason.
        assert!(contained_public_asset(&public, &public.join("logo.svg")).is_some());
    }

    /// The escaping half of the same fixture.
    ///
    /// `escape_html` calls itself the one place this rule lives, and it is --
    /// on this host. The deployed hosts write it again twice, so the fixture is
    /// what makes that claim true across all three rather than within one.
    #[test]
    fn escapes_the_shared_cross_language_character_set() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/document-head-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("conformance fixture is valid JSON");

        let cases = fixture["escaping"]["cases"]
            .as_array()
            .expect("the fixture carries an escaping table");
        assert!(!cases.is_empty(), "an empty table would assert nothing");
        for case in cases {
            let name = case["name"].as_str().expect("each case is named");
            let input = case["input"].as_str().expect("each case has an input");
            let expect = case["expect"].as_str().expect("each case has an answer");
            assert_eq!(
                crate::html_document::escape_html(input),
                expect,
                "escaping case: {name}",
            );
            // The private attribute escaper is the same function now, and this
            // is what says so: it was a second copy with the same four
            // replacements, which is how it would have missed the fifth.
            assert_eq!(escape_attribute(input), expect, "attribute case: {name}");
        }
    }

    /// `tests/packages/ruvyxa/document-head-parity.test.mjs` drives the same
    /// file through the generated JavaScript writer.
    #[test]
    fn composes_the_shared_cross_language_document_head_defaults() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/document-head-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("conformance fixture is valid JSON");

        assert_eq!(
            fixture["viewportMeta"].as_str(),
            Some(DEFAULT_VIEWPORT_META),
            "the fixture and the constant must be the same declaration"
        );

        let cases = fixture["cases"].as_array().expect("fixture declares cases");
        assert!(!cases.is_empty(), "an empty table gates nothing");
        for case in cases {
            let name = case["name"].as_str().unwrap_or("<unnamed>");
            let document = case["document"].as_str().expect("case declares a document");
            let asset_links = case["assetLinks"]
                .as_str()
                .expect("case declares assetLinks");
            let expected = case["expect"].as_str().expect("case declares expect");
            assert_eq!(
                document_head_defaults(document, asset_links),
                expected,
                "{name}"
            );
        }
    }

    /// Replay the shared cross-language static-asset table.
    ///
    /// The JavaScript servers read `STATIC_CONTENT_TYPES` from
    /// `@ruvyxa/core/utils`; this handler cannot, so the two are held together
    /// by `tests/fixtures/static-asset-conformance.json` instead.
    /// `tests/packages/core/static-asset-contract.test.ts` drives the same file
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

    /// A 304 restates the headers the 200 would have carried.
    ///
    /// RFC 9110 §15.4.5 requires `ETag` and `Cache-Control` on a 304 whenever a
    /// 200 to the same request would have sent them, and this host sent neither
    /// — while `dynamic_image_endpoint` in `framework_endpoints.rs`, one crate
    /// over, already sent both, and the standalone server emitted by
    /// `@ruvyxa/core/standalone-server` does too. Three answers to one question
    /// inside one framework.
    ///
    /// Both fields are read off the 200 rather than written out here, because a
    /// literal repeated in the test is a fourth copy that can drift with the
    /// other three.
    #[tokio::test]
    async fn a_304_carries_the_validator_and_lifetime_its_200_would_have() {
        let temp = tempfile::tempdir().unwrap();
        let public_dir = temp.path();
        fs::write(public_dir.join("logo.png"), vec![7u8; 4096]).unwrap();
        // Past the threshold, so the weak-validator branch is exercised too:
        // that path answers from a different validator than the buffered one.
        let streamed = vec![b'x'; (DEFAULT_STREAMED_ASSET_THRESHOLD + 1) as usize];
        fs::write(public_dir.join("movie.webm"), &streamed).unwrap();

        for request_path in ["/logo.png", "/movie.webm"] {
            let full = serve_public_file(public_dir, request_path, None)
                .await
                .unwrap()
                .unwrap();
            let etag = full.headers()[header::ETAG].clone();
            let cache_control = full.headers()[header::CACHE_CONTROL].clone();

            let mut headers = HeaderMap::new();
            headers.insert(header::IF_NONE_MATCH, etag.clone());
            let revalidated = serve_public_file(public_dir, request_path, Some(&headers))
                .await
                .unwrap()
                .unwrap();

            assert_eq!(
                revalidated.status(),
                StatusCode::NOT_MODIFIED,
                "{request_path}"
            );
            assert_eq!(
                revalidated.headers().get(header::ETAG),
                Some(&etag),
                "{request_path}: a 304 without the validator leaves a cache unable to \
                 refresh what it stored"
            );
            assert_eq!(
                revalidated.headers().get(header::CACHE_CONTROL),
                Some(&cache_control),
                "{request_path}: and without the lifetime it cannot refresh when to ask again"
            );
        }
    }

    /// The same rule for hashed client bundles, whose lifetime is the other one.
    ///
    /// A bundle is `immutable` for a year, so a 304 that drops the header is the
    /// case where the omission costs the most: the entry a cache keeps is the
    /// one it never has to revalidate again.
    #[tokio::test]
    async fn a_client_bundle_304_carries_its_immutable_lifetime() {
        let temp = tempfile::tempdir().unwrap();
        let client_dir = temp.path();
        fs::write(client_dir.join("app.abc123.js"), b"export default 1\n").unwrap();

        let full = serve_client_file(client_dir, "/__ruvyxa/client/app.abc123.js", None)
            .await
            .unwrap()
            .unwrap();
        let etag = full.headers()[header::ETAG].clone();
        assert_eq!(
            full.headers()[header::CACHE_CONTROL],
            HeaderValue::from_static(IMMUTABLE_CACHE_CONTROL)
        );

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.clone());
        let revalidated =
            serve_client_file(client_dir, "/__ruvyxa/client/app.abc123.js", Some(&headers))
                .await
                .unwrap()
                .unwrap();

        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(revalidated.headers().get(header::ETAG), Some(&etag));
        assert_eq!(
            revalidated.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static(IMMUTABLE_CACHE_CONTROL))
        );
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

    /// Every asset response carries the validator a date-based revalidation
    /// needs, and answers one when it arrives. Before this, `public/` shipped
    /// `must-revalidate` with no `Last-Modified`, so a client that cannot use
    /// entity tags re-downloaded the file on every revalidation.
    #[tokio::test]
    async fn a_public_asset_carries_last_modified_and_answers_a_date_revalidation() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("logo.svg"), b"<svg/>").expect("write");

        let full = serve_with(temp.path(), "/logo.svg", &[]).await;
        assert_eq!(full.status(), StatusCode::OK);
        let last_modified = full.headers()[header::LAST_MODIFIED]
            .to_str()
            .expect("an ASCII date")
            .to_string();
        assert!(
            parse_http_date(&last_modified).is_some(),
            "the date this server sends has to be one it can read back: {last_modified}",
        );

        let revalidated = serve_with(
            temp.path(),
            "/logo.svg",
            &[(header::IF_MODIFIED_SINCE, &last_modified)],
        )
        .await;
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(revalidated.headers()[header::LAST_MODIFIED], last_modified);

        // A date older than the file is a client holding something else.
        let stale = serve_with(
            temp.path(),
            "/logo.svg",
            &[(header::IF_MODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:00 GMT")],
        )
        .await;
        assert_eq!(stale.status(), StatusCode::OK);
    }

    /// RFC 9110 §13.1.3 through the serving path, not just the helper: an
    /// entity tag that does not match must answer 200 even when the date beside
    /// it covers the file.
    #[tokio::test]
    async fn a_stale_entity_tag_wins_over_a_covering_date_on_the_wire() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("logo.svg"), b"<svg/>").expect("write");

        let full = serve_with(temp.path(), "/logo.svg", &[]).await;
        let last_modified = full.headers()[header::LAST_MODIFIED]
            .to_str()
            .expect("an ASCII date")
            .to_string();

        let response = serve_with(
            temp.path(),
            "/logo.svg",
            &[
                (header::IF_NONE_MATCH, "\"0000000000000000\""),
                (header::IF_MODIFIED_SINCE, &last_modified),
            ],
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "the date must be ignored while an entity tag is present",
        );
    }

    /// A resumed download sends the validator it was given. It was given a date
    /// as well as an entity tag, so both forms have to be honoured -- a
    /// date-formed `If-Range` used to fall through to the whole file, which is
    /// exactly the restart-from-zero a resume exists to avoid.
    #[tokio::test]
    async fn a_date_formed_if_range_resumes_rather_than_restarting() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("clip.mp3"), b"0123456789").expect("write");

        let full = serve_with(temp.path(), "/clip.mp3", &[]).await;
        let last_modified = full.headers()[header::LAST_MODIFIED]
            .to_str()
            .expect("an ASCII date")
            .to_string();

        let resumed = serve_with(
            temp.path(),
            "/clip.mp3",
            &[
                (header::RANGE, "bytes=3-5"),
                (header::IF_RANGE, &last_modified),
            ],
        )
        .await;
        assert_eq!(resumed.status(), StatusCode::PARTIAL_CONTENT);

        // A date that is not this file's is a file that changed underneath the
        // client, and the whole representation is the only correct answer.
        let changed = serve_with(
            temp.path(),
            "/clip.mp3",
            &[
                (header::RANGE, "bytes=3-5"),
                (header::IF_RANGE, "Thu, 01 Jan 1970 00:00:00 GMT"),
            ],
        )
        .await;
        assert_eq!(changed.status(), StatusCode::OK);
    }

    /// Serve `request_path` with an arbitrary request header set.
    async fn serve_with(
        public_dir: &Path,
        request_path: &str,
        request_headers: &[(header::HeaderName, &str)],
    ) -> Response {
        let mut headers = HeaderMap::new();
        for (name, value) in request_headers {
            headers.insert(name, HeaderValue::from_str(value).unwrap());
        }
        serve_public_file(public_dir, request_path, Some(&headers))
            .await
            .expect("serving must not fail")
            .expect("asset must resolve")
    }

    /// The bytes a response actually delivered, plus its status and headers.
    async fn delivered(response: Response) -> (StatusCode, HeaderMap, Vec<u8>) {
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body must be readable");
        (status, headers, body.to_vec())
    }

    /// Replay the shared cross-language byte-range table.
    ///
    /// `ruvyxa start` and a standalone deployment serve the same `public/`
    /// directory, so a video that scrubs under one has to scrub under the
    /// other. `parseByteRange` in `serverless-handler.mjs` answers this same
    /// file from `tests/packages/ruvyxa/byte-range-contract.test.mjs`.
    ///
    /// The interesting part is boundary arithmetic — inclusive ends, suffixes
    /// longer than the file, a start exactly at the length — and each of those
    /// is one fixture entry rather than a test of its own.
    #[test]
    fn resolves_the_shared_cross_language_byte_range_table() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/byte-range-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("conformance fixture is valid JSON");

        let cases = fixture["cases"].as_array().expect("fixture declares cases");
        assert!(!cases.is_empty(), "the fixture must carry cases");
        for case in cases {
            let value = case["value"].as_str().expect("case value");
            let length = case["length"].as_u64().expect("case length");
            let why = case["why"].as_str().unwrap_or_default();
            let expected = match case["kind"].as_str().expect("case kind") {
                "whole" => RangeRequest::Whole,
                "unsatisfiable" => RangeRequest::Unsatisfiable,
                "partial" => RangeRequest::Partial(ByteRange {
                    start: case["start"].as_u64().expect("partial case start"),
                    end: case["end"].as_u64().expect("partial case end"),
                }),
                kind => panic!("unknown case kind {kind:?}"),
            };
            assert_eq!(
                parse_single_byte_range(value, length),
                expected,
                "{value:?} against {length} bytes — {why}"
            );
        }
    }

    /// A media element's opening probe is answered with 206 and only those bytes.
    ///
    /// This is the request a browser makes before it will play anything, and
    /// the one a scrubbed video repeats for every seek. The streaming threshold
    /// was introduced for exactly these files, so a large asset had to answer
    /// it — before this, dragging the scrubber restarted the download from zero
    /// and a strict player refused the resource outright.
    #[tokio::test]
    async fn a_ranged_request_for_a_streamed_asset_returns_only_that_range() {
        let temp = tempfile::tempdir().expect("temp dir");
        let size = (DEFAULT_STREAMED_ASSET_THRESHOLD + 1024) as usize;
        let content: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
        std::fs::write(temp.path().join("movie.mp4"), &content).expect("write");

        let response = serve_with(temp.path(), "/movie.mp4", &[(header::RANGE, "bytes=0-1")]).await;
        let (status, headers, body) = delivered(response).await;

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
        assert_eq!(headers[header::CONTENT_LENGTH], "2");
        assert_eq!(
            headers[header::CONTENT_RANGE],
            format!("bytes 0-1/{size}"),
            "the client learns the real length from the range, not Content-Length"
        );
        assert_eq!(body, content[0..=1]);

        // A seek near the end must read from there rather than from zero.
        let start = size as u64 - 512;
        let response = serve_with(
            temp.path(),
            "/movie.mp4",
            &[(header::RANGE, &format!("bytes={start}-"))],
        )
        .await;
        let (status, headers, body) = delivered(response).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_LENGTH], "512");
        assert_eq!(body, content[start as usize..]);
    }

    /// `Accept-Ranges` is a promise about the resource, so the branch below the
    /// streaming threshold has to keep it. Audio, short clips and resumed
    /// downloads all live there.
    #[tokio::test]
    async fn a_ranged_request_for_a_buffered_asset_returns_only_that_range() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("clip.mp3"), b"0123456789").expect("write");

        let whole = serve_with(temp.path(), "/clip.mp3", &[]).await;
        assert_eq!(
            whole.headers()[header::ACCEPT_RANGES],
            "bytes",
            "a resource that advertises ranges must answer them"
        );

        let response = serve_with(temp.path(), "/clip.mp3", &[(header::RANGE, "bytes=3-5")]).await;
        let (status, headers, body) = delivered(response).await;

        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes 3-5/10");
        assert_eq!(body, b"345");
    }

    /// A range this file cannot satisfy is refused with the length, not with
    /// some other part of the file.
    #[tokio::test]
    async fn an_unsatisfiable_range_is_refused_with_the_real_length() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("clip.mp3"), b"0123456789").expect("write");

        let response =
            serve_with(temp.path(), "/clip.mp3", &[(header::RANGE, "bytes=64-128")]).await;
        let (status, headers, body) = delivered(response).await;

        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(headers[header::CONTENT_RANGE], "bytes */10");
        assert!(body.is_empty(), "a refusal must not carry file bytes");
    }

    /// `If-Range` decides whether the client is still continuing the version it
    /// started, and a mismatch means the whole file rather than a splice of two.
    #[tokio::test]
    async fn if_range_against_a_stale_validator_returns_the_whole_file() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("clip.mp3"), b"0123456789").expect("write");
        let current = etag_of(temp.path(), "/clip.mp3").await;

        let matched = serve_with(
            temp.path(),
            "/clip.mp3",
            &[(header::RANGE, "bytes=3-5"), (header::IF_RANGE, &current)],
        )
        .await;
        assert_eq!(matched.status(), StatusCode::PARTIAL_CONTENT);

        let stale = serve_with(
            temp.path(),
            "/clip.mp3",
            &[
                (header::RANGE, "bytes=3-5"),
                (header::IF_RANGE, "\"0000000000000000\""),
            ],
        )
        .await;
        let (status, _, body) = delivered(stale).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a client holding a stale version must be handed the current one whole"
        );
        assert_eq!(body, b"0123456789");
    }

    /// The client bundle directory is flat, and only that directory is served.
    ///
    /// `/__ruvyxa/client/` is the one route that reads from the build output
    /// rather than from `public/`, and the rule deciding what it may name had
    /// no test at all. Both callers reach it through `resolve_client_file`, so
    /// `render_request_cached` and a live request cannot disagree about a path.
    #[test]
    fn the_client_bundle_route_serves_one_flat_directory() {
        let temp = tempfile::tempdir().expect("temp dir");
        let client_dir = temp.path().join("client");
        std::fs::create_dir_all(client_dir.join("nested")).expect("create");
        std::fs::write(client_dir.join("entry.abc123.js"), b"//bundle").expect("write");
        std::fs::write(client_dir.join("nested").join("inner.js"), b"//inner").expect("write");
        // A real file next to the directory, which is what a traversal is for.
        std::fs::write(temp.path().join("secret.js"), b"//secret").expect("write");

        assert_eq!(
            resolve_client_file(&client_dir, "/__ruvyxa/client/entry.abc123.js"),
            contained_public_asset(&client_dir, &client_dir.join("entry.abc123.js")),
            "a hashed bundle must still be served"
        );

        for refused in [
            // Not this route.
            "/entry.abc123.js",
            "/__ruvyxa/entry.abc123.js",
            "/__ruvyxa/client",
            // The directory is flat: a separator is never part of a name.
            "/__ruvyxa/client/",
            "/__ruvyxa/client/nested/inner.js",
            "/__ruvyxa/client/nested\\inner.js",
            // Traversal, in both separator spellings and disguised in a name.
            "/__ruvyxa/client/../secret.js",
            "/__ruvyxa/client/..\\secret.js",
            "/__ruvyxa/client/..",
            "/__ruvyxa/client/a..b",
        ] {
            assert_eq!(
                resolve_client_file(&client_dir, refused),
                None,
                "{refused} must not resolve to a file"
            );
        }
    }

    /// A symlink inside the client directory cannot lead out of it.
    ///
    /// The name rules above are about text. This is the case they cannot see:
    /// every segment is an ordinary name, and only canonicalizing the result
    /// shows that it left the directory.
    #[test]
    fn a_symlinked_client_bundle_cannot_escape_its_directory() {
        let temp = tempfile::tempdir().expect("temp dir");
        let client_dir = temp.path().join("client");
        std::fs::create_dir_all(&client_dir).expect("create");
        std::fs::write(temp.path().join("secret.js"), b"//secret").expect("write");

        if link_file(
            &temp.path().join("secret.js"),
            &client_dir.join("escape.js"),
        )
        .is_none()
        {
            // Windows without the symlink privilege. The rule is unchanged;
            // this host simply cannot build the case, and CI can.
            return;
        }

        assert_eq!(
            resolve_client_file(&client_dir, "/__ruvyxa/client/escape.js"),
            None,
            "a symlinked bundle escaped the client directory"
        );
    }

    /// Create `link` pointing at `target`, or report that this host will not.
    fn link_file(target: &Path, link: &Path) -> Option<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            None
        }
    }
}
