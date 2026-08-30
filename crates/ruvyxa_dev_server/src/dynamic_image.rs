use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::image_codec::{Pixels, WebpSettings, encode_webp, scaled_height};
use crate::image_decode::{DecodeError, decode_within_pixel_budget};
use crate::static_assets::{contained_public_asset, is_safe_relative_path};

const MAX_SOURCE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SOURCE_PIXELS: u64 = 50_000_000;
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// The bounds a caller of `/__ruvyxa/image` is held to.
///
/// Named rather than written inline because the deployed handler encodes the
/// same four numbers in JavaScript and cannot import them, so
/// `tests/fixtures/dynamic-image-conformance.json` holds the two together and a
/// test needs something to compare the fixture against. The quality pair also
/// had two readers inside this crate — the endpoint rejected outside it and
/// `optimize` clamped to it — written as two separate literal ranges.
pub(crate) const MIN_WIDTH: u32 = 16;
/// Widest transform any host performs, whatever `max_width` a project sets.
pub(crate) const MAX_WIDTH: u32 = 8192;
pub(crate) const MIN_QUALITY: u8 = 1;
pub(crate) const MAX_QUALITY: u8 = 100;

/// The fallback answers for a host that has loaded no project configuration.
///
/// `default_quality` is replaced by `image.quality` and `max_width` by
/// `image.onDemand.maxWidth` as soon as a config is read (`runtime_config.rs`),
/// so these are reached only by a bare `DynamicImageConfig`. They still have to
/// match `ImageOptimizationOptions::default()` in the CLI and
/// `DEFAULT_IMAGE_QUALITY` in the serverless handler: a fallback that differs
/// re-encodes at a quality nobody configured, and says nothing while it does.
pub(crate) const DEFAULT_MAX_WIDTH: u32 = 3840;
pub(crate) const DEFAULT_QUALITY: u8 = 82;

#[derive(Debug, Clone)]
pub struct DynamicImageConfig {
    pub enabled: bool,
    pub max_width: u32,
    pub default_quality: u8,
}

impl Default for DynamicImageConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_width: DEFAULT_MAX_WIDTH,
            default_quality: DEFAULT_QUALITY,
        }
    }
}

#[derive(Default)]
pub(crate) struct DynamicImageCache {
    inner: Mutex<CacheInner>,
}

/// Recency is a monotonic counter rather than a queue.
///
/// Promoting a key used to scan the order queue for it and remove it by index,
/// which is linear in the cache size on every hit and every insert. Stamping the
/// entry instead makes a hit O(1); eviction pays the scan, and only when the
/// cache is actually over its bound.
struct CacheEntry {
    bytes: Arc<[u8]>,
    used_at: u64,
}

#[derive(Default)]
struct CacheInner {
    entries: HashMap<String, CacheEntry>,
    clock: u64,
    bytes: usize,
}

impl CacheInner {
    fn tick(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    fn evict_until_within_bounds(&mut self) {
        while self.entries.len() > MAX_CACHE_ENTRIES || self.bytes > MAX_CACHE_BYTES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(removed.bytes.len());
            }
        }
    }
}

impl DynamicImageCache {
    fn get(&self, key: &str) -> Option<Arc<[u8]>> {
        let mut inner = self.inner.lock().ok()?;
        let stamp = inner.tick();
        let entry = inner.entries.get_mut(key)?;
        entry.used_at = stamp;
        Some(Arc::clone(&entry.bytes))
    }

    fn insert(&self, key: String, value: Arc<[u8]>) -> Arc<[u8]> {
        let Ok(mut inner) = self.inner.lock() else {
            return value;
        };
        if value.len() > MAX_CACHE_BYTES {
            return value;
        }
        if let Some(previous) = inner.entries.remove(&key) {
            inner.bytes = inner.bytes.saturating_sub(previous.bytes.len());
        }
        inner.bytes = inner.bytes.saturating_add(value.len());
        let used_at = inner.tick();
        inner.entries.insert(
            key,
            CacheEntry {
                bytes: Arc::clone(&value),
                used_at,
            },
        );
        inner.evict_until_within_bounds();
        value
    }
}

#[derive(Debug)]
pub(crate) enum DynamicImageError {
    InvalidRequest(&'static str),
    NotFound,
    TooLarge,
    Decode,
    Io(std::io::Error),
    Worker,
}

/// The cache key for a source whose modification time already identifies it.
///
/// Every field the answer depends on is in it: which file, how long it is, when
/// it was last written, and the two transform parameters. The content hash used
/// to carry the last two by mixing them into the digest, so dropping it for
/// settled sources moves that job here — a key that forgot either would hand one
/// request's transform to another's.
///
/// `file` arrives from `contained_public_asset`, which already returns a
/// `normalized_canonical_path`: no `\\?\` prefix, and one spelling per file
/// however the request spelled it. The path goes in as its raw OS bytes rather
/// than lossily as text, and it is the only variable-length field, so no two
/// distinct inputs can produce the same byte stream.
///
/// `None` when the timestamp predates the Unix epoch, which sends the request
/// down the content-hash path exactly as an unsettled file does.
fn settled_cache_key(
    file: &Path,
    identity: &crate::static_assets::AssetIdentity,
    width: u32,
    quality: u8,
) -> Option<String> {
    let modified = identity
        .modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(file.as_os_str().as_encoded_bytes());
    hasher.update(&identity.len.to_le_bytes());
    hasher.update(&modified.as_nanos().to_le_bytes());
    hasher.update(&width.to_le_bytes());
    hasher.update(&[quality]);
    Some(format!("settled:{}", hasher.finalize().to_hex()))
}

pub(crate) async fn optimize(
    public_dir: &Path,
    config: &DynamicImageConfig,
    cache: &DynamicImageCache,
    src: &str,
    width: u32,
    quality: Option<u8>,
) -> Result<Arc<[u8]>, DynamicImageError> {
    if !config.enabled {
        return Err(DynamicImageError::NotFound);
    }
    if width < MIN_WIDTH || width > config.max_width.min(MAX_WIDTH) {
        return Err(DynamicImageError::InvalidRequest("invalid image width"));
    }
    let relative = src
        .strip_prefix('/')
        .filter(|path| !src.starts_with("//") && is_safe_relative_path(path))
        .ok_or(DynamicImageError::InvalidRequest(
            "image src must be a root-relative public path",
        ))?;
    if src.contains(['?', '#']) {
        return Err(DynamicImageError::InvalidRequest(
            "image src must not contain a query or fragment",
        ));
    }
    // Again, inline, beside the join it guards. See `is_safe_relative_path`.
    if relative.contains("../") {
        return Err(DynamicImageError::InvalidRequest(
            "image src must not traverse outside the public directory",
        ));
    }
    let candidate = public_dir.join(relative);
    let file = contained_public_asset(public_dir, &candidate).ok_or(DynamicImageError::NotFound)?;
    let extension = file
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(extension.as_deref(), Some("png" | "jpg" | "jpeg" | "webp")) {
        return Err(DynamicImageError::InvalidRequest(
            "runtime optimization supports PNG, JPEG, and WebP",
        ));
    }
    let metadata = tokio::fs::metadata(&file)
        .await
        .map_err(DynamicImageError::Io)?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(DynamicImageError::TooLarge);
    }
    let quality = quality
        .unwrap_or(config.default_quality)
        .clamp(MIN_QUALITY, MAX_QUALITY);

    // Ask the cache before reading the file, not after.
    //
    // The key used to be a hash of the source's *contents*, so the contents had
    // to be materialised before the cache could be consulted: every hit read up
    // to `MAX_SOURCE_BYTES` off disk and blake3-hashed it only to discover it
    // already had the answer. The cache saved the encode and nothing else, and
    // an unauthenticated caller could make the server repeat that read at will.
    //
    // `static_assets` already solved this for the conditional-request path:
    // once a file's mtime is old enough that no later write can share it,
    // `(len, mtime)` identifies its bytes as well as a hash does. The same
    // `AssetIdentity` and the same `is_settled` rule decide it here, so there is
    // one settle window in this crate rather than two.
    let settled_key = crate::static_assets::asset_identity(&metadata)
        .filter(|identity| crate::static_assets::is_settled(identity, SystemTime::now()))
        .and_then(|identity| settled_cache_key(&file, &identity, width, quality));
    if let Some(key) = settled_key.as_deref()
        && let Some(bytes) = cache.get(key)
    {
        return Ok(bytes);
    }

    // Reading and hashing both move onto the blocking pool. The hash ran on the
    // async worker thread before, pinning it for the length of a 20 MiB digest.
    let read_path = file.clone();
    let hash_contents = settled_key.is_none();
    let (source, content_key) = tokio::task::spawn_blocking(move || {
        let source = std::fs::read(&read_path)?;
        // Only when the timestamp cannot vouch for the bytes. A settled source
        // is already fully named by `settled_key`, and hashing it again would
        // put back the per-request digest this change exists to remove.
        let key = hash_contents.then(|| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&source);
            hasher.update(&width.to_le_bytes());
            hasher.update(&[quality]);
            format!("content:{}", hasher.finalize().to_hex())
        });
        Ok::<_, std::io::Error>((source, key))
    })
    .await
    .map_err(|_| DynamicImageError::Worker)?
    .map_err(DynamicImageError::Io)?;

    let Some(key) = settled_key.or(content_key) else {
        return Err(DynamicImageError::Worker);
    };
    // A second lookup: it is the first one for the content-keyed path, and for
    // the settled path it costs a lock to catch a fill that landed while this
    // request was reading.
    if let Some(bytes) = cache.get(&key) {
        return Ok(bytes);
    }

    let encoded = tokio::task::spawn_blocking(move || {
        // The budget is answered from the header, before any pixels exist.
        // `MAX_SOURCE_BYTES` bounds the *compressed* size, which says nothing
        // about the decoded size: PNG compresses a uniform 50000x50000 canvas
        // into a few hundred kilobytes, so decoding first and measuring after
        // means the 10 GB allocation this limit exists to prevent has already
        // happened by the time the check runs. The decoder reads the header it
        // has already parsed and answers the budget from there, so this costs
        // no extra pass; it then re-checks under its own allocation limit,
        // because a header is only a claim and a hand-edited file can declare a
        // small image before streaming a much larger one.
        let decoded =
            decode_within_pixel_budget(&source, MAX_SOURCE_PIXELS).map_err(
                |error| match error {
                    DecodeError::TooLarge { .. } => DynamicImageError::TooLarge,
                    _ => DynamicImageError::Decode,
                },
            )?;
        let (source_width, source_height) = (decoded.width, decoded.height);
        // Borrows the decoded buffer when it is already RGB8/RGBA8, which is
        // what both PNG and JPEG decode to. A request for the source's own
        // width then reaches the encoder without a single pixel copy.
        let pixels = Pixels::from_decoded(&decoded);
        let target_width = width.min(source_width).max(1);
        let settings = WebpSettings {
            quality,
            lossless: false,
            // Runtime transforms are on the request path, where latency is what
            // the user feels; the build has a cache to amortize a slower,
            // smaller encode and this does not.
            effort: 2,
        };
        let encoded = if target_width == source_width {
            encode_webp(&pixels, settings)
        } else {
            let height = scaled_height(source_width, source_height, target_width);
            pixels
                .resize(target_width, height)
                .and_then(|resized| encode_webp(&resized, settings))
        }
        .map_err(|_| DynamicImageError::Decode)?;
        Ok::<Vec<u8>, DynamicImageError>(encoded)
    })
    .await
    .map_err(|_| DynamicImageError::Worker)??;
    Ok(cache.insert(key, Arc::from(encoded)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replay the shared `/__ruvyxa/image` bounds table.
    ///
    /// The deployed handler answers the same path from its own copy of these
    /// numbers and cannot import them, so
    /// `tests/fixtures/dynamic-image-conformance.json` holds the two together.
    /// `tests/packages/ruvyxa/serverless-shared-tables.test.mjs` drives the same
    /// file through `createHandler`. A bound changed in one language and not the
    /// other fails here rather than on one deployment target after release.
    ///
    /// Asserted through `optimize` rather than against the constants alone: a
    /// constant nothing reads is not a bound, and the width check is the one
    /// place a project's `max_width` and the absolute ceiling meet.
    #[tokio::test]
    async fn honours_the_shared_dynamic_image_bounds() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/dynamic-image-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("conformance fixture is valid JSON");

        let number = |pointer: &str| -> u64 {
            fixture
                .pointer(pointer)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_else(|| panic!("fixture declares {pointer}"))
        };

        let defaults = DynamicImageConfig::default();
        assert_eq!(
            u64::from(defaults.default_quality),
            number("/defaultQuality")
        );
        assert_eq!(u64::from(defaults.max_width), number("/defaultMaxWidth"));
        assert_eq!(u64::from(MIN_QUALITY), number("/quality/min"));
        assert_eq!(u64::from(MAX_QUALITY), number("/quality/max"));

        // The ceiling is only observable when the project's own maximum is not
        // the narrower of the two, so the config opens all the way up to it.
        let min_width = u32::try_from(number("/width/min")).expect("width fits u32");
        let max_width = u32::try_from(number("/width/max")).expect("width fits u32");
        let config = DynamicImageConfig {
            enabled: true,
            max_width,
            ..Default::default()
        };
        let temp = tempfile::tempdir().unwrap();
        let cache = DynamicImageCache::default();
        // `true` where the width itself was refused. Every other outcome means
        // the bound accepted it and the request failed later for having no file
        // behind it, which is what proves the width passed.
        let mut width_rejected = Vec::new();
        for width in [min_width - 1, min_width, max_width, max_width + 1] {
            let outcome = optimize(temp.path(), &config, &cache, "/a.png", width, None).await;
            width_rejected.push(matches!(
                outcome,
                Err(DynamicImageError::InvalidRequest("invalid image width"))
            ));
        }
        assert_eq!(
            width_rejected,
            vec![true, false, false, true],
            "widths {:?} against the shared bounds",
            [min_width - 1, min_width, max_width, max_width + 1]
        );
    }

    #[tokio::test]
    async fn rejects_external_and_traversing_sources_before_io() {
        let temp = tempfile::tempdir().unwrap();
        let config = DynamicImageConfig {
            enabled: true,
            ..Default::default()
        };
        let cache = DynamicImageCache::default();
        for source in ["https://example.com/a.png", "/../a.png", "//host/a.png"] {
            assert!(matches!(
                optimize(temp.path(), &config, &cache, source, 640, None).await,
                Err(DynamicImageError::InvalidRequest(_))
            ));
        }
    }

    #[tokio::test]
    async fn resizes_public_images_and_reuses_the_bounded_cache() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("avatar.png");
        std::fs::write(
            &source,
            crate::image_decode::fixtures::png(
                100,
                50,
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                &[10, 40, 90, 255],
            ),
        )
        .unwrap();
        let config = DynamicImageConfig {
            enabled: true,
            ..Default::default()
        };
        let cache = DynamicImageCache::default();
        let first = optimize(temp.path(), &config, &cache, "/avatar.png", 40, Some(80))
            .await
            .unwrap();
        let second = optimize(temp.path(), &config, &cache, "/avatar.png", 40, Some(80))
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        // The response is WebP, so the header read doubles as a format check.
        assert_eq!(
            crate::image_decode::header_dimensions(&first).unwrap(),
            (40, 20)
        );
    }

    /// Two solid-colour PNGs of the same dimensions, and therefore the same
    /// encoded length: the shape a `(len, mtime)` key cannot tell apart on
    /// length alone.
    fn same_length_sources() -> (Vec<u8>, Vec<u8>) {
        let of = |rgba: &[u8; 4]| {
            crate::image_decode::fixtures::png(
                100,
                50,
                png::ColorType::Rgba,
                png::BitDepth::Eight,
                rgba,
            )
        };
        let first = of(&[10, 40, 90, 255]);
        let second = of(&[200, 30, 15, 255]);
        assert_eq!(
            first.len(),
            second.len(),
            "this fixture only tests what it means to while both encode to the same length"
        );
        assert_ne!(first, second);
        (first, second)
    }

    /// Stamp a file's modification time, so a test can say how long ago a write
    /// happened without waiting for it.
    fn set_modified(path: &std::path::Path, at: std::time::SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    fn settled_config() -> DynamicImageConfig {
        DynamicImageConfig {
            enabled: true,
            ..Default::default()
        }
    }

    /// A rewritten source is a different image, even at the same byte length.
    ///
    /// This is the risk the metadata key takes on: the cache key stopped being
    /// the file's contents, so anything the `(len, mtime)` pair cannot
    /// distinguish would be served stale. Length alone cannot distinguish these
    /// two — they are the same PNG dimensions in a different colour — so the
    /// modification time is doing all of the work, and this is the test that
    /// says so. Both writes are stamped far enough back to be settled, which is
    /// the path that reuses metadata rather than bytes.
    #[tokio::test]
    async fn a_rewritten_source_of_the_same_length_serves_the_new_image() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("avatar.png");
        let (before, after) = same_length_sources();
        let config = settled_config();
        let cache = DynamicImageCache::default();
        let now = std::time::SystemTime::now();

        std::fs::write(&source, &before).unwrap();
        set_modified(&source, now - std::time::Duration::from_secs(100));
        let first = optimize(temp.path(), &config, &cache, "/avatar.png", 40, Some(80))
            .await
            .unwrap();

        std::fs::write(&source, &after).unwrap();
        set_modified(&source, now - std::time::Duration::from_secs(10));
        let second = optimize(temp.path(), &config, &cache, "/avatar.png", 40, Some(80))
            .await
            .unwrap();

        assert_ne!(
            first.as_ref(),
            second.as_ref(),
            "a rewritten source must not be served from the previous encode"
        );
    }

    /// Below the settle window the key is still the file's contents.
    ///
    /// Several filesystems record mtime to the second, so two writes inside one
    /// second can leave an identical `(len, mtime)` for different bytes. That
    /// is why the metadata key is only used once the timestamp is old enough to
    /// rule that out; everything younger falls back to hashing. Neither write
    /// here is stamped, so both are as fresh as the clock allows.
    #[tokio::test]
    async fn an_unsettled_source_is_still_identified_by_its_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("avatar.png");
        let (before, after) = same_length_sources();
        let config = settled_config();
        let cache = DynamicImageCache::default();

        std::fs::write(&source, &before).unwrap();
        let first = optimize(temp.path(), &config, &cache, "/avatar.png", 40, Some(80))
            .await
            .unwrap();

        std::fs::write(&source, &after).unwrap();
        let second = optimize(temp.path(), &config, &cache, "/avatar.png", 40, Some(80))
            .await
            .unwrap();

        assert_ne!(
            first.as_ref(),
            second.as_ref(),
            "inside the settle window the bytes must still decide the answer"
        );
    }

    /// A warm hit on a settled source does not read the file.
    ///
    /// Every request used to read the whole source (up to `MAX_SOURCE_BYTES`)
    /// and blake3-hash it *to derive the cache key*, so a hit saved the encode
    /// and nothing else: peak I/O per request stayed proportional to the source
    /// however warm the cache was.
    ///
    /// "Did not read the file" is asserted through the only thing a caller can
    /// observe: the contents are replaced with different bytes and the
    /// modification time is put back exactly, and the same `Arc` still comes
    /// back. A reader would have hashed the new bytes and produced a different
    /// key, so byte-identical reuse is proof the read did not happen.
    ///
    /// Restoring an mtime is something no ordinary write does — it is forged
    /// here precisely because the filesystem will not produce it — and it is
    /// also the one thing the settle window cannot defend against, exactly as
    /// `ASSET_ETAG_SETTLE` already documents for the ETag cache.
    #[tokio::test]
    async fn a_settled_source_is_not_reread_on_a_cache_hit() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("avatar.png");
        let (before, after) = same_length_sources();
        let config = settled_config();
        let cache = DynamicImageCache::default();
        let settled_at = std::time::SystemTime::now() - std::time::Duration::from_secs(100);

        std::fs::write(&source, &before).unwrap();
        set_modified(&source, settled_at);
        let first = optimize(temp.path(), &config, &cache, "/avatar.png", 40, Some(80))
            .await
            .unwrap();

        std::fs::write(&source, &after).unwrap();
        set_modified(&source, settled_at);
        let second = optimize(temp.path(), &config, &cache, "/avatar.png", 40, Some(80))
            .await
            .unwrap();

        assert!(
            Arc::ptr_eq(&first, &second),
            "a settled hit must answer from the cache without reading the source"
        );
    }

    /// The metadata key still separates width, quality, and path.
    ///
    /// The content hash covered width and quality by mixing them in. Dropping
    /// it for settled sources moves that job to the metadata key, and a key
    /// that forgot either would serve one request's transform to another's.
    #[tokio::test]
    async fn a_settled_key_still_separates_width_quality_and_path() {
        let temp = tempfile::tempdir().unwrap();
        let (bytes, other_bytes) = same_length_sources();
        let settled_at = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        for name in ["a.png", "b.png"] {
            let path = temp.path().join(name);
            let source = if name == "a.png" {
                &bytes
            } else {
                &other_bytes
            };
            std::fs::write(&path, source).unwrap();
            set_modified(&path, settled_at);
        }
        let config = settled_config();
        let cache = DynamicImageCache::default();

        let base = optimize(temp.path(), &config, &cache, "/a.png", 40, Some(80))
            .await
            .unwrap();
        let wider = optimize(temp.path(), &config, &cache, "/a.png", 64, Some(80))
            .await
            .unwrap();
        let coarser = optimize(temp.path(), &config, &cache, "/a.png", 40, Some(20))
            .await
            .unwrap();
        let other = optimize(temp.path(), &config, &cache, "/b.png", 40, Some(80))
            .await
            .unwrap();

        assert_eq!(
            crate::image_decode::header_dimensions(&base).unwrap(),
            (40, 20)
        );
        assert_eq!(
            crate::image_decode::header_dimensions(&wider).unwrap(),
            (64, 32)
        );
        assert_ne!(
            base.as_ref(),
            coarser.as_ref(),
            "quality is part of the key"
        );
        assert_ne!(base.as_ref(), other.as_ref(), "the path is part of the key");
    }
}
