//! # ruvyxa_bundler
//!
//! Ruvyxa Bundler TypeScript/JSX compiler and module bundler for the Ruvyxa framework.
//!
//! This crate provides the Ruvyxa Bundler production pipeline and
//! integrates directly with [`ruvyxa_diagnostics`]
//! and the route graph from `ruvyxa_graph`.
//!
//! ## Pipeline
//!
//! ```text
//! Entry file (TSX/TS/JSX/JS)
//!   └─ resolver   → resolve all imports to absolute paths
//!                   (package.json `exports` map, tsconfig `paths`/`baseUrl`)
//!   └─ compiler   → Oxc TypeScript stripping + JSX transform (classic or automatic runtime)
//!                   + Ruvyxa decorator compatibility pre-pass
//!   └─ boundary   → enforce server/client rules (RUV1007, RUV1008, RUV1010)
//!   └─ linker     → topological sort + concatenate modules
//!                   (circular dependency detection)
//!   └─ minifier   → linker-aware export pruning + Oxc AST compression/mangling
//!   └─ output     → wrap in IIFE (client) or ESM (SSR)
//!                   (chunk manifest + HTML preload hints)
//! ```

#[path = "task_graph.rs"]
pub mod artifact_graph;
pub mod ast;
pub mod atomic_file;
pub mod boundary;
pub mod cache;
#[path = "cache_budget.rs"]
pub mod cache_budget;
pub mod chunking;
pub mod compiler;
pub mod content;
pub mod context;
#[path = "glob.rs"]
mod glob_import;
pub mod hooks;
pub mod incremental;
pub mod linker;
pub mod minifier;
pub mod output;
#[path = "references.rs"]
pub mod reference_manifest;
pub mod resolver;
pub mod sourcemap;
pub mod style_module;
pub mod types;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::cache::CompileCache;
use crate::chunking::{
    build_dynamic_output_chunks, dynamic_import_chunks, plan_dynamic_chunk_files,
    static_entry_modules,
};
use crate::hooks::BuildHookPipeline;
use crate::resolver::ResolveGraphCache;
use artifact_graph::{ArtifactDependency, ArtifactKey, ArtifactKind, ArtifactTaskGraph};
pub use context::BundleContext;
pub use types::*;

/// A route graph that has already completed resolution, compilation, boundary
/// validation, and dynamic-import planning.
///
/// Keeping this plan in memory lets callers discover common route modules and
/// then emit the final route bundle without repeating the expensive front half
/// of the bundling pipeline.
pub struct PreparedBundle {
    input: BundleInput,
    compiled: Vec<compiler::CompiledModule>,
    hook_source_maps: BTreeMap<PathBuf, String>,
    diagnostics: Vec<ruvyxa_diagnostics::Diagnostic>,
    dynamic_import_files: BTreeMap<PathBuf, String>,
    static_modules: Vec<compiler::CompiledModule>,
    graph_module_count: usize,
    prepare_duration: Duration,
    artifact_graph: ArtifactTaskGraph,
    chunk_plan_key: ArtifactKey,
    watch_paths: BTreeSet<PathBuf>,
    glob_matches: BTreeSet<PathBuf>,
}

impl PreparedBundle {
    /// Project modules in the static entry graph, using the same ordering and
    /// selection rules as the emitted chunk manifest.
    #[must_use]
    pub fn module_paths(&self) -> BTreeSet<PathBuf> {
        linker::ordered_project_modules(&self.static_modules)
            .into_iter()
            .filter(|module| !module.is_external)
            .map(|module| module.path.clone())
            .collect()
    }

    /// Every project module compiled for this route, including modules emitted
    /// into dynamic-import chunks. Callers can use this complete set for cache
    /// invalidation without changing static shared-chunk membership.
    #[must_use]
    pub fn dependency_paths(&self) -> BTreeSet<PathBuf> {
        self.compiled
            .iter()
            .filter(|module| !module.is_external)
            .map(|module| module.path.clone())
            .chain(self.watch_paths.iter().cloned())
            .collect()
    }

    /// Versioned canonical module ownership for transport and action consumers.
    pub fn reference_manifest(&self) -> Result<reference_manifest::ReferenceManifest> {
        reference_manifest::build_reference_manifest(&self.compiled, &self.input.project_root)
    }
}

// ─────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────

/// Bundle a single route entry into its target format.
///
/// # Errors
///
/// Returns a [`BundleError`] if a hard boundary violation is detected, a
/// module cannot be resolved, a circular dependency is found, or a compile
/// error occurs.
pub fn bundle(input: BundleInput) -> Result<BundleOutput> {
    let context = BundleContext::new(&input.project_root);
    bundle_with_context(input, &context)
}

/// Bundle a single route using shared batch context.
pub fn bundle_with_context(input: BundleInput, context: &BundleContext) -> Result<BundleOutput> {
    bundle_with_shared_modules(input, context, &BTreeSet::new())
}

/// Bundle a route while reading selected modules from a previously imported
/// executable shared-route registry.
pub fn bundle_with_shared_modules(
    input: BundleInput,
    context: &BundleContext,
    shared_modules: &BTreeSet<PathBuf>,
) -> Result<BundleOutput> {
    let prepared = prepare_bundle_with_parts(
        input,
        context.compile_cache(),
        context.graph_cache(),
        context.incremental(),
        context.build_hooks(),
        context.artifacts(),
    )?;
    let output = bundle_prepared(&prepared, shared_modules)?;
    context.enforce_cache_budget();
    Ok(output)
}

/// Resolve and compile a route once so it can be inspected and emitted later.
pub fn prepare_bundle(input: BundleInput, context: &BundleContext) -> Result<PreparedBundle> {
    let prepared = prepare_bundle_with_parts(
        input,
        context.compile_cache(),
        context.graph_cache(),
        context.incremental(),
        context.build_hooks(),
        context.artifacts(),
    )?;
    context.enforce_cache_budget();
    Ok(prepared)
}

/// Emit a previously prepared route while reading selected modules from a
/// shared-route registry.
pub fn bundle_prepared(
    prepared: &PreparedBundle,
    shared_modules: &BTreeSet<PathBuf>,
) -> Result<BundleOutput> {
    emit_prepared_bundle(prepared, shared_modules)
}

/// Compile shared route modules into one executable browser registry.
///
/// The caller supplies paths already proven common to multiple routes. Their
/// static closure is linked dependency-first so a route bundle can safely read
/// the registry after importing this output.
pub fn bundle_shared_route_modules(
    project_root: PathBuf,
    app_dir: PathBuf,
    module_paths: &BTreeSet<PathBuf>,
    options: BundleOptions,
    context: &BundleContext,
) -> Result<SharedRouteBundleOutput> {
    let entry_label = "ruvyxa:shared-route-entry.ts".to_string();
    let entry_source = module_paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let path = path.to_string_lossy().replace('\\', "/");
            let path = path
                .strip_prefix("//?/")
                .or_else(|| path.strip_prefix("\\\\?\\"))
                .unwrap_or(&path);
            // Escaping only `"` leaves newlines and other control characters in
            // the specifier able to break the generated import statement.
            format!(
                "import * as __ruvyxa_shared_{index} from {};",
                output::js_string(path)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let input = BundleInput {
        entry: PathBuf::from(&entry_label),
        project_root,
        app_dir,
        layouts: Vec::new(),
        request_path: "/__ruvyxa/shared".to_string(),
        target: BundleTarget::Client,
        options,
        specials: RouteSpecials::default(),
    };
    let graph = resolver::resolve_graph_with_incremental(
        &entry_source,
        &entry_label,
        &input.project_root,
        &input.app_dir,
        context.graph_cache(),
        context.build_hooks(),
        input.target,
        input.options.jsx_runtime,
        Some(context.incremental()),
    )?;
    let (compiled, _) = compiler::compile_graph_with_hooks_and_maps(
        &graph,
        &input,
        context.compile_cache(),
        context.build_hooks(),
    )?;
    let mut diagnostics = Vec::new();
    boundary::check(&compiled, &input, &mut diagnostics)?;
    let shared_modules = compiled
        .into_iter()
        .filter(|module| !module.is_external && module.path != *entry_label)
        .collect::<Vec<_>>();
    emit_shared_route_modules(shared_modules, input)
}

/// Emit a shared-route registry directly from routes prepared in the same
/// immutable build snapshot.
///
/// This preserves the legacy synthetic-entry breadth-first module order while
/// avoiding a second resolve and compile pass for the selected closure.
pub fn bundle_shared_prepared_route_modules(
    prepared_routes: &[&PreparedBundle],
    module_paths: &BTreeSet<PathBuf>,
    options: BundleOptions,
) -> Result<SharedRouteBundleOutput> {
    let Some(first) = prepared_routes.first() else {
        return Err(BundleError::Compiler(
            "shared route preparation requires at least one prepared route".to_string(),
        ));
    };
    let available = prepared_routes
        .iter()
        .flat_map(|prepared| prepared.compiled.iter())
        .filter(|module| {
            !module.is_external && !module.path.to_string_lossy().starts_with("ruvyxa:")
        })
        .map(|module| (module.path.clone(), module))
        .collect::<BTreeMap<_, _>>();
    let mut queue = module_paths.iter().cloned().collect::<VecDeque<_>>();
    let mut visited = BTreeSet::new();
    let mut shared_modules = Vec::new();
    while let Some(path) = queue.pop_front() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let Some(module) = available.get(&path) else {
            return Err(BundleError::Compiler(format!(
                "prepared shared route module is unavailable: {}",
                path.display()
            )));
        };
        for dependency in module.deps.iter() {
            if available.contains_key(dependency) && !visited.contains(dependency) {
                queue.push_back(dependency.clone());
            }
        }
        shared_modules.push((*module).clone());
    }

    let mut input = first.input.clone();
    input.entry = PathBuf::from("ruvyxa:shared-route-entry.ts");
    input.layouts.clear();
    input.request_path = "/__ruvyxa/shared".to_string();
    input.target = BundleTarget::Client;
    input.options = options;
    let mut diagnostics = Vec::new();
    boundary::check(&shared_modules, &input, &mut diagnostics)?;
    emit_shared_route_modules(shared_modules, input)
}

fn emit_shared_route_modules(
    shared_modules: Vec<compiler::CompiledModule>,
    input: BundleInput,
) -> Result<SharedRouteBundleOutput> {
    let linked = linker::link_shared_route_modules(&shared_modules, &input)?;
    let code = if input.options.minify {
        minifier::minify_with_options(&linked, input.target, false)?
    } else {
        linked
    };
    Ok(SharedRouteBundleOutput {
        code,
        modules: shared_modules
            .into_iter()
            .map(|module| module.path)
            .collect(),
    })
}

fn prepare_bundle_with_parts(
    input: BundleInput,
    compile_cache: &CompileCache,
    graph_cache: &ResolveGraphCache,
    incremental: &incremental::IncrementalGraphCache,
    build_hooks: &BuildHookPipeline,
    artifacts: &ArtifactTaskGraph,
) -> Result<PreparedBundle> {
    let started = Instant::now();

    // 1. Build the virtual entry source that wires layouts → page.
    let (entry_source, entry_label) = output::build_entry_source(&input);

    // 2. Resolve the full dependency graph from the entry.
    let graph = resolver::resolve_graph_with_incremental(
        &entry_source,
        &entry_label,
        &input.project_root,
        &input.app_dir,
        graph_cache,
        build_hooks,
        input.target,
        input.options.jsx_runtime,
        Some(incremental),
    )?;
    let resolver_configuration_hash = graph_cache.configuration_hash(&input.project_root);

    let mut resolve_keys = Vec::with_capacity(graph.len());
    for module in &graph {
        let path = stable_path(&module.path);
        let source_key = artifacts.key(
            ArtifactKind::Source,
            [
                ("path", path.as_bytes()),
                ("source", module.source.as_bytes()),
                (
                    "loadedContent",
                    module
                        .compiled_content
                        .as_deref()
                        .unwrap_or_default()
                        .as_bytes(),
                ),
                (
                    "loadSourceMap",
                    module
                        .load_source_map
                        .as_deref()
                        .unwrap_or_default()
                        .as_bytes(),
                ),
            ],
        );
        let source_hash = content_hash_parts([
            module.source.as_bytes(),
            module
                .compiled_content
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
            module
                .load_source_map
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
        ]);
        publish_artifact(
            artifacts,
            source_key.clone(),
            BTreeSet::new(),
            source_hash.clone(),
        )?;

        let dependency_fingerprint = module
            .deps
            .iter()
            .map(|dependency| stable_path(dependency))
            .chain(
                module
                    .dependency_aliases
                    .iter()
                    .map(|(alias, path)| format!("{alias}={}", stable_path(path))),
            )
            .collect::<Vec<_>>()
            .join("\n");
        let resolve_key = artifacts.key(
            ArtifactKind::Resolve,
            [
                ("source", source_key.identity.as_bytes()),
                ("target", format!("{:?}", input.target).as_bytes()),
                (
                    "jsxRuntime",
                    format!("{:?}", input.options.jsx_runtime).as_bytes(),
                ),
                ("resolverConfig", resolver_configuration_hash.as_bytes()),
                ("resolvedDependencies", dependency_fingerprint.as_bytes()),
            ],
        );
        let resolve_hash = content_hash_parts([dependency_fingerprint.as_bytes()]);
        publish_artifact(
            artifacts,
            resolve_key.clone(),
            BTreeSet::from([ArtifactDependency::new(source_key, Some(source_hash))]),
            resolve_hash.clone(),
        )?;
        resolve_keys.push(resolve_key);
    }

    let watch_paths = graph
        .iter()
        .flat_map(|module| module.watch_paths.iter().cloned())
        .collect();
    let glob_matches = graph
        .iter()
        .flat_map(|module| module.glob_matches.iter().cloned())
        .collect();

    // 3. Compile each module (strip TS types, transform JSX).
    let (compiled, hook_source_maps) =
        compiler::compile_graph_with_hooks_and_maps(&graph, &input, compile_cache, build_hooks)?;

    let options_fingerprint = serde_json::to_vec(&input.options).map_err(|error| {
        BundleError::Compiler(format!("cannot fingerprint bundle options: {error}"))
    })?;
    let mut transform_dependencies = BTreeSet::new();
    for (module, resolve_key) in compiled.iter().zip(&resolve_keys) {
        let transform_key = artifacts.key(
            ArtifactKind::Transform,
            [
                ("resolve", resolve_key.identity.as_bytes()),
                ("options", options_fingerprint.as_slice()),
            ],
        );
        let output_hash = content_hash_parts([
            module.js.as_bytes(),
            hook_source_maps
                .get(&module.path)
                .map(String::as_bytes)
                .unwrap_or_default(),
        ]);
        publish_artifact(
            artifacts,
            transform_key.clone(),
            BTreeSet::from([ArtifactDependency::new(resolve_key.clone(), None)]),
            output_hash.clone(),
        )?;
        transform_dependencies.insert(ArtifactDependency::new(transform_key, Some(output_hash)));
    }

    // 4. Enforce server/client boundaries.
    let mut diagnostics = Vec::new();
    boundary::check(&compiled, &input, &mut diagnostics)?;
    let transform_closure = dependency_fingerprint(&transform_dependencies);
    let analyze_key = artifacts.key(
        ArtifactKind::Analyze,
        [
            ("entry", entry_label.as_bytes()),
            ("target", format!("{:?}", input.target).as_bytes()),
            ("dependencies", transform_closure.as_bytes()),
        ],
    );
    publish_artifact(
        artifacts,
        analyze_key.clone(),
        transform_dependencies.clone(),
        content_hash_parts([format!("{diagnostics:?}").as_bytes()]),
    )?;

    // 5. Plan client dynamic chunks before linking. The entry bundle follows only static edges so
    // dynamic modules are evaluated only when their generated ESM import runs.
    let split_dynamic_imports =
        input.target == BundleTarget::Client && input.options.emit_chunk_manifest;
    let dynamic_import_files = if split_dynamic_imports {
        plan_dynamic_chunk_files(&compiled, &PathBuf::from(&entry_label))
    } else {
        Default::default()
    };
    let static_modules = if split_dynamic_imports {
        static_entry_modules(
            &compiled,
            &PathBuf::from(&entry_label),
            &dynamic_import_files,
        )
    } else {
        compiled.clone()
    };
    let chunk_plan = dynamic_import_files
        .iter()
        .map(|(path, file)| format!("{}={file}", stable_path(path)))
        .chain(
            static_modules
                .iter()
                .map(|module| stable_path(&module.path)),
        )
        .collect::<Vec<_>>()
        .join("\n");
    let mut chunk_dependencies = transform_dependencies;
    chunk_dependencies.insert(ArtifactDependency::new(analyze_key, None));
    let chunk_closure = dependency_fingerprint(&chunk_dependencies);
    let chunk_plan_key = artifacts.key(
        ArtifactKind::ChunkPlan,
        [
            ("entry", entry_label.as_bytes()),
            ("route", input.request_path.as_bytes()),
            ("options", options_fingerprint.as_slice()),
            ("dependencies", chunk_closure.as_bytes()),
        ],
    );
    publish_artifact(
        artifacts,
        chunk_plan_key.clone(),
        chunk_dependencies,
        content_hash_parts([chunk_plan.as_bytes()]),
    )?;

    Ok(PreparedBundle {
        input,
        compiled,
        hook_source_maps,
        diagnostics,
        dynamic_import_files,
        static_modules,
        graph_module_count: graph.len(),
        prepare_duration: started.elapsed(),
        artifact_graph: artifacts.clone(),
        chunk_plan_key,
        watch_paths,
        glob_matches,
    })
}

fn emit_prepared_bundle(
    prepared: &PreparedBundle,
    shared_modules: &BTreeSet<PathBuf>,
) -> Result<BundleOutput> {
    let started = Instant::now();
    let input = &prepared.input;
    let compiled = &prepared.compiled;
    let hook_source_maps = &prepared.hook_source_maps;
    let dynamic_import_files = &prepared.dynamic_import_files;
    let static_modules = &prepared.static_modules;
    let reference_manifest = prepared.reference_manifest()?;
    let split_dynamic_imports =
        input.target == BundleTarget::Client && input.options.emit_chunk_manifest;
    let linked_modules = static_modules
        .iter()
        .filter(|module| !shared_modules.contains(&module.path))
        .cloned()
        .collect::<Vec<_>>();
    let chunks = if split_dynamic_imports {
        build_dynamic_output_chunks(compiled, input, dynamic_import_files)?
    } else {
        Vec::new()
    };

    // 6. Link modules into a single concatenated script. This also detects circular dependencies
    // and returns an error.
    let mut linked = linker::link_parallel_with_dynamic_imports_and_shared_modules(
        &linked_modules,
        input,
        dynamic_import_files,
        shared_modules,
    )?;
    if input.target == BundleTarget::Client {
        linked.push_str(&format!(
            "\n;(globalThis.__RUVYXA_ROUTE_ARTIFACTS__ ||= {{}})[{}] = {};\n",
            output::js_string(&input.request_path),
            output::js_string(&reference_manifest.artifact_version),
        ));
    }

    // 7. Optionally tree-shake, then minify. Tree-shaking is controlled
    // independently from whitespace/identifier minification.
    let optimized_linked = if input.options.tree_shaking {
        minifier::tree_shake_exports(&linked)
    } else {
        linked.clone()
    };
    let minify_output = input.options.minify;
    let final_code = if minify_output {
        minifier::minify_with_options(&optimized_linked, input.target, false)?
    } else {
        optimized_linked.clone()
    };

    // 8. Wrap in the appropriate output format.
    let code = output::wrap(final_code, input);

    // Count modules whose JS came from the compile cache, not freshly compiled.
    let cache_hits = compiled.iter().filter(|m| m.cache_hit).count();

    // 9. Generate source map if requested.
    let source_map = if input.options.source_map {
        let hash = blake3::hash(code.as_bytes()).to_hex();
        let map_file = format!("{}.js.map", &hash[..16]);
        let mut builder = sourcemap::SourceMapBuilder::new(&map_file, &input.project_root);

        let wrapper_lines = match input.target {
            BundleTarget::Client => 2,
            BundleTarget::Ssr | BundleTarget::Edge => 3,
        };

        let linker_header_lines: u32 = 3;
        let total_offset = wrapper_lines + linker_header_lines;

        let mut current_line = total_offset;
        for module in linker::ordered_project_modules(&linked_modules) {
            if module.is_external {
                continue;
            }
            let source_idx = builder.add_source(&module.path, Some(&module.js));
            let line_count = module.js.lines().count() as u32;
            let imported_hook_map = hook_source_maps
                .get(&module.path)
                .map(String::as_str)
                .is_some_and(|map| builder.add_source_map(map, current_line));
            if !imported_hook_map {
                builder.add_identity_mappings(source_idx, &module.js, current_line);
            }
            current_line += line_count + 5;
        }

        Some(builder.to_json())
    } else {
        None
    };

    // 10. Optionally emit a chunk manifest.
    let chunk_manifest =
        if input.options.emit_chunk_manifest || input.options.collect_module_manifest {
            let hash = blake3::hash(code.as_bytes()).to_hex();
            let bundle_id = hash[..16].to_string();
            let output_file = format!("{bundle_id}.js");
            let sm_file = source_map.as_ref().map(|_| format!("{bundle_id}.js.map"));

            let modules: Vec<String> = linker::ordered_project_modules(static_modules)
                .iter()
                .filter(|m| !m.is_external)
                .map(|m| m.path.display().to_string().replace('\\', "/"))
                .collect();

            let dynamic_imports = dynamic_import_chunks(compiled, dynamic_import_files);
            let glob_matches = prepared
                .glob_matches
                .iter()
                .map(|path| path.display().to_string().replace('\\', "/"))
                .collect();
            Some(ChunkManifest {
                bundle_id,
                route: input.request_path.clone(),
                modules,
                output_file,
                source_map_file: sm_file,
                size_bytes: code.len(),
                dynamic_imports,
                glob_matches,
                reference_manifest: Some(reference_manifest.clone()),
            })
        } else {
            None
        };

    // Count modules removed by tree-shaking.
    let tree_shaken_modules = if input.options.tree_shaking {
        // Approximate by counting `[tree-shaken]` comments before minification
        // strips comments.
        optimized_linked
            .lines()
            .filter(|l| l.contains("[tree-shaken]"))
            .count()
    } else {
        0
    };

    let output_bytes = code.len();
    let stats = BundleStats {
        module_count: prepared.graph_module_count,
        output_bytes,
        estimated_gz_bytes: (output_bytes as f64 * 0.35) as usize,
        minified: minify_output,
        tree_shaken: input.options.tree_shaking,
        duration_ms: (prepared.prepare_duration + started.elapsed()).as_millis() as u64,
        tree_shaken_modules,
        cache_hits,
    };

    let output = BundleOutput {
        code,
        source_map,
        diagnostics: prepared.diagnostics.clone(),
        stats,
        chunk_manifest,
        chunks,
    };
    publish_emitted_artifacts(prepared, shared_modules, &output)?;
    Ok(output)
}

fn publish_emitted_artifacts(
    prepared: &PreparedBundle,
    shared_modules: &BTreeSet<PathBuf>,
    output: &BundleOutput,
) -> Result<()> {
    let shared = shared_modules
        .iter()
        .map(|path| stable_path(path))
        .collect::<Vec<_>>()
        .join("\n");
    let emit_key = prepared.artifact_graph.key(
        ArtifactKind::Emit,
        [
            ("plan", prepared.chunk_plan_key.identity.as_bytes()),
            ("sharedModules", shared.as_bytes()),
        ],
    );
    let chunks = serde_json::to_vec(&output.chunks).map_err(|error| {
        BundleError::Compiler(format!("cannot fingerprint output chunks: {error}"))
    })?;
    let emit_hash = content_hash_parts([output.code.as_bytes(), chunks.as_slice()]);
    publish_artifact(
        &prepared.artifact_graph,
        emit_key.clone(),
        BTreeSet::from([ArtifactDependency::new(
            prepared.chunk_plan_key.clone(),
            None,
        )]),
        emit_hash.clone(),
    )?;

    if let Some(source_map) = &output.source_map {
        let key = prepared.artifact_graph.key(
            ArtifactKind::SourceMap,
            [("emit", emit_key.identity.as_bytes())],
        );
        publish_artifact(
            &prepared.artifact_graph,
            key,
            BTreeSet::from([ArtifactDependency::new(
                emit_key.clone(),
                Some(emit_hash.clone()),
            )]),
            content_hash_parts([source_map.as_bytes()]),
        )?;
    }
    if let Some(manifest) = &output.chunk_manifest {
        let manifest = serde_json::to_vec(manifest).map_err(|error| {
            BundleError::Compiler(format!("cannot fingerprint chunk manifest: {error}"))
        })?;
        let key = prepared.artifact_graph.key(
            ArtifactKind::Manifest,
            [("emit", emit_key.identity.as_bytes())],
        );
        publish_artifact(
            &prepared.artifact_graph,
            key,
            BTreeSet::from([ArtifactDependency::new(emit_key, Some(emit_hash))]),
            content_hash_parts([manifest.as_slice()]),
        )?;
    }
    Ok(())
}

fn publish_artifact(
    graph: &ArtifactTaskGraph,
    key: ArtifactKey,
    dependencies: BTreeSet<ArtifactDependency>,
    content_hash: String,
) -> Result<bool> {
    let kind = key.kind;
    graph
        .publish(key, dependencies, content_hash)
        .map_err(|error| {
            BundleError::Compiler(format!("artifact graph rejected {kind:?} output: {error}"))
        })
}

fn content_hash_parts<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize().to_hex().to_string()
}

fn dependency_fingerprint(dependencies: &BTreeSet<ArtifactDependency>) -> String {
    let mut hasher = blake3::Hasher::new();
    for dependency in dependencies {
        hash_artifact_part(&mut hasher, dependency.key.kind.as_str().as_bytes());
        hash_artifact_part(&mut hasher, dependency.key.identity.as_bytes());
        hash_artifact_part(
            &mut hasher,
            dependency
                .content_hash
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
        );
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_artifact_part(hasher: &mut blake3::Hasher, part: &[u8]) {
    hasher.update(&(part.len() as u64).to_le_bytes());
    hasher.update(part);
}

fn stable_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn client_input(
        root: &std::path::Path,
        app_dir: &std::path::Path,
        entry: PathBuf,
        layouts: Vec<PathBuf>,
        request_path: &str,
    ) -> BundleInput {
        BundleInput {
            entry,
            project_root: root.to_path_buf(),
            app_dir: app_dir.to_path_buf(),
            layouts,
            request_path: request_path.to_string(),
            target: BundleTarget::Client,
            options: BundleOptions {
                minify: false,
                source_map: true,
                tree_shaking: true,
                emit_chunk_manifest: true,
                ..Default::default()
            },
            specials: RouteSpecials::default(),
        }
    }

    #[test]
    fn bundles_css_module_imports_as_deterministic_class_maps() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import styles from './card.module.css'; export default function Page() { return <main className={styles.card}>ok</main>; }",
        )
        .unwrap();
        fs::write(app.join("card.module.css"), ".card { color: navy; }").unwrap();

        let output = bundle(client_input(&root, &app, page, Vec::new(), "/")).unwrap();
        let scoped = crate::style_module::scope_css_module(
            ".card { color: navy; }",
            &app.join("card.module.css"),
            &root,
        );

        assert!(output.code.contains(&scoped.classes["card"]));
        assert!(output.code.contains("const styles"));
    }

    /// The client half of the JSON module-kind contract. `compiler.mjs` covers
    /// the server and serverless graphs; this covers the Rust graph, which
    /// resolves `.json` by exact path just as readily.
    #[test]
    fn bundles_a_json_import_as_data_rather_than_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import config from './config.json'; export default function Page() { return <main>{config.title}</main>; }",
        )
        .unwrap();
        // A value that would be a syntax error if the file were parsed as
        // JavaScript, and a string that would be read as an import if the
        // document were scanned for specifiers.
        fs::write(
            app.join("config.json"),
            r#"{ "title": "Ruvyxa", "note": "require('./missing.json')" }"#,
        )
        .unwrap();

        let output = bundle(client_input(&root, &app, page, Vec::new(), "/")).unwrap();

        assert!(output.code.contains("JSON.parse("), "{}", output.code);
        assert!(output.code.contains("Ruvyxa"));
    }

    #[test]
    fn a_malformed_json_import_names_the_json_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import config from './config.json'; export default function Page() { return <main>{config.title}</main>; }",
        )
        .unwrap();
        fs::write(app.join("config.json"), "{ \"title\": }").unwrap();

        let error = bundle(client_input(&root, &app, page, Vec::new(), "/"))
            .expect_err("malformed JSON must not bundle");
        let message = error.to_string();
        assert!(message.contains("RUV1805"), "{message}");
        assert!(message.contains("config.json"), "{message}");
    }

    #[test]
    fn bundle_context_reuses_graph_cache_across_routes() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let shared = app.join("shared.ts");
        let layout = app.join("layout.tsx");
        let page_a = app.join("page-a.tsx");
        let page_b = app.join("page-b.tsx");

        fs::write(&shared, "export const label = \"Ruvyxa\";").unwrap();
        fs::write(
            &layout,
            "import { label } from \"./shared\";\nexport default function Layout({ children }) { return <section data-label={label}>{children}</section>; }",
        )
        .unwrap();
        fs::write(
            &page_a,
            "import { label } from \"./shared\";\nexport default function PageA() { return <main>{label}</main>; }",
        )
        .unwrap();
        fs::write(
            &page_b,
            "import { label } from \"./shared\";\nexport default function PageB() { return <main>{label}</main>; }",
        )
        .unwrap();

        let context = BundleContext::new(&root);

        let first = bundle_with_context(
            client_input(&root, &app, page_a, vec![layout.clone()], "/a"),
            &context,
        )
        .unwrap();
        let second = bundle_with_context(
            client_input(&root, &app, page_b, vec![layout], "/b"),
            &context,
        )
        .unwrap();

        assert!(first.code.contains("PageA"));
        assert!(second.code.contains("PageB"));
        assert!(first.source_map.is_some());
        assert!(second.source_map.is_some());
        assert_eq!(context.graph_cache().source_count(), 4);
        assert!(context.graph_cache().resolution_count() >= 1);
    }

    #[test]
    fn bundle_context_reuses_persisted_dependency_edges_across_builds() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        let shared = app.join("shared.ts");
        fs::write(&shared, "export const label = 'cached';").unwrap();
        fs::write(
            &page,
            "import { label } from './shared'; export default function Page() { return <main>{label}</main>; }",
        )
        .unwrap();

        let first = BundleContext::new(&root);
        bundle_with_context(
            client_input(&root, &app, page.clone(), Vec::new(), "/"),
            &first,
        )
        .unwrap();
        first.save_incremental().unwrap();

        let second = BundleContext::new(&root);
        bundle_with_context(client_input(&root, &app, page, Vec::new(), "/"), &second).unwrap();

        assert!(second.incremental().edge_hits() >= 2);
        assert!(second.artifacts().stats().hits >= 1);
        assert!(second.artifacts().stats().ready >= 1);
    }

    #[test]
    fn leaf_edit_reuses_unaffected_artifacts_and_preserves_clean_output() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let shared = app.join("shared.ts");
        let first_page = app.join("first.tsx");
        let second_page = app.join("second.tsx");
        fs::write(&shared, "export const label = 'stable';").unwrap();
        fs::write(
            &first_page,
            "import { label } from './shared'; export default function First() { return <main>{label}</main>; }",
        )
        .unwrap();
        fs::write(
            &second_page,
            "import { label } from './shared'; export default function Second() { return <main>{label}</main>; }",
        )
        .unwrap();

        let cold_context = BundleContext::new(&root);
        let cold_first = bundle_with_context(
            client_input(&root, &app, first_page.clone(), Vec::new(), "/first"),
            &cold_context,
        )
        .unwrap();
        let cold_second = bundle_with_context(
            client_input(&root, &app, second_page.clone(), Vec::new(), "/second"),
            &cold_context,
        )
        .unwrap();
        cold_context.save_incremental().unwrap();

        fs::write(
            &first_page,
            "import { label } from './shared'; export default function First() { return <main>{label}-edited</main>; }",
        )
        .unwrap();
        let edit_context = BundleContext::new(&root);
        let edited_first = bundle_with_context(
            client_input(&root, &app, first_page, Vec::new(), "/first"),
            &edit_context,
        )
        .unwrap();
        let warm_second = bundle_with_context(
            client_input(&root, &app, second_page, Vec::new(), "/second"),
            &edit_context,
        )
        .unwrap();
        let stats = edit_context.artifacts().stats();

        assert_ne!(edited_first.code, cold_first.code);
        assert_eq!(warm_second.code, cold_second.code);
        assert!(
            stats.hits > 0,
            "the unchanged dependency lane must be reused"
        );
        assert!(
            stats.misses > 0,
            "the edited dependency lane must be rebuilt"
        );
    }

    #[test]
    fn small_and_large_cache_budgets_emit_identical_output_and_pin_active_tasks() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        fs::write(
            &page,
            "export default function Page() { return <main>budget</main>; }",
        )
        .unwrap();
        let input = client_input(&root, &app, page, Vec::new(), "/");

        let large = BundleContext::new(&root).with_test_cache_budget(
            cache_budget::CacheBudget::new(1024 * 1024, 0.8, 0.65).unwrap(),
        );
        let expected = bundle_with_context(input.clone(), &large).unwrap();

        let small = BundleContext::new(&root)
            .with_test_cache_budget(cache_budget::CacheBudget::new(1, 0.8, 0.65).unwrap());
        let active_key = small
            .artifacts()
            .key(ArtifactKind::Emit, [("fixture", b"pinned".as_slice())]);
        let active = small
            .artifacts()
            .begin(active_key.clone(), BTreeSet::new())
            .unwrap();
        let actual = bundle_with_context(input, &small).unwrap();
        let budget = small.cache_budget_snapshot();

        assert_eq!(actual.code, expected.code);
        assert_eq!(actual.source_map, expected.source_map);
        assert_eq!(
            small.artifacts().record(&active_key).unwrap().state,
            artifact_graph::ArtifactState::Building,
            "budget enforcement must not evict an in-flight artifact"
        );
        assert!(
            budget.evictions.values().sum::<u64>() > 0,
            "the tiny budget must exercise an eviction path"
        );
        assert!(
            budget.resident_bytes <= budget.target_bytes,
            "evictable cache memory must return below the hysteresis target"
        );
        drop(active);
    }

    /// A long edit session stays inside the cache budget and keeps producing
    /// byte-identical output.
    ///
    /// The invariant asserted per edit is the **hard limit**, not the hysteresis
    /// target. Those are different promises, and asserting the wrong one made
    /// this test environment-dependent: [`CacheBudget::observe`] deliberately
    /// leaves residency alone in the `[target, soft)` band when no pressure has
    /// been entered yet, because that band is what stops a build from evicting
    /// on every trivial fluctuation. Residency landing in that band is correct
    /// behaviour, but the old assertion read it as a failure, so the test passed
    /// or failed on whether artifact-graph metadata — roughly 135 bytes per
    /// record and per dependency edge — happened to land under 2662 bytes on a
    /// given machine. It had a 173-byte margin locally and none in CI.
    ///
    /// What the controller does guarantee, and what this now checks, is that
    /// residency never exceeds the hard limit, that sustained editing genuinely
    /// drives it into pressure, and that eviction under pressure never changes
    /// the emitted bytes.
    #[test]
    fn repeated_route_edits_stay_within_the_shared_cache_budget() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        let input = client_input(&root, &app, page.clone(), Vec::new(), "/");
        let context = BundleContext::new(&root)
            .with_test_cache_budget(cache_budget::CacheBudget::new(4 * 1024, 0.8, 0.65).unwrap());
        let mut last = None;

        for edit in 0..24 {
            let marker = "x".repeat(edit + 1);
            fs::write(
                &page,
                format!(
                    "export default function Page() {{ return <main>edit-{edit}-{marker}</main>; }}"
                ),
            )
            .unwrap();
            let output = bundle_with_context(input.clone(), &context).unwrap();
            let budget = context.cache_budget_snapshot();
            assert!(
                budget.resident_bytes <= budget.hard_limit_bytes,
                "edit {edit} left {} resident bytes above the hard limit {}",
                budget.resident_bytes,
                budget.hard_limit_bytes
            );
            last = Some(output);
        }

        // Memory pressure must never change output semantics: the worst legal
        // result of eviction is a slower rebuild.
        let clean = bundle_with_context(input, &BundleContext::new(&root)).unwrap();
        assert_eq!(last.unwrap().code, clean.code);

        let budget = context.cache_budget_snapshot();
        assert!(
            budget.evictions.values().sum::<u64>() > 0,
            "24 edits under a 4 KiB budget must exercise eviction"
        );
        // Residency is only ever sampled after enforcement has already run, so
        // the in-build peak is not observable from here. `pressure_events` is
        // the counter that records the controller actually entering pressure,
        // and it is the honest way to prove the budget was under test rather
        // than merely never approached.
        assert!(
            budget.pressure_events > 0,
            "the fixture never entered cache pressure, so it proves nothing about eviction"
        );
    }

    /// A reused dependency entry must reproduce the cold build exactly,
    /// including how its specifiers resolved.
    ///
    /// The linker consults a module's alias map before matching by path suffix,
    /// and `~/lib/label` shares no suffix with `<root>/lib/label.ts`. Reusing
    /// cached edges while dropping the alias map therefore gave the linker a
    /// resolution input the cold build never produced. The alias lives in a
    /// *dependency* here on purpose: entry modules are always resolved fresh, so
    /// only a dependency exercises the reuse path.
    #[test]
    fn reused_dependency_entries_resolve_aliases_like_a_cold_build() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        let lib = root.join("lib");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&lib).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"~/*":["./*"]}}}"#,
        )
        .unwrap();
        fs::write(lib.join("label.ts"), "export const label = 'aliased';").unwrap();
        let shared = app.join("shared.ts");
        fs::write(
            &shared,
            "import { label } from '~/lib/label';\nexport const shared = label;",
        )
        .unwrap();
        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import { shared } from './shared';\nexport default function Page() { return <main>{shared}</main>; }",
        )
        .unwrap();

        let cold_context = BundleContext::new(&root);
        let cold = bundle_with_context(
            client_input(&root, &app, page.clone(), Vec::new(), "/"),
            &cold_context,
        )
        .unwrap();
        cold_context.save_incremental().unwrap();
        assert!(
            cold.code.contains("aliased"),
            "the cold build must link the aliased module"
        );

        let warm_context = BundleContext::new(&root);
        let warm = bundle_with_context(
            client_input(&root, &app, page, Vec::new(), "/"),
            &warm_context,
        )
        .unwrap();

        assert!(
            warm_context.incremental().edge_hits() >= 1,
            "the warm build must actually reuse a persisted entry, or this \
             test would pass without exercising the path it guards"
        );
        assert_eq!(
            warm.code, cold.code,
            "a warm build must produce the cold build's output"
        );
    }

    /// An entry persisted before aliases were recorded cannot describe how its
    /// specifiers resolved. It must be resolved fresh rather than reused as
    /// "no aliases", and the fresh resolve must rewrite it complete so the cost
    /// is paid once instead of on every later build.
    #[test]
    fn an_entry_missing_aliases_is_resolved_fresh_and_then_repaired() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        let lib = root.join("lib");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&lib).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"~/*":["./*"]}}}"#,
        )
        .unwrap();
        fs::write(lib.join("label.ts"), "export const label = 'aliased';").unwrap();
        let shared = app.join("shared.ts");
        fs::write(
            &shared,
            "import { label } from '~/lib/label';\nexport const shared = label;",
        )
        .unwrap();
        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import { shared } from './shared';\nexport default function Page() { return <main>{shared}</main>; }",
        )
        .unwrap();

        let cold_context = BundleContext::new(&root);
        let cold = bundle_with_context(
            client_input(&root, &app, page.clone(), Vec::new(), "/"),
            &cold_context,
        )
        .unwrap();
        cold_context.save_incremental().unwrap();

        // Rewrite the persisted manifest as an older build would have left it:
        // real edges, no `aliases` key anywhere.
        let manifest_path = root
            .join(".ruvyxa")
            .join("cache")
            .join("graph")
            .join("graph-manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let modules = manifest["modules"].as_object_mut().unwrap();
        assert!(!modules.is_empty(), "the cold build must persist entries");
        for (_, entry) in modules.iter_mut() {
            entry.as_object_mut().unwrap().remove("aliases");
        }
        fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap()).unwrap();

        let legacy_context = BundleContext::new(&root);
        let repaired = bundle_with_context(
            client_input(&root, &app, page, Vec::new(), "/"),
            &legacy_context,
        )
        .unwrap();

        assert_eq!(
            legacy_context.incremental().edge_hits(),
            0,
            "an entry without recorded aliases must not be reused"
        );
        assert_eq!(
            repaired.code, cold.code,
            "resolving fresh must reproduce the cold build"
        );

        // The repair is persisted, so the next build reuses it normally.
        legacy_context.save_incremental().unwrap();
        let repaired_manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let shared_entry = &repaired_manifest["modules"][shared.to_string_lossy().as_ref()];
        assert!(
            shared_entry["aliases"].is_object(),
            "the fresh resolve must rewrite the entry complete: {shared_entry}"
        );
    }

    #[test]
    fn bundle_emits_chunk_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "export default function Page() { return <main>Hi</main>; }",
        )
        .unwrap();

        let input = client_input(&root, &app, page, vec![], "/");
        let out = bundle(input).unwrap();

        assert!(out.chunk_manifest.is_some());
        let manifest = out.chunk_manifest.unwrap();
        assert!(!manifest.bundle_id.is_empty());
        assert_eq!(manifest.route, "/");
        assert!(manifest.size_bytes > 0);
    }

    #[test]
    fn bundles_markdown_page_through_ruvyxa_bundler_pipeline() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.md");
        fs::write(
            &page,
            "---\ntitle: Native content\n---\n# Fast docs\n\nBuilt with **Ruvyxa**.",
        )
        .unwrap();

        let output = bundle(client_input(&root, &app, page, vec![], "/")).unwrap();
        assert!(output.code.contains("Native content"));
        assert!(output.code.contains("ruvyxa-content"));
        assert!(output.code.contains("Fast docs"));
    }

    #[test]
    fn bundles_mdx_multiline_imports_and_gfm_through_ruvyxa_bundler_pipeline() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.mdx");
        fs::write(
            &page,
            "import {\n  Card\n} from './Card'\n\n# Rich docs\n\n| Feature | Ready |\n| :-- | --: |\n| MDX | yes |\n\n<Card>Bundled</Card>",
        )
        .unwrap();
        fs::write(
            app.join("Card.tsx"),
            "export function Card({ children }) { return <section data-card>{children}</section>; }",
        )
        .unwrap();

        let output = bundle(client_input(&root, &app, page, vec![], "/docs")).unwrap();

        assert!(output.code.contains("data-card"));
        assert!(output.code.contains("Rich docs"));
        assert!(output.code.contains("textAlign"));
        assert!(output.code.contains("Bundled"));
    }

    #[test]
    fn bundle_manifest_records_dynamic_import_split_points() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.tsx");
        let lazy = app.join("lazy.ts");
        fs::write(
            &page,
            "export default async function Page() { const mod = await import(\"./lazy\"); return <main>{mod.label}</main>; }",
        )
        .unwrap();
        fs::write(&lazy, "export const label = \"Lazy\";").unwrap();

        let input = client_input(&root, &app, page, vec![], "/");
        let out = bundle(input).unwrap();
        let manifest = out.chunk_manifest.unwrap();

        assert_eq!(manifest.dynamic_imports.len(), 1);
        assert!(manifest.dynamic_imports[0].module.ends_with("lazy.ts"));
        assert!(manifest.dynamic_imports[0].file.starts_with("chunk."));
        assert_eq!(out.chunks.len(), 1);
        assert_eq!(manifest.dynamic_imports[0].file, out.chunks[0].file_name);
        assert!(out.chunks[0].code.contains("export default"));
        assert!(out.code.contains(&format!(
            "import(\"./{}\").then((module) => module.default)",
            out.chunks[0].file_name
        )));
        assert!(!out.code.contains("const label = \"Lazy\";"));
        assert!(out.chunks[0].code.contains("const label = \"Lazy\";"));
    }

    #[test]
    fn import_meta_glob_expands_lazy_eager_and_alias_patterns_deterministically() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/glob-contract.json"))
                .unwrap();
        assert_eq!(contract["contract"], "ruvyxa.glob");
        assert_eq!(contract["schemaVersion"], 2);
        let dir = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(dir.path());
        let app = root.join("app");
        std::fs::create_dir_all(app.join("posts/nested")).unwrap();
        std::fs::write(app.join("posts/one.ts"), "export const value = 'one';").unwrap();
        std::fs::write(
            app.join("posts/nested/two.ts"),
            "export const value = 'two';",
        )
        .unwrap();
        std::fs::create_dir_all(app.join("posts/node_modules")).unwrap();
        std::fs::write(
            app.join("posts/node_modules/hidden.ts"),
            "throw new Error('must stay ignored')",
        )
        .unwrap();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@/*":["./app/*"]}}}"#,
        )
        .unwrap();
        let page = app.join("page.ts");
        std::fs::write(
            &page,
            r#"
export const lazy = import.meta.glob('./posts/**/*.ts');
export const eager = import.meta.glob('./posts/*.ts', { eager: true });
export const aliased = import.meta.glob('@/posts/*.ts');
export default function Page() { return null }
"#,
        )
        .unwrap();

        let input = client_input(&root, &app, page, vec![], "/");
        let output = bundle(input).unwrap();
        let manifest = output.chunk_manifest.unwrap();
        assert_eq!(manifest.glob_matches.len(), 2, "{manifest:?}");
        // The aliased lazy edge resolves to the same module that the eager
        // glob already pulled into the static graph, so only the nested module
        // remains a split point.
        assert_eq!(manifest.dynamic_imports.len(), 1);
        assert!(output.code.contains("./posts/one.ts"));
        assert!(output.code.contains("one"));
    }

    #[test]
    fn prepared_bundle_emits_the_same_route_output_as_direct_bundling() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.mdx");
        let card = app.join("Card.tsx");
        fs::write(
            &page,
            "import { Card } from './Card'\n\n# Prepared MDX\n\n<Card>Ready</Card>",
        )
        .unwrap();
        fs::write(
            &card,
            "export function Card({ children }) { return <aside>{children}</aside>; }",
        )
        .unwrap();

        let input = client_input(&root, &app, page, vec![], "/prepared");
        let direct_context = BundleContext::new(&root);
        let direct = bundle_with_context(input.clone(), &direct_context).unwrap();
        let prepared_context = BundleContext::new(&root);
        let prepared = prepare_bundle(input, &prepared_context).unwrap();
        let emitted = bundle_prepared(&prepared, &BTreeSet::new()).unwrap();

        assert!(prepared.module_paths().contains(&card));
        assert_eq!(emitted.code, direct.code);
        assert_eq!(emitted.source_map, direct.source_map);
        assert_eq!(emitted.diagnostics, direct.diagnostics);
        assert_eq!(
            serde_json::to_value(&emitted.chunk_manifest).unwrap(),
            serde_json::to_value(&direct.chunk_manifest).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&emitted.chunks).unwrap(),
            serde_json::to_value(&direct.chunks).unwrap()
        );
        assert_eq!(emitted.stats.module_count, direct.stats.module_count);
        assert_eq!(emitted.stats.output_bytes, direct.stats.output_bytes);
        assert_eq!(
            emitted.stats.tree_shaken_modules,
            direct.stats.tree_shaken_modules
        );
    }

    #[test]
    fn prepared_shared_registry_matches_the_legacy_synthetic_entry() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let shared = app.join("shared.ts");
        let page_a = app.join("a.tsx");
        let page_b = app.join("b.tsx");
        fs::write(&shared, "export const label = 'shared';").unwrap();
        fs::write(
            &page_a,
            "import { label } from './shared'; export default function A() { return <main>{label}</main> }",
        )
        .unwrap();
        fs::write(
            &page_b,
            "import { label } from './shared'; export default function B() { return <aside>{label}</aside> }",
        )
        .unwrap();
        let context = BundleContext::new(&root);
        let prepared_a =
            prepare_bundle(client_input(&root, &app, page_a, vec![], "/a"), &context).unwrap();
        let prepared_b =
            prepare_bundle(client_input(&root, &app, page_b, vec![], "/b"), &context).unwrap();
        let shared_paths = prepared_a
            .module_paths()
            .intersection(&prepared_b.module_paths())
            .filter(|path| path.is_file())
            .cloned()
            .collect::<BTreeSet<_>>();
        let options = BundleOptions {
            minify: false,
            source_map: false,
            tree_shaking: false,
            jsx_runtime: JsxRuntime::Automatic,
            es_target: EsTarget::Es2022,
            split_strategy: SplitStrategy::Route,
            emit_chunk_manifest: false,
            collect_module_manifest: false,
        };

        let legacy = bundle_shared_route_modules(
            root.clone(),
            app,
            &shared_paths,
            options.clone(),
            &context,
        )
        .unwrap();
        let prepared = bundle_shared_prepared_route_modules(
            &[&prepared_a, &prepared_b],
            &shared_paths,
            options,
        )
        .unwrap();

        assert_eq!(prepared.modules, legacy.modules);
        assert_eq!(prepared.code, legacy.code);
    }

    #[test]
    fn keeps_overlapping_dynamic_closures_in_the_entry_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import { singleton } from './shared'; export default async function Page() { return <main>{singleton + (await import('./lazy')).label}</main>; }",
        )
        .unwrap();
        fs::write(
            app.join("shared.ts"),
            "export const singleton = globalThis.__ruvyxa_shared = (globalThis.__ruvyxa_shared || 0) + 1;",
        )
        .unwrap();
        fs::write(
            app.join("lazy.ts"),
            "import { singleton } from './shared'; export const label = singleton;",
        )
        .unwrap();

        let out = bundle(client_input(&root, &app, page, vec![], "/")).unwrap();
        assert!(out.chunks.is_empty());
        assert_eq!(
            out.code.matches("__ruvyxa_shared").count(),
            2,
            "{}",
            out.code
        );
    }

    #[test]
    fn bundle_skips_dynamic_chunks_without_manifest_output() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "export default async function Page() { return (await import(\"./lazy\")).label; }",
        )
        .unwrap();
        fs::write(app.join("lazy.ts"), "export const label = \"Lazy\";").unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.emit_chunk_manifest = false;
        let output = bundle(input).unwrap();

        assert!(output.chunk_manifest.is_none());
        assert!(output.chunks.is_empty());
        assert!(output.code.contains("Promise.resolve("));
    }

    #[test]
    fn bundle_stats_includes_estimated_gz() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "export default function Page() { return <main>Stats</main>; }",
        )
        .unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.source_map = false;
        input.options.emit_chunk_manifest = false;
        let out = bundle(input).unwrap();

        assert!(out.stats.estimated_gz_bytes > 0);
        assert!(out.stats.estimated_gz_bytes < out.stats.output_bytes);
    }

    #[test]
    fn automatic_jsx_runtime_injects_import() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "export default function Page() { return <main>Automatic</main>; }",
        )
        .unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.jsx_runtime = JsxRuntime::Automatic;
        input.options.source_map = false;
        input.options.emit_chunk_manifest = false;
        let out = bundle(input).unwrap();

        // The compiled output should reference _jsx from react/jsx-runtime.
        assert!(
            out.code.contains("_jsx") || out.code.contains("jsx-runtime"),
            "expected automatic JSX runtime in output, got: {}",
            &out.code[..out.code.len().min(500)]
        );
    }

    #[test]
    fn client_bundle_inlines_automatic_jsx_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        let react = root.join("node_modules/react");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&react).unwrap();

        fs::write(
            react.join("package.json"),
            r#"{"exports":{".":"./index.js","./jsx-runtime":"./jsx-runtime.js"}}"#,
        )
        .unwrap();
        fs::write(react.join("index.js"), "module.exports = {};").unwrap();
        fs::write(
            react.join("jsx-runtime.js"),
            "module.exports = { jsx() { return 'jsxRuntimeMarker'; }, jsxs() {} };",
        )
        .unwrap();

        // No explicit `react` import: the automatic transform injects the
        // `react/jsx-runtime` import after the graph walk, so the resolver has
        // to seed that edge itself or the browser receives a bare specifier.
        let page = app.join("page.tsx");
        fs::write(
            &page,
            "export default function Page() { return <main>Ready</main>; }",
        )
        .unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.jsx_runtime = JsxRuntime::Automatic;
        input.options.source_map = false;
        input.options.emit_chunk_manifest = false;
        let out = bundle(input).unwrap();

        assert!(
            !out.code.contains("\"react/jsx-runtime\"")
                && !out.code.contains("'react/jsx-runtime'"),
            "jsx runtime leaked as a bare specifier: {}",
            &out.code[..out.code.len().min(500)]
        );
        assert!(out.code.contains("jsxRuntimeMarker"));
    }

    #[test]
    fn client_bundle_includes_commonjs_react_dependencies() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        let react = root.join("node_modules/react");
        let react_dom = root.join("node_modules/react-dom");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(react.join("cjs")).unwrap();
        fs::create_dir_all(react_dom.join("cjs")).unwrap();

        fs::write(
            react.join("package.json"),
            r#"{"exports":{".":"./index.js"}}"#,
        )
        .unwrap();
        fs::write(
            react.join("index.js"),
            "if (process.env.NODE_ENV === 'production') { module.exports = require('./cjs/react.production.js'); } else { module.exports = require('./cjs/react.development.js'); }",
        )
        .unwrap();
        fs::write(
            react.join("cjs/react.production.js"),
            "const stack = /\\n( *(at)?)/; module.exports = { createElement() {}, useState() {}, stack };",
        )
        .unwrap();
        fs::write(
            react.join("cjs/react.development.js"),
            "module.exports = { developmentOnlyReactRuntime: true };",
        )
        .unwrap();
        fs::write(
            react_dom.join("package.json"),
            r#"{"exports":{"./client":"./client.js"}}"#,
        )
        .unwrap();
        fs::write(
            react_dom.join("client.js"),
            "if (process.env.NODE_ENV === 'production') { module.exports = require('./cjs/react-dom-client.production.js'); } else { module.exports = require('./cjs/react-dom-client.development.js'); }",
        )
        .unwrap();
        fs::write(
            react_dom.join("cjs/react-dom-client.production.js"),
            "module.exports = { hydrateRoot() {} };",
        )
        .unwrap();
        fs::write(
            react_dom.join("cjs/react-dom-client.development.js"),
            "module.exports = { developmentOnlyReactDomRuntime: true };",
        )
        .unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import { useState } from 'react'; export default function Page() { useState(); return <main>Ready</main>; }",
        )
        .unwrap();

        let mut readable_input = client_input(&root, &app, page.clone(), vec![], "/");
        readable_input.options.source_map = false;
        readable_input.options.emit_chunk_manifest = false;
        let readable_output = bundle(readable_input).unwrap();

        let mut minified_input = client_input(&root, &app, page, vec![], "/");
        minified_input.options.minify = true;
        minified_input.options.source_map = false;
        minified_input.options.emit_chunk_manifest = false;
        let output = bundle(minified_input).unwrap();

        assert!(!output.code.contains("from \"react\""));
        assert!(!output.code.contains("from \"react-dom/client\""));
        assert!(output.code.contains("/\\n( *(at)?)/"));
        assert!(!output.code.contains("developmentOnlyReactRuntime"));
        assert!(!output.code.contains("developmentOnlyReactDomRuntime"));
        assert!(!output.code.contains("node_modules/react/index.js"));
        assert!(output.code.len() < readable_output.code.len());
        assert!(output.stats.minified);
    }

    /// End-to-end guard for `RUV1610: Cannot require "scheduler"`.
    ///
    /// The package is reachable only through a nested `node_modules` (the
    /// non-hoisted layout npm produces on a version conflict and pnpm produces
    /// for every transitive dependency) and ships no `exports` map. Both traits
    /// used to make it unresolvable, and the client linker then replaced the
    /// `require` with a throw that only fired once the browser ran the chunk.
    /// `import React from "react"` used to compile to `react_ns.default`, and a
    /// CommonJS package's `module.exports` has no `default` — so `React` was
    /// `undefined` and the first `React.Component` in the bundle threw
    /// `Cannot read properties of undefined`. A default import has to mean
    /// `module.exports` for CommonJS and the `default` export for ESM.
    #[test]
    fn client_bundle_default_imports_interop_with_commonjs_packages() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        let pkg = root.join("node_modules").join("cjs-widget");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&pkg).unwrap();

        // A CommonJS package: no `exports` map, no `default` on its exports.
        fs::write(pkg.join("package.json"), r#"{"main":"index.js"}"#).unwrap();
        fs::write(
            pkg.join("index.js"),
            "module.exports = { widgetMarker() {} };",
        )
        .unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import Widget from 'cjs-widget'; export default function Page() { Widget.widgetMarker(); return <main>x</main>; }",
        )
        .unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.source_map = false;
        input.options.emit_chunk_manifest = false;
        let output = bundle(input).unwrap();

        assert!(
            output.code.contains("__esModule"),
            "the bundle must distinguish compiled ES modules from CommonJS: {}",
            &output.code[..output.code.len().min(600)]
        );
        // The page itself is an ES module, so its own namespace carries the
        // marker and a default import of it still resolves to `.default`.
        assert!(
            output.code.contains("__exports.__esModule = true;"),
            "compiled ES modules must mark their namespace"
        );
        assert!(
            output.code.contains("widgetMarker"),
            "the CommonJS package must still be bundled"
        );
    }

    #[test]
    fn tree_shaking_keeps_consumed_exports_from_a_barrel_package() {
        // A barrel of pure re-exports (`@ruvyxa/react`'s `dist/index.js`) puts
        // every name of one `export { … } from` statement on a single linked
        // line. Shaking must judge each name on its own, or an unused sibling
        // takes the consumed one down with it and the import lands `undefined`.
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        let pkg = root.join("node_modules").join("barrel-ui");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&pkg).unwrap();

        fs::write(
            pkg.join("package.json"),
            r#"{"type":"module","main":"index.js"}"#,
        )
        .unwrap();
        fs::write(
            pkg.join("index.js"),
            "export { DEFAULT_WIDTHS, BarrelImage, BarrelPicture } from './image.js';\n",
        )
        .unwrap();
        fs::write(
            pkg.join("image.js"),
            "export const DEFAULT_WIDTHS = [640, 1080];\n\
             export function BarrelImage(props) { return props; }\n\
             export function BarrelPicture(props) { return props; }\n",
        )
        .unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import { BarrelImage } from 'barrel-ui';\n\
             export default function Page() { return <BarrelImage src=\"/x.png\" />; }\n",
        )
        .unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.source_map = false;
        input.options.emit_chunk_manifest = false;
        let output = bundle(input).unwrap();

        let live = |needle: &str| {
            output
                .code
                .lines()
                .any(|line| line.contains(needle) && !line.contains("[tree-shaken]"))
        };

        assert!(
            live("__exports.BarrelImage"),
            "the consumed re-export must survive shaking:\n{}",
            output.code
        );
        assert!(
            !live("__exports.BarrelPicture") && !live("__exports.DEFAULT_WIDTHS"),
            "unused line-mates should still shake out:\n{}",
            output.code
        );
    }

    #[test]
    fn tree_shaking_keeps_exports_reached_through_a_namespace_import() {
        // `import * as ui` binds the whole namespace to a local alias, so
        // `ui.NsWidget` never appears as `__ruv_x__.NsWidget`. Shaking must not
        // conclude the export is dead just because it cannot see the read.
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        let pkg = root.join("node_modules").join("ns-ui");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&pkg).unwrap();

        fs::write(
            pkg.join("package.json"),
            r#"{"type":"module","main":"index.js"}"#,
        )
        .unwrap();
        fs::write(
            pkg.join("index.js"),
            "export function NsWidget(props) { return props; }\n\
             export function NsOther(props) { return props; }\n",
        )
        .unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import * as ui from 'ns-ui';\n\
             export default function Page() { return <ui.NsWidget x={1} />; }\n",
        )
        .unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.source_map = false;
        input.options.emit_chunk_manifest = false;
        let output = bundle(input).unwrap();

        assert!(
            output
                .code
                .lines()
                .any(|line| line.contains("__exports.NsWidget") && !line.contains("[tree-shaken]")),
            "an export read through a namespace alias must survive:\n{}",
            output.code
        );
    }

    #[test]
    fn client_bundle_resolves_transitive_commonjs_dependencies() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        let react_dom = root.join("node_modules/react-dom");
        let scheduler = react_dom.join("node_modules/scheduler");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(react_dom.join("cjs")).unwrap();
        fs::create_dir_all(&scheduler).unwrap();

        fs::write(
            react_dom.join("package.json"),
            r#"{"exports":{"./client":"./client.js"}}"#,
        )
        .unwrap();
        fs::write(
            react_dom.join("client.js"),
            "module.exports = require('./cjs/react-dom-client.production.js');",
        )
        .unwrap();
        fs::write(
            react_dom.join("cjs/react-dom-client.production.js"),
            "const Scheduler = require('scheduler'); module.exports = { hydrateRoot() { Scheduler.schedulerMarker(); } };",
        )
        .unwrap();

        // No `exports` map and no `main`: resolution has to fall back to
        // `index.js`, the way Node does.
        fs::write(scheduler.join("package.json"), r#"{"version":"0.27.0"}"#).unwrap();
        fs::write(
            scheduler.join("index.js"),
            "module.exports = { schedulerMarker() {} };",
        )
        .unwrap();

        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import { hydrateRoot } from 'react-dom/client'; export default function Page() { hydrateRoot(); return <main>Ready</main>; }",
        )
        .unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.source_map = false;
        input.options.emit_chunk_manifest = false;
        let output = bundle(input).unwrap();

        assert!(
            !output.code.contains("RUV1610"),
            "transitive package must resolve instead of becoming a runtime throw"
        );
        assert!(
            output.code.contains("schedulerMarker"),
            "the transitive package must be bundled: {}",
            &output.code[..output.code.len().min(2000)]
        );
        assert!(
            !output.code.contains("require('scheduler')")
                && !output.code.contains("require(\"scheduler\")"),
            "no bare require may survive in a browser bundle"
        );
    }

    /// An unresolvable bare import must not reach the browser as a bare ESM
    /// specifier: nothing resolves it from a `<script type="module">`, so the
    /// whole chunk failed to load and the browser's message named neither the
    /// package nor the file that wanted it. The failure has to survive
    /// minification as an attributable, deferred one.
    #[test]
    fn client_bundle_defers_unresolvable_imports_instead_of_shipping_bare_specifiers() {
        let temp = tempfile::tempdir().unwrap();
        let root = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        fs::write(
            &page,
            "import { thing } from 'ghost-pkg'; export default function Page() { thing(); return <main>x</main>; }",
        )
        .unwrap();

        let mut input = client_input(&root, &app, page, vec![], "/");
        input.options.minify = true;
        input.options.source_map = false;
        input.options.emit_chunk_manifest = false;
        let output = bundle(input).unwrap();

        assert!(
            !output.code.contains("from\"ghost-pkg\"")
                && !output.code.contains("from \"ghost-pkg\""),
            "bare specifier leaked into the browser bundle: {}",
            &output.code[..output.code.len().min(600)]
        );
        assert!(
            output.code.contains("RUV1611"),
            "the deferred failure must survive minification: {}",
            &output.code[..output.code.len().min(600)]
        );
        assert!(
            output.code.contains("ghost-pkg") && output.code.contains("page.tsx"),
            "the stub must still name the package and the importer: {}",
            &output.code[..output.code.len().min(600)]
        );
    }
}
