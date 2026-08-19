use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::image_codec::{
    Pixels, WebpSettings, decode_within_pixel_budget, encode_webp, header_dimensions, scaled_height,
};
use crate::static_assets::{contained_public_asset, is_safe_relative_path};

const MAX_SOURCE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SOURCE_PIXELS: u64 = 50_000_000;
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;

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
            max_width: 3840,
            default_quality: 82,
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
    if width < 16 || width > config.max_width.min(8192) {
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
    let source = tokio::fs::read(&file)
        .await
        .map_err(DynamicImageError::Io)?;
    let quality = quality.unwrap_or(config.default_quality).clamp(1, 100);
    let mut hasher = blake3::Hasher::new();
    hasher.update(&source);
    hasher.update(&width.to_le_bytes());
    hasher.update(&[quality]);
    let key = hasher.finalize().to_hex().to_string();
    if let Some(bytes) = cache.get(&key) {
        return Ok(bytes);
    }

    let encoded = tokio::task::spawn_blocking(move || {
        // The budget is answered from the header, before any pixels exist.
        // `MAX_SOURCE_BYTES` bounds the *compressed* size, which says nothing
        // about the decoded size: PNG compresses a uniform 50000x50000 canvas
        // into a few hundred kilobytes, so decoding first and measuring after
        // means the 10 GB allocation this limit exists to prevent has already
        // happened by the time the check runs.
        let (header_width, header_height) =
            header_dimensions(&source).map_err(|_| DynamicImageError::Decode)?;
        if u64::from(header_width) * u64::from(header_height) > MAX_SOURCE_PIXELS {
            return Err(DynamicImageError::TooLarge);
        }
        // The decoder re-checks under its own allocation limit, because a
        // header is only a claim: a truncated or hand-edited file can declare a
        // small image and then stream a much larger one.
        let decoded = decode_within_pixel_budget(&source, MAX_SOURCE_PIXELS)
            .map_err(|_| DynamicImageError::Decode)?;
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
}
