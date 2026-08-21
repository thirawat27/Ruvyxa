//! Public bundler contracts.

use std::path::PathBuf;

use ruvyxa_diagnostics::Diagnostic;
use serde::{Deserialize, Serialize};

/// Which target environment to emit code for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BundleTarget {
    /// Browser module bundle (hydration entry).
    Client,
    /// Node.js ESM module (SSR render entry).
    Ssr,
    /// Edge runtime ESM module (Cloudflare Workers, Vercel Edge Functions).
    /// Like SSR but restricts Node.js-specific APIs (fs, native modules).
    Edge,
    /// React Server Components graph: Node.js ESM resolved with the
    /// `react-server` export condition.
    ///
    /// A separate target rather than a flag on [`Self::Ssr`] because the
    /// condition changes which *file* a specifier names — `react` itself
    /// resolves to its `react-server` build, which has no `useState` — and that
    /// is a resolution decision, not a rendering one. The two graphs cannot
    /// share a module instance, which is why they are also rendered in
    /// different processes.
    ReactServer,
}

/// JSX transform runtime mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum JsxRuntime {
    Classic,
    #[default]
    Automatic,
}

/// JavaScript language level the emitted bundle is written down to.
///
/// `ruvyxa.config.ts` exposes this as `build.esTarget`. The value reaches
/// `TransformOptions::from_target` here and the `target` option of
/// `transformSync` in `packages/ruvyxa/runtime/compiler.mjs`, so a project
/// renders the same way under `ruvyxa dev` and in a built bundle.
///
/// `es5` is absent because oxc does not implement it. Nothing below
/// [`Self::EsNext`] is refused outright: what a target costs depends on the
/// source, not on the number. Downlevelling private class fields needs runtime
/// helpers from roughly es2021 down, and a `using` declaration needs one at
/// every target below es2026 — so the guard is on the *emitted* code
/// (`compiler::reject_runtime_helpers`) rather than on the configured value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EsTarget {
    Es2015,
    Es2016,
    Es2017,
    Es2018,
    Es2019,
    Es2020,
    Es2021,
    Es2022,
    Es2023,
    Es2024,
    Es2025,
    Es2026,
    #[default]
    EsNext,
}

impl EsTarget {
    /// Every accepted value, in the order the diagnostic lists them.
    pub const ALL: [Self; 13] = [
        Self::Es2015,
        Self::Es2016,
        Self::Es2017,
        Self::Es2018,
        Self::Es2019,
        Self::Es2020,
        Self::Es2021,
        Self::Es2022,
        Self::Es2023,
        Self::Es2024,
        Self::Es2025,
        Self::Es2026,
        Self::EsNext,
    ];

    /// The token both compilers hand their transformer.
    ///
    /// One spelling, so the Rust and JavaScript graphs cannot be pointed at
    /// different language levels for the same configuration.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Es2015 => "es2015",
            Self::Es2016 => "es2016",
            Self::Es2017 => "es2017",
            Self::Es2018 => "es2018",
            Self::Es2019 => "es2019",
            Self::Es2020 => "es2020",
            Self::Es2021 => "es2021",
            Self::Es2022 => "es2022",
            Self::Es2023 => "es2023",
            Self::Es2024 => "es2024",
            Self::Es2025 => "es2025",
            Self::Es2026 => "es2026",
            Self::EsNext => "esnext",
        }
    }

    /// Parse a configured value. `es6` is accepted as the alias oxc accepts.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("es6") {
            return Some(Self::Es2015);
        }
        Self::ALL
            .into_iter()
            .find(|target| value.eq_ignore_ascii_case(target.as_str()))
    }

    /// Whether the transform can be skipped entirely.
    pub fn is_default(self) -> bool {
        matches!(self, Self::EsNext)
    }
}

impl std::fmt::Display for EsTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Code-splitting strategy for a bundle job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SplitStrategy {
    /// All modules concatenated into a single output file.
    #[default]
    Single,
    /// Route-oriented chunks with shared module metadata.
    Route,
}

/// Options forwarded from `ruvyxa.config.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleOptions {
    pub minify: bool,
    pub source_map: bool,
    pub tree_shaking: bool,
    pub jsx_runtime: JsxRuntime,
    /// JavaScript language level the emitted modules are written down to.
    #[serde(default)]
    pub es_target: EsTarget,
    pub split_strategy: SplitStrategy,
    pub emit_chunk_manifest: bool,
    /// Collect a module graph for internal multi-route coordination without
    /// requiring a user-facing chunk manifest file.
    pub collect_module_manifest: bool,
}

impl Default for BundleOptions {
    fn default() -> Self {
        Self {
            minify: true,
            source_map: false,
            tree_shaking: true,
            jsx_runtime: JsxRuntime::Automatic,
            es_target: EsTarget::EsNext,
            split_strategy: SplitStrategy::Single,
            emit_chunk_manifest: false,
            collect_module_manifest: false,
        }
    }
}

/// Special files that wrap a route's page, à la Next.js.
///
/// Each field is the nearest `error.tsx` / `loading.tsx` / `not-found.tsx`
/// walking up from the route directory to the app root, or `None` when the
/// route has no such file in scope. `Default` (all `None`) is the common case
/// and keeps the many test constructors terse.
#[derive(Debug, Clone, Default)]
pub struct RouteSpecials {
    pub error: Option<PathBuf>,
    pub loading: Option<PathBuf>,
    pub not_found: Option<PathBuf>,
}

/// One parallel-route slot handed to the entry generator.
///
/// `level` is a directory, not a file: it names which layout in the chain
/// receives this slot, and the generator matches it against each layout's own
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSlotInput {
    pub level: PathBuf,
    pub name: String,
    pub file: PathBuf,
}

/// One intercepting route handed to the entry generator.
///
/// `level` is the directory holding the `@name` folder, so this merges into the
/// same wrapper level a slot does. `level_id` is that directory as a route id
/// (`app/feed`) and is what the emitted source carries: the client router names
/// the slot it is filling, and a route id is the one spelling both languages
/// and both hosts already agree on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteInterceptInput {
    pub level: PathBuf,
    pub level_id: String,
    pub name: String,
    /// Route pattern this interception covers, matched against the URL.
    pub target: String,
    pub file: PathBuf,
}

/// Input descriptor for a single bundle job.
#[derive(Debug, Clone)]
pub struct BundleInput {
    pub entry: PathBuf,
    pub project_root: PathBuf,
    pub app_dir: PathBuf,
    pub layouts: Vec<PathBuf>,
    /// `template.tsx` files on the path to this route, root first.
    ///
    /// Kept apart from `layouts` because a level may have either, both, or
    /// neither, and the two interleave during composition. Templates contribute
    /// no metadata — only `layouts` and the page do.
    pub templates: Vec<PathBuf>,
    /// Parallel-route slots, as `(level directory, slot name, file)`.
    ///
    /// The level is the directory holding the `@name` folder; its layout is the
    /// one that receives the slot as a prop.
    pub slots: Vec<RouteSlotInput>,
    /// Interceptions this route can render into one of its slots.
    ///
    /// Carried by the route the user is standing on rather than by the route
    /// being intercepted: the overlay has to be in the bundle that is already
    /// running, or opening it would cost the round trip it exists to avoid.
    pub intercepts: Vec<RouteInterceptInput>,
    pub request_path: String,
    pub target: BundleTarget,
    pub options: BundleOptions,
    pub specials: RouteSpecials,
}

/// Statistics emitted alongside a completed bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleStats {
    pub module_count: usize,
    pub output_bytes: usize,
    pub estimated_gz_bytes: usize,
    pub minified: bool,
    pub tree_shaken: bool,
    pub duration_ms: u64,
    pub tree_shaken_modules: usize,
    pub cache_hits: usize,
}

/// A successfully produced bundle.
#[derive(Debug, Clone)]
pub struct BundleOutput {
    pub code: String,
    pub source_map: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub stats: BundleStats,
    pub chunk_manifest: Option<ChunkManifest>,
    pub chunks: Vec<OutputChunk>,
}

/// Executable module registry shared by more than one route bundle.
#[derive(Debug, Clone)]
pub struct SharedRouteBundleOutput {
    pub code: String,
    pub modules: Vec<PathBuf>,
}

/// A JSON-serializable chunk manifest for use in preload link injection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChunkManifest {
    pub bundle_id: String,
    pub route: String,
    pub modules: Vec<String>,
    pub output_file: String,
    pub source_map_file: Option<String>,
    pub size_bytes: usize,
    pub dynamic_imports: Vec<DynamicImportChunk>,
    /// Deterministically ordered files expanded from `import.meta.glob` calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub glob_matches: Vec<String>,
    /// Canonical lane ownership consumed by version-bound runtime protocols.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_manifest: Option<crate::reference_manifest::ReferenceManifest>,
}

/// A dynamic import split point.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DynamicImportChunk {
    pub importer: String,
    pub module: String,
    pub file: String,
}

/// Additional chunk file produced by the bundler.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputChunk {
    pub file_name: String,
    pub code: String,
    pub modules: Vec<String>,
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
