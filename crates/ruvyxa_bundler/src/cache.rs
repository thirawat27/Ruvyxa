//! Incremental build cache: content-addressed compilation artifacts.
//!
//! Compiled JS output is stored under `.ruvyxa/cache/bundler/<hash>.js` keyed
//! by the blake3 hash of the source text.  On subsequent builds, if the source
//! hash matches a cached entry, the compilation step is skipped entirely.
//!
//! ## Cache key
//!
//! ```text
//! blake3(source_content || has_jsx_flag || compiler_version)
//! ```
//!
//! The compiler version is included so that cache entries are automatically
//! invalidated when the compiler is updated.

use std::fs;
use std::path::{Path, PathBuf};

/// Current compiler version stamp — bump this when the transform logic changes
/// to automatically invalidate stale cache entries.
const COMPILER_VERSION: &str = "ruvyxa_bundler:0.1.0";

/// On-disk compilation cache.
#[derive(Debug, Clone)]
pub struct CompileCache {
    /// Directory where cached `.js` files are stored.
    cache_dir: PathBuf,
    /// Whether caching is enabled.
    enabled: bool,
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
        Self { cache_dir, enabled }
    }

    /// Create a disabled (no-op) cache.
    pub fn disabled() -> Self {
        Self {
            cache_dir: PathBuf::new(),
            enabled: false,
        }
    }

    /// Look up a source file's compiled output in the cache.
    ///
    /// Returns [`CacheLookup::Hit`] with the cached JS if found,
    /// or [`CacheLookup::Miss`] with the computed cache key otherwise.
    pub fn lookup(&self, source: &str, has_jsx: bool) -> CacheLookup {
        let key = Self::cache_key(source, has_jsx);

        if !self.enabled {
            return CacheLookup::Miss(key);
        }

        let path = self.cache_dir.join(format!("{key}.js"));
        match fs::read_to_string(&path) {
            Ok(cached_js) => CacheLookup::Hit(cached_js),
            Err(_) => CacheLookup::Miss(key),
        }
    }

    /// Store compiled JS output in the cache under the given key.
    ///
    /// Silently ignores write failures (cache is best-effort).
    pub fn store(&self, key: &str, compiled_js: &str) {
        if !self.enabled {
            return;
        }

        if let Err(_e) = fs::create_dir_all(&self.cache_dir) {
            return;
        }

        let path = self.cache_dir.join(format!("{key}.js"));
        let _ = fs::write(&path, compiled_js.as_bytes());
    }

    /// Invalidate (remove) a specific cache entry.
    pub fn invalidate(&self, source: &str, has_jsx: bool) {
        if !self.enabled {
            return;
        }
        let key = Self::cache_key(source, has_jsx);
        let path = self.cache_dir.join(format!("{key}.js"));
        let _ = fs::remove_file(&path);
    }

    /// Remove all cached entries.
    pub fn clear(&self) {
        if !self.enabled {
            return;
        }
        if self.cache_dir.exists() {
            let _ = fs::remove_dir_all(&self.cache_dir);
        }
    }

    /// Compute the cache key for a given source and JSX flag.
    ///
    /// Key = blake3(source || "\0" || jsx_flag || "\0" || compiler_version)[..32] as hex.
    pub fn cache_key(source: &str, has_jsx: bool) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(source.as_bytes());
        hasher.update(b"\0");
        hasher.update(if has_jsx { b"jsx" } else { b"ts" });
        hasher.update(b"\0");
        hasher.update(COMPILER_VERSION.as_bytes());
        let hash = hasher.finalize();
        hash.to_hex()[..32].to_string()
    }

    /// Return the cache directory path for diagnostics/reporting.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Return whether the cache is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
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

        // Initially miss.
        let key = match cache.lookup(source, has_jsx) {
            CacheLookup::Miss(k) => k,
            CacheLookup::Hit(_) => panic!("should be miss initially"),
        };

        // Store.
        cache.store(&key, compiled);

        // Now should hit.
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

        // Confirm hit.
        assert!(matches!(cache.lookup(source, false), CacheLookup::Hit(_)));

        // Invalidate.
        cache.invalidate(source, false);

        // Now miss.
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
}
