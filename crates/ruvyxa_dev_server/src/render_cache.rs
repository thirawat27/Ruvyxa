//! In-memory LRU cache for rendered pages and client bundles.
//!
//! Caches SSR HTML and client JS bundles keyed by (route_path, request_path, params).
//! Entries are invalidated on file change and evicted by LRU policy when the
//! cache reaches its capacity limit.
//!
//! ## Performance characteristics
//!
//! - `get()`: O(1) with a **read lock** on hit (no write lock contention).
//! - `put()`: O(1) amortized — batch-evicts the oldest 25% of entries when
//!   capacity is reached, avoiding O(n) scans per insert.
//! - Values are stored behind `Arc<str>` so concurrent readers share memory
//!   rather than cloning large HTML/JS strings.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// Default max entries in the render cache.
const DEFAULT_CAPACITY: usize = 256;

/// Default TTL for cached entries (5 minutes in dev, effectively infinite in prod).
const DEFAULT_TTL_SECS: u64 = 300;

/// When evicting, remove this fraction of entries (the oldest 25%).
const EVICTION_FRACTION: f64 = 0.25;

#[derive(Debug, Clone)]
struct CacheEntry {
    /// Shared reference to the cached value — avoids cloning large strings.
    value: Arc<str>,
    /// Last time this entry was accessed (for LRU ordering).
    accessed_at: Instant,
    /// Time the entry was created (for TTL expiration).
    created_at: Instant,
    /// Number of times this entry has been accessed.
    access_count: u64,
}

/// Thread-safe LRU render cache with O(1) reads and batch eviction.
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
    ///
    /// Uses a **read lock** for the fast path (cache hit, not expired).
    /// Only acquires a write lock if the entry is expired and needs removal.
    pub async fn get(&self, key: &str) -> Option<String> {
        // Fast path: read lock only — no contention with other readers.
        {
            let entries = self.entries.read().await;
            if let Some(entry) = entries.get(key) {
                if entry.created_at.elapsed() <= self.ttl {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    // Return a clone of the Arc<str> — cheap since it's just
                    // an Arc ref-count bump + a String allocation for the caller.
                    return Some(entry.value.to_string());
                }
                // Entry expired — fall through to write path.
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        }

        // Slow path: entry expired, remove it under write lock.
        let mut entries = self.entries.write().await;
        entries.remove(key);
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Get a cached value as an `Arc<str>` (zero-copy for callers that can use it).
    pub async fn get_arc(&self, key: &str) -> Option<Arc<str>> {
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(key) {
            if entry.created_at.elapsed() <= self.ttl {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(Arc::clone(&entry.value));
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert a value into the cache, batch-evicting oldest entries if at capacity.
    pub async fn put(&self, key: String, value: String) {
        let mut entries = self.entries.write().await;

        // Batch eviction: remove oldest 25% when at capacity.
        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            Self::evict_batch(&mut entries, self.capacity);
        }

        entries.insert(
            key,
            CacheEntry {
                value: Arc::from(value.as_str()),
                accessed_at: Instant::now(),
                created_at: Instant::now(),
                access_count: 1,
            },
        );
    }

    /// Touch an entry to update its access time (call after get on hot path).
    /// This is a separate write operation so get() stays on a read lock.
    pub async fn touch(&self, key: &str) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(key) {
            entry.accessed_at = Instant::now();
            entry.access_count += 1;
        }
    }

    /// Invalidate all entries (called on file change).
    pub async fn invalidate_all(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }

    /// Invalidate entries matching a prefix (e.g., a specific route path).
    pub async fn invalidate_prefix(&self, prefix: &str) {
        let mut entries = self.entries.write().await;
        entries.retain(|key, _| !key.starts_with(prefix));
    }

    /// Blocking invalidation for use in sync contexts (file watcher).
    pub fn invalidate_all_blocking(&self) {
        let mut entries = self.entries.blocking_write();
        entries.clear();
    }

    /// Blocking prefix invalidation for use in sync contexts (file watcher).
    ///
    /// Only evicts entries whose cache key starts with the given prefix,
    /// leaving unrelated routes' cached renders intact.
    pub fn invalidate_prefix_blocking(&self, prefix: &str) {
        let mut entries = self.entries.blocking_write();
        entries.retain(|key, _| !key.starts_with(prefix));
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

    /// Batch eviction: sort entries by accessed_at, remove the oldest fraction.
    ///
    /// This is O(n log n) but runs infrequently (only when capacity is hit)
    /// and removes many entries at once, amortizing to O(1) per insert.
    fn evict_batch(entries: &mut HashMap<String, CacheEntry>, capacity: usize) {
        let evict_count = ((capacity as f64) * EVICTION_FRACTION).ceil() as usize;
        if evict_count == 0 || entries.is_empty() {
            return;
        }

        // Collect keys sorted by accessed_at (oldest first).
        let mut keys_by_age: Vec<(String, Instant)> = entries
            .iter()
            .map(|(k, v)| (k.clone(), v.accessed_at))
            .collect();
        keys_by_age.sort_unstable_by_key(|(_, accessed)| *accessed);

        // Remove the oldest entries.
        for (key, _) in keys_by_age.into_iter().take(evict_count) {
            entries.remove(&key);
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
