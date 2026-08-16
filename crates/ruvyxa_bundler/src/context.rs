//! Shared bundler execution context.

use crate::artifact_graph::ArtifactTaskGraph;
use crate::cache::CompileCache;
use crate::cache_budget::{CacheBudget, CacheBudgetSnapshot, CachePressureLevel};
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
    artifacts: ArtifactTaskGraph,
    cache_budget: CacheBudget,
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
            artifacts: ArtifactTaskGraph::new(root, true),
            cache_budget: default_cache_budget(),
            build_hooks: BuildHookPipeline::empty(),
        }
    }

    /// Create a production context whose compile and dependency-graph caches
    /// share an explicit root and configuration namespace.
    pub fn for_build(
        compile_cache: CompileCache,
        graph_cache: ResolveGraphCache,
        cache_dir: &std::path::Path,
        namespace: &str,
    ) -> Self {
        Self::for_build_with_artifacts(compile_cache, graph_cache, cache_dir, namespace, true)
    }

    /// Create a production context with an explicit artifact-graph bypass.
    /// Stage caches continue operating when the graph is bypassed.
    pub fn for_build_with_artifacts(
        compile_cache: CompileCache,
        graph_cache: ResolveGraphCache,
        cache_dir: &std::path::Path,
        namespace: &str,
        artifact_graph_enabled: bool,
    ) -> Self {
        Self {
            compile_cache,
            graph_cache,
            incremental: IncrementalGraphCache::at_dir(cache_dir, namespace, true),
            artifacts: ArtifactTaskGraph::at_dir(cache_dir, namespace, artifact_graph_enabled),
            cache_budget: default_cache_budget(),
            build_hooks: BuildHookPipeline::empty(),
        }
    }

    /// Create a build-hook context with a namespaced persistent artifact graph.
    /// The hook implementation and inputs must be represented by `namespace`.
    pub fn with_build_hooks_for_build(
        compile_cache: CompileCache,
        graph_cache: ResolveGraphCache,
        incremental: IncrementalGraphCache,
        build_hooks: BuildHookPipeline,
        cache_dir: &std::path::Path,
        namespace: &str,
        artifact_graph_enabled: bool,
    ) -> Self {
        Self {
            compile_cache,
            graph_cache,
            incremental,
            artifacts: ArtifactTaskGraph::at_dir(cache_dir, namespace, artifact_graph_enabled),
            cache_budget: default_cache_budget(),
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

    pub fn artifacts(&self) -> &ArtifactTaskGraph {
        &self.artifacts
    }

    /// Apply the shared build-cache budget in correctness-preserving order.
    pub fn enforce_cache_budget(&self) -> CacheBudgetSnapshot {
        let mut resident = self.cache_resident_bytes();
        let action = self.cache_budget.observe(resident);
        if action.level != CachePressureLevel::None {
            let resolver_evictions = self.graph_cache.evict_disposable();
            self.cache_budget
                .record_eviction("resolverDerived", resolver_evictions);
            resident = self.cache_resident_bytes();
            if resident > action.target_bytes {
                let fixed_bytes = self
                    .graph_cache
                    .stats()
                    .disposable_bytes
                    .saturating_add(self.compile_cache.memory_resident_bytes());
                let artifact_target = action.target_bytes.saturating_sub(fixed_bytes);
                let artifact_evictions = self.artifacts.evict_to_bytes(artifact_target);
                self.cache_budget
                    .record_eviction("artifactMetadata", artifact_evictions);
                resident = self.cache_resident_bytes();
            }
            if resident > action.target_bytes {
                let fixed_bytes = self
                    .graph_cache
                    .stats()
                    .disposable_bytes
                    .saturating_add(self.artifacts.evictable_bytes());
                let compile_target = action.target_bytes.saturating_sub(fixed_bytes);
                let compiler_evictions = self.compile_cache.evict_memory_to(compile_target);
                self.cache_budget
                    .record_eviction("compilerMemory", compiler_evictions);
                resident = self.cache_resident_bytes();
            }
            self.cache_budget.observe(resident);
        }
        self.cache_budget.snapshot(resident)
    }

    pub fn cache_budget_snapshot(&self) -> CacheBudgetSnapshot {
        self.cache_budget.snapshot(self.cache_resident_bytes())
    }

    #[cfg(test)]
    pub(crate) fn with_test_cache_budget(mut self, cache_budget: CacheBudget) -> Self {
        self.cache_budget = cache_budget;
        self
    }

    pub fn save_incremental(&self) -> std::io::Result<()> {
        self.incremental.save()?;
        self.artifacts.save()
    }

    fn cache_resident_bytes(&self) -> u64 {
        self.compile_cache
            .memory_resident_bytes()
            .saturating_add(self.graph_cache.stats().disposable_bytes)
            .saturating_add(self.artifacts.evictable_bytes())
    }
}

fn default_cache_budget() -> CacheBudget {
    const DEFAULT_BUILD_CACHE_MIB: u64 = 256;
    const MAX_BUILD_CACHE_MIB: u64 = 16 * 1024;
    let configured = std::env::var("RUVYXA_BUILD_CACHE_MEMORY_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=MAX_BUILD_CACHE_MIB).contains(value))
        .unwrap_or(DEFAULT_BUILD_CACHE_MIB);
    CacheBudget::from_mebibytes(configured).expect("validated cache budget must fit in bytes")
}
