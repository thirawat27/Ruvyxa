//! Typed task graph for incremental build artifacts.
//!
//! Existing compiler, resolved-edge, route-plan, and emitted-output caches keep
//! owning artifact bytes. This graph gives those independent stores one shared
//! identity, state, dependency, cancellation, and persistence contract. A graph
//! hit is never treated as artifact bytes or as a source of truth: callers must
//! still validate and load the owning cache entry.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const ARTIFACT_GRAPH_IDENTITY: &str = "ruvyxa_artifact_graph";

/// Stable stage identity for an internal build artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    Source,
    Resolve,
    Transform,
    Analyze,
    ChunkPlan,
    Emit,
    SourceMap,
    Manifest,
}

impl ArtifactKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Resolve => "resolve",
            Self::Transform => "transform",
            Self::Analyze => "analyze",
            Self::ChunkPlan => "chunkPlan",
            Self::Emit => "emit",
            Self::SourceMap => "sourceMap",
            Self::Manifest => "manifest",
        }
    }
}

/// Typed identity of one memoizable build computation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactKey {
    pub kind: ArtifactKind,
    pub namespace: String,
    pub identity: String,
}

impl ArtifactKey {
    /// Derive a collision-safe key from named semantic inputs.
    ///
    /// Inputs are sorted by name, length-framed, and hashed with the artifact
    /// kind and configuration namespace. Callers therefore cannot change a key
    /// merely by iterating the same map in a different order.
    pub fn from_inputs<I, K, V>(kind: ArtifactKind, namespace: &str, inputs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<Vec<u8>>,
    {
        let mut inputs = inputs
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect::<Vec<_>>();
        inputs.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        let mut hasher = blake3::Hasher::new();
        hash_part(&mut hasher, kind.as_str().as_bytes());
        hash_part(&mut hasher, namespace.as_bytes());
        for (name, value) in inputs {
            hash_part(&mut hasher, name.as_bytes());
            hash_part(&mut hasher, &value);
        }
        Self {
            kind,
            namespace: namespace.to_string(),
            identity: hasher.finalize().to_hex().to_string(),
        }
    }
}

fn hash_part(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// One upstream artifact and the content version observed by its consumer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDependency {
    pub key: ArtifactKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl ArtifactDependency {
    pub fn new(key: ArtifactKey, content_hash: Option<String>) -> Self {
        Self { key, content_hash }
    }
}

/// Lifecycle state of one artifact computation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactState {
    Building,
    Ready,
    Failed,
    Cancelled,
}

/// Persisted task metadata. Artifact bytes stay in their owning cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub key: ArtifactKey,
    pub state: ArtifactState,
    pub dependencies: BTreeSet<ArtifactDependency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub generation: u64,
    #[serde(skip)]
    active_builders: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactGraphStats {
    pub records: usize,
    pub ready: usize,
    pub building: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub dependency_edges: usize,
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub resident_bytes: u64,
    pub evictions: u64,
    pub evictable_bytes: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ArtifactGraphError {
    #[error("artifact namespace `{actual}` does not match graph namespace `{expected}`")]
    NamespaceMismatch { expected: String, actual: String },
    #[error("artifact {identity} was started with a different dependency set")]
    DependencyMismatch { identity: String },
    #[error("artifact {identity} completed after its generation was invalidated")]
    StaleCompletion { identity: String },
    #[error("artifact {identity} produced two different outputs in one generation")]
    NonDeterministicOutput { identity: String },
}

#[derive(Debug, Default)]
struct ArtifactGraphInner {
    records: BTreeMap<ArtifactKey, ArtifactRecord>,
    dependents: BTreeMap<ArtifactKey, BTreeSet<ArtifactKey>>,
    hits: u64,
    misses: u64,
    invalidations: u64,
    evictions: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactGraphManifest {
    identity: String,
    namespace: String,
    records: Vec<ArtifactRecord>,
}

/// Shared artifact-task graph owned by a [`crate::BundleContext`].
#[derive(Debug, Clone)]
pub struct ArtifactTaskGraph {
    manifest_path: PathBuf,
    namespace: Arc<str>,
    inner: Arc<Mutex<ArtifactGraphInner>>,
    enabled: bool,
}

impl ArtifactTaskGraph {
    pub fn new(project_root: &Path, enabled: bool) -> Self {
        Self::at_dir(
            &project_root.join(".ruvyxa").join("cache").join("bundler"),
            "default",
            enabled,
        )
    }

    pub fn at_dir(cache_dir: &Path, namespace: &str, enabled: bool) -> Self {
        let manifest_path = cache_dir.join("artifact-graph.json");
        let records = enabled
            .then(|| load_manifest(&manifest_path, namespace))
            .flatten()
            .unwrap_or_default();
        let mut inner = ArtifactGraphInner {
            records,
            dependents: BTreeMap::new(),
            hits: 0,
            misses: 0,
            invalidations: 0,
            evictions: 0,
        };
        for record in inner.records.values_mut() {
            record.active_builders = 0;
            if record.state == ArtifactState::Building {
                record.state = ArtifactState::Cancelled;
                record.reason = Some("build process ended before artifact completion".to_string());
            }
        }
        rebuild_dependents(&mut inner);
        Self {
            manifest_path,
            namespace: Arc::from(namespace),
            inner: Arc::new(Mutex::new(inner)),
            enabled,
        }
    }

    pub fn disabled() -> Self {
        Self {
            manifest_path: PathBuf::new(),
            namespace: Arc::from("disabled"),
            inner: Arc::new(Mutex::new(ArtifactGraphInner::default())),
            enabled: false,
        }
    }

    /// Build a key in this graph's configuration namespace.
    pub fn key<I, K, V>(&self, kind: ArtifactKind, inputs: I) -> ArtifactKey
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<Vec<u8>>,
    {
        ArtifactKey::from_inputs(kind, &self.namespace, inputs)
    }

    /// Publish a completed artifact and return whether the identical record was
    /// already ready. The bytes remain in the stage-specific owning cache.
    /// Conflicting output for the same semantic key fails closed.
    pub fn publish(
        &self,
        key: ArtifactKey,
        dependencies: BTreeSet<ArtifactDependency>,
        content_hash: impl Into<String>,
    ) -> Result<bool, ArtifactGraphError> {
        self.ensure_namespace(&key)?;
        if !self.enabled {
            return Ok(false);
        }
        let content_hash = content_hash.into();
        {
            let mut inner = self.lock();
            if let Some(existing) = inner.records.get(&key) {
                if existing.dependencies != dependencies {
                    return Err(ArtifactGraphError::DependencyMismatch {
                        identity: key.identity,
                    });
                }
                if existing.state == ArtifactState::Ready {
                    if existing.content_hash.as_deref() != Some(content_hash.as_str()) {
                        let identity = key.identity.clone();
                        if let Some(existing) = inner.records.get_mut(&key) {
                            existing.state = ArtifactState::Failed;
                            existing.reason =
                                Some("same semantic key produced different content".to_string());
                        }
                        return Err(ArtifactGraphError::NonDeterministicOutput { identity });
                    }
                    inner.hits = inner.hits.saturating_add(1);
                    return Ok(true);
                }
            }
        }
        self.begin(key, dependencies)?.complete(content_hash)?;
        Ok(false)
    }

    /// Begin one artifact computation and pin its generation until completion.
    pub fn begin(
        &self,
        key: ArtifactKey,
        dependencies: BTreeSet<ArtifactDependency>,
    ) -> Result<ArtifactTask, ArtifactGraphError> {
        self.ensure_namespace(&key)?;
        if !self.enabled {
            return Ok(ArtifactTask {
                graph: self.clone(),
                key,
                generation: 0,
                finished: true,
            });
        }
        let mut inner = self.lock();
        if let Some(existing) = inner.records.get_mut(&key)
            && existing.state == ArtifactState::Building
        {
            if existing.dependencies != dependencies {
                return Err(ArtifactGraphError::DependencyMismatch {
                    identity: key.identity,
                });
            }
            existing.active_builders += 1;
            return Ok(ArtifactTask {
                graph: self.clone(),
                key,
                generation: existing.generation,
                finished: false,
            });
        }

        replace_dependency_edges(&mut inner, &key, &dependencies);
        inner.misses = inner.misses.saturating_add(1);
        let generation = inner
            .records
            .get(&key)
            .map(|record| record.generation.saturating_add(1))
            .unwrap_or(1);
        inner.records.insert(
            key.clone(),
            ArtifactRecord {
                key: key.clone(),
                state: ArtifactState::Building,
                dependencies,
                content_hash: None,
                reason: None,
                generation,
                active_builders: 1,
            },
        );
        Ok(ArtifactTask {
            graph: self.clone(),
            key,
            generation,
            finished: false,
        })
    }

    /// Invalidate an artifact and only its transitive dependents.
    pub fn invalidate(&self, key: &ArtifactKey, reason: impl Into<String>) -> Vec<ArtifactKey> {
        if !self.enabled {
            return Vec::new();
        }
        let reason = reason.into();
        let mut inner = self.lock();
        let mut queue = VecDeque::from([key.clone()]);
        let mut invalidated = BTreeSet::new();
        while let Some(current) = queue.pop_front() {
            if !invalidated.insert(current.clone()) {
                continue;
            }
            if let Some(dependents) = inner.dependents.get(&current) {
                queue.extend(dependents.iter().cloned());
            }
            if let Some(record) = inner.records.get_mut(&current) {
                record.state = ArtifactState::Cancelled;
                record.reason = Some(reason.clone());
                record.content_hash = None;
                record.generation = record.generation.saturating_add(1);
                record.active_builders = 0;
            }
        }
        inner.invalidations = inner.invalidations.saturating_add(invalidated.len() as u64);
        invalidated.into_iter().collect()
    }

    pub fn record(&self, key: &ArtifactKey) -> Option<ArtifactRecord> {
        self.lock().records.get(key).cloned()
    }

    pub fn stats(&self) -> ArtifactGraphStats {
        let inner = self.lock();
        let protected = protected_artifacts(&inner);
        let mut stats = ArtifactGraphStats {
            records: inner.records.len(),
            dependency_edges: inner
                .records
                .values()
                .map(|record| record.dependencies.len())
                .sum(),
            hits: inner.hits,
            misses: inner.misses,
            invalidations: inner.invalidations,
            resident_bytes: inner.records.values().map(artifact_record_bytes).sum(),
            evictions: inner.evictions,
            evictable_bytes: inner
                .records
                .iter()
                .filter(|(key, record)| {
                    record.state != ArtifactState::Building && !protected.contains(*key)
                })
                .map(|(_, record)| artifact_record_bytes(record))
                .sum(),
            ..Default::default()
        };
        for record in inner.records.values() {
            match record.state {
                ArtifactState::Building => stats.building += 1,
                ArtifactState::Ready => stats.ready += 1,
                ArtifactState::Failed => stats.failed += 1,
                ArtifactState::Cancelled => stats.cancelled += 1,
            }
        }
        stats
    }

    pub fn save(&self) -> std::io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let records = self
            .lock()
            .records
            .values()
            .map(|record| {
                let mut record = record.clone();
                record.active_builders = 0;
                if record.state == ArtifactState::Building {
                    record.state = ArtifactState::Cancelled;
                    record.reason = Some("artifact was active when graph was saved".to_string());
                }
                record
            })
            .collect();
        let manifest = ArtifactGraphManifest {
            identity: ARTIFACT_GRAPH_IDENTITY.to_string(),
            namespace: self.namespace.to_string(),
            records,
        };
        let json = serde_json::to_vec(&manifest).map_err(std::io::Error::other)?;
        crate::atomic_file::write_atomic(&self.manifest_path, &json)
    }

    pub fn clear(&self) -> std::io::Result<()> {
        *self.lock() = ArtifactGraphInner::default();
        if self.manifest_path.is_file() {
            fs::remove_file(&self.manifest_path)?;
        }
        Ok(())
    }

    /// Compact unowned leaf records until the graph fits `target_bytes`.
    /// Building records and their dependency closure remain pinned by state and
    /// dependent edges. Eviction can therefore only turn a hit into a rebuild.
    pub fn evict_to_bytes(&self, target_bytes: u64) -> u64 {
        if !self.enabled {
            return 0;
        }
        let mut inner = self.lock();
        let mut resident = inner
            .records
            .values()
            .map(artifact_record_bytes)
            .sum::<u64>();
        let mut evicted = 0_u64;
        while resident > target_bytes {
            let candidate = inner
                .records
                .iter()
                .filter(|(key, record)| {
                    record.state != ArtifactState::Building
                        && record.active_builders == 0
                        && inner.dependents.get(*key).is_none_or(BTreeSet::is_empty)
                })
                .min_by_key(|(_, record)| match record.state {
                    ArtifactState::Failed | ArtifactState::Cancelled => 0,
                    ArtifactState::Ready => 1,
                    ArtifactState::Building => 2,
                })
                .map(|(key, _)| key.clone());
            let Some(key) = candidate else {
                break;
            };
            let Some(record) = inner.records.remove(&key) else {
                break;
            };
            resident = resident.saturating_sub(artifact_record_bytes(&record));
            inner.dependents.remove(&key);
            for dependency in record.dependencies {
                if let Some(dependents) = inner.dependents.get_mut(&dependency.key) {
                    dependents.remove(&key);
                    if dependents.is_empty() {
                        inner.dependents.remove(&dependency.key);
                    }
                }
            }
            evicted = evicted.saturating_add(1);
        }
        inner.evictions = inner.evictions.saturating_add(evicted);
        evicted
    }

    fn ensure_namespace(&self, key: &ArtifactKey) -> Result<(), ArtifactGraphError> {
        if key.namespace == self.namespace.as_ref() || !self.enabled {
            return Ok(());
        }
        Err(ArtifactGraphError::NamespaceMismatch {
            expected: self.namespace.to_string(),
            actual: key.namespace.clone(),
        })
    }

    fn complete(
        &self,
        key: &ArtifactKey,
        generation: u64,
        content_hash: String,
    ) -> Result<(), ArtifactGraphError> {
        if !self.enabled {
            return Ok(());
        }
        let mut inner = self.lock();
        let Some(record) = inner.records.get_mut(key) else {
            return Err(ArtifactGraphError::StaleCompletion {
                identity: key.identity.clone(),
            });
        };
        if record.generation != generation
            || matches!(
                record.state,
                ArtifactState::Cancelled | ArtifactState::Failed
            )
        {
            return Err(ArtifactGraphError::StaleCompletion {
                identity: key.identity.clone(),
            });
        }
        if record.state == ArtifactState::Ready
            && record.content_hash.as_deref() != Some(content_hash.as_str())
        {
            record.state = ArtifactState::Failed;
            record.reason = Some("two builders produced different content".to_string());
            record.active_builders = record.active_builders.saturating_sub(1);
            return Err(ArtifactGraphError::NonDeterministicOutput {
                identity: key.identity.clone(),
            });
        }
        record.state = ArtifactState::Ready;
        record.content_hash = Some(content_hash);
        record.reason = None;
        record.active_builders = record.active_builders.saturating_sub(1);
        Ok(())
    }

    fn fail(&self, key: &ArtifactKey, generation: u64, reason: String) {
        if !self.enabled {
            return;
        }
        let mut inner = self.lock();
        if let Some(record) = inner.records.get_mut(key)
            && record.generation == generation
        {
            record.active_builders = record.active_builders.saturating_sub(1);
            record.state = ArtifactState::Failed;
            record.content_hash = None;
            record.reason = Some(reason);
        }
    }

    fn cancel(&self, key: &ArtifactKey, generation: u64) {
        if !self.enabled {
            return;
        }
        let mut inner = self.lock();
        if let Some(record) = inner.records.get_mut(key)
            && record.generation == generation
        {
            record.active_builders = record.active_builders.saturating_sub(1);
            if record.active_builders == 0 && record.state == ArtifactState::Building {
                record.state = ArtifactState::Cancelled;
                record.reason = Some("artifact task ended without completion".to_string());
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, ArtifactGraphInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn artifact_record_bytes(record: &ArtifactRecord) -> u64 {
    let dependency_bytes = record
        .dependencies
        .iter()
        .map(|dependency| {
            dependency.key.namespace.len() as u64
                + dependency.key.identity.len() as u64
                + dependency
                    .content_hash
                    .as_deref()
                    .map(str::len)
                    .unwrap_or(0) as u64
        })
        .sum::<u64>();
    record.key.namespace.len() as u64
        + record.key.identity.len() as u64
        + record.content_hash.as_deref().map(str::len).unwrap_or(0) as u64
        + record.reason.as_deref().map(str::len).unwrap_or(0) as u64
        + dependency_bytes
}

fn protected_artifacts(inner: &ArtifactGraphInner) -> BTreeSet<ArtifactKey> {
    let mut protected = inner
        .records
        .iter()
        .filter(|(_, record)| record.state == ArtifactState::Building)
        .map(|(key, _)| key.clone())
        .collect::<BTreeSet<_>>();
    let mut queue = protected.iter().cloned().collect::<VecDeque<_>>();
    while let Some(key) = queue.pop_front() {
        let Some(record) = inner.records.get(&key) else {
            continue;
        };
        for dependency in &record.dependencies {
            if protected.insert(dependency.key.clone()) {
                queue.push_back(dependency.key.clone());
            }
        }
    }
    protected
}

/// Generation-scoped completion token. Dropping it cancels unfinished work.
pub struct ArtifactTask {
    graph: ArtifactTaskGraph,
    key: ArtifactKey,
    generation: u64,
    finished: bool,
}

impl ArtifactTask {
    pub fn complete(mut self, content_hash: impl Into<String>) -> Result<(), ArtifactGraphError> {
        let result = self
            .graph
            .complete(&self.key, self.generation, content_hash.into());
        self.finished = true;
        result
    }

    pub fn fail(mut self, reason: impl Into<String>) {
        self.graph.fail(&self.key, self.generation, reason.into());
        self.finished = true;
    }
}

impl Drop for ArtifactTask {
    fn drop(&mut self) {
        if !self.finished {
            self.graph.cancel(&self.key, self.generation);
        }
    }
}

fn replace_dependency_edges(
    inner: &mut ArtifactGraphInner,
    key: &ArtifactKey,
    dependencies: &BTreeSet<ArtifactDependency>,
) {
    let previous_dependencies = inner
        .records
        .get(key)
        .map(|record| record.dependencies.clone())
        .unwrap_or_default();
    for dependency in previous_dependencies {
        if let Some(dependents) = inner.dependents.get_mut(&dependency.key) {
            dependents.remove(key);
            if dependents.is_empty() {
                inner.dependents.remove(&dependency.key);
            }
        }
    }
    for dependency in dependencies {
        inner
            .dependents
            .entry(dependency.key.clone())
            .or_default()
            .insert(key.clone());
    }
}

fn rebuild_dependents(inner: &mut ArtifactGraphInner) {
    inner.dependents.clear();
    let edges = inner
        .records
        .iter()
        .flat_map(|(key, record)| {
            record
                .dependencies
                .iter()
                .map(move |dependency| (dependency.key.clone(), key.clone()))
        })
        .collect::<Vec<_>>();
    for (dependency, dependent) in edges {
        inner
            .dependents
            .entry(dependency)
            .or_default()
            .insert(dependent);
    }
}

fn load_manifest(path: &Path, namespace: &str) -> Option<BTreeMap<ArtifactKey, ArtifactRecord>> {
    let source = fs::read(path).ok()?;
    let manifest: ArtifactGraphManifest = serde_json::from_slice(&source).ok()?;
    if manifest.identity != ARTIFACT_GRAPH_IDENTITY || manifest.namespace != namespace {
        return None;
    }
    let mut records = BTreeMap::new();
    for record in manifest.records {
        if records.insert(record.key.clone(), record).is_some() {
            return None;
        }
    }
    Some(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: ArtifactKind, name: &str) -> ArtifactKey {
        ArtifactKey::from_inputs(kind, "test", [("name", name.as_bytes())])
    }

    #[test]
    fn semantic_keys_are_order_independent_and_input_complete() {
        let first = ArtifactKey::from_inputs(
            ArtifactKind::Transform,
            "config-a",
            [("source", b"one".as_slice()), ("target", b"es2022")],
        );
        let reordered = ArtifactKey::from_inputs(
            ArtifactKind::Transform,
            "config-a",
            [("target", b"es2022".as_slice()), ("source", b"one")],
        );
        let changed = ArtifactKey::from_inputs(
            ArtifactKind::Transform,
            "config-a",
            [("source", b"two".as_slice()), ("target", b"es2022")],
        );
        assert_eq!(first, reordered);
        assert_ne!(first, changed);
    }

    #[test]
    fn invalidation_reaches_only_transitive_dependents() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let source = key(ArtifactKind::Source, "source");
        let transform = key(ArtifactKind::Transform, "transform");
        let emit = key(ArtifactKind::Emit, "emit");
        let sibling = key(ArtifactKind::Emit, "sibling");
        graph
            .begin(source.clone(), BTreeSet::new())
            .unwrap()
            .complete("source-v1")
            .unwrap();
        graph
            .begin(
                transform.clone(),
                BTreeSet::from([ArtifactDependency::new(
                    source.clone(),
                    Some("source-v1".to_string()),
                )]),
            )
            .unwrap()
            .complete("transform-v1")
            .unwrap();
        graph
            .begin(
                emit.clone(),
                BTreeSet::from([ArtifactDependency::new(transform.clone(), None)]),
            )
            .unwrap()
            .complete("emit-v1")
            .unwrap();
        graph
            .begin(sibling.clone(), BTreeSet::new())
            .unwrap()
            .complete("sibling-v1")
            .unwrap();

        assert_eq!(
            graph.invalidate(&source, "source changed"),
            [source.clone(), transform.clone(), emit.clone()]
        );
        assert_eq!(
            graph.record(&source).unwrap().state,
            ArtifactState::Cancelled
        );
        assert_eq!(
            graph.record(&transform).unwrap().state,
            ArtifactState::Cancelled
        );
        assert_eq!(graph.record(&emit).unwrap().state, ArtifactState::Cancelled);
        assert_eq!(graph.record(&sibling).unwrap().state, ArtifactState::Ready);
    }

    #[test]
    fn unfinished_tasks_cancel_and_stale_completions_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let artifact = key(ArtifactKind::Resolve, "route");
        let task = graph.begin(artifact.clone(), BTreeSet::new()).unwrap();
        graph.invalidate(&artifact, "new input");
        assert_eq!(
            task.complete("old-output"),
            Err(ArtifactGraphError::StaleCompletion {
                identity: artifact.identity.clone()
            })
        );

        let abandoned = key(ArtifactKind::Analyze, "abandoned");
        drop(graph.begin(abandoned.clone(), BTreeSet::new()).unwrap());
        assert_eq!(
            graph.record(&abandoned).unwrap().state,
            ArtifactState::Cancelled
        );
    }

    #[test]
    fn concurrent_builders_must_produce_identical_content() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let artifact = key(ArtifactKind::ChunkPlan, "route");
        let first = graph.begin(artifact.clone(), BTreeSet::new()).unwrap();
        let second = graph.begin(artifact.clone(), BTreeSet::new()).unwrap();
        first.complete("same").unwrap();
        assert_eq!(
            second.complete("different"),
            Err(ArtifactGraphError::NonDeterministicOutput {
                identity: artifact.identity
            })
        );
    }

    #[test]
    fn ready_records_and_edges_survive_atomic_persistence() {
        let temp = tempfile::tempdir().unwrap();
        let source = key(ArtifactKind::Source, "page");
        let emit = key(ArtifactKind::Emit, "page");
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        graph
            .begin(source.clone(), BTreeSet::new())
            .unwrap()
            .complete("source")
            .unwrap();
        graph
            .begin(
                emit.clone(),
                BTreeSet::from([ArtifactDependency::new(source.clone(), None)]),
            )
            .unwrap()
            .complete("output")
            .unwrap();
        graph.save().unwrap();

        let loaded = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        assert_eq!(loaded.record(&emit).unwrap().state, ArtifactState::Ready);
        assert_eq!(loaded.stats().dependency_edges, 1);
        assert_eq!(
            loaded.invalidate(&source, "edit"),
            [source.clone(), emit.clone()]
        );
        assert!(!temp.path().join("artifact-graph.json.tmp").exists());
    }

    #[test]
    fn namespace_mismatch_cannot_reuse_or_start_an_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "config-a", true);
        let wrong = ArtifactKey::from_inputs(
            ArtifactKind::Resolve,
            "config-b",
            [("entry", b"app/page.tsx".as_slice())],
        );
        assert!(matches!(
            graph.begin(wrong, BTreeSet::new()),
            Err(ArtifactGraphError::NamespaceMismatch { .. })
        ));
    }

    #[test]
    fn publish_distinguishes_misses_from_identical_hits() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let artifact = key(ArtifactKind::Emit, "route");

        assert!(
            !graph
                .publish(artifact.clone(), BTreeSet::new(), "output")
                .unwrap()
        );
        assert!(graph.publish(artifact, BTreeSet::new(), "output").unwrap());
        assert_eq!(graph.stats().misses, 1);
        assert_eq!(graph.stats().hits, 1);
    }

    #[test]
    fn corrupt_persistence_is_a_safe_cache_miss() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("artifact-graph.json"), b"not-json").unwrap();

        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);

        assert_eq!(graph.stats().records, 0);
        let artifact = key(ArtifactKind::Manifest, "route");
        assert!(!graph.publish(artifact, BTreeSet::new(), "rebuilt").unwrap());
    }

    #[test]
    fn bypass_keeps_builds_correct_without_persistence() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", false);
        let artifact = key(ArtifactKind::Transform, "module");

        assert!(!graph.publish(artifact, BTreeSet::new(), "output").unwrap());
        graph.save().unwrap();
        assert_eq!(graph.stats(), ArtifactGraphStats::default());
        assert!(!temp.path().join("artifact-graph.json").exists());
    }

    #[test]
    fn pressure_evicts_unowned_leaves_but_pins_active_dependency_closures() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let source = key(ArtifactKind::Source, "active-source");
        let active = key(ArtifactKind::Transform, "active-transform");
        let unrelated = key(ArtifactKind::Emit, "unrelated");
        graph
            .publish(source.clone(), BTreeSet::new(), "source")
            .unwrap();
        graph
            .publish(unrelated.clone(), BTreeSet::new(), "output")
            .unwrap();
        let task = graph
            .begin(
                active.clone(),
                BTreeSet::from([ArtifactDependency::new(source.clone(), None)]),
            )
            .unwrap();

        assert_eq!(graph.evict_to_bytes(0), 1);
        assert!(graph.record(&unrelated).is_none());
        assert!(graph.record(&source).is_some());
        assert_eq!(
            graph.record(&active).unwrap().state,
            ArtifactState::Building
        );

        drop(task);
        assert_eq!(graph.evict_to_bytes(0), 2);
        assert_eq!(graph.stats().records, 0);
        assert_eq!(graph.stats().evictions, 3);
    }
}
