//! The deployment description a build writes for whoever has to serve it.
//!
//! Ruvyxa ships eleven adapters plus a standalone server, and each one has to
//! answer the same two questions about a finished build before it can turn it
//! into a deployment: **which URLs may be answered from a file**, and **what
//! cache-control does each class of emitted file carry**. Every adapter used to
//! answer them for itself, by reading `manifest.json` and re-deriving the rules
//! from route metadata. That is how three of them grew their own copy of the
//! "do not publish an ISR page as a static file" rule — and how the fourth did
//! not, quietly turning ISR into SSG on that platform — and how the same two
//! cache-control strings ended up hand-written in six places with one of them
//! drifted.
//!
//! The `deploy` section of `manifest.json` answers both once, in the one place
//! that already knows: the build that produced the output. It is a *derived*
//! document — no field is a stamp somebody has to remember to bump — and it is
//! versioned, so an adapter can refuse a build it does not understand instead
//! of misreading it.
//!
//! It lives inside the route manifest rather than beside it. A second top-level
//! file describing the same routes is one more thing to find, to keep in sync,
//! and to guess the authority of; the route manifest is already what every
//! consumer opens. The copy written into a function directory has this section
//! removed — how to *serve* a build is a build-time question, and a serverless
//! bundle should not carry the answer.
//!
//! The classification rules themselves live in
//! `tests/fixtures/deploy-output-conformance.json`, replayed here and by
//! `packages/@ruvyxa/core/src/deploy-manifest.ts`, which is what the adapters
//! read. Adding a rule to one language and not the other fails in a test rather
//! than in a deployment.

use std::path::Path;

use ruvyxa_graph::{RouteKind, RouteManifest, RuntimeTarget};

use crate::artifact_cache::content_hash;
use crate::prerender::{NOT_FOUND_DOCUMENT_FILE, PrerenderedRoute};

/// The key the deployment description occupies in `manifest.json`.
///
/// One manifest, not two. A second top-level file describing the same routes
/// left a reader with no way to tell which one was authoritative, and the route
/// manifest is already what every consumer opens.
pub(crate) const DEPLOY_MANIFEST_KEY: &str = "deploy";

/// The version of the deployment-output contract this build writes.
///
/// Bumped only when an existing field changes meaning or disappears; adding a
/// field does not, because a reader that does not know it ignores it. An
/// adapter written against version 1 can therefore keep working across Ruvyxa
/// releases, and is entitled to refuse anything else.
pub(crate) const DEPLOY_MANIFEST_VERSION: u32 = 1;

/// Cache-control for content-addressed browser chunks under `/__ruvyxa/client/`.
///
/// The file name is the hash of the contents, so the URL cannot change meaning
/// and the answer is permanent.
pub(crate) const CLIENT_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// Cache-control for files published from `public/`.
///
/// The URL is chosen by the author, so the same path can mean something else
/// after the next deploy — cacheable, never immutable.
pub(crate) const ASSET_CACHE_CONTROL: &str = "public, max-age=3600, must-revalidate";

/// Cache-control for a pre-rendered document served as a file.
///
/// Safe to store, never safe to pin: a redeploy replaces the document under the
/// same URL, and a reader holding a heuristically-cached copy would keep seeing
/// the old site with no way to know.
pub(crate) const DOCUMENT_CACHE_CONTROL: &str = "public, max-age=0, must-revalidate";

/// How a route is served once the build is deployed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServeMode {
    /// A file the CDN or publish directory may answer directly.
    Static,
    /// The request must reach the server function.
    Function,
}

impl ServeMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ServeMode::Static => "static",
            ServeMode::Function => "function",
        }
    }
}

/// Whether a route may be answered from a file, given what the build produced.
///
/// The subtle case is the one that is silent when it is wrong: an ISR or PPR
/// page *has* a pre-rendered document, and a host that serves the publish
/// directory before invoking the function will happily answer from it forever.
/// The function that owns revalidation is then never reached, and the project's
/// `revalidate` becomes decoration. Those two are always served by the
/// function, which reads the same document as its first cache entry.
pub(crate) fn route_serve_mode(kind: RouteKind, strategy: &str, prerendered: bool) -> ServeMode {
    if kind == RouteKind::Api {
        return ServeMode::Function;
    }
    match strategy {
        "ssg" | "csr" if prerendered => ServeMode::Static,
        _ => ServeMode::Function,
    }
}

/// The cache-control a function returns with a document it just served.
///
/// ISR advertises the project's own clock so a CDN in front of the function can
/// hold the page for exactly as long as the project asked, and refresh it
/// without a gap. A per-request render advertises nothing cacheable: it may
/// carry one visitor's data, and a shared cache with no instruction has been
/// observed to store it anyway under heuristic freshness.
pub(crate) fn document_cache_control(strategy: &str, revalidate: Option<u64>) -> String {
    match strategy {
        "isr" => format!(
            "s-maxage={}, stale-while-revalidate",
            revalidate.unwrap_or(60)
        ),
        "ssg" | "csr" => DOCUMENT_CACHE_CONTROL.to_string(),
        _ => "no-store".to_string(),
    }
}

/// Everything the manifest is derived from.
pub(crate) struct DeployManifestInput<'a> {
    pub(crate) manifest: &'a RouteManifest,
    pub(crate) prerendered: &'a [PrerenderedRoute],
    pub(crate) prerender_dir: &'a Path,
    /// The `404.html` the build wrote, when the project has a not-found page.
    pub(crate) not_found_document: Option<&'a Path>,
    /// Content-addressed browser chunk file names, as emitted.
    pub(crate) client_assets: Vec<String>,
    pub(crate) base_path: String,
    pub(crate) adapter: Option<&'a str>,
    /// The head fragments a deployed request-time render has to add for
    /// itself, computed here and carried rather than recomputed.
    ///
    /// A deployed function renders pages the build never baked, and it has no
    /// `public/` to stat and no plugin host to ask — so whatever the build knew
    /// has to travel with the deployment or the document goes without it.
    /// Passing the finished strings keeps `public_asset_links` and
    /// `render_plugin_head` as the only implementations of either rule; the
    /// deployment holds bytes, not a second copy of the logic.
    pub(crate) document_head: DocumentHeadDefaults<'a>,
}

/// Head fragments the build resolved that a deployed render cannot re-derive.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DocumentHeadDefaults<'a> {
    /// The icon link derived from what the build published, or empty.
    pub(crate) asset_links: &'a str,
    /// Every plugin's declared head, rendered.
    pub(crate) plugin_head: &'a str,
}

/// Build the deployment manifest.
pub(crate) fn deploy_manifest(input: &DeployManifestInput<'_>) -> serde_json::Value {
    let documents: std::collections::BTreeMap<&str, String> = input
        .prerendered
        .iter()
        .filter_map(|route| {
            let relative = route
                .html_file
                .strip_prefix(input.prerender_dir)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            Some((route.path.as_str(), relative))
        })
        .collect();

    let mut routes = Vec::new();
    let mut static_paths = Vec::new();
    let mut function_paths = Vec::new();
    for route in &input.manifest.routes {
        let strategy = strategy_name(route.render.strategy);
        let document = documents.get(route.path.as_str()).cloned();
        let serve = route_serve_mode(route.kind, strategy, document.is_some());
        match serve {
            ServeMode::Static => static_paths.push(route.path.clone()),
            ServeMode::Function => function_paths.push(route.path.clone()),
        }
        routes.push(serde_json::json!({
            "id": route.id,
            "path": route.path,
            "kind": match route.kind {
                RouteKind::Page => "page",
                RouteKind::Api => "api",
            },
            "serve": serve.as_str(),
            "strategy": strategy,
            "runtime": match route.runtime {
                RuntimeTarget::Node => "node",
                RuntimeTarget::Edge => "edge",
                RuntimeTarget::Static => "static",
            },
            "revalidate": route.render.revalidate,
            "serverComponents": route.render.server_components,
            "hydrate": route.render.ships_client_bundle(),
            "document": document,
            "cacheControl": document_cache_control(strategy, route.render.revalidate),
        }));
    }

    // Documents the build produced that no route entry names: every path a
    // dynamic SSG route expanded to through `getStaticParams`. A CDN may answer
    // all of them from a file, and an adapter that only walked `routes` would
    // send each one through the function instead.
    let route_paths: std::collections::BTreeSet<&str> = input
        .manifest
        .routes
        .iter()
        .map(|route| route.path.as_str())
        .collect();
    let mut expanded = Vec::new();
    for route in input.prerendered {
        if route_paths.contains(route.path.as_str()) {
            continue;
        }
        let strategy = strategy_name(route.strategy);
        if route_serve_mode(RouteKind::Page, strategy, true) != ServeMode::Static {
            continue;
        }
        if let Some(document) = documents.get(route.path.as_str()) {
            expanded.push(serde_json::json!({
                "path": route.path,
                "document": document,
                "strategy": strategy,
            }));
        }
    }

    serde_json::json!({
        "version": DEPLOY_MANIFEST_VERSION,
        "framework": "ruvyxa",
        "frameworkVersion": env!("CARGO_PKG_VERSION"),
        "buildId": build_id(input),
        "basePath": input.base_path,
        "adapter": input.adapter,
        "directories": {
            "client": "client",
            "assets": "assets",
            "prerender": "prerender",
            "server": "server",
        },
        "endpoints": {
            "client": "/__ruvyxa/client/",
            "action": "/__ruvyxa/action",
            "flight": "/__ruvyxa/flight",
            "rsc": "/__ruvyxa/rsc",
            "image": "/__ruvyxa/image",
        },
        "headers": [
            {
                "source": "/__ruvyxa/client/(.*)",
                "headers": { "cache-control": CLIENT_CACHE_CONTROL },
            },
            {
                "source": "/(.*)",
                "class": "asset",
                "headers": { "cache-control": ASSET_CACHE_CONTROL },
            },
        ],
        // Read by `documentAssetsPrelude`, which bakes them into the function
        // bundle's document writer. Only a request-time render needs them: a
        // pre-rendered page already carries both, injected before it was
        // written to disk.
        "documentHead": {
            "assetLinks": input.document_head.asset_links,
            "pluginHead": input.document_head.plugin_head,
        },
        "assetClasses": {
            "client": CLIENT_CACHE_CONTROL,
            "asset": ASSET_CACHE_CONTROL,
            "document": DOCUMENT_CACHE_CONTROL,
        },
        "routes": routes,
        "staticPaths": static_paths,
        "functionPaths": function_paths,
        "prerendered": expanded,
        "notFound": input.not_found_document.map(|_| serde_json::json!({
            "status": 404,
            "document": NOT_FOUND_DOCUMENT_FILE,
        })),
        // No `i18n` here: this section sits inside the route manifest, which
        // already carries it. Repeating a value one level up is a second place
        // for it to be wrong.
    })
}

/// A stable identity for this build's output.
///
/// Derived from what was emitted — the framework version, the content-addressed
/// chunk names, the documents produced, and the route table — so two builds of
/// the same sources produce the same id and a changed output cannot keep the
/// old one. Deliberately not a timestamp or a random value: this repository
/// asserts that `ruvyxa build` is reproducible byte for byte, and a build id
/// that changed on its own would be the one field that broke it.
fn build_id(input: &DeployManifestInput<'_>) -> String {
    let mut assets = input.client_assets.clone();
    assets.sort();
    let mut documents = input
        .prerendered
        .iter()
        .map(|route| route.path.clone())
        .collect::<Vec<_>>();
    documents.sort();
    let identity = serde_json::json!({
        "version": DEPLOY_MANIFEST_VERSION,
        "framework": env!("CARGO_PKG_VERSION"),
        "assets": assets,
        "documents": documents,
        "routes": input.manifest.routes.iter().map(|route| {
            serde_json::json!({
                "id": route.id,
                "path": route.path,
                "strategy": strategy_name(route.render.strategy),
                "revalidate": route.render.revalidate,
            })
        }).collect::<Vec<_>>(),
    });
    content_hash(&identity.to_string())[..32].to_string()
}

/// The browser chunk file names a build emitted, sorted.
///
/// Names only: every one of them is the hash of its own contents, so the list
/// of names is already a fingerprint of the client output and re-reading the
/// bytes would answer the same question twice.
pub(crate) fn emitted_client_assets(client_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(client_dir) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn strategy_name(strategy: ruvyxa_graph::RenderStrategy) -> &'static str {
    match strategy {
        ruvyxa_graph::RenderStrategy::Ssr => "ssr",
        ruvyxa_graph::RenderStrategy::Ssg => "ssg",
        ruvyxa_graph::RenderStrategy::Isr => "isr",
        ruvyxa_graph::RenderStrategy::Csr => "csr",
        ruvyxa_graph::RenderStrategy::Ppr => "ppr",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/deploy-output-conformance.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        serde_json::from_str(&source).expect("deploy-output-conformance.json parses")
    }

    #[test]
    fn serve_mode_matches_the_shared_conformance_table() {
        let fixture = fixture();
        let cases = fixture["serve"]["cases"].as_array().expect("serve cases");
        assert!(!cases.is_empty(), "the fixture must carry cases");
        for case in cases {
            let kind = match case["kind"].as_str().expect("kind") {
                "page" => RouteKind::Page,
                "api" => RouteKind::Api,
                other => panic!("unknown route kind in fixture: {other}"),
            };
            let strategy = case["strategy"].as_str().expect("strategy");
            let prerendered = case["prerendered"].as_bool().expect("prerendered");
            let expected = case["expect"].as_str().expect("expect");
            assert_eq!(
                route_serve_mode(kind, strategy, prerendered).as_str(),
                expected,
                "{:?} {strategy} (prerendered: {prerendered}) — {}",
                kind,
                case["why"].as_str().unwrap_or_default()
            );
        }
    }

    #[test]
    fn document_cache_control_matches_the_shared_conformance_table() {
        let fixture = fixture();
        let cases = fixture["documentCacheControl"]["cases"]
            .as_array()
            .expect("documentCacheControl cases");
        for case in cases {
            let strategy = case["strategy"].as_str().expect("strategy");
            let revalidate = case["revalidate"].as_u64();
            assert_eq!(
                document_cache_control(strategy, revalidate),
                case["expect"].as_str().expect("expect"),
                "{strategy}"
            );
        }
    }

    #[test]
    fn asset_classes_match_the_shared_conformance_table() {
        let fixture = fixture();
        let classes = &fixture["assetClasses"];
        assert_eq!(classes["client"]["cacheControl"], CLIENT_CACHE_CONTROL);
        assert_eq!(classes["asset"]["cacheControl"], ASSET_CACHE_CONTROL);
        assert_eq!(classes["document"]["cacheControl"], DOCUMENT_CACHE_CONTROL);
    }

    #[test]
    fn the_build_id_is_derived_from_the_output_and_not_from_the_clock() {
        let manifest = RouteManifest {
            app_dir: Path::new("app").to_path_buf(),
            routes: Vec::new(),
            i18n: None,
        };
        let input = DeployManifestInput {
            manifest: &manifest,
            prerendered: &[],
            prerender_dir: Path::new("prerender"),
            not_found_document: None,
            client_assets: vec!["b.js".to_string(), "a.js".to_string()],
            base_path: String::new(),
            document_head: DocumentHeadDefaults::default(),
            adapter: None,
        };
        let first = build_id(&input);
        let second = build_id(&DeployManifestInput {
            // Same output, listed in the other order: the id is about what was
            // emitted, not about the order a directory walk happened to return.
            client_assets: vec!["a.js".to_string(), "b.js".to_string()],
            ..input
        });
        assert_eq!(first, second);

        let changed = build_id(&DeployManifestInput {
            manifest: &manifest,
            prerendered: &[],
            prerender_dir: Path::new("prerender"),
            not_found_document: None,
            client_assets: vec!["a.js".to_string(), "c.js".to_string()],
            base_path: String::new(),
            document_head: DocumentHeadDefaults::default(),
            adapter: None,
        });
        assert_ne!(first, changed, "a changed output must change the build id");
    }
}
