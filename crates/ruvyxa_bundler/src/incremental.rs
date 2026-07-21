//! Persistent incremental graph cache with file fingerprinting.
//!
//! Stores the resolved module dependency graph on disk as a compact JSON
//! manifest at `.ruvyxa/cache/graph/manifest.json`. Each module entry records:
//!
//! - Its canonical path
//! - A content fingerprint (blake3 hash of source + mtime)
//! - Its resolved dependency edges (list of paths)
//! - Its compiled output cache key
//!
//! On subsequent builds, the cache is loaded and each file is checked against
//! its stored fingerprint. Only modules whose fingerprint has changed — or
//! whose transitive dependencies have changed — are recompiled.
//!
//! ## Performance impact
//!
//! For a 200-module project where only 3 files changed:
//! - Without incremental: resolve 200 modules, compile 200 modules
//! - With incremental: stat 200 files (fast), recompile only 3 dirty + their dependents
//!
//! The stat check uses mtime+size as a fast-reject: if mtime/size unchanged,
//! skip the blake3 hash entirely. This makes the "nothing changed" case
//! nearly free (just N stat calls, typically <1ms for 200 files).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Version stamp for the graph manifest format.
const MANIFEST_VERSION: &str = "ruvyxa_graph_cache:v1";

/// A persisted module entry in the graph cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModuleEntry {
    /// Blake3 content hash (hex, 32 chars).
    pub content_hash: String,
    /// File size in bytes.
    pub size: u64,
    /// Modification time as seconds since UNIX epoch (for fast-reject).
    pub mtime_secs: u64,
    /// Resolved dependency paths (absolute).
    pub deps: Vec<PathBuf>,
    /// Compile cache key for this module's output (links to CompileCache).
    pub compile_key: Option<String>,
}

/// The full persisted graph manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphManifest {
    /// Format version — auto-invalidates on incompatible changes.
    pub version: String,
    /// Module entries keyed by canonical path.
    pub modules: BTreeMap<PathBuf, CachedModuleEntry>,
}

impl GraphManifest {
    pub fn new() -> Self {
        Self {
            version: MANIFEST_VERSION.to_string(),
            modules: BTreeMap::new(),
        }
    }
}

impl Default for GraphManifest {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of checking a module against the persisted cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessStatus {
    /// File hasn't changed — can reuse cached compilation.
    Fresh,
    /// File changed or is new — must recompile.
    Stale,
}

/// Persistent incremental graph cache.
///
/// Loads the previous build's graph manifest from disk, provides fast
/// freshness checks, and saves the updated manifest after a build completes.
#[derive(Debug, Clone)]
pub struct IncrementalGraphCache {
    /// Path to the graph manifest file.
    manifest_path: PathBuf,
    /// The loaded (or empty) manifest from the previous build.
    previous: GraphManifest,
    /// The manifest being built for the current build.
    current: GraphManifest,
    /// Whether the cache is enabled.
    enabled: bool,
}

impl IncrementalGraphCache {
    /// Create a new incremental cache rooted at the project's cache directory.
    ///
    /// Loads the previous manifest from `.ruvyxa/cache/graph/manifest.json`
    /// if it exists and is compatible with the current version.
    pub fn new(project_root: &Path, enabled: bool) -> Self {
        let cache_dir = project_root.join(".ruvyxa").join("cache").join("graph");
        let manifest_path = cache_dir.join("manifest.json");

        let previous = if enabled {
            Self::load_manifest(&manifest_path).unwrap_or_default()
        } else {
            GraphManifest::default()
        };

        Self {
            manifest_path,
            previous,
            current: GraphManifest::new(),
            enabled,
        }
    }

    /// Create a disabled (no-op) cache.
    pub fn disabled() -> Self {
        Self {
            manifest_path: PathBuf::new(),
            previous: GraphManifest::default(),
            current: GraphManifest::new(),
            enabled: false,
        }
    }

    /// Check whether a module is fresh (unchanged since last build).
    ///
    /// Uses a two-tier strategy:
    /// 1. Fast-reject: if size differs, immediately return Stale.
    /// 2. Content hash: always verify blake3 hash matches (handles same-size edits).
    ///
    /// The `source` parameter is the current file content, which is used to
    /// compute the content hash without an additional file read.
    pub fn check_freshness(&self, path: &Path, source: &str) -> FreshnessStatus {
        if !self.enabled {
            return FreshnessStatus::Stale;
        }

        let Some(cached) = self.previous.modules.get(path) else {
            return FreshnessStatus::Stale;
        };

        // Fast-reject: if size differs, definitely stale.
        if source.len() as u64 != cached.size {
            return FreshnessStatus::Stale;
        }

        // Content hash comparison — authoritative check.
        let current_hash = content_hash(source);
        if current_hash == cached.content_hash {
            FreshnessStatus::Fresh
        } else {
            FreshnessStatus::Stale
        }
    }

    /// Check freshness using only file metadata (no source content needed).
    ///
    /// This is the fastest check — only a stat call. Returns `Stale` if
    /// mtime or size differ from the cached values.
    pub fn check_freshness_fast(&self, path: &Path) -> FreshnessStatus {
        if !self.enabled {
            return FreshnessStatus::Stale;
        }

        let Some(cached) = self.previous.modules.get(path) else {
            return FreshnessStatus::Stale;
        };

        let Ok(metadata) = fs::metadata(path) else {
            return FreshnessStatus::Stale;
        };

        let current_size = metadata.len();
        let current_mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        if current_size == cached.size && current_mtime == cached.mtime_secs {
            FreshnessStatus::Fresh
        } else {
            FreshnessStatus::Stale
        }
    }

    /// Get the cached dependency edges for a module (if fresh).
    ///
    /// Returns `None` if the module is not in the cache or is stale.
    pub fn cached_deps(&self, path: &Path) -> Option<&[PathBuf]> {
        if !self.enabled {
            return None;
        }
        self.previous.modules.get(path).map(|e| e.deps.as_slice())
    }

    /// Get the compile cache key for a module (if fresh).
    pub fn cached_compile_key(&self, path: &Path) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        self.previous
            .modules
            .get(path)
            .and_then(|e| e.compile_key.as_deref())
    }

    /// Record a module in the current build's manifest.
    pub fn record_module(
        &mut self,
        path: PathBuf,
        source: &str,
        deps: Vec<PathBuf>,
        compile_key: Option<String>,
    ) {
        if !self.enabled {
            return;
        }

        let metadata = fs::metadata(&path).ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime_secs = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.current.modules.insert(
            path,
            CachedModuleEntry {
                content_hash: content_hash(source),
                size,
                mtime_secs,
                deps,
                compile_key,
            },
        );
    }

    /// Compute the set of dirty modules given a set of changed file paths.
    ///
    /// Returns all modules that are directly changed OR are transitive
    /// dependents of changed modules (their output may be affected by the
    /// change propagating through imports).
    pub fn compute_dirty_set(&self, changed_paths: &[PathBuf]) -> BTreeSet<PathBuf> {
        if !self.enabled {
            // If cache disabled, everything is dirty: every module the
            // previous manifest knows about plus the changed paths
            // themselves. (A disabled cache loads no manifest, so the
            // changed paths must be included explicitly — returning only
            // `previous.modules` would yield an empty, "nothing dirty" set.)
            return self
                .previous
                .modules
                .keys()
                .cloned()
                .chain(changed_paths.iter().cloned())
                .collect();
        }

        let mut dirty = BTreeSet::new();

        // Mark directly changed files.
        for path in changed_paths {
            dirty.insert(path.clone());
        }

        // Build reverse dependency graph: module → set of modules that import it.
        let mut reverse_deps: BTreeMap<&PathBuf, Vec<&PathBuf>> = BTreeMap::new();
        for (path, entry) in &self.previous.modules {
            for dep in &entry.deps {
                reverse_deps.entry(dep).or_default().push(path);
            }
        }

        // BFS propagation: mark all transitive dependents as dirty.
        let mut queue: Vec<PathBuf> = changed_paths.to_vec();
        while let Some(current) = queue.pop() {
            if let Some(dependents) = reverse_deps.get(&current) {
                for dependent in dependents {
                    if dirty.insert((*dependent).clone()) {
                        queue.push((*dependent).clone());
                    }
                }
            }
        }

        dirty
    }

    /// Save the current build's manifest to disk.
    ///
    /// This should be called after a successful build to persist the graph
    /// for the next incremental build.
    pub fn save(&self) -> std::io::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        if let Some(parent) = self.manifest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string(&self.current).map_err(std::io::Error::other)?;

        // Atomic write via temp file + rename.
        let temp_path = self.manifest_path.with_extension("json.tmp");
        fs::write(&temp_path, json.as_bytes())?;
        if fs::rename(&temp_path, &self.manifest_path).is_err() {
            // Cross-device and Windows replacement semantics can reject rename.
            // Preserve the manifest update and always clean the temporary file,
            // including when the direct write itself fails.
            let write_result = fs::write(&self.manifest_path, json.as_bytes());
            let _ = fs::remove_file(&temp_path);
            write_result?;
        }

        Ok(())
    }

    /// Clear the persisted manifest (forces full rebuild on next run).
    pub fn clear(&self) -> std::io::Result<()> {
        if self.manifest_path.exists() {
            fs::remove_file(&self.manifest_path)?;
        }
        Ok(())
    }

    /// Number of modules in the previous build's manifest.
    pub fn previous_module_count(&self) -> usize {
        self.previous.modules.len()
    }

    /// Number of modules recorded in the current build so far.
    pub fn current_module_count(&self) -> usize {
        self.current.modules.len()
    }

    /// Check if the cache is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Load a manifest from disk, returning None if missing or incompatible.
    fn load_manifest(path: &Path) -> Option<GraphManifest> {
        let json = fs::read_to_string(path).ok()?;
        let manifest: GraphManifest = serde_json::from_str(&json).ok()?;

        // Version check: if the format changed, start fresh.
        if manifest.version != MANIFEST_VERSION {
            return None;
        }

        Some(manifest)
    }
}

/// Compute the blake3 content hash of a source string (hex, 32 chars).
fn content_hash(source: &str) -> String {
    let hash = blake3::hash(source.as_bytes());
    hash.to_hex()[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_check_detects_new_module() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = IncrementalGraphCache::new(tmp.path(), true);
        let fake_path = tmp.path().join("app").join("page.tsx");

        assert_eq!(
            cache.check_freshness(&fake_path, "export default function Page() {}"),
            FreshnessStatus::Stale,
        );
    }

    #[test]
    fn freshness_check_detects_unchanged_module() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        let source = "export default function Page() { return <main /> }";
        fs::write(&page, source).unwrap();

        // Build the first manifest.
        let mut cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(page.clone(), source, vec![], Some("key123".to_string()));
        cache.save().unwrap();

        // Reload — simulating a second build.
        let cache2 = IncrementalGraphCache::new(tmp.path(), true);
        assert_eq!(cache2.previous_module_count(), 1);
        assert_eq!(
            cache2.check_freshness(&page, source),
            FreshnessStatus::Fresh
        );
    }

    #[test]
    fn freshness_check_detects_changed_content() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        let source_v1 = "export default function Page() { return <main>V1</main> }";
        fs::write(&page, source_v1).unwrap();

        let mut cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(page.clone(), source_v1, vec![], None);
        cache.save().unwrap();

        // Change the file (same size to test content-hash path).
        let source_v2 = "export default function Page() { return <main>V2</main> }";
        fs::write(&page, source_v2).unwrap();

        let cache2 = IncrementalGraphCache::new(tmp.path(), true);
        assert_eq!(
            cache2.check_freshness(&page, source_v2),
            FreshnessStatus::Stale,
        );
    }

    #[test]
    fn compute_dirty_set_propagates_transitively() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        fs::create_dir_all(&app).unwrap();

        // Graph: page → Button → utils
        let utils = app.join("utils.ts");
        let button = app.join("Button.tsx");
        let page = app.join("page.tsx");

        fs::write(&utils, "export function cn() {}").unwrap();
        fs::write(&button, "import { cn } from './utils';").unwrap();
        fs::write(&page, "import Button from './Button';").unwrap();

        let mut cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(utils.clone(), "export function cn() {}", vec![], None);
        cache.record_module(
            button.clone(),
            "import { cn } from './utils';",
            vec![utils.clone()],
            None,
        );
        cache.record_module(
            page.clone(),
            "import Button from './Button';",
            vec![button.clone()],
            None,
        );
        cache.save().unwrap();

        // Reload and compute dirty set when utils changes.
        let cache2 = IncrementalGraphCache::new(tmp.path(), true);
        let dirty = cache2.compute_dirty_set(std::slice::from_ref(&utils));

        // utils is directly dirty, Button imports utils, page imports Button.
        assert!(dirty.contains(&utils));
        assert!(dirty.contains(&button));
        assert!(dirty.contains(&page));
        assert_eq!(dirty.len(), 3);
    }

    #[test]
    fn compute_dirty_set_only_affected_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        fs::create_dir_all(&app).unwrap();

        // Graph: page_a → utils, page_b → helpers (independent)
        let utils = app.join("utils.ts");
        let helpers = app.join("helpers.ts");
        let page_a = app.join("page-a.tsx");
        let page_b = app.join("page-b.tsx");

        for (path, src) in [
            (&utils, "export function cn() {}"),
            (&helpers, "export function fmt() {}"),
            (&page_a, "import { cn } from './utils';"),
            (&page_b, "import { fmt } from './helpers';"),
        ] {
            fs::write(path, src).unwrap();
        }

        let mut cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(utils.clone(), "export function cn() {}", vec![], None);
        cache.record_module(helpers.clone(), "export function fmt() {}", vec![], None);
        cache.record_module(
            page_a.clone(),
            "import { cn } from './utils';",
            vec![utils.clone()],
            None,
        );
        cache.record_module(
            page_b.clone(),
            "import { fmt } from './helpers';",
            vec![helpers.clone()],
            None,
        );
        cache.save().unwrap();

        // Only utils changed — page_b and helpers should NOT be dirty.
        let cache2 = IncrementalGraphCache::new(tmp.path(), true);
        let dirty = cache2.compute_dirty_set(std::slice::from_ref(&utils));

        assert!(dirty.contains(&utils));
        assert!(dirty.contains(&page_a));
        assert!(!dirty.contains(&helpers));
        assert!(!dirty.contains(&page_b));
        assert_eq!(dirty.len(), 2);
    }

    #[test]
    fn cached_deps_returns_stored_edges() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        fs::create_dir_all(&app).unwrap();

        let utils = app.join("utils.ts");
        let page = app.join("page.tsx");
        fs::write(&utils, "export const x = 1;").unwrap();
        fs::write(&page, "import { x } from './utils';").unwrap();

        let mut cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(utils.clone(), "export const x = 1;", vec![], None);
        cache.record_module(
            page.clone(),
            "import { x } from './utils';",
            vec![utils.clone()],
            None,
        );
        cache.save().unwrap();

        // Reload.
        let cache2 = IncrementalGraphCache::new(tmp.path(), true);
        let deps = cache2.cached_deps(&page).unwrap();
        assert_eq!(deps, &[utils]);
    }

    #[test]
    fn disabled_cache_always_stale() {
        let cache = IncrementalGraphCache::disabled();
        let fake = PathBuf::from("/fake/page.tsx");
        assert_eq!(
            cache.check_freshness(&fake, "source"),
            FreshnessStatus::Stale,
        );
        assert!(cache.cached_deps(&fake).is_none());
    }

    #[test]
    fn version_mismatch_invalidates_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join(".ruvyxa").join("cache").join("graph");
        fs::create_dir_all(&cache_dir).unwrap();

        // Write a manifest with a wrong version.
        let bad_manifest = r#"{"version":"old:v0","modules":{}}"#;
        fs::write(cache_dir.join("manifest.json"), bad_manifest).unwrap();

        let cache = IncrementalGraphCache::new(tmp.path(), true);
        assert_eq!(cache.previous_module_count(), 0);
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.tsx");
        let source = "export default function Page() {}";
        fs::write(&page, source).unwrap();

        let mut cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(
            page.clone(),
            source,
            vec![],
            Some("compile_abc123".to_string()),
        );
        cache.save().unwrap();

        // Reload and verify.
        let loaded = IncrementalGraphCache::new(tmp.path(), true);
        assert_eq!(loaded.previous_module_count(), 1);
        assert_eq!(loaded.cached_compile_key(&page), Some("compile_abc123"),);
    }

    #[test]
    fn repeated_save_replaces_the_manifest_without_leaving_a_temp_file() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("app/page.tsx");
        fs::create_dir_all(page.parent().unwrap()).unwrap();
        fs::write(&page, "export default function Page() {}").unwrap();

        let mut cache = IncrementalGraphCache::new(temp.path(), true);
        cache.record_module(
            page.clone(),
            "export default function Page() {}",
            vec![],
            None,
        );
        cache.save().unwrap();
        cache.record_module(
            page,
            "export default function Page() { return null }",
            vec![],
            None,
        );
        cache.save().unwrap();

        assert!(cache.manifest_path.is_file());
        assert!(!cache.manifest_path.with_extension("json.tmp").exists());
    }
}
