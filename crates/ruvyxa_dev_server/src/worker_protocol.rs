//! The NDJSON request/response protocol spoken to a Node worker.
//!
//! One JSON document per line over the worker's stdin and stdout. This is the
//! wire format `packages/ruvyxa/runtime/worker-pool.mjs` reads and writes, so
//! every field name here is a cross-language contract: renaming one in this
//! file alone leaves the worker silently ignoring it, which reads downstream as
//! a route that renders without its parameters rather than as an error.
//!
//! Serialization only. Which worker a request goes to, how long it may take,
//! and what happens when one dies belong to `worker_pool.rs`; how a response is
//! demultiplexed back to its caller belongs there too. Keeping the format apart
//! from the policy is what makes the contract readable next to the JavaScript
//! half.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use ruvyxa_graph::RouteParams;
use serde::{Deserialize, Serialize};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_request_id() -> String {
    REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed).to_string()
}

// --- Public Request/Response Types ---

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerRequest {
    #[serde(rename = "ssr")]
    Ssr {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        /// Original path and query used to populate the ambient request
        /// context. Routing and page rendering continue to use `requestPath`.
        #[serde(rename = "requestTarget")]
        request_target: String,
        /// Route pattern (`/blog/[slug]`), not the concrete URL. It keys the
        /// worker's bundle cache and the browser's client-route registry, so a
        /// per-URL value would make every dynamic request a cache miss and
        /// register a route the client router can never look up.
        #[serde(rename = "routePath")]
        route_path: String,
        params: RouteParams,
        /// Ordered request headers, so a page can read `cookies()` and
        /// `headers()` while it renders. Additive: a worker script that
        /// predates request context ignores the field and renders as before.
        #[serde(rename = "headerPairs")]
        header_pairs: Vec<(String, String)>,
        /// Request method, uppercased.
        method: String,
        /// Render through the React Server Components pipeline.
        ///
        /// Additive: a worker script that predates server components ignores
        /// the field and renders the route the way it always did, which is the
        /// right answer for a worker that could not render it at all.
        #[serde(rename = "serverComponents")]
        server_components: bool,
        /// A `<form action={fn}>` submitted by a browser running no JavaScript.
        ///
        /// Absent for every ordinary render, which is what keeps this request
        /// retryable — see [`WorkerRequest::is_idempotent`]. Present, it makes
        /// the render run a server function first, and running it twice is
        /// exactly what a retry must not do.
        #[serde(rename = "formContentType", skip_serializing_if = "Option::is_none")]
        form_content_type: Option<String>,
        /// The submitted bytes, base64-encoded because a multipart body is not
        /// text and the worker protocol is line-delimited JSON.
        #[serde(rename = "formBody", skip_serializing_if = "Option::is_none")]
        form_body: Option<String>,
    },
    #[serde(rename = "flight")]
    Flight {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        #[serde(rename = "routePath")]
        route_path: String,
        params: RouteParams,
        #[serde(rename = "artifactVersion")]
        artifact_version: String,
    },
    #[serde(rename = "api")]
    Api {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "routeFile")]
        route_file: String,
        method: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        /// Legacy collapsed headers, retained so older worker scripts can still
        /// execute the request. New workers must prefer `headerPairs` below.
        headers: BTreeMap<String, String>,
        /// Ordered request header values. An HTTP header name can occur more
        /// than once, so a map would silently discard values at this boundary.
        #[serde(rename = "headerPairs")]
        header_pairs: Vec<(String, String)>,
        body: Option<String>,
        /// Lossless request body transport for bytes that are not valid UTF-8.
        /// The explicit field name is the NDJSON protocol tag for base64 data.
        #[serde(rename = "bodyBase64", skip_serializing_if = "Option::is_none")]
        body_base64: Option<String>,
        /// Ask workers that support framed responses to stream the API body.
        /// Older workers ignore this additive field and return the legacy body.
        #[serde(rename = "streamResponse")]
        stream_response: bool,
        params: RouteParams,
        /// Graph version already owned by the Rust HMR tracker. A matching
        /// worker may omit the otherwise repeated dependency list.
        #[serde(rename = "knownInputsVersion", skip_serializing_if = "Option::is_none")]
        known_inputs_version: Option<String>,
    },
    #[serde(rename = "action")]
    Action {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "actionFile")]
        action_file: String,
        #[serde(rename = "actionName")]
        action_name: String,
        #[serde(rename = "payloadJson")]
        payload_json: String,
        #[serde(rename = "contentType")]
        content_type: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        /// Ordered request header values so action handlers can observe the
        /// same cookies, authorization, and tracing headers as the endpoint.
        /// This additive field is ignored by older worker scripts.
        #[serde(rename = "headerPairs")]
        header_pairs: Vec<(String, String)>,
        /// Action graph version already owned by the Rust HMR tracker.
        #[serde(rename = "knownInputsVersion", skip_serializing_if = "Option::is_none")]
        known_inputs_version: Option<String>,
    },
    #[serde(rename = "client")]
    Client {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        /// Route pattern (`/blog/[slug]`), not the concrete URL. It keys the
        /// worker's bundle cache and the browser's client-route registry, so a
        /// per-URL value would make every dynamic request a cache miss and
        /// register a route the client router can never look up.
        #[serde(rename = "routePath")]
        route_path: String,
        params: RouteParams,
        /// Build the browser bundle for a server-components route: the client
        /// modules the payload references, not the page itself.
        #[serde(rename = "serverComponents")]
        server_components: bool,
    },
    /// Compile one shared browser module — React and its family.
    ///
    /// A build gives every route one shared chunk; an on-demand bundle has no
    /// cross-route analysis to build one from, so each would otherwise inline
    /// its own React and a soft navigation would render a component from one
    /// copy into a root owned by another.
    #[serde(rename = "clientVendor")]
    ClientVendor {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        name: String,
    },
    /// Render a server-components route's Flight payload, with no HTML.
    ///
    /// What a soft navigation asks for: the browser already has a document and
    /// a running React root, so the SSR pass would render markup nothing reads.
    /// Unlike the `flight` request above — Ruvyxa's own public, cacheable route
    /// payload — this one is request-scoped and carries the visitor's headers,
    /// because a server component may read `cookies()` exactly as it does on a
    /// full render.
    #[serde(rename = "rscPayload")]
    RscPayload {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        #[serde(rename = "requestTarget")]
        request_target: String,
        #[serde(rename = "routePath")]
        route_path: String,
        params: RouteParams,
        #[serde(rename = "headerPairs")]
        header_pairs: Vec<(String, String)>,
        method: String,
    },
    /// Render a server-components document as a stream.
    ///
    /// The same render as `Ssr` with `server_components`, framed as a body the
    /// host can pass through as it arrives. Only for a route whose document is
    /// produced per request: anything cached or pre-rendered has to become a
    /// string, and a stream is the wrong shape for that.
    #[serde(rename = "rscDocument")]
    RscDocument {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        #[serde(rename = "requestTarget")]
        request_target: String,
        #[serde(rename = "routePath")]
        route_path: String,
        params: RouteParams,
        #[serde(rename = "headerPairs")]
        header_pairs: Vec<(String, String)>,
        method: String,
        /// A `<form action={fn}>` submitted without JavaScript. See the same
        /// pair on [`WorkerRequest::Ssr`].
        #[serde(rename = "formContentType", skip_serializing_if = "Option::is_none")]
        form_content_type: Option<String>,
        #[serde(rename = "formBody", skip_serializing_if = "Option::is_none")]
        form_body: Option<String>,
    },
    /// Run one of a server-components route's server functions.
    ///
    /// The reference names the function; the route names the graphs searched
    /// for it. Both are needed because a server function is reachable from the
    /// route whose page or client components import it, and there is no
    /// build-wide index of every action in the application to consult instead.
    ///
    /// The body arrives base64-encoded because React's own encoder produces
    /// either UTF-8 text or multipart bytes depending on the arguments, and this
    /// protocol is line-delimited JSON: one encoding that survives both is
    /// cheaper than a second framing.
    #[serde(rename = "rscAction")]
    RscAction {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        #[serde(rename = "requestTarget")]
        request_target: String,
        #[serde(rename = "routePath")]
        route_path: String,
        params: RouteParams,
        #[serde(rename = "headerPairs")]
        header_pairs: Vec<(String, String)>,
        method: String,
        /// The `"<module>#<export>"` id the browser asked to call.
        reference: String,
        #[serde(rename = "contentType")]
        content_type: String,
        /// Base64 of the request body exactly as it arrived.
        body: String,
    },
    /// Ask for a server-components route's browser entry *source*, not a bundle.
    ///
    /// The build compiles it with the Rust bundler, which is where `NODE_ENV`
    /// folding, tree-shaking, minification, and the chunk budget live. The
    /// source has to come from the worker because only the `react-server` graph
    /// knows which of a route's modules are client references.
    #[serde(rename = "rscClientEntry")]
    RscClientEntry {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "routePath")]
        route_path: String,
    },
    #[serde(rename = "invalidate")]
    Invalidate {
        id: String,
        paths: Vec<String>,
        #[serde(rename = "traceId", skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    /// Read one stored document through the project's `cache.handler`.
    ///
    /// The document half of that setting, and the reason it has to cross this
    /// protocol at all: the store is a JavaScript module the project wrote, and
    /// only a worker can call it. `ruvyxa start` otherwise reads the build's own
    /// `prerender` directory, which is per-instance — right for one container
    /// and wrong for the several this setting exists to serve.
    #[serde(rename = "documentRead")]
    DocumentRead {
        id: String,
        pathname: String,
        /// The route's window, so a handler can answer `stale` for itself. The
        /// same argument `readPrerendered` takes in every deployed host.
        #[serde(skip_serializing_if = "Option::is_none")]
        revalidate: Option<u64>,
    },
    /// Persist one rendered document through the project's `cache.handler`.
    #[serde(rename = "documentWrite")]
    DocumentWrite {
        id: String,
        pathname: String,
        html: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        revalidate: Option<u64>,
        /// Whether `revalidatePath()` asked for this write. A handler may treat
        /// a forced write differently, and the deployed hosts pass the same
        /// flag.
        forced: bool,
    },
    #[serde(rename = "ping")]
    Ping { id: String },
    /// Withdraw a request the host is no longer listening to.
    ///
    /// It names the id of the work being abandoned rather than carrying one of
    /// its own, and the worker answers it with nothing at all: the host has no
    /// pending entry for a cancel, so any frame written for one would be
    /// delivered to — and read as — a response to the request it names.
    ///
    /// Without it a disconnect was a statement about the host's own bookkeeping
    /// and nothing more. The worker's stream loop is bounded only by *idle* time
    /// between chunks, which an SSE or long-poll response never reaches, so an
    /// abandoned stream ran forever. The worker aborts the request's
    /// `AbortController`, which reaches both the `Request.signal` the route
    /// handler holds and the reader draining its response body.
    ///
    /// A cancel for an id the worker has already answered is a no-op: the
    /// worker drops its entry when the terminal frame is written. Ids come from
    /// [`next_request_id`], a monotonic counter, so a later request can never
    /// reuse a cancelled one's id and inherit its abort.
    #[serde(rename = "cancel")]
    Cancel { id: String },
    #[serde(rename = "warmup")]
    Warmup {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        routes: Vec<WarmupRoute>,
    },
    /// Pre-render a page (used for ISR background revalidation at runtime).
    #[serde(rename = "ssg")]
    Ssg {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "appDir")]
        app_dir: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "requestPath")]
        request_path: String,
        /// Route pattern (`/blog/[slug]`), not the concrete URL. It keys the
        /// worker's bundle cache and the browser's client-route registry, so a
        /// per-URL value would make every dynamic request a cache miss and
        /// register a route the client router can never look up.
        #[serde(rename = "routePath")]
        route_path: String,
        params: RouteParams,
        /// "full" | "ppr" — controls whether to wait for all content or just the shell.
        mode: String,
        /// Build-only isolation: reload the module without discarding the compiled bundle cache.
        fresh: bool,
        /// Render through the React Server Components pipeline.
        #[serde(rename = "serverComponents")]
        server_components: bool,
    },
    /// Resolve static route parameters during production builds.
    #[serde(rename = "staticParams")]
    StaticParams {
        id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
        #[serde(rename = "pageFile")]
        page_file: String,
        #[serde(rename = "routePath")]
        route_path: String,
        segments: Vec<StaticParamSegment>,
        routes: Vec<StaticParamsRoute>,
    },
}

impl WorkerRequest {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Ssr { id, .. }
            | Self::Flight { id, .. }
            | Self::Api { id, .. }
            | Self::Action { id, .. }
            | Self::Client { id, .. }
            | Self::Invalidate { id, .. }
            | Self::Ping { id, .. }
            | Self::Cancel { id, .. }
            | Self::Warmup { id, .. }
            | Self::Ssg { id, .. }
            | Self::ClientVendor { id, .. }
            | Self::RscPayload { id, .. }
            | Self::RscDocument { id, .. }
            | Self::RscAction { id, .. }
            | Self::RscClientEntry { id, .. }
            | Self::DocumentRead { id, .. }
            | Self::DocumentWrite { id, .. }
            | Self::StaticParams { id, .. } => id,
        }
    }

    /// Whether serving this request makes the worker import a bundle under a
    /// fresh module URL, permanently adding one module graph to its ESM
    /// registry.
    pub(crate) fn retains_an_isolated_module_graph(&self) -> bool {
        matches!(self, Self::Ssg { fresh: true, .. })
    }

    /// Returns `true` if this request type is safe to retry without risk of
    /// duplicate side effects. Actions and API calls are NOT idempotent.
    ///
    /// A page render normally is: it reads and returns markup. One carrying a
    /// posted form is not, because the server function runs before the render
    /// — the same reason `Action` and `RscAction` are excluded.
    pub fn is_idempotent(&self) -> bool {
        matches!(
            self,
            Self::Ssr {
                form_body: None,
                ..
            } | Self::Flight { .. }
                | Self::Ssg { .. }
                | Self::StaticParams { .. }
                | Self::Client { .. }
                | Self::ClientVendor { .. }
                | Self::RscPayload { .. }
                | Self::RscClientEntry { .. }
                | Self::Ping { .. }
                | Self::Warmup { .. }
                // A read of the project's store, and a write that names the
                // document it stores rather than appending to anything. Both
                // land on the same key twice with the same result, so a worker
                // that died mid-flight can be asked again.
                | Self::DocumentRead { .. }
                | Self::DocumentWrite { .. }
                | Self::Invalidate { .. }
        )
    }
}

/// A route to pre-warm in the worker's module cache.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WarmupRoute {
    pub page_file: String,
    pub app_dir: String,
    /// Route pattern, so warmup compiles the exact bundle a later request asks
    /// for. A mismatched key would leave the warm module unused.
    pub route_path: String,
}

/// Route metadata passed to build-time parameter discovery.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticParamsRoute {
    pub path: String,
    pub id: String,
}

/// Dynamic segment metadata used to normalize the single-segment shorthand.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticParamSegment {
    pub name: String,
    pub catch_all: bool,
    pub optional: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerResponse {
    pub id: String,
    pub ok: bool,
    /// Framed API response discriminator. Absent for the legacy one-message protocol.
    pub frame: Option<String>,
    pub html: Option<String>,
    /// Encoded `ruvyxa.flight` envelope for a public navigation request.
    pub flight: Option<String>,
    pub script: Option<String>,
    pub status: Option<u16>,
    pub headers: Option<BTreeMap<String, String>>,
    /// Ordered response headers. Prefer this over `headers` so repeated
    /// `Set-Cookie` values survive the Node-to-Rust boundary.
    pub header_pairs: Option<Vec<(String, String)>>,
    pub body: Option<String>,
    /// Base64-encoded bytes for an `api-chunk` frame.
    pub body_base64: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub stack: Option<String>,
    pub pong: Option<bool>,
    pub warmed: Option<usize>,
    pub module_cache_size: Option<usize>,
    /// Distinct module URLs retained by the worker's ESM registry. Node cannot
    /// evict them; the host uses this telemetry to retire the process safely
    /// before normal dev/HMR rebuilds grow memory without bound.
    pub retained_module_urls: Option<usize>,
    pub params: Option<Vec<RouteParams>>,
    /// Set when the render read request state — a cookie, a header, draft
    /// mode. Such HTML belongs to one request and must never be stored in a
    /// cache other requests can read. Absent from older workers, which is
    /// treated as `false` because those workers cannot expose the accessors
    /// that would make it true.
    pub request_scoped: Option<bool>,
    /// Whether a document the project's `cache.handler` answered with is past
    /// its window and wants a background refresh.
    ///
    /// `None` from a worker that answered no document, and from every request
    /// that is not a `documentRead` — which is why it is an `Option<bool>`
    /// rather than a `bool`: "the store said fresh" and "nothing was asked" are
    /// different answers and only one of them means serve this.
    pub stale: Option<bool>,
    /// Concrete URLs `revalidatePath()` asked the host to refresh, collected
    /// from the API route or server action that just ran. Absent from older
    /// workers, which never call it.
    pub revalidate: Option<Vec<String>>,
    /// Content hash of the compiled SSG dependency graph.
    pub dependency_hash: Option<String>,
    /// Version of the normalized dependency set in `inputs`. When the request
    /// supplied the same known version, a current worker omits `inputs` to
    /// avoid repeated NDJSON work.
    pub inputs_version: Option<String>,
    /// Absolute source files used by the compiled bundle.
    pub inputs: Option<Vec<PathBuf>>,
    /// `'use server'` modules the browser graph reaches, with the source that
    /// must stand in for each of them there.
    ///
    /// The build compiles that graph with the Rust bundler, which would
    /// otherwise walk the real file — server code, in the action lane, and
    /// rejected as `RUV1820` the moment a client component imports it. The text
    /// travels rather than the rule so there is one implementation of what a
    /// server reference looks like.
    #[serde(rename = "serverReferences")]
    pub server_references: Option<Vec<ServerReferenceSource>>,
    /// Browser entry source for a server-components route, from `rscClientEntry`.
    pub entry_source: Option<String>,
    /// React Flight payload for a server-components render.
    ///
    /// Distinct from `flight` above, which carries Ruvyxa's own JSON route-data
    /// envelope and has nothing to do with React's wire format. Both names are
    /// on the wire because both things exist; naming this one `flight` too
    /// would have made one of the two silently win.
    pub rsc_payload: Option<String>,
}

/// One `'use server'` module and the source a browser graph must see instead.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerReferenceSource {
    /// The `ruv:s_…` id the module's exports are registered under.
    pub id: String,
    /// Absolute path of the real module, as the compiling graph resolved it.
    pub file: PathBuf,
    /// The stand-in source: references that post their arguments to the server.
    pub source: String,
}

impl WorkerResponse {
    pub(crate) fn is_terminal(&self) -> bool {
        !matches!(self.frame.as_deref(), Some("api-start" | "api-chunk"))
    }

    pub(crate) fn stream_error(id: String, message: impl Into<String>) -> Self {
        Self {
            id,
            frame: Some("api-error".to_string()),
            code: Some("RUV1704".to_string()),
            message: Some(message.into()),
            ..Self::default()
        }
    }
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Read the request id out of a serialized frame.
///
/// Both this module's tests and the pool's read it: a frame the pool wrote is
/// the only place its generated id is observable, and parsing it in two places
/// is how the two suites would come to disagree about the field name.
#[cfg(test)]
pub(crate) fn request_id_of(frame: &str) -> String {
    serde_json::from_str::<serde_json::Value>(frame.trim())
        .expect("worker frames must be valid JSON")["id"]
        .as_str()
        .expect("invalidate frames carry a string id")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every request that can render a page carries the server-components
    /// opt-in, under the name the worker reads.
    ///
    /// The three are separate variants but one decision: `ssr` serves a
    /// request, `ssg` pre-renders at build time, and `client` builds the
    /// browser bundle. A route that opted in and reached only two of them would
    /// render its payload and then ship a bundle with nothing to hydrate.
    #[test]
    fn every_page_request_carries_the_server_components_opt_in() {
        let ssr = WorkerRequest::Ssr {
            id: "1".to_string(),
            project_root: "/project".to_string(),
            app_dir: "/project/app".to_string(),
            page_file: "/project/app/page.tsx".to_string(),
            request_path: "/".to_string(),
            request_target: "/".to_string(),
            route_path: "/".to_string(),
            params: BTreeMap::new(),
            header_pairs: Vec::new(),
            method: "GET".to_string(),
            server_components: true,
            form_content_type: None,
            form_body: None,
        };
        let ssg = WorkerRequest::Ssg {
            id: "2".to_string(),
            project_root: "/project".to_string(),
            app_dir: "/project/app".to_string(),
            page_file: "/project/app/page.tsx".to_string(),
            request_path: "/".to_string(),
            route_path: "/".to_string(),
            params: BTreeMap::new(),
            mode: "full".to_string(),
            fresh: true,
            server_components: true,
        };
        let client = WorkerRequest::Client {
            id: "3".to_string(),
            project_root: "/project".to_string(),
            app_dir: "/project/app".to_string(),
            page_file: "/project/app/page.tsx".to_string(),
            request_path: "/".to_string(),
            route_path: "/".to_string(),
            params: BTreeMap::new(),
            server_components: true,
        };

        for request in [ssr, ssg, client] {
            let json = serde_json::to_value(&request).unwrap();
            assert_eq!(
                json.get("serverComponents"),
                Some(&serde_json::Value::Bool(true)),
                "{json}"
            );
        }
    }

    /// A page render is retryable; one that runs a server function first is not.
    ///
    /// The pool retries an idempotent request against a fresh worker when one
    /// dies mid-flight. A `<form action={fn}>` submission is a page render by
    /// shape and an action by effect, and retrying it would run the action a
    /// second time — charge the card twice, send the mail twice. The absent
    /// body is what tells the two apart, so the pattern below matches on it
    /// rather than on the variant.
    #[test]
    fn a_page_render_carrying_a_submitted_form_is_not_retried() {
        let render = |form_body: Option<String>| WorkerRequest::Ssr {
            id: "1".to_string(),
            project_root: "/project".to_string(),
            app_dir: "/project/app".to_string(),
            page_file: "/project/app/page.tsx".to_string(),
            request_path: "/".to_string(),
            request_target: "/".to_string(),
            route_path: "/".to_string(),
            params: BTreeMap::new(),
            header_pairs: Vec::new(),
            method: "POST".to_string(),
            server_components: true,
            form_content_type: form_body
                .as_ref()
                .map(|_| "multipart/form-data; boundary=x".to_string()),
            form_body,
        };

        assert!(render(None).is_idempotent());
        assert!(!render(Some("LS14".to_string())).is_idempotent());

        // Absent rather than null on the wire: a worker that predates form
        // actions reads the same object it always did.
        let json = serde_json::to_value(render(None)).unwrap();
        assert!(json.get("formBody").is_none(), "{json}");
        assert!(json.get("formContentType").is_none(), "{json}");
    }

    /// The browser entry for a server-components route comes back as source,
    /// not as a bundle: the build compiles it with the Rust bundler.
    #[test]
    fn the_rsc_client_entry_request_asks_for_a_route_by_pattern() {
        let request = WorkerRequest::RscClientEntry {
            id: "4".to_string(),
            project_root: "/project".to_string(),
            app_dir: "/project/app".to_string(),
            page_file: "/project/app/blog/[slug]/page.tsx".to_string(),
            route_path: "/blog/[slug]".to_string(),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["type"], "rscClientEntry");
        assert_eq!(json["routePath"], "/blog/[slug]");
        assert_eq!(json["pageFile"], "/project/app/blog/[slug]/page.tsx");
    }

    /// React's payload and Ruvyxa's own Flight envelope are different things
    /// travelling on the same wire, so they are read from different fields.
    #[test]
    fn the_response_keeps_the_two_flight_payloads_apart() {
        let response: WorkerResponse = serde_json::from_str(
            r#"{"id":"1","ok":true,"rscPayload":"0:[]","flight":"{\"protocol\":\"ruvyxa.flight\"}"}"#,
        )
        .unwrap();
        assert_eq!(response.rsc_payload.as_deref(), Some("0:[]"));
        assert!(response.flight.is_some());

        // Absent from a worker that predates server components, which is not an
        // error: that worker rendered the route the way it always did.
        let legacy: WorkerResponse = serde_json::from_str(r#"{"id":"1","ok":true}"#).unwrap();
        assert_eq!(legacy.rsc_payload, None);
    }

    /// A cancel names the request it withdraws, under the id field every other
    /// frame uses, because that is the only id the worker has to abort by.
    #[test]
    fn a_cancel_carries_the_id_of_the_request_it_withdraws() {
        let request = WorkerRequest::Cancel {
            id: "412".to_string(),
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["type"], "cancel");
        assert_eq!(value["id"], "412");
        assert_eq!(request.id(), "412");

        // Never retried: a retry would re-cancel a request that is already gone,
        // and the pool has nothing to wait for on this frame in the first place.
        assert!(!request.is_idempotent());
    }

    /// Cancellation aborts by id, so an id must never name two requests.
    ///
    /// The abort reaching a *retried* request that reused an id would cancel
    /// work the host is still waiting for, and it would do so only under the
    /// load that produces retries. The counter is monotonic, which makes reuse
    /// impossible — this pins that property rather than trusting it, because it
    /// is the whole reason cancellation can be keyed on the id alone.
    #[test]
    fn request_ids_are_never_reused() {
        let ids: Vec<u64> = (0..1_000)
            .map(|_| {
                next_request_id()
                    .parse()
                    .expect("request ids are decimal counter values")
            })
            .collect();

        // Strictly increasing, so no id this process ever issues can repeat —
        // including across the interleaving of other tests sharing the counter.
        assert!(
            ids.windows(2).all(|pair| pair[1] > pair[0]),
            "request ids must increase strictly: {ids:?}"
        );
    }

    #[test]
    fn ssr_worker_request_serializes_path_and_query_separately() {
        let request = WorkerRequest::Ssr {
            id: "test".to_string(),
            project_root: "/project".to_string(),
            app_dir: "/project/app".to_string(),
            page_file: "/project/app/search/page.tsx".to_string(),
            request_path: "/search".to_string(),
            request_target: "/search?q=ruvyxa".to_string(),
            route_path: "/search".to_string(),
            params: BTreeMap::new(),
            server_components: false,
            header_pairs: Vec::new(),
            method: "GET".to_string(),
            form_content_type: None,
            form_body: None,
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["requestPath"], "/search");
        assert_eq!(value["requestTarget"], "/search?q=ruvyxa");
    }

    #[test]
    fn api_worker_request_serializes_lossless_body_and_header_pairs() {
        let request = WorkerRequest::Api {
            id: "test".to_string(),
            project_root: "/project".to_string(),
            route_file: "/project/app/api/upload/route.ts".to_string(),
            method: "POST".to_string(),
            request_path: "/api/upload".to_string(),
            headers: BTreeMap::from([("x-repeat".to_string(), "second".to_string())]),
            header_pairs: vec![
                ("x-repeat".to_string(), "first".to_string()),
                ("x-repeat".to_string(), "second".to_string()),
            ],
            body: None,
            body_base64: Some(base64_encode(&[0, 255, 128, 13, 10])),
            stream_response: true,
            params: BTreeMap::new(),
            known_inputs_version: Some("graph-v1".to_string()),
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(
            value["headerPairs"][0],
            serde_json::json!(["x-repeat", "first"])
        );
        assert_eq!(
            value["headerPairs"][1],
            serde_json::json!(["x-repeat", "second"])
        );
        assert_eq!(value["bodyBase64"], "AP+ADQo=");
        assert_eq!(value["streamResponse"], true);
        assert_eq!(value["knownInputsVersion"], "graph-v1");
    }

    #[test]
    fn action_worker_request_serializes_lossless_request_header_pairs() {
        let request = WorkerRequest::Action {
            id: "action".to_string(),
            project_root: "/project".to_string(),
            action_file: "/project/app/action.ts".to_string(),
            action_name: "inspect".to_string(),
            payload_json: "{}".to_string(),
            content_type: "application/json".to_string(),
            request_path: "/account".to_string(),
            header_pairs: vec![
                ("authorization".to_string(), "Bearer token".to_string()),
                ("cookie".to_string(), "a=1".to_string()),
                ("cookie".to_string(), "b=2".to_string()),
            ],
            known_inputs_version: Some("action-v1".to_string()),
        };

        let value = serde_json::to_value(request).unwrap();
        assert_eq!(
            value["headerPairs"][0],
            serde_json::json!(["authorization", "Bearer token"])
        );
        assert_eq!(
            value["headerPairs"][1],
            serde_json::json!(["cookie", "a=1"])
        );
        assert_eq!(value["knownInputsVersion"], "action-v1");
        assert_eq!(
            value["headerPairs"][2],
            serde_json::json!(["cookie", "b=2"])
        );
    }
}
