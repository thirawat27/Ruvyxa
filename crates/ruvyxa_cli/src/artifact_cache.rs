//! Content-addressed build artifact cache.
//!
//! Every cached artifact — a client bundle, a route plan, a shared chunk, a
//! prerendered page — is keyed by a hash of everything that can change it. The
//! rule this module exists to enforce is that the key covers *all* such inputs:
//! a cache hit must be indistinguishable from doing the work again. A key that
//! omits an input does not make builds slower, it makes them wrong, and the
//! failure surfaces as stale output nobody can reproduce.
//!
//! That is why the prerender key mixes in the runtime script hash and a
//! filtered view of `process.env` ([`stable_process_env`]): both change what
//! the renderer produces. Volatile keys are excluded on purpose, since a key
//! that changes every run caches nothing.
//!
//! Reads degrade to a miss on any error. A corrupt or unreadable cache entry
//! costs time; trusting one costs correctness.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ruvyxa_dev_server::find_runtime_script;

use crate::*;

// Every local module imported by the persistent render worker can change
// prerendered output or worker lifecycle. Keep names visible here so the npm
// runtime contract test can prove package inclusion and cache coverage together.
const WORKER_RUNTIME_FILES: &[&str] = &[
    "worker-pool.mjs",
    "worker-admission.mjs",
    "cache-budget.mjs",
    "glob.mjs",
    // glob.mjs delegates every source scan here, so a scanner change alters
    // what expansion emits and must invalidate prerendered output with it.
    "scanner.mjs",
    "request-context.mjs",
    // The worker builds an action's realtime event from the same validated
    // rule the deployed handler uses, so a change to those channel/size limits
    // reaches worker output and must invalidate prerendered artifacts too.
    "action-runtime.mjs",
    // `action-runtime.mjs` delegates its cross-site checks here. Extracting
    // them out of a hashed file would otherwise have taken them out of cache
    // identity: the rule could change while every hash above stayed equal.
    "origin-policy.mjs",
    // Which exported function answers a method, and what a 405 has to say. A
    // route that starts answering HEAD renders differently from one that
    // refused it, so prerendered output built against the old rule is stale.
    "api-methods.mjs",
    "compiler.mjs",
    // The identity a `'use client'` module carries into the server-components
    // graph. A change to it changes which reference a rendered payload names,
    // so prerendered output built against the old one is stale.
    "client-references.mjs",
    // Which file a bare specifier names. `compiler.mjs` delegates that whole
    // decision here, so a change to a condition list changes which modules a
    // prerendered page was built from — the definition of stale output.
    "package-exports.mjs",
    // The ordering rule the worker's cache keys and fingerprints sort by. A
    // change here reorders key material, so it belongs in cache identity.
    "order.mjs",
    "paths.mjs",
    "entry-templates.mjs",
    // Which interceptions a route composes. A change here changes the client
    // entry the worker emits, so prerendered output built from it is stale.
    "route-intercepts.mjs",
    // The two halves of a server-components render: the pipeline that turns a
    // route into HTML plus a payload, and the browser-side registry both it and
    // the client bundle resolve reference ids through. A change to either
    // changes what a pre-rendered page ships.
    "server-components.mjs",
    "rsc-client-runtime.mjs",
    "rsc-client-install.mjs",
    "flight.mjs",
    "react-compiler.mjs",
    // The project's own `build.onTransform` hooks, which `compiler.mjs` runs
    // over every module it reads. A plugin that rewrites source changes what a
    // pre-rendered page was built from, and the registry that dispatches those
    // hooks lives here — so a change to the dispatch changes rendered output
    // exactly like a change to the compiler does.
    "plugin-http.mjs",
    // Reached through `plugin-http.mjs`, which scopes every HTTP hook by the
    // path this module canonicalises — there is one answer to "what is this
    // request's path" and both the router and the plugin stage read it here.
    // Nothing the worker runs asks it anything, so this is the list being wider
    // than the closure needs, which `the_worker_runtime_list_covers_everything_the_worker_imports`
    // permits and a narrower list would not.
    "route-match.mjs",
];

pub(crate) fn content_hash(input: &str) -> String {
    content_hash_bytes(input.as_bytes())
}

pub(crate) fn content_hash_bytes(input: &[u8]) -> String {
    blake3::hash(input).to_hex().to_string()
}

/// The toolchain itself is a cache input, and the one users cannot see.
///
/// Every key below mixes this in so upgrading Ruvyxa invalidates artifacts the
/// new compiler, resolver, or linker would emit differently. Without it a fixed
/// bundler still served the broken chunk it emitted before the upgrade — the
/// project's own inputs had not changed, so every key still matched — and the
/// only cure was a manual `ruvyxa clean` nobody knew to run.
/// A cached shape carries no version counter of its own. Compatibility across
/// releases is this key's job, and compatibility across an edit *within* a
/// release belongs in the entry format: a field added later is `Option`, and a
/// field whose meaning changes is renamed, so an entry that cannot answer the
/// new question fails to deserialize instead of being trusted. A hand-stamped
/// `version: 3` said the same thing only while somebody remembered to raise
/// it — see `MANIFEST_VERSION` in `ruvyxa_bundler::incremental` for the
/// decision this follows.
fn versioned_key(parts: &str) -> String {
    content_hash(&format!("{}\0{parts}", env!("CARGO_PKG_VERSION")))
}

pub(crate) fn client_artifact_cache_file(
    cache_dir: &Path,
    route_path: &str,
    variant: &str,
) -> PathBuf {
    let key = versioned_key(&format!("{route_path}\0{variant}"));
    cache_dir.join("client-routes").join(format!("{key}.json"))
}

pub(crate) fn client_plan_cache_file(cache_dir: &Path, route_path: &str, variant: &str) -> PathBuf {
    let key = versioned_key(&format!("{route_path}\0{variant}"));
    cache_dir
        .join("client-route-plans")
        .join(format!("{key}.json"))
}

/// Where the shared chunk built from exactly these modules, in exactly this
/// order, is cached.
///
/// Order belongs in the key because it is in the output: the registry runs its
/// modules in the sequence it was handed, so two orders are two different
/// chunks and must not answer to one key.
pub(crate) fn shared_route_artifact_cache_file(
    cache_dir: &Path,
    module_paths: &[PathBuf],
    variant: &str,
) -> PathBuf {
    let mut key_source = String::from(variant);
    for path in module_paths {
        key_source.push('\0');
        key_source.push_str(&path.to_string_lossy());
    }
    cache_dir
        .join("shared-route-artifacts")
        .join(format!("{}.json", versioned_key(&key_source)))
}

pub(crate) fn prerender_artifact_cache_file(cache_dir: &Path, job: &PrerenderJob) -> PathBuf {
    let kind = match &job.kind {
        PrerenderJobKind::Csr => "csr",
        PrerenderJobKind::Render { mode, .. } => mode,
    };
    let key = serde_json::json!({
        "routePath": job.route_path,
        "renderPath": job.render_path,
        "params": job.params,
        "strategy": format!("{:?}", job.strategy),
        "revalidate": job.revalidate,
        "kind": kind,
    });
    cache_dir
        .join("prerender-routes")
        .join(format!("{}.json", versioned_key(&key.to_string())))
}

pub(crate) fn prerender_context_hash(
    root: &Path,
    head: &PrerenderHead,
    client_assets: &BTreeMap<String, PrerenderClientAssets>,
    build: &BuildConfigOptions,
    project_env: &BTreeMap<String, String>,
) -> String {
    let process_env = stable_process_env();
    let context = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "styles": content_hash(&head.styles),
        // In the key because it is in the output: these links are written into
        // every pre-rendered document, so publishing or removing the file one
        // names changes what a cached page would serve.
        "assetLinks": content_hash(&head.asset_links),
        // Same reason: a plugin's head entries are written into every baked
        // page, so adding, removing, or editing one has to invalidate the
        // pages that were baked without it.
        "pluginHead": content_hash(&head.plugin_head),
        "clientAssets": client_assets,
        "jsx": build.jsx_runtime.as_deref().unwrap_or("automatic"),
        // `build.target` decides what the transform emits, so a change to it
        // has to invalidate prerendered output. It was deliberately absent
        // while the key reached no transform and keying on it only forced a
        // rebuild that reproduced byte-identical output.
        "esTarget": build.es_target,
        "workerRuntime": runtime_script_hashes(root, WORKER_RUNTIME_FILES),
        "projectEnv": project_env,
        "processEnv": process_env,
    });
    content_hash(&context.to_string())
}

pub(crate) fn stable_process_env() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(key, _)| is_stable_process_env_key(key))
        .collect()
}

pub(crate) fn is_stable_process_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    !matches!(
        upper.as_str(),
        "PATH" | "PWD" | "OLDPWD" | "SHLVL" | "CARGO" | "_"
    ) && ![
        "CARGO_", "RUST", "RUSTUP_", "CODEX_", "POSH_", "NPM_", "PNPM_",
    ]
    .iter()
    .any(|prefix| upper.starts_with(prefix))
}

pub(crate) fn runtime_script_hash(root: &Path, name: &str) -> String {
    find_runtime_script(root, name)
        .and_then(|path| fs::read(path).ok())
        .map(|source| content_hash_bytes(&source))
        .unwrap_or_default()
}

fn runtime_script_hashes(root: &Path, names: &[&str]) -> BTreeMap<String, String> {
    names
        .iter()
        .map(|name| ((*name).to_owned(), runtime_script_hash(root, name)))
        .collect()
}

pub(crate) fn load_prerender_artifact(
    cache: &PrerenderArtifactCache,
    job: &PrerenderJob,
) -> Option<String> {
    let cache_file = prerender_artifact_cache_file(&cache.directory, job);
    let source = fs::read_to_string(&cache_file).ok()?;
    let artifact: CachedPrerenderArtifact = serde_json::from_str(&source).ok()?;
    if artifact.dependency_hash != cache.dependency_hash
        || artifact.render_context_hash != cache.render_context_hash
        || artifact.renderer_dependency_hash.is_empty()
        || artifact.files.is_empty()
    {
        return None;
    }
    let valid = artifact
        .files
        .iter()
        .all(|(path, expected)| cache.fingerprints.fingerprint(path).as_deref() == Some(expected));
    valid.then_some(artifact.html)
}

/// Record one rendered document against the files it was rendered from.
///
/// `inputs` are keyed as given. Both callers pass `stable_prerender_inputs`
/// output, which has already resolved every worker-reported path against the
/// project root and canonicalized it, and `normalized_canonical_path` is a
/// `canonicalize` syscall each time — the expensive filesystem call on Windows.
/// Repeating it here answered identically by construction while costing a second
/// syscall per input on every stored artifact, so a dynamic route expanded to
/// thousands of paths paid `2 × modules` of them per path. A caller with
/// unresolved paths canonicalizes before calling; this is the hot loop and that
/// is not. Held by `store_prerender_artifact_keys_by_the_paths_it_is_given`.
///
/// The sibling [`store_server_component_entry`] deliberately still normalizes:
/// it takes its inputs straight out of the worker response with nothing between.
pub(crate) fn store_prerender_artifact(
    cache: &PrerenderArtifactCache,
    job: &PrerenderJob,
    renderer_dependency_hash: &str,
    inputs: &[PathBuf],
    html: &str,
) {
    if renderer_dependency_hash.is_empty() {
        return;
    }
    let files = inputs
        .iter()
        .filter_map(|path| {
            cache
                .fingerprints
                .fingerprint(path)
                .map(|fingerprint| (path.clone(), fingerprint))
        })
        .collect::<BTreeMap<_, _>>();
    if files.is_empty() {
        return;
    }
    let artifact = CachedPrerenderArtifact {
        dependency_hash: cache.dependency_hash.clone(),
        render_context_hash: cache.render_context_hash.clone(),
        renderer_dependency_hash: renderer_dependency_hash.to_string(),
        files,
        html: html.to_string(),
    };
    let Ok(source) = serde_json::to_vec(&artifact) else {
        return;
    };
    write_client_cache_file(prerender_artifact_cache_file(&cache.directory, job), source);
}

pub(crate) fn load_shared_route_artifact(
    cache_dir: &Path,
    dependency_hash: &str,
    module_paths: &[PathBuf],
    variant: &str,
    fingerprints: &ArtifactFingerprintCache,
) -> Option<ruvyxa_bundler::SharedRouteBundleOutput> {
    let source = fs::read_to_string(shared_route_artifact_cache_file(
        cache_dir,
        module_paths,
        variant,
    ))
    .ok()?;
    let artifact: CachedSharedRouteArtifact = serde_json::from_str(&source).ok()?;
    if artifact.dependency_hash != dependency_hash
        || artifact.files.is_empty()
        || artifact.modules.is_empty()
    {
        return None;
    }
    artifact
        .files
        .iter()
        .all(|(path, expected)| fingerprints.fingerprint(path).as_deref() == Some(expected))
        .then_some(ruvyxa_bundler::SharedRouteBundleOutput {
            code: artifact.code,
            modules: artifact.modules,
            diagnostics: artifact.diagnostics,
        })
}

pub(crate) fn store_shared_route_artifact(
    cache_dir: &Path,
    dependency_hash: &str,
    module_paths: &[PathBuf],
    variant: &str,
    output: &ruvyxa_bundler::SharedRouteBundleOutput,
    fingerprints: &ArtifactFingerprintCache,
) {
    let files = output
        .modules
        .iter()
        .filter_map(|path| {
            fingerprints
                .fingerprint(path)
                .map(|fingerprint| (path.clone(), fingerprint))
        })
        .collect::<BTreeMap<_, _>>();
    if files.is_empty() {
        return;
    }
    let artifact = CachedSharedRouteArtifact {
        dependency_hash: dependency_hash.to_string(),
        files,
        code: output.code.clone(),
        modules: output.modules.clone(),
        diagnostics: output.diagnostics.clone(),
    };
    let Ok(source) = serde_json::to_vec(&artifact) else {
        return;
    };
    write_client_cache_file(
        shared_route_artifact_cache_file(cache_dir, module_paths, variant),
        source,
    );
}

pub(crate) fn load_client_plan(
    cache_dir: &Path,
    dependency_hash: &str,
    route_path: &str,
    variant: &str,
    fingerprints: &ArtifactFingerprintCache,
) -> Option<Vec<PathBuf>> {
    let source = fs::read_to_string(client_plan_cache_file(cache_dir, route_path, variant)).ok()?;
    let plan: CachedClientPlan = serde_json::from_str(&source).ok()?;
    if plan.dependency_hash != dependency_hash
        || plan.files.is_empty()
        || plan.module_paths.is_empty()
    {
        return None;
    }
    plan.files
        .iter()
        .all(|(path, expected)| fingerprints.fingerprint(path).as_deref() == Some(expected))
        .then_some(plan.module_paths)
}

pub(crate) fn store_client_plan(
    cache_dir: &Path,
    dependency_hash: &str,
    route_path: &str,
    variant: &str,
    module_paths: &[PathBuf],
    dependency_paths: &BTreeSet<PathBuf>,
    fingerprints: &ArtifactFingerprintCache,
) {
    let files = dependency_paths
        .iter()
        .filter_map(|path| {
            fingerprints
                .fingerprint(path)
                .map(|fingerprint| (path.clone(), fingerprint))
        })
        .collect::<BTreeMap<_, _>>();
    if files.is_empty() {
        return;
    }
    let plan = CachedClientPlan {
        dependency_hash: dependency_hash.to_string(),
        files,
        module_paths: module_paths.to_vec(),
    };
    let Ok(source) = serde_json::to_vec(&plan) else {
        return;
    };
    write_client_cache_file(
        client_plan_cache_file(cache_dir, route_path, variant),
        source,
    );
}

pub(crate) fn server_component_entry_cache_file(cache_dir: &Path, route_path: &str) -> PathBuf {
    cache_dir
        .join("server-component-entries")
        .join(format!("{}.json", versioned_key(route_path)))
}

/// Everything a `react-server` compile depends on that is not a project file.
///
/// The worker runtime list is the prerender list unchanged, and deliberately
/// wider than what this compile reads: a superset can only invalidate an entry
/// that would still have been valid, while a subset serves a reference list the
/// current worker would not produce — and a wrong reference list does not fail
/// here, it fails much later as `RUV1820` on an import the project is right to
/// have. The environment is in too, because it carries the JSX runtime, the ES
/// target, `NODE_ENV`, and the project's own variables, each of which can change
/// which modules the graph resolves.
pub(crate) fn server_component_context_hash(
    root: &Path,
    runtime: ruvyxa_dev_server::JavaScriptRuntime,
    worker_env: &BTreeMap<String, String>,
) -> String {
    let context = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "runtime": runtime.command(),
        "workerRuntime": runtime_script_hashes(root, WORKER_RUNTIME_FILES),
        "workerEnv": worker_env,
    });
    content_hash(&context.to_string())
}

pub(crate) fn load_server_component_entry(
    cache: &ServerComponentEntryCache,
    route_path: &str,
) -> Option<CachedServerComponentEntry> {
    let source = fs::read_to_string(server_component_entry_cache_file(
        &cache.directory,
        route_path,
    ))
    .ok()?;
    let entry: CachedServerComponentEntry = serde_json::from_str(&source).ok()?;
    if entry.dependency_hash != cache.dependency_hash
        || entry.context_hash != cache.context_hash
        || entry.files.is_empty()
        || entry.entry_source.is_empty()
    {
        return None;
    }
    entry
        .files
        .iter()
        .all(|(path, expected)| cache.fingerprints.fingerprint(path).as_deref() == Some(expected))
        .then_some(entry)
}

pub(crate) fn store_server_component_entry(
    cache: &ServerComponentEntryCache,
    route_path: &str,
    inputs: &[PathBuf],
    entry_source: &str,
    server_references: &[ruvyxa_dev_server::ServerReferenceSource],
) {
    let files = inputs
        .iter()
        .map(|path| ruvyxa_diagnostics::normalized_canonical_path(path))
        .filter_map(|path| {
            cache
                .fingerprints
                .fingerprint(&path)
                .map(|fingerprint| (path, fingerprint))
        })
        .collect::<BTreeMap<_, _>>();
    // A worker that reported no readable inputs has given nothing to invalidate
    // against, and an entry with an empty file list would answer for every
    // future state of the project.
    if files.is_empty() {
        return;
    }
    let entry = CachedServerComponentEntry {
        dependency_hash: cache.dependency_hash.clone(),
        context_hash: cache.context_hash.clone(),
        files,
        entry_source: entry_source.to_string(),
        server_references: server_references.to_vec(),
    };
    let Ok(source) = serde_json::to_vec(&entry) else {
        return;
    };
    write_client_cache_file(
        server_component_entry_cache_file(&cache.directory, route_path),
        source,
    );
}

pub(crate) fn load_client_artifact(
    cache_dir: &Path,
    dependency_hash: &str,
    route_path: &str,
    variant: &str,
    fingerprints: &ArtifactFingerprintCache,
) -> Option<ClientBundle> {
    let source =
        fs::read_to_string(client_artifact_cache_file(cache_dir, route_path, variant)).ok()?;
    let artifact: CachedClientArtifact = serde_json::from_str(&source).ok()?;
    if artifact.dependency_hash != dependency_hash || artifact.files.is_empty() {
        return None;
    }
    let valid = artifact
        .files
        .iter()
        .all(|(path, expected)| fingerprints.fingerprint(path).as_deref() == Some(expected));
    valid.then_some(ClientBundle {
        artifact_cache_hit: true,
        ..artifact.bundle
    })
}

pub(crate) fn store_client_artifact(
    cache_dir: &Path,
    dependency_hash: &str,
    route_path: &str,
    variant: &str,
    bundle: &ClientBundle,
    fingerprints: &ArtifactFingerprintCache,
) {
    let files = bundle
        .dependency_paths
        .iter()
        .filter_map(|path| {
            fingerprints
                .fingerprint(path)
                .map(|fingerprint| (path.clone(), fingerprint))
        })
        .collect::<BTreeMap<_, _>>();
    if files.is_empty() {
        return;
    }
    let artifact = CachedClientArtifact {
        dependency_hash: dependency_hash.to_string(),
        files,
        bundle: bundle.clone(),
    };
    let Ok(source) = serde_json::to_vec(&artifact) else {
        return;
    };
    write_client_cache_file(
        client_artifact_cache_file(cache_dir, route_path, variant),
        source,
    );
}

pub(crate) fn write_client_cache_file(path: PathBuf, source: Vec<u8>) {
    // A cache miss costs a rebuild, never a wrong answer, so a failed publish is
    // dropped. It must not fall back to writing *something*: the previous
    // recovery read the temporary back with `unwrap_or_default()`, so a recovery
    // that itself failed replaced a good entry with zero bytes and the next build
    // served an empty client bundle from cache.
    let _ = ruvyxa_bundler::atomic_file::write_atomic(&path, &source);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime directory the worker list names, for walking its imports.
    fn runtime_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/ruvyxa/runtime")
    }

    /// Every sibling `.mjs` the worker reaches, starting from its entry.
    ///
    /// The walk goes through `ruvyxa_bundler::ast`, which is the one scanner
    /// this repository has: a second hand-rolled one would answer differently
    /// about a specifier inside a comment or a template literal, and would then
    /// be wrong in the direction that makes this test pass while the list is
    /// short.
    fn worker_import_closure() -> BTreeSet<String> {
        let directory = runtime_dir();
        let mut seen = BTreeSet::new();
        let mut pending = vec!["worker-pool.mjs".to_string()];
        while let Some(name) = pending.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Ok(source) = fs::read_to_string(directory.join(&name)) else {
                continue;
            };
            for specifier in ruvyxa_bundler::ast::parse_module(&source).import_specifiers() {
                let Some(sibling) = specifier.strip_prefix("./") else {
                    continue;
                };
                if sibling.ends_with(".mjs") {
                    pending.push(sibling.to_string());
                }
            }
        }
        seen
    }

    /// Everything the prerender worker runs is part of what its output depends
    /// on.
    ///
    /// A missing entry does not fail: it serves a prerendered page built by a
    /// worker that no longer exists, which is the definition of a stale
    /// artifact and looks exactly like a correct warm build. The list is
    /// allowed to be wider than the closure — `server_component_context_hash`
    /// says why, and over-hashing only costs a rebuild that reproduces the same
    /// bytes — but it may never be narrower, and nothing checked that until an
    /// import added to a worker-reachable module would have made it so.
    #[test]
    fn the_worker_runtime_list_covers_everything_the_worker_imports() {
        let listed: BTreeSet<String> = WORKER_RUNTIME_FILES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        let reached = worker_import_closure();
        assert!(
            reached.len() > 1,
            "the walk found nothing to walk — the runtime directory moved"
        );

        let unhashed: Vec<&String> = reached.difference(&listed).collect();
        assert!(
            unhashed.is_empty(),
            "the worker imports {unhashed:?}, and a change to one of those would not invalidate \
             a single prerendered page. Add them to WORKER_RUNTIME_FILES."
        );
    }

    /// A name in the list has to be a file the list can hash.
    ///
    /// `runtime_script_hash` answers an unreadable name with an empty string
    /// rather than an error, so a typo — or a module that was renamed with the
    /// list left behind — contributes a constant to every cache key it appears
    /// in. It goes on invalidating nothing, for as long as nobody looks.
    #[test]
    fn every_worker_runtime_entry_names_a_file_that_exists() {
        let directory = runtime_dir();
        for name in WORKER_RUNTIME_FILES {
            assert!(
                directory.join(name).is_file(),
                "WORKER_RUNTIME_FILES names {name}, which is not in {}. Its hash is the empty \
                 string, and it has been invalidating nothing.",
                directory.display()
            );
        }
    }

    /// A Ruvyxa upgrade must invalidate build artifacts. Keys that hashed only
    /// project inputs kept serving chunks emitted by the previous compiler —
    /// including, before the resolver fix, a shared chunk whose `require` had
    /// been replaced by a `RUV1610` throw.
    #[test]
    fn artifact_cache_keys_are_scoped_to_the_toolchain_version() {
        let cache_dir = Path::new("/app/.ruvyxa/cache/bundler");
        let unversioned = content_hash("/blog\0base");

        for path in [
            client_artifact_cache_file(cache_dir, "/blog", "base"),
            client_plan_cache_file(cache_dir, "/blog", "base"),
        ] {
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            assert_ne!(stem, unversioned, "key must not ignore the Ruvyxa version");
            assert_eq!(stem, versioned_key("/blog\0base"));
        }

        let modules = [PathBuf::from("/app/app/page.tsx")];
        let shared = shared_route_artifact_cache_file(cache_dir, &modules, "base");
        let mut expected = String::from("base");
        expected.push('\0');
        expected.push_str("/app/app/page.tsx");
        assert_eq!(
            shared.file_stem().unwrap().to_string_lossy(),
            versioned_key(&expected)
        );
    }

    /// A warning must be a function of the code, never of the cache.
    ///
    /// Non-fatal boundary diagnostics were reported beside the bundler call, so
    /// an artifact-cache hit returned the bundle and said nothing: `RUV1008` --
    /// a private `process.env` read reachable from browser code -- printed on
    /// the first build of a project and on no build after it. Whether a
    /// developer saw it depended on whether `.ruvyxa/cache` happened to be warm.
    ///
    /// They now ride on the bundle, so this asserts the only property that
    /// makes the reporter's job possible: what was stored is what comes back.
    #[test]
    fn a_cached_bundle_still_carries_the_warnings_its_build_produced() {
        let temp = tempfile::tempdir().unwrap();
        let cache_dir = temp.path().join("bundler");
        let entry = temp.path().join("page.tsx");
        fs::write(
            &entry,
            "export default () => null
",
        )
        .unwrap();

        let fingerprints = ArtifactFingerprintCache::default();
        let diagnostics = vec![
            "RUV1008 Private environment variable used in client bundle".to_string(),
            "RUV1008 second one, so a list is not flattened to its first".to_string(),
        ];
        let bundle = ClientBundle {
            path: "/blog".to_string(),
            entry: entry.clone(),
            file_name: "blog.js".to_string(),
            script: "/__ruvyxa/client/blog.js".to_string(),
            source_map_file: None,
            source_map: None,
            output_bytes: 1,
            estimated_gz_bytes: 1,
            duration_ms: 0,
            module_count: 1,
            cache_hits: 0,
            tree_shaken_modules: 0,
            artifact_cache_hit: false,
            module_paths: BTreeSet::from([entry.clone()]),
            dependency_paths: BTreeSet::from([entry.clone()]),
            chunk_manifest: None,
            chunks: Vec::new(),
            diagnostics: diagnostics.clone(),
        };

        store_client_artifact(&cache_dir, "dep", "/blog", "base", &bundle, &fingerprints);
        let restored = load_client_artifact(&cache_dir, "dep", "/blog", "base", &fingerprints)
            .expect("the artifact just written must load");

        assert!(restored.artifact_cache_hit, "this is the cache-hit path");
        assert_eq!(
            restored.diagnostics, diagnostics,
            "a cache hit must be able to reprint every warning the build found"
        );
    }
}
