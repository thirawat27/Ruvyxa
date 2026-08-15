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
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedClientArtifact {
    pub(crate) version: u8,
    pub(crate) dependency_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) bundle: ClientBundle,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedClientPlan {
    pub(crate) version: u8,
    pub(crate) dependency_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) module_paths: BTreeSet<PathBuf>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedSharedRouteArtifact {
    pub(crate) version: u8,
    pub(crate) dependency_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) code: String,
    pub(crate) modules: Vec<PathBuf>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct CachedPrerenderArtifact {
    pub(crate) version: u8,
    pub(crate) dependency_hash: String,
    pub(crate) render_context_hash: String,
    pub(crate) renderer_dependency_hash: String,
    pub(crate) files: BTreeMap<PathBuf, String>,
    pub(crate) html: String,
}

#[derive(Clone)]
pub(crate) struct ClientRoutePlan {
    pub(crate) path: String,
    pub(crate) module_paths: BTreeSet<PathBuf>,
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
) -> anyhow::Result<serde_json::Value> {
    let page_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        // `export const hydrate = false` pages ship no client bundle at all;
        // prerender injection and the serve path skip them via the same flag.
        .filter(|route| route.render.hydrate)
        .cloned()
        .collect::<Vec<_>>();
    let parallelism = build_parallelism(build.parallelism, page_routes.len());
    let bundle_context =
        bundle_context_for_build(cache.dependency_hash, cache.directory, plugin_session)?;
    let artifact_cache_dir = cache.directory.to_path_buf();
    let artifact_dependency_hash = cache.dependency_hash.to_string();
    let artifact_fingerprints = ArtifactFingerprintCache::default();
    let empty_shared_modules = BTreeSet::new();
    let split_strategy = parse_split_strategy(build.split_strategy.as_deref())?;
    let (bundles, shared_route_chunks) = if split_strategy == ruvyxa_bundler::SplitStrategy::Route {
        let plan_variant = format!(
            "route-v2-manifest-{}",
            build.emit_chunk_manifest.unwrap_or(false)
        );
        let plans = bundle_routes_parallel(&page_routes, parallelism, |route| {
            prepare_client_route_plan(
                root,
                app_dir,
                route,
                build,
                &bundle_context,
                &artifact_cache_dir,
                &artifact_dependency_hash,
                &plan_variant,
                &artifact_fingerprints,
            )
        })?;
        let plans_by_route = plans
            .iter()
            .map(|(_, plan)| (plan.path.clone(), plan.clone()))
            .collect::<BTreeMap<_, _>>();
        let shared_modules = shared_route_module_paths(&plans);
        if shared_modules.is_empty() {
            let bundles = bundle_routes_parallel(&page_routes, parallelism, |route| {
                let prepared = plans_by_route
                    .get(&route.path)
                    .and_then(|plan| plan.prepared.as_deref());
                bundle_client_route(
                    root,
                    app_dir,
                    route,
                    build,
                    &bundle_context,
                    prepared,
                    &empty_shared_modules,
                    None,
                    &artifact_cache_dir,
                    &artifact_dependency_hash,
                    "base",
                    &artifact_fingerprints,
                )
            })?;
            (bundles, Vec::new())
        } else {
            let shared_options = client_bundle_options(build)?;
            let shared_variant = serde_json::to_string(&shared_options)?;
            let shared_output = if let Some(output) = load_shared_route_artifact(
                &artifact_cache_dir,
                &artifact_dependency_hash,
                &shared_modules,
                &shared_variant,
                &artifact_fingerprints,
            ) {
                output
            } else {
                let prepared_routes = plans
                    .iter()
                    .filter_map(|(_, plan)| plan.prepared.as_deref())
                    .collect::<Vec<_>>();
                let output = if prepared_routes.len() == plans.len()
                    && bundle_context.build_hooks().host_count() == 0
                {
                    ruvyxa_bundler::bundle_shared_prepared_route_modules(
                        &prepared_routes,
                        &shared_modules,
                        shared_options,
                    )
                } else {
                    ruvyxa_bundler::bundle_shared_route_modules(
                        ruvyxa_diagnostics::normalized_canonical_path(root),
                        ruvyxa_diagnostics::normalized_canonical_path(app_dir),
                        &shared_modules,
                        shared_options,
                        &bundle_context,
                    )
                }
                .map_err(|error| anyhow::anyhow!("Ruvyxa Bundler shared route error: {error}"))?;
                store_shared_route_artifact(
                    &artifact_cache_dir,
                    &artifact_dependency_hash,
                    &shared_modules,
                    &shared_variant,
                    &output,
                    &artifact_fingerprints,
                );
                output
            };
            let executable_modules = shared_output
                .modules
                .into_iter()
                .map(|path| ruvyxa_diagnostics::normalized_canonical_path(&path))
                .collect::<BTreeSet<_>>();
            let shared_chunk = emit_shared_route_chunk(
                client_dir,
                shared_output.code,
                &executable_modules,
                &plans,
            )?;
            let bundles = bundle_routes_parallel(&page_routes, parallelism, |route| {
                let plan = plans_by_route.get(&route.path);
                let route_shared_modules = plan.map_or_else(BTreeSet::new, |plan| {
                    plan.module_paths
                        .intersection(&executable_modules)
                        .cloned()
                        .collect::<BTreeSet<_>>()
                });
                let shared_file =
                    (!route_shared_modules.is_empty()).then_some(shared_chunk.file_name.as_str());
                bundle_client_route(
                    root,
                    app_dir,
                    route,
                    build,
                    &bundle_context,
                    plan.and_then(|plan| plan.prepared.as_deref()),
                    &route_shared_modules,
                    shared_file,
                    &artifact_cache_dir,
                    &artifact_dependency_hash,
                    &shared_chunk.file_name,
                    &artifact_fingerprints,
                )
            })?;
            (bundles, vec![shared_chunk])
        }
    } else {
        let bundles = bundle_routes_parallel(&page_routes, parallelism, |route| {
            bundle_client_route(
                root,
                app_dir,
                route,
                build,
                &bundle_context,
                None,
                &empty_shared_modules,
                None,
                &artifact_cache_dir,
                &artifact_dependency_hash,
                "base",
                &artifact_fingerprints,
            )
        })?;
        (bundles, Vec::new())
    };

    let mut routes = Vec::new();
    let mut route_chunk_manifests = Vec::new();
    let mut total_output_bytes = 0usize;
    let mut total_estimated_gz_bytes = 0usize;
    let mut total_duration_ms = 0u64;
    let mut total_modules = 0usize;
    let mut total_cache_hits = 0usize;
    let mut total_tree_shaken_modules = 0usize;

    for (_, bundle) in bundles {
        fs::write(client_dir.join(&bundle.file_name), bundle.script.as_bytes())?;
        if let (Some(source_map_file), Some(source_map)) =
            (&bundle.source_map_file, &bundle.source_map)
        {
            fs::write(client_dir.join(source_map_file), source_map.as_bytes())?;
        }
        total_output_bytes += bundle.output_bytes;
        total_estimated_gz_bytes += bundle.estimated_gz_bytes;
        total_duration_ms += bundle.duration_ms;
        total_modules += bundle.module_count;
        total_cache_hits += bundle.cache_hits;
        total_tree_shaken_modules += bundle.tree_shaken_modules;

        if let Some(chunk_manifest) = &bundle.chunk_manifest {
            route_chunk_manifests.push(chunk_manifest.clone());
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

    for route in &mut routes {
        let route_path = route
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or("/")
            .to_string();
        let hydration = page_routes
            .iter()
            .find(|entry| entry.path == route_path)
            .map(|entry| entry.render.hydration)
            .unwrap_or_default();
        route["hydration"] = serde_json::to_value(hydration)?;
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
            attach_shared_chunks_to_manifest(chunk_manifest, &shared_route_chunks);
        }
    }

    write_client_route_manifest(client_dir, &routes)?;

    if build.emit_chunk_manifest.unwrap_or(false) {
        fs::write(
            client_dir.join("chunk-manifest.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "routes": route_chunk_manifests
                    .iter()
                    .map(|manifest| {
                        let mut manifest = manifest.clone();
                        attach_shared_chunks_to_manifest(&mut manifest, &shared_route_chunks);
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

    let bundle_budget = bundle_budget_report(&routes);
    let cache_budget = bundle_context.enforce_cache_budget();
    // Persist only after every route artifact has been emitted successfully;
    // a failed batch leaves the previous dependency graph intact.
    bundle_context
        .save_incremental()
        .context("failed to persist incremental module graph")?;
    let artifact_graph = bundle_context.artifacts().stats();

    Ok(serde_json::json!({
        "chunkStrategy": build.split_strategy.as_deref().unwrap_or("route"),
        "minify": build.minify.unwrap_or(true),
        "sourcemap": build.sourcemap.unwrap_or(false),
        "treeShaking": build.tree_shaking.unwrap_or(true),
        "jsxRuntime": build.jsx_runtime.as_deref().unwrap_or("automatic"),
        "esTarget": build.es_target.as_deref().unwrap_or("es2022"),
        "emitChunkManifest": build.emit_chunk_manifest.unwrap_or(false),
        "parallelism": parallelism,
        "moduleCount": total_modules,
        "outputBytes": total_output_bytes,
        "estimatedGzBytes": total_estimated_gz_bytes,
        "durationMs": total_duration_ms,
        "cacheHits": total_cache_hits,
        "treeShakenModules": total_tree_shaken_modules,
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
    }))
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
    let cpu_budget = configured
        .map(|value| value.min(MAX_CONFIGURED_PRERENDER_PARALLELISM))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1)
                .min(MAX_PRERENDER_PARALLELISM)
        });
    crate::host_resources::prerender_worker_budget(cpu_budget).clamp(1, work_items.max(1))
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
) -> anyhow::Result<ClientBundle> {
    if let Some(bundle) = load_client_artifact(
        cache_dir,
        dependency_hash,
        &route.path,
        cache_variant,
        artifact_fingerprints,
    ) {
        return Ok(bundle);
    }
    let output = if let Some(prepared) = prepared {
        ruvyxa_bundler::bundle_prepared(prepared, shared_modules)
    } else {
        let input = client_bundle_input(root, app_dir, route, build)?;
        ruvyxa_bundler::bundle_with_shared_modules(input, bundle_context, shared_modules)
    }
    .map_err(|e| anyhow::anyhow!("Ruvyxa Bundler error for {}: {e}", route.path))?;

    // Report non-fatal diagnostics.
    for diagnostic in &output.diagnostics {
        tracing::warn!("{diagnostic}");
    }

    let code = shared_chunk_file.map_or_else(
        || output.code.clone(),
        |file_name| format!("import \"./{file_name}\";\n{}", output.code),
    );
    let hash = content_hash(&code);
    let file_name = format!("{hash}.js");
    let source_map_file = output.source_map.as_ref().map(|_| format!("{hash}.js.map"));
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
        source_map: output.source_map,
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
    };
    store_client_artifact(
        cache_dir,
        dependency_hash,
        &route.path,
        cache_variant,
        &bundle,
        artifact_fingerprints,
    );
    Ok(bundle)
}

pub(crate) fn client_bundle_input(
    root: &Path,
    app_dir: &Path,
    route: &RouteEntry,
    build: &BuildConfigOptions,
) -> anyhow::Result<ruvyxa_bundler::BundleInput> {
    use ruvyxa_bundler::{BundleInput, BundleOptions, BundleTarget};

    let root = ruvyxa_diagnostics::normalized_canonical_path(root);
    let app_dir = ruvyxa_diagnostics::normalized_canonical_path(app_dir);
    let entry = canonical_route_file(&root, &route.file);
    let layouts = route
        .layout_chain
        .iter()
        .filter_map(|layout_path| resolve_layout_file(&root, &app_dir, layout_path))
        .collect();
    let route_dir = entry.parent().unwrap_or(&app_dir).to_path_buf();
    let specials = resolve_route_specials(&app_dir, &route_dir);

    Ok(BundleInput {
        entry,
        project_root: root,
        app_dir,
        layouts,
        request_path: route.path.clone(),
        target: BundleTarget::Client,
        specials,
        options: BundleOptions {
            minify: build.minify.unwrap_or(true),
            source_map: build.sourcemap.unwrap_or(false),
            tree_shaking: build.tree_shaking.unwrap_or(true),
            jsx_runtime: parse_jsx_runtime(build.jsx_runtime.as_deref())?,
            es_target: parse_es_target(build.es_target.as_deref())?,
            split_strategy: parse_split_strategy(build.split_strategy.as_deref())?,
            emit_chunk_manifest: build.emit_chunk_manifest.unwrap_or(false),
            collect_module_manifest: parse_split_strategy(build.split_strategy.as_deref())?
                == ruvyxa_bundler::SplitStrategy::Route,
        },
    })
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
        ruvyxa_bundler::prepare_bundle(input, bundle_context)
            .map_err(|error| anyhow::anyhow!("Ruvyxa Bundler error for {}: {error}", route.path))?,
    );
    let module_paths = prepared
        .module_paths()
        .into_iter()
        .map(|path| ruvyxa_diagnostics::normalized_canonical_path(&path))
        .collect();
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
        es_target: parse_es_target(build.es_target.as_deref())?,
        split_strategy: parse_split_strategy(build.split_strategy.as_deref())?,
        emit_chunk_manifest: false,
        collect_module_manifest: false,
    })
}

pub(crate) fn shared_route_module_paths(plans: &[(usize, ClientRoutePlan)]) -> BTreeSet<PathBuf> {
    let mut module_routes = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for (_, plan) in plans {
        for module in &plan.module_paths {
            module_routes
                .entry(module.clone())
                .or_default()
                .insert(plan.path.clone());
        }
    }
    module_routes
        .into_iter()
        .filter_map(|(module, routes)| (routes.len() >= 2 && module.is_file()).then_some(module))
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
            "RUV1601 build.jsxRuntime must be `classic` or `automatic`, got `{other}`"
        ),
    }
}

pub(crate) fn parse_es_target(value: Option<&str>) -> anyhow::Result<ruvyxa_bundler::EsTarget> {
    match value.unwrap_or("es2022").to_ascii_lowercase().as_str() {
        "es2018" => Ok(ruvyxa_bundler::EsTarget::Es2018),
        "es2019" => Ok(ruvyxa_bundler::EsTarget::Es2019),
        "es2020" => Ok(ruvyxa_bundler::EsTarget::Es2020),
        "es2022" => Ok(ruvyxa_bundler::EsTarget::Es2022),
        "esnext" => Ok(ruvyxa_bundler::EsTarget::EsNext),
        other => anyhow::bail!(
            "RUV1601 build.esTarget must be es2018, es2019, es2020, es2022, or esnext, got `{other}`"
        ),
    }
}

pub(crate) fn parse_split_strategy(
    value: Option<&str>,
) -> anyhow::Result<ruvyxa_bundler::SplitStrategy> {
    match value.unwrap_or("route").to_ascii_lowercase().as_str() {
        "single" | "manual" => Ok(ruvyxa_bundler::SplitStrategy::Single),
        "route" => Ok(ruvyxa_bundler::SplitStrategy::Route),
        other => anyhow::bail!(
            "RUV1601 build.splitStrategy must be `single`, `route`, or `manual`, got `{other}`"
        ),
    }
}
