//! In-memory FIFO cache for rendered pages and client bundles.
//!
//! Caches SSR HTML and client JS bundles keyed by (route_path, request_path, params).
//! Entries are invalidated on file change and evicted by FIFO policy when the
//! cache reaches its capacity limit.
//!
//! ## Performance characteristics
//!
//! - `get()`: O(1) with a **read lock** on hit (no write lock contention).
//! - `put()`: O(1) for new keys; replacing an existing key removes its prior
//!   queue slot to keep memory bounded and FIFO bookkeeping correct.
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
        let expired = {
            let entries = self.entries.read().await;
            if let Some(entry) = entries.get(key) {
                if entry.created_at.elapsed() <= self.ttl {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    return Some(entry.value.to_string());
                }
                true
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };

        if expired {
            self.remove_if_expired(key).await;
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Get a cached value as an `Arc<str>` (zero-copy for callers that can use it).
    pub async fn get_arc(&self, key: &str) -> Option<Arc<str>> {
        let expired = {
            let entries = self.entries.read().await;
            if let Some(entry) = entries.get(key) {
                if entry.created_at.elapsed() <= self.ttl {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    return Some(Arc::clone(&entry.value));
                }
                true
            } else {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };

        if expired {
            self.remove_if_expired(key).await;
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Return a cached value and its age without applying the cache TTL.
    /// ISR deliberately serves stale output while it regenerates in the
    /// background, so it cannot use the normal freshness-enforcing getters.
    pub async fn get_stale_with_age(&self, key: &str) -> Option<(String, Duration)> {
        let entries = self.entries.read().await;
        let entry = entries.get(key)?;
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some((entry.value.to_string(), entry.created_at.elapsed()))
    }

    /// Insert a value into the cache, evicting the oldest entry if at capacity.
    pub async fn put(&self, key: String, value: String) {
        // A zero-sized cache is explicitly disabled. Without this guard, the
        // capacity check cannot evict an item and the cache would grow forever.
        if self.capacity == 0 {
            return;
        }

        let mut entries = self.entries.write().await;
        let mut order = self.order.write().await;

        if entries.contains_key(&key) {
            // Replacements must discard the old queue position. Keeping it
            // would later evict the fresh value and grow the queue unbounded.
            order.retain(|queued_key| queued_key != &key);
        } else {
            while entries.len() >= self.capacity {
                let Some(oldest) = order.pop_front() else {
                    // The queue is internal bookkeeping; recover safely if a
                    // future change ever violates its invariant.
                    entries.clear();
                    break;
                };
                entries.remove(&oldest);
            }
        }

        entries.insert(
            key.clone(),
            CacheEntry {
                value: Arc::from(value.as_str()),
                created_at: Instant::now(),
            },
        );
        order.push_back(key);
        debug_assert_eq!(entries.len(), order.len());
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

    async fn remove_if_expired(&self, key: &str) {
        let removed = {
            let mut entries = self.entries.write().await;
            if entries
                .get(key)
                .is_some_and(|entry| entry.created_at.elapsed() > self.ttl)
            {
                entries.remove(key);
                true
            } else {
                false
            }
        };

        if removed {
            self.order
                .write()
                .await
                .retain(|queued_key| queued_key != key);
        }
    }
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
    async fn stale_lookup_keeps_isr_content_available_after_ttl() {
        let cache = RenderCache::new(1, 0);
        cache.put("isr:/".into(), "stale".into()).await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(
            cache
                .get_stale_with_age("isr:/")
                .await
                .map(|(value, _)| value),
            Some("stale".to_string())
        );
        assert_eq!(cache.get("isr:/").await, None);
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

    #[tokio::test]
    async fn replacing_a_key_keeps_fifo_bookkeeping_in_sync() {
        let cache = RenderCache::new(2, 60);
        cache.put("a".into(), "first".into()).await;
        cache.put("b".into(), "second".into()).await;
        cache.put("a".into(), "updated".into()).await;
        cache.put("c".into(), "third".into()).await;

        assert_eq!(cache.get("a").await, Some("updated".into()));
        assert_eq!(cache.get("b").await, None);
        assert_eq!(cache.get("c").await, Some("third".into()));
        assert_eq!(cache.entries.read().await.len(), 2);
        assert_eq!(cache.order.read().await.len(), 2);
    }

    #[tokio::test]
    async fn expired_entries_do_not_leave_stale_fifo_slots() {
        let cache = RenderCache::new(2, 0);
        cache.put("a".into(), "first".into()).await;
        cache.put("b".into(), "second".into()).await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(cache.get_arc("a").await, None);
        cache.put("c".into(), "third".into()).await;
        cache.put("d".into(), "fourth".into()).await;

        let entries = cache.entries.read().await;
        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key("c"));
        assert!(entries.contains_key("d"));
        drop(entries);
        let order = cache.order.read().await;
        assert_eq!(
            order.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["c", "d"]
        );
    }

    #[tokio::test]
    async fn zero_capacity_disables_cache_storage() {
        let cache = RenderCache::new(0, 60);
        cache.put("a".into(), "value".into()).await;

        assert_eq!(cache.get("a").await, None);
        assert!(cache.entries.read().await.is_empty());
        assert!(cache.order.read().await.is_empty());
    }
}
