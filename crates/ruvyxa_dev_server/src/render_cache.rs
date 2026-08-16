//! In-memory LRU cache for rendered pages and client bundles.
//!
//! Caches SSR HTML and client JS bundles keyed by (route_path, request_path, params).
//! Entries are invalidated on file change and evicted by least-recently-used policy when the
//! cache reaches its capacity limit.
//!
//! ## Performance characteristics
//!
//! - `get()`: O(1) lookup, then O(1) recency promotion on hit via a hash-indexed
//!   doubly linked recency list (no linear queue scans).
//! - `put()`: O(1) insert or refresh; evicts the least recently used key in O(1)
//!   when the cache reaches capacity.
//! - Values are stored behind `Arc<str>` so concurrent readers share memory
//!   rather than cloning large HTML/JS strings.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use ruvyxa_graph::RouteParams;
use tokio::sync::RwLock;

/// Default max entries in the render cache.
const DEFAULT_CAPACITY: usize = 1024;

/// Default TTL for cached entries (5 minutes in dev, effectively infinite in prod).
const DEFAULT_TTL_SECS: u64 = 300;

/// Maximum capacity accepted from `RUVYXA_RENDER_CACHE_SIZE`.
///
/// `RenderCache::new` remains useful for internal callers that need an exact capacity. This limit
/// applies only to the environment-controlled default constructors, preventing a typo in a process
/// environment from triggering an unbounded eager allocation during server startup.
const MAX_ENV_RENDER_CACHE_CAPACITY: usize = 16_384;

#[derive(Debug, Clone)]
struct CacheEntry {
    /// Shared reference to the cached value — avoids cloning large strings.
    value: Arc<str>,
    /// Compressed copy of `value`, built on the first hit that can use one.
    compressed: Arc<OnceLock<CompressedDocument>>,
    /// Time the entry was created (for TTL expiration).
    created_at: Instant,
}

/// One content-encoded copy of a cached document.
#[derive(Debug)]
pub struct CompressedDocument {
    /// `Content-Encoding` this copy carries.
    pub encoding: &'static str,
    pub bytes: Arc<[u8]>,
}

/// A cached document together with the compressed copy that shares its lifetime.
///
/// Serving a cached page used to cost a full compression pass per request: the
/// render cache stored the HTML, and the `CompressionLayer` outside it saw only
/// a response body and re-compressed the identical bytes every time. Sharing the
/// stored `Arc<str>` saved a copy measured in microseconds while the compression
/// it fed cost milliseconds — the optimisation was one layer short.
///
/// The compressed copy is built on the first hit that can use it, not at `put`,
/// so a page rendered and never requested again pays nothing.
#[derive(Debug, Clone)]
pub struct CachedDocument {
    pub html: Arc<str>,
    compressed: Arc<OnceLock<CompressedDocument>>,
}

impl CachedDocument {
    /// A document with nowhere to keep a compressed copy.
    ///
    /// Used by responses that are not cache-backed — error pages, dev-mode
    /// documents — so they behave exactly as before: compressed once, by the
    /// layer, for this request only.
    pub fn uncached(html: Arc<str>) -> Self {
        Self {
            html,
            compressed: Arc::new(OnceLock::new()),
        }
    }

    /// The compressed copy, building it with `encode` on first use.
    ///
    /// Returns `None` when the caller cannot use any encoding this document has
    /// or could produce; the plain body is then served and the outer layer
    /// decides what to do with it.
    pub fn compressed(
        &self,
        accepts: impl Fn(&str) -> bool,
        encode: impl FnOnce(&str) -> Option<CompressedDocument>,
    ) -> Option<&CompressedDocument> {
        if let Some(existing) = self.compressed.get() {
            // A stored copy is only usable by a client that accepts it. Anyone
            // else falls through rather than forcing a second encoding into a
            // slot sized for one.
            return accepts(existing.encoding).then_some(existing);
        }
        let built = encode(&self.html)?;
        // A concurrent hit may have won the race; either copy encodes the same
        // bytes, so whichever landed first stands.
        let stored = match self.compressed.set(built) {
            Ok(()) => self.compressed.get()?,
            Err(_) => self.compressed.get()?,
        };
        accepts(stored.encoding).then_some(stored)
    }
}

/// Neighbor links for one key in the recency order.
#[derive(Debug, Default, Clone)]
struct RecencyLinks {
    /// Key one step closer to least recently used, `None` at the front.
    prev: Option<Arc<str>>,
    /// Key one step closer to most recently used, `None` at the back.
    next: Option<Arc<str>>,
}

/// Least-to-most recently used key order with O(1) promotion and removal.
///
/// Implemented as a doubly linked list whose nodes are addressed by key through
/// a hash map, so recency updates never scan the whole order.
#[derive(Debug, Default)]
struct RecencyList {
    links: HashMap<Arc<str>, RecencyLinks>,
    /// Least recently used key.
    head: Option<Arc<str>>,
    /// Most recently used key.
    tail: Option<Arc<str>>,
}

impl RecencyList {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            links: HashMap::with_capacity(capacity),
            head: None,
            tail: None,
        }
    }

    fn len(&self) -> usize {
        self.links.len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// Append a key as most recently used. The key must not already be linked.
    fn push_back(&mut self, key: Arc<str>) {
        debug_assert!(
            !self.links.contains_key(&*key),
            "push_back requires an unlinked key"
        );
        let links = RecencyLinks {
            prev: self.tail.clone(),
            next: None,
        };
        match &self.tail {
            Some(tail) => {
                let tail_links = self
                    .links
                    .get_mut(tail)
                    .expect("tail key must stay linked while holding the order lock");
                tail_links.next = Some(Arc::clone(&key));
            }
            None => self.head = Some(Arc::clone(&key)),
        }
        self.tail = Some(Arc::clone(&key));
        self.links.insert(key, links);
    }

    /// Unlink a key and return its owned handle, or `None` when absent.
    fn take(&mut self, key: &str) -> Option<Arc<str>> {
        let (owned, links) = self.links.remove_entry(key)?;
        match &links.prev {
            Some(prev) => {
                let prev_links = self
                    .links
                    .get_mut(prev)
                    .expect("linked neighbor must stay indexed while holding the order lock");
                prev_links.next = links.next.clone();
            }
            None => self.head = links.next.clone(),
        }
        match &links.next {
            Some(next) => {
                let next_links = self
                    .links
                    .get_mut(next)
                    .expect("linked neighbor must stay indexed while holding the order lock");
                next_links.prev = links.prev.clone();
            }
            None => self.tail = links.prev.clone(),
        }
        Some(owned)
    }

    /// Remove a key from the order, if present.
    fn remove(&mut self, key: &str) -> bool {
        self.take(key).is_some()
    }

    /// Move an existing key to most recently used. Absent keys are ignored.
    fn promote(&mut self, key: &str) {
        if let Some(owned) = self.take(key) {
            self.push_back(owned);
        }
    }

    /// Remove and return the least recently used key.
    fn pop_front(&mut self) -> Option<Arc<str>> {
        let head = self.head.clone()?;
        self.take(&head)
    }

    fn clear(&mut self) {
        self.links.clear();
        self.head = None;
        self.tail = None;
    }

    /// Drop every key rejected by the predicate, preserving relative order.
    fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        let mut cursor = self.head.clone();
        while let Some(key) = cursor {
            cursor = self.links.get(&*key).and_then(|links| links.next.clone());
            if !keep(&key) {
                self.take(&key);
            }
        }
    }

    /// Keys from least to most recently used, for test assertions.
    fn keys_front_to_back(&self) -> Vec<Arc<str>> {
        let mut keys = Vec::with_capacity(self.links.len());
        let mut cursor = self.head.clone();
        while let Some(key) = cursor {
            cursor = self.links[&*key].next.clone();
            keys.push(key);
        }
        keys
    }

    /// Keys from most to least recently used, for test assertions.
    #[cfg(test)]
    fn keys_back_to_front(&self) -> Vec<Arc<str>> {
        let mut keys = Vec::with_capacity(self.links.len());
        let mut cursor = self.tail.clone();
        while let Some(key) = cursor {
            cursor = self.links[&*key].prev.clone();
            keys.push(key);
        }
        keys
    }
}

/// Read-only cache state exposed to development tooling.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderCacheSnapshot {
    pub entries: usize,
    pub capacity: usize,
    pub ttl_seconds: u64,
    pub hits: u64,
    pub misses: u64,
    /// Keys ordered from least to most recently used.
    pub lru_keys: Vec<String>,
    /// Paths named by `revalidatePath()` that are still waiting for a render to
    /// retire their claim.
    ///
    /// A claim keeps its path from being served out of the prerender directory,
    /// so a number that climbs and never falls is the visible form of an
    /// artifact that cannot be replaced — and the approach to
    /// [`MAX_FORCED_REVALIDATIONS`], past which `bypass_prerendered` turns
    /// on for the whole process.
    pub forced_pending: usize,
    /// True once the bounded claim set overflowed and every prerendered
    /// artifact is being bypassed until the process restarts.
    pub bypass_prerendered: bool,
}

/// Maximum number of exact paths waiting to bypass a prerendered artifact.
///
/// Retaining more strings would let application input grow process memory
/// without bound. On overflow the cache deliberately fails closed: every
/// prerendered artifact is bypassed until restart, so correctness is preserved
/// at the cost of cache performance rather than silently losing invalidations.
const MAX_FORCED_REVALIDATIONS: usize = 1_024;

/// Pending-claim count that earns a warning while there is still time to act.
///
/// Reaching [`MAX_FORCED_REVALIDATIONS`] bypasses every prerendered artifact
/// for the life of the process, and the log line that reports it arrives after
/// the fact. A host whose prerender directory cannot be rewritten — read-only
/// image, wrong ownership — retires no claims and walks to that limit with no
/// other symptom than rising render load. `RenderCacheSnapshot` carries the
/// same numbers, but its only reader is the dev-mode DevTools endpoint, so a
/// production server needs the log.
const FORCED_REVALIDATION_HIGH_WATER: usize = MAX_FORCED_REVALIDATIONS * 3 / 4;

/// A warning at the limit would arrive after the bypass it warns about.
const _: () = assert!(FORCED_REVALIDATION_HIGH_WATER < MAX_FORCED_REVALIDATIONS);

/// What recording one claim did to the bounded state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkOutcome {
    Recorded,
    /// The pending set just crossed [`FORCED_REVALIDATION_HIGH_WATER`].
    HighWater,
    /// The bounded set overflowed; every prerendered artifact is now bypassed.
    FailedClosed,
}

#[derive(Debug, Default)]
struct ForcedRevalidations {
    paths: HashMap<String, u64>,
    next_generation: u64,
    bypass_prerendered: bool,
    /// Set once the high-water warning has been logged, so a server sitting
    /// just above the mark reports it once instead of on every claim.
    warned_high_water: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForcedRevalidationClaim {
    Exact(u64),
    All,
}

impl ForcedRevalidations {
    /// Mark one path, reporting what that did to the bounded state.
    fn mark(&mut self, path: &str) -> MarkOutcome {
        if self.bypass_prerendered {
            return MarkOutcome::Recorded;
        }
        if !self.paths.contains_key(path) && self.paths.len() >= MAX_FORCED_REVALIDATIONS {
            self.fail_closed();
            return MarkOutcome::FailedClosed;
        }
        let Some(generation) = self.next_generation.checked_add(1) else {
            self.fail_closed();
            return MarkOutcome::FailedClosed;
        };
        self.next_generation = generation;
        self.paths.insert(path.to_string(), generation);

        if !self.warned_high_water && self.paths.len() >= FORCED_REVALIDATION_HIGH_WATER {
            self.warned_high_water = true;
            return MarkOutcome::HighWater;
        }
        MarkOutcome::Recorded
    }

    fn claim(&self, path: &str) -> Option<ForcedRevalidationClaim> {
        if self.bypass_prerendered {
            Some(ForcedRevalidationClaim::All)
        } else {
            self.paths
                .get(path)
                .copied()
                .map(ForcedRevalidationClaim::Exact)
        }
    }

    fn acknowledge(&mut self, path: &str, claim: ForcedRevalidationClaim) {
        let ForcedRevalidationClaim::Exact(generation) = claim else {
            return;
        };
        if self.paths.get(path) == Some(&generation) {
            self.paths.remove(path);
        }
    }

    fn fail_closed(&mut self) {
        self.paths.clear();
        self.bypass_prerendered = true;
        // Nothing left to warn about approaching: the limit is already behind
        // us, and the caller logs the overflow itself.
        self.warned_high_water = true;
    }
}

/// Thread-safe LRU render cache.
pub struct RenderCache {
    entries: RwLock<HashMap<Arc<str>, CacheEntry>>,
    /// Least-to-most recently used key order.
    order: RwLock<RecencyList>,
    /// Generation claims for paths an application asked to revalidate.
    ///
    /// Dropping the in-memory entry is not enough for a prerendered strategy:
    /// SSG, ISR, PPR, and CSR fall back to HTML the build wrote to disk, so a
    /// bounded-memory refresh alone cannot acknowledge the claim — TTL/LRU
    /// eviction would resurrect that stale artifact. Those strategies replace
    /// the artifact with their fresh render before acknowledging
    /// (`settle_forced_revalidation` in `render_pipeline.rs`); when the write
    /// cannot happen the claim stays pending and keeps bypassing disk. SSR has
    /// no build artifact and acknowledges its exact generation after a
    /// successful render. A failed render leaves every claim intact, and
    /// generation matching prevents an older success from clearing a newer
    /// request for the same URL.
    ///
    /// `RenderCacheSnapshot` reports the pending count and the global bypass
    /// flag, so a host that cannot write its prerender directory shows up as a
    /// climbing number rather than as unexplained render load.
    forced: RwLock<ForcedRevalidations>,
    capacity: usize,
    ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl RenderCache {
    pub fn new(capacity: usize, ttl_secs: u64) -> Self {
        Self {
            entries: RwLock::new(HashMap::with_capacity(capacity)),
            order: RwLock::new(RecencyList::with_capacity(capacity)),
            forced: RwLock::new(ForcedRevalidations::default()),
            capacity,
            ttl: Duration::from_secs(ttl_secs),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn default_dev() -> Self {
        let configured = std::env::var("RUVYXA_RENDER_CACHE_SIZE").ok();
        let capacity = render_cache_capacity(configured.as_deref(), DEFAULT_CAPACITY);
        Self::new(capacity, DEFAULT_TTL_SECS)
    }

    pub fn default_production() -> Self {
        let configured = std::env::var("RUVYXA_RENDER_CACHE_SIZE").ok();
        let capacity = render_cache_capacity(configured.as_deref(), 512);
        // 30 minutes TTL in production
        Self::new(capacity, 1800)
    }

    /// Capture cache counters and LRU state without changing recency.
    pub async fn snapshot(&self) -> RenderCacheSnapshot {
        let entries = self.entries.read().await.len();
        let lru_keys = self
            .order
            .read()
            .await
            .keys_front_to_back()
            .into_iter()
            .map(|key| key.to_string())
            .collect();
        let (forced_pending, bypass_prerendered) = {
            let forced = self.forced.read().await;
            (forced.paths.len(), forced.bypass_prerendered)
        };
        RenderCacheSnapshot {
            entries,
            capacity: self.capacity,
            ttl_seconds: self.ttl.as_secs(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            lru_keys,
            forced_pending,
            bypass_prerendered,
        }
    }

    /// Try to get a cached value as an owned `String`.
    ///
    /// A successful read promotes the entry to most recently used.
    ///
    /// Prefer [`RenderCache::get_arc`] on request paths: this variant copies the
    /// whole document on every cache hit, which for a large page at high request
    /// rates is the dominant allocation in an otherwise trivial response.
    #[cfg(test)]
    pub async fn get(&self, key: &str) -> Option<String> {
        self.get_arc(key).await.map(|value| value.to_string())
    }

    /// Get a cached value as an `Arc<str>`, sharing the stored allocation.
    #[cfg(test)]
    pub async fn get_arc(&self, key: &str) -> Option<Arc<str>> {
        self.get_document(key).await.map(|document| document.html)
    }

    /// Get a cached document, sharing both the stored allocation and the slot
    /// holding its compressed copy.
    pub async fn get_document(&self, key: &str) -> Option<CachedDocument> {
        let cached = {
            let entries = self.entries.read().await;
            if let Some(entry) = entries.get(key) {
                if entry.created_at.elapsed() <= self.ttl {
                    Some(CachedDocument {
                        html: Arc::clone(&entry.value),
                        compressed: Arc::clone(&entry.compressed),
                    })
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(value) = cached {
            self.hits.fetch_add(1, Ordering::Relaxed);
            self.promote(key).await;
            return Some(value);
        }

        if self.entries.read().await.contains_key(key) {
            self.remove_if_expired(key).await;
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Return a cached value and its age without applying the cache TTL.
    /// ISR deliberately serves stale output while it regenerates in the
    /// background, so it cannot use the normal freshness-enforcing getters.
    pub async fn get_stale_with_age(&self, key: &str) -> Option<(CachedDocument, Duration)> {
        let cached = {
            let entries = self.entries.read().await;
            let entry = entries.get(key)?;
            (
                CachedDocument {
                    html: Arc::clone(&entry.value),
                    compressed: Arc::clone(&entry.compressed),
                },
                entry.created_at.elapsed(),
            )
        };
        self.hits.fetch_add(1, Ordering::Relaxed);
        self.promote(key).await;
        Some(cached)
    }

    /// Insert a value into the cache, evicting the oldest entry if at capacity.
    ///
    /// Returns the stored [`Arc<str>`] so the caller can serve the same
    /// allocation it just cached. Callers used to pass `value.clone()` and keep
    /// the original, which made a second full copy of every rendered page on top
    /// of the one this method has to make to build the `Arc`.
    pub async fn put(&self, key: String, value: String) -> CachedDocument {
        let stored: Arc<str> = Arc::from(value);
        let compressed = Arc::new(OnceLock::new());

        // A zero-sized cache is explicitly disabled. Without this guard, the
        // capacity check cannot evict an item and the cache would grow forever.
        // The value is still returned so a disabled cache changes only caching,
        // never what the caller serves.
        if self.capacity == 0 {
            return CachedDocument {
                html: stored,
                compressed,
            };
        }

        let key: Arc<str> = Arc::from(key);
        let mut entries = self.entries.write().await;
        let mut order = self.order.write().await;

        if entries.contains_key(&*key) {
            // A replacement becomes the most recently used value.
            order.remove(&key);
        } else {
            while entries.len() >= self.capacity {
                let Some(oldest) = order.pop_front() else {
                    // The order is internal bookkeeping; recover safely if a
                    // future change ever violates its invariant.
                    entries.clear();
                    break;
                };
                entries.remove(&*oldest);
            }
        }

        entries.insert(
            Arc::clone(&key),
            CacheEntry {
                value: Arc::clone(&stored),
                compressed: Arc::clone(&compressed),
                created_at: Instant::now(),
            },
        );
        order.push_back(key);
        debug_assert_eq!(entries.len(), order.len());
        CachedDocument {
            html: stored,
            compressed,
        }
    }

    /// Drop every entry the predicate rejects, keeping the index and the
    /// recency order in step. Returns how many entries were removed.
    ///
    /// Both guards are taken by the caller, which is the only thing that
    /// differs between the async and blocking invalidation entry points. The
    /// selection rule itself lives here once: it used to be written out again
    /// in every variant, so a change to what a route key matches had to be made
    /// in each copy or the two halves of the same operation would disagree.
    fn retain_matching(
        entries: &mut HashMap<Arc<str>, CacheEntry>,
        order: &mut RecencyList,
        discard: impl Fn(&str) -> bool,
    ) -> usize {
        let before = entries.len();
        entries.retain(|key, _| !discard(key));
        order.retain(|key| !discard(key));
        debug_assert_eq!(entries.len(), order.len());
        before - entries.len()
    }

    /// Invalidate all entries (called on file change).
    pub async fn invalidate_all(&self) -> usize {
        let mut entries = self.entries.write().await;
        let mut order = self.order.write().await;
        Self::retain_matching(&mut entries, &mut order, |_| true)
    }

    /// Drop every cached render of one concrete URL and force the next one.
    ///
    /// This is what `revalidatePath()` reaches. It matches across every render
    /// namespace, because the caller names a URL and does not know — and should
    /// not have to know — whether that URL is served as SSR, SSG, ISR, PPR, or
    /// CSR. Parameterised variants of the same path are matched too: the key
    /// carries the bound parameters after a `?`, and they belong to this URL.
    ///
    /// Returns the number of entries dropped, which is zero for a path that was
    /// never rendered — a legitimate outcome, since a webhook may revalidate a
    /// URL no one has requested yet. The path is still marked, so the first
    /// request for it renders fresh rather than serving the build's HTML.
    pub async fn revalidate_path(&self, request_path: &str) -> usize {
        let matches = |key: &str| cache_key_matches_path(key, request_path);
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|key, _| !matches(key));
        self.order.write().await.retain(|key| !matches(key));
        let dropped = before - entries.len();
        drop(entries);
        match self.forced.write().await.mark(request_path) {
            MarkOutcome::Recorded => {}
            MarkOutcome::HighWater => {
                tracing::warn!(
                    pending = FORCED_REVALIDATION_HIGH_WATER,
                    limit = MAX_FORCED_REVALIDATIONS,
                    "pending path revalidations are approaching the bounded exact set; \
                     a claim is retired only once its prerendered document is replaced, \
                     so check that the prerender directory is writable"
                );
            }
            MarkOutcome::FailedClosed => {
                tracing::warn!(
                    limit = MAX_FORCED_REVALIDATIONS,
                    "pending path revalidations exceeded the bounded exact set; bypassing prerendered artifacts"
                );
            }
        }
        dropped
    }

    /// Invalidate every in-memory render and bypass all build artifacts.
    ///
    /// This is the bounded defensive response to an oversized revalidation
    /// payload from an older or untrusted worker: walking an arbitrary vector
    /// would itself be a CPU/memory denial of service, while dropping its tail
    /// could serve stale content.
    pub async fn revalidate_all_paths(&self) -> usize {
        let mut entries = self.entries.write().await;
        let dropped = entries.len();
        entries.clear();
        self.order.write().await.clear();
        drop(entries);
        self.forced.write().await.fail_closed();
        dropped
    }

    /// Snapshot a pending revalidation for `request_path` without consuming it.
    ///
    /// Several requests may receive the same claim and render concurrently.
    /// Correctness comes first: the claim remains live until one successful
    /// render stores fresh output and acknowledges the exact generation.
    pub(crate) async fn forced_claim(&self, request_path: &str) -> Option<ForcedRevalidationClaim> {
        self.forced.read().await.claim(request_path)
    }

    /// Acknowledge a forced render only after its fresh output was stored.
    ///
    /// Generation matching prevents a slow render from clearing a newer
    /// `revalidatePath()` call for the same URL. Global fail-closed claims are
    /// intentionally never acknowledged and last until process restart.
    pub(crate) async fn acknowledge_forced(
        &self,
        request_path: &str,
        claim: ForcedRevalidationClaim,
    ) {
        self.forced.write().await.acknowledge(request_path, claim);
    }

    /// Invalidate SSR/client entries belonging to a route pattern.
    pub async fn invalidate_route(&self, route_path: &str) -> usize {
        let mut entries = self.entries.write().await;
        let mut order = self.order.write().await;
        Self::retain_matching(&mut entries, &mut order, |key| {
            cache_key_matches_route(key, route_path)
        })
    }

    /// Blocking invalidation for use in sync contexts (file watcher).
    pub fn invalidate_all_blocking(&self) -> usize {
        let mut entries = self.entries.blocking_write();
        let mut order = self.order.blocking_write();
        Self::retain_matching(&mut entries, &mut order, |_| true)
    }

    /// Invalidate SSR/client entries belonging to a route pattern.
    ///
    /// This is the file watcher's selective path, and it already reaches the
    /// route's `client:` entries as well as its `ssr:` ones — see
    /// `cache_key_matches_route`. A prefix-based variant existed alongside it
    /// and was only ever called from tests.
    pub fn invalidate_route_blocking(&self, route_path: &str) -> usize {
        let mut entries = self.entries.blocking_write();
        let mut order = self.order.blocking_write();
        Self::retain_matching(&mut entries, &mut order, |key| {
            cache_key_matches_route(key, route_path)
        })
    }

    /// Drop an entry whose TTL has passed.
    ///
    /// Both maps are locked for the whole removal, in the same order `put`
    /// takes them. Releasing `entries` first left a window where a concurrent
    /// `put` of the same key could re-insert it and push it onto `order`, only
    /// for this call to then remove it from `order` alone — leaving a key that
    /// eviction could never reach. The eviction loop recovers from that by
    /// clearing the whole cache, so the cost of the race was a silent flush of
    /// every cached render, not a leak.
    async fn remove_if_expired(&self, key: &str) {
        let mut entries = self.entries.write().await;
        if entries
            .get(key)
            .is_some_and(|entry| entry.created_at.elapsed() > self.ttl)
        {
            entries.remove(key);
            self.order.write().await.remove(key);
        }
    }

    async fn promote(&self, key: &str) {
        self.order.write().await.promote(key);
    }
}

fn render_cache_capacity(value: Option<&str>, default: usize) -> usize {
    value
        .and_then(|value| value.parse::<usize>().ok())
        .map(|capacity| capacity.min(MAX_ENV_RENDER_CACHE_CAPACITY))
        .unwrap_or(default)
}

/// The render strategies that give a cached page its own key space.
///
/// Both halves of the key contract live here: the prefix a key is built with and
/// the prefix invalidation strips off. They used to be apart — keys were built at
/// each call site with `format!("csr:{…}")` and stripped from a list in
/// `cache_key_matches_route` — and the list had never gained `csr:`. A CSR page
/// therefore matched no route during invalidation: editing its file left the
/// cached document in place until its TTL expired, so the dev server kept serving
/// the previous render of a file the author had just changed.
pub const RENDER_NAMESPACES: [&str; 4] = ["ssg:", "isr:", "ppr:", "csr:"];

/// Build the cache key for a page render.
///
/// `namespace` is one of [`RENDER_NAMESPACES`], or empty for plain SSR. Taking it
/// here rather than wrapping the result in a second `format!` also drops one
/// string allocation from every page request.
pub fn page_cache_key(namespace: &str, request_path: &str, params: &RouteParams) -> String {
    if params.is_empty() {
        format!("{namespace}ssr:{request_path}")
    } else {
        let params_str = serde_json::to_string(params).unwrap_or_default();
        format!("{namespace}ssr:{request_path}?{params_str}")
    }
}

/// Generate a cache key for SSR pages.
pub fn ssr_cache_key(request_path: &str, params: &RouteParams) -> String {
    page_cache_key("", request_path, params)
}

/// Generate a cache key for client bundles.
pub fn client_cache_key(request_path: &str, params: &RouteParams) -> String {
    if params.is_empty() {
        format!("client:{request_path}")
    } else {
        let params_str = serde_json::to_string(params).unwrap_or_default();
        format!("client:{request_path}?{params_str}")
    }
}

/// Does `cache_key` hold a render of exactly `request_path`?
///
/// Shares the structural prefix stripping with `cache_key_matches_route` and
/// for the same reason: a catch-all path or a serialized parameter can contain
/// the text `ssr:`, so searching for the marker anywhere in the key leaves
/// stale entries alive.
fn cache_key_matches_path(cache_key: &str, request_path: &str) -> bool {
    let Some(keyed_path) = cache_key_request_path(cache_key) else {
        return false;
    };
    keyed_path == request_path
}

/// The request path a cache key was built from, or `None` if it is not a page
/// or client key.
fn cache_key_request_path(cache_key: &str) -> Option<&str> {
    let without_namespace = RENDER_NAMESPACES
        .into_iter()
        .find_map(|namespace| cache_key.strip_prefix(namespace))
        .unwrap_or(cache_key);
    ["client:", "ssr:"]
        .into_iter()
        .find_map(|marker| without_namespace.strip_prefix(marker))
        .map(|path| path.split('?').next().unwrap_or(path))
}

fn cache_key_matches_route(cache_key: &str, route_path: &str) -> bool {
    // Keys are `ssr:`/`client:` optionally wrapped in a render namespace
    // (`ssg:`/`isr:`/`ppr:`). Strip prefixes structurally — searching for
    // the marker anywhere in the key would mis-parse catch-all request
    // paths or serialized params that contain "ssr:"/"client:" as text,
    // leaving stale entries alive after a file change.
    let without_namespace = RENDER_NAMESPACES
        .into_iter()
        .find_map(|namespace| cache_key.strip_prefix(namespace))
        .unwrap_or(cache_key);
    let request_path = ["client:", "ssr:"]
        .into_iter()
        .find_map(|marker| without_namespace.strip_prefix(marker))
        .map(|path| path.split('?').next().unwrap_or(path))
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
    use std::collections::HashSet;

    /// Assert that the entry index and the recency order agree on the same
    /// live keys and that the doubly linked order is internally consistent
    /// in both directions.
    async fn assert_index_and_order_consistent(cache: &RenderCache) {
        let entries = cache.entries.read().await;
        let order = cache.order.read().await;

        assert_eq!(entries.len(), order.len(), "index and order length differ");

        let forward = order.keys_front_to_back();
        let mut backward = order.keys_back_to_front();
        backward.reverse();
        assert_eq!(
            forward.iter().map(|key| key.as_ref()).collect::<Vec<_>>(),
            backward.iter().map(|key| key.as_ref()).collect::<Vec<_>>(),
            "forward and backward order walks disagree"
        );
        assert_eq!(forward.len(), order.len(), "order walk skipped linked keys");

        let entry_keys: HashSet<&str> = entries.keys().map(|key| key.as_ref()).collect();
        let order_keys: HashSet<&str> = forward.iter().map(|key| key.as_ref()).collect();
        assert_eq!(entry_keys, order_keys, "index and order key sets differ");
    }

    async fn order_snapshot(cache: &RenderCache) -> Vec<String> {
        cache
            .order
            .read()
            .await
            .keys_front_to_back()
            .iter()
            .map(|key| key.to_string())
            .collect()
    }

    /// `put` hands back the very allocation it stored, and `get_arc` hands back
    /// that same one. Callers used to pass `value.clone()` and read with `get`,
    /// making one full copy of every rendered page on write and another on every
    /// cache hit.
    #[tokio::test]
    async fn put_and_get_arc_share_one_allocation() {
        let cache = RenderCache::new(4, 60);
        let stored = cache.put("ssr:/".into(), "<p>page</p>".into()).await;
        let read = cache.get_arc("ssr:/").await.expect("just stored");

        assert!(
            Arc::ptr_eq(&stored.html, &read),
            "a cache hit must share the stored allocation, not copy it"
        );

        let read_again = cache.get_arc("ssr:/").await.expect("still cached");
        assert!(Arc::ptr_eq(&read, &read_again));
        assert_eq!(&*read, "<p>page</p>");
    }

    /// A disabled cache must still return what the caller asked it to store, or
    /// setting `RUVYXA_RENDER_CACHE_SIZE=0` would blank out every page.
    #[tokio::test]
    async fn a_disabled_cache_still_returns_the_value_it_was_given() {
        let cache = RenderCache::new(0, 60);
        let stored = cache.put("ssr:/".into(), "<p>page</p>".into()).await;

        assert_eq!(&*stored.html, "<p>page</p>");
        assert!(cache.get_arc("ssr:/").await.is_none());
        assert!(cache.entries.read().await.is_empty());
    }

    /// ISR reads stale entries; it must share the allocation too.
    #[tokio::test]
    async fn stale_reads_share_the_stored_allocation() {
        let cache = RenderCache::new(4, 0);
        let stored = cache.put("isr:/".into(), "<p>stale</p>".into()).await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        let (read, age) = cache
            .get_stale_with_age("isr:/")
            .await
            .expect("stale reads ignore the TTL");
        assert!(Arc::ptr_eq(&stored.html, &read.html));
        assert!(age >= Duration::from_millis(10));
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let cache = RenderCache::new(4, 60);
        cache.put("a".into(), "1".into()).await;
        cache.put("b".into(), "2".into()).await;
        assert_eq!(cache.get("a").await, Some("1".into()));
        assert_eq!(cache.get("b").await, Some("2".into()));
        assert_eq!(cache.get("c").await, None);
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let cache = RenderCache::new(3, 60);
        cache.put("a".into(), "1".into()).await;
        cache.put("b".into(), "2".into()).await;
        cache.put("c".into(), "3".into()).await;
        assert_eq!(cache.get("a").await, Some("1".into()));
        // Cache is full. `a` was just read, so `b` is now least recently used.
        cache.put("d".into(), "4".into()).await;
        assert_eq!(cache.get("a").await, Some("1".into()));
        assert_eq!(
            cache.get("b").await,
            None,
            "least recently used entry should be evicted"
        );
        assert_eq!(cache.get("c").await, Some("3".into()));
        assert_eq!(cache.get("d").await, Some("4".into()));
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn every_hit_variant_promotes_to_most_recently_used() {
        let cache = RenderCache::new(3, 60);

        cache.put("a".into(), "1".into()).await;
        cache.put("b".into(), "2".into()).await;
        cache.put("c".into(), "3".into()).await;
        assert_eq!(order_snapshot(&cache).await, vec!["a", "b", "c"]);

        assert_eq!(cache.get("a").await, Some("1".into()));
        assert_eq!(order_snapshot(&cache).await, vec!["b", "c", "a"]);

        assert!(cache.get_arc("b").await.is_some());
        assert_eq!(order_snapshot(&cache).await, vec!["c", "a", "b"]);

        assert!(cache.get_stale_with_age("c").await.is_some());
        assert_eq!(order_snapshot(&cache).await, vec!["a", "b", "c"]);

        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn test_ttl_expiry() {
        let cache = RenderCache::new(4, 0); // TTL = 0 seconds, immediate expiry
        cache.put("a".into(), "1".into()).await;
        // Small delay to ensure TTL elapses
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(cache.get("a").await, None);
        assert_index_and_order_consistent(&cache).await;
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
                .map(|(value, _)| value.html.to_string()),
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
        assert!(cache.order.read().await.is_empty());
        assert_index_and_order_consistent(&cache).await;
    }

    /// Route invalidation must take a route's `ssr:` and `client:` entries
    /// together and leave every other route alone. This is what the file
    /// watcher relies on to avoid serving a stale client bundle after an edit.
    #[tokio::test]
    async fn invalidate_route_drops_both_namespaces_for_that_route_only() {
        let cache = RenderCache::new(4, 60);
        cache.put("ssr:/a".into(), "1".into()).await;
        cache.put("ssr:/b".into(), "2".into()).await;
        cache.put("client:/a".into(), "3".into()).await;
        assert_eq!(cache.invalidate_route("/a").await, 2);
        assert_eq!(cache.get("ssr:/a").await, None);
        assert_eq!(cache.get("client:/a").await, None);
        assert_eq!(cache.get("ssr:/b").await, Some("2".into()));
        assert_index_and_order_consistent(&cache).await;
    }

    /// Every namespace a page key can carry must be reachable by invalidation.
    /// `csr:` was not, so editing a CSR page left its previous render cached
    /// until the TTL expired.
    #[tokio::test]
    async fn invalidate_route_reaches_every_render_namespace() {
        let params = RouteParams::new();
        let cache = RenderCache::new(16, 60);

        for namespace in RENDER_NAMESPACES {
            cache
                .put(page_cache_key(namespace, "/about", &params), "stale".into())
                .await;
        }
        cache
            .put(ssr_cache_key("/about", &params), "stale".into())
            .await;

        assert_eq!(
            cache.invalidate_route("/about").await,
            RENDER_NAMESPACES.len() + 1,
            "a page render must be invalidated whatever strategy produced it"
        );
        assert!(cache.entries.read().await.is_empty());
        assert_index_and_order_consistent(&cache).await;
    }

    /// The builder and the matcher have to agree by construction, not by two
    /// people remembering the same list.
    #[tokio::test]
    async fn every_namespace_key_is_matched_by_its_own_route() {
        let params = RouteParams::new();
        for namespace in RENDER_NAMESPACES.into_iter().chain([""]) {
            let key = page_cache_key(namespace, "/blog/one", &params);
            assert!(
                cache_key_matches_route(&key, "/blog/[slug]"),
                "{key} must match the route that produced it"
            );
            assert!(
                !cache_key_matches_route(&key, "/other"),
                "{key} must not match an unrelated route"
            );
        }
    }

    #[tokio::test]
    async fn test_invalidate_route_across_render_namespaces() {
        let cache = RenderCache::new(8, 60);
        cache.put("ssr:/blog/one".into(), "1".into()).await;
        cache.put("client:/blog/one".into(), "2".into()).await;
        cache.put("isr:ssr:/blog/two".into(), "3".into()).await;
        cache.put("ssr:/about".into(), "4".into()).await;

        assert_eq!(cache.invalidate_route("/blog/[slug]").await, 3);
        assert_eq!(cache.get("ssr:/about").await, Some("4".into()));
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn invalidate_route_handles_marker_text_inside_paths_and_params() {
        let cache = RenderCache::new(8, 60);
        // Catch-all URL whose captured segment contains "ssr:" as text; the
        // serialized params repeat it. Structural prefix parsing must still
        // recognize the real request path and evict the entry.
        cache
            .put(
                "ssr:/docs/ssr:evil?{\"path\":[\"ssr:evil\"]}".into(),
                "stale".into(),
            )
            .await;
        cache.put("ssr:/about".into(), "keep".into()).await;

        assert_eq!(cache.invalidate_route("/docs/[...path]").await, 1);
        assert_eq!(cache.get("ssr:/about").await, Some("keep".into()));
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn blocking_invalidation_keeps_index_and_order_in_sync() {
        let cache = Arc::new(RenderCache::new(8, 60));
        cache.put("ssr:/blog/one".into(), "1".into()).await;
        cache.put("client:/blog/one".into(), "2".into()).await;
        cache.put("ssr:/about".into(), "3".into()).await;
        cache.put("client:/about".into(), "4".into()).await;

        let worker_cache = Arc::clone(&cache);
        let removed = tokio::task::spawn_blocking(move || {
            worker_cache.invalidate_route_blocking("/blog/[slug]")
        })
        .await
        .expect("blocking invalidation task must not panic");

        // Both namespaces of the matched route go; the untouched route stays.
        assert_eq!(removed, 2);
        assert_eq!(cache.get("ssr:/about").await, Some("3".into()));
        assert_eq!(cache.get("client:/about").await, Some("4".into()));
        assert_index_and_order_consistent(&cache).await;

        let worker_cache = Arc::clone(&cache);
        let removed = tokio::task::spawn_blocking(move || worker_cache.invalidate_all_blocking())
            .await
            .expect("blocking invalidation task must not panic");
        assert_eq!(removed, 2);
        assert!(cache.order.read().await.is_empty());
        assert_index_and_order_consistent(&cache).await;
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
        assert_index_and_order_consistent(&cache).await;
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
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn replacing_a_key_keeps_lru_bookkeeping_in_sync() {
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
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn expired_entries_do_not_leave_stale_lru_slots() {
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
        assert_eq!(order_snapshot(&cache).await, vec!["c", "d"]);
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn zero_capacity_disables_cache_storage() {
        let cache = RenderCache::new(0, 60);
        cache.put("a".into(), "value".into()).await;

        assert_eq!(cache.get("a").await, None);
        assert!(cache.entries.read().await.is_empty());
        assert!(cache.order.read().await.is_empty());
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn mixed_operations_keep_index_and_order_consistent() {
        let cache = RenderCache::new(4, 60);
        for round in 0..3 {
            for key in ["ssr:/a", "ssr:/b", "client:/a", "ssr:/c", "client:/b"] {
                cache.put(key.into(), format!("{key}-{round}")).await;
                assert_index_and_order_consistent(&cache).await;
            }
            assert_eq!(
                cache.get("ssr:/b").await,
                Some(format!("ssr:/b-{round}")),
                "recently written key must stay cached"
            );
            assert_index_and_order_consistent(&cache).await;
            cache.invalidate_route("/b").await;
            assert_index_and_order_consistent(&cache).await;
        }

        cache.invalidate_route("/a").await;
        assert_index_and_order_consistent(&cache).await;
        cache.invalidate_all().await;
        assert_index_and_order_consistent(&cache).await;
        assert!(cache.entries.read().await.is_empty());
    }

    #[tokio::test]
    async fn revalidate_path_drops_every_strategy_for_that_url() {
        // The caller names a URL, not a strategy. A page moved from SSG to ISR
        // must not need the webhook that revalidates it to be updated too.
        let cache = RenderCache::new(32, 60);
        let params = RouteParams::new();
        for namespace in ["", "ssg:", "isr:", "ppr:", "csr:"] {
            cache
                .put(
                    page_cache_key(namespace, "/blog/hello", &params),
                    "old".into(),
                )
                .await;
        }
        cache
            .put(client_cache_key("/blog/hello", &params), "bundle".into())
            .await;
        cache
            .put(
                page_cache_key("ssg:", "/blog/other", &params),
                "keep".into(),
            )
            .await;

        let dropped = cache.revalidate_path("/blog/hello").await;

        assert_eq!(dropped, 6);
        assert!(
            cache
                .get(&page_cache_key("ssg:", "/blog/other", &params))
                .await
                .is_some()
        );
        assert_index_and_order_consistent(&cache).await;
    }

    #[tokio::test]
    async fn revalidate_path_drops_parameterised_variants_of_the_same_url() {
        let cache = RenderCache::new(32, 60);
        let mut params = RouteParams::new();
        params.insert("slug".into(), serde_json::json!("hello"));
        cache
            .put(page_cache_key("isr:", "/blog/hello", &params), "old".into())
            .await;

        assert_eq!(cache.revalidate_path("/blog/hello").await, 1);
    }

    #[tokio::test]
    async fn revalidate_path_does_not_match_a_longer_url_with_the_same_prefix() {
        // Prefix matching would make revalidating `/blog` drop every post.
        let cache = RenderCache::new(32, 60);
        let params = RouteParams::new();
        cache
            .put(
                page_cache_key("ssg:", "/blog/hello", &params),
                "keep".into(),
            )
            .await;

        assert_eq!(cache.revalidate_path("/blog").await, 0);
        assert!(
            cache
                .get(&page_cache_key("ssg:", "/blog/hello", &params))
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_revalidated_path_remains_forced_until_success_is_acknowledged() {
        let cache = RenderCache::new(32, 60);
        cache.revalidate_path("/blog/hello").await;

        let claim = cache.forced_claim("/blog/hello").await.unwrap();
        assert_eq!(cache.forced_claim("/blog/hello").await, Some(claim));
        cache.acknowledge_forced("/blog/hello", claim).await;
        assert_eq!(cache.forced_claim("/blog/hello").await, None);
        assert_eq!(cache.forced_claim("/blog/never-revalidated").await, None);
    }

    #[tokio::test]
    async fn an_old_success_cannot_acknowledge_a_newer_revalidation() {
        let cache = RenderCache::new(32, 60);
        cache.revalidate_path("/blog/hello").await;
        let old_claim = cache.forced_claim("/blog/hello").await.unwrap();
        cache.revalidate_path("/blog/hello").await;
        let current_claim = cache.forced_claim("/blog/hello").await.unwrap();
        assert_ne!(old_claim, current_claim);

        cache.acknowledge_forced("/blog/hello", old_claim).await;
        assert_eq!(cache.forced_claim("/blog/hello").await, Some(current_claim));
        cache.acknowledge_forced("/blog/hello", current_claim).await;
        assert_eq!(cache.forced_claim("/blog/hello").await, None);
    }

    #[tokio::test]
    async fn caching_a_fresh_render_alone_never_retires_a_prerender_claim() {
        for namespace in ["ssg:", "isr:", "ppr:", "csr:"] {
            let cache = RenderCache::new(1, 60);
            let path = format!("/{namespace}page");
            cache.revalidate_path(&path).await;
            let claim = cache.forced_claim(&path).await.unwrap();
            cache
                .put(format!("{namespace}{path}"), "fresh-in-memory".into())
                .await;
            cache.put("other".into(), "evicts-fresh".into()).await;

            // The build artifact was never replaced, so eviction must not make
            // it eligible again. The strategy will bypass disk and rerender.
            assert_eq!(cache.forced_claim(&path).await, Some(claim), "{namespace}");
        }
    }

    #[tokio::test]
    async fn updating_an_exact_path_at_capacity_does_not_fail_closed() {
        let cache = RenderCache::new(1, 60);
        for index in 0..MAX_FORCED_REVALIDATIONS {
            cache.revalidate_path(&format!("/posts/{index}")).await;
        }
        let old_claim = cache.forced_claim("/posts/0").await.unwrap();

        cache.revalidate_path("/posts/0").await;

        let new_claim = cache.forced_claim("/posts/0").await.unwrap();
        assert_ne!(old_claim, new_claim);
        assert!(!cache.forced.read().await.bypass_prerendered);
        assert_eq!(
            cache.forced.read().await.paths.len(),
            MAX_FORCED_REVALIDATIONS
        );
    }

    /// The approach to the limit must be reported while it can still be acted
    /// on, and reported once. The overflow warning arrives after every
    /// prerendered artifact is already bypassed, which is too late to be a
    /// signal and too late to prevent.
    #[tokio::test]
    async fn approaching_the_claim_limit_warns_once_before_the_overflow() {
        let cache = RenderCache::new(32, 60);
        let mut outcomes = Vec::new();
        for index in 0..MAX_FORCED_REVALIDATIONS {
            outcomes.push(cache.forced.write().await.mark(&format!("/posts/{index}")));
        }

        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == MarkOutcome::HighWater)
                .count(),
            1,
            "the high-water mark must be reported exactly once"
        );
        assert_eq!(
            outcomes[FORCED_REVALIDATION_HIGH_WATER - 1],
            MarkOutcome::HighWater,
            "the warning must land on the claim that crosses the mark"
        );
        // The next distinct path overflows the bounded set.
        assert_eq!(
            cache.forced.write().await.mark("/posts/overflow"),
            MarkOutcome::FailedClosed
        );
    }

    /// Pending claims and the global bypass have to be readable from outside
    /// the cache. A host that cannot replace its prerendered documents keeps
    /// accumulating claims until every artifact is bypassed process-wide, and
    /// before this the only trace of that was one log line and unexplained
    /// render load.
    #[tokio::test]
    async fn the_snapshot_reports_pending_claims_and_the_global_bypass() {
        let cache = RenderCache::new(32, 60);
        assert_eq!(cache.snapshot().await.forced_pending, 0);
        assert!(!cache.snapshot().await.bypass_prerendered);

        cache.revalidate_path("/blog/hello").await;
        let claim = cache.forced_claim("/blog/hello").await.unwrap();
        assert_eq!(cache.snapshot().await.forced_pending, 1);

        cache.acknowledge_forced("/blog/hello", claim).await;
        assert_eq!(
            cache.snapshot().await.forced_pending,
            0,
            "an acknowledged claim must stop being reported as pending"
        );

        cache.revalidate_all_paths().await;
        let snapshot = cache.snapshot().await;
        assert!(snapshot.bypass_prerendered);
        assert_eq!(snapshot.forced_pending, 0);
    }

    #[tokio::test]
    async fn revalidating_an_unrendered_path_still_forces_it() {
        // A webhook may name a URL nobody has requested yet. Dropping nothing
        // is correct; leaving the build's HTML in place for it is not.
        let cache = RenderCache::new(32, 60);
        assert_eq!(cache.revalidate_path("/blog/brand-new").await, 0);
        assert!(cache.forced_claim("/blog/brand-new").await.is_some());
    }

    #[tokio::test]
    async fn excessive_pending_revalidations_fail_closed_with_bounded_memory() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/revalidation-conformance.json"
        ))
        .unwrap();
        assert_eq!(
            MAX_FORCED_REVALIDATIONS as u64,
            contract["maxPendingExactPaths"].as_u64().unwrap()
        );
        let cache = RenderCache::new(32, 60);
        for index in 0..=MAX_FORCED_REVALIDATIONS {
            cache.revalidate_path(&format!("/posts/{index}")).await;
        }

        let forced = cache.forced.read().await;
        assert!(forced.bypass_prerendered);
        assert!(forced.paths.is_empty());
        drop(forced);

        // Overflow must never discard an invalidation silently. Failing closed
        // also protects paths named after the exact set reached its limit.
        assert!(cache.forced_claim("/posts/0").await.is_some());
        assert!(cache.forced_claim("/posts/after-overflow").await.is_some());
    }

    #[tokio::test]
    async fn oversized_protocol_payload_can_fail_closed_in_constant_state() {
        let cache = RenderCache::new(32, 60);
        cache.put("ssg:/one".into(), "one".into()).await;
        cache.put("isr:/two".into(), "two".into()).await;

        assert_eq!(cache.revalidate_all_paths().await, 2);
        assert!(cache.entries.read().await.is_empty());
        assert!(cache.order.read().await.is_empty());
        let forced = cache.forced.read().await;
        assert!(forced.bypass_prerendered);
        assert!(forced.paths.is_empty());
    }

    #[test]
    fn environment_cache_capacity_is_bounded_without_removing_the_disable_setting() {
        assert_eq!(
            render_cache_capacity(None, DEFAULT_CAPACITY),
            DEFAULT_CAPACITY
        );
        assert_eq!(
            render_cache_capacity(Some("not-a-number"), DEFAULT_CAPACITY),
            DEFAULT_CAPACITY
        );
        assert_eq!(render_cache_capacity(Some("0"), DEFAULT_CAPACITY), 0);
        assert_eq!(
            render_cache_capacity(Some("999999999"), DEFAULT_CAPACITY),
            MAX_ENV_RENDER_CACHE_CAPACITY
        );
    }
}
