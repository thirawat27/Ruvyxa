//! Module resolver: walks `import`/`require` specifiers and produces a
//! topologically-ordered list of (absolute-path, source-code) pairs.
//!
//! ## Resolution order
//!
//! For a given specifier the resolver tries the following strategies in order:
//!
//! 1. **Relative path** (starts with `./` or `../`) — probes TypeScript/JS
//!    extensions via [`resolve_specifier`].
//! 2. **Absolute path** — used for framework-generated virtual imports.
//! 3. **tsconfig.json `paths`/`baseUrl`** — checked before `node_modules`.
//! 4. **Bare specifier** — resolved against `node_modules` the way Node does:
//!    walk upward from the importing file, and in the first matching package
//!    consult `exports`, then the legacy `browser`/`module`/`main` fields, then
//!    `index`. The upward walk is what makes non-hoisted installs work — under
//!    pnpm a transitive dependency lives beside its dependent in the store and
//!    never appears in the project's own `node_modules`.
//!
//! ## Performance
//!
//! The resolver uses a **lock-free concurrent resolution cache** backed by
//! [`DashMap`] that maps `(base_dir, specifier)` pairs to resolved absolute
//! paths. This eliminates both redundant filesystem stat calls and lock
//! contention when multiple rayon threads resolve modules in parallel.
//!
//! ### Key optimizations over the previous Mutex-based design:
//!
//! 1. **DashMap sharded locking** — concurrent reads and writes operate on
//!    independent shards, so parallel resolvers rarely contend.
//! 2. **Parallel subtree resolution** — once the entry module's direct deps
//!    are known, independent subtrees are resolved concurrently via rayon.
//! 3. **Memory-mapped source reads** — files over 64 KiB are read via mmap
//!    to avoid unnecessary copies and exploit OS page cache.
//! 4. **Batch stat elision** — resolved paths are fingerprinted by (mtime, len)
//!    and served from cache on subsequent builds without re-statting.
//! 5. **tsconfig path aliases** — `@/components/Button` resolves to the mapped
//!    project path without hitting `node_modules`.
//!
//! For large module graphs (100+ modules), this reduces resolution wall-time
//! by 3–5× compared to the sequential BFS approach.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use rayon::prelude::*;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::hooks::{BuildHookContext, BuildHookPipeline};
use crate::incremental::{FreshnessStatus, IncrementalGraphCache};
use crate::{BundleError, BundleTarget, JsxRuntime, Result};
use crate::{ast, minifier};

/// A resolved module: its canonical path and raw source text.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    /// Canonical absolute path to the source file.
    pub path: PathBuf,
    /// Raw UTF-8 source (TypeScript/TSX/JS/JSX).
    pub source: String,
    /// Content already lowered by the configured persistent MDX host. Direct
    /// crate consumers without a host leave this empty and use the Rust fallback.
    pub compiled_content: Option<Arc<str>>,
    /// Optional source map supplied by a build load hook.
    pub load_source_map: Option<String>,
    /// Specifiers that this module imports (absolute paths after resolution).
    pub deps: Vec<PathBuf>,
    /// Exact source specifier to resolved path bindings, including plugin aliases.
    pub dependency_aliases: BTreeMap<String, PathBuf>,
    /// Directories whose membership affects compile-time glob expansion.
    pub watch_paths: Vec<PathBuf>,
    /// Files materialized by compile-time glob expansion.
    pub glob_matches: Vec<PathBuf>,
    /// Whether this module is part of `node_modules` (external).
    pub is_external: bool,
}

#[derive(Debug, Clone, Default)]
struct ResolvedDependencies {
    paths: Vec<PathBuf>,
    aliases: BTreeMap<String, PathBuf>,
}

/// Fingerprint for a cached source file: mtime + length.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourceFingerprint {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

/// Cached source entry with fingerprint for invalidation.
#[derive(Debug, Clone)]
struct CachedSource {
    fingerprint: SourceFingerprint,
    source: Arc<str>,
}

/// Fingerprints for the two resolver configuration files we support.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TsConfigFingerprint {
    files: Vec<(PathBuf, Option<[u8; 32]>)>,
}

/// Parsed resolver configuration and the file state it was derived from.
#[derive(Debug, Clone)]
struct CachedTsConfig {
    fingerprint: TsConfigFingerprint,
    paths: TsConfigPaths,
}

/// Resolution cache key: (base_dir, specifier).
type ResolutionKey = (Arc<str>, Arc<str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DependencyCacheKey {
    base_dir: Arc<str>,
    source_hash: [u8; 32],
    target: u8,
    /// Automatic JSX injects an extra `react/jsx-runtime` edge, so the same
    /// source resolves to a different dependency set per JSX runtime.
    jsx_automatic: bool,
    /// Whether the importing module's extension lets the transform produce JSX
    /// at all — `compiler::jsx_is_enabled(extension, true)`. A `.ts` and a
    /// `.tsx` file holding the same bytes in the same directory resolve to
    /// different dependency sets, because only one of them gets the injected
    /// `react/jsx-runtime` import.
    jsx_allowed_by_extension: bool,
}

/// Module the automatic JSX transform imports its factory helpers from.
const JSX_RUNTIME_SPECIFIER: &str = "react/jsx-runtime";

/// Shared resolver cache for a batch of bundle jobs.
///
/// Uses [`DashMap`] for lock-free concurrent access from multiple rayon
/// Memoized `import.meta.glob` directory walks, keyed by
/// `(absolute_pattern, watch_root)`.
///
/// Named rather than spelled inline so the field below reads as one idea, and
/// because `clippy::type_complexity` refuses the nested form.
type GlobMatchMemo = Arc<DashMap<(PathBuf, PathBuf), Arc<Vec<PathBuf>>>>;

/// threads. This cache is designed to be shared across parallel route
/// bundling workers — no mutex contention on hot paths.
#[derive(Debug, Clone, Default)]
pub struct ResolveGraphCache {
    /// Resolution results: (base_dir, specifier) → Option<absolute_path>.
    resolutions: Arc<DashMap<ResolutionKey, Option<PathBuf>>>,
    /// Source file cache: path → (fingerprint, source_text).
    sources: Arc<DashMap<PathBuf, CachedSource>>,
    /// Parsed tsconfig/jsconfig cache, keyed by canonical project root.
    tsconfigs: Arc<DashMap<PathBuf, CachedTsConfig>>,
    /// Fully resolved dependencies for build-hook-free source snapshots.
    ///
    /// Stores the whole [`ResolvedDependencies`] — paths *and* the
    /// specifier-to-path alias map — because the two are one answer. The linker
    /// consults the alias map first and only then matches by path suffix, and an
    /// alias like `~/components/Button` shares no suffix with its target, so
    /// handing back the paths with an empty map makes a warm hit resolve
    /// differently from a cold miss. The persistent cache states the same
    /// contract on [`crate::incremental::CachedModuleEntry::aliases`].
    dependencies: Arc<DashMap<DependencyCacheKey, Arc<ResolvedDependencies>>>,
    /// Parsed `package.json` entry-point fields, keyed by the package.json
    /// path. Avoids re-reading and re-parsing the same `node_modules`
    /// package.json for every importing module that resolves a bare specifier
    /// from it.
    package_json: Arc<DashMap<PathBuf, CachedPackageJson>>,
    /// Files an `import.meta.glob` pattern matched, keyed by the resolved
    /// absolute pattern and the directory the walk starts from.
    ///
    /// `collect_matches` is a directory-tree traversal and the expander runs
    /// once per module per route, so a content site globbing
    /// `./content/**/*.md` re-walked that whole tree for every route that
    /// reached the module — and paid it again on every incremental rebuild,
    /// because a non-empty `watch_roots` disables persistent dependency-edge
    /// reuse.
    ///
    /// Keyed on this cache rather than on a process global, deliberately. Under
    /// `dev` a file can appear between two routes, and this cache is what the
    /// host recreates or invalidates when it does; a `BundleContext` is built
    /// and dropped inside one bundle pass, so the memo lives exactly as long as
    /// its answer is stable.
    glob_matches: GlobMatchMemo,
    /// Production builds operate on one immutable input snapshot and can skip
    /// repeated metadata checks after the first source read.
    stable_snapshot: bool,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveCacheStats {
    pub resolution_entries: usize,
    pub source_entries: usize,
    pub dependency_entries: usize,
    pub configuration_entries: usize,
    pub package_entries: usize,
    pub glob_entries: usize,
    pub resident_bytes: u64,
    pub disposable_bytes: u64,
}

/// Entry-point fields of one `package.json`.
///
/// `exports` is the modern map; `browser`/`module`/`main` are the legacy
/// fields Node and every bundler still honour for the (very common) packages
/// that ship no `exports` map at all — `scheduler`, for one, which React DOM
/// requires at runtime.
#[derive(Debug, Default)]
struct PackageManifest {
    exports: Option<PackageJsonValue>,
    browser: Option<String>,
    module: Option<String>,
    main: Option<String>,
}

/// Cached `package.json` entry points, fingerprinted for invalidation.
/// `manifest: None` means the file is missing or unparsable, which is itself
/// worth caching to avoid re-reading it.
#[derive(Debug, Clone)]
struct CachedPackageJson {
    fingerprint: SourceFingerprint,
    manifest: Option<Arc<PackageManifest>>,
}

impl ResolveGraphCache {
    /// Create an empty resolver cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cache for one immutable production-build input snapshot.
    pub fn for_build() -> Self {
        Self {
            stable_snapshot: true,
            ..Self::default()
        }
    }

    /// Create a cache pre-sized for an expected module count.
    pub fn with_capacity(resolution_hint: usize, source_hint: usize) -> Self {
        Self {
            resolutions: Arc::new(DashMap::with_capacity(resolution_hint)),
            sources: Arc::new(DashMap::with_capacity(source_hint)),
            tsconfigs: Arc::new(DashMap::with_capacity(1)),
            dependencies: Arc::new(DashMap::with_capacity(source_hint)),
            package_json: Arc::new(DashMap::new()),
            glob_matches: Arc::new(DashMap::new()),
            stable_snapshot: false,
        }
    }

    /// Look up a cached resolution result.
    #[inline]
    fn resolution(&self, base_dir: &str, specifier: &str) -> Option<Option<PathBuf>> {
        let key = (Arc::from(base_dir), Arc::from(specifier));
        self.resolutions.get(&key).map(|entry| entry.clone())
    }

    /// Insert a resolution result into the cache.
    #[inline]
    fn insert_resolution(&self, base_dir: &str, specifier: &str, result: Option<PathBuf>) {
        let key = (Arc::from(base_dir), Arc::from(specifier));
        self.resolutions.insert(key, result);
    }

    /// Read source text for a file, using the cache and mmap for large files.
    ///
    /// Hands back the cached `Arc<str>` rather than a `String`. The cache
    /// already owns the bytes, so materializing a private copy per lookup
    /// charged one full duplication of every module's source to every
    /// resolution — and the caller then had to clone that copy again to keep it
    /// alive alongside the value it derived from it. Sharing the allocation
    /// removes both copies without changing what any caller reads.
    fn read_source(&self, path: &Path) -> Result<Arc<str>> {
        if self.stable_snapshot
            && let Some(entry) = self.sources.get(path)
        {
            return Ok(Arc::clone(&entry.source));
        }
        let metadata = fs::metadata(path).map_err(|error| {
            BundleError::Io(std::io::Error::new(
                error.kind(),
                format!("{}: {}", path.display(), error),
            ))
        })?;
        let fingerprint = SourceFingerprint {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        };

        // Fast path: check cache with fingerprint validation.
        if let Some(entry) = self.sources.get(path)
            && entry.fingerprint == fingerprint
        {
            return Ok(Arc::clone(&entry.source));
        }

        // Cache miss or stale — read the file.
        let source: Arc<str> = Arc::from(read_source_fast(path)?.as_str());

        self.sources.insert(
            path.to_path_buf(),
            CachedSource {
                fingerprint,
                source: Arc::clone(&source),
            },
        );

        Ok(source)
    }

    /// Load resolver aliases once per configuration version across route builds.
    fn tsconfig_paths(&self, project_root: &Path) -> TsConfigPaths {
        if self.stable_snapshot
            && let Some(entry) = self.tsconfigs.get(project_root)
        {
            return entry.paths.clone();
        }
        if let Some(entry) = self.tsconfigs.get(project_root) {
            let fingerprint = tsconfig_fingerprint(project_root, &entry.paths.config_files);
            if entry.fingerprint == fingerprint {
                return entry.paths.clone();
            }
        }

        let paths = TsConfigPaths::load(project_root);
        let fingerprint = tsconfig_fingerprint(project_root, &paths.config_files);
        self.tsconfigs.insert(
            project_root.to_path_buf(),
            CachedTsConfig {
                fingerprint,
                paths: paths.clone(),
            },
        );
        paths
    }

    /// Load a package.json's entry-point fields once per (path, fingerprint),
    /// mirroring the tsconfig cache above.
    ///
    /// Returns `None` when the file does not exist or does not parse as a JSON
    /// object — the caller reads that as "this directory is not a package".
    fn package_manifest(&self, pkg_json_path: &Path) -> Option<Arc<PackageManifest>> {
        if self.stable_snapshot
            && let Some(entry) = self.package_json.get(pkg_json_path)
        {
            return entry.manifest.clone();
        }
        let Ok(metadata) = fs::metadata(pkg_json_path) else {
            // A missing package.json is the common case while walking
            // `node_modules` candidates, but it cannot be fingerprint-cached
            // (there is nothing to stat), so do not poison the map with it.
            return None;
        };
        let fingerprint = SourceFingerprint {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        };
        if let Some(entry) = self.package_json.get(pkg_json_path)
            && entry.fingerprint == fingerprint
        {
            return entry.manifest.clone();
        }

        let manifest = fs::read_to_string(pkg_json_path)
            .ok()
            .and_then(|content| parse_package_manifest(&content))
            .map(Arc::new);
        self.package_json.insert(
            pkg_json_path.to_path_buf(),
            CachedPackageJson {
                fingerprint,
                manifest: manifest.clone(),
            },
        );
        manifest
    }

    /// Files matching one glob pattern, walked once per (pattern, watch root).
    ///
    /// `collect` runs only on a miss, and its answer is shared rather than
    /// copied — every module that names the same pattern wants the same list.
    /// An empty answer is cached too: it costs exactly the same walk to produce
    /// as a full one, and a project globbing a directory it has not created yet
    /// is the case that would otherwise re-walk hardest.
    pub(crate) fn matched_glob_files(
        &self,
        pattern: &Path,
        watch_root: &Path,
        collect: impl FnOnce() -> Result<Vec<PathBuf>>,
    ) -> Result<Arc<Vec<PathBuf>>> {
        let key = (pattern.to_path_buf(), watch_root.to_path_buf());
        if let Some(hit) = self.glob_matches.get(&key) {
            return Ok(Arc::clone(hit.value()));
        }
        let matches = Arc::new(collect()?);
        self.glob_matches.insert(key, Arc::clone(&matches));
        Ok(matches)
    }

    /// Number of cached resolution entries. Intended for diagnostics/tests.
    pub fn resolution_count(&self) -> usize {
        self.resolutions.len()
    }

    /// Number of cached source files. Intended for diagnostics/tests.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Number of content-keyed dependency lists retained by this context.
    pub fn dependency_count(&self) -> usize {
        self.dependencies.len()
    }

    /// Content fingerprint of every tsconfig/jsconfig file that contributes to
    /// the effective resolver model, including inherited package/local files.
    pub fn configuration_hash(&self, project_root: &Path) -> String {
        let config = self.tsconfig_paths(project_root);
        let mut hasher = blake3::Hasher::new();
        for file in &config.config_files {
            let path = file.to_string_lossy().replace('\\', "/");
            hasher.update(&(path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
            let source = fs::read(file).unwrap_or_default();
            hasher.update(&(source.len() as u64).to_le_bytes());
            hasher.update(&source);
        }
        hasher.finalize().to_hex().to_string()
    }

    /// Approximate bytes owned by resolver cache keys and values.
    pub fn stats(&self) -> ResolveCacheStats {
        let resolution_bytes = self
            .resolutions
            .iter()
            .map(|entry| {
                let ((base, specifier), resolved) = entry.pair();
                base.len() as u64
                    + specifier.len() as u64
                    + resolved
                        .as_ref()
                        .map(|path| path.as_os_str().len() as u64)
                        .unwrap_or(0)
            })
            .sum::<u64>();
        let source_bytes = self
            .sources
            .iter()
            .map(|entry| entry.key().as_os_str().len() as u64 + entry.value().source.len() as u64)
            .sum::<u64>();
        let dependency_bytes = self
            .dependencies
            .iter()
            .map(|entry| {
                let dependencies = entry.value();
                entry.key().base_dir.len() as u64
                    + 34
                    + dependencies
                        .paths
                        .iter()
                        .map(|path| path.as_os_str().len() as u64)
                        .sum::<u64>()
                    // The alias map is part of the cached answer, so it is part
                    // of the budget this heuristic spends.
                    + dependencies
                        .aliases
                        .iter()
                        .map(|(specifier, path)| {
                            specifier.len() as u64 + path.as_os_str().len() as u64
                        })
                        .sum::<u64>()
            })
            .sum::<u64>();
        let configuration_bytes = self
            .tsconfigs
            .iter()
            .map(|entry| {
                entry.key().as_os_str().len() as u64
                    + entry.value().paths.config_dir.as_os_str().len() as u64
                    + entry
                        .value()
                        .paths
                        .paths
                        .iter()
                        .map(|(alias, targets)| {
                            alias.len() as u64
                                + targets
                                    .iter()
                                    .map(|target| target.len() as u64)
                                    .sum::<u64>()
                        })
                        .sum::<u64>()
            })
            .sum::<u64>();
        let package_bytes = self
            .package_json
            .iter()
            .map(|entry| entry.key().as_os_str().len() as u64)
            .sum::<u64>();
        let glob_bytes = self
            .glob_matches
            .iter()
            .map(|entry| {
                let (pattern, watch_root) = entry.key();
                pattern.as_os_str().len() as u64
                    + watch_root.as_os_str().len() as u64
                    + entry
                        .value()
                        .iter()
                        .map(|path| path.as_os_str().len() as u64)
                        .sum::<u64>()
            })
            .sum::<u64>();
        ResolveCacheStats {
            resolution_entries: self.resolutions.len(),
            source_entries: self.sources.len(),
            dependency_entries: self.dependencies.len(),
            configuration_entries: self.tsconfigs.len(),
            package_entries: self.package_json.len(),
            glob_entries: self.glob_matches.len(),
            resident_bytes: resolution_bytes
                .saturating_add(source_bytes)
                .saturating_add(dependency_bytes)
                .saturating_add(configuration_bytes)
                .saturating_add(package_bytes)
                .saturating_add(glob_bytes),
            disposable_bytes: resolution_bytes
                .saturating_add(dependency_bytes)
                .saturating_add(configuration_bytes)
                .saturating_add(package_bytes)
                .saturating_add(glob_bytes),
        }
    }

    /// Drop rebuildable resolver derivations while retaining source snapshots.
    /// Returns the number of entries removed.
    pub fn evict_disposable(&self) -> u64 {
        let evicted = self
            .resolutions
            .len()
            .saturating_add(self.dependencies.len())
            .saturating_add(self.tsconfigs.len())
            .saturating_add(self.package_json.len())
            .saturating_add(self.glob_matches.len()) as u64;
        self.resolutions.clear();
        self.dependencies.clear();
        self.tsconfigs.clear();
        self.package_json.clear();
        self.glob_matches.clear();
        evicted
    }

    /// Drop cached entries for specific file paths.
    ///
    /// The only cache that outlives a single read is the [`Self::for_build`]
    /// snapshot, which skips metadata checks entirely, so this is how a caller
    /// tells that snapshot a file underneath it moved. Every `BundleContext` in
    /// the tree is built and dropped inside one bundle pass, which is why no
    /// production caller needs it today: the snapshot cannot outlive the inputs
    /// it froze. A longer-lived context would.
    pub fn invalidate_paths(&self, paths: &[PathBuf]) {
        for path in paths {
            self.sources.remove(path);
            self.package_json.remove(path);
            // Remove any resolution entries that resolved to this path.
            self.resolutions.retain(|_, v| v.as_ref() != Some(path));
            self.tsconfigs.retain(|root, entry| {
                path != &root.join("tsconfig.json")
                    && path != &root.join("jsconfig.json")
                    && !entry.paths.config_files.contains(path)
            });
        }
        self.dependencies.clear();
        // A glob answers with the files that exist, so the event this method
        // reports — a file moved underneath a snapshot that skips metadata
        // checks — is exactly the event that can add or remove a match. The
        // path that changed need not be one the pattern matched (a new
        // directory changes what the walk reaches), so the whole memo goes.
        self.glob_matches.clear();
    }
}

fn tsconfig_fingerprint(project_root: &Path, config_files: &[PathBuf]) -> TsConfigFingerprint {
    let mut files = BTreeSet::from([
        project_root.join("tsconfig.json"),
        project_root.join("jsconfig.json"),
    ]);
    files.extend(config_files.iter().cloned());
    TsConfigFingerprint {
        files: files
            .into_iter()
            .map(|path| {
                let fingerprint = fs::read(&path)
                    .ok()
                    .map(|source| *blake3::hash(&source).as_bytes());
                (path, fingerprint)
            })
            .collect(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// tsconfig.json path alias support
// ─────────────────────────────────────────────────────────────────────────────

/// A parsed subset of `tsconfig.json` relevant to module resolution.
#[derive(Debug, Clone, Default)]
pub struct TsConfigPaths {
    /// Directory containing tsconfig.json. `paths` targets are resolved from
    /// `baseUrl` when present, otherwise from this directory.
    pub config_dir: PathBuf,
    /// Base URL for non-relative imports (usually the project root or `src/`).
    pub base_url: Option<PathBuf>,
    /// Path alias mappings, e.g. `"@/*" → ["./src/*"]`.
    pub paths: Vec<(String, Vec<String>)>,
    /// Base directory for each declaration in `paths`.
    path_bases: Vec<PathBuf>,
    /// Every configuration file whose contents formed this effective model.
    config_files: Vec<PathBuf>,
    project_root: PathBuf,
}

/// A `tsconfig.json` that exists but could not be read as JSONC.
///
/// A project with no tsconfig and a project whose tsconfig is malformed both end
/// up with no aliases, and until this existed the two were indistinguishable:
/// resolution simply stopped honouring `paths` and every aliased import failed
/// with "module not found", naming the import rather than the config that was
/// skipped. Reported by `ruvyxa doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsConfigProblem {
    pub path: PathBuf,
    pub message: String,
}

impl TsConfigPaths {
    /// Load and parse `tsconfig.json` (or `jsconfig.json`) from the given root.
    ///
    /// Only `compilerOptions.baseUrl` and `compilerOptions.paths` are read.
    /// Returns an empty config if the file is missing or malformed.
    pub fn load(project_root: &Path) -> Self {
        Self::load_reporting(project_root).0
    }

    /// Load as [`TsConfigPaths::load`], also reporting a file that failed to parse.
    ///
    /// Resolution cannot fail on a malformed config — a build that stopped dead
    /// because a comma was in the wrong place would be worse than one that
    /// resolves what it can — so the aliases are still dropped. The difference is
    /// that the reason is now available to say out loud.
    pub fn load_reporting(project_root: &Path) -> (Self, Option<TsConfigProblem>) {
        let candidates = [
            project_root.join("tsconfig.json"),
            project_root.join("jsconfig.json"),
        ];

        let mut problem = None;
        for path in &candidates {
            if !path.is_file() {
                continue;
            }
            let mut visiting = BTreeSet::new();
            let (config, config_problem) = load_config_chain(path, project_root, &mut visiting);
            if !config.paths.is_empty() || config.base_url.is_some() || config_problem.is_none() {
                return (config, config_problem);
            }
            problem = config_problem;
        }

        (TsConfigPaths::default(), problem)
    }

    /// Attempt to resolve a specifier using the path aliases.
    ///
    /// Returns `Some(absolute_path)` if an alias matches and the target file
    /// exists, `None` otherwise.
    pub fn resolve(&self, specifier: &str) -> Option<PathBuf> {
        if is_reserved_alias_specifier(specifier) {
            return None;
        }
        // 1. Try exact path aliases.
        let mut indices = (0..self.paths.len()).collect::<Vec<_>>();
        indices.sort_by(|left, right| {
            alias_pattern_order(&self.paths[*left].0, &self.paths[*right].0)
        });
        for index in indices {
            let (pattern, targets) = &self.paths[index];
            let suffix = match_alias_pattern(pattern, specifier);

            if let Some(suffix) = suffix {
                for target in targets {
                    let candidate_str = target.replacen('*', suffix, 1);

                    let target = Path::new(&candidate_str);
                    let candidate = if target.is_absolute() {
                        target.to_path_buf()
                    } else {
                        self.path_bases
                            .get(index)
                            .unwrap_or(&self.config_dir)
                            .join(target)
                    };

                    if let Some(resolved) = resolve_file_candidate(&candidate)
                        && path_is_inside(&resolved, &self.project_root)
                    {
                        return Some(resolved);
                    }
                }
            }
        }

        // 2. Try baseUrl-relative resolution (for non-relative, non-bare specifiers).
        if !specifier.starts_with('.')
            && !specifier.starts_with('/')
            && let Some(base) = &self.base_url
        {
            let candidate = base.join(specifier);
            if let Some(resolved) = resolve_file_candidate(&candidate)
                && path_is_inside(&resolved, &self.project_root)
            {
                return Some(resolved);
            }
        }

        None
    }

    pub(crate) fn resolve_glob_pattern(&self, pattern: &str) -> Option<PathBuf> {
        if is_reserved_alias_specifier(pattern) {
            return None;
        }
        let mut indices = (0..self.paths.len()).collect::<Vec<_>>();
        indices.sort_by(|left, right| {
            alias_pattern_order(&self.paths[*left].0, &self.paths[*right].0)
        });
        for index in indices {
            let (alias, targets) = &self.paths[index];
            if let Some(suffix) = match_alias_pattern(alias, pattern)
                && let Some(target) = targets.first()
            {
                let target = PathBuf::from(target.replacen('*', suffix, 1));
                return Some(if target.is_absolute() {
                    target
                } else {
                    self.path_bases
                        .get(index)
                        .unwrap_or(&self.config_dir)
                        .join(target)
                });
            }
        }
        self.base_url.as_ref().map(|base| base.join(pattern))
    }
}

fn load_config_chain(
    config_path: &Path,
    project_root: &Path,
    visiting: &mut BTreeSet<PathBuf>,
) -> (TsConfigPaths, Option<TsConfigProblem>) {
    let config_path = config_path.to_path_buf();
    let cycle_key = ruvyxa_diagnostics::normalized_canonical_path(&config_path);
    if !visiting.insert(cycle_key.clone()) {
        return (
            empty_tsconfig(project_root),
            Some(TsConfigProblem {
                path: config_path,
                message: "cyclic tsconfig/jsconfig extends chain".to_string(),
            }),
        );
    }
    let content = match fs::read_to_string(&config_path) {
        Ok(content) => content,
        Err(error) => {
            visiting.remove(&cycle_key);
            return (
                empty_tsconfig(project_root),
                Some(TsConfigProblem {
                    path: config_path,
                    message: format!("cannot read extended configuration: {error}"),
                }),
            );
        }
    };
    let value = match parse_jsonc(&content) {
        Ok(value) => value,
        Err(error) => {
            visiting.remove(&cycle_key);
            return (
                empty_tsconfig(project_root),
                Some(TsConfigProblem {
                    path: config_path,
                    message: error.to_string(),
                }),
            );
        }
    };
    let config_dir = config_path.parent().unwrap_or(project_root);
    let mut problem = None;
    let mut effective = value
        .get("extends")
        .and_then(serde_json::Value::as_str)
        .map(|specifier| {
            if let Some(parent) = resolve_extends_config(specifier, config_dir) {
                let (parent, parent_problem) = load_config_chain(&parent, project_root, visiting);
                problem = parent_problem;
                parent
            } else {
                problem = Some(TsConfigProblem {
                    path: config_path.clone(),
                    message: format!("cannot resolve extended configuration `{specifier}`"),
                });
                empty_tsconfig(project_root)
            }
        })
        .unwrap_or_else(|| empty_tsconfig(project_root));

    if let Some(compiler_options) = value.get("compilerOptions") {
        if let Some(base_url) = compiler_options
            .get("baseUrl")
            .and_then(serde_json::Value::as_str)
        {
            let base_url = Path::new(base_url);
            effective.base_url = Some(if base_url.is_absolute() {
                base_url.to_path_buf()
            } else {
                config_dir.join(base_url)
            });
        }
        if let Some(paths) = compiler_options
            .get("paths")
            .and_then(serde_json::Value::as_object)
        {
            effective.paths.clear();
            effective.path_bases.clear();
            // TypeScript resolves `paths` against the effective `baseUrl` —
            // including one inherited through `extends` — and only falls back to
            // the declaring config's directory when no `baseUrl` is in effect.
            // `effective.base_url` already holds local-over-inherited above, so
            // reading it here is what keeps a base config's `baseUrl` from being
            // silently ignored by a child that declares `paths`.
            let declaration_base = effective
                .base_url
                .clone()
                .unwrap_or_else(|| config_dir.to_path_buf());
            for (pattern, targets) in paths {
                let targets = targets
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                effective.paths.push((pattern.clone(), targets));
                effective.path_bases.push(declaration_base.clone());
            }
        }
    }
    effective.config_dir = config_dir.to_path_buf();
    effective.project_root = project_root.to_path_buf();
    effective.config_files.push(config_path);
    effective.config_files.sort();
    effective.config_files.dedup();
    visiting.remove(&cycle_key);
    (effective, problem)
}

fn empty_tsconfig(project_root: &Path) -> TsConfigPaths {
    TsConfigPaths {
        config_dir: project_root.to_path_buf(),
        base_url: None,
        paths: Vec::new(),
        path_bases: Vec::new(),
        config_files: Vec::new(),
        project_root: project_root.to_path_buf(),
    }
}

fn resolve_extends_config(specifier: &str, config_dir: &Path) -> Option<PathBuf> {
    let path = Path::new(specifier);
    if path.is_absolute() || specifier.starts_with('.') {
        return config_file_candidate(&config_dir.join(path));
    }

    let mut current = Some(config_dir);
    while let Some(directory) = current {
        let package_candidate = directory.join("node_modules").join(path);
        if let Some(candidate) = config_file_candidate(&package_candidate) {
            return Some(candidate);
        }
        if package_candidate.is_dir()
            && let Ok(package_source) = fs::read_to_string(package_candidate.join("package.json"))
            && let Ok(package) = serde_json::from_str::<serde_json::Value>(&package_source)
        {
            let entry = package
                .get("tsconfig")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tsconfig.json");
            if let Some(candidate) = config_file_candidate(&package_candidate.join(entry)) {
                return Some(candidate);
            }
        }
        current = directory.parent();
    }
    None
}

fn config_file_candidate(path: &Path) -> Option<PathBuf> {
    [
        path.to_path_buf(),
        path.with_extension("json"),
        path.join("tsconfig.json"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn alias_pattern_order(left: &str, right: &str) -> std::cmp::Ordering {
    let rank = |pattern: &str| {
        let wildcard = pattern.find('*');
        (
            wildcard.is_none(),
            wildcard.unwrap_or(pattern.len()),
            wildcard.map_or(0, |index| pattern.len().saturating_sub(index + 1)),
            pattern.len(),
        )
    };
    rank(right).cmp(&rank(left)).then_with(|| left.cmp(right))
}

fn match_alias_pattern<'a>(pattern: &str, specifier: &'a str) -> Option<&'a str> {
    let Some(star) = pattern.find('*') else {
        return (pattern == specifier).then_some("");
    };
    if pattern[star + 1..].contains('*') {
        return None;
    }
    let (prefix, remainder) = pattern.split_at(star);
    let suffix = &remainder[1..];
    specifier
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(suffix))
}

fn is_reserved_alias_specifier(specifier: &str) -> bool {
    matches!(specifier, "react" | "react-dom" | "ruvyxa")
        || specifier.starts_with("react/")
        || specifier.starts_with("react-dom/")
        || specifier.starts_with("ruvyxa/")
        || specifier.starts_with("@ruvyxa/")
}

fn path_is_inside(path: &Path, project_root: &Path) -> bool {
    project_root.as_os_str().is_empty()
        || ruvyxa_diagnostics::normalized_canonical_path(path)
            .starts_with(ruvyxa_diagnostics::normalized_canonical_path(project_root))
}

/// Minimally parse tsconfig.json to extract `compilerOptions.baseUrl` and
/// `compilerOptions.paths` without pulling in a full JSON parser.
///
/// We use `serde_json` which is already in scope.
/// Parse tsconfig text as JSONC, keeping the error for the caller to report.
fn parse_jsonc(content: &str) -> std::result::Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(&strip_json_comments(content))
}

/// Read `compilerOptions.baseUrl` and `compilerOptions.paths` from parsed JSON.
///
/// `None` means the file declares no compiler options at all, which is a valid
/// config rather than a failure — the caller keeps looking at the next candidate.
#[cfg(test)]
fn paths_from_value(value: &serde_json::Value, project_root: &Path) -> Option<TsConfigPaths> {
    paths_from_value_at(value, &project_root.join("tsconfig.json"), project_root)
}

#[cfg(test)]
fn paths_from_value_at(
    value: &serde_json::Value,
    config_path: &Path,
    project_root: &Path,
) -> Option<TsConfigPaths> {
    let compiler_options = value.get("compilerOptions")?;
    let config_dir = config_path.parent().unwrap_or(project_root);

    let base_url = compiler_options
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .map(|s| {
            let p = Path::new(s);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                config_dir.join(p)
            }
        });

    let mut paths: Vec<(String, Vec<String>)> = Vec::new();
    let mut path_bases = Vec::new();

    if let Some(paths_obj) = compiler_options.get("paths").and_then(|v| v.as_object()) {
        for (pattern, targets) in paths_obj {
            if let Some(arr) = targets.as_array() {
                let target_strs: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(ToString::to_string)
                    .collect();
                paths.push((pattern.clone(), target_strs));
                path_bases.push(base_url.clone().unwrap_or_else(|| config_dir.to_path_buf()));
            }
        }
    }

    Some(TsConfigPaths {
        config_dir: config_dir.to_path_buf(),
        base_url,
        paths,
        path_bases,
        config_files: vec![config_path.to_path_buf()],
        project_root: project_root.to_path_buf(),
    })
}

/// Rewrite JSONC into JSON that `serde_json` accepts.
///
/// TypeScript reads `tsconfig.json` as JSONC, which adds three things to JSON:
/// `//` comments, `/* */` comments, and trailing commas. This handled only the
/// first. The other two are not exotic — `tsc --init` emits a file whose every
/// option is documented in `/* */` blocks — and the cost of missing them was
/// silent: `parse_tsconfig_paths` turns any parse failure into "this project
/// declares no aliases", so a perfectly valid tsconfig stopped contributing
/// `baseUrl` and `paths` and every aliased import failed to resolve with no
/// mention of the config that was skipped.
///
/// Comment bodies collapse to nothing but their newlines are kept, so a parse
/// error still points at the line the user wrote.
fn strip_json_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_string {
            if ch == '\\' {
                out.push(ch);
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else {
                if ch == '"' {
                    in_string = false;
                }
                out.push(ch);
            }
        } else {
            match ch {
                '"' => {
                    in_string = true;
                    out.push(ch);
                }
                '/' if chars.peek() == Some(&'/') => {
                    // Line comment — consume until newline.
                    for c in chars.by_ref() {
                        if c == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    let mut previous = '\0';
                    for c in chars.by_ref() {
                        if previous == '*' && c == '/' {
                            break;
                        }
                        // Keep the line count so reported positions stay honest.
                        if c == '\n' {
                            out.push('\n');
                        }
                        previous = c;
                    }
                }
                _ => out.push(ch),
            }
        }
    }

    strip_trailing_commas(&out)
}

/// Drop the comma in `[1, 2, ]` and `{"a": 1, }`, which JSONC allows.
///
/// Runs after comment removal so a comma separated from its bracket only by a
/// comment is still recognized as trailing.
fn strip_trailing_commas(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut chars = input.chars().peekable();
    // Index in `out` of a comma that has seen only whitespace since.
    let mut pending_comma: Option<usize> = None;

    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if ch == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                pending_comma = None;
                out.push(ch);
            }
            ',' => {
                pending_comma = Some(out.len());
                out.push(ch);
            }
            ']' | '}' => {
                if let Some(comma) = pending_comma.take() {
                    out.remove(comma);
                }
                out.push(ch);
            }
            _ => {
                if !ch.is_whitespace() {
                    pending_comma = None;
                }
                out.push(ch);
            }
        }
    }

    out
}

// ─────────────────────────────────────────────────────────────────────────────
// package.json `exports` field support
// ─────────────────────────────────────────────────────────────────────────────

/// Attempt to resolve a bare package specifier (e.g. `"react/server"`) using
/// the package's `package.json` `exports` map.
///
#[derive(Debug, PartialEq, Eq)]
enum PackageExportsResolution {
    Unavailable,
    Blocked,
    Resolved(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
enum ExportTargets {
    Targets(Vec<String>),
    Blocked,
    Unmatched,
}

/// Minimal JSON representation that preserves object declaration order.
/// Conditional `exports` keys are evaluated in declaration order by Node, so
/// `serde_json::Value`'s default sorted map representation is not sufficient.
#[derive(Debug, PartialEq)]
enum PackageJsonValue {
    Null,
    String(String),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
    Unsupported,
}

impl<'de> Deserialize<'de> for PackageJsonValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(PackageJsonVisitor)
    }
}

struct PackageJsonVisitor;

impl<'de> Visitor<'de> for PackageJsonVisitor {
    type Value = PackageJsonValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(PackageJsonValue::Null)
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(PackageJsonValue::Null)
    }

    fn visit_bool<E>(self, _value: bool) -> std::result::Result<Self::Value, E> {
        Ok(PackageJsonValue::Unsupported)
    }

    fn visit_i64<E>(self, _value: i64) -> std::result::Result<Self::Value, E> {
        Ok(PackageJsonValue::Unsupported)
    }

    fn visit_u64<E>(self, _value: u64) -> std::result::Result<Self::Value, E> {
        Ok(PackageJsonValue::Unsupported)
    }

    fn visit_f64<E>(self, _value: f64) -> std::result::Result<Self::Value, E> {
        Ok(PackageJsonValue::Unsupported)
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(PackageJsonValue::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(PackageJsonValue::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element()? {
            values.push(value);
        }
        Ok(PackageJsonValue::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }
        Ok(PackageJsonValue::Object(entries))
    }
}

/// Extract the entry-point fields we care about from a `package.json`.
fn parse_package_manifest(content: &str) -> Option<PackageManifest> {
    let PackageJsonValue::Object(fields) =
        serde_json::from_str::<PackageJsonValue>(content).ok()?
    else {
        return None;
    };

    let mut manifest = PackageManifest::default();
    for (field, value) in fields {
        match (field.as_str(), value) {
            ("exports", value) => manifest.exports = Some(value),
            // `browser` also has an object form (a per-file substitution map).
            // Only the string form names an entry point; the map form is
            // deliberately ignored rather than half-honoured.
            ("browser", PackageJsonValue::String(path)) => manifest.browser = Some(path),
            ("module", PackageJsonValue::String(path)) => manifest.module = Some(path),
            ("main", PackageJsonValue::String(path)) => manifest.main = Some(path),
            _ => {}
        }
    }
    Some(manifest)
}

/// Resolve a bare package specifier the way Node does: walk `node_modules`
/// directories upward from the importer, and inside the first matching package
/// consult `exports`, then the legacy `browser`/`module`/`main` fields, then
/// `index`.
///
/// The upward walk is what makes non-hoisted installs work. Under pnpm a
/// transitive dependency lives beside its dependent inside the store
/// (`.pnpm/react-dom@19/node_modules/scheduler`) and never appears in the
/// project's own `node_modules`; resolving only against the project root made
/// every such package unresolvable, which the client linker then turned into a
/// `RUV1610` throw that fired at runtime.
fn resolve_node_modules_specifier(
    cache: &ResolveGraphCache,
    importer_dir: &Path,
    project_root: &Path,
    specifier: &str,
    target: BundleTarget,
) -> PackageExportsResolution {
    let Some((pkg_name, export_key)) = package_name_and_export_key(specifier) else {
        return PackageExportsResolution::Unavailable;
    };

    for modules_dir in node_modules_candidates(importer_dir, project_root) {
        let pkg_dir = modules_dir.join(&pkg_name);
        let Some(manifest) = cache.package_manifest(&pkg_dir.join("package.json")) else {
            // No manifest here. A bare directory can still satisfy a deep
            // subpath import (`pkg/dist/thing.js`); anything else means this
            // is not the package and the walk continues.
            if export_key != "."
                && let Some(path) = resolve_package_relative(&pkg_dir, &export_key)
            {
                return PackageExportsResolution::Resolved(path);
            }
            continue;
        };

        // The nearest package with this name wins, exactly as in Node: once a
        // manifest is found the walk stops, successfully or not.
        if let Some(exports) = manifest.exports.as_ref() {
            match resolve_exports_entry(exports, &export_key, target) {
                ExportTargets::Blocked => return PackageExportsResolution::Blocked,
                ExportTargets::Targets(targets) => {
                    return targets
                        .into_iter()
                        .find_map(|entry| resolve_export_target(&pkg_dir, &entry))
                        .map(PackageExportsResolution::Resolved)
                        .unwrap_or(PackageExportsResolution::Unavailable);
                }
                // An `exports` map that does not cover this subpath falls
                // through to the legacy fields rather than failing outright.
                ExportTargets::Unmatched => {}
            }
        }

        return resolve_legacy_entry(&pkg_dir, &manifest, &export_key, target)
            .map(PackageExportsResolution::Resolved)
            .unwrap_or(PackageExportsResolution::Unavailable);
    }

    PackageExportsResolution::Unavailable
}

#[cfg(test)]
fn resolve_package_exports(
    cache: &ResolveGraphCache,
    project_root: &Path,
    specifier: &str,
    target: BundleTarget,
) -> PackageExportsResolution {
    resolve_node_modules_specifier(cache, project_root, project_root, specifier, target)
}

/// `node_modules` directories to probe, nearest importer first.
///
/// Mirrors Node's `NODE_MODULES_PATHS`: every ancestor of the importer that is
/// not itself named `node_modules` contributes `<ancestor>/node_modules`. The
/// project root's own chain is appended so a package installed only at the
/// root still resolves for an importer that lives outside the root — a pnpm
/// store on another path, for example.
fn node_modules_candidates(importer_dir: &Path, project_root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();

    for dir in [importer_dir, project_root] {
        let mut current = Some(dir);
        while let Some(path) = current {
            if path.file_name().is_none_or(|name| name != "node_modules") {
                let candidate = path.join("node_modules");
                if seen.insert(candidate.clone()) {
                    candidates.push(candidate);
                }
            }
            current = path.parent();
        }
    }

    candidates
}

/// Resolve an entry point from the legacy `browser`/`module`/`main` fields, or
/// from the package's directory layout when none of them apply.
fn resolve_legacy_entry(
    pkg_dir: &Path,
    manifest: &PackageManifest,
    export_key: &str,
    target: BundleTarget,
) -> Option<PathBuf> {
    legacy_entry_candidates(manifest, export_key, target)
        .into_iter()
        .find_map(|field| resolve_package_relative(pkg_dir, &field))
}

/// Package-relative candidates a package offers when `exports` answers nothing.
///
/// Split out from the probe so `tests/fixtures/module-resolution-conformance.json`
/// can compare the *order* against the JavaScript graph without needing a
/// package on disk. `legacyEntryCandidates` in
/// `packages/ruvyxa/runtime/package-exports.mjs` is the other half.
fn legacy_entry_candidates(
    manifest: &PackageManifest,
    export_key: &str,
    target: BundleTarget,
) -> Vec<String> {
    if export_key != "." {
        return vec![strip_dot_slash(export_key).to_string()];
    }

    let mut fields: Vec<&str> = Vec::new();
    // `browser` only wins for browser bundles; on the server it would swap in
    // a build that assumes `window`.
    if target == BundleTarget::Client
        && let Some(browser) = manifest.browser.as_deref()
    {
        fields.push(browser);
    }
    if let Some(module) = manifest.module.as_deref() {
        fields.push(module);
    }
    if let Some(main) = manifest.main.as_deref() {
        fields.push(main);
    }
    fields.push("index");

    fields
        .into_iter()
        .map(|field| strip_dot_slash(field).to_string())
        .collect()
}

fn strip_dot_slash(value: &str) -> &str {
    value.strip_prefix("./").unwrap_or(value)
}

/// Join a package-relative path and probe it, refusing anything that escapes
/// the package directory.
fn resolve_package_relative(pkg_dir: &Path, relative: &str) -> Option<PathBuf> {
    let relative = relative.strip_prefix("./").unwrap_or(relative);
    if relative.is_empty() || relative.contains('\\') {
        return None;
    }
    let relative = Path::new(relative);
    if !relative
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }

    let resolved = resolve_file_candidate(&pkg_dir.join(relative))?;
    let package_root = ruvyxa_diagnostics::normalized_canonical_path(pkg_dir);
    let candidate = ruvyxa_diagnostics::normalized_canonical_path(&resolved);
    candidate.starts_with(package_root).then_some(candidate)
}

fn resolve_export_target(pkg_dir: &Path, target: &str) -> Option<PathBuf> {
    let relative = target.strip_prefix("./")?;
    if relative.is_empty() || relative.contains('\\') {
        return None;
    }
    let relative = Path::new(relative);
    if !relative
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }

    // Canonicalize to verify existence, then strip Windows verbatim prefixes so
    // module paths compare equal across every resolver branch (shared-route
    // chunk planning keys on these paths).
    //
    // The existence probe and the compared path are the same canonicalization,
    // so it is taken once. `normalized_canonical_path` cannot stand in for the
    // probe: it falls back to its argument when canonicalization fails, which
    // would admit a subpath that does not exist.
    let canonical = pkg_dir.join(relative).canonicalize().ok()?;
    let candidate = ruvyxa_diagnostics::without_verbatim_prefix(&canonical);
    let package_root = ruvyxa_diagnostics::normalized_canonical_path(pkg_dir);
    candidate.starts_with(package_root).then_some(candidate)
}

/// Split a package specifier into the package directory name and `exports` key.
///
/// Examples:
/// - `react` -> (`react`, `.`)
/// - `react/jsx-runtime` -> (`react`, `./jsx-runtime`)
/// - `@scope/pkg` -> (`@scope/pkg`, `.`)
/// - `@scope/pkg/sub/path` -> (`@scope/pkg`, `./sub/path`)
fn package_name_and_export_key(specifier: &str) -> Option<(String, String)> {
    if specifier.is_empty() || specifier.starts_with('.') || specifier.starts_with('/') {
        return None;
    }

    if specifier.starts_with('@') {
        let mut parts = specifier.splitn(3, '/');
        let scope = parts.next()?;
        let name = parts.next()?;
        let subpath = parts.next();
        let pkg_name = format!("{scope}/{name}");
        let export_key = subpath
            .filter(|s| !s.is_empty())
            .map(|s| format!("./{s}"))
            .unwrap_or_else(|| ".".to_string());
        return Some((pkg_name, export_key));
    }

    let (pkg_name, export_key) = if let Some((name, subpath)) = specifier.split_once('/') {
        (name.to_string(), format!("./{subpath}"))
    } else {
        (specifier.to_string(), ".".to_string())
    };

    Some((pkg_name, export_key))
}

/// Walk a Node-style exports map for a requested subpath and bundle target.
fn resolve_exports_entry(
    exports: &PackageJsonValue,
    key: &str,
    target: BundleTarget,
) -> ExportTargets {
    match exports {
        PackageJsonValue::Null => ExportTargets::Blocked,
        PackageJsonValue::String(_) | PackageJsonValue::Array(_) => {
            if key == "." {
                resolve_exports_value(exports, target, None)
            } else {
                ExportTargets::Unmatched
            }
        }
        PackageJsonValue::Object(map) => {
            if map.iter().any(|(entry, _)| entry.starts_with('.')) {
                resolve_exports_subpath(map, key, target)
            } else if key == "." {
                resolve_exports_value(exports, target, None)
            } else {
                // A map with no `.`-prefixed key is sugar for `{ ".": <map> }`:
                // it defines the root entry and nothing else. Answering a
                // subpath from it resolved `pkg/sub` to the package's *root*
                // file — a wrong file, silently — where leaving it unmatched
                // falls through to the legacy branch and probes `pkg/sub`
                // itself. Node refuses the subpath outright here
                // (`ERR_PACKAGE_PATH_NOT_EXPORTED`); falling through is the
                // documented divergence, taking the wrong file was a defect.
                ExportTargets::Unmatched
            }
        }
        PackageJsonValue::Unsupported => ExportTargets::Unmatched,
    }
}

fn resolve_exports_subpath(
    map: &[(String, PackageJsonValue)],
    key: &str,
    target: BundleTarget,
) -> ExportTargets {
    if let Some((_, value)) = map.iter().find(|(entry, _)| entry == key) {
        return resolve_exports_value(value, target, None);
    }

    map.iter()
        .filter_map(|(pattern, value)| {
            let (prefix, suffix) = pattern.split_once('*')?;
            if pattern.matches('*').count() != 1
                || !key.starts_with(prefix)
                || !key.ends_with(suffix)
                || key.len() < prefix.len() + suffix.len()
            {
                return None;
            }
            let wildcard = &key[prefix.len()..key.len() - suffix.len()];
            Some((prefix.len(), suffix.len(), wildcard, value))
        })
        .max_by_key(|(prefix_len, suffix_len, _, _)| (*prefix_len, *suffix_len))
        .map_or(ExportTargets::Unmatched, |(_, _, wildcard, value)| {
            resolve_exports_value(value, target, Some(wildcard))
        })
}

fn resolve_exports_value(
    value: &PackageJsonValue,
    target: BundleTarget,
    wildcard: Option<&str>,
) -> ExportTargets {
    match value {
        PackageJsonValue::Null => ExportTargets::Blocked,
        PackageJsonValue::String(path) => {
            let path = wildcard
                .map(|wildcard| path.replace('*', wildcard))
                .unwrap_or_else(|| path.clone());
            if path.starts_with("./") {
                ExportTargets::Targets(vec![path])
            } else {
                ExportTargets::Unmatched
            }
        }
        PackageJsonValue::Array(values) => {
            let mut targets = Vec::new();
            for value in values {
                match resolve_exports_value(value, target, wildcard) {
                    ExportTargets::Targets(mut candidates) => targets.append(&mut candidates),
                    ExportTargets::Blocked if targets.is_empty() => return ExportTargets::Blocked,
                    ExportTargets::Blocked | ExportTargets::Unmatched => {}
                }
            }
            if targets.is_empty() {
                ExportTargets::Unmatched
            } else {
                ExportTargets::Targets(targets)
            }
        }
        PackageJsonValue::Object(map) => {
            // Node matches conditions in the order the package author wrote
            // them, and the first supported one wins. `require` is therefore
            // not in the same pass as the rest: a package that lists it first —
            //
            //   "exports": { ".": { "require": "./cjs.js", "import": "./esm.mjs" } }
            //
            // handed its CommonJS build to a browser bundle that had an ESM
            // build sitting right beside it, because author order put `require`
            // ahead of `import` and both were equally acceptable. Ruvyxa emits
            // ESM, so `require` is a fallback for packages that ship nothing
            // else, not a peer of `import`.
            //
            // Two passes rather than a reordered list, because reordering would
            // break author order for the conditions that legitimately compete
            // (`browser` before `import`, say). Within each pass the author's
            // order still decides.
            let (preferred, fallback): (&[&str], &[&str]) = match target {
                BundleTarget::Client => (&["browser", "import", "module", "default"], &["require"]),
                BundleTarget::Ssr => (&["node", "import", "module", "default"], &["require"]),
                // `react-server` first, ahead of `node`, because a package that
                // ships a server-components build lists it as a narrower case
                // of the same runtime — React's own `exports` does exactly
                // that, and taking `node` instead would load the build with
                // `useState` in it and make every server component throw.
                BundleTarget::ReactServer => (
                    &["react-server", "node", "import", "module", "default"],
                    &["require"],
                ),
                BundleTarget::Edge => (
                    &["worker", "edge-light", "import", "module", "default"],
                    &[],
                ),
            };

            for conditions in [preferred, fallback] {
                for (condition, value) in map {
                    if conditions.contains(&condition.as_str()) {
                        let resolved = resolve_exports_value(value, target, wildcard);
                        if !matches!(resolved, ExportTargets::Unmatched) {
                            return resolved;
                        }
                    }
                }
            }
            ExportTargets::Unmatched
        }
        PackageJsonValue::Unsupported => ExportTargets::Unmatched,
    }
}

/// Read a source file into an owned `String`.
///
/// Files above 64 KiB used to take a memory-mapped path, on the stated grounds
/// that mapping is "zero-copy … and avoids a full heap allocation + copy". It
/// was not: the mapped bytes were immediately `to_vec()`-ed and then UTF-8
/// validated, so the allocation and the copy both happened anyway, on top of
/// the `mmap`/`munmap` syscalls. It also made every source read an `unsafe`
/// one — a mapped file that is truncated by the editor mid-build faults the
/// process rather than returning a short read, and `dev` watches files while
/// they are being saved.
///
/// A plain read is one sized allocation, one read, one validation. Reach for
/// mapping again only with a measurement showing it wins, and only if the
/// borrowed bytes can reach the scanner without an intermediate copy — that is
/// the version that would actually be zero-copy.
fn read_source_fast(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|error| {
        BundleError::Io(std::io::Error::new(
            error.kind(),
            format!("{}: {}", path.display(), error),
        ))
    })
}

/// Walk the import graph using a shared resolver/source cache.
///
/// Uses a parallel BFS strategy: after the initial entry is resolved, each
/// "frontier" (set of newly-discovered deps) is resolved concurrently via
/// rayon. This exploits independent subtrees where modules don't share
/// resolution state.
pub fn resolve_graph_with_cache(
    entry_source: &str,
    entry_label: &str,
    project_root: &Path,
    app_dir: &Path,
    cache: &ResolveGraphCache,
) -> Result<Vec<ResolvedModule>> {
    resolve_graph_with_hooks(
        entry_source,
        entry_label,
        project_root,
        app_dir,
        cache,
        &BuildHookPipeline::empty(),
        BundleTarget::Client,
        JsxRuntime::Automatic,
    )
}

/// Walk the import graph using a shared resolver/source cache and TypeScript build hooks.
#[allow(clippy::too_many_arguments)]
pub fn resolve_graph_with_hooks(
    entry_source: &str,
    entry_label: &str,
    project_root: &Path,
    _app_dir: &Path,
    cache: &ResolveGraphCache,
    build_hooks: &BuildHookPipeline,
    target: BundleTarget,
    jsx_runtime: JsxRuntime,
) -> Result<Vec<ResolvedModule>> {
    resolve_graph_with_incremental(
        entry_source,
        entry_label,
        project_root,
        _app_dir,
        cache,
        build_hooks,
        target,
        jsx_runtime,
        None,
    )
}

/// Walk the import graph with optional persistent dependency-edge reuse.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_graph_with_incremental(
    entry_source: &str,
    entry_label: &str,
    project_root: &Path,
    _app_dir: &Path,
    cache: &ResolveGraphCache,
    build_hooks: &BuildHookPipeline,
    target: BundleTarget,
    jsx_runtime: JsxRuntime,
    incremental: Option<&IncrementalGraphCache>,
) -> Result<Vec<ResolvedModule>> {
    let project_root = ruvyxa_diagnostics::normalized_canonical_path(project_root);
    // A graph is resolved against a single configuration snapshot. Keeping it
    // local to this run avoids repeated I/O and parsing for every module.
    let tsconfig = cache.tsconfig_paths(&project_root);

    let mut visited: BTreeMap<PathBuf, ResolvedModule> = BTreeMap::new();
    let mut order: Vec<PathBuf> = Vec::new();
    let mut visited_set: BTreeSet<PathBuf> = BTreeSet::new();

    // Virtual entry — synthetic key that won't collide with real files.
    let entry_key = PathBuf::from(entry_label);

    // Phase 1: Resolve the entry module (always sequential — it's a single node).
    let entry_deps = collect_deps_cached(
        entry_source,
        // The synthetic entry is generated JSX with no file behind it, so it has
        // no extension to read. `tsx` names what it is; the seed still waits on
        // the source guess, so an entry with no element seeds nothing.
        "tsx",
        &project_root,
        &project_root,
        &tsconfig,
        cache,
        build_hooks,
        target,
        jsx_runtime,
    )?;

    order.push(entry_key.clone());
    visited_set.insert(entry_key.clone());
    visited.insert(
        entry_key.clone(),
        ResolvedModule {
            path: entry_key.clone(),
            source: entry_source.to_string(),
            compiled_content: None,
            load_source_map: None,
            deps: entry_deps.paths.clone(),
            dependency_aliases: entry_deps.aliases,
            watch_paths: Vec::new(),
            glob_matches: Vec::new(),
            is_external: false,
        },
    );

    // Phase 2: Parallel BFS — resolve frontier layers concurrently.
    let mut frontier: Vec<PathBuf> = entry_deps
        .paths
        .into_iter()
        .filter(|dep| visited_set.insert(dep.clone()))
        .collect();

    while !frontier.is_empty() {
        // Parallel resolve: read sources and extract deps for all frontier nodes.
        let resolved_frontier: Vec<Result<(PathBuf, ResolvedModule)>> = frontier
            .par_iter()
            .map(|dep_path| {
                let is_external = matches!(target, BundleTarget::Ssr | BundleTarget::Edge)
                    && dep_path
                        .components()
                        .any(|c| c.as_os_str() == "node_modules");

                let hook_context = BuildHookContext {
                    project_root: project_root.clone(),
                    importer: None,
                    target,
                };
                let loaded = build_hooks.load(dep_path, &hook_context)?;
                // `Arc<str>` because the module's text is needed twice — once
                // as the compiled module body, once as the fingerprint the
                // incremental cache records — and the two do not have to be
                // separate allocations.
                let raw_source: Arc<str> = match &loaded {
                    Some(output) => Arc::from(output.code.as_str()),
                    None => cache.read_source(dep_path)?,
                };
                let load_source_map = loaded.and_then(|output| output.map);
                // A JSON module is data, not code: it is never rewritten and
                // never scanned for imports. Folding `process.env.NODE_ENV` into
                // it, or reading `require(` out of one of its string values,
                // would corrupt the document or invent dependencies.
                let is_json = matches!(
                    dep_path.extension().and_then(|extension| extension.to_str()),
                    Some(extension) if extension.eq_ignore_ascii_case("json")
                );
                let source = if !is_json
                    && target == BundleTarget::Client
                    && dep_path
                        .components()
                        .any(|component| component.as_os_str() == "node_modules")
                {
                    minifier::fold_production_node_env(&raw_source)
                } else {
                    raw_source.to_string()
                };

                // Configured applications compile content once in the
                // persistent MDX host and reuse the result for dependency
                // scanning and code generation. Direct bundler consumers keep
                // the native compiler fallback.
                let is_content = matches!(
                    dep_path
                        .extension()
                        .and_then(|extension| extension.to_str()),
                    Some("md" | "mdx")
                );
                let glob_expansion = if is_json || is_content || is_external {
                    crate::glob_import::GlobExpansion {
                        source,
                        ..Default::default()
                    }
                } else {
                    let resolve_base = dep_path.parent().unwrap_or(&project_root);
                    crate::glob_import::expand_import_meta_glob(
                        &source,
                        resolve_base,
                        &project_root,
                        cache,
                        |pattern| tsconfig.resolve_glob_pattern(pattern),
                    )?
                };
                let source = glob_expansion.source;
                let compiled_content = if is_content {
                    if let Some(output) =
                        build_hooks.compile_content(&source, dep_path, &hook_context)?
                    {
                        Some(Arc::from(output.code))
                    } else {
                        Some(
                            crate::content::compile_content_module_shared_in_root(
                                &source,
                                dep_path,
                                &project_root,
                            )
                            .map_err(BundleError::Compiler)?,
                        )
                    }
                } else {
                    None
                };

                // Reuse a persisted resolution only when it is complete. Paths
                // and aliases are one answer: the linker consults the alias map
                // first and only then matches by path suffix, and an alias like
                // `~/components/Button` shares no suffix with its target. Taking
                // the paths while defaulting the aliases to empty would make a
                // warm build resolve differently from a cold one, so an entry
                // that never recorded aliases is resolved fresh instead.
                let reusable_dependencies = if glob_expansion.watch_roots.is_empty()
                    && target == BundleTarget::Client
                    && build_hooks.host_count() == 0
                {
                    incremental.and_then(|cache| {
                        if cache.check_freshness(dep_path, &raw_source) != FreshnessStatus::Fresh {
                            return None;
                        }
                        let paths = cache.cached_deps(dep_path)?.to_vec();
                        let aliases = cache.cached_aliases(dep_path)?.clone();
                        cache.record_edge_hit();
                        Some(ResolvedDependencies { paths, aliases })
                    })
                } else {
                    None
                };

                let dependencies = if is_external || is_json {
                    ResolvedDependencies::default()
                } else if let Some(reused) = reusable_dependencies {
                    reused
                } else {
                    let resolve_base = dep_path.parent().unwrap_or(&project_root).to_path_buf();
                    // Only content files need a compiled stand-in to scan for
                    // imports; everything else scans the source it already has.
                    // Materializing this as an owned `String` in both arms made
                    // the common arm clone the whole module for no reason —
                    // the branch that has to own was deciding the shape for the
                    // branch that only has to borrow. The content arm keeps the
                    // shared `Arc<str>` the content cache already holds instead
                    // of copying out of it.
                    let dependency_source: &str =
                        compiled_content.as_deref().unwrap_or(source.as_str());
                    collect_deps_cached(
                        dependency_source,
                        // The real file's extension, even when the source being
                        // scanned is a compiled stand-in: a `.md` module's
                        // stand-in is JSX, and `.md` is one of the extensions
                        // that leaves the answer to the source.
                        dep_path
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .unwrap_or(""),
                        &resolve_base,
                        &project_root,
                        &tsconfig,
                        cache,
                        build_hooks,
                        target,
                        jsx_runtime,
                    )?
                };

                if target == BundleTarget::Client
                    && build_hooks.host_count() == 0
                    && let Some(incremental) = incremental
                {
                    incremental.record_module(
                        dep_path.clone(),
                        &raw_source,
                        dependencies.paths.clone(),
                        dependencies.aliases.clone(),
                    );
                }

                Ok((
                    dep_path.clone(),
                    ResolvedModule {
                        path: dep_path.clone(),
                        source,
                        compiled_content,
                        load_source_map,
                        deps: dependencies.paths,
                        dependency_aliases: dependencies.aliases,
                        watch_paths: glob_expansion.watch_roots,
                        glob_matches: glob_expansion.matches,
                        is_external,
                    },
                ))
            })
            .collect();

        // Collect results and build the next frontier.
        let mut next_frontier: Vec<PathBuf> = Vec::new();

        for result in resolved_frontier {
            let (path, module) = result?;
            // Collect new deps for the next frontier.
            for dep in &module.deps {
                if visited_set.insert(dep.clone()) {
                    next_frontier.push(dep.clone());
                }
            }
            order.push(path.clone());
            visited.insert(path, module);
        }

        frontier = next_frontier;
    }

    Ok(order
        .into_iter()
        .filter_map(|path| visited.remove(&path))
        .collect())
}

/// Resolve dependencies using the lock-free resolution cache.
///
/// Specifiers within a single module are resolved sequentially (they share
/// the same base_dir and are typically few), but the cache lookups are
/// contention-free thanks to DashMap's sharded design.
#[allow(clippy::too_many_arguments)]
fn collect_deps_cached(
    source: &str,
    extension: &str,
    base_dir: &Path,
    project_root: &Path,
    tsconfig: &TsConfigPaths,
    cache: &ResolveGraphCache,
    build_hooks: &BuildHookPipeline,
    target: BundleTarget,
    jsx_runtime: JsxRuntime,
) -> Result<ResolvedDependencies> {
    if build_hooks.host_count() == 0 {
        let key = DependencyCacheKey {
            base_dir: Arc::from(base_dir.to_string_lossy().as_ref()),
            source_hash: *blake3::hash(source.as_bytes()).as_bytes(),
            target: match target {
                BundleTarget::Client => 0,
                BundleTarget::Ssr => 1,
                BundleTarget::Edge => 2,
                BundleTarget::ReactServer => 3,
            },
            jsx_automatic: matches!(jsx_runtime, JsxRuntime::Automatic),
            // Asked with `true` because the seed below is already conditioned on
            // the source guess; what is left for the extension to decide is
            // whether the transform would honour it. Two files in one directory
            // holding identical bytes under `.ts` and `.tsx` hash the same, and
            // this is what keeps them apart.
            jsx_allowed_by_extension: crate::compiler::jsx_is_enabled(extension, true),
        };
        if let Some(cached) = cache.dependencies.get(&key) {
            // Both halves, always. Returning the paths under an empty alias map
            // would leave every `tsconfig` alias in this module unresolvable for
            // every route after the first one to scan it.
            return Ok(ResolvedDependencies::clone(cached.value()));
        }
        let dependencies = Arc::new(collect_deps_uncached(
            source,
            extension,
            base_dir,
            project_root,
            tsconfig,
            cache,
            build_hooks,
            target,
            jsx_runtime,
        )?);
        cache.dependencies.insert(key, Arc::clone(&dependencies));
        return Ok(ResolvedDependencies::clone(&dependencies));
    }

    collect_deps_uncached(
        source,
        extension,
        base_dir,
        project_root,
        tsconfig,
        cache,
        build_hooks,
        target,
        jsx_runtime,
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_deps_uncached(
    source: &str,
    extension: &str,
    base_dir: &Path,
    project_root: &Path,
    tsconfig: &TsConfigPaths,
    cache: &ResolveGraphCache,
    build_hooks: &BuildHookPipeline,
    target: BundleTarget,
    jsx_runtime: JsxRuntime,
) -> Result<ResolvedDependencies> {
    let parsed = ast::parse_module(source);
    let mut specifiers = parsed.import_specifiers();

    // The automatic JSX transform injects `import { jsx as _jsx } from
    // "react/jsx-runtime"` *after* this graph walk, so the specifier never
    // appears in the source being scanned. Seed the edge here; without it the
    // linker treats the injected import as an external package and emits a bare
    // specifier that no browser can resolve.
    //
    // Both conditions, and neither alone. `has_jsx` is the scanner's
    // `<`-heuristic, so `new Map<string, number>()` sets it — and gating on that
    // alone seeded the edge for plain-TypeScript modules the transform gives no
    // JSX to, because `jsx_is_enabled` refuses JSX for `.ts`/`.mts`/`.cts`
    // outright. Gating on the extension alone would seed it for every `.tsx`
    // module, element or not. The transform injects the import exactly when
    // both are true, and this is the same rule read from the same function.
    if matches!(jsx_runtime, JsxRuntime::Automatic)
        && parsed.has_jsx
        && crate::compiler::jsx_is_enabled(extension, parsed.has_jsx)
        && !specifiers.iter().any(|s| s == JSX_RUNTIME_SPECIFIER)
    {
        specifiers.push(JSX_RUNTIME_SPECIFIER.to_string());
    }

    let mut dependencies = ResolvedDependencies {
        paths: Vec::with_capacity(specifiers.len()),
        aliases: BTreeMap::new(),
    };
    let base_dir_str = base_dir.to_string_lossy();

    for specifier in specifiers {
        if is_non_js_asset_specifier(&specifier) && !is_css_module_specifier(&specifier) {
            continue;
        }

        let hook_context = BuildHookContext {
            project_root: project_root.to_path_buf(),
            importer: Some(base_dir.to_path_buf()),
            target,
        };
        let hook_resolved = build_hooks.resolve_id(&specifier, Some(base_dir), &hook_context)?;

        let resolved = if let Some(path) = hook_resolved {
            Some(path)
        } else if specifier.starts_with('.') {
            // Relative import: check resolution cache first (lock-free DashMap read).
            if let Some(cached) = cache.resolution(&base_dir_str, &specifier) {
                cached
            } else {
                let result = resolve_specifier(base_dir, &specifier);
                cache.insert_resolution(&base_dir_str, &specifier, result.clone());
                result
            }
        } else if specifier.starts_with('/') || Path::new(&specifier).is_absolute() {
            // Absolute path — framework-generated imports. The `is_absolute`
            // half is Windows: a generated entry names
            // `D:/project/app/page.tsx`, which carries a drive prefix instead of
            // a leading slash. `compiler.mjs` answers both through
            // `path.isAbsolute`, and until this branch did too, a drive-prefixed
            // path fell through to the package walk below and was only ever
            // resolved by the project-root probe that walk used to end in.
            resolve_project_specifier(project_root, &specifier)
        } else {
            // Non-relative specifier: `tsconfig` mappings (`paths`, then
            // `baseUrl`), then `node_modules`. Nothing sits between them — a
            // bare specifier names a package, exactly as Node and `tsc` read
            // it, and `baseUrl` is how a project asks for root-relative
            // resolution out loud. This used to probe the project root with the
            // bare specifier as well, which `compiler.mjs` never did: one
            // import took `<root>/utils/index.ts` into the client bundle while
            // the dev server and every prerender worker took
            // `node_modules/utils`. See the `resolutionOrder` section of
            // `tests/fixtures/module-resolution-conformance.json`.
            match tsconfig.resolve(&specifier) {
                Some(path) => Some(path),
                None => match resolve_node_modules_specifier(
                    cache,
                    base_dir,
                    project_root,
                    &specifier,
                    target,
                ) {
                    PackageExportsResolution::Resolved(path) => Some(path),
                    // Nothing answered. Dropping the specifier here would make
                    // removing the project-root probe a *silent* change for a
                    // project that relied on it — an unresolved external that
                    // no host reports and every host fails at run time. If a
                    // file at the project root would have answered, say so.
                    PackageExportsResolution::Blocked | PackageExportsResolution::Unavailable => {
                        if let Some(shadow) = resolve_project_specifier(project_root, &specifier)
                            && is_project_local(&shadow, project_root)
                        {
                            return Err(BundleError::Compiler(project_root_shadow_message(
                                &specifier, base_dir, &shadow,
                            )));
                        }
                        None
                    }
                },
            }
        };

        match resolved {
            Some(abs_path) => {
                // A relative specifier is the one spelling the author controls
                // and the one `base_dir` cannot distort — `base_dir` is itself
                // a resolved, canonical path, so a segment that differs only in
                // case came from this specifier and nothing above it.
                //
                // Package and alias specifiers stay out of scope on purpose:
                // pnpm reaches a package through a symlink farm, so the
                // canonical path there differs from the request for reasons
                // that have nothing to do with how anybody spelled it.
                if specifier.starts_with('.')
                    && let Some(mismatch) =
                        import_case_mismatch(&base_dir.join(&specifier), &abs_path)
                {
                    return Err(BundleError::Compiler(format!(
                        "RUV1807 import {specifier:?} from {} asks for {:?}, but the file on disk is \
                         named {:?}. This filesystem matches names case-insensitively, so the \
                         import resolves here and resolves nothing on a case-sensitive one — \
                         Linux CI, or the host the build is deployed to. Spell the import the \
                         way the file is named.",
                        base_dir.display(),
                        mismatch.requested,
                        mismatch.resolved
                    )));
                }

                if is_project_local(&abs_path, project_root) || target == BundleTarget::Client {
                    dependencies
                        .aliases
                        .insert(specifier.clone(), abs_path.clone());
                    dependencies.paths.push(abs_path);
                }
            }
            None => {
                if !specifier.starts_with('.') {
                    // Bare specifier that couldn't be resolved — treated as external.
                    continue;
                }
                return Err(BundleError::Unresolved {
                    specifier,
                    importer: base_dir.to_path_buf(),
                });
            }
        }
    }

    Ok(dependencies)
}

fn is_non_js_asset_specifier(specifier: &str) -> bool {
    let lower = specifier.to_ascii_lowercase();
    matches!(
        Path::new(&lower).extension().and_then(|ext| ext.to_str()),
        Some("css" | "scss" | "sass" | "less")
    )
}

fn is_css_module_specifier(specifier: &str) -> bool {
    crate::style_module::is_css_module_path(Path::new(
        specifier.split(['?', '#']).next().unwrap_or(specifier),
    ))
}

/// Extract all import/export specifier strings from source text.
///
/// This is a lightweight line-oriented scanner — not a full AST parse.  It
/// handles the common patterns used inside Ruvyxa projects.
#[cfg(test)]
fn extract_specifiers(source: &str) -> Vec<String> {
    ast::parse_module(source).import_specifiers()
}

/// Extract the string value between the first pair of quotes.
#[cfg(test)]
fn quoted_value(s: &str) -> Option<String> {
    let quote = s.chars().find(|c| *c == '"' || *c == '\'')?;
    let start = s.find(quote)? + 1;
    let rest = &s[start..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

/// Resolve a relative specifier like `"./utils"` to an absolute file path,
/// probing TypeScript/JavaScript extensions in priority order.
pub fn resolve_specifier(base_dir: &Path, specifier: &str) -> Option<PathBuf> {
    let joined = base_dir.join(specifier);
    resolve_file_candidate(&joined)
}

/// RUV1808: a bare specifier nothing answered, with a project file behind it.
///
/// The message names the file on purpose. The author wrote a package specifier
/// and meant a project file, and the two ways to say that — a relative import,
/// or `baseUrl`/`paths` in `tsconfig.json` — are both understood by `tsc`, by
/// the editor, and by both of Ruvyxa's module graphs. The
/// `packages/ruvyxa/runtime/compiler.mjs` half is
/// `unresolvedBareSpecifierMessage`.
fn project_root_shadow_message(specifier: &str, importer: &Path, shadow: &Path) -> String {
    format!(
        "RUV1808 import {specifier:?} from {} names no package, but {} exists. A bare specifier \
         names a package here, the way Node and TypeScript both read it. Import the project file \
         relatively, or declare `compilerOptions.baseUrl` (or a `paths` entry) in tsconfig.json so \
         the type checker and the bundler answer it the same way.",
        importer.display(),
        shadow.display()
    )
}

fn resolve_project_specifier(project_root: &Path, specifier: &str) -> Option<PathBuf> {
    let path = Path::new(specifier);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    resolve_file_candidate(&candidate)
}

/// One path segment an import spelled differently from the file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportCaseMismatch {
    /// The segment as the import spelled it.
    pub requested: String,
    /// The segment as the filesystem actually holds it.
    pub resolved: String,
}

/// Segments of `path`, with `.` dropped and `..` applied lexically.
///
/// Lexical rather than by `canonicalize`: the point is to compare what the
/// import *asked for* with what the filesystem *answered*, so the request must
/// not be run through the filesystem first.
fn lexical_segments(path: &Path) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => match value.to_str() {
                Some(text) => segments.push(text.to_string()),
                // A segment that is not UTF-8 cannot be compared as text, and
                // guessing at its case would be worse than saying nothing.
                None => return Vec::new(),
            },
            Component::ParentDir => {
                segments.pop();
            }
            Component::CurDir => {}
            // A prefix or root carries no spelling an import chose: a drive
            // letter's case is the shell's, not the source file's.
            Component::Prefix(_) | Component::RootDir => segments.clear(),
        }
    }
    segments
}

/// True when two segments are the same letters differing only in ASCII case.
///
/// ASCII-only on purpose. Case outside ASCII is decided by the host's locale
/// tables — the reason `localeCompare` and `toLocaleLowerCase` are banned in
/// this repository — so a non-ASCII difference is never reported rather than
/// reported on one machine and not another.
fn differs_only_by_ascii_case(requested: &str, resolved: &str) -> bool {
    requested.eq_ignore_ascii_case(resolved) && requested != resolved
}

/// Report the first segment an import spelled in a case the filesystem does not
/// hold.
///
/// `is_file()` answers case-insensitively on Windows and on default macOS, so
/// `import "./Header"` resolves `header.tsx` and the project builds. On Linux
/// the same import resolves nothing. The failure is therefore invisible on the
/// machine that writes it and arrives in CI or on the deployed host, so this
/// comparison exists to move it back to the author's build.
///
/// Pure string work over two paths the caller already holds: no syscall, and on
/// a case-sensitive filesystem it can never fire, because a mis-spelled import
/// does not resolve there in the first place.
///
/// Replays `tests/fixtures/import-case-conformance.json`, whose JavaScript half
/// is `importCaseMismatch` in `packages/ruvyxa/runtime/compiler.mjs`.
#[must_use]
pub fn import_case_mismatch(requested: &Path, resolved: &Path) -> Option<ImportCaseMismatch> {
    let requested_segments = lexical_segments(requested);
    let resolved_segments = lexical_segments(resolved);
    if requested_segments.is_empty() || resolved_segments.len() < requested_segments.len() {
        return None;
    }

    let last = requested_segments.len() - 1;
    for (index, requested_segment) in requested_segments.iter().enumerate() {
        let resolved_segment = &resolved_segments[index];

        // The resolver appends an extension, so the last requested segment can
        // be a prefix of what is on disk. Compare only the characters the
        // import actually spelled; anywhere else the segments are directory
        // names and must match whole.
        let comparable = if index == last && resolved_segment.len() > requested_segment.len() {
            let cut = requested_segment.len();
            if !resolved_segment.is_char_boundary(cut) {
                continue;
            }
            &resolved_segment[..cut]
        } else {
            resolved_segment.as_str()
        };

        if differs_only_by_ascii_case(requested_segment, comparable) {
            return Some(ImportCaseMismatch {
                requested: requested_segment.clone(),
                resolved: comparable.to_string(),
            });
        }
    }
    None
}

/// Extensions probed by **appending** to what the import wrote, in priority
/// order. Mirrors `JS_EXTENSIONS`/`PACKAGE_FILE_EXTENSIONS` in
/// `packages/ruvyxa/runtime/compiler.mjs`.
pub(crate) const PROBE_EXTENSIONS: [&str; 10] = [
    "ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs", "md", "mdx",
];

/// TypeScript sources a written extension may stand for, keyed by that
/// extension: `./x.js` names `x.ts` in a project whose TypeScript has not been
/// emitted. Mirrors `extensionFallbacks` in `resolveFile` (`compiler.mjs`).
///
/// Only these four are rewritten. Replacing the last dotted segment of anything
/// else asks for the wrong file and never asks for the right one:
/// `./util.inspect` becomes `util.js`, which does not exist, while
/// `util.inspect.js` — the file `object-inspect` actually ships, and the one
/// Node finds by appending — is never probed. Node appends; it does not
/// replace, and a basename with a dot in it is ordinary.
fn typescript_source_extensions(extension: &str) -> &'static [&'static str] {
    match extension {
        "js" => &["ts", "tsx", "jsx"],
        "mjs" => &["mts", "ts"],
        "cjs" => &["cts", "ts"],
        "jsx" => &["tsx"],
        _ => &[],
    }
}

/// `path` with `.extension` on the end — appended, never substituted.
fn with_appended_extension(path: &Path, extension: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".");
    name.push(extension);
    PathBuf::from(name)
}

fn resolve_file_candidate(joined: &Path) -> Option<PathBuf> {
    // Probe in priority order; each candidate is a stat syscall. The order is
    // the shared one: a `.ts` source the written `.js` stands for, then the
    // exact path, then appended extensions, then a directory index. Replayed
    // against the JavaScript graph by the `fileProbe` section of
    // `tests/fixtures/module-resolution-conformance.json`.
    let written = joined
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();

    let rewritten = typescript_source_extensions(written)
        .iter()
        .map(|extension| joined.with_extension(extension));
    let appended = PROBE_EXTENSIONS
        .iter()
        .map(|extension| with_appended_extension(joined, extension));
    let indexed = PROBE_EXTENSIONS
        .iter()
        .map(|extension| joined.join(format!("index.{extension}")));

    // `normalized_canonical_path` already falls back to its argument when
    // canonicalization fails, so probing with a separate `canonicalize()` only
    // repeated the syscall on the path that succeeds — the branch could not
    // change the answer. One call per resolved module, on the hottest path in
    // the resolver.
    rewritten
        .chain(std::iter::once(joined.to_path_buf()))
        .chain(appended)
        .chain(indexed)
        .find(|candidate| candidate.is_file())
        .map(|candidate| ruvyxa_diagnostics::normalized_canonical_path(&candidate))
}

fn is_project_local(path: &Path, project_root: &Path) -> bool {
    let rel = match path.strip_prefix(project_root) {
        Ok(r) => r,
        Err(_) => return false,
    };
    !rel.starts_with("node_modules")
}

#[cfg(test)]
mod tests {
    /// The Rust half of the shared ordering table.
    ///
    /// Everything this side sorts into an artifact comes out in `str::cmp`
    /// order, which compares UTF-8 bytes and is therefore code-point order.
    /// `packages/ruvyxa/runtime/order.mjs` is the other side, and it used to
    /// compare UTF-16 code units -- the same answer everywhere below U+10000
    /// and the opposite one wherever a surrogate pair meets U+E000-U+FFFF. The
    /// sites downstream are cache keys, content fingerprints, glob key order and
    /// emitted bytes, so a disagreement is one project building to two outputs.
    #[test]
    fn string_ordering_matches_the_shared_cross_language_table() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/ordering-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("the ordering fixture is valid JSON");

        let cases = fixture["cases"].as_array().expect("the fixture has cases");
        assert!(!cases.is_empty(), "an empty table asserts nothing");
        for case in cases {
            let name = case["name"].as_str().expect("each case is named");
            let left = case["left"].as_str().expect("each case has a left");
            let right = case["right"].as_str().expect("each case has a right");
            let expect = case["expect"].as_i64().expect("each case has an answer");
            let expected = match expect {
                -1 => std::cmp::Ordering::Less,
                0 => std::cmp::Ordering::Equal,
                1 => std::cmp::Ordering::Greater,
                other => panic!("case {name}: expect must be -1, 0 or 1, not {other}"),
            };
            assert_eq!(left.cmp(right), expected, "ordering case: {name}");
            assert_eq!(
                right.cmp(left),
                expected.reverse(),
                "ordering case reversed: {name}",
            );
        }
    }

    use super::*;

    /// The rule `isProjectLocal` in `packages/ruvyxa/runtime/compiler.mjs`
    /// mirrors, and which it did not: that half asked only whether the path was
    /// under the root, so a browser bundle reported every file of every bundled
    /// dependency as project input.
    /// `tests/packages/ruvyxa/project-inputs.test.mjs` holds the other side.
    #[test]
    fn project_local_excludes_the_project_s_own_node_modules() {
        let root = Path::new("/app");
        assert!(is_project_local(Path::new("/app/src/page.tsx"), root));
        assert!(!is_project_local(
            Path::new("/app/node_modules/tiny/index.js"),
            root
        ));
        assert!(!is_project_local(Path::new("/elsewhere/page.tsx"), root));
        // Component-wise, so only the *first* segment disqualifies and a name
        // that merely begins with the word does not.
        assert!(is_project_local(
            Path::new("/app/src/node_modules_notes.ts"),
            root
        ));
        assert!(is_project_local(
            Path::new("/app/src/node_modules/x.ts"),
            root
        ));
    }

    #[test]
    fn extracts_import_specifiers() {
        let source = r#"
            import React from "react"
            import { foo } from "./foo"
            import type { Bar } from './bar'
            import "./styles.css"
            export { baz } from "../baz"
            const helper = require("./helper")
            const lazy = import("./lazy")
        "#;

        let specs = extract_specifiers(source);
        assert!(specs.contains(&"./foo".to_string()));
        assert!(!specs.contains(&"./bar".to_string()));
        assert!(specs.contains(&"./styles.css".to_string()));
        assert!(specs.contains(&"../baz".to_string()));
        assert!(specs.contains(&"react".to_string()));
        assert!(specs.contains(&"./helper".to_string()));
        assert!(specs.contains(&"./lazy".to_string()));
    }

    #[test]
    fn content_dependency_scan_ignores_import_examples_in_code_fences() {
        let source =
            "import Card from './Card'\n\n```js\nimport Secret from './missing'\n```\n\n<Card />";
        let compiled =
            crate::content::compile_content_module(source, Path::new("page.mdx")).unwrap();
        let specifiers = extract_specifiers(&compiled);
        assert!(specifiers.iter().any(|specifier| specifier == "./Card"));
        assert!(!specifiers.iter().any(|specifier| specifier == "./missing"));
    }

    #[test]
    fn quoted_value_handles_double_and_single_quotes() {
        assert_eq!(quoted_value(r#""hello""#), Some("hello".to_string()));
        assert_eq!(quoted_value("'world'"), Some("world".to_string()));
        assert_eq!(quoted_value("nothing"), None);
    }

    #[test]
    fn resolve_cache_deduplicates() {
        let cache = ResolveGraphCache::new();
        let base = "/project/src";

        // Initially empty
        assert!(cache.resolution(base, "./utils").is_none());

        // Insert a result
        cache.insert_resolution(
            base,
            "./utils",
            Some(PathBuf::from("/project/src/utils.ts")),
        );

        // Now cached
        let cached = cache.resolution(base, "./utils");
        assert!(cached.is_some());
        assert_eq!(
            cached.unwrap().as_ref().unwrap(),
            &PathBuf::from("/project/src/utils.ts")
        );
    }

    /// Replay the shared import-case table.
    ///
    /// The JavaScript half is
    /// `tests/packages/ruvyxa/import-case-contract.test.mjs` over
    /// `importCaseMismatch` in `packages/ruvyxa/runtime/compiler.mjs`. The two
    /// module graphs both resolve imports, so a rule enforced by one alone is a
    /// build that refuses under `ruvyxa build` and passes at prerender, or the
    /// reverse.
    ///
    /// Fixture paths are written with forward slashes only, which `Path`
    /// splits on every platform this ships to, so the table replays the same
    /// way on Windows and on Linux.
    #[test]
    fn import_case_comparison_matches_the_shared_conformance_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/import-case-conformance.json"
        ))
        .unwrap();

        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let requested = Path::new(case["requested"].as_str().unwrap());
            let resolved = Path::new(case["resolved"].as_str().unwrap());
            let actual = import_case_mismatch(requested, resolved);

            match case["mismatch"].as_object() {
                None => assert_eq!(actual, None, "{name}: expected no mismatch"),
                Some(expected) => {
                    let mismatch = actual
                        .unwrap_or_else(|| panic!("{name}: expected a mismatch and got none"));
                    assert_eq!(
                        mismatch.requested,
                        expected["requested"].as_str().unwrap(),
                        "{name}: requested segment"
                    );
                    assert_eq!(
                        mismatch.resolved,
                        expected["resolved"].as_str().unwrap(),
                        "{name}: resolved segment"
                    );
                }
            }
        }
    }

    #[test]
    fn resolve_cache_stores_none_for_unresolved() {
        let cache = ResolveGraphCache::new();
        let base = "/project/src";

        cache.insert_resolution(base, "./missing", None);

        let cached = cache.resolution(base, "./missing");
        assert!(cached.is_some()); // entry exists
        assert!(cached.unwrap().is_none()); // but value is None
    }

    #[test]
    fn tsconfig_cache_reloads_when_config_fingerprint_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let cache = ResolveGraphCache::new();

        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"src"}}"#,
        )
        .unwrap();
        assert_eq!(cache.tsconfig_paths(root).base_url, Some(root.join("src")));

        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"source"}}"#,
        )
        .unwrap();
        assert_eq!(
            cache.tsconfig_paths(root).base_url,
            Some(root.join("source"))
        );
    }

    #[test]
    fn resolves_absolute_project_imports() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        fs::write(&page, "export default function Page() {}").unwrap();

        let import_path = page.display().to_string().replace('\\', "/");
        let source = format!(
            "import Page from {};",
            serde_json::to_string(&import_path).unwrap()
        );
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let tsconfig = TsConfigPaths::load(&root);
        let deps = collect_deps_cached(
            &source,
            "ts",
            &root,
            &root,
            &tsconfig,
            &ResolveGraphCache::new(),
            &BuildHookPipeline::empty(),
            BundleTarget::Client,
            JsxRuntime::Automatic,
        )
        .unwrap();

        assert_eq!(deps.paths.len(), 1);
        assert_eq!(
            deps.paths[0],
            ruvyxa_diagnostics::normalized_canonical_path(&page)
        );
    }

    /// A project with a resolvable `react/jsx-runtime`, so the seeded edge is
    /// something the resolver can actually answer. An unresolvable bare
    /// specifier is dropped silently, which would make both halves of the test
    /// below pass for the wrong reason.
    fn project_with_jsx_runtime() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let react = root.join("node_modules").join("react");
        fs::create_dir_all(&react).unwrap();
        fs::write(
            react.join("package.json"),
            r#"{ "name": "react", "version": "0.0.0", "main": "index.js" }"#,
        )
        .unwrap();
        fs::write(react.join("index.js"), "export default {};\n").unwrap();
        fs::write(
            react.join("jsx-runtime.js"),
            "export const jsx = () => {};\n",
        )
        .unwrap();
        (temp, root, app)
    }

    /// The `react/jsx-runtime` edge is seeded for the module the transform will
    /// actually give JSX to — not for every module whose source contains a `<`.
    ///
    /// `has_jsx` is the scanner's `<`-heuristic, and in a `.ts` file `<` opens a
    /// type: `new Map<string, number>()` sets it. The transform refuses JSX for
    /// `.ts`/`.mts`/`.cts` outright, so it injects no helper import there and
    /// the seeded edge answered a question nobody asked — a phantom dependency
    /// on every plain-TypeScript module mentioning a generic, inflating the
    /// module graph, perturbing shared-chunk analysis (which keys on
    /// `module.deps`), and pulling React into route bundles that render nothing.
    ///
    /// The JavaScript graph never had this: `compiler.mjs` scans the module
    /// *after* the transform, so the helper import is there or it is not.
    #[test]
    fn only_a_module_the_transform_gives_jsx_to_seeds_the_jsx_runtime_edge() {
        let (_temp, root, app) = project_with_jsx_runtime();
        let tsconfig = TsConfigPaths::load(&root);
        let seeds_the_edge = |source: &str, extension: &str| {
            collect_deps_cached(
                source,
                extension,
                &app,
                &root,
                &tsconfig,
                &ResolveGraphCache::new(),
                &BuildHookPipeline::empty(),
                BundleTarget::Client,
                JsxRuntime::Automatic,
            )
            .unwrap()
            .aliases
            .contains_key(JSX_RUNTIME_SPECIFIER)
        };

        for extension in ["ts", "mts", "cts", ".TS"] {
            assert!(
                !seeds_the_edge("export const m = new Map<string, number>();\n", extension),
                "a generic type in a {extension} module seeded a JSX edge"
            );
        }
        assert!(
            seeds_the_edge("export const el = <main />;\n", "tsx"),
            "a .tsx module with an element must keep the seeded edge"
        );
        // The `.js` family is where the extension decides nothing and the guess
        // still answers — the one case `jsx_is_enabled` defers on.
        assert!(
            seeds_the_edge("export const el = <main />;\n", "js"),
            "a .js module with an element must keep the seeded edge"
        );
        assert!(
            !seeds_the_edge("export const n = 1;\n", "tsx"),
            "a .tsx module with no element imports no helper to link"
        );
    }

    /// True when this filesystem answers `is_file()` without regard to case.
    ///
    /// Probed rather than assumed from the target triple: macOS ships
    /// case-insensitive by default and case-sensitive on request, and a Linux
    /// checkout can sit on a mounted volume that folds case. The behaviour
    /// under test is the filesystem's, so ask the filesystem.
    fn filesystem_folds_case(dir: &Path) -> bool {
        let probe = dir.join("ruvyxa-case-probe.ts");
        fs::write(&probe, "export {};").unwrap();
        let folded = dir.join("RUVYXA-CASE-PROBE.ts").is_file();
        fs::remove_file(&probe).unwrap();
        folded
    }

    /// An import spelled in the wrong case fails here instead of on Linux.
    ///
    /// This is the whole point of RUV1807: on a case-folding filesystem the
    /// import resolves and the project builds, and the identical source tree
    /// resolves nothing on the case-sensitive host it deploys to. On a
    /// case-sensitive filesystem there is nothing to report, because the
    /// import does not resolve in the first place — so the assertion flips
    /// rather than the test skipping, and both halves stay exercised.
    #[test]
    fn a_wrongly_cased_relative_import_is_refused_before_linux_sees_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("header.tsx"), "export default function H() {}").unwrap();

        let tsconfig = TsConfigPaths::load(&root);
        let result = collect_deps_cached(
            "import Header from \"./Header\";",
            "ts",
            &app,
            &root,
            &tsconfig,
            &ResolveGraphCache::new(),
            &BuildHookPipeline::empty(),
            BundleTarget::Client,
            JsxRuntime::Automatic,
        );

        if filesystem_folds_case(&app) {
            let message = result
                .expect_err("a case-folding filesystem resolves ./Header and must refuse it")
                .to_string();
            assert!(message.contains("RUV1807"), "{message}");
            assert!(message.contains("Header"), "{message}");
            assert!(message.contains("header"), "{message}");
        } else {
            // Nothing to report on a case-sensitive filesystem: `./Header`
            // names no file there, so this is the ordinary unresolved-import
            // failure and RUV1807 must stay out of it.
            let error =
                result.expect_err("a case-sensitive filesystem never resolves ./Header at all");
            assert!(
                matches!(error, BundleError::Unresolved { .. }),
                "expected an unresolved import, got: {error}"
            );
            assert!(!error.to_string().contains("RUV1807"), "{error}");
        }
    }

    /// An import naming a file that exists in no casing is unresolved, not a
    /// case report.
    ///
    /// Runs the same on every filesystem, which is the point: the branch above
    /// can only exercise whichever answer the host gives, and the first version
    /// of it asserted the wrong one for the host it could not run on. This one
    /// pins the invariant that actually matters — RUV1807 speaks only when
    /// there is a real file with a different spelling behind it — and pins it
    /// everywhere.
    #[test]
    fn an_import_with_no_file_behind_it_is_unresolved_not_a_case_report() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let tsconfig = TsConfigPaths::load(&root);
        let error = collect_deps_cached(
            "import Missing from \"./Missing\";",
            "ts",
            &app,
            &root,
            &tsconfig,
            &ResolveGraphCache::new(),
            &BuildHookPipeline::empty(),
            BundleTarget::Client,
            JsxRuntime::Automatic,
        )
        .expect_err("nothing on disk answers ./Missing");

        assert!(
            matches!(error, BundleError::Unresolved { .. }),
            "expected an unresolved import, got: {error}"
        );
        assert!(!error.to_string().contains("RUV1807"), "{error}");
    }

    /// The correctly spelled import of the same file stays silent.
    ///
    /// Written because the cheapest way to pass the test above is a check that
    /// fires on everything.
    #[test]
    fn a_correctly_cased_relative_import_is_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("header.tsx"), "export default function H() {}").unwrap();

        let tsconfig = TsConfigPaths::load(&root);
        let deps = collect_deps_cached(
            "import Header from \"./header\";",
            "ts",
            &app,
            &root,
            &tsconfig,
            &ResolveGraphCache::new(),
            &BuildHookPipeline::empty(),
            BundleTarget::Client,
            JsxRuntime::Automatic,
        )
        .expect("the spelling matches the file on disk");

        assert_eq!(deps.paths.len(), 1);
        assert_eq!(
            deps.paths[0],
            ruvyxa_diagnostics::normalized_canonical_path(&app.join("header.tsx"))
        );
    }

    #[test]
    fn ignores_css_side_effect_imports() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("global.css"), "body { margin: 0; }").unwrap();
        let tsconfig = TsConfigPaths::load(temp.path());

        let deps = collect_deps_cached(
            "import \"./global.css\";",
            "ts",
            &app,
            temp.path(),
            &tsconfig,
            &ResolveGraphCache::new(),
            &BuildHookPipeline::empty(),
            BundleTarget::Client,
            JsxRuntime::Automatic,
        )
        .unwrap();

        assert!(deps.paths.is_empty());
    }

    /// Two routes sharing one module that reaches its own dependency through a
    /// `tsconfig` alias. The alias is the part a warm cache hit has to carry:
    /// `~/lib/x` shares no path suffix with `lib/x.ts`, so the linker can only
    /// resolve it through the alias map.
    fn aliased_two_route_project() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let lib = temp.path().join("lib");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&lib).unwrap();

        fs::write(
            temp.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"~/*":["./*"]}}}"#,
        )
        .unwrap();

        let aliased = lib.join("x.ts");
        let shared = app.join("shared.ts");
        let page_a = app.join("a.tsx");
        let page_b = app.join("b.tsx");

        fs::write(&aliased, "export const x = 1;").unwrap();
        fs::write(
            &shared,
            "import { x } from \"~/lib/x\";\nexport const label = x;",
        )
        .unwrap();
        // Byte-identical on purpose: the two routes must collide on the
        // dependency cache key, which is what puts the warm path under test.
        fs::write(&page_a, "import { label } from \"./shared\";").unwrap();
        fs::write(&page_b, "import { label } from \"./shared\";").unwrap();

        (temp, app, shared, page_a, page_b)
    }

    fn virtual_entry_for(page: &Path) -> String {
        format!(
            "import Page from {};",
            serde_json::to_string(&page.display().to_string().replace('\\', "/")).unwrap()
        )
    }

    fn shared_module_aliases(
        modules: &[ResolvedModule],
        shared: &Path,
    ) -> BTreeMap<String, PathBuf> {
        let key = ruvyxa_diagnostics::normalized_canonical_path(shared);
        modules
            .iter()
            .find(|module| module.path == key)
            .expect("the shared module is part of every route's graph")
            .dependency_aliases
            .clone()
    }

    #[test]
    fn shared_graph_cache_reuses_source_reads_across_routes() {
        let (temp, app, shared, page_a, page_b) = aliased_two_route_project();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let cache = ResolveGraphCache::new();

        let route_a = resolve_graph_with_cache(
            &virtual_entry_for(&page_a),
            "ruvyxa:test-a.tsx",
            &root,
            &app,
            &cache,
        )
        .unwrap();
        let dependencies_after_first_route = cache.dependency_count();
        let route_b = resolve_graph_with_cache(
            &virtual_entry_for(&page_b),
            "ruvyxa:test-b.tsx",
            &root,
            &app,
            &cache,
        )
        .unwrap();

        assert_eq!(cache.source_count(), 4);
        assert!(cache.resolution_count() >= 1);
        assert_eq!(
            cache.dependency_count(),
            dependencies_after_first_route + 1,
            "only the second virtual entry is new; identical page and shared scans are reused"
        );

        let aliases_a = shared_module_aliases(&route_a, &shared);
        let aliases_b = shared_module_aliases(&route_b, &shared);
        assert!(
            aliases_a.contains_key("~/lib/x"),
            "the cold route records the tsconfig alias: {aliases_a:?}"
        );
        assert_eq!(
            aliases_a, aliases_b,
            "a warm dependency-cache hit must answer with both halves — paths and aliases — or \
             the second route's linker cannot resolve `~/lib/x`"
        );
    }

    #[test]
    fn warm_cache_hits_persist_a_complete_alias_map() {
        let (temp, app, shared, page_a, page_b) = aliased_two_route_project();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let cache = ResolveGraphCache::new();
        let cache_dir = temp.path().join(".ruvyxa-test-cache");
        let incremental = IncrementalGraphCache::at_dir(&cache_dir, "test", true);

        for (label, page) in [
            ("ruvyxa:test-a.tsx", &page_a),
            ("ruvyxa:test-b.tsx", &page_b),
        ] {
            resolve_graph_with_incremental(
                &virtual_entry_for(page),
                label,
                &root,
                &app,
                &cache,
                &BuildHookPipeline::empty(),
                BundleTarget::Client,
                JsxRuntime::Automatic,
                Some(&incremental),
            )
            .unwrap();
        }
        incremental.save().unwrap();

        let reloaded = IncrementalGraphCache::at_dir(&cache_dir, "test", true);
        let shared_key = ruvyxa_diagnostics::normalized_canonical_path(&shared);
        let persisted = reloaded
            .cached_aliases(&shared_key)
            .expect("the shared module was recorded with an alias map");
        assert!(
            persisted.contains_key("~/lib/x"),
            "the last route to record the module must not overwrite the alias map with an empty \
             one — `Some({{}})` passes the Option guard, so the next build reuses it: {persisted:?}"
        );
    }

    #[test]
    fn production_snapshot_skips_revalidation_until_explicit_invalidation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.ts");
        fs::write(&source, "export const value = 'first';").unwrap();
        let cache = ResolveGraphCache::for_build();

        assert!(cache.read_source(&source).unwrap().contains("first"));
        fs::write(&source, "export const value = 'after';").unwrap();
        assert!(cache.read_source(&source).unwrap().contains("first"));

        cache.invalidate_paths(std::slice::from_ref(&source));
        assert!(cache.read_source(&source).unwrap().contains("after"));
    }

    #[test]
    fn parallel_resolution_produces_same_results_as_sequential() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let components = app.join("components");
        fs::create_dir_all(&components).unwrap();

        // Build a small dependency tree:
        // page.tsx → Button.tsx, Card.tsx
        // Button.tsx → utils.ts
        // Card.tsx → utils.ts (shared dep)
        fs::write(
            app.join("page.tsx"),
            r#"import Button from "./components/Button";
import Card from "./components/Card";
export default function Page() { return <Button /><Card /> }"#,
        )
        .unwrap();
        fs::write(
            components.join("Button.tsx"),
            r#"import { cn } from "./utils";
export default function Button() { return <button className={cn("btn")} /> }"#,
        )
        .unwrap();
        fs::write(
            components.join("Card.tsx"),
            r#"import { cn } from "./utils";
export default function Card() { return <div className={cn("card")} /> }"#,
        )
        .unwrap();
        fs::write(
            components.join("utils.ts"),
            "export function cn(...args: string[]) { return args.join(' ') }",
        )
        .unwrap();

        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let page_path = app.join("page.tsx");
        let import_path = page_path.display().to_string().replace('\\', "/");
        let entry_source = format!(
            "import Page from {};",
            serde_json::to_string(&import_path).unwrap()
        );

        let cache = ResolveGraphCache::new();
        let result =
            resolve_graph_with_cache(&entry_source, "ruvyxa:test-entry.tsx", &root, &app, &cache)
                .unwrap();

        // Should find: entry + page + Button + Card + utils = 5 modules
        assert_eq!(result.len(), 5);

        // utils.ts should appear in deps of both Button and Card
        let utils_path =
            ruvyxa_diagnostics::normalized_canonical_path(&components.join("utils.ts"));
        let button_module = result
            .iter()
            .find(|m| {
                m.path
                    .file_name()
                    .map(|f| f == "Button.tsx")
                    .unwrap_or(false)
            })
            .unwrap();
        let card_module = result
            .iter()
            .find(|m| m.path.file_name().map(|f| f == "Card.tsx").unwrap_or(false))
            .unwrap();

        assert!(button_module.deps.contains(&utils_path));
        assert!(card_module.deps.contains(&utils_path));

        // Cache should have stored the source reads (no duplicate reads)
        assert!(cache.source_count() >= 4); // page, Button, Card, utils
    }

    #[test]
    fn tsconfig_paths_resolve_alias() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let src = root.join("src");
        let components = src.join("components");
        fs::create_dir_all(&components).unwrap();

        let button = components.join("Button.tsx");
        fs::write(&button, "export default function Button() {}").unwrap();

        // Write tsconfig.json with @/* path alias.
        let tsconfig = serde_json::json!({
            "compilerOptions": {
                "baseUrl": ".",
                "paths": {
                    "@/*": ["./src/*"]
                }
            }
        });
        fs::write(
            root.join("tsconfig.json"),
            serde_json::to_string(&tsconfig).unwrap(),
        )
        .unwrap();

        let tc = TsConfigPaths::load(root);
        let resolved = tc.resolve("@/components/Button");

        assert!(
            resolved.is_some(),
            "should resolve @/components/Button via tsconfig paths"
        );
        let resolved_path = resolved.unwrap();
        assert!(
            resolved_path.to_string_lossy().contains("Button"),
            "resolved path should point to Button: {}",
            resolved_path.display()
        );
    }

    #[test]
    fn tsconfig_paths_resolve_targets_relative_to_base_url() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let components = root.join("src/components");
        fs::create_dir_all(&components).unwrap();
        fs::write(
            components.join("Button.tsx"),
            "export default function Button() {}",
        )
        .unwrap();

        let tsconfig = serde_json::json!({
            "compilerOptions": {
                "baseUrl": "src",
                "paths": { "@/*": ["./components/*"] }
            }
        });
        fs::write(
            root.join("tsconfig.json"),
            serde_json::to_string(&tsconfig).unwrap(),
        )
        .unwrap();

        let resolved = TsConfigPaths::load(root).resolve("@/Button");
        assert_eq!(
            resolved,
            Some(ruvyxa_diagnostics::normalized_canonical_path(
                &components.join("Button.tsx")
            ))
        );
    }

    #[test]
    fn replays_tsconfig_path_conformance_fixture() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Fixture {
            contract: String,
            schema_version: u32,
            files: BTreeMap<String, String>,
            configs: BTreeMap<String, String>,
            outside_files: BTreeMap<String, String>,
            cases: Vec<Case>,
        }
        #[derive(serde::Deserialize)]
        struct Case {
            name: String,
            specifier: String,
            expected: Option<String>,
        }
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../../tests/fixtures/path-alias-contract.json"
        ))
        .unwrap();
        assert_eq!(fixture.contract, "ruvyxa.path-alias");
        assert_eq!(fixture.schema_version, 1);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        for (relative, source) in fixture.files.into_iter().chain(fixture.configs) {
            let file = root.join(relative);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, source).unwrap();
        }
        for (relative, source) in fixture.outside_files {
            let file = temp.path().join(relative);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, source).unwrap();
        }

        let config = TsConfigPaths::load(&root);
        let canonical_root = ruvyxa_diagnostics::normalized_canonical_path(&root);
        for case in fixture.cases {
            let actual = config.resolve(&case.specifier).map(|path| {
                path.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            });
            assert_eq!(actual, case.expected, "{}", case.name);
        }
        assert_eq!(config.config_files.len(), 2);
    }

    /// A `baseUrl` inherited through `extends` must anchor the child's `paths`.
    ///
    /// This is the half of TypeScript's merge rule both Ruvyxa graphs used to
    /// get wrong in the same direction: they anchored `paths` to the directory
    /// of the config that declared them, ignoring an inherited `baseUrl`. The
    /// two agreed with each other, so no parity fixture caught it, while the
    /// editor and the type checker resolved these imports somewhere else.
    #[test]
    fn inherited_base_url_anchors_child_path_aliases() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Fixture {
            inherited_base_url: Scenario,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Scenario {
            files: BTreeMap<String, String>,
            configs: BTreeMap<String, String>,
            cases: Vec<Case>,
        }
        #[derive(serde::Deserialize)]
        struct Case {
            name: String,
            specifier: String,
            expected: Option<String>,
        }
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../../tests/fixtures/path-alias-contract.json"
        ))
        .unwrap();
        let scenario = fixture.inherited_base_url;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        for (relative, source) in scenario.files.into_iter().chain(scenario.configs) {
            let file = root.join(relative);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, source).unwrap();
        }

        let config = TsConfigPaths::load(&root);
        let canonical_root = ruvyxa_diagnostics::normalized_canonical_path(&root);
        for case in scenario.cases {
            let actual = config.resolve(&case.specifier).map(|path| {
                path.strip_prefix(&canonical_root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            });
            assert_eq!(actual, case.expected, "{}", case.name);
        }
    }

    #[test]
    fn local_aliases_survive_a_missing_or_cyclic_parent_with_a_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/value.ts"), "export const value = 1;").unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":"./missing.json","compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        )
        .unwrap();
        let (missing, problem) = TsConfigPaths::load_reporting(root);
        assert!(problem.is_some());
        assert!(missing.resolve("@/value").is_some());

        fs::write(
            root.join("tsconfig.json"),
            r#"{"extends":"./base.json","compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        )
        .unwrap();
        fs::write(root.join("base.json"), r#"{"extends":"./tsconfig.json"}"#).unwrap();
        let (cyclic, problem) = TsConfigPaths::load_reporting(root);
        assert!(
            problem
                .unwrap()
                .message
                .contains("cyclic tsconfig/jsconfig extends chain")
        );
        assert!(cyclic.resolve("@/value").is_some());
    }

    #[test]
    fn inherited_configuration_content_participates_in_the_resolver_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("tsconfig.json"), r#"{"extends":"./base.json"}"#).unwrap();
        fs::write(
            root.join("base.json"),
            r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        )
        .unwrap();
        let cache = ResolveGraphCache::new();
        let first = cache.configuration_hash(root);

        fs::write(
            root.join("base.json"),
            r#"{/* changed */"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        )
        .unwrap();
        let second = cache.configuration_hash(root);

        assert_ne!(first, second);
    }

    #[test]
    fn tsconfig_baseurl_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lib = root.join("lib");
        fs::create_dir_all(&lib).unwrap();
        fs::write(lib.join("utils.ts"), "export const x = 1;").unwrap();

        let tsconfig = serde_json::json!({
            "compilerOptions": {
                "baseUrl": "."
            }
        });
        fs::write(
            root.join("tsconfig.json"),
            serde_json::to_string(&tsconfig).unwrap(),
        )
        .unwrap();

        let tc = TsConfigPaths::load(root);
        // "lib/utils" should resolve via baseUrl.
        let resolved = tc.resolve("lib/utils");
        assert!(resolved.is_some(), "should resolve lib/utils via baseUrl");
    }

    #[test]
    fn strip_json_comments_handles_line_comments() {
        let input = r#"{
            // this is a comment
            "key": "value" // inline comment
        }"#;
        let stripped = strip_json_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    /// `tsc --init` documents every option in a `/* */` block. A tsconfig kept in
    /// that shape parsed as nothing, so `baseUrl` and `paths` silently vanished
    /// and every aliased import failed to resolve.
    #[test]
    fn strip_json_comments_handles_block_comments_and_trailing_commas() {
        let input = r#"{
            /* Visit https://aka.ms/tsconfig to read more about this file */
            "compilerOptions": {
                "baseUrl": ".", /* Base directory to resolve non-relative modules. */
                "paths": {
                    "@/*": ["./src/*"],
                },
            },
        }"#;

        let parsed: serde_json::Value =
            serde_json::from_str(&strip_json_comments(input)).expect("JSONC must parse");
        assert_eq!(parsed["compilerOptions"]["baseUrl"], ".");
        assert_eq!(parsed["compilerOptions"]["paths"]["@/*"][0], "./src/*");
    }

    /// The stripper must not reach inside string values. Path patterns are full
    /// of `/*`, and rewriting one would corrupt the alias it is meant to read.
    #[test]
    fn strip_json_comments_leaves_string_contents_alone() {
        let input = r#"{"paths": {"@/*": ["./src/*"], "x": ["a//b", "c/*d", "e,"]}}"#;

        let parsed: serde_json::Value =
            serde_json::from_str(&strip_json_comments(input)).expect("must parse");
        assert_eq!(parsed["paths"]["@/*"][0], "./src/*");
        assert_eq!(parsed["paths"]["x"][0], "a//b");
        assert_eq!(parsed["paths"]["x"][1], "c/*d");
        assert_eq!(parsed["paths"]["x"][2], "e,");
    }

    /// A block comment keeps its newlines so a later parse error still names the
    /// line the author wrote.
    #[test]
    fn strip_json_comments_preserves_line_numbers() {
        let input = "{\n/* one\ntwo\nthree */\n\"key\": 1\n}";
        assert_eq!(
            strip_json_comments(input).lines().count(),
            input.lines().count()
        );
    }

    /// A project with no tsconfig and a project with a broken one both resolve
    /// no aliases; only one of them is a problem the user needs told about.
    #[test]
    fn a_malformed_tsconfig_is_reported_rather_than_silently_ignored() {
        let temp = tempfile::tempdir().expect("temp dir");

        let (paths, problem) = TsConfigPaths::load_reporting(temp.path());
        assert!(paths.paths.is_empty());
        assert_eq!(problem, None, "an absent tsconfig is not a problem");

        fs::write(
            temp.path().join("tsconfig.json"),
            "{ \"compilerOptions\": }",
        )
        .expect("write malformed config");
        let (paths, problem) = TsConfigPaths::load_reporting(temp.path());
        assert!(
            paths.paths.is_empty(),
            "resolution still degrades rather than failing the build"
        );
        let problem = problem.expect("a malformed tsconfig must be reported");
        assert_eq!(problem.path, temp.path().join("tsconfig.json"));
        assert!(!problem.message.is_empty(), "the reason must be carried");
    }

    /// A broken `tsconfig.json` must not cost a project the `jsconfig.json`
    /// sitting beside it that parses perfectly well.
    #[test]
    fn a_valid_jsconfig_still_loads_past_a_broken_tsconfig() {
        let temp = tempfile::tempdir().expect("temp dir");
        fs::write(temp.path().join("tsconfig.json"), "{ not json").expect("write");
        fs::write(
            temp.path().join("jsconfig.json"),
            r#"{"compilerOptions": {"paths": {"@/*": ["./src/*"]}}}"#,
        )
        .expect("write");

        let (paths, problem) = TsConfigPaths::load_reporting(temp.path());
        assert_eq!(
            paths.paths,
            vec![("@/*".to_string(), vec!["./src/*".to_string()])]
        );
        assert_eq!(problem, None, "a config that loaded is not a failure");
    }

    /// End to end: the aliases must survive a tsconfig written the way `tsc`
    /// generates one.
    #[test]
    fn parse_tsconfig_paths_reads_a_tsc_init_style_config() {
        let content = r#"{
            "compilerOptions": {
                /* Modules */
                "baseUrl": "./src", /* resolve non-relative modules from here */
                "paths": {
                    "@app/*": ["./app/*"],
                },
            },
        }"#;

        let value = parse_jsonc(content).expect("a tsc --init style config must parse");
        let parsed =
            paths_from_value(&value, Path::new("/project")).expect("compilerOptions are present");
        assert_eq!(parsed.base_url, Some(Path::new("/project").join("./src")));
        assert_eq!(
            parsed.paths,
            vec![("@app/*".to_string(), vec!["./app/*".to_string()])]
        );
    }

    #[test]
    fn package_name_and_export_key_handles_subpaths() {
        assert_eq!(
            package_name_and_export_key("react/jsx-runtime"),
            Some(("react".to_string(), "./jsx-runtime".to_string()))
        );
        assert_eq!(
            package_name_and_export_key("@scope/pkg"),
            Some(("@scope/pkg".to_string(), ".".to_string()))
        );
        assert_eq!(
            package_name_and_export_key("@scope/pkg/runtime/jsx"),
            Some(("@scope/pkg".to_string(), "./runtime/jsx".to_string()))
        );
    }

    #[test]
    fn resolves_package_exports_subpath() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();
        let pkg = root.join("node_modules").join("pkg");
        fs::create_dir_all(pkg.join("dist")).unwrap();
        fs::write(pkg.join("dist").join("runtime.mjs"), "export const x = 1;").unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"type":"module","exports":{"./runtime":{"import":"./dist/runtime.mjs"}}}"#,
        )
        .unwrap();

        let PackageExportsResolution::Resolved(resolved) =
            resolve_package_exports(&cache, root, "pkg/runtime", BundleTarget::Ssr)
        else {
            panic!("expected package exports resolution");
        };
        assert!(resolved.ends_with("dist/runtime.mjs"));
    }

    /// The `RUV1610: Cannot require "scheduler"` regression. Under pnpm a
    /// transitive dependency lives beside its dependent inside the store and
    /// never appears in the project's own `node_modules`, so resolving only
    /// against the project root left it unresolved and the client linker
    /// replaced the `require` with a throw that fired in the browser.
    #[test]
    fn resolves_transitive_packages_from_a_nested_node_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();

        let store = root.join("node_modules/.pnpm/react-dom@19.2.8/node_modules");
        let react_dom = store.join("react-dom");
        fs::create_dir_all(react_dom.join("cjs")).unwrap();
        fs::write(
            react_dom.join("package.json"),
            r#"{"exports":{".":{"default":"./index.js"}}}"#,
        )
        .unwrap();
        fs::write(
            react_dom.join("cjs").join("react-dom.production.js"),
            r#"var Scheduler = require("scheduler");"#,
        )
        .unwrap();

        // `scheduler` exists only next to its dependent, and ships no
        // `exports` map — the two halves of the original failure.
        let scheduler = store.join("scheduler");
        fs::create_dir_all(&scheduler).unwrap();
        fs::write(scheduler.join("package.json"), r#"{"main":"index.js"}"#).unwrap();
        fs::write(scheduler.join("index.js"), "module.exports = {};").unwrap();

        let PackageExportsResolution::Resolved(resolved) = resolve_node_modules_specifier(
            &cache,
            &react_dom.join("cjs"),
            root,
            "scheduler",
            BundleTarget::Client,
        ) else {
            panic!("expected the nested scheduler package to resolve");
        };
        assert!(resolved.ends_with("scheduler/index.js"), "{resolved:?}");
    }

    #[test]
    fn resolves_packages_without_an_exports_map() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();

        // `main`, then a bare `index.js`, then a subpath — none of which the
        // `exports`-only resolver could reach.
        let main_only = root.join("node_modules").join("main-only");
        fs::create_dir_all(main_only.join("lib")).unwrap();
        fs::write(main_only.join("package.json"), r#"{"main":"lib/entry.js"}"#).unwrap();
        fs::write(
            main_only.join("lib").join("entry.js"),
            "module.exports = 1;",
        )
        .unwrap();

        let index_only = root.join("node_modules").join("index-only");
        fs::create_dir_all(&index_only).unwrap();
        fs::write(index_only.join("package.json"), r#"{"version":"1.0.0"}"#).unwrap();
        fs::write(index_only.join("index.js"), "module.exports = 2;").unwrap();

        for (specifier, expected) in [
            ("main-only", "main-only/lib/entry.js"),
            ("index-only", "index-only/index.js"),
            ("main-only/lib/entry.js", "main-only/lib/entry.js"),
        ] {
            let PackageExportsResolution::Resolved(resolved) =
                resolve_package_exports(&cache, root, specifier, BundleTarget::Client)
            else {
                panic!("expected {specifier} to resolve");
            };
            assert!(resolved.ends_with(expected), "{specifier}: {resolved:?}");
        }
    }

    #[test]
    fn legacy_entry_fields_follow_the_bundle_target() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();

        let pkg = root.join("node_modules").join("dual");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"browser":"./browser.js","main":"./node.js"}"#,
        )
        .unwrap();
        fs::write(pkg.join("browser.js"), "module.exports = 'browser';").unwrap();
        fs::write(pkg.join("node.js"), "module.exports = 'node';").unwrap();

        for (target, expected) in [
            (BundleTarget::Client, "browser.js"),
            (BundleTarget::Ssr, "node.js"),
        ] {
            let PackageExportsResolution::Resolved(resolved) =
                resolve_package_exports(&cache, root, "dual", target)
            else {
                panic!("expected dual package to resolve for {target:?}");
            };
            assert!(resolved.ends_with(expected), "{target:?}: {resolved:?}");
        }
    }

    #[test]
    fn node_modules_candidates_skip_nested_node_modules_segments() {
        let root = Path::new("/app");
        let importer = Path::new("/app/node_modules/.pnpm/react-dom@19/node_modules/react-dom/cjs");
        let candidates = node_modules_candidates(importer, root);

        let contains = |suffix: &str| candidates.iter().any(|path| path.ends_with(suffix));
        assert!(contains("react-dom/cjs/node_modules"));
        assert!(contains("react-dom@19/node_modules"));
        assert!(contains("app/node_modules"));
        // Node never appends `node_modules` to a `node_modules` directory.
        assert!(!contains("node_modules/node_modules"));
    }

    #[test]
    fn resolves_scoped_package_exports() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();
        let pkg = root.join("node_modules").join("@scope").join("pkg");
        fs::create_dir_all(pkg.join("dist")).unwrap();
        fs::write(pkg.join("dist").join("index.js"), "export default 1;").unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"exports":{".":{"default":"./dist/index.js"}}}"#,
        )
        .unwrap();

        let PackageExportsResolution::Resolved(resolved) =
            resolve_package_exports(&cache, root, "@scope/pkg", BundleTarget::Ssr)
        else {
            panic!("expected package exports resolution");
        };
        assert!(resolved.ends_with("dist/index.js"));
    }

    #[test]
    fn resolves_exports_wildcards_and_array_fallbacks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();
        let pkg = root.join("node_modules").join("pkg");
        fs::create_dir_all(pkg.join("dist/features")).unwrap();
        fs::write(
            pkg.join("dist/features/alpha.mjs"),
            "export const alpha = 1;",
        )
        .unwrap();
        fs::write(pkg.join("dist/fallback.js"), "export default 1;").unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"exports":{"./features/*":"./dist/features/*.mjs","./fallback":["./dist/missing.js","./dist/fallback.js"]}}"#,
        )
        .unwrap();

        let PackageExportsResolution::Resolved(wildcard) =
            resolve_package_exports(&cache, root, "pkg/features/alpha", BundleTarget::Client)
        else {
            panic!("expected wildcard resolution");
        };
        assert!(wildcard.ends_with("dist/features/alpha.mjs"));

        let PackageExportsResolution::Resolved(fallback) =
            resolve_package_exports(&cache, root, "pkg/fallback", BundleTarget::Client)
        else {
            panic!("expected fallback resolution");
        };
        assert!(fallback.ends_with("dist/fallback.js"));
    }

    #[test]
    fn resolves_exports_for_the_active_runtime_condition() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();
        let pkg = root.join("node_modules").join("pkg");
        fs::create_dir_all(pkg.join("dist")).unwrap();
        fs::write(
            pkg.join("dist/browser.js"),
            "export const runtime = 'browser';",
        )
        .unwrap();
        fs::write(pkg.join("dist/node.js"), "export const runtime = 'node';").unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"exports":{".":{"browser":"./dist/browser.js","node":"./dist/node.js","default":"./dist/default.js"}}}"#,
        )
        .unwrap();

        let PackageExportsResolution::Resolved(browser) =
            resolve_package_exports(&cache, root, "pkg", BundleTarget::Client)
        else {
            panic!("expected browser export resolution");
        };
        let PackageExportsResolution::Resolved(node) =
            resolve_package_exports(&cache, root, "pkg", BundleTarget::Ssr)
        else {
            panic!("expected node export resolution");
        };

        assert!(browser.ends_with("dist/browser.js"));
        assert!(node.ends_with("dist/node.js"));
    }

    #[test]
    fn resolves_conditional_exports_in_package_declaration_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();
        let pkg = root.join("node_modules").join("pkg");
        fs::create_dir_all(pkg.join("dist")).unwrap();
        fs::write(
            pkg.join("dist/default.js"),
            "export const runtime = 'default';",
        )
        .unwrap();
        fs::write(
            pkg.join("dist/browser.js"),
            "export const runtime = 'browser';",
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"exports":{".":{"default":"./dist/default.js","browser":"./dist/browser.js"}}}"#,
        )
        .unwrap();

        let PackageExportsResolution::Resolved(resolved) =
            resolve_package_exports(&cache, root, "pkg", BundleTarget::Client)
        else {
            panic!("expected conditional export resolution");
        };

        assert!(resolved.ends_with("dist/default.js"));
    }

    #[test]
    fn package_exports_blocks_null_entries_and_rejects_path_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();
        let pkg = root.join("node_modules").join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            root.join("node_modules/secret.js"),
            "export default 'secret';",
        )
        .unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"exports":{"./private":null,"./escape":"./../secret.js"}}"#,
        )
        .unwrap();

        assert_eq!(
            resolve_package_exports(&cache, root, "pkg/private", BundleTarget::Ssr),
            PackageExportsResolution::Blocked
        );
        assert_eq!(
            resolve_package_exports(&cache, root, "pkg/escape", BundleTarget::Ssr),
            PackageExportsResolution::Unavailable
        );
    }

    #[test]
    fn package_exports_cache_invalidates_on_content_change() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache = ResolveGraphCache::new();
        let pkg = root.join("node_modules").join("pkg");
        fs::create_dir_all(pkg.join("dist")).unwrap();
        fs::write(pkg.join("dist/a.js"), "export default 1;").unwrap();
        fs::write(pkg.join("dist/second.js"), "export default 2;").unwrap();
        fs::write(
            pkg.join("package.json"),
            r#"{"exports":{".":"./dist/a.js"}}"#,
        )
        .unwrap();

        let PackageExportsResolution::Resolved(first) =
            resolve_package_exports(&cache, root, "pkg", BundleTarget::Ssr)
        else {
            panic!("expected first resolution");
        };
        assert!(first.ends_with("dist/a.js"));

        // Different byte length than the first package.json (not just
        // different mtime) so the fingerprint changes even if the two
        // writes land within the same filesystem timestamp tick.
        fs::write(
            pkg.join("package.json"),
            r#"{"exports":{".":"./dist/second.js"}}"#,
        )
        .unwrap();

        let PackageExportsResolution::Resolved(second) =
            resolve_package_exports(&cache, root, "pkg", BundleTarget::Ssr)
        else {
            panic!("expected second resolution after package.json change");
        };
        assert!(second.ends_with("dist/second.js"));
    }

    /// One `exports` shape and what it must resolve to.
    struct ExportsCase {
        /// What the case asserts, printed when it fails.
        why: &'static str,
        manifest: &'static str,
        /// Files created inside the package, relative to its directory.
        files: &'static [&'static str],
        specifier: &'static str,
        target: BundleTarget,
        expect: ExpectedResolution,
    }

    enum ExpectedResolution {
        EndsWith(&'static str),
        Blocked,
        Unavailable,
    }

    /// The bundle target each fixture name stands for.
    fn conformance_target(name: &str) -> BundleTarget {
        match name {
            "client" => BundleTarget::Client,
            "ssr" => BundleTarget::Ssr,
            "edge" => BundleTarget::Edge,
            "react-server" => BundleTarget::ReactServer,
            other => panic!("the shared fixture names a target this host does not have: {other}"),
        }
    }

    /// How the shared fixture spells one outcome.
    fn conformance_outcome(resolved: &ExportTargets) -> serde_json::Value {
        match resolved {
            ExportTargets::Blocked => serde_json::Value::String("blocked".to_string()),
            ExportTargets::Unmatched => serde_json::Value::String("unmatched".to_string()),
            ExportTargets::Targets(targets) => serde_json::Value::Array(
                targets
                    .iter()
                    .map(|target| serde_json::Value::String(target.clone()))
                    .collect(),
            ),
        }
    }

    /// Both module graphs answer a bare specifier the same way.
    ///
    /// The JavaScript half is
    /// `tests/packages/ruvyxa/module-resolution-contract.test.mjs` over
    /// `packages/ruvyxa/runtime/package-exports.mjs`, which `compiler.mjs`
    /// reads. They used to disagree outright: `compiler.mjs` answered bare
    /// specifiers with `createRequire().resolve()`, Node's *CommonJS* resolver,
    /// which matches only `["node", "require"]` — so for any dual package the
    /// client bundle built here took `browser`/`import` while the same import
    /// inlined by the other graph took the CommonJS build, and an edge artifact
    /// never saw `worker` or `edge-light` at all.
    ///
    /// `exportsJson` is fixture *text*, parsed here rather than read through
    /// `serde_json::Value`: condition order is what the rule reads, and a map
    /// that sorts its keys would lose it.
    #[test]
    fn package_exports_resolution_matches_the_shared_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/module-resolution-conformance.json"
        ))
        .unwrap();

        for case in fixture["exports"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let exports: PackageJsonValue =
                serde_json::from_str(case["exportsJson"].as_str().unwrap()).unwrap();
            let key = case["key"].as_str().unwrap();
            for (target_name, expected) in case["results"].as_object().unwrap() {
                let resolved =
                    resolve_exports_entry(&exports, key, conformance_target(target_name));
                assert_eq!(
                    &conformance_outcome(&resolved),
                    expected,
                    "{name} disagrees with the shared fixture for target {target_name}"
                );
            }
        }

        for case in fixture["specifiers"].as_array().unwrap() {
            let specifier = case["specifier"].as_str().unwrap();
            let split = package_name_and_export_key(specifier);
            match case["package"].as_str() {
                None => assert!(
                    split.is_none(),
                    "{specifier:?} is not a package specifier in the shared fixture"
                ),
                Some(package) => {
                    let (name, key) =
                        split.unwrap_or_else(|| panic!("{specifier:?} must split into a package"));
                    assert_eq!(name, package, "{specifier:?} package name");
                    assert_eq!(
                        key,
                        case["key"].as_str().unwrap(),
                        "{specifier:?} export key"
                    );
                }
            }
        }

        for case in fixture["legacyEntries"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let manifest =
                parse_package_manifest(&case["manifest"].to_string()).expect("manifest parses");
            let key = case["key"].as_str().unwrap();
            for (target_name, expected) in case["results"].as_object().unwrap() {
                let candidates =
                    legacy_entry_candidates(&manifest, key, conformance_target(target_name));
                assert_eq!(
                    &serde_json::Value::Array(
                        candidates
                            .into_iter()
                            .map(serde_json::Value::String)
                            .collect()
                    ),
                    expected,
                    "{name} disagrees with the shared fixture for target {target_name}"
                );
            }
        }

        for value in fixture["unsafeRelativePaths"].as_array().unwrap() {
            let relative = value.as_str().unwrap();
            let temp = tempfile::tempdir().unwrap();
            assert!(
                resolve_package_relative(temp.path(), relative).is_none(),
                "the shared fixture refuses {relative:?} and this host joined it onto a package"
            );
        }

        for case in fixture["fileProbe"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let temp = tempfile::tempdir().unwrap();
            for file in case["files"].as_array().unwrap() {
                let path = temp.path().join(file.as_str().unwrap());
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, "").unwrap();
            }

            let resolved = resolve_specifier(temp.path(), case["specifier"].as_str().unwrap());
            let answered = resolved.as_ref().map(|path| {
                path.strip_prefix(ruvyxa_diagnostics::normalized_canonical_path(temp.path()))
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/")
            });
            assert_eq!(
                answered.as_deref(),
                case["expect"].as_str(),
                "{name} disagrees with the shared fixture"
            );
        }
    }

    /// `exports` shapes real packages ship, and what each must resolve to.
    ///
    /// Written against Node's own rules, then checked case by case against what
    /// this resolver deliberately does differently — the two intentional
    /// divergences are pinned here too, so a later change has to argue with a
    /// test rather than quietly move them.
    #[test]
    fn package_exports_resolution_matches_the_documented_rules() {
        use ExpectedResolution::{Blocked, EndsWith, Unavailable};

        let cases = [
            ExportsCase {
                why: "nested conditions resolve inside the matched branch",
                manifest: r#"{"exports":{".":{"browser":{"import":"./b.mjs","default":"./b.js"},"default":"./n.js"}}}"#,
                files: &["b.mjs", "b.js", "n.js"],
                specifier: "pkg",
                target: BundleTarget::Client,
                expect: EndsWith("b.mjs"),
            },
            ExportsCase {
                why: "an ESM build beats a CommonJS one listed before it",
                manifest: r#"{"exports":{".":{"require":"./cjs.js","import":"./esm.mjs"}}}"#,
                files: &["cjs.js", "esm.mjs"],
                specifier: "pkg",
                target: BundleTarget::Client,
                expect: EndsWith("esm.mjs"),
            },
            ExportsCase {
                why: "require still answers a package that ships nothing else",
                manifest: r#"{"exports":{".":{"require":"./cjs.js"}}}"#,
                files: &["cjs.js"],
                specifier: "pkg",
                target: BundleTarget::Client,
                expect: EndsWith("cjs.js"),
            },
            ExportsCase {
                why: "author order still decides between competing conditions",
                manifest: r#"{"exports":{".":{"browser":"./b.js","import":"./esm.mjs"}}}"#,
                files: &["b.js", "esm.mjs"],
                specifier: "pkg",
                target: BundleTarget::Client,
                expect: EndsWith("b.js"),
            },
            ExportsCase {
                why: "an explicitly blocked subpath stays blocked",
                manifest: r#"{"exports":{".":"./index.js","./private":null}}"#,
                files: &["index.js", "private.js"],
                specifier: "pkg/private",
                target: BundleTarget::Client,
                expect: Blocked,
            },
            ExportsCase {
                why: "a wildcard target blocked by a more specific null",
                manifest: r#"{"exports":{"./*":"./src/*.js","./internal/*":null}}"#,
                files: &["src/internal/x.js"],
                specifier: "pkg/internal/x",
                target: BundleTarget::Client,
                expect: Blocked,
            },
            ExportsCase {
                why: "the more specific wildcard wins",
                manifest: r#"{"exports":{"./*":"./generic/*.js","./deep/*":"./special/*.js"}}"#,
                files: &["generic/deep/x.js", "special/x.js"],
                specifier: "pkg/deep/x",
                target: BundleTarget::Client,
                expect: EndsWith("special/x.js"),
            },
            ExportsCase {
                why: "a wildcard with a suffix",
                manifest: r#"{"exports":{"./*.js":"./src/*.js"}}"#,
                files: &["src/thing.js"],
                specifier: "pkg/thing.js",
                target: BundleTarget::Client,
                expect: EndsWith("src/thing.js"),
            },
            ExportsCase {
                // Node resolves an `exports` target exactly: no extension
                // search, no directory index. A target naming a file that is
                // not there is not there.
                why: "an export target is taken literally",
                manifest: r#"{"exports":{".":"./src/entry"}}"#,
                files: &["src/entry.js"],
                specifier: "pkg",
                target: BundleTarget::Ssr,
                expect: Unavailable,
            },
            ExportsCase {
                // Trailing-slash folder mappings were deprecated and then
                // removed in Node 17. Matching Node means refusing them.
                why: "a trailing-slash folder mapping is not honoured",
                manifest: r#"{"exports":{"./lib/":"./src/lib/"}}"#,
                files: &["src/lib/thing.js"],
                specifier: "pkg/lib/thing.js",
                target: BundleTarget::Client,
                expect: Unavailable,
            },
            ExportsCase {
                // Deliberately more permissive than Node, which raises
                // ERR_PACKAGE_PATH_NOT_EXPORTED. An omitted subpath falls
                // through to the legacy fields; only an explicit `null` blocks.
                why: "an unlisted subpath falls through instead of failing",
                manifest: r#"{"exports":{".":"./index.js"}}"#,
                files: &["index.js", "secret.js"],
                specifier: "pkg/secret",
                target: BundleTarget::Client,
                expect: EndsWith("secret.js"),
            },
            ExportsCase {
                why: "module beats main for a browser bundle",
                manifest: r#"{"main":"./cjs.js","module":"./esm.mjs"}"#,
                files: &["cjs.js", "esm.mjs"],
                specifier: "pkg",
                target: BundleTarget::Client,
                expect: EndsWith("esm.mjs"),
            },
            ExportsCase {
                why: "the legacy browser field is honoured",
                manifest: r#"{"main":"./node.js","browser":"./browser.js"}"#,
                files: &["node.js", "browser.js"],
                specifier: "pkg",
                target: BundleTarget::Client,
                expect: EndsWith("browser.js"),
            },
            ExportsCase {
                why: "main may name a directory with an index",
                manifest: r#"{"main":"./lib"}"#,
                files: &["lib/index.js"],
                specifier: "pkg",
                target: BundleTarget::Ssr,
                expect: EndsWith("index.js"),
            },
            ExportsCase {
                why: "a manifest with no entry fields falls back to index",
                manifest: r#"{"name":"pkg"}"#,
                files: &["index.js"],
                specifier: "pkg",
                target: BundleTarget::Ssr,
                expect: EndsWith("index.js"),
            },
            ExportsCase {
                why: "an edge bundle falls through to default",
                manifest: r#"{"exports":{".":{"node":"./node.js","default":"./universal.js"}}}"#,
                files: &["node.js", "universal.js"],
                specifier: "pkg",
                target: BundleTarget::Edge,
                expect: EndsWith("universal.js"),
            },
        ];

        for case in cases {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let pkg = root.join("node_modules").join("pkg");
            fs::create_dir_all(&pkg).unwrap();
            for file in case.files {
                let path = pkg.join(file);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, "export default 1;").unwrap();
            }
            fs::write(pkg.join("package.json"), case.manifest).unwrap();

            let cache = ResolveGraphCache::new();
            let got = resolve_package_exports(&cache, root, case.specifier, case.target);
            let why = case.why;
            match (case.expect, &got) {
                (EndsWith(tail), PackageExportsResolution::Resolved(path)) => assert!(
                    path.to_string_lossy().replace('\\', "/").ends_with(tail),
                    "{why}: expected a path ending in {tail}, got {path:?}"
                ),
                (Blocked, PackageExportsResolution::Blocked) => {}
                (Unavailable, PackageExportsResolution::Unavailable) => {}
                _ => panic!("{why}: unexpected resolution {got:?}"),
            }
        }
    }

    /// The `resolutionOrder` section of
    /// `tests/fixtures/module-resolution-conformance.json`.
    ///
    /// Every other section of that table describes a rule that applies once a
    /// package has been chosen. This one describes the walk that chooses it —
    /// which source answers a non-relative specifier, and in what order — which
    /// is the step the two graphs had drifted apart on. The JavaScript half is
    /// `resolution order` in
    /// `tests/packages/ruvyxa/module-resolution-contract.test.mjs`.
    #[test]
    fn resolution_order_matches_the_shared_table() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Fixture {
            resolution_order: Vec<Scenario>,
        }
        #[derive(serde::Deserialize)]
        struct Scenario {
            name: String,
            importer: String,
            files: BTreeMap<String, String>,
            cases: Vec<Case>,
        }
        #[derive(serde::Deserialize)]
        struct Case {
            name: String,
            specifier: String,
            /// Prefix the temporary project root, forward-slashed: the form a
            /// generated entry emits, and absolute on both hosts.
            #[serde(default)]
            absolute: bool,
            expect: Expect,
        }
        #[derive(serde::Deserialize)]
        #[serde(tag = "kind", rename_all = "lowercase")]
        enum Expect {
            Resolved { path: String },
            Diagnostic { code: String, names: String },
            External,
        }

        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../../tests/fixtures/module-resolution-conformance.json"
        ))
        .unwrap();

        for scenario in fixture.resolution_order {
            let temp = tempfile::tempdir().unwrap();
            let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
            for (relative, source) in &scenario.files {
                let file = root.join(relative);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, source).unwrap();
            }
            let base_dir = root.join(&scenario.importer);
            fs::create_dir_all(&base_dir).unwrap();
            let tsconfig = TsConfigPaths::load(&root);

            for case in &scenario.cases {
                let why = format!("{}: {}", scenario.name, case.name);
                let specifier = if case.absolute {
                    format!(
                        "{}/{}",
                        root.to_string_lossy().replace('\\', "/"),
                        case.specifier
                    )
                } else {
                    case.specifier.clone()
                };
                let source = format!(
                    "import * as module from {};\n",
                    serde_json::to_string(&specifier).unwrap()
                );
                let result = collect_deps_cached(
                    &source,
                    "ts",
                    &base_dir,
                    &root,
                    &tsconfig,
                    &ResolveGraphCache::new(),
                    &BuildHookPipeline::empty(),
                    BundleTarget::Client,
                    JsxRuntime::Automatic,
                );

                match &case.expect {
                    Expect::Resolved { path } => {
                        let deps = result.unwrap_or_else(|error| panic!("{why}: {error}"));
                        let resolved = deps
                            .aliases
                            .get(&specifier)
                            .unwrap_or_else(|| panic!("{why}: nothing answered the specifier"));
                        let actual = resolved
                            .strip_prefix(&root)
                            .unwrap_or_else(|_| panic!("{why}: {resolved:?} is outside {root:?}"))
                            .to_string_lossy()
                            .replace('\\', "/");
                        assert_eq!(&actual, path, "{why}");
                    }
                    Expect::Diagnostic { code, names } => {
                        let message = result
                            .err()
                            .unwrap_or_else(|| {
                                panic!("{why}: expected {code}, the build succeeded")
                            })
                            .to_string();
                        assert!(message.contains(code), "{why}: {message}");
                        assert!(message.contains(names), "{why}: {message}");
                    }
                    Expect::External => {
                        let deps = result.unwrap_or_else(|error| panic!("{why}: {error}"));
                        assert!(deps.aliases.is_empty(), "{why}: {:?}", deps.aliases);
                    }
                }
            }
        }
    }
}
