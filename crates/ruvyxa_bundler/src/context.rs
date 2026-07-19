//! Shared bundler execution context.

use crate::cache::CompileCache;
use crate::hooks::BuildHookPipeline;
use crate::incremental::IncrementalGraphCache;
use crate::resolver::ResolveGraphCache;

/// Shared state for a batch of bundle jobs.
///
/// Production builds should keep one context for the whole route batch so
/// parallel workers reuse compiled transforms, resolved specifiers, source
/// reads, incremental state, and TypeScript build hooks.
#[derive(Debug, Clone)]
pub struct BundleContext {
    compile_cache: CompileCache,
    graph_cache: ResolveGraphCache,
    incremental: IncrementalGraphCache,
    build_hooks: BuildHookPipeline,
}

impl BundleContext {
    /// Create a context rooted at the project cache directory.
    pub fn new(project_root: impl AsRef<std::path::Path>) -> Self {
        let root = project_root.as_ref();
        Self {
            compile_cache: CompileCache::new(root, true),
            graph_cache: ResolveGraphCache::new(),
            incremental: IncrementalGraphCache::new(root, true),
            build_hooks: BuildHookPipeline::empty(),
        }
    }

    /// Create a context from explicit caches.
    pub fn with_caches(compile_cache: CompileCache, graph_cache: ResolveGraphCache) -> Self {
        Self {
            compile_cache,
            graph_cache,
            incremental: IncrementalGraphCache::disabled(),
            build_hooks: BuildHookPipeline::empty(),
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
            build_hooks: BuildHookPipeline::empty(),
        }
    }

    /// Create a context with explicit caches and a TypeScript build-hook host.
    pub fn with_build_hooks(
        compile_cache: CompileCache,
        graph_cache: ResolveGraphCache,
        incremental: IncrementalGraphCache,
        build_hooks: BuildHookPipeline,
    ) -> Self {
        Self {
            compile_cache,
            graph_cache,
            incremental,
            build_hooks,
        }
    }

    pub fn compile_cache(&self) -> &CompileCache {
        &self.compile_cache
    }

    pub fn graph_cache(&self) -> &ResolveGraphCache {
        &self.graph_cache
    }

    pub fn incremental(&self) -> &IncrementalGraphCache {
        &self.incremental
    }

    pub fn build_hooks(&self) -> &BuildHookPipeline {
        &self.build_hooks
    }

    pub fn incremental_mut(&mut self) -> &mut IncrementalGraphCache {
        &mut self.incremental
    }

    pub fn save_incremental(&self) -> std::io::Result<()> {
        self.incremental.save()
    }
}
