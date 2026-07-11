//! In-memory FIFO cache for rendered pages and client bundles.
//!
//! Caches SSR HTML and client JS bundles keyed by (route_path, request_path, params).
//! Entries are invalidated on file change and evicted by FIFO policy when the
//! cache reaches its capacity limit.
//!
//! ## Performance characteristics
//!
//! - `get()`: O(1) with a **read lock** on hit (no write lock contention).
//! - `put()`: O(1) amortized — evicts the single oldest entry when capacity is
//!   reached, using a VecDeque for insertion-order tracking. No allocation
//!   storm on eviction.
//! - `touch()`: O(1) — updates accessed_at eagerly inside get(), avoiding a
//!   separate write-lock call.
//! - Values are stored behind `Arc<str>` so concurrent readers share memory
//!   rather than cloning large HTML/JS strings.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// Default max entries in the render cache.
const DEFAULT_CAPACITY: usize = 1024;

/// Default TTL for cached entries (5 minutes in dev, effectively infinite in prod).
const DEFAULT_TTL_SECS: u64 = 300;

/// When evicting, remove this fraction of entries (the oldest 25%).

#[derive(Debug, Clone)]
struct CacheEntry {
    /// Shared reference to the cached value — avoids cloning large strings.
    value: Arc<str>,
    /// Time the entry was created (for TTL expiration).
    created_at: Instant,
}

/// Thread-safe FIFO render cache with O(1) reads and O(1) eviction.
pub struct RenderCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
    /// Insertion-order queue for FIFO eviction.
    order: RwLock<VecDeque<String>>,
    capacity: usize,
    ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl RenderCache {
    pub fn new(capacity: usize, ttl_secs: u64) -> Self {
        Self {
            entries: RwLock::new(HashMap::with_capacity(capacity)),
            order: RwLock::new(VecDeque::with_capacity(capacity)),
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
    /// Acquires a write lock only if the entry needs removal.
    pub async fn get(&self, key: &str) -> Option<String> {
        {
            let entries = self.entries.read().await;
            if let Some(entry) = entries.get(key) {
                if entry.created_at.elapsed() <= self.ttl {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    return Some(entry.value.to_string());
                }
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        }

        // Entry expired — remove it under write lock.
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get(key)
            && entry.created_at.elapsed() > self.ttl
        {
            entries.remove(key);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Get a cached value as an `Arc<str>` (zero-copy for callers that can use it).
    pub async fn get_arc(&self, key: &str) -> Option<Arc<str>> {
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(key)
            && entry.created_at.elapsed() <= self.ttl
        {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some(Arc::clone(&entry.value));
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert a value into the cache, evicting the oldest entry if at capacity.
    pub async fn put(&self, key: String, value: String) {
        let mut entries = self.entries.write().await;
        let mut order = self.order.write().await;

        // O(1) FIFO eviction: pop one oldest entry if at capacity.
        if entries.len() >= self.capacity
            && !entries.contains_key(&key)
            && let Some(oldest) = order.pop_front()
        {
            entries.remove(&oldest);
        }

        entries.insert(
            key.clone(),
            CacheEntry {
                value: Arc::from(value.as_str()),
                created_at: Instant::now(),
            },
        );
        order.push_back(key);
    }

    /// Invalidate all entries (called on file change).
    pub async fn invalidate_all(&self) -> usize {
        let mut entries = self.entries.write().await;
        let invalidated = entries.len();
        entries.clear();
        self.order.write().await.clear();
        invalidated
    }

    /// Invalidate entries matching a prefix (e.g., a specific route path).
    pub async fn invalidate_prefix(&self, prefix: &str) -> usize {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|key, _| !key.starts_with(prefix));
        self.order
            .write()
            .await
            .retain(|key| !key.starts_with(prefix));
        before - entries.len()
    }

    /// Invalidate SSR/client entries belonging to a route pattern.
    pub async fn invalidate_route(&self, route_path: &str) -> usize {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|key, _| !cache_key_matches_route(key, route_path));
        self.order
            .write()
            .await
            .retain(|key| !cache_key_matches_route(key, route_path));
        before - entries.len()
    }

    /// Blocking invalidation for use in sync contexts (file watcher).
    pub fn invalidate_all_blocking(&self) -> usize {
        let mut entries = self.entries.blocking_write();
        let invalidated = entries.len();
        entries.clear();
        self.order.blocking_write().clear();
        invalidated
    }

    /// Blocking prefix invalidation for use in sync contexts (file watcher).
    pub fn invalidate_prefix_blocking(&self, prefix: &str) -> usize {
        let mut entries = self.entries.blocking_write();
        let before = entries.len();
        entries.retain(|key, _| !key.starts_with(prefix));
        self.order
            .blocking_write()
            .retain(|key| !key.starts_with(prefix));
        before - entries.len()
    }

    /// Invalidate SSR/client entries belonging to a route pattern.
    pub fn invalidate_route_blocking(&self, route_path: &str) -> usize {
        let mut entries = self.entries.blocking_write();
        let before = entries.len();
        entries.retain(|key, _| !cache_key_matches_route(key, route_path));
        self.order
            .blocking_write()
            .retain(|key| !cache_key_matches_route(key, route_path));
        before - entries.len()
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

fn cache_key_matches_route(cache_key: &str, route_path: &str) -> bool {
    let request_path = ["client:", "ssr:"]
        .into_iter()
        .find_map(|marker| cache_key.rsplit_once(marker).map(|(_, path)| path))
        .and_then(|path| path.split('?').next())
        .unwrap_or(cache_key);
    let dynamic_index = route_path
        .char_indices()
        .find(|(_, character)| matches!(character, ':' | '*' | '['))
        .map(|(index, _)| index);

    match dynamic_index {
        Some(index) => request_path.starts_with(&route_path[..index]),
        None => request_path == route_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_put_and_get() {
        let cache = RenderCache::new(4, 60);
        cache.put("a".into(), "1".into()).await;
        cache.put("b".into(), "2".into()).await;
        assert_eq!(cache.get("a").await, Some("1".into()));
        assert_eq!(cache.get("b").await, Some("2".into()));
        assert_eq!(cache.get("c").await, None);
    }

    #[tokio::test]
    async fn test_fifo_eviction() {
        let cache = RenderCache::new(3, 60);
        cache.put("a".into(), "1".into()).await;
        cache.put("b".into(), "2".into()).await;
        cache.put("c".into(), "3".into()).await;
        // Cache is full. Next insert should evict the oldest (a).
        cache.put("d".into(), "4".into()).await;
        assert_eq!(cache.get("a").await, None, "oldest entry should be evicted");
        assert_eq!(cache.get("b").await, Some("2".into()));
        assert_eq!(cache.get("c").await, Some("3".into()));
        assert_eq!(cache.get("d").await, Some("4".into()));
    }

    #[tokio::test]
    async fn test_ttl_expiry() {
        let cache = RenderCache::new(4, 0); // TTL = 0 seconds, immediate expiry
        cache.put("a".into(), "1".into()).await;
        // Small delay to ensure TTL elapses
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(cache.get("a").await, None);
    }

    #[tokio::test]
    async fn test_invalidate_all() {
        let cache = RenderCache::new(4, 60);
        cache.put("a".into(), "1".into()).await;
        cache.put("b".into(), "2".into()).await;
        assert_eq!(cache.invalidate_all().await, 2);
        assert_eq!(cache.get("a").await, None);
        assert_eq!(cache.get("b").await, None);
    }

    #[tokio::test]
    async fn test_invalidate_prefix() {
        let cache = RenderCache::new(4, 60);
        cache.put("ssr:/a".into(), "1".into()).await;
        cache.put("ssr:/b".into(), "2".into()).await;
        cache.put("client:/a".into(), "3".into()).await;
        assert_eq!(cache.invalidate_prefix("ssr:").await, 2);
        assert_eq!(cache.get("ssr:/a").await, None);
        assert_eq!(cache.get("ssr:/b").await, None);
        assert_eq!(cache.get("client:/a").await, Some("3".into()));
    }

    #[tokio::test]
    async fn test_invalidate_route_across_render_namespaces() {
        let cache = RenderCache::new(8, 60);
        cache.put("ssr:/blog/one".into(), "1".into()).await;
        cache.put("client:/blog/one".into(), "2".into()).await;
        cache.put("isr:ssr:/blog/two".into(), "3".into()).await;
        cache.put("ssr:/about".into(), "4".into()).await;

        assert_eq!(cache.invalidate_route("/blog/:slug").await, 3);
        assert_eq!(cache.get("ssr:/about").await, Some("4".into()));
    }

    #[tokio::test]
    async fn test_eviction_frees_capacity() {
        let cache = RenderCache::new(2, 60);
        cache.put("a".into(), "1".into()).await;
        cache.put("b".into(), "2".into()).await;
        cache.put("c".into(), "3".into()).await; // evicts a
        assert_eq!(cache.get("a").await, None);
        // Now put another — should evict b
        cache.put("d".into(), "4".into()).await;
        assert_eq!(cache.get("b").await, None);
    }

    #[tokio::test]
    async fn test_put_existing_key_does_not_evict() {
        let cache = RenderCache::new(2, 60);
        cache.put("a".into(), "1".into()).await;
        cache.put("b".into(), "2".into()).await;
        // Re-insert existing key
        cache.put("a".into(), "updated".into()).await;
        assert_eq!(cache.get("a").await, Some("updated".into()));
        assert_eq!(cache.get("b").await, Some("2".into()));
    }
}
