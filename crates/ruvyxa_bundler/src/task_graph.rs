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

/// Identity of the compiler whose artifacts this graph describes.
///
/// Derived from the crate version, exactly as `cache::COMPILER_VERSION` is, and
/// for the same reason — not a hand-maintained counter, which is only correct
/// while somebody remembers it. The namespace beside it is derived from the
/// *project*: config, lockfile, build hooks. Nothing in it moves when the
/// compiler does.
///
/// That gap was load-bearing. A `Transform` record's content hash is the
/// compiler's output while its key is not, and `publish` fails closed when a
/// key it already holds `Ready` produces a different hash. A graph filled by
/// one build of this crate and read by another therefore turned a changed
/// transform into `NonDeterministicOutput` — a hard build failure that
/// survived across runs until the cache directory was deleted, naming an
/// artifact identity and nothing actionable. Comparing the version here makes
/// the same situation a cold start.
const ARTIFACT_GRAPH_IDENTITY: &str = concat!("ruvyxa_artifact_graph:", env!("CARGO_PKG_VERSION"));

/// How many build epochs a record survives without being touched.
///
/// Three keeps the cache warm across a branch switch and back — the shape that
/// costs the most to rebuild — while bounding the manifest at roughly the work
/// of the last three builds instead of every build ever run in this directory.
const RETENTION_EPOCHS: u64 = 3;

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
    /// Build epoch in which this record was last begun, published, or hit.
    ///
    /// Retention is decided from this and nothing else: see
    /// [`ArtifactTaskGraph::save`]. A record persisted before epochs were
    /// recorded reads as `0`, which is older than any live epoch and therefore
    /// ages out on the third build after the upgrade — a rebuild, never a wrong
    /// answer.
    #[serde(default)]
    pub last_touched_epoch: u64,
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
    /// This process's build epoch: one past whatever the loaded manifest wrote.
    epoch: u64,
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
    /// Epoch the writing build ran under. Absent in a manifest written before
    /// retention existed, which reads as `0` and makes the next build epoch 1.
    #[serde(default)]
    epoch: u64,
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
        let (records, previous_epoch) = enabled
            .then(|| load_manifest(&manifest_path, namespace))
            .flatten()
            .unwrap_or_default();
        let mut inner = ArtifactGraphInner {
            records,
            dependents: BTreeMap::new(),
            // One load is one build, so the epoch advances here and nowhere
            // else. Every save in this process stamps the same number.
            epoch: previous_epoch.saturating_add(1),
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
            let epoch = inner.epoch;
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
                    // A hit is a touch. Answering from the graph is the whole
                    // point of the record, so it must renew retention exactly
                    // as building it would — otherwise the artifacts a build
                    // reuses most are the first ones aged out.
                    if let Some(existing) = inner.records.get_mut(&key) {
                        existing.last_touched_epoch = epoch;
                    }
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
        let epoch = inner.epoch;
        // Join whenever work is still in flight, not only while the state still
        // reads `Building`.
        //
        // `publish` is `begin` then `complete` with the lock released between
        // them, so several route builds that share one module overlap here. The
        // first of them to finish flips the record to `Ready` while its
        // siblings are still running; deciding from `state` alone then opened a
        // new generation for the next arrival, and every sibling's `complete`
        // was rejected as `StaleCompletion` — a build failure with no bad input
        // behind it, reproduced by
        // `a_sibling_that_joined_still_completes_after_the_first_one_finishes`.
        //
        // `Failed` and `Cancelled` are deliberately not joinable: completing
        // into either is refused anyway, so a new generation is the only useful
        // answer there.
        if let Some(existing) = inner.records.get_mut(&key)
            && existing.active_builders > 0
            && matches!(
                existing.state,
                ArtifactState::Building | ArtifactState::Ready
            )
        {
            if existing.dependencies != dependencies {
                return Err(ArtifactGraphError::DependencyMismatch {
                    identity: key.identity,
                });
            }
            existing.active_builders += 1;
            existing.last_touched_epoch = epoch;
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
                last_touched_epoch: epoch,
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

    /// Bytes eviction may reclaim right now.
    ///
    /// The budget controller consults this once per route build and, under
    /// pressure, several times more. Reading it through [`Self::stats`] also
    /// recomputed dependency-edge totals, state counters, and a second residency
    /// pass that no caller of this number reads.
    pub fn evictable_bytes(&self) -> u64 {
        let inner = self.lock();
        let protected = protected_artifacts(&inner);
        inner
            .records
            .iter()
            .filter(|(key, _)| !protected.contains(*key))
            .map(|(_, record)| artifact_record_bytes(record))
            .sum()
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

    /// Persist the graph, keeping only what the next few builds can still use.
    ///
    /// The graph has invalidation and eviction but no *retention*, and neither
    /// stands in for it: invalidation cancels a record without removing it, and
    /// eviction only runs under a soft memory limit measured against a byte
    /// estimate several times smaller than this JSON encoding. Every edit gives
    /// a `Transform` a new semantic key, so writing the whole map grew the file
    /// by one record per changed module per build, forever, and both ends of
    /// every later build paid to parse and rewrite it.
    ///
    /// A record survives if it was touched — begun, published, or hit — within
    /// the last [`RETENTION_EPOCHS`] build epochs, or if [`is_evictable`] says
    /// something still holds it: work in flight, a joined builder, or another
    /// record's dependency edge. The dependency closure of everything retained
    /// is then retained too, so pruning can never leave a dangling edge behind.
    /// Dropping a record the next build would have hit costs that build a
    /// rebuild, which is the same trade `evict_to_bytes` already documents.
    ///
    /// In-memory state is untouched: this decides what reaches disk, not what
    /// this process may still answer from.
    pub fn save(&self) -> std::io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let (records, epoch) = {
            let inner = self.lock();
            let epoch = inner.epoch;
            let mut retained = inner
                .records
                .iter()
                .filter(|(key, record)| {
                    epoch.saturating_sub(record.last_touched_epoch) < RETENTION_EPOCHS
                        || !is_evictable(record, &inner.dependents, key)
                })
                .map(|(key, _)| key.clone())
                .collect::<BTreeSet<_>>();
            let mut queue = retained.iter().cloned().collect::<VecDeque<_>>();
            while let Some(key) = queue.pop_front() {
                let Some(record) = inner.records.get(&key) else {
                    continue;
                };
                for dependency in &record.dependencies {
                    if inner.records.contains_key(&dependency.key)
                        && retained.insert(dependency.key.clone())
                    {
                        queue.push_back(dependency.key.clone());
                    }
                }
            }
            let records = inner
                .records
                .iter()
                .filter(|(key, _)| retained.contains(*key))
                .map(|(_, record)| {
                    let mut record = record.clone();
                    record.active_builders = 0;
                    if record.state == ArtifactState::Building {
                        record.state = ArtifactState::Cancelled;
                        record.reason =
                            Some("artifact was active when graph was saved".to_string());
                    }
                    record
                })
                .collect();
            (records, epoch)
        };
        let manifest = ArtifactGraphManifest {
            identity: ARTIFACT_GRAPH_IDENTITY.to_string(),
            namespace: self.namespace.to_string(),
            epoch,
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
    ///
    /// `target_bytes` is compared against the same quantity the budget
    /// controller accounts for — [`ArtifactGraphStats::evictable_bytes`], which
    /// excludes pinned records. Measuring every record here instead would count
    /// bytes eviction is forbidden to reclaim, so a build holding a large pinned
    /// closure would evict healthy `Ready` artifacts to make up the difference
    /// and turn them into rebuilds that the budget never actually required.
    pub fn evict_to_bytes(&self, target_bytes: u64) -> u64 {
        if !self.enabled {
            return 0;
        }
        let mut inner = self.lock();
        let protected = protected_artifacts(&inner);
        let mut resident = inner
            .records
            .iter()
            .filter(|(key, _)| !protected.contains(*key))
            .map(|(_, record)| artifact_record_bytes(record))
            .sum::<u64>();
        let mut evicted = 0_u64;

        // Eligible records are kept in a `(priority, key)` set rather than
        // rediscovered by scanning every record on each pass. Evicting a chain
        // of N artifacts used to cost O(N²) — 8,000 records took 4.4s in release
        // mode, and that work lands precisely when the process is already short
        // on memory. Only the dependencies of an evicted record can newly become
        // eligible, so the set is repaired incrementally instead.
        //
        // Ordering matches the previous `min_by_key` over a `BTreeMap`: the
        // lowest priority first, ties broken by artifact key.
        let mut eligible = inner
            .records
            .iter()
            .filter(|(key, record)| is_evictable(record, &inner.dependents, key))
            .map(|(key, record)| (eviction_priority(record.state), key.clone()))
            .collect::<BTreeSet<_>>();

        while resident > target_bytes {
            let Some(entry) = eligible.iter().next().cloned() else {
                break;
            };
            eligible.remove(&entry);
            let (_, key) = entry;
            let Some(record) = inner.records.remove(&key) else {
                break;
            };
            resident = resident.saturating_sub(artifact_record_bytes(&record));
            inner.dependents.remove(&key);
            for dependency in record.dependencies {
                let Some(dependents) = inner.dependents.get_mut(&dependency.key) else {
                    continue;
                };
                dependents.remove(&key);
                if !dependents.is_empty() {
                    continue;
                }
                inner.dependents.remove(&dependency.key);
                // Losing its last dependent can make this record evictable.
                if let Some(freed) = inner.records.get(&dependency.key)
                    && is_evictable(freed, &inner.dependents, &dependency.key)
                {
                    eligible.insert((eviction_priority(freed.state), dependency.key.clone()));
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

/// Whether a record may be evicted right now.
///
/// A `Building` record owns work in flight, an `active_builders` count outlives
/// a joined build, and a record something else depends on is pinned by that
/// edge — which is what keeps the dependency closure of an in-flight build
/// resident without tracking it separately.
fn is_evictable(
    record: &ArtifactRecord,
    dependents: &BTreeMap<ArtifactKey, BTreeSet<ArtifactKey>>,
    key: &ArtifactKey,
) -> bool {
    record.state != ArtifactState::Building
        && record.active_builders == 0
        && dependents.get(key).is_none_or(BTreeSet::is_empty)
}

/// Lower evicts first: work already discarded costs nothing to lose again.
fn eviction_priority(state: ArtifactState) -> u8 {
    match state {
        ArtifactState::Failed | ArtifactState::Cancelled => 0,
        ArtifactState::Ready => 1,
        ArtifactState::Building => 2,
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

/// Load the persisted records and the epoch the writing build ran under.
fn load_manifest(
    path: &Path,
    namespace: &str,
) -> Option<(BTreeMap<ArtifactKey, ArtifactRecord>, u64)> {
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
    Some((records, manifest.epoch))
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

    /// The graph identity must follow the compiler that filled it.
    ///
    /// A `Transform` record's content hash is the compiler's output while its
    /// key is not, so a graph written by a different build of this crate can
    /// hold a `Ready` record whose hash the current transform will never
    /// reproduce — and `publish` fails closed on that, giving a sticky hard
    /// build failure that survives across runs. An identity that moves with the
    /// crate version turns that into a clean cold start.
    #[test]
    fn a_graph_from_another_compiler_version_loads_as_zero_records() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let artifact = key(ArtifactKind::Transform, "module");
        graph
            .publish(artifact.clone(), BTreeSet::new(), "output-v1")
            .unwrap();
        graph.save().unwrap();
        assert_eq!(
            ArtifactTaskGraph::at_dir(temp.path(), "test", true)
                .stats()
                .records,
            1
        );

        let manifest_path = temp.path().join("artifact-graph.json");
        let json = fs::read_to_string(&manifest_path).unwrap();
        let doctored = json.replace(ARTIFACT_GRAPH_IDENTITY, "ruvyxa_artifact_graph:0.0.0-other");
        assert_ne!(doctored, json, "the identity must be part of the manifest");
        fs::write(&manifest_path, doctored).unwrap();

        let reloaded = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        assert_eq!(
            reloaded.stats().records,
            0,
            "a graph from another compiler must be a cold start, not a poisoned graph"
        );
        assert!(
            !reloaded
                .publish(artifact, BTreeSet::new(), "output-v2")
                .unwrap(),
            "a changed transform must republish rather than fail closed"
        );
    }

    /// A superseded generation must not be persisted forever.
    ///
    /// Every edit gives a `Transform` a new semantic key, so the record the
    /// previous build published is never hit again — and nothing dropped it.
    /// `save` wrote every record in memory, so `artifact-graph.json` grew by
    /// one record per changed module per build, was parsed in full at the start
    /// of the next build and re-serialised at the end of it, and `ruvyxa clean`
    /// was the only reclaim. Eviction could not stand in: it is driven by a
    /// soft memory limit against a byte estimate several times smaller than the
    /// JSON encoding, so the file reached hundreds of megabytes before the
    /// accounting reported any pressure at all.
    #[test]
    fn a_superseded_generation_is_not_kept_forever() {
        let temp = tempfile::tempdir().unwrap();
        const BUILDS: u64 = 10;
        // Ten builds, each editing the module, so each publishes a new key.
        for build in 0..BUILDS {
            let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
            graph
                .publish(
                    key(ArtifactKind::Transform, &format!("module-v{build}")),
                    BTreeSet::new(),
                    format!("output-v{build}"),
                )
                .unwrap();
            graph.save().unwrap();
        }

        let reloaded = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let records = reloaded.stats().records;
        assert!(
            records < BUILDS as usize,
            "the persisted graph must not grow by one record per build: {records}"
        );
        assert!(
            records <= RETENTION_EPOCHS as usize,
            "the persisted graph must be bounded by the retention window: {records}"
        );
        // The window is warm, not empty: the most recent build's record is
        // still there, so a branch switch and back is still a hit.
        assert!(
            reloaded
                .record(&key(ArtifactKind::Transform, "module-v9"))
                .is_some(),
            "the newest record must survive its own build"
        );
    }

    /// A hit is a touch. A module nobody edits is republished with the same key
    /// every build, which `publish` answers from the graph without opening a
    /// generation — if that did not renew the record, the artifacts a build
    /// reuses most would be the first ones dropped.
    #[test]
    fn a_repeatedly_hit_record_is_never_aged_out() {
        let temp = tempfile::tempdir().unwrap();
        let stable = key(ArtifactKind::Transform, "unchanged");
        for build in 0..10 {
            let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
            graph
                .publish(stable.clone(), BTreeSet::new(), "stable-output")
                .unwrap();
            graph
                .publish(
                    key(ArtifactKind::Emit, &format!("route-v{build}")),
                    BTreeSet::new(),
                    format!("emit-v{build}"),
                )
                .unwrap();
            graph.save().unwrap();
        }

        let reloaded = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        assert!(
            reloaded.record(&stable).is_some(),
            "a record hit by every build must not be aged out"
        );
        assert!(
            reloaded
                .publish(stable, BTreeSet::new(), "stable-output")
                .unwrap(),
            "and it must still answer as a hit"
        );
    }

    /// Retention must never break the graph it prunes. A record something else
    /// still depends on stays, however long ago it was last touched — the same
    /// pin `is_evictable` already applies under memory pressure.
    #[test]
    fn retention_keeps_a_record_a_surviving_dependent_still_needs() {
        let temp = tempfile::tempdir().unwrap();
        let source = key(ArtifactKind::Source, "shared");

        let first = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        first
            .publish(source.clone(), BTreeSet::new(), "source-v1")
            .unwrap();
        first.save().unwrap();

        // Many later builds, none of which touch `source` directly; each emits
        // a fresh route that depends on it.
        for build in 0..8 {
            let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
            graph
                .publish(
                    key(ArtifactKind::Emit, &format!("route-v{build}")),
                    BTreeSet::from([ArtifactDependency::new(
                        source.clone(),
                        Some("source-v1".to_string()),
                    )]),
                    format!("emit-v{build}"),
                )
                .unwrap();
            graph.save().unwrap();
        }

        let reloaded = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        assert!(
            reloaded.record(&source).is_some(),
            "a dependency of a retained record must be retained with it"
        );
        let records = reloaded.lock().records.clone();
        for (key, record) in &records {
            for dependency in &record.dependencies {
                assert!(
                    records.contains_key(&dependency.key),
                    "record {} kept a dangling edge to {}",
                    key.identity,
                    dependency.key.identity
                );
            }
        }
    }

    /// The identity is derived, never stamped: it names the crate version and
    /// carries no hand-maintained `vN` counter.
    #[test]
    fn the_graph_identity_carries_no_version_counter() {
        assert!(
            ARTIFACT_GRAPH_IDENTITY.contains(env!("CARGO_PKG_VERSION")),
            "the identity must follow the compiler: {ARTIFACT_GRAPH_IDENTITY}"
        );
        for segment in ARTIFACT_GRAPH_IDENTITY.split(':') {
            assert!(
                !segment.strip_prefix('v').is_some_and(
                    |rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                ),
                "a `{segment}` counter would have to be maintained by hand"
            );
        }
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

    /// On-demand benchmark for the two graph operations a build calls per route.
    ///
    /// Not a CI gate: a wall-clock assertion on a shared runner is a flake, not a
    /// guard. It exists so a change to eviction or residency accounting can be
    /// measured the same way twice. Run it with:
    ///
    /// ```text
    /// cargo test --release -p ruvyxa_bundler --lib measure_graph_hot_paths -- --ignored --nocapture
    /// ```
    ///
    /// Eviction was O(N²) until the eligible-candidate set replaced a full scan
    /// per eviction. Measured on one machine, evicting every record:
    ///
    /// | records | before   | after   |
    /// | ------- | -------- | ------- |
    /// | 500     | 8.75ms   | 0.83ms  |
    /// | 2000    | 194.75ms | 4.38ms  |
    /// | 8000    | 4.41s    | 20.59ms |
    ///
    /// Absolute numbers are machine-specific; the shape is the point. If the
    /// last column starts growing quadratically again, the candidate set has
    /// stopped being repaired incrementally.
    #[test]
    #[ignore]
    fn measure_graph_hot_paths() {
        for size in [500_usize, 2_000, 8_000] {
            let temp = tempfile::tempdir().unwrap();
            let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
            // A chain of dependencies mirrors a real module graph: each artifact
            // depends on the previous one, so `dependents` is populated.
            let mut previous: Option<ArtifactKey> = None;
            for index in 0..size {
                let current = key(ArtifactKind::Transform, &format!("module-{index}"));
                let dependencies = previous
                    .take()
                    .map(|parent| BTreeSet::from([ArtifactDependency::new(parent, None)]))
                    .unwrap_or_default();
                graph
                    .publish(current.clone(), dependencies, format!("hash-{index}"))
                    .unwrap();
                previous = Some(current);
            }

            let started = std::time::Instant::now();
            for _ in 0..100 {
                std::hint::black_box(graph.stats());
            }
            let stats_elapsed = started.elapsed();

            let started = std::time::Instant::now();
            for _ in 0..100 {
                std::hint::black_box(graph.evictable_bytes());
            }
            let evictable_elapsed = started.elapsed();

            let started = std::time::Instant::now();
            let evicted = graph.evict_to_bytes(0);
            let evict_elapsed = started.elapsed();

            eprintln!(
                "size={size:>5}  stats={:>9.2?}/call  evictable_bytes={:>9.2?}/call  evict_to_bytes(0)={evict_elapsed:>10.2?} for {evicted} records",
                stats_elapsed / 100,
                evictable_elapsed / 100,
            );
        }
    }

    /// Eviction must budget against the same bytes the controller accounts for.
    ///
    /// `enforce_cache_budget` measures the graph as `evictable_bytes`, which
    /// excludes the pinned closure of an in-flight build, and then asks this
    /// method to reach a target derived from that number. Measuring every record
    /// here instead counted pinned bytes that eviction is forbidden to reclaim,
    /// so a build holding a large pinned closure made up the difference by
    /// discarding healthy `Ready` artifacts — rebuilds the budget never asked
    /// for. Reaching a target already satisfied must therefore evict nothing.
    #[test]
    fn eviction_targets_evictable_bytes_and_ignores_pinned_closure_size() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let pinned_source = key(ArtifactKind::Source, "pinned-source");
        let building = key(ArtifactKind::Transform, "in-flight");
        let spare = key(ArtifactKind::Emit, "spare");
        graph
            .publish(pinned_source.clone(), BTreeSet::new(), "source")
            .unwrap();
        graph
            .publish(spare.clone(), BTreeSet::new(), "spare")
            .unwrap();
        let task = graph
            .begin(
                building.clone(),
                BTreeSet::from([ArtifactDependency::new(pinned_source.clone(), None)]),
            )
            .unwrap();

        // The controller's view of the graph excludes the pinned closure.
        let evictable = graph.stats().evictable_bytes;
        assert!(evictable > 0, "the spare artifact must be evictable");
        assert!(
            evictable < graph.stats().resident_bytes,
            "the pinned closure must not count as evictable"
        );

        // Asking for a target the graph already meets must cost nothing.
        assert_eq!(
            graph.evict_to_bytes(evictable),
            0,
            "an already-satisfied budget must not discard a healthy artifact"
        );
        assert!(graph.record(&spare).is_some());
        assert!(graph.record(&pinned_source).is_some());

        drop(task);
    }
}

#[cfg(test)]
mod concurrent_publish_tests {
    use super::*;

    fn transform_key(graph: &ArtifactTaskGraph) -> ArtifactKey {
        graph.key(
            ArtifactKind::Transform,
            [("module", b"shared.tsx".as_slice())],
        )
    }

    /// A builder that joined an in-flight artifact must still be allowed to
    /// finish after a sibling finishes first.
    ///
    /// `publish` is `begin` then `complete` with the lock released between
    /// them, so several route builds that share one module interleave exactly
    /// like this. `begin` used to decide join-or-restart from `state` alone:
    /// the first completion flips the record to `Ready` while its siblings are
    /// still mid-flight, the next `begin` therefore started generation N+1, and
    /// every sibling was then told its generation had been invalidated — a
    /// build failure with no bad input behind it, and one that only shows up
    /// when three builds of one module overlap.
    #[test]
    fn a_sibling_that_joined_still_completes_after_the_first_one_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let key = transform_key(&graph);

        let first = graph.begin(key.clone(), BTreeSet::new()).unwrap();
        let joined = graph.begin(key.clone(), BTreeSet::new()).unwrap();
        first.complete("shared-output").unwrap();

        // A third build arrives while `joined` is still running. It must join
        // the same generation rather than open a new one.
        let third = graph.begin(key.clone(), BTreeSet::new()).unwrap();
        third.complete("shared-output").unwrap();

        joined
            .complete("shared-output")
            .expect("a joined builder must not be rejected as stale");
    }

    /// The same shape under real threads, as the build hits it.
    #[test]
    fn concurrent_publishers_of_one_artifact_all_succeed() {
        for round in 0..64 {
            let temp = tempfile::tempdir().unwrap();
            let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
            let key = graph.key(
                ArtifactKind::Transform,
                [("round", round.to_string().as_bytes())],
            );
            let errors = std::sync::Arc::new(Mutex::new(Vec::new()));
            std::thread::scope(|scope| {
                for _ in 0..8 {
                    let graph = graph.clone();
                    let key = key.clone();
                    let errors = std::sync::Arc::clone(&errors);
                    scope.spawn(move || {
                        if let Err(error) = graph.publish(key, BTreeSet::new(), "same-content") {
                            errors.lock().unwrap().push(error);
                        }
                    });
                }
            });
            let errors = errors.lock().unwrap();
            assert!(
                errors.is_empty(),
                "round {round}: publishing one artifact from several threads must not fail: {errors:?}"
            );
        }
    }

    /// Restarting is still what happens when nothing is in flight — otherwise
    /// an artifact could never be rebuilt after an invalidation.
    #[test]
    fn a_settled_artifact_still_opens_a_new_generation() {
        let temp = tempfile::tempdir().unwrap();
        let graph = ArtifactTaskGraph::at_dir(temp.path(), "test", true);
        let key = transform_key(&graph);

        graph
            .begin(key.clone(), BTreeSet::new())
            .unwrap()
            .complete("first")
            .unwrap();
        let settled = graph.record(&key).unwrap().generation;

        graph.invalidate(&key, "source changed");
        graph
            .begin(key.clone(), BTreeSet::new())
            .unwrap()
            .complete("second")
            .unwrap();

        let record = graph.record(&key).unwrap();
        assert!(
            record.generation > settled,
            "a rebuild after invalidation must open a new generation"
        );
        assert_eq!(record.content_hash.as_deref(), Some("second"));
    }
}
