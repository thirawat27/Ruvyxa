//! Incremental build cache: content-addressed compilation artifacts.
//!
//! Compiled JS output is stored under `.ruvyxa/cache/bundler/<hash>.js` keyed
//! by the blake3 hash of the source text.  On subsequent builds, if the source
//! hash matches a cached entry, the compilation step is skipped entirely.
//!
//! ## Cache key
//!
//! ```text
//! blake3(source_content || "\0" || jsx_flag || "\0" || jsx_runtime || "\0" || compiler_version || "\0" || namespace)
//! ```
//!
//! The compiler version is included so that cache entries are automatically
//! invalidated when the compiler is updated.
//!
//! ## Memory cache (LRU eviction)
//!
//! The in-process cache holds up to [`MEMORY_CACHE_LIMIT`] entries.  When the
//! limit is reached the least-recently-used entry is evicted to bound memory
//! consumption.  Disk entries are not evicted automatically — run
//! `ruvyxa clean` or call [`CompileCache::clear`] to purge them.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::JsxRuntime;

/// Maximum number of entries kept in the in-process memory cache.
const MEMORY_CACHE_LIMIT: usize = 512;

/// Current compiler version stamp — bump this when the transform logic changes
/// to automatically invalidate stale cache entries.
const COMPILER_VERSION: &str = concat!(
    "ruvyxa_bundler:",
    env!("CARGO_PKG_VERSION"),
    ":ast-build-hooks"
);

/// LRU-ordered in-memory cache entry.
#[derive(Debug)]
struct MemEntry {
    value: String,
    /// Monotonically increasing generation counter for LRU tracking. Doubles as
    /// this entry's slot in [`MemoryCache::recency`].
    last_used: u64,
}

/// Atomic generation counter for LRU tracking.
///
/// Global rather than per-cache, so generations are unique across every
/// [`CompileCache`] instance in the process and can be used directly as
/// recency-index keys without collision.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

/// Bounded in-process LRU over compiled module output.
///
/// The recency index exists so eviction does not have to scan. Finding the
/// least-recently-used entry used to be a `min_by_key` pass over every entry,
/// which runs once per insert for the whole remainder of a build once the cache
/// is full — and it diverged from the O(1) recency list `RenderCache` already
/// uses in this workspace for the same job. A `BTreeMap` keyed by generation
/// gives the same eviction order in O(log n) with far less code than a
/// hand-rolled linked list.
#[derive(Debug, Default)]
struct MemoryCache {
    entries: HashMap<String, MemEntry>,
    /// `last_used` generation -> cache key. Exactly one slot per entry.
    recency: BTreeMap<u64, String>,
    resident_bytes: u64,
}

impl MemoryCache {
    fn len(&self) -> usize {
        self.entries.len()
    }

    /// Read an entry and mark it most-recently-used.
    fn touch(&mut self, key: &str) -> Option<&str> {
        let entry = self.entries.get_mut(key)?;
        self.recency.remove(&entry.last_used);
        entry.last_used = next_generation();
        self.recency.insert(entry.last_used, key.to_string());
        Some(&entry.value)
    }

    /// Insert `value`, evicting the least-recently-used entry first when the
    /// cache is at its limit and this is a new key.
    fn insert(&mut self, key: String, value: String, limit: usize) {
        if self.entries.len() >= limit && !self.entries.contains_key(&key) {
            self.evict_lru();
        }
        let last_used = next_generation();
        let entry_bytes = cache_entry_bytes(&key, &value);
        if let Some(previous) = self
            .entries
            .insert(key.clone(), MemEntry { value, last_used })
        {
            self.recency.remove(&previous.last_used);
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(cache_entry_bytes(&key, &previous.value));
        }
        self.resident_bytes = self.resident_bytes.saturating_add(entry_bytes);
        self.recency.insert(last_used, key);
    }

    fn evict_lru(&mut self) -> bool {
        if let Some((_, lru_key)) = self.recency.pop_first()
            && let Some(entry) = self.entries.remove(&lru_key)
        {
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(cache_entry_bytes(&lru_key, &entry.value));
            return true;
        }
        false
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.recency.remove(&entry.last_used);
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(cache_entry_bytes(key, &entry.value));
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.recency.clear();
        self.resident_bytes = 0;
    }

    fn evict_until_bytes(&mut self, target_bytes: u64) -> u64 {
        let mut evicted = 0_u64;
        while self.resident_bytes > target_bytes && self.evict_lru() {
            evicted = evicted.saturating_add(1);
        }
        evicted
    }
}

fn cache_entry_bytes(key: &str, value: &str) -> u64 {
    (key.len() as u64).saturating_add(value.len() as u64)
}

#[derive(Debug, Default)]
struct CacheTelemetry {
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileCacheStats {
    pub memory_entries: usize,
    pub resident_bytes: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

/// On-disk compilation cache with an in-process LRU memory layer.
#[derive(Debug, Clone)]
pub struct CompileCache {
    /// Directory where cached `.js` files are stored.
    cache_dir: PathBuf,
    /// Whether caching is enabled.
    enabled: bool,
    /// Build-input namespace used to invalidate artifacts when config or build-hook
    /// dependencies change without changing a source module.
    namespace: String,
    /// Process-local hot cache shared by cloned cache handles.
    memory: Arc<Mutex<MemoryCache>>,
    telemetry: Arc<CacheTelemetry>,
}

/// Cache lookup result.
#[derive(Debug)]
pub enum CacheLookup {
    /// Cache hit: contains the compiled JS source.
    Hit(String),
    /// Cache miss: contains the cache key for later storage.
    Miss(String),
}

impl CompileCache {
    /// Create a cache instance rooted at `project_root/.ruvyxa/cache/bundler/`.
    pub fn new(project_root: &Path, enabled: bool) -> Self {
        let cache_dir = project_root.join(".ruvyxa").join("cache").join("bundler");
        Self::at_dir(cache_dir, enabled)
    }

    /// Create a cache rooted at a caller-selected directory.
    ///
    /// This supports shared filesystems mounted by CI or developer machines.
    pub fn at_dir(cache_dir: impl Into<PathBuf>, enabled: bool) -> Self {
        Self::at_dir_with_namespace(cache_dir, enabled, "")
    }

    /// Create a cache at a caller-selected directory with an invalidation namespace.
    pub fn at_dir_with_namespace(
        cache_dir: impl Into<PathBuf>,
        enabled: bool,
        namespace: impl Into<String>,
    ) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            enabled,
            namespace: namespace.into(),
            memory: Arc::new(Mutex::new(MemoryCache::default())),
            telemetry: Arc::new(CacheTelemetry::default()),
        }
    }

    /// Create a disabled (no-op) cache.
    pub fn disabled() -> Self {
        Self {
            cache_dir: PathBuf::new(),
            enabled: false,
            namespace: String::new(),
            memory: Arc::new(Mutex::new(MemoryCache::default())),
            telemetry: Arc::new(CacheTelemetry::default()),
        }
    }

    /// Look up a source file's compiled output in the cache (classic JSX mode).
    pub fn lookup(&self, source: &str, has_jsx: bool) -> CacheLookup {
        self.lookup_with_options(source, has_jsx, JsxRuntime::Classic)
    }

    /// Look up with explicit JSX runtime selection.
    ///
    /// Returns [`CacheLookup::Hit`] with the cached JS if found,
    /// or [`CacheLookup::Miss`] with the computed cache key otherwise.
    pub fn lookup_with_options(
        &self,
        source: &str,
        has_jsx: bool,
        jsx_runtime: JsxRuntime,
    ) -> CacheLookup {
        let key = Self::cache_key_with_options_and_namespace(
            source,
            has_jsx,
            jsx_runtime,
            &self.namespace,
        );

        if !self.enabled {
            self.telemetry.misses.fetch_add(1, Ordering::Relaxed);
            return CacheLookup::Miss(key);
        }

        // Fast path: memory cache (LRU-updated on hit).
        if let Ok(mut memory) = self.memory.lock()
            && let Some(value) = memory.touch(&key)
        {
            self.telemetry.hits.fetch_add(1, Ordering::Relaxed);
            return CacheLookup::Hit(value.to_string());
        }

        // Disk cache.
        let path = self.cache_dir.join(format!("{key}.js"));
        match fs::read_to_string(&path) {
            Ok(cached_js) => {
                self.insert_to_memory(key, cached_js.clone());
                self.telemetry.hits.fetch_add(1, Ordering::Relaxed);
                CacheLookup::Hit(cached_js)
            }
            Err(_) => {
                self.telemetry.misses.fetch_add(1, Ordering::Relaxed);
                CacheLookup::Miss(key)
            }
        }
    }

    /// Store compiled JS output in the cache under the given key.
    ///
    /// Silently ignores write failures (cache is best-effort).
    pub fn store(&self, key: &str, compiled_js: &str) {
        if !self.enabled {
            return;
        }

        self.insert_to_memory(key.to_string(), compiled_js.to_string());

        if let Err(_e) = fs::create_dir_all(&self.cache_dir) {
            return;
        }

        // A cache miss costs a recompile, never a wrong answer, so a failed
        // publish is dropped rather than propagated.
        let path = self.cache_dir.join(format!("{key}.js"));
        let _ = crate::atomic_file::write_atomic(&path, compiled_js.as_bytes());
    }

    /// Insert a value into the in-memory LRU cache, evicting the LRU entry
    /// when the cache has grown past [`MEMORY_CACHE_LIMIT`].
    fn insert_to_memory(&self, key: String, value: String) {
        if let Ok(mut memory) = self.memory.lock() {
            memory.insert(key, value, MEMORY_CACHE_LIMIT);
        }
    }

    /// Invalidate (remove) a specific cache entry.
    ///
    /// Keys on `JsxRuntime::Classic`, matching its [`Self::lookup`] and
    /// [`Self::cache_key`] siblings. The compile path keys on the project's
    /// real runtime through [`Self::lookup_with_options`], and the default is
    /// `Automatic` — so wiring this into a watcher or a build step would
    /// compute a key nothing was ever stored under and remove nothing, quietly.
    /// Use [`Self::cache_key_with_options_and_namespace`] and delete the file
    /// directly if a production caller ever needs this.
    pub fn invalidate(&self, source: &str, has_jsx: bool) {
        if !self.enabled {
            return;
        }
        let key = Self::cache_key_with_options_and_namespace(
            source,
            has_jsx,
            JsxRuntime::Classic,
            &self.namespace,
        );
        if let Ok(mut memory) = self.memory.lock() {
            memory.remove(&key);
        }
        let path = self.cache_dir.join(format!("{key}.js"));
        let _ = fs::remove_file(&path);
    }

    /// Remove all cached entries (memory + disk).
    pub fn clear(&self) {
        if !self.enabled {
            return;
        }
        if let Ok(mut memory) = self.memory.lock() {
            memory.clear();
        }
        if self.cache_dir.exists() {
            let _ = fs::remove_dir_all(&self.cache_dir);
        }
    }

    /// Compute the cache key for a given source and JSX flag (classic mode).
    pub fn cache_key(source: &str, has_jsx: bool) -> String {
        Self::cache_key_with_options(source, has_jsx, JsxRuntime::Classic)
    }

    /// Compute the cache key for a given source, JSX flag, and JSX runtime mode.
    ///
    /// Key = blake3(source || "\0" || jsx_flag || "\0" || jsx_runtime || "\0" || compiler_version)[..32] as hex.
    pub fn cache_key_with_options(source: &str, has_jsx: bool, jsx_runtime: JsxRuntime) -> String {
        Self::cache_key_with_options_and_namespace(source, has_jsx, jsx_runtime, "")
    }

    /// Compute a cache key with an additional build-input namespace.
    pub fn cache_key_with_options_and_namespace(
        source: &str,
        has_jsx: bool,
        jsx_runtime: JsxRuntime,
        namespace: &str,
    ) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(source.as_bytes());
        hasher.update(b"\0");
        hasher.update(if has_jsx { b"jsx" } else { b"ts" });
        hasher.update(b"\0");
        hasher.update(match jsx_runtime {
            JsxRuntime::Classic => b"classic",
            JsxRuntime::Automatic => b"automatic",
        });
        hasher.update(b"\0");
        hasher.update(COMPILER_VERSION.as_bytes());
        hasher.update(b"\0");
        hasher.update(namespace.as_bytes());
        let hash = hasher.finalize();
        hash.to_hex()[..32].to_string()
    }

    /// Return the cache directory path for diagnostics/reporting.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Count the number of cached entries on disk.
    pub fn entry_count(&self) -> usize {
        if !self.enabled || !self.cache_dir.exists() {
            return 0;
        }
        fs::read_dir(&self.cache_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map(|ext| ext == "js").unwrap_or(false))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Total size of all cached files in bytes.
    pub fn total_bytes(&self) -> u64 {
        if !self.enabled || !self.cache_dir.exists() {
            return 0;
        }
        fs::read_dir(&self.cache_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Whether `key` is currently resident in the memory layer.
    ///
    /// Test-only: [`Self::lookup`] cannot answer this because a memory miss
    /// falls through to disk and repopulates the entry, which is exactly the
    /// eviction the caller wants to observe.
    #[cfg(test)]
    fn memory_contains(&self, key: &str) -> bool {
        self.memory
            .lock()
            .map(|m| m.entries.contains_key(key))
            .unwrap_or(false)
    }

    /// Return the current number of entries in the memory cache.
    pub fn memory_entry_count(&self) -> usize {
        self.memory.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// Approximate resident bytes owned by the in-process cache layer.
    pub fn memory_resident_bytes(&self) -> u64 {
        self.memory
            .lock()
            .map(|memory| memory.resident_bytes)
            .unwrap_or(0)
    }

    /// Evict least-recently-used memory entries until `target_bytes` is met.
    /// Disk artifacts remain available, so this can only make a later lookup slower.
    pub fn evict_memory_to(&self, target_bytes: u64) -> u64 {
        let evicted = self
            .memory
            .lock()
            .map(|mut memory| memory.evict_until_bytes(target_bytes))
            .unwrap_or(0);
        self.telemetry
            .evictions
            .fetch_add(evicted, Ordering::Relaxed);
        evicted
    }

    pub fn stats(&self) -> CompileCacheStats {
        CompileCacheStats {
            memory_entries: self.memory_entry_count(),
            resident_bytes: self.memory_resident_bytes(),
            hits: self.telemetry.hits.load(Ordering::Relaxed),
            misses: self.telemetry.misses.load(Ordering::Relaxed),
            evictions: self.telemetry.evictions.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_deterministic() {
        let k1 = CompileCache::cache_key("const x = 1;", false);
        let k2 = CompileCache::cache_key("const x = 1;", false);
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 32);
    }

    #[test]
    fn cache_key_differs_by_content() {
        let k1 = CompileCache::cache_key("const x = 1;", false);
        let k2 = CompileCache::cache_key("const x = 2;", false);
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_differs_by_jsx_flag() {
        let k1 = CompileCache::cache_key("const x = 1;", false);
        let k2 = CompileCache::cache_key("const x = 1;", true);
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_differs_by_jsx_runtime() {
        let k1 = CompileCache::cache_key_with_options("const x = 1;", true, JsxRuntime::Classic);
        let k2 = CompileCache::cache_key_with_options("const x = 1;", true, JsxRuntime::Automatic);
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_namespace_invalidates_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let first = CompileCache::at_dir_with_namespace(tmp.path(), true, "config-a");
        let second = CompileCache::at_dir_with_namespace(tmp.path(), true, "config-b");
        let source = "const x: number = 1;";

        let key = match first.lookup(source, false) {
            CacheLookup::Miss(key) => key,
            CacheLookup::Hit(_) => panic!("new namespace should initially miss"),
        };
        first.store(&key, "const x = 1;");

        assert!(matches!(first.lookup(source, false), CacheLookup::Hit(_)));
        assert!(matches!(second.lookup(source, false), CacheLookup::Miss(_)));
    }

    #[test]
    fn shared_directory_reuses_artifacts_across_project_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("network-cache");
        let first = CompileCache::at_dir_with_namespace(&shared, true, "same-config");
        let second = CompileCache::at_dir_with_namespace(&shared, true, "same-config");
        let source = "const shared: string = 'cache';";

        let key = match first.lookup(source, false) {
            CacheLookup::Miss(key) => key,
            CacheLookup::Hit(_) => panic!("shared cache should initially miss"),
        };
        first.store(&key, "const shared = 'cache';");

        assert!(matches!(second.lookup(source, false), CacheLookup::Hit(_)));
    }

    #[test]
    fn disabled_cache_always_misses() {
        let cache = CompileCache::disabled();
        match cache.lookup("const x = 1;", false) {
            CacheLookup::Miss(_) => {}
            CacheLookup::Hit(_) => panic!("disabled cache should not hit"),
        }
    }

    #[test]
    fn store_and_retrieve() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::new(tmp.path(), true);

        let source = "const x = 1;";
        let has_jsx = false;
        let compiled = "var x = 1;";

        let key = match cache.lookup(source, has_jsx) {
            CacheLookup::Miss(k) => k,
            CacheLookup::Hit(_) => panic!("should be miss initially"),
        };

        cache.store(&key, compiled);

        match cache.lookup(source, has_jsx) {
            CacheLookup::Hit(cached) => assert_eq!(cached, compiled),
            CacheLookup::Miss(_) => panic!("should be hit after store"),
        }
    }

    #[test]
    fn invalidate_removes_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::new(tmp.path(), true);

        let source = "const y = 2;";
        let compiled = "var y = 2;";
        let key = CompileCache::cache_key(source, false);
        cache.store(&key, compiled);

        assert!(matches!(cache.lookup(source, false), CacheLookup::Hit(_)));

        cache.invalidate(source, false);

        assert!(matches!(cache.lookup(source, false), CacheLookup::Miss(_)));
    }

    #[test]
    fn clear_removes_all() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::new(tmp.path(), true);

        cache.store(&CompileCache::cache_key("a", false), "compiled_a");
        cache.store(&CompileCache::cache_key("b", false), "compiled_b");
        assert_eq!(cache.entry_count(), 2);

        cache.clear();
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn entry_count_and_total_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::new(tmp.path(), true);

        cache.store(&CompileCache::cache_key("x", false), "12345");
        cache.store(&CompileCache::cache_key("y", true), "67890ab");

        assert_eq!(cache.entry_count(), 2);
        assert_eq!(cache.total_bytes(), 12); // 5 + 7 bytes
    }

    #[test]
    fn lru_eviction_bounds_memory_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::new(tmp.path(), true);

        // Fill the memory cache to the limit.
        for i in 0..MEMORY_CACHE_LIMIT {
            let src = format!("const x{i} = {i};");
            let key = CompileCache::cache_key(&src, false);
            cache.store(&key, &format!("var x{i} = {i};"));
        }

        assert_eq!(cache.memory_entry_count(), MEMORY_CACHE_LIMIT);

        // Add one more — should evict an existing entry.
        let extra_src = "const extra = 999;";
        let key = CompileCache::cache_key(extra_src, false);
        cache.store(&key, "var extra = 999;");

        assert_eq!(
            cache.memory_entry_count(),
            MEMORY_CACHE_LIMIT,
            "memory cache should not grow past the limit"
        );
    }

    /// Eviction must follow read recency, not insertion order. The bound test
    /// above passes for any eviction policy, including one that drops the
    /// most-recently-used entry, so the ordering needs its own guard.
    #[test]
    fn lru_eviction_drops_the_least_recently_used_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::new(tmp.path(), true);

        let sources: Vec<String> = (0..MEMORY_CACHE_LIMIT)
            .map(|i| format!("const x{i} = {i};"))
            .collect();
        let keys: Vec<String> = sources
            .iter()
            .map(|src| CompileCache::cache_key(src, false))
            .collect();
        for (i, key) in keys.iter().enumerate() {
            cache.store(key, &format!("var x{i} = {i};"));
        }

        // Read the oldest entry so it becomes the most recently used, leaving
        // entry 1 as the least recently used.
        assert!(matches!(
            cache.lookup(&sources[0], false),
            CacheLookup::Hit(_)
        ));

        let extra = "const extra = 999;";
        cache.store(&CompileCache::cache_key(extra, false), "var extra = 999;");

        assert_eq!(cache.memory_entry_count(), MEMORY_CACHE_LIMIT);
        assert!(
            cache.memory_contains(&keys[0]),
            "the entry read most recently must survive eviction"
        );
        assert!(
            !cache.memory_contains(&keys[1]),
            "the least recently used entry must be the one evicted"
        );
    }

    /// Re-storing an existing key must replace its recency slot rather than
    /// leaving a stale one behind, or the index and the entry map drift apart
    /// and eviction starts removing keys that are no longer resident.
    #[test]
    fn restoring_an_existing_key_keeps_the_recency_index_consistent() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::new(tmp.path(), true);

        let source = "const a = 1;";
        let key = CompileCache::cache_key(source, false);
        for _ in 0..5 {
            cache.store(&key, "var a = 1;");
        }

        assert_eq!(cache.memory_entry_count(), 1);
        let recency_len = cache.memory.lock().map(|m| m.recency.len()).unwrap();
        assert_eq!(
            recency_len, 1,
            "one resident entry must hold exactly one recency slot"
        );
    }

    #[test]
    fn byte_budget_evicts_lru_entries_without_removing_disk_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::at_dir(tmp.path(), true);
        let first_source = "export const first = 'a';";
        let second_source = "export const second = 'bbbbbbbb';";
        let first_key = match cache.lookup(first_source, false) {
            CacheLookup::Miss(key) => key,
            CacheLookup::Hit(_) => unreachable!(),
        };
        cache.store(&first_key, "compiled-first");
        let second_key = match cache.lookup(second_source, false) {
            CacheLookup::Miss(key) => key,
            CacheLookup::Hit(_) => unreachable!(),
        };
        cache.store(&second_key, "compiled-second");
        assert!(cache.memory_resident_bytes() > 0);

        let evicted = cache.evict_memory_to(0);

        assert_eq!(evicted, 2);
        assert_eq!(cache.memory_resident_bytes(), 0);
        assert_eq!(cache.stats().evictions, 2);
        assert!(matches!(
            cache.lookup(first_source, false),
            CacheLookup::Hit(_)
        ));
    }

    #[test]
    fn store_and_retrieve_automatic_jsx() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = CompileCache::new(tmp.path(), true);

        let source = "const el = <div/>;";
        let compiled_auto = "_jsx(\"div\", {})";

        let key = CompileCache::cache_key_with_options(source, true, JsxRuntime::Automatic);
        cache.store(&key, compiled_auto);

        // Automatic mode should hit.
        match cache.lookup_with_options(source, true, JsxRuntime::Automatic) {
            CacheLookup::Hit(v) => assert_eq!(v, compiled_auto),
            CacheLookup::Miss(_) => panic!("should be hit for automatic mode"),
        }

        // Classic mode should miss (different cache key).
        assert!(matches!(
            cache.lookup_with_options(source, true, JsxRuntime::Classic),
            CacheLookup::Miss(_)
        ));
    }
}
