//! # ruvyxa_bundler
//!
//! Native TypeScript/JSX compiler and module bundler for the Ruvyxa framework.
//!
//! This crate provides Ruvyxa's native production bundling pipeline and
//! integrates directly with [`ruvyxa_diagnostics`]
//! and the route graph from `ruvyxa_graph`.
//!
//! ## Pipeline
//!
//! ```text
//! Entry file (TSX/TS/JSX/JS)
//!   └─ resolver   → resolve all imports to absolute paths
//!                   (package.json `exports` map, tsconfig `paths`/`baseUrl`)
//!   └─ compiler   → strip types + transform JSX (classic or automatic runtime)
//!                   + expand enums + strip decorators
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
pub mod incremental;
pub mod linker;
pub mod minifier;
pub mod output;
pub mod plugin;
pub mod resolver;
pub mod sourcemap;
pub mod types;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::cache::CompileCache;
use crate::chunking::{
    build_dynamic_output_chunks, dynamic_import_chunks, plan_dynamic_chunk_files,
    static_entry_modules,
};
use crate::plugin::PluginPipeline;
use crate::resolver::ResolveGraphCache;
pub use context::BundleContext;
pub use types::*;

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
    bundle_with_parts(
        input,
        context.compile_cache(),
        context.graph_cache(),
        context.plugins(),
        shared_modules,
    )
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
            format!(
                "import * as __ruvyxa_shared_{index} from \"{}\";",
                path.replace('"', "\\\"")
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
    let graph = resolver::resolve_graph_with_plugins(
        &entry_source,
        &entry_label,
        &input.project_root,
        &input.app_dir,
        context.graph_cache(),
        context.plugins(),
        input.target,
    )?;
    let (compiled, _) = compiler::compile_graph_with_pipeline_and_maps(
        &graph,
        &input,
        context.compile_cache(),
        context.plugins(),
    )?;
    let mut diagnostics = Vec::new();
    boundary::check(&compiled, &input, &mut diagnostics)?;
    let shared_modules = compiled
        .into_iter()
        .filter(|module| !module.is_external && module.path != *entry_label)
        .collect::<Vec<_>>();
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

fn bundle_with_parts(
    input: BundleInput,
    compile_cache: &CompileCache,
    graph_cache: &ResolveGraphCache,
    plugins: &PluginPipeline,
    shared_modules: &BTreeSet<PathBuf>,
) -> Result<BundleOutput> {
    let started = Instant::now();

    // 1. Build the virtual entry source that wires layouts → page.
    let (entry_source, entry_label) = output::build_entry_source(&input);

    // 2. Resolve the full dependency graph from the entry.
    let graph = resolver::resolve_graph_with_plugins(
        &entry_source,
        &entry_label,
        &input.project_root,
        &input.app_dir,
        graph_cache,
        plugins,
        input.target,
    )?;

    // 3. Compile each module (strip TS types, transform JSX).
    let (compiled, plugin_source_maps) =
        compiler::compile_graph_with_pipeline_and_maps(&graph, &input, compile_cache, plugins)?;

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
    let linked_modules = static_modules
        .iter()
        .filter(|module| !shared_modules.contains(&module.path))
        .cloned()
        .collect::<Vec<_>>();
    let chunks = if split_dynamic_imports {
        build_dynamic_output_chunks(&compiled, &input, &dynamic_import_files)?
    } else {
        Vec::new()
    };

    // 6. Link modules into a single concatenated script. This also detects circular dependencies
    // and returns an error.
    let linked = linker::link_parallel_with_dynamic_imports_and_shared_modules(
        &linked_modules,
        &input,
        &dynamic_import_files,
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
    let code = output::wrap(final_code, &input);

    // Count modules whose JS came from the compile cache, not freshly compiled.
    let cache_hits = compiled.iter().filter(|m| m.cache_hit).count();

    // 9. Generate source map if requested.
    let source_map = if input.options.source_map {
        let hash = blake3::hash(code.as_bytes()).to_hex();
        let map_file = format!("{}.js.map", &hash[..16]);
        let mut builder = sourcemap::SourceMapBuilder::new(&map_file, &input.project_root);

        let wrapper_lines = match input.target {
            BundleTarget::Client => 2,
            BundleTarget::Ssr => 3,
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
            let imported_plugin_map = plugin_source_maps
                .get(&module.path)
                .map(String::as_str)
                .is_some_and(|map| builder.add_source_map(map, current_line));
            if !imported_plugin_map {
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

            let modules: Vec<String> = linker::ordered_project_modules(&static_modules)
                .iter()
                .filter(|m| !m.is_external)
                .map(|m| m.path.display().to_string().replace('\\', "/"))
                .collect();

            let dynamic_imports = dynamic_import_chunks(&compiled, &dynamic_import_files);

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
        module_count: graph.len(),
        output_bytes,
        estimated_gz_bytes: (output_bytes as f64 * 0.35) as usize,
        minified: minify_output,
        tree_shaken: input.options.tree_shaking,
        duration_ms: started.elapsed().as_millis() as u64,
        tree_shaken_modules,
        cache_hits,
    };

    Ok(BundleOutput {
        code,
        source_map,
        diagnostics,
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
    fn bundle_context_reuses_graph_cache_across_routes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
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
        let root = temp.path().canonicalize().unwrap();
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
    fn bundles_markdown_page_through_native_content_pipeline() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
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
    fn bundle_manifest_records_dynamic_import_split_points() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
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
    fn keeps_overlapping_dynamic_closures_in_the_entry_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
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
        let root = temp.path().canonicalize().unwrap();
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
        let root = temp.path().canonicalize().unwrap();
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
        let root = temp.path().canonicalize().unwrap();
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
        let root = temp.path().canonicalize().unwrap();
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
