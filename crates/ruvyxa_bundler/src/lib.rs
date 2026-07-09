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
//!   └─ minifier   → scope-aware identifier mangling + dead-code elimination
//!   └─ output     → wrap in IIFE (client) or ESM (SSR)
//!                   (chunk manifest + HTML preload hints)
//! ```

pub mod ast;
pub mod boundary;
pub mod cache;
pub mod compiler;
pub mod incremental;
pub mod linker;
pub mod minifier;
pub mod output;
pub mod plugin;
pub mod resolver;
pub mod sourcemap;

use std::path::PathBuf;
use std::time::Instant;

use crate::cache::CompileCache;
use crate::incremental::IncrementalGraphCache;
use crate::plugin::PluginPipeline;
use crate::resolver::ResolveGraphCache;
use ruvyxa_diagnostics::Diagnostic;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────

/// Which target environment to emit code for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BundleTarget {
    /// Browser IIFE bundle (hydration entry).
    Client,
    /// Node.js ESM module (SSR render entry).
    Ssr,
}

/// JSX transform runtime mode.
///
/// - `Classic` — the default; emits `React.createElement(…)` calls.
///   Requires `import React from "react"` in every file that uses JSX.
/// - `Automatic` — React 17+ automatic runtime; emits `_jsx(…)` calls and
///   auto-injects `import { jsx as _jsx, … } from "react/jsx-runtime"`.
///   No per-file React import is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum JsxRuntime {
    #[default]
    Classic,
    Automatic,
}

/// Code-splitting strategy for a bundle job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SplitStrategy {
    /// All modules concatenated into a single output file (default).
    #[default]
    Single,
    /// Each entry point gets its own chunk; shared dependencies are split into
    /// a common chunk when they appear in two or more entry bundles.
    Route,
}

/// ECMAScript target version for output.
///
/// The compiler always produces valid ES2020+ output; this field controls
/// which syntax features the minifier may use/remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EsTarget {
    Es2018,
    Es2019,
    Es2020,
    #[default]
    Es2022,
    EsNext,
}

/// Options forwarded from `ruvyxa.config.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleOptions {
    /// Minify identifiers and strip whitespace.
    pub minify: bool,
    /// Emit source maps alongside the bundle.
    pub source_map: bool,
    /// Enable tree-shaking (DCE on exported symbols).
    pub tree_shaking: bool,
    /// JSX transform runtime mode.
    pub jsx_runtime: JsxRuntime,
    /// ECMAScript output target.
    pub es_target: EsTarget,
    /// Code-splitting strategy.
    pub split_strategy: SplitStrategy,
    /// Emit a `chunk-manifest.json` alongside bundles (useful for preload hints).
    pub emit_chunk_manifest: bool,
}

impl Default for BundleOptions {
    fn default() -> Self {
        Self {
            minify: true,
            source_map: false,
            tree_shaking: true,
            jsx_runtime: JsxRuntime::Classic,
            es_target: EsTarget::Es2022,
            split_strategy: SplitStrategy::Single,
            emit_chunk_manifest: false,
        }
    }
}

/// Input descriptor for a single bundle job.
#[derive(Debug, Clone)]
pub struct BundleInput {
    /// Absolute path to the page or action entry file.
    pub entry: PathBuf,
    /// Absolute path to the project root (used for boundary checks and
    /// resolving `node_modules`).
    pub project_root: PathBuf,
    /// Absolute path to the `app/` directory.
    pub app_dir: PathBuf,
    /// Ordered list of layout files to wrap the entry (root-to-leaf).
    pub layouts: Vec<PathBuf>,
    /// The URL path of the route (e.g. `/blog/:slug`).
    pub request_path: String,
    /// Compilation target.
    pub target: BundleTarget,
    /// Build options.
    pub options: BundleOptions,
}

/// Statistics emitted alongside a completed bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleStats {
    /// Number of modules included in the bundle.
    pub module_count: usize,
    /// Final output size in bytes (uncompressed).
    pub output_bytes: usize,
    /// Estimated gzip size (rough: output_bytes * 0.35).
    pub estimated_gz_bytes: usize,
    /// Whether the output was minified.
    pub minified: bool,
    /// Whether the output uses tree-shaking.
    pub tree_shaken: bool,
    /// Time taken to complete the bundle, in milliseconds.
    pub duration_ms: u64,
    /// Number of modules removed by tree-shaking.
    pub tree_shaken_modules: usize,
    /// Number of modules served from the compile cache (not recompiled).
    pub cache_hits: usize,
}

/// A successfully produced bundle.
#[derive(Debug, Clone)]
pub struct BundleOutput {
    /// Compiled JavaScript source.
    pub code: String,
    /// Optional source map JSON string.
    pub source_map: Option<String>,
    /// Non-fatal diagnostics produced during bundling.
    pub diagnostics: Vec<Diagnostic>,
    /// Bundle statistics.
    pub stats: BundleStats,
    /// Chunk manifest (module path → chunk file name), emitted when
    /// `options.emit_chunk_manifest` is `true`.
    pub chunk_manifest: Option<ChunkManifest>,
    /// Additional chunks discovered by split-point analysis.
    pub chunks: Vec<OutputChunk>,
}

/// A JSON-serializable chunk manifest for use in preload link injection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChunkManifest {
    /// Bundle identifier (blake3 hash prefix of the output).
    pub bundle_id: String,
    /// Route path this manifest belongs to.
    pub route: String,
    /// Ordered list of module paths that were included.
    pub modules: Vec<String>,
    /// Output file name (e.g. `bundle.abc1234.js`).
    pub output_file: String,
    /// Optional source map file name.
    pub source_map_file: Option<String>,
    /// Output size in bytes.
    pub size_bytes: usize,
    /// Dynamic import split points discovered in this route bundle.
    pub dynamic_imports: Vec<DynamicImportChunk>,
}

/// A dynamic import split point.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DynamicImportChunk {
    /// Source module containing the dynamic import.
    pub importer: String,
    /// Resolved dynamically imported module path.
    pub module: String,
    /// Content-addressed chunk file name reserved for this split point.
    pub file: String,
}

/// Additional chunk file produced by the bundler.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputChunk {
    /// Content-addressed file name.
    pub file_name: String,
    /// JavaScript source for the chunk.
    pub code: String,
    /// Ordered module paths represented by this chunk.
    pub modules: Vec<String>,
    /// Chunk category.
    pub kind: OutputChunkKind,
}

/// Chunk category.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OutputChunkKind {
    #[default]
    DynamicImport,
    SharedRoute,
}

/// Shared state for a batch of bundle jobs.
///
/// Production builds should keep one context for the whole route batch so
/// parallel workers reuse compiled transforms, resolved specifiers, and source
/// file reads for shared layouts/components.
#[derive(Debug, Clone)]
pub struct BundleContext {
    compile_cache: CompileCache,
    graph_cache: ResolveGraphCache,
    incremental: IncrementalGraphCache,
    plugins: PluginPipeline,
}

impl BundleContext {
    /// Create a context rooted at the project cache directory.
    pub fn new(project_root: impl AsRef<std::path::Path>) -> Self {
        let root = project_root.as_ref();
        Self {
            compile_cache: CompileCache::new(root, true),
            graph_cache: ResolveGraphCache::new(),
            incremental: IncrementalGraphCache::new(root, true),
            plugins: PluginPipeline::empty(),
        }
    }

    /// Create a context from explicit caches.
    pub fn with_caches(compile_cache: CompileCache, graph_cache: ResolveGraphCache) -> Self {
        Self {
            compile_cache,
            graph_cache,
            incremental: IncrementalGraphCache::disabled(),
            plugins: PluginPipeline::empty(),
        }
    }

    /// Create a context with full cache control.
    pub fn with_all_caches(
        compile_cache: CompileCache,
        graph_cache: ResolveGraphCache,
        incremental: IncrementalGraphCache,
    ) -> Self {
        Self {
            compile_cache,
            graph_cache,
            incremental,
            plugins: PluginPipeline::empty(),
        }
    }

    /// Create a context with explicit caches and a native plugin pipeline.
    pub fn with_plugins(
        compile_cache: CompileCache,
        graph_cache: ResolveGraphCache,
        incremental: IncrementalGraphCache,
        plugins: PluginPipeline,
    ) -> Self {
        Self {
            compile_cache,
            graph_cache,
            incremental,
            plugins,
        }
    }

    /// Compile cache used by this context.
    pub fn compile_cache(&self) -> &CompileCache {
        &self.compile_cache
    }

    /// Resolver/source cache used by this context.
    pub fn graph_cache(&self) -> &ResolveGraphCache {
        &self.graph_cache
    }

    /// Incremental graph cache for cross-build persistence.
    pub fn incremental(&self) -> &IncrementalGraphCache {
        &self.incremental
    }

    /// Native plugin pipeline used by this context.
    pub fn plugins(&self) -> &PluginPipeline {
        &self.plugins
    }

    /// Mutable access to the incremental cache (for recording modules).
    pub fn incremental_mut(&mut self) -> &mut IncrementalGraphCache {
        &mut self.incremental
    }

    /// Save the incremental cache to disk (call after a successful build).
    pub fn save_incremental(&self) -> std::io::Result<()> {
        self.incremental.save()
    }
}

// ─────────────────────────────────────────────
// Error type
// ─────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// A hard diagnostic that aborted the build.
    #[error("{0}")]
    Diagnostic(Box<Diagnostic>),

    /// An I/O error during file reads.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Compiler error from the native transformer.
    #[error("compiler error: {0}")]
    Compiler(String),

    /// A module could not be resolved.
    #[error("cannot resolve '{specifier}' from {importer}")]
    Unresolved {
        specifier: String,
        importer: PathBuf,
    },

    /// A circular dependency was detected in the module graph.
    #[error("circular dependency detected: {cycle}")]
    CircularDependency { cycle: String },
}

pub type Result<T> = std::result::Result<T, BundleError>;

impl From<Diagnostic> for BundleError {
    fn from(d: Diagnostic) -> Self {
        Self::Diagnostic(Box::new(d))
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

/// Bundle a single route using a caller-provided compile cache.
pub fn bundle_with_cache(input: BundleInput, cache: &CompileCache) -> Result<BundleOutput> {
    let graph_cache = ResolveGraphCache::new();
    bundle_with_parts(input, cache, &graph_cache, &PluginPipeline::empty())
}

/// Bundle a single route using shared batch context.
pub fn bundle_with_context(input: BundleInput, context: &BundleContext) -> Result<BundleOutput> {
    bundle_with_parts(
        input,
        context.compile_cache(),
        context.graph_cache(),
        context.plugins(),
    )
}

fn bundle_with_parts(
    input: BundleInput,
    compile_cache: &CompileCache,
    graph_cache: &ResolveGraphCache,
    plugins: &PluginPipeline,
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
    let compiled = compiler::compile_graph_with_pipeline(&graph, &input, compile_cache, plugins)?;

    // 4. Enforce server/client boundaries.
    let mut diagnostics = Vec::new();
    boundary::check(&compiled, &input, &mut diagnostics)?;

    // 5. Link modules into a single concatenated script.
    //    This also detects circular dependencies and returns an error.
    let linked = linker::link_parallel(&compiled, &input)?;

    // 6. Optionally tree-shake, then minify. Tree-shaking is controlled
    // independently from whitespace/identifier minification.
    let optimized_linked = if input.options.tree_shaking {
        minifier::tree_shake_exports(&linked)
    } else {
        linked.clone()
    };
    let final_code = if input.options.minify {
        minifier::minify_parallel_with_options(&optimized_linked, input.target, false)?
    } else {
        optimized_linked.clone()
    };

    // 7. Wrap in the appropriate output format.
    let code = output::wrap(final_code, &input);

    // Count modules whose JS came from the compile cache, not freshly compiled.
    let cache_hits = compiled.iter().filter(|m| m.cache_hit).count();

    // 8. Generate source map if requested.
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
        for module in linker::ordered_project_modules(&compiled) {
            if module.is_external {
                continue;
            }
            let source_idx = builder.add_source(&module.path, Some(&module.js));
            let line_count = module.js.lines().count() as u32;
            builder.add_identity_mappings(source_idx, &module.js, current_line);
            current_line += line_count + 5;
        }

        Some(builder.to_json())
    } else {
        None
    };

    let chunks = build_dynamic_output_chunks(&compiled, &input)?;

    // 9. Optionally emit a chunk manifest.
    let chunk_manifest = if input.options.emit_chunk_manifest {
        let hash = blake3::hash(code.as_bytes()).to_hex();
        let bundle_id = hash[..16].to_string();
        let output_file = format!("{bundle_id}.js");
        let sm_file = source_map.as_ref().map(|_| format!("{bundle_id}.js.map"));

        let modules: Vec<String> = linker::ordered_project_modules(&compiled)
            .iter()
            .filter(|m| !m.is_external)
            .map(|m| m.path.display().to_string().replace('\\', "/"))
            .collect();

        let dynamic_imports = dynamic_import_chunks(&compiled, &chunks);

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
        minified: input.options.minify,
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

fn build_dynamic_output_chunks(
    compiled: &[compiler::CompiledModule],
    input: &BundleInput,
) -> Result<Vec<OutputChunk>> {
    use std::collections::{BTreeMap, BTreeSet};

    let module_map: BTreeMap<PathBuf, &compiler::CompiledModule> = compiled
        .iter()
        .filter(|module| !module.is_external)
        .map(|module| (module.path.clone(), module))
        .collect();
    let mut dynamic_roots = BTreeSet::<PathBuf>::new();

    for module in compiled.iter().filter(|module| !module.is_external) {
        let ast = ast::parse_module(&module.js);
        for specifier in ast.dynamic_import_specifiers() {
            if let Some(dep) = linker::find_dep_for_specifier(&specifier, &module.deps) {
                dynamic_roots.insert(dep.clone());
            }
        }
    }

    let mut chunks = Vec::new();
    for root in dynamic_roots {
        let mut selected = BTreeSet::new();
        collect_transitive_modules(&root, &module_map, &mut selected);
        let modules = compiled
            .iter()
            .filter(|module| selected.contains(&module.path))
            .cloned()
            .collect::<Vec<_>>();

        if modules.is_empty() {
            continue;
        }

        let mut linked = linker::link_parallel(&modules, input)?;
        linked.push_str("export default ");
        linked.push_str(&linker::module_id(&root));
        linked.push_str(";\n");

        let code = if input.options.minify {
            minifier::minify_parallel_with_options(&linked, input.target, false)?
        } else {
            linked
        };
        let hash = blake3::hash(code.as_bytes()).to_hex();
        let file_name = format!("chunk.{}.js", &hash[..16]);
        let modules = modules
            .iter()
            .map(|module| module.path.display().to_string().replace('\\', "/"))
            .collect::<Vec<_>>();

        chunks.push(OutputChunk {
            file_name,
            code,
            modules,
            kind: OutputChunkKind::DynamicImport,
        });
    }

    Ok(chunks)
}

fn collect_transitive_modules(
    path: &PathBuf,
    module_map: &std::collections::BTreeMap<PathBuf, &compiler::CompiledModule>,
    selected: &mut std::collections::BTreeSet<PathBuf>,
) {
    if !selected.insert(path.clone()) {
        return;
    }

    let Some(module) = module_map.get(path) else {
        return;
    };

    for dep in &module.deps {
        if module_map.contains_key(dep) {
            collect_transitive_modules(dep, module_map, selected);
        }
    }
}

fn dynamic_import_chunks(
    compiled: &[compiler::CompiledModule],
    output_chunks: &[OutputChunk],
) -> Vec<DynamicImportChunk> {
    let mut dynamic_imports = Vec::new();
    for module in compiled.iter().filter(|module| !module.is_external) {
        let ast = ast::parse_module(&module.js);
        for specifier in ast.dynamic_import_specifiers() {
            if let Some(dep) = linker::find_dep_for_specifier(&specifier, &module.deps) {
                let module_path = dep.display().to_string().replace('\\', "/");
                let file = output_chunks
                    .iter()
                    .find(|chunk| chunk.modules.iter().any(|m| m == &module_path))
                    .map(|chunk| chunk.file_name.clone())
                    .unwrap_or_else(|| {
                        let hash = blake3::hash(module_path.as_bytes()).to_hex();
                        format!("chunk.{}.js", &hash[..16])
                    });
                dynamic_imports.push(DynamicImportChunk {
                    importer: module.path.display().to_string().replace('\\', "/"),
                    module: module_path,
                    file,
                });
            }
        }
    }
    dynamic_imports
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
}
