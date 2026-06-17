//! In-memory LRU cache for rendered pages and client bundles.
//!
//! Caches SSR HTML and client JS bundles keyed by (route_path, request_path, params).
//! Entries are invalidated on file change and evicted by LRU policy when the
//! cache reaches its capacity limit.
//!
//! In production mode, pages rarely change so the cache provides near-instant
//! response times for repeated visits. In dev mode, the cache is invalidated on
//! every file change via the watcher, keeping it fresh while still deduplicating
//! concurrent requests for the same page.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// Default max entries in the render cache.
const DEFAULT_CAPACITY: usize = 256;

/// Default TTL for cached entries (5 minutes in dev, effectively infinite in prod).
const DEFAULT_TTL_SECS: u64 = 300;

#[derive(Debug, Clone)]
struct CacheEntry {
    value: String,
    accessed_at: Instant,
    created_at: Instant,
    access_count: u64,
}

/// Thread-safe LRU render cache.
pub struct RenderCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
    capacity: usize,
    ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl RenderCache {
    pub fn new(capacity: usize, ttl_secs: u64) -> Self {
        Self {
            entries: RwLock::new(HashMap::with_capacity(capacity)),
            capacity,
            ttl: Duration::from_secs(ttl_secs),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn default_dev() -> Self {
        let capacity = std::env::var("RUVYXA_RENDER_CACHE_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_CAPACITY);
        Self::new(capacity, DEFAULT_TTL_SECS)
    }

    pub fn default_production() -> Self {
        let capacity = std::env::var("RUVYXA_RENDER_CACHE_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(512);
        // 30 minutes TTL in production
        Self::new(capacity, 1800)
    }

    /// Try to get a cached value. Returns None on miss or expired entry.
    pub async fn get(&self, key: &str) -> Option<String> {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(key) {
            if entry.created_at.elapsed() > self.ttl {
                entries.remove(key);
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            entry.accessed_at = Instant::now();
            entry.access_count += 1;
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry.value.clone())
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Insert a value into the cache, evicting LRU entries if at capacity.
    pub async fn put(&self, key: String, value: String) {
        let mut entries = self.entries.write().await;

        // Evict if at capacity
        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            Self::evict_lru(&mut entries);
        }

        entries.insert(
            key,
            CacheEntry {
                value,
                accessed_at: Instant::now(),
                created_at: Instant::now(),
                access_count: 1,
            },
        );
    }

    /// Invalidate all entries (called on file change).
    pub async fn invalidate_all(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }

    /// Invalidate entries matching a prefix (e.g., a specific route path).
    #[allow(dead_code)]
    pub async fn invalidate_prefix(&self, prefix: &str) {
        let mut entries = self.entries.write().await;
        entries.retain(|key, _| !key.starts_with(prefix));
    }

    /// Blocking invalidation for use in sync contexts (file watcher).
    pub fn invalidate_all_blocking(&self) {
        let mut entries = self.entries.blocking_write();
        entries.clear();
    }

    /// Get cache statistics.
    #[allow(dead_code)]
    pub fn stats(&self) -> CacheStats {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        CacheStats {
            hits,
            misses,
            hit_rate: if hits + misses > 0 {
                hits as f64 / (hits + misses) as f64
            } else {
                0.0
            },
        }
    }

    fn evict_lru(entries: &mut HashMap<String, CacheEntry>) {
        // Find the least recently accessed entry
        if let Some(lru_key) = entries
            .iter()
            .min_by_key(|(_, entry)| entry.accessed_at)
            .map(|(key, _)| key.clone())
        {
            entries.remove(&lru_key);
        }
    }
}

#[allow(dead_code)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

/// Generate a cache key for SSR pages.
pub fn ssr_cache_key(
    request_path: &str,
    params: &std::collections::BTreeMap<String, String>,
) -> String {
    if params.is_empty() {
        format!("ssr:{request_path}")
    } else {
        let params_str: String = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        format!("ssr:{request_path}?{params_str}")
    }
}

/// Generate a cache key for client bundles.
pub fn client_cache_key(
    request_path: &str,
    params: &std::collections::BTreeMap<String, String>,
) -> String {
    if params.is_empty() {
        format!("client:{request_path}")
    } else {
        let params_str: String = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        format!("client:{request_path}?{params_str}")
    }
}
