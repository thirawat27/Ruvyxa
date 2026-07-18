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
}

/// JSX transform runtime mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum JsxRuntime {
    Classic,
    #[default]
    Automatic,
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

/// ECMAScript target version for output.
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
    pub minify: bool,
    pub source_map: bool,
    pub tree_shaking: bool,
    pub jsx_runtime: JsxRuntime,
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
            es_target: EsTarget::Es2022,
            split_strategy: SplitStrategy::Single,
            emit_chunk_manifest: false,
            collect_module_manifest: false,
        }
    }
}

/// Input descriptor for a single bundle job.
#[derive(Debug, Clone)]
pub struct BundleInput {
    pub entry: PathBuf,
    pub project_root: PathBuf,
    pub app_dir: PathBuf,
    pub layouts: Vec<PathBuf>,
    pub request_path: String,
    pub target: BundleTarget,
    pub options: BundleOptions,
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
