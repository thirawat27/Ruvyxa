//! Client bundle emission: one browser bundle per route, plus shared chunks.
//!
//! Routes are bundled concurrently, but the work is deliberately staged so the
//! concurrency stays correct rather than merely wide:
//!
//! 1. Each route is *prepared* — resolved and compiled — producing a plan.
//! 2. Modules appearing in more than one plan are lifted into a shared chunk,
//!    so a module common to many routes is evaluated once in the browser
//!    instead of once per route.
//! 3. Each route is linked against that shared chunk and written out.
//!
//! Steps 1 and 3 fan out; step 2 is a barrier because a shared chunk cannot be
//! decided from one route's view of the graph.
//!
//! Every route's output is content-addressed, so an unchanged route reuses its
//! cached artifact instead of re-bundling — see [`crate::artifact_cache`].

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Context;
use ruvyxa_graph::{HydrationMode, RouteEntry, RouteManifest};

use crate::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ClientBundle {
    pub(crate) path: String,
    pub(crate) entry: PathBuf,
    pub(crate) file_name: String,
    pub(crate) script: String,
    pub(crate) source_map_file: Option<String>,
    pub(crate) source_map: Option<String>,
    pub(crate) output_bytes: usize,
    pub(crate) estimated_gz_bytes: usize,
    pub(crate) duration_ms: u64,
    pub(crate) module_count: usize,
    pub(crate) cache_hits: usize,
    pub(crate) tree_shaken_modules: usize,
    pub(crate) artifact_cache_hit: bool,
    pub(crate) module_paths: BTreeSet<PathBuf>,
    pub(crate) dependency_paths: BTreeSet<PathBuf>,
    pub(crate) chunk_manifest: Option<serde_json::Value>,
    pub(crate) chunks: Vec<ruvyxa_bundler::OutputChunk>,
    /// Non-fatal boundary diagnostics this bundle's modules produced.
    ///
    /// Carried on the bundle rather than reported where the bundler ran,
    /// because the bundler does not run on an artifact-cache hit. It used to be
    /// reported there, so a `RUV1008` — a private `process.env` read reachable
    /// from browser code — printed on the first build of a project and on no
    /// build after it. The warning became a function of cache state instead of
    /// the code, which is the one thing a warning must never be.
    ///
    /// Rendered rather than structured: `Diagnostic::code` is a `&'static str`,
    /// which cannot be deserialized from a cache file, and the only thing this
    /// field is for is printing the same warning again.
    ///
    /// Deliberately not `#[serde(default)]`: an artifact written before this
    /// field existed must fail to parse and be rebuilt, because loading it as
    /// "no diagnostics" is exactly the silence this exists to remove.
    pub(crate) diagnostics: Vec<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedClientArtifact {
    pub(crate) dependency_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) bundle: ClientBundle,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedClientPlan {
    pub(crate) dependency_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    /// Ordered, because the shared chunk is emitted in this order and a plan
    /// read from disk has to place its modules where a freshly prepared one
    /// would. Version 3 is where it stopped being a sorted set.
    pub(crate) module_paths: Vec<PathBuf>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedSharedRouteArtifact {
    pub(crate) dependency_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) code: String,
    pub(crate) modules: Vec<PathBuf>,
    /// Not `#[serde(default)]`, for the same reason as `ClientBundle`: an
    /// artifact written before this field existed must be rebuilt rather than
    /// load as "this shared chunk produced no warnings".
    pub(crate) diagnostics: Vec<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedPrerenderArtifact {
    pub(crate) dependency_hash: String,
    pub(crate) render_context_hash: String,
    pub(crate) renderer_dependency_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) html: String,
}

/// What one `react-server` compile told the build about one route.
///
/// Cached because producing it is the most expensive single step of a warm
/// production build and none of it depends on the request: two compiles run in
/// a Node worker to answer two questions — which modules of this route are
/// client references, and which `'use server'` modules those references reach.
/// Both answers are a pure function of the files the two compiles read, which
/// is exactly what `files` records.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedServerComponentEntry {
    pub(crate) dependency_hash: String,
    /// Covers the worker runtime and the environment it was started with — the
    /// inputs that are not project files. See
    /// [`crate::artifact_cache::server_component_context_hash`].
    pub(crate) context_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) entry_source: String,
    pub(crate) server_references: Vec<ruvyxa_dev_server::ServerReferenceSource>,
}

/// Where a route's `react-server` answer is cached, and what makes it stale.
pub(crate) struct ServerComponentEntryCache {
    pub(crate) directory: PathBuf,
    pub(crate) dependency_hash: String,
    pub(crate) context_hash: String,
    pub(crate) fingerprints: Arc<ArtifactFingerprintCache>,
}

#[derive(Clone)]
pub(crate) struct ClientRoutePlan {
    pub(crate) path: String,
    /// This route's static modules in the order it evaluates them. See
    /// [`ruvyxa_bundler::PreparedBundle::module_paths`] for why the order is
    /// carried rather than sorted away.
    pub(crate) module_paths: Vec<PathBuf>,
    pub(crate) prepared: Option<Arc<ruvyxa_bundler::PreparedBundle>>,
}

/// One production build observes a stable content snapshot. Sharing these
/// fingerprints prevents common layouts and dependencies from being read and
/// hashed once per route while retaining content-based cache invalidation.
#[derive(Default)]
pub(crate) struct ArtifactFingerprintCache {
    pub(crate) entries: Mutex<BTreeMap<PathBuf, Arc<OnceLock<Option<String>>>>>,
}

impl ArtifactFingerprintCache {
    pub(crate) fn fingerprint(&self, path: &Path) -> Option<String> {
        let cell = {
            let mut entries = self.entries.lock().ok()?;
            entries
                .entry(path.to_path_buf())
                .or_insert_with(|| Arc::new(OnceLock::new()))
                .clone()
        };
        cell.get_or_init(|| {
            fs::read(path)
                .ok()
                .map(|source| content_hash_bytes(&source))
        })
        .clone()
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }
}

/// Lines the shared-chunk import adds above a route bundle.
///
/// Named because the source map has to be shifted by exactly this much: the
/// number is a fact about the emitted file, shared by the writer and the map.
const PREPENDED_IMPORT_LINES: usize = 1;

pub(crate) struct SharedRouteChunk {
    pub(crate) file_name: String,
    pub(crate) code: String,
    pub(crate) modules: Vec<String>,
    pub(crate) routes: Vec<String>,
}

#[cfg(test)]
pub(crate) fn emit_client_bundles(
    root: &Path,
    app_dir: &Path,
    manifest: &RouteManifest,
    client_dir: &Path,
    build: &BuildConfigOptions,
    plugins: &[BuildPluginConfig],
    cache: RuvyxaBuildCache<'_>,
) -> anyhow::Result<serde_json::Value> {
    let plugin_session = TypeScriptPluginBuildSession::new(
        root,
        plugins,
        ruvyxa_dev_server::JavaScriptRuntime::Node,
        false,
        false,
    )?;
    emit_client_bundles_with_session(
        root,
        app_dir,
        manifest,
        client_dir,
        build,
        plugins,
        cache,
        &plugin_session,
        // The analyzer path has no worker, so no server-components entries.
        &ServerComponentEntries::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_client_bundles_with_session(
    root: &Path,
    app_dir: &Path,
    manifest: &RouteManifest,
    client_dir: &Path,
    build: &BuildConfigOptions,
    plugins: &[BuildPluginConfig],
    cache: RuvyxaBuildCache<'_>,
    plugin_session: &TypeScriptPluginBuildSession,
    rsc_entries: &ServerComponentEntries,
) -> anyhow::Result<serde_json::Value> {
    // A server-components route's browser bundle holds the `'use client'`
    // modules its payload references, not the page — and only the
    // `react-server` graph knows which those are, so it wrote the entry. The
    // route is bundled here with every other one anyway: its modules have to
    // join the shared-chunk analysis, or its bundle inlines a second copy of
    // React and a soft navigation into it renders with two Reacts on the page.
    let page_routes = client_page_routes(manifest, rsc_entries);
    let pass = ClientBundlePass {
        root,
        app_dir,
        build,
        parallelism: build_parallelism(build.parallelism, page_routes.len()),
        bundle_context: bundle_context_for_build(
            cache.dependency_hash,
            cache.directory,
            plugin_session,
            &rsc_entries.server_references,
        )?,
        artifact_cache_dir: cache.directory.to_path_buf(),
        artifact_dependency_hash: cache.dependency_hash.to_string(),
        artifact_fingerprints: ArtifactFingerprintCache::default(),
        empty_shared_modules: BTreeSet::new(),
        split_strategy: parse_split_strategy(build.split_strategy.as_deref())?,
        page_routes,
        rsc_entries,
    };
    let bundled = pass.bundle_page_routes(client_dir)?;
    let shared_route_chunks = bundled.shared_chunks;

    let written = write_route_bundles(client_dir, bundled.bundles, build)?;
    let mut routes = written.routes;

    finalize_route_entries(
        client_dir,
        &mut routes,
        &pass.page_routes,
        &shared_route_chunks,
    )?;

    write_client_route_manifest(client_dir, &routes)?;

    write_chunk_manifest(
        client_dir,
        build,
        &written.chunk_manifests,
        &shared_route_chunks,
    )?;

    // Persist only after every route artifact has been emitted successfully;
    // a failed batch leaves the previous dependency graph intact.
    pass.bundle_context
        .save_incremental()
        .context("failed to persist incremental module graph")?;

    Ok(client_bundle_report(
        build,
        plugins,
        pass.parallelism,
        &written.totals,
        routes,
        &shared_route_chunks,
        &pass.bundle_context,
    ))
}

/// Everything a client-bundling pass reads but does not decide.
///
/// The route-splitting branch below needs eleven values that the caller has
/// already resolved. Naming them together is what lets the split strategy be a
/// method with one argument instead of a twelve-parameter function, and it puts
/// the caller's setup in one place rather than interleaved with the branch.
struct ClientBundlePass<'a> {
    root: &'a Path,
    app_dir: &'a Path,
    build: &'a BuildConfigOptions,
    bundle_context: ruvyxa_bundler::BundleContext,
    artifact_cache_dir: PathBuf,
    artifact_dependency_hash: String,
    artifact_fingerprints: ArtifactFingerprintCache,
    empty_shared_modules: BTreeSet<PathBuf>,
    parallelism: usize,
    split_strategy: ruvyxa_bundler::SplitStrategy,
    page_routes: Vec<RouteEntry>,
    rsc_entries: &'a ServerComponentEntries,
}

impl ClientBundlePass<'_> {
    /// A server-components route's browser bundle holds the `'use client'`
    /// modules its payload references, not the page — and only the
    /// `react-server` graph knows which those are, so it wrote the entry. The
    /// route is bundled here with every other one anyway: its modules have to
    /// join the shared-chunk analysis, or its bundle inlines a second copy of
    /// React and a soft navigation into it renders with two Reacts on the page.
    fn entry_source_for(&self, route: &RouteEntry) -> Option<&str> {
        self.rsc_entries
            .entries
            .get(&route.path)
            .map(String::as_str)
    }

    /// Bundle every page route, and the shared chunk they read from when the
    /// route-split strategy found modules worth sharing.
    fn bundle_page_routes(&self, client_dir: &Path) -> anyhow::Result<BundledRoutes> {
        if self.split_strategy != ruvyxa_bundler::SplitStrategy::Route {
            return Ok(BundledRoutes {
                bundles: self.bundle_routes_alone(None)?,
                shared_chunks: Vec::new(),
            });
        }
        let plan_variant = client_route_plan_variant(self.build)?;
        let plans = bundle_routes_parallel(&self.page_routes, self.parallelism, |route| {
            prepare_client_route_plan(
                self.root,
                self.app_dir,
                route,
                self.build,
                &self.bundle_context,
                &self.artifact_cache_dir,
                &self.artifact_dependency_hash,
                &plan_variant,
                &self.artifact_fingerprints,
                self.entry_source_for(route),
            )
        })?;
        let plans_by_route = plans
            .iter()
            .map(|(_, plan)| (plan.path.clone(), plan.clone()))
            .collect::<BTreeMap<_, _>>();
        let shared_modules = shared_route_module_paths(&plans);
        if shared_modules.is_empty() {
            return Ok(BundledRoutes {
                bundles: self.bundle_routes_alone(Some(&plans_by_route))?,
                shared_chunks: Vec::new(),
            });
        }
        self.bundle_routes_around_shared_chunk(client_dir, &plans, &plans_by_route, &shared_modules)
    }

    /// Bundle each route on its own, reusing a prepared plan where one exists.
    ///
    /// The two callers differ only in whether a plan is available: the
    /// non-route strategy never prepared one, and the route strategy prepared
    /// plans that turned out to share nothing worth hoisting. Everything past
    /// that — no shared modules, no shared chunk file, the `base` cache variant
    /// — is the same answer, and it was written out twice.
    fn bundle_routes_alone(
        &self,
        plans_by_route: Option<&BTreeMap<String, ClientRoutePlan>>,
    ) -> anyhow::Result<Vec<(usize, ClientBundle)>> {
        bundle_routes_parallel(&self.page_routes, self.parallelism, |route| {
            bundle_client_route(
                self.root,
                self.app_dir,
                route,
                self.build,
                &self.bundle_context,
                plans_by_route
                    .and_then(|plans| plans.get(&route.path))
                    .and_then(|plan| plan.prepared.as_deref()),
                &self.empty_shared_modules,
                None,
                &self.artifact_cache_dir,
                &self.artifact_dependency_hash,
                "base",
                &self.artifact_fingerprints,
                self.entry_source_for(route),
            )
        })
    }

    /// Emit the chunk these routes share, then bundle each route against it.
    fn bundle_routes_around_shared_chunk(
        &self,
        client_dir: &Path,
        plans: &[(usize, ClientRoutePlan)],
        plans_by_route: &BTreeMap<String, ClientRoutePlan>,
        shared_modules: &[PathBuf],
    ) -> anyhow::Result<BundledRoutes> {
        let shared_output = self.shared_chunk_output(plans, shared_modules)?;
        let executable_modules = shared_output
            .modules
            .into_iter()
            .map(|path| ruvyxa_diagnostics::normalized_canonical_path(&path))
            .collect::<BTreeSet<_>>();
        let shared_chunk =
            emit_shared_route_chunk(client_dir, shared_output.code, &executable_modules, plans)?;
        let bundles = bundle_routes_parallel(&self.page_routes, self.parallelism, |route| {
            let plan = plans_by_route.get(&route.path);
            // A set, not a list: this one answers "does this route read that
            // module from the registry", and nothing about order.
            let route_shared_modules = plan.map_or_else(BTreeSet::new, |plan| {
                plan.module_paths
                    .iter()
                    .filter(|module| executable_modules.contains(*module))
                    .cloned()
                    .collect::<BTreeSet<_>>()
            });
            let shared_file =
                (!route_shared_modules.is_empty()).then_some(shared_chunk.file_name.as_str());
            bundle_client_route(
                self.root,
                self.app_dir,
                route,
                self.build,
                &self.bundle_context,
                plan.and_then(|plan| plan.prepared.as_deref()),
                &route_shared_modules,
                shared_file,
                &self.artifact_cache_dir,
                &self.artifact_dependency_hash,
                &shared_chunk.file_name,
                &self.artifact_fingerprints,
                self.entry_source_for(route),
            )
        })?;
        Ok(BundledRoutes {
            bundles,
            shared_chunks: vec![shared_chunk],
        })
    }

    /// The shared chunk's code and module list, from the artifact cache when it
    /// is warm and from the bundler when it is not.
    fn shared_chunk_output(
        &self,
        plans: &[(usize, ClientRoutePlan)],
        shared_modules: &[PathBuf],
    ) -> anyhow::Result<ruvyxa_bundler::SharedRouteBundleOutput> {
        let shared_options = client_bundle_options(self.build)?;
        let shared_variant = serde_json::to_string(&shared_options)?;
        if let Some(output) = load_shared_route_artifact(
            &self.artifact_cache_dir,
            &self.artifact_dependency_hash,
            shared_modules,
            &shared_variant,
            &self.artifact_fingerprints,
        ) {
            // Same rule as a cached route bundle: a warning belongs to the code,
            // so it is reprinted whether or not the bundler ran this time.
            report_shared_chunk_diagnostics(&output);
            return Ok(output);
        }
        // Every route having a prepared bundle means the shared chunk can be
        // composed from what is already compiled; a build hook can rewrite a
        // module after that point, so its presence sends this back through the
        // bundler.
        let prepared_routes = plans
            .iter()
            .filter_map(|(_, plan)| plan.prepared.as_deref())
            .collect::<Vec<_>>();
        let output = if prepared_routes.len() == plans.len()
            && self.bundle_context.build_hooks().host_count() == 0
        {
            ruvyxa_bundler::bundle_shared_prepared_route_modules(
                &prepared_routes,
                shared_modules,
                shared_options,
            )
        } else {
            ruvyxa_bundler::bundle_shared_route_modules(
                ruvyxa_diagnostics::normalized_canonical_path(self.root),
                ruvyxa_diagnostics::normalized_canonical_path(self.app_dir),
                shared_modules,
                shared_options,
                &self.bundle_context,
            )
        }
        .map_err(|error| anyhow::anyhow!("Ruvyxa Bundler shared route error: {error}"))?;
        store_shared_route_artifact(
            &self.artifact_cache_dir,
            &self.artifact_dependency_hash,
            shared_modules,
            &shared_variant,
            &output,
            &self.artifact_fingerprints,
        );
        report_shared_chunk_diagnostics(&output);
        Ok(output)
    }
}

/// Print a shared chunk's non-fatal diagnostics, however the chunk was obtained.
///
/// The bundler used to collect these into a local `Vec` and drop it, so a module
/// that reaches the browser only through the shared chunk — one imported by two
/// or more routes — warned about nothing, while the same module inside a single
/// route's bundle warned normally.
fn report_shared_chunk_diagnostics(output: &ruvyxa_bundler::SharedRouteBundleOutput) {
    for diagnostic in &output.diagnostics {
        tracing::warn!("{diagnostic}");
    }
}

/// One bundling pass's output: a bundle per page route, plus the shared
/// chunks those bundles read modules from. Named rather than returned as a
/// tuple because the two halves are read far apart — the bundles are written
/// immediately, the chunks decorate the manifest at the end.
struct BundledRoutes {
    bundles: Vec<(usize, ClientBundle)>,
    shared_chunks: Vec<SharedRouteChunk>,
}

/// Running totals over every route bundle written in one pass.
#[derive(Default)]
struct ClientBundleTotals {
    output_bytes: usize,
    estimated_gz_bytes: usize,
    duration_ms: u64,
    modules: usize,
    cache_hits: usize,
    tree_shaken_modules: usize,
}

/// What writing the route bundles produced: the manifest entry per route, the
/// per-route chunk manifests the optional `chunk-manifest.json` republishes,
/// and the totals the build report quotes.
struct WrittenRouteBundles {
    routes: Vec<serde_json::Value>,
    chunk_manifests: Vec<serde_json::Value>,
    totals: ClientBundleTotals,
}

/// The routes that get a browser bundle, and the three reasons one does not.
fn client_page_routes(
    manifest: &RouteManifest,
    rsc_entries: &ServerComponentEntries,
) -> Vec<RouteEntry> {
    manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        // `export const hydrate = false` pages ship no client bundle at all;
        // prerender injection and the serve path skip them via the same flag.
        .filter(|route| route.render.ships_client_bundle())
        // A server-components route without its entry is skipped rather than
        // bundled from the generated one, which imports the page — the module
        // this pipeline exists to keep out of the browser. `ruvyxa analyze`
        // takes that branch and omits such routes from its report; `ruvyxa
        // build` always supplies them.
        .filter(|route| {
            !route.render.server_components || rsc_entries.entries.contains_key(&route.path)
        })
        .cloned()
        .collect()
}

/// Write every route bundle, its source map, and its chunks, collecting the
/// manifest entry each one contributes.
fn write_route_bundles(
    client_dir: &Path,
    bundles: Vec<(usize, ClientBundle)>,
    build: &BuildConfigOptions,
) -> anyhow::Result<WrittenRouteBundles> {
    let mut routes = Vec::new();
    let mut chunk_manifests = Vec::new();
    let mut totals = ClientBundleTotals::default();

    for (_, bundle) in bundles {
        fs::write(client_dir.join(&bundle.file_name), bundle.script.as_bytes())?;
        if let (Some(source_map_file), Some(source_map)) =
            (&bundle.source_map_file, &bundle.source_map)
        {
            fs::write(client_dir.join(source_map_file), source_map.as_bytes())?;
        }
        totals.output_bytes += bundle.output_bytes;
        totals.estimated_gz_bytes += bundle.estimated_gz_bytes;
        totals.duration_ms += bundle.duration_ms;
        totals.modules += bundle.module_count;
        totals.cache_hits += bundle.cache_hits;
        totals.tree_shaken_modules += bundle.tree_shaken_modules;

        if let Some(chunk_manifest) = &bundle.chunk_manifest {
            chunk_manifests.push(chunk_manifest.clone());
        }

        for chunk in &bundle.chunks {
            fs::write(client_dir.join(&chunk.file_name), chunk.code.as_bytes())?;
        }

        let mut route_info = serde_json::json!({
            "path": bundle.path,
            "entry": bundle.entry,
            "file": bundle.file_name,
            "src": format!("/__ruvyxa/client/{}", bundle.file_name),
            "sourceMap": bundle.source_map_file,
            "bytes": bundle.script.len(),
            "outputBytes": bundle.output_bytes,
            "estimatedGzBytes": bundle.estimated_gz_bytes,
            "durationMs": bundle.duration_ms,
            "moduleCount": bundle.module_count,
            "cacheHits": bundle.cache_hits,
            "artifactCacheHit": bundle.artifact_cache_hit,
            "treeShakenModules": bundle.tree_shaken_modules,
            "optimized": true,
            "treeShaken": build.tree_shaking.unwrap_or(true),
            "chunkStrategy": build.split_strategy.as_deref().unwrap_or("route")
        });
        let source = fs::read_to_string(&bundle.entry).unwrap_or_default();
        let module = ruvyxa_bundler::ast::parse_module(&source);
        let has_flight = ruvyxa_bundler::ast::has_named_runtime_export(&source, &module, "flight");
        let uses_cache =
            ruvyxa_bundler::reference_manifest::has_module_directive(&source, "use cache");
        if uses_cache && !has_flight {
            anyhow::bail!(
                "RUV1842 {} declares 'use cache' without exporting flight(context); Ruvyxa cache directives apply to the public Flight producer",
                bundle.entry.display()
            );
        }
        route_info["flight"] = serde_json::Value::Bool(has_flight);
        route_info["cache"] = serde_json::Value::Bool(uses_cache);

        if let Some(chunk_manifest) = bundle.chunk_manifest {
            if let Some(version) = chunk_manifest
                .pointer("/referenceManifest/artifactVersion")
                .and_then(serde_json::Value::as_str)
            {
                route_info["artifactVersion"] = serde_json::Value::String(version.to_string());
            }
            route_info["chunkManifest"] = chunk_manifest;
        }
        route_info["chunks"] = serde_json::Value::Array(
            bundle
                .chunks
                .iter()
                .map(output_chunk_manifest)
                .collect::<Vec<_>>(),
        );

        routes.push(route_info);
    }
    Ok(WrittenRouteBundles {
        routes,
        chunk_manifests,
        totals,
    })
}

/// Add to each manifest entry what only the whole set of routes could decide:
/// its hydration mode, the deferred-hydration loader shared by every route that
/// needs one, and the shared chunks it reads modules from.
fn finalize_route_entries(
    client_dir: &Path,
    routes: &mut [serde_json::Value],
    page_routes: &[RouteEntry],
    shared_route_chunks: &[SharedRouteChunk],
) -> anyhow::Result<()> {
    let hydration_loader = page_routes
        .iter()
        .any(|route| {
            matches!(
                route.render.hydration,
                HydrationMode::Idle | HydrationMode::Visible
            )
        })
        .then(|| {
            let source = ruvyxa_dev_server::hydration_loader_source();
            let file_name = format!("hydration-{}.js", &content_hash(source)[..16]);
            fs::write(client_dir.join(&file_name), source)?;
            Ok::<_, std::io::Error>(format!("/__ruvyxa/client/{file_name}"))
        })
        .transpose()?;

    for route in routes.iter_mut() {
        let route_path = route
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("/")
            .to_string();
        let page_route = page_routes.iter().find(|entry| entry.path == route_path);
        let hydration = page_route
            .map(|entry| entry.render.hydration)
            .unwrap_or_default();
        route["hydration"] = serde_json::to_value(hydration)?;
        // Read by the client router: a navigation into such a route fetches a
        // Flight payload instead of calling a registered tree factory, because
        // the page it would build a tree from is not in the bundle.
        if page_route.is_some_and(|entry| entry.render.server_components) {
            route["serverComponents"] = serde_json::Value::Bool(true);
        }
        if matches!(hydration, HydrationMode::Idle | HydrationMode::Visible)
            && let Some(loader) = &hydration_loader
        {
            route["hydrationLoader"] = serde_json::Value::String(loader.clone());
        }
        let route_shared_chunks = shared_route_chunks
            .iter()
            .filter(|chunk| chunk.routes.iter().any(|path| path == &route_path))
            .map(shared_route_chunk_manifest)
            .collect::<Vec<_>>();
        route["sharedChunks"] = serde_json::Value::Array(route_shared_chunks);
        if let Some(chunk_manifest) = route.get_mut("chunkManifest") {
            attach_shared_chunks_to_manifest(chunk_manifest, shared_route_chunks);
        }
    }
    Ok(())
}

/// Emit `chunk-manifest.json` when the build asked for it.
fn write_chunk_manifest(
    client_dir: &Path,
    build: &BuildConfigOptions,
    chunk_manifests: &[serde_json::Value],
    shared_route_chunks: &[SharedRouteChunk],
) -> anyhow::Result<()> {
    if build.emit_chunk_manifest.unwrap_or(false) {
        fs::write(
            client_dir.join("chunk-manifest.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "routes": chunk_manifests
                    .iter()
                    .map(|manifest| {
                        let mut manifest = manifest.clone();
                        attach_shared_chunks_to_manifest(&mut manifest, shared_route_chunks);
                        manifest
                    })
                    .collect::<Vec<_>>(),
                "shared": shared_route_chunks
                    .iter()
                    .map(shared_route_chunk_manifest)
                    .collect::<Vec<_>>()
            }))?,
        )?;
    }
    Ok(())
}

/// The build report for this client pass. Pure formatting over what the pass
/// already decided, apart from the cache-budget sweep it triggers.
fn client_bundle_report(
    build: &BuildConfigOptions,
    plugins: &[BuildPluginConfig],
    parallelism: usize,
    totals: &ClientBundleTotals,
    routes: Vec<serde_json::Value>,
    shared_route_chunks: &[SharedRouteChunk],
    bundle_context: &ruvyxa_bundler::BundleContext,
) -> serde_json::Value {
    let bundle_budget = bundle_budget_report(&routes);
    let cache_budget = bundle_context.enforce_cache_budget();
    let artifact_graph = bundle_context.artifacts().stats();

    serde_json::json!({
        "chunkStrategy": build.split_strategy.as_deref().unwrap_or("route"),
        "minify": build.minify.unwrap_or(true),
        "sourcemap": build.sourcemap.unwrap_or(false),
        "treeShaking": build.tree_shaking.unwrap_or(true),
        "jsxRuntime": build.jsx_runtime.as_deref().unwrap_or("automatic"),
        "emitChunkManifest": build.emit_chunk_manifest.unwrap_or(false),
        "parallelism": parallelism,
        "moduleCount": totals.modules,
        "outputBytes": totals.output_bytes,
        "estimatedGzBytes": totals.estimated_gz_bytes,
        "durationMs": totals.duration_ms,
        "cacheHits": totals.cache_hits,
        "treeShakenModules": totals.tree_shaken_modules,
        "budget": bundle_budget,
        "plugins": build_plugin_manifest(plugins),
        "sharedRouteChunks": shared_route_chunks
            .iter()
            .map(shared_route_chunk_manifest)
            .collect::<Vec<_>>(),
        "cache": {
            "directory": bundle_context.compile_cache().cache_dir(),
            "compileEntries": bundle_context.compile_cache().entry_count(),
            "compileBytes": bundle_context.compile_cache().total_bytes(),
            "graphHits": bundle_context.incremental().edge_hits(),
            "graphModules": bundle_context.incremental().current_module_count(),
            "artifactGraph": artifact_graph,
            "budget": cache_budget,
            "compiler": bundle_context.compile_cache().stats(),
            "resolver": bundle_context.graph_cache().stats()
        },
        "routes": routes
    })
}

pub(crate) fn bundle_routes_parallel<F, T>(
    routes: &[RouteEntry],
    parallelism: usize,
    bundle_route: F,
) -> anyhow::Result<Vec<(usize, T)>>
where
    F: Fn(&RouteEntry) -> anyhow::Result<T> + Sync,
    T: Send,
{
    if routes.is_empty() {
        return Ok(Vec::new());
    }

    if parallelism <= 1 || routes.len() == 1 {
        return routes
            .iter()
            .enumerate()
            .map(|(index, route)| bundle_route(route).map(|bundle| (index, bundle)))
            .collect();
    }

    // Route complexity varies substantially, so static contiguous chunks can
    // leave one worker processing an expensive tail while its peers sit idle.
    // A shared atomic cursor gives the bounded outer workers dynamic scheduling.
    // Keep them as scoped OS threads rather than Rayon workers: nested bundler
    // jobs can then use Rayon's global module pool instead of recursively
    // competing with route jobs in the same scheduler.
    let next_route = AtomicUsize::new(0);
    let worker_count = parallelism.min(routes.len());
    let mut outcomes = std::thread::scope(|scope| -> anyhow::Result<Vec<_>> {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let next_route = &next_route;
            let bundle_route = &bundle_route;
            handles.push(scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    let index = next_route.fetch_add(1, Ordering::Relaxed);
                    let Some(route) = routes.get(index) else {
                        break;
                    };
                    local.push((index, bundle_route(route)));
                }
                local
            }));
        }

        let mut outcomes = Vec::with_capacity(routes.len());
        for handle in handles {
            outcomes.extend(
                handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("client bundler worker panicked"))?,
            );
        }
        Ok(outcomes)
    })?;
    outcomes.sort_by_key(|(index, _)| *index);
    outcomes
        .into_iter()
        .map(|(index, outcome)| outcome.map(|bundle| (index, bundle)))
        .collect()
}

/// First-load size a route is expected to stay under.
///
/// This is an observation, not a failing contract — but it is the number the
/// build table colours against, so the two always agree about what "large"
/// means.
pub(crate) const DEFAULT_FIRST_LOAD_BUDGET_BYTES: usize = 250 * 1024;

/// Summarize first-load bundle offenders without turning a build observation
/// into a new failing production contract.
pub(crate) fn bundle_budget_report(routes: &[serde_json::Value]) -> serde_json::Value {
    let mut offenders = routes
        .iter()
        .map(|route| {
            let first_load = first_load_bytes(route);
            serde_json::json!({
                "path": route.get("path").and_then(serde_json::Value::as_str).unwrap_or("/"),
                "firstLoadBytes": first_load,
                "estimatedGzBytes": route.get("estimatedGzBytes").and_then(serde_json::Value::as_u64).unwrap_or_default(),
                "overBudget": first_load > DEFAULT_FIRST_LOAD_BUDGET_BYTES
            })
        })
        .collect::<Vec<_>>();
    offenders.sort_by(|left, right| {
        right["firstLoadBytes"]
            .as_u64()
            .cmp(&left["firstLoadBytes"].as_u64())
            .then_with(|| left["path"].as_str().cmp(&right["path"].as_str()))
    });
    let over_budget_count = offenders
        .iter()
        .filter(|route| route["overBudget"].as_bool() == Some(true))
        .count();
    serde_json::json!({
        "firstLoadBytes": DEFAULT_FIRST_LOAD_BUDGET_BYTES,
        "overBudgetCount": over_budget_count,
        "topRoutes": offenders.into_iter().take(10).collect::<Vec<_>>(),
    })
}

/// How many routes are bundled at once.
///
/// Three limits apply, and the smallest wins: how much work there is, how many
/// cores are available (honouring an operator's `RAYON_NUM_THREADS`), and how
/// much memory is free. The memory bound is what stops a many-core machine from
/// reserving hundreds of megabytes for parallelism that measurement shows it
/// barely converts into speed — and what stops a memory-capped CI container
/// from being killed for asking.
///
/// An explicit `build.workers` still sets the CPU budget, but it no longer
/// escapes the memory bound: a value copied from another project's config must
/// not decide how much memory this machine is asked for.
pub(crate) fn build_parallelism(configured: Option<usize>, work_items: usize) -> usize {
    let cpu_budget = configured.unwrap_or_else(rayon::current_num_threads);
    crate::host_resources::bundle_worker_budget(cpu_budget).clamp(1, work_items.max(1))
}

/// How many routes are prerendered at once.
///
/// Each worker is a whole JavaScript runtime process, which is why the ceiling
/// is far lower than for bundling and why the memory bound matters more here:
/// on the demo these processes account for more resident memory than the CLI
/// itself.
pub(crate) fn prerender_parallelism(configured: Option<usize>, work_items: usize) -> usize {
    crate::host_resources::prerender_worker_budget(prerender_cpu_budget(configured))
        .clamp(1, work_items.max(1))
}

/// The CPU-side ceiling, before the host's free memory has a say.
///
/// Split out because it is the only part of the decision a test can state an
/// exact number for. `prerender_parallelism` also passes through
/// `prerender_worker_budget`, which lowers the answer when the machine is short
/// on memory — by design, and the reason a test asserting
/// `prerender_parallelism(Some(64), 32) == MAX_CONFIGURED_PRERENDER_PARALLELISM`
/// passed on an idle machine and failed on a busy one. It was asserting the
/// host's free memory, which is not a property of this code.
pub(crate) fn prerender_cpu_budget(configured: Option<usize>) -> usize {
    configured
        .map(|value| value.min(MAX_CONFIGURED_PRERENDER_PARALLELISM))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1)
                .min(MAX_PRERENDER_PARALLELISM)
        })
}

/// Flatten every plugin's declared head elements in configuration order.
///
/// Order is the order plugins are listed, so a project controls which entry
/// wins when two plugins contribute the same tag.
pub(crate) fn collect_plugin_head(
    plugins: &[BuildPluginConfig],
) -> Vec<ruvyxa_dev_server::PluginHeadEntry> {
    plugins
        .iter()
        .flat_map(|plugin| plugin.head.iter().cloned())
        .collect()
}

pub(crate) fn build_plugin_manifest(plugins: &[BuildPluginConfig]) -> serde_json::Value {
    serde_json::Value::Array(
        plugins
            .iter()
            .map(|plugin| serde_json::json!({ "name": plugin.name }))
            .collect(),
    )
}

/// Bundle a client route using Ruvyxa Bundler (`ruvyxa_bundler`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn bundle_client_route(
    root: &Path,
    app_dir: &Path,
    route: &RouteEntry,
    build: &BuildConfigOptions,
    bundle_context: &ruvyxa_bundler::BundleContext,
    prepared: Option<&ruvyxa_bundler::PreparedBundle>,
    shared_modules: &BTreeSet<PathBuf>,
    shared_chunk_file: Option<&str>,
    cache_dir: &Path,
    dependency_hash: &str,
    cache_variant: &str,
    artifact_fingerprints: &ArtifactFingerprintCache,
    entry_source: Option<&str>,
) -> anyhow::Result<ClientBundle> {
    if let Some(bundle) = load_client_artifact(
        cache_dir,
        dependency_hash,
        &route.path,
        cache_variant,
        artifact_fingerprints,
    ) {
        // A cached bundle reports what the bundler found when it produced it.
        // Skipping this is what made a boundary warning appear once and never
        // again.
        report_bundle_diagnostics(&bundle);
        return Ok(bundle);
    }
    let output = if let Some(prepared) = prepared {
        ruvyxa_bundler::bundle_prepared(prepared, shared_modules)
    } else {
        let input = client_bundle_input(root, app_dir, route, build)?;
        match entry_source {
            // A server-components route with no cached plan: the entry the
            // `react-server` graph wrote is the only description of what its
            // browser bundle contains.
            Some(source) => {
                ruvyxa_bundler::bundle_entry_source(source, input, bundle_context, shared_modules)
            }
            None => {
                ruvyxa_bundler::bundle_with_shared_modules(input, bundle_context, shared_modules)
            }
        }
    }
    .map_err(|e| anyhow::anyhow!("Ruvyxa Bundler error for {}: {e}", route.path))?;

    // Reported below, from the finished bundle, so the cache-hit path above
    // reports the same set rather than nothing.
    let diagnostics = output
        .diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    // Prepending to the bundle moves every line the map describes, so the map
    // moves with it. Without this the shared-chunk import shifted the whole
    // file by one line and the map pointed one line short of everything.
    let (code, source_map) = match shared_chunk_file {
        Some(file_name) => (
            format!("import \"./{file_name}\";\n{}", output.code),
            output.source_map.as_deref().and_then(|map| {
                ruvyxa_bundler::sourcemap::shift_generated_lines(map, PREPENDED_IMPORT_LINES)
            }),
        ),
        None => (output.code.clone(), output.source_map.clone()),
    };
    let hash = content_hash(&code);
    let file_name = format!("{hash}.js");
    let source_map_file = source_map.as_ref().map(|_| format!("{hash}.js.map"));
    let script = if let Some(source_map_file) = &source_map_file {
        format!("{code}\n//# sourceMappingURL={source_map_file}\n")
    } else {
        code.clone()
    };
    let module_paths: BTreeSet<PathBuf> = output
        .chunk_manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .modules
                .iter()
                .map(PathBuf::from)
                .map(|path| ruvyxa_diagnostics::normalized_canonical_path(&path))
                .collect()
        })
        .unwrap_or_default();
    let dependency_paths = module_paths
        .iter()
        .cloned()
        .chain(output.chunks.iter().flat_map(|chunk| {
            chunk
                .modules
                .iter()
                .map(PathBuf::from)
                .map(|path| ruvyxa_diagnostics::normalized_canonical_path(&path))
        }))
        .collect();

    let bundle = ClientBundle {
        path: route.path.clone(),
        entry: route.file.clone(),
        file_name,
        script,
        source_map_file,
        source_map,
        output_bytes: code.len(),
        estimated_gz_bytes: (code.len() as f64 * 0.35) as usize,
        duration_ms: output.stats.duration_ms,
        module_count: output.stats.module_count,
        cache_hits: output.stats.cache_hits,
        tree_shaken_modules: output.stats.tree_shaken_modules,
        artifact_cache_hit: false,
        module_paths,
        dependency_paths,
        chunk_manifest: output
            .chunk_manifest
            .map(serde_json::to_value)
            .transpose()?,
        chunks: output.chunks,
        diagnostics,
    };
    store_client_artifact(
        cache_dir,
        dependency_hash,
        &route.path,
        cache_variant,
        &bundle,
        artifact_fingerprints,
    );
    report_bundle_diagnostics(&bundle);
    Ok(bundle)
}

/// Print a bundle's non-fatal diagnostics, however the bundle was obtained.
///
/// One reporter for both paths. While it lived inline beside the bundler call,
/// a cache hit returned early and said nothing, so `RUV1008` was printed on a
/// cold build and on no warm one — the finding this function exists to close.
fn report_bundle_diagnostics(bundle: &ClientBundle) {
    for diagnostic in &bundle.diagnostics {
        tracing::warn!("{diagnostic}");
    }
}

pub(crate) fn client_bundle_input(
    root: &Path,
    app_dir: &Path,
    route: &RouteEntry,
    build: &BuildConfigOptions,
) -> anyhow::Result<ruvyxa_bundler::BundleInput> {
    use ruvyxa_bundler::{BundleInput, BundleTarget};

    let root = ruvyxa_diagnostics::normalized_canonical_path(root);
    let app_dir = ruvyxa_diagnostics::normalized_canonical_path(app_dir);
    let entry = canonical_route_file(&root, &route.file);
    let layouts = route
        .layout_chain
        .iter()
        .filter_map(|layout_path| resolve_layout_file(&root, &app_dir, layout_path))
        .collect();
    // Resolved the same way as layouts: the chain holds route ids, and the same
    // resolver turns one into the file on disk regardless of which file it names.
    let templates = route
        .template_chain
        .iter()
        .filter_map(|template_path| resolve_layout_file(&root, &app_dir, template_path))
        .collect();
    // Slot files are already absolute paths from discovery; the level is a
    // route id, resolved to a directory the same way a layout id is.
    let slots = route
        .slots
        .iter()
        .map(|slot| ruvyxa_bundler::RouteSlotInput {
            level: app_dir.join(
                slot.level
                    .strip_prefix("app")
                    .unwrap_or(&slot.level)
                    .trim_start_matches('/'),
            ),
            name: slot.name.clone(),
            file: ruvyxa_diagnostics::normalized_canonical_path(&slot.file),
        })
        .collect();
    // Interception files are absolute paths from discovery; the level is a
    // route id, resolved to a directory the way a slot's level is, and carried
    // through as an id as well because the emitted source names it.
    let intercepts = route
        .intercepts
        .iter()
        .map(|intercept| ruvyxa_bundler::RouteInterceptInput {
            level: app_dir.join(
                intercept
                    .level
                    .strip_prefix("app")
                    .unwrap_or(&intercept.level)
                    .trim_start_matches('/'),
            ),
            level_id: intercept.level.clone(),
            name: intercept.name.clone(),
            target: intercept.target.clone(),
            file: ruvyxa_diagnostics::normalized_canonical_path(&intercept.file),
        })
        .collect();
    let route_dir = entry.parent().unwrap_or(&app_dir).to_path_buf();
    let specials = resolve_route_specials(&app_dir, &route_dir);

    Ok(BundleInput {
        entry,
        project_root: root,
        app_dir,
        layouts,
        templates,
        slots,
        intercepts,
        request_path: route.path.clone(),
        target: BundleTarget::Client,
        specials,
        options: client_route_bundle_options(build)?,
    })
}

/// The bundler options a client route bundle is compiled with.
///
/// One place, because two things read them and they must agree: the compile
/// itself, and the cache key of the plan that compile produces. They did not
/// agree — the plan key named a literal (`route-v2-manifest-<bool>`) that
/// encoded only `emitChunkManifest`, so changing `jsx`, `target`, `minify`, or
/// `treeShake` left the key equal and a warm build reused a plan built under
/// the previous options. A `jsx` change alone moves the module set, because
/// the automatic runtime imports `react/jsx-runtime` and the classic one does
/// not.
pub(crate) fn client_route_bundle_options(
    build: &BuildConfigOptions,
) -> anyhow::Result<ruvyxa_bundler::BundleOptions> {
    let split_strategy = parse_split_strategy(build.split_strategy.as_deref())?;
    Ok(ruvyxa_bundler::BundleOptions {
        minify: build.minify.unwrap_or(true),
        source_map: build.sourcemap.unwrap_or(false),
        tree_shaking: build.tree_shaking.unwrap_or(true),
        jsx_runtime: parse_jsx_runtime(build.jsx_runtime.as_deref())?,
        es_target: parse_es_target(build.es_target.as_ref())?,
        split_strategy,
        emit_chunk_manifest: build.emit_chunk_manifest.unwrap_or(false),
        collect_module_manifest: split_strategy == ruvyxa_bundler::SplitStrategy::Route,
    })
}

/// The cache identity of a client route plan: the options that produced it.
///
/// Derived rather than stamped. A literal here has to be edited by hand every
/// time the plan's shape or meaning changes, and forgetting the edit is
/// silent — the same reason `MANIFEST_VERSION` in the bundler's
/// `incremental.rs` carries no counter.
pub(crate) fn client_route_plan_variant(build: &BuildConfigOptions) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&client_route_bundle_options(build)?)?)
}

/// Resolve the special files (`error.tsx` / `loading.tsx` / `not-found.tsx`)
/// that apply to a route, nearest-wins from the app root down to `route_dir`.
///
/// Mirrors `collectSpecials` in `packages/ruvyxa/runtime/compiler.mjs` — the
/// dev server and adapters discover these from the filesystem the same way, so
/// a built client bundle composes the identical boundary a dev render does.
pub(crate) fn resolve_route_specials(
    app_dir: &Path,
    route_dir: &Path,
) -> ruvyxa_bundler::RouteSpecials {
    let mut specials = ruvyxa_bundler::RouteSpecials::default();

    let mut dirs = vec![app_dir.to_path_buf()];
    if let Ok(relative) = route_dir.strip_prefix(app_dir) {
        let mut current = app_dir.to_path_buf();
        for component in relative.components() {
            if let std::path::Component::Normal(segment) = component {
                current.push(segment);
                dirs.push(current.clone());
            }
        }
    }

    for dir in dirs {
        let error = dir.join("error.tsx");
        if error.is_file() {
            specials.error = Some(error);
        }
        let loading = dir.join("loading.tsx");
        if loading.is_file() {
            specials.loading = Some(loading);
        }
        let not_found = dir.join("not-found.tsx");
        if not_found.is_file() {
            specials.not_found = Some(not_found);
        }
    }

    specials
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_client_route_plan(
    root: &Path,
    app_dir: &Path,
    route: &RouteEntry,
    build: &BuildConfigOptions,
    bundle_context: &ruvyxa_bundler::BundleContext,
    cache_dir: &Path,
    dependency_hash: &str,
    cache_variant: &str,
    fingerprints: &ArtifactFingerprintCache,
    entry_source: Option<&str>,
) -> anyhow::Result<ClientRoutePlan> {
    if let Some(module_paths) = load_client_plan(
        cache_dir,
        dependency_hash,
        &route.path,
        cache_variant,
        fingerprints,
    ) {
        return Ok(ClientRoutePlan {
            path: route.path.clone(),
            module_paths,
            prepared: None,
        });
    }

    let input = client_bundle_input(root, app_dir, route, build)?;
    let prepared = Arc::new(
        match entry_source {
            Some(source) => {
                ruvyxa_bundler::prepare_bundle_entry_source(source, input, bundle_context)
            }
            None => ruvyxa_bundler::prepare_bundle(input, bundle_context),
        }
        .map_err(|error| anyhow::anyhow!("Ruvyxa Bundler error for {}: {error}", route.path))?,
    );
    let module_paths = prepared
        .module_paths()
        .into_iter()
        .map(|path| ruvyxa_diagnostics::normalized_canonical_path(&path))
        .collect::<Vec<_>>();
    let dependency_paths = prepared
        .dependency_paths()
        .into_iter()
        .map(|path| ruvyxa_diagnostics::normalized_canonical_path(&path))
        .collect::<BTreeSet<_>>();
    store_client_plan(
        cache_dir,
        dependency_hash,
        &route.path,
        cache_variant,
        &module_paths,
        &dependency_paths,
        fingerprints,
    );
    Ok(ClientRoutePlan {
        path: route.path.clone(),
        module_paths,
        prepared: Some(prepared),
    })
}

pub(crate) fn output_chunk_manifest(chunk: &ruvyxa_bundler::OutputChunk) -> serde_json::Value {
    serde_json::json!({
        "file": chunk.file_name,
        "src": format!("/__ruvyxa/client/{}", chunk.file_name),
        "kind": chunk.kind,
        "modules": chunk.modules,
        "bytes": chunk.code.len()
    })
}

pub(crate) fn client_bundle_options(
    build: &BuildConfigOptions,
) -> anyhow::Result<ruvyxa_bundler::BundleOptions> {
    Ok(ruvyxa_bundler::BundleOptions {
        minify: build.minify.unwrap_or(true),
        source_map: false,
        tree_shaking: false,
        jsx_runtime: parse_jsx_runtime(build.jsx_runtime.as_deref())?,
        es_target: parse_es_target(build.es_target.as_ref())?,
        split_strategy: parse_split_strategy(build.split_strategy.as_deref())?,
        emit_chunk_manifest: false,
        collect_module_manifest: false,
    })
}

/// Modules more than one route evaluates, in the order the routes evaluate them.
///
/// Two answers in one pass, and the second is the one that used to be missing.
/// *Which* modules is a counting question and order-free. *In what order* is
/// not: a route bundle `import`s the shared chunk, so the whole chunk runs
/// before the route's own first statement, and the route's import order stops
/// deciding anything about the modules inside it. Whatever order this returns
/// is the order the browser evaluates them in.
///
/// Sorting them by pathname — which is what collecting into a `BTreeSet` did —
/// is a real reordering of the browser's work, and it is invisible until two
/// modules have a load-order dependency the graph cannot express. One does:
/// `react-server-dom-webpack/client.browser` reads a global that
/// `rsc-client-install.mjs` defines, with no import between them, and sorted by
/// path the reader came first and every server-components page stopped
/// hydrating in production.
///
/// The order is taken from the routes themselves, first occurrence winning.
/// Each route's list is already dependency-ordered, so a module lands ahead of
/// everything that depends on it, and the routes are walked in a fixed order so
/// two builds of one tree agree.
pub(crate) fn shared_route_module_paths(plans: &[(usize, ClientRoutePlan)]) -> Vec<PathBuf> {
    let mut module_routes = BTreeMap::<&PathBuf, BTreeSet<&str>>::new();
    for (_, plan) in plans {
        for module in &plan.module_paths {
            module_routes
                .entry(module)
                .or_default()
                .insert(plan.path.as_str());
        }
    }
    let mut seen = BTreeSet::new();
    plans
        .iter()
        .flat_map(|(_, plan)| plan.module_paths.iter())
        .filter(|module| {
            module_routes
                .get(*module)
                .is_some_and(|routes| routes.len() >= 2)
                && module.is_file()
        })
        .filter(|module| seen.insert((*module).clone()))
        .cloned()
        .collect()
}

pub(crate) fn emit_shared_route_chunk(
    client_dir: &Path,
    code: String,
    module_paths: &BTreeSet<PathBuf>,
    plans: &[(usize, ClientRoutePlan)],
) -> anyhow::Result<SharedRouteChunk> {
    let modules = module_paths
        .iter()
        .map(|path| path.display().to_string().replace('\\', "/"))
        .collect::<Vec<_>>();
    let routes = plans
        .iter()
        .filter(|(_, plan)| {
            plan.module_paths
                .iter()
                .any(|module| module_paths.contains(module))
        })
        .map(|(_, plan)| plan.path.clone())
        .collect::<Vec<_>>();
    let file_name = format!("shared.{}.js", content_hash(&code));
    fs::write(client_dir.join(&file_name), code.as_bytes())?;

    Ok(SharedRouteChunk {
        file_name,
        code,
        modules,
        routes,
    })
}

/// Emit the lean route table the browser router fetches for soft navigation.
///
/// `manifest.json` is a build report: it carries absolute source paths, module
/// lists, byte counts, and per-route chunk graphs — none of which the browser
/// needs, and the absolute paths of which should never be shipped to clients.
/// This sibling file exposes only `{ path, src, sharedChunks, artifactVersion }` per
/// page route, so `@ruvyxa/react`'s router downloads kilobytes, not the full
/// build manifest. The dev server synthesizes the same shape at
/// `/__ruvyxa/client/route-manifest.json`.
/// Write the project's compiled CSS as a content-addressed client asset.
///
/// Returns the URL a document should link, or `None` when the project has no
/// CSS at all. Named by the hash of its own bytes, like every other emitted
/// chunk, so a byte-identical rebuild produces the same URL and a changed
/// stylesheet can never be served from a cache under the old one.
///
/// The URL is also recorded in `route-manifest.json`, which is the file every
/// host already reads to find a route's scripts: the Rust server, the generated
/// standalone server, and each adapter's function bundle.
pub(crate) fn write_style_asset(client_dir: &Path, css: &str) -> std::io::Result<Option<String>> {
    if css.trim().is_empty() {
        return Ok(None);
    }
    fs::create_dir_all(client_dir)?;
    let digest = blake3::hash(css.as_bytes()).to_hex();
    let file_name = format!("styles.{}.css", &digest[..16]);
    fs::write(client_dir.join(&file_name), css)?;

    let url = format!("/__ruvyxa/client/{file_name}");
    let manifest_path = client_dir.join("route-manifest.json");
    if manifest_path.exists() {
        let mut manifest: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)
            .unwrap_or_else(
                |_| serde_json::json!({ "routes": serde_json::Value::Array(Vec::new()) }),
            );
        manifest["styles"] = serde_json::json!([url.clone()]);
        fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
    }
    Ok(Some(url))
}

pub(crate) fn write_client_route_manifest(
    client_dir: &Path,
    routes: &[serde_json::Value],
) -> std::io::Result<()> {
    let lean = routes
        .iter()
        .filter_map(|route| {
            let path = route.get("path")?.as_str()?;
            let src = route.get("src")?.as_str()?;
            let shared = route
                .get("sharedChunks")
                .and_then(|chunks| chunks.as_array())
                .into_iter()
                .flatten()
                .filter_map(|chunk| chunk.get("src").and_then(|src| src.as_str()))
                .map(|src| serde_json::json!({ "src": src }))
                .collect::<Vec<_>>();
            let artifact_version = route
                .get("chunkManifest")
                .and_then(|manifest| manifest.get("referenceManifest"))
                .and_then(|manifest| manifest.get("artifactVersion"))
                .and_then(|version| version.as_str());
            let mut entry = serde_json::json!({
                "path": path,
                "src": src,
                "sharedChunks": shared,
                "flight": route.get("flight").and_then(|value| value.as_bool()).unwrap_or(false),
                "cache": route.get("cache").and_then(|value| value.as_bool()).unwrap_or(false)
            });
            // Present only when true. The router reads it to decide whether a
            // navigation renders a registered tree factory or a Flight payload
            // fetched from `/__ruvyxa/rsc`; every other route keeps the entry it
            // had.
            if route
                .get("serverComponents")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                entry["serverComponents"] = serde_json::Value::Bool(true);
            }
            if let Some(artifact_version) = artifact_version {
                entry["artifactVersion"] = serde_json::Value::String(artifact_version.to_string());
            }
            Some(entry)
        })
        .collect::<Vec<_>>();

    fs::write(
        client_dir.join("route-manifest.json"),
        serde_json::to_vec(&serde_json::json!({ "routes": lean }))?,
    )
}

pub(crate) fn shared_route_chunk_manifest(chunk: &SharedRouteChunk) -> serde_json::Value {
    serde_json::json!({
        "file": chunk.file_name,
        "src": format!("/__ruvyxa/client/{}", chunk.file_name),
        "modules": chunk.modules,
        "routes": chunk.routes,
        "bytes": chunk.code.len()
    })
}

pub(crate) fn attach_shared_chunks_to_manifest(
    manifest: &mut serde_json::Value,
    shared_chunks: &[SharedRouteChunk],
) {
    let route_modules = manifest
        .get("modules")
        .and_then(|value| value.as_array())
        .map(|modules| {
            modules
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();

    let route_shared = shared_chunks
        .iter()
        .filter(|chunk| {
            chunk
                .modules
                .iter()
                .any(|module| route_modules.contains(module.as_str()))
        })
        .map(shared_route_chunk_manifest)
        .collect::<Vec<_>>();

    manifest["sharedChunks"] = serde_json::Value::Array(route_shared);
}

pub(crate) fn parse_jsx_runtime(value: Option<&str>) -> anyhow::Result<ruvyxa_bundler::JsxRuntime> {
    match value.unwrap_or("automatic").to_ascii_lowercase().as_str() {
        "classic" => Ok(ruvyxa_bundler::JsxRuntime::Classic),
        "automatic" => Ok(ruvyxa_bundler::JsxRuntime::Automatic),
        other => anyhow::bail!(
            // Name the key as a user writes it in `ruvyxa.config.ts`, not the
            // Rust field it deserializes into. `jsx_runtime` is
            // `#[serde(rename = "jsx")]`, so "build.jsxRuntime" sent readers
            // looking for a key that does not exist.
            "RUV1601 build.jsx must be `classic` or `automatic`, got `{other}`"
        ),
    }
}

/// Read `build.target` into the language level both compilers apply.
///
/// Absent means [`ruvyxa_bundler::EsTarget::EsNext`], which is what every
/// project got while the key reached no transform — so a build that configures
/// nothing keeps emitting exactly the bytes it did before.
pub(crate) fn parse_es_target(
    value: Option<&serde_json::Value>,
) -> anyhow::Result<ruvyxa_bundler::EsTarget> {
    let Some(value) = value else {
        return Ok(ruvyxa_bundler::EsTarget::EsNext);
    };
    let accepted = ruvyxa_bundler::EsTarget::ALL
        .iter()
        .map(|target| target.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let Some(text) = value.as_str() else {
        anyhow::bail!("RUV1601 build.target must be a string, one of: {accepted}");
    };
    ruvyxa_bundler::EsTarget::parse(text).ok_or_else(|| {
        // `es5` is the one a reader is most likely to try, and oxc does not
        // implement it, so say that rather than leaving it to the list.
        let hint = if text.trim().eq_ignore_ascii_case("es5") {
            " (es5 is not implemented by the transformer)"
        } else {
            ""
        };
        anyhow::anyhow!("RUV1601 build.target must be one of: {accepted}, got `{text}`{hint}")
    })
}

pub(crate) fn parse_split_strategy(
    value: Option<&str>,
) -> anyhow::Result<ruvyxa_bundler::SplitStrategy> {
    match value.unwrap_or("route").to_ascii_lowercase().as_str() {
        "single" | "manual" => Ok(ruvyxa_bundler::SplitStrategy::Single),
        "route" => Ok(ruvyxa_bundler::SplitStrategy::Route),
        other => anyhow::bail!(
            "RUV1601 build.split must be `single`, `route`, or `manual`, got `{other}`"
        ),
    }
}

/// Ask a worker for every server-components route's browser entry source.
///
/// These entries cannot be generated here: only the `react-server` graph knows
/// which of a route's modules are client references, and it is the graph that
/// produced the payload naming them. It writes the entry; the Rust bundler
/// compiles it alongside every other route, which is what puts its modules into
/// the shared-chunk analysis and gives the page one React.
///
/// Building the bundle in the worker instead was tried and rejected twice: this
/// package's JavaScript compiler does not minify and does not fold
/// `process.env.NODE_ENV`, so the output carried both of React's builds and came
/// to 1.5 MB for a page with one button on it; and a bundle emitted outside the
/// shared-chunk analysis inlines its own React, which a soft navigation into the
/// route then renders beside the mounted page's copy.
///
/// Returns an empty map — starting no worker — for an app that uses no server
/// components.
///
/// A route whose cached answer is still valid is served from disk and never
/// reaches a worker, and a build where every route hits starts no worker
/// process at all. Both compiles behind one answer are expensive — together
/// they were the largest single cost of a warm production build of the demo,
/// ahead of bundling, pre-rendering, and asset preparation combined — and
/// neither depends on anything but the files it read.
pub(crate) async fn collect_server_component_entries(
    root: &Path,
    app_dir: &Path,
    manifest: &RouteManifest,
    build: &BuildConfigOptions,
    runtime: ruvyxa_dev_server::JavaScriptRuntime,
    cache: Option<&ServerComponentEntryCache>,
) -> anyhow::Result<ServerComponentEntries> {
    let routes = server_component_page_routes(manifest);
    if routes.is_empty() {
        return Ok(ServerComponentEntries::default());
    }

    let mut collected = ServerComponentEntries::default();
    let mut pending = Vec::new();
    for route in routes {
        match cache.and_then(|cache| {
            crate::artifact_cache::load_server_component_entry(cache, &route.path)
        }) {
            Some(entry) => {
                collected
                    .entries
                    .insert(route.path.clone(), entry.entry_source);
                collected.server_references.extend(entry.server_references);
            }
            None => pending.push(route),
        }
    }
    if pending.is_empty() {
        return Ok(collected);
    }

    let worker_env = crate::prerender::build_worker_env(root, build, runtime)?;
    let pool = ruvyxa_dev_server::NodeWorkerPool::start_with_size_and_runtime(
        root,
        worker_env,
        Some(1),
        runtime,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    for route in &pending {
        let response = pool
            .rsc_client_entry(root, app_dir, &route.file, &route.path)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "Server-components client entry failed for {}: {error}",
                    route.path
                )
            })?;
        if !response.ok {
            let code = response.code.unwrap_or_else(|| "RUV1300".to_string());
            let message = ruvyxa_diagnostics::worker_failure_message(response.message);
            pool.shutdown().await;
            anyhow::bail!(
                "Server-components client entry failed for {}: {}",
                route.path,
                // The worker's message usually already opens with its own code,
                // and joining by hand printed both: `RUV1700 RUV1863 …`.
                ruvyxa_diagnostics::label_with_code(&code, &message)
            );
        }
        let source = response.entry_source.ok_or_else(|| {
            anyhow::anyhow!(
                "Server-components client entry for {} produced no source",
                route.path
            )
        })?;
        let server_references = response.server_references.unwrap_or_default();
        if let Some(cache) = cache {
            // Stored against the inputs of *both* compiles behind this answer,
            // which is why the worker reports their union: the `react-server`
            // graph reads a `'use client'` module and stops, so the `'use
            // server'` module behind one is known only to the registry compile
            // — and the reference ids in this answer are versioned by that
            // module's source.
            crate::artifact_cache::store_server_component_entry(
                cache,
                &route.path,
                &response.inputs.unwrap_or_default(),
                &source,
                &server_references,
            );
        }
        collected.entries.insert(route.path.clone(), source);
        collected.server_references.extend(server_references);
    }
    pool.shutdown().await;
    Ok(collected)
}

/// What the `react-server` graph reported about every server-components route.
///
/// Two answers, collected in one pass because they come from one compile: the
/// browser entry each route needs, and the `'use server'` modules that entry's
/// graph will reach. Both have to be in hand before the shared-chunk analysis
/// runs, and only a worker can produce either.
#[derive(Debug, Default)]
pub(crate) struct ServerComponentEntries {
    /// Browser entry source, keyed by route pattern.
    pub(crate) entries: BTreeMap<String, String>,
    /// Every `'use server'` module those entries reach, with its stand-in
    /// source. Accumulated across routes; two routes reaching one actions file
    /// report the same id and the same text.
    pub(crate) server_references: Vec<ruvyxa_dev_server::ServerReferenceSource>,
}

/// Page routes that render through the server-components pipeline and ship JS.
pub(crate) fn server_component_page_routes(manifest: &RouteManifest) -> Vec<RouteEntry> {
    manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .filter(|route| route.render.server_components)
        .filter(|route| route.render.ships_client_bundle())
        .cloned()
        .collect()
}
