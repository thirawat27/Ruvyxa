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

pub mod ast;
pub mod boundary;
pub mod cache;
pub mod chunking;
pub mod compiler;
pub mod content;
pub mod context;
pub mod hooks;
pub mod incremental;
pub mod linker;
pub mod minifier;
pub mod output;
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
            .collect()
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
        context.build_hooks(),
    )?;
    bundle_prepared(&prepared, shared_modules)
}

/// Resolve and compile a route once so it can be inspected and emitted later.
pub fn prepare_bundle(input: BundleInput, context: &BundleContext) -> Result<PreparedBundle> {
    prepare_bundle_with_parts(
        input,
        context.compile_cache(),
        context.graph_cache(),
        context.build_hooks(),
    )
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
    };
    let graph = resolver::resolve_graph_with_hooks(
        &entry_source,
        &entry_label,
        &input.project_root,
        &input.app_dir,
        context.graph_cache(),
        context.build_hooks(),
        input.target,
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
        for dependency in &module.deps {
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
    build_hooks: &BuildHookPipeline,
) -> Result<PreparedBundle> {
    let started = Instant::now();

    // 1. Build the virtual entry source that wires layouts → page.
    let (entry_source, entry_label) = output::build_entry_source(&input);

    // 2. Resolve the full dependency graph from the entry.
    let graph = resolver::resolve_graph_with_hooks(
        &entry_source,
        &entry_label,
        &input.project_root,
        &input.app_dir,
        graph_cache,
        build_hooks,
        input.target,
    )?;

    // 3. Compile each module (strip TS types, transform JSX).
    let (compiled, hook_source_maps) =
        compiler::compile_graph_with_hooks_and_maps(&graph, &input, compile_cache, build_hooks)?;

    // 4. Enforce server/client boundaries.
    let mut diagnostics = Vec::new();
    boundary::check(&compiled, &input, &mut diagnostics)?;

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

    Ok(PreparedBundle {
        input,
        compiled,
        hook_source_maps,
        diagnostics,
        dynamic_import_files,
        static_modules,
        graph_module_count: graph.len(),
        prepare_duration: started.elapsed(),
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
    let linked = linker::link_parallel_with_dynamic_imports_and_shared_modules(
        &linked_modules,
        input,
        dynamic_import_files,
        shared_modules,
    )?;

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

            Some(ChunkManifest {
                bundle_id,
                route: input.request_path.clone(),
                modules,
                output_file,
                source_map_file: sm_file,
                size_bytes: code.len(),
                dynamic_imports,
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

    Ok(BundleOutput {
        code,
        source_map,
        diagnostics: prepared.diagnostics.clone(),
        stats,
        chunk_manifest,
        chunks,
    })
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
}
