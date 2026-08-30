//! Persistent incremental graph cache with file fingerprinting.
//!
//! Stores the resolved module dependency graph on disk as a compact JSON
//! manifest at `<configured-cache-dir>/graph-manifest.json`. Each module entry records:
//!
//! - Its canonical path
//! - An authoritative blake3 content fingerprint plus a source-length fast-reject
//! - Its resolved dependency edges (list of paths)
//! - The specifier-to-path alias map those edges were resolved through
//!
//! On subsequent plugin-free client builds, the resolver reuses dependency
//! edges whose source content is unchanged. Compilation remains independently
//! content-addressed by `CompileCache`.
//!
//! ## Performance impact
//!
//! For a warm build, unchanged modules skip import extraction and dependency
//! resolution. Source bytes are still hashed so timestamp-preserving edits
//! cannot return stale edges.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Identity of the resolver whose edges this manifest records.
///
/// A version counter here would have to be bumped by hand every time an entry
/// gains a field, and forgetting the bump is silent: the build reuses entries
/// that cannot describe what the new field needs. Compatibility across *format*
/// changes is therefore a property of the entry format — every field added
/// after the fact is an `Option`, so "absent" is distinguishable from "empty"
/// and a reader can decline to reuse an entry that predates it. See
/// [`CachedModuleEntry::aliases`] for the pattern to follow when adding the
/// next one.
///
/// The crate version is a different thing from that counter: nothing maintains
/// it, and it moves on the one boundary a released build can cross. It has to
/// be here because what this manifest stores is the *resolver's answer*, not
/// the module's content — [`IncrementalGraphCache::check_freshness`] compares
/// content only, so a change to the resolution rules is invisible to it and
/// stale edges would be reused indefinitely. `cache::COMPILER_VERSION` folds
/// the same component in, for the same reason.
const MANIFEST_VERSION: &str = concat!("ruvyxa_graph_cache:", env!("CARGO_PKG_VERSION"));

/// A persisted module entry in the graph cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModuleEntry {
    /// Blake3 content hash (hex, 32 chars).
    pub content_hash: String,
    /// Byte length of the source this entry was recorded from.
    ///
    /// Recorded from the source text rather than from the file's metadata, so
    /// it is the same quantity [`IncrementalGraphCache::check_freshness`]
    /// compares against. Reading it back off disk cost one `stat` per module
    /// per build to obtain a number the caller already held, and would have
    /// disagreed with the check the moment a caller recorded source that is not
    /// byte-for-byte the file — turning the fast-reject into a permanent miss
    /// rather than a fast path.
    pub size: u64,
    /// Resolved dependency paths (absolute).
    pub deps: Vec<PathBuf>,
    /// Exact source-specifier to resolved-path bindings for those edges.
    ///
    /// Stored alongside the paths because the linker resolves a specifier
    /// through this map first and only then falls back to matching by path
    /// suffix. A tsconfig or plugin alias (`~/components/Button`) shares no
    /// suffix with its target, so a reused entry without the map would hand the
    /// linker a different resolution input than a cold build produced.
    ///
    /// `None` means an older build wrote this entry before aliases were
    /// recorded — not that the module has no aliases. The two must stay
    /// distinguishable, or a stale entry would silently claim "no aliases" and
    /// reintroduce that divergence. A reader treats `None` as not reusable and
    /// resolves the module fresh, which rewrites the entry complete. This is
    /// what keeps a *format* change out of [`MANIFEST_VERSION`], which carries
    /// only the derived compiler identity and no hand-maintained counter.
    #[serde(default)]
    pub aliases: Option<BTreeMap<String, PathBuf>>,
}

/// The full persisted graph manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphManifest {
    /// Manifest identity. Derived, never stamped — see [`MANIFEST_VERSION`]. A
    /// mismatch means the file belongs to a different cache or to a different
    /// build of this crate; either way it is a cold start, not a migration.
    pub version: String,
    /// Build/config namespace used while resolving these dependency edges.
    #[serde(default)]
    pub namespace: String,
    /// Module entries keyed by canonical path.
    pub modules: BTreeMap<PathBuf, CachedModuleEntry>,
}

impl GraphManifest {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            version: MANIFEST_VERSION.to_string(),
            namespace: namespace.into(),
            modules: BTreeMap::new(),
        }
    }
}

impl Default for GraphManifest {
    fn default() -> Self {
        Self::new(String::new())
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
    previous: Arc<GraphManifest>,
    /// The manifest being built for the current build.
    current: Arc<DashMap<PathBuf, CachedModuleEntry>>,
    /// Identity of the resolver/config contract for this cache generation.
    namespace: Arc<str>,
    /// Number of dependency lists reused during this process.
    edge_hits: Arc<AtomicUsize>,
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
        Self::at_dir(&cache_dir, "default", enabled)
    }

    /// Create a cache inside an explicit build-cache directory.
    pub fn at_dir(cache_dir: &Path, namespace: &str, enabled: bool) -> Self {
        let manifest_path = cache_dir.join("graph-manifest.json");

        let previous = if enabled {
            Self::load_manifest(&manifest_path, namespace)
                .unwrap_or_else(|| GraphManifest::new(namespace))
        } else {
            GraphManifest::new(namespace)
        };

        Self {
            manifest_path,
            previous: Arc::new(previous),
            current: Arc::new(DashMap::new()),
            namespace: Arc::from(namespace),
            edge_hits: Arc::new(AtomicUsize::new(0)),
            enabled,
        }
    }

    /// Create a disabled (no-op) cache.
    pub fn disabled() -> Self {
        Self {
            manifest_path: PathBuf::new(),
            previous: Arc::new(GraphManifest::default()),
            current: Arc::new(DashMap::new()),
            namespace: Arc::from("disabled"),
            edge_hits: Arc::new(AtomicUsize::new(0)),
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

    /// Get the cached dependency edges for a module.
    ///
    /// Returns `None` when the module is absent from the previous manifest.
    /// Freshness is the caller's gate: reuse is only sound after
    /// [`Self::check_freshness`] reports [`FreshnessStatus::Fresh`] for the same
    /// path and source.
    pub fn cached_deps(&self, path: &Path) -> Option<&[PathBuf]> {
        if !self.enabled {
            return None;
        }
        self.previous.modules.get(path).map(|e| e.deps.as_slice())
    }

    /// Get the cached specifier-to-path alias map recorded with those edges.
    ///
    /// Paired with [`Self::cached_deps`]: reusing edges without their aliases
    /// makes a warm build resolve alias specifiers differently from a cold one.
    /// `None` means the entry is missing or predates alias recording, and in
    /// both cases the caller must resolve the module fresh rather than reuse it.
    pub fn cached_aliases(&self, path: &Path) -> Option<&BTreeMap<String, PathBuf>> {
        if !self.enabled {
            return None;
        }
        self.previous
            .modules
            .get(path)
            .and_then(|entry| entry.aliases.as_ref())
    }

    /// Record a module in the current build's manifest.
    pub fn record_module(
        &self,
        path: PathBuf,
        source: &str,
        deps: Vec<PathBuf>,
        aliases: BTreeMap<String, PathBuf>,
    ) {
        if !self.enabled {
            return;
        }

        self.current.insert(
            path,
            CachedModuleEntry {
                content_hash: content_hash(source),
                size: source.len() as u64,
                deps,
                aliases: Some(aliases),
            },
        );
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

        // Complete route artifacts can skip graph traversal. Overlay modules
        // observed by this build onto still-existing previous entries instead
        // of erasing cache state for untouched routes.
        let mut modules = self
            .previous
            .modules
            .iter()
            .filter(|(path, _)| path.exists())
            .map(|(path, entry)| (path.clone(), entry.clone()))
            .collect::<BTreeMap<_, _>>();
        for entry in self.current.iter() {
            modules.insert(entry.key().clone(), entry.value().clone());
        }
        let manifest = GraphManifest {
            version: MANIFEST_VERSION.to_string(),
            namespace: self.namespace.to_string(),
            modules,
        };
        let json = serde_json::to_string(&manifest).map_err(std::io::Error::other)?;

        crate::atomic_file::write_atomic(&self.manifest_path, json.as_bytes())
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
        self.current.len()
    }

    /// Record one fingerprint-validated dependency-edge reuse.
    pub(crate) fn record_edge_hit(&self) {
        self.edge_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Number of persistent dependency lists reused in this process.
    pub fn edge_hits(&self) -> usize {
        self.edge_hits.load(Ordering::Relaxed)
    }

    /// Load a manifest from disk, returning None if missing or incompatible.
    fn load_manifest(path: &Path, namespace: &str) -> Option<GraphManifest> {
        let json = fs::read_to_string(path).ok()?;
        let manifest: GraphManifest = serde_json::from_str(&json).ok()?;

        // Version check: if the format changed, start fresh.
        if manifest.version != MANIFEST_VERSION || manifest.namespace != namespace {
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
        let cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(page.clone(), source, vec![], BTreeMap::new());
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

        let cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(page.clone(), source_v1, vec![], BTreeMap::new());
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
    fn cached_deps_returns_stored_edges() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        fs::create_dir_all(&app).unwrap();

        let utils = app.join("utils.ts");
        let page = app.join("page.tsx");
        fs::write(&utils, "export const x = 1;").unwrap();
        fs::write(&page, "import { x } from './utils';").unwrap();

        let cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(
            utils.clone(),
            "export const x = 1;",
            vec![],
            BTreeMap::new(),
        );
        cache.record_module(
            page.clone(),
            "import { x } from './utils';",
            vec![utils.clone()],
            BTreeMap::new(),
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

    /// The manifest identity carries no *hand-maintained* counter. Adding a
    /// field must be absorbed by the entry format, never by editing a literal —
    /// a bump somebody has to remember is silent when they forget, and the
    /// build then reuses entries that cannot answer what the new field needs.
    ///
    /// The crate version is not such a counter: nothing here maintains it, and
    /// it already changes on the one boundary a released build can cross. The
    /// resolver's *rules* live in this binary and are invisible to a content
    /// hash, so an identity scoped only to the project reuses edges a new
    /// resolver would not produce. This assertion used to pin the literal
    /// string, which is stricter than the rule its own doc comment states.
    #[test]
    fn the_manifest_identity_carries_no_version_counter() {
        assert!(
            MANIFEST_VERSION.contains(env!("CARGO_PKG_VERSION")),
            "the identity must follow the compiler that produced the entries: {MANIFEST_VERSION}"
        );
        // No `vN` segment anywhere: a counter is the thing that has to be
        // maintained by hand, and the entry format carries compatibility instead.
        for segment in MANIFEST_VERSION.split(':') {
            assert!(
                !segment.strip_prefix('v').is_some_and(
                    |rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                ),
                "compatibility belongs in the entry format, not in a `{segment}` version counter"
            );
        }
    }

    /// A manifest written by a different compiler build must not be reused.
    ///
    /// `check_freshness` compares source content only, so a change to the
    /// resolver's rules is invisible to it and stale dependency edges would be
    /// reused forever. The identity is the only thing that can see that change.
    #[test]
    fn a_manifest_from_another_compiler_version_loads_as_zero_modules() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("page.tsx");
        fs::write(&page, "export default function Page() {}").unwrap();

        let cache = IncrementalGraphCache::at_dir(temp.path(), "ns", true);
        cache.record_module(
            page.clone(),
            "export default function Page() {}",
            vec![],
            BTreeMap::new(),
        );
        cache.save().unwrap();
        assert_eq!(
            IncrementalGraphCache::at_dir(temp.path(), "ns", true).previous_module_count(),
            1
        );

        // Rewrite the identity as an older compiler would have spelled it.
        let manifest_path = temp.path().join("graph-manifest.json");
        let json = fs::read_to_string(&manifest_path).unwrap();
        let doctored = json.replace(MANIFEST_VERSION, "ruvyxa_graph_cache:0.0.0-other");
        assert_ne!(doctored, json, "the identity must be part of the manifest");
        fs::write(&manifest_path, doctored).unwrap();

        assert_eq!(
            IncrementalGraphCache::at_dir(temp.path(), "ns", true).previous_module_count(),
            0,
            "a manifest from another compiler must be a cold start, not a reuse"
        );
    }

    /// `record_module` and `check_freshness` must measure the same thing.
    ///
    /// The recorded length used to come from the file's metadata while the
    /// check compares the source text it is handed. For every caller today
    /// those are the same bytes, so the fast-reject worked — but a caller that
    /// records source the file does not literally contain would have written a
    /// length no later check could ever match, and the entry would report stale
    /// forever while looking like a cache hit was possible. Recording the
    /// length of the source removes the way for the two to disagree.
    #[test]
    fn a_recorded_entry_is_fresh_for_the_source_it_was_recorded_from() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("page.tsx");
        // On disk: something else entirely, and a different length.
        fs::write(&page, "// placeholder").unwrap();
        let recorded = "export default function Page() { return null }";

        let cache = IncrementalGraphCache::new(temp.path(), true);
        cache.record_module(page.clone(), recorded, vec![], BTreeMap::new());
        cache.save().unwrap();

        let reloaded = IncrementalGraphCache::new(temp.path(), true);
        assert_eq!(
            reloaded.check_freshness(&page, recorded),
            FreshnessStatus::Fresh,
            "the source that was recorded must read back as fresh"
        );
        assert_eq!(
            reloaded.check_freshness(&page, "export default function Page() { return 1234 }"),
            FreshnessStatus::Stale,
            "same length, different bytes: the hash still has to catch it"
        );
    }

    /// An entry written before aliases were recorded cannot say how its
    /// specifiers resolved. Reading it as "no aliases" would let a warm build
    /// resolve an aliased import differently from a cold one, so it must read as
    /// "not reusable" and be resolved fresh.
    #[test]
    fn an_entry_without_recorded_aliases_is_not_reusable() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join(".ruvyxa").join("cache").join("graph");
        fs::create_dir_all(&cache_dir).unwrap();
        let page = temp.path().join("page.tsx");
        let source = "import Button from '~/components/Button';";
        fs::write(&page, source).unwrap();

        // A manifest from a build that predates alias recording: same identity,
        // real edges, no `aliases` key at all. `mtime_secs` and `compile_key`
        // are fields the entry format has since dropped; they stay here because
        // an older manifest on a developer's disk still carries them, and it
        // has to keep loading rather than discarding a whole build's edges.
        let legacy = serde_json::json!({
            "version": MANIFEST_VERSION,
            "namespace": "default",
            "modules": {
                page.to_string_lossy(): {
                    "content_hash": content_hash(source),
                    "size": source.len(),
                    "mtime_secs": 0,
                    "deps": [temp.path().join("components/Button.tsx").to_string_lossy()],
                    "compile_key": null,
                }
            }
        });
        fs::write(
            cache_dir.join("graph-manifest.json"),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();

        let cache = IncrementalGraphCache::new(temp.path(), true);
        // The old manifest still loads — the identity did not change.
        assert_eq!(cache.previous_module_count(), 1);
        assert_eq!(cache.check_freshness(&page, source), FreshnessStatus::Fresh);
        assert!(cache.cached_deps(&page).is_some(), "edges still readable");
        assert!(
            cache.cached_aliases(&page).is_none(),
            "an unrecorded alias map must not read as an empty one"
        );
    }

    /// A recorded-but-genuinely-empty alias map is reusable, and must not be
    /// confused with the unrecorded case above.
    #[test]
    fn an_entry_with_no_aliases_is_still_reusable() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("page.tsx");
        let source = "export default function Page() {}";
        fs::write(&page, source).unwrap();

        let cache = IncrementalGraphCache::new(temp.path(), true);
        cache.record_module(page.clone(), source, vec![], BTreeMap::new());
        cache.save().unwrap();

        let reloaded = IncrementalGraphCache::new(temp.path(), true);
        assert_eq!(
            reloaded.cached_aliases(&page),
            Some(&BTreeMap::new()),
            "an empty map was recorded, so it is known and reusable"
        );
    }

    /// Aliases survive the disk round-trip, which is the whole point: the warm
    /// build must hand the linker the same map the cold build resolved.
    #[test]
    fn recorded_aliases_survive_a_reload() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("page.tsx");
        let button = temp.path().join("components").join("Button.tsx");
        let source = "import Button from '~/components/Button';";
        fs::write(&page, source).unwrap();

        let aliases = BTreeMap::from([("~/components/Button".to_string(), button.clone())]);
        let cache = IncrementalGraphCache::new(temp.path(), true);
        cache.record_module(page.clone(), source, vec![button.clone()], aliases.clone());
        cache.save().unwrap();

        let reloaded = IncrementalGraphCache::new(temp.path(), true);
        assert_eq!(reloaded.cached_aliases(&page), Some(&aliases));
        assert_eq!(reloaded.cached_deps(&page), Some([button].as_slice()));
    }

    #[test]
    fn version_mismatch_invalidates_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join(".ruvyxa").join("cache").join("graph");
        fs::create_dir_all(&cache_dir).unwrap();

        // Write a manifest with a wrong version.
        let bad_manifest = r#"{"version":"old:v0","modules":{}}"#;
        fs::write(cache_dir.join("graph-manifest.json"), bad_manifest).unwrap();

        let cache = IncrementalGraphCache::new(tmp.path(), true);
        assert_eq!(cache.previous_module_count(), 0);
    }

    #[test]
    fn namespace_mismatch_invalidates_dependency_edges() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("cache");
        let source_file = temp.path().join("page.tsx");
        fs::write(&source_file, "export default function Page() {}").unwrap();

        let first = IncrementalGraphCache::at_dir(&cache_dir, "config-a", true);
        first.record_module(
            source_file,
            "export default function Page() {}",
            Vec::new(),
            BTreeMap::new(),
        );
        first.save().unwrap();

        let second = IncrementalGraphCache::at_dir(&cache_dir, "config-b", true);
        assert_eq!(second.previous_module_count(), 0);
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.tsx");
        let source = "export default function Page() {}";
        fs::write(&page, source).unwrap();

        let dep = app.join("shared.ts");
        let aliases = BTreeMap::from([("~/shared".to_string(), dep.clone())]);
        let cache = IncrementalGraphCache::new(tmp.path(), true);
        cache.record_module(page.clone(), source, vec![dep.clone()], aliases.clone());
        cache.save().unwrap();

        // Reload and verify the whole entry survives, not just its edges.
        let loaded = IncrementalGraphCache::new(tmp.path(), true);
        assert_eq!(loaded.previous_module_count(), 1);
        assert_eq!(loaded.cached_deps(&page), Some([dep].as_slice()));
        assert_eq!(loaded.cached_aliases(&page), Some(&aliases));
        assert_eq!(
            loaded.check_freshness(&page, source),
            FreshnessStatus::Fresh
        );
    }

    #[test]
    fn repeated_save_replaces_the_manifest_without_leaving_a_temp_file() {
        let temp = tempfile::tempdir().unwrap();
        let page = temp.path().join("app/page.tsx");
        fs::create_dir_all(page.parent().unwrap()).unwrap();
        fs::write(&page, "export default function Page() {}").unwrap();

        let cache = IncrementalGraphCache::new(temp.path(), true);
        cache.record_module(
            page.clone(),
            "export default function Page() {}",
            vec![],
            BTreeMap::new(),
        );
        cache.save().unwrap();
        cache.record_module(
            page,
            "export default function Page() { return null }",
            vec![],
            BTreeMap::new(),
        );
        cache.save().unwrap();

        assert!(cache.manifest_path.is_file());
        assert!(!cache.manifest_path.with_extension("json.tmp").exists());
    }
}
