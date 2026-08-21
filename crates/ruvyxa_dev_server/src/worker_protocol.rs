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
    },
    #[serde(rename = "invalidate")]
    Invalidate {
        id: String,
        paths: Vec<String>,
        #[serde(rename = "traceId", skip_serializing_if = "Option::is_none")]
        trace_id: Option<String>,
    },
    #[serde(rename = "ping")]
    Ping { id: String },
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
            | Self::Warmup { id, .. }
            | Self::Ssg { id, .. }
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
    pub fn is_idempotent(&self) -> bool {
        matches!(
            self,
            Self::Ssr { .. }
                | Self::Flight { .. }
                | Self::Ssg { .. }
                | Self::StaticParams { .. }
                | Self::Client { .. }
                | Self::Ping { .. }
                | Self::Warmup { .. }
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
            header_pairs: Vec::new(),
            method: "GET".to_string(),
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
