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
//!   └─ compiler   → SWC: strip types + transform JSX
//!   └─ boundary   → enforce server/client rules (RUV1007, RUV1008, RUV1010)
//!   └─ linker     → topological sort + concatenate modules
//!   └─ minifier   → identifier shortening + dead-code elimination
//!   └─ output     → wrap in IIFE (client) or ESM (SSR)
//! ```

pub mod boundary;
pub mod cache;
pub mod compiler;
pub mod incremental;
pub mod linker;
pub mod minifier;
pub mod output;
pub mod resolver;
pub mod sourcemap;

use std::path::PathBuf;
use std::time::Instant;

use crate::cache::CompileCache;
use crate::incremental::IncrementalGraphCache;
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

/// Options forwarded from `ruvyxa.config.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleOptions {
    /// Minify identifiers and strip whitespace.
    pub minify: bool,
    /// Emit source maps alongside the bundle.
    pub source_map: bool,
    /// Enable tree-shaking (DCE on exported symbols).
    pub tree_shaking: bool,
}

impl Default for BundleOptions {
    fn default() -> Self {
        Self {
            minify: true,
            source_map: false,
            tree_shaking: true,
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
    /// Only used for `BundleTarget::Client` and `BundleTarget::Ssr`.
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
    /// Final output size in bytes.
    pub output_bytes: usize,
    /// Whether the output was minified.
    pub minified: bool,
    /// Whether the output uses tree-shaking.
    pub tree_shaken: bool,
    /// Time taken to complete the bundle, in milliseconds.
    pub duration_ms: u64,
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
}

impl BundleContext {
    /// Create a context rooted at the project cache directory.
    pub fn new(project_root: impl AsRef<std::path::Path>) -> Self {
        let root = project_root.as_ref();
        Self {
            compile_cache: CompileCache::new(root, true),
            graph_cache: ResolveGraphCache::new(),
            incremental: IncrementalGraphCache::new(root, true),
        }
    }

    /// Create a context from explicit caches.
    pub fn with_caches(compile_cache: CompileCache, graph_cache: ResolveGraphCache) -> Self {
        Self {
            compile_cache,
            graph_cache,
            incremental: IncrementalGraphCache::disabled(),
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

    /// Compiler error from SWC.
    #[error("compiler error: {0}")]
    Compiler(String),

    /// A module could not be resolved.
    #[error("cannot resolve '{specifier}' from {importer}")]
    Unresolved {
        specifier: String,
        importer: PathBuf,
    },
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
/// module cannot be resolved, or a compile error occurs.
pub fn bundle(input: BundleInput) -> Result<BundleOutput> {
    let context = BundleContext::new(&input.project_root);
    bundle_with_context(input, &context)
}

/// Bundle a single route using a caller-provided compile cache.
///
/// Build orchestrators that emit many route bundles should share one
/// [`CompileCache`] so common modules are reused across worker threads and
/// across routes within the same production build.
pub fn bundle_with_cache(input: BundleInput, cache: &CompileCache) -> Result<BundleOutput> {
    let graph_cache = ResolveGraphCache::new();
    bundle_with_parts(input, cache, &graph_cache)
}

/// Bundle a single route using shared batch context.
pub fn bundle_with_context(input: BundleInput, context: &BundleContext) -> Result<BundleOutput> {
    bundle_with_parts(input, context.compile_cache(), context.graph_cache())
}

fn bundle_with_parts(
    input: BundleInput,
    compile_cache: &CompileCache,
    graph_cache: &ResolveGraphCache,
) -> Result<BundleOutput> {
    let started = Instant::now();

    // 1. Build the virtual entry source that wires layouts → page.
    let (entry_source, entry_label) = output::build_entry_source(&input);

    // 2. Resolve the full dependency graph from the entry.
    let graph = resolver::resolve_graph_with_cache(
        &entry_source,
        &entry_label,
        &input.project_root,
        &input.app_dir,
        graph_cache,
    )?;

    // 3. Compile each module (strip TS types, transform JSX).
    let compiled = compiler::compile_graph_with_cache(&graph, &input, compile_cache)?;

    // 4. Enforce server/client boundaries.
    let mut diagnostics = Vec::new();
    boundary::check(&compiled, &input, &mut diagnostics)?;

    // 5. Link modules into a single concatenated script.
    let linked = linker::link_parallel(&compiled, &input)?;

    // 6. Optionally minify.
    let final_code = if input.options.minify {
        minifier::minify_parallel(&linked, input.target)?
    } else {
        linked.clone()
    };

    // 7. Wrap in the appropriate output format.
    let code = output::wrap(final_code, &input);

    // 8. Generate source map if requested.
    let source_map = if input.options.source_map {
        let hash = blake3::hash(code.as_bytes()).to_hex();
        let map_file = format!("{}.js.map", &hash[..16]);
        let mut builder = sourcemap::SourceMapBuilder::new(&map_file, &input.project_root);

        // Count wrapper header lines.
        let wrapper_lines = match input.target {
            BundleTarget::Client => 2, // IIFE header + "use strict"
            BundleTarget::Ssr => 3,    // comment + import React + import renderToString
        };

        // Linker header lines: "// Generated…" + "\"use strict\";" + blank
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
            // linker wraps each module with: comment line + var X = (function() { + "use strict"; + code + })();\n\n
            current_line += line_count + 5; // approximate overhead per module
        }

        Some(builder.to_json())
    } else {
        None
    };

    let stats = BundleStats {
        module_count: graph.len(),
        output_bytes: code.len(),
        minified: input.options.minify,
        tree_shaken: input.options.tree_shaking,
        duration_ms: started.elapsed().as_millis() as u64,
    };

    Ok(BundleOutput {
        code,
        source_map,
        diagnostics,
        stats,
    })
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
}
