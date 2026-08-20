//! HTML document assembly: head/HMR injection, client hydration scripts,
//! and the dev error overlay / production error pages.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use axum::http::StatusCode;
use axum::response::Response;
use ruvyxa_diagnostics::{Diagnostic, RuvyxaError};
use ruvyxa_graph::{HydrationMode, I18nRouting, RouteEntry, RouteParams};
use serde::Deserialize;

use crate::{ServerConfig, html_response};

pub(crate) fn compose_document(rendered: &str, head_content: &str, hmr: &str) -> String {
    if contains_ascii_case(rendered, "<html") {
        let with_head = if contains_ascii_case(rendered, "<head") {
            insert_before_ascii_case(rendered, "</head>", head_content)
        } else if let Some(body_index) = find_ascii_case(rendered, "<body") {
            let mut document = String::with_capacity(rendered.len() + head_content.len() + 32);
            document.push_str(&rendered[..body_index]);
            document.push_str("<head>");
            document.push_str(head_content);
            document.push_str("</head>");
            document.push_str(&rendered[body_index..]);
            document
        } else {
            insert_after_opening_html(rendered, head_content)
        };

        return insert_before_ascii_case(&with_head, "</body>", hmr);
    }

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{head_content}</head><body>{rendered}{hmr}</body></html>"
    )
}

pub(crate) fn compose_localized_document(
    rendered: &str,
    head_content: &str,
    hmr: &str,
    i18n: Option<&I18nRouting>,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
) -> String {
    localize_document(
        &compose_document(rendered, head_content, hmr),
        i18n,
        &route.path,
        request_path,
        params,
    )
}

/// Apply validated file-system locale metadata to a complete HTML document.
/// Native rendering and production prerendering share this implementation.
pub fn localize_document(
    document: &str,
    i18n: Option<&I18nRouting>,
    route_path: &str,
    request_path: &str,
    params: &RouteParams,
) -> String {
    let Some((locale, locale_head)) =
        crate::i18n::localized_head(i18n, route_path, request_path, params)
    else {
        return document.to_string();
    };
    let document = insert_before_ascii_case(document, "</head>", &locale_head);
    set_document_lang(&document, &locale)
}

fn set_document_lang(document: &str, locale: &str) -> String {
    let Some(html_index) = find_ascii_case(document, "<html") else {
        return document.to_string();
    };
    let Some(relative_end) = document[html_index..].find('>') else {
        return document.to_string();
    };
    let tag_end = html_index + relative_end;
    let opening = &document[html_index..tag_end];
    if let Some(relative_attr) = find_ascii_case(opening, " lang=") {
        let equals = html_index + relative_attr + " lang".len();
        let value_start = equals + 1;
        let bytes = document.as_bytes();
        if value_start >= tag_end {
            return document.to_string();
        }
        let quote = bytes[value_start];
        let value_end = if matches!(quote, b'\'' | b'"') {
            document[value_start + 1..tag_end]
                .find(char::from(quote))
                .map(|index| value_start + 1 + index)
        } else {
            document[value_start..tag_end]
                .find(char::is_whitespace)
                .map(|index| value_start + index)
                .or(Some(tag_end))
        };
        if let Some(value_end) = value_end {
            let content_start = if matches!(quote, b'\'' | b'"') {
                value_start + 1
            } else {
                value_start
            };
            let mut output = String::with_capacity(document.len() + locale.len());
            output.push_str(&document[..content_start]);
            output.push_str(locale);
            output.push_str(&document[value_end..]);
            return output;
        }
    }

    let mut output = String::with_capacity(document.len() + locale.len() + 8);
    output.push_str(&document[..tag_end]);
    output.push_str(" lang=\"");
    output.push_str(locale);
    output.push('"');
    output.push_str(&document[tag_end..]);
    output
}

pub(crate) fn insert_after_opening_html(rendered: &str, head_content: &str) -> String {
    let Some(html_index) = find_ascii_case(rendered, "<html") else {
        return rendered.to_string();
    };
    let Some(close_index) = rendered[html_index..].find('>') else {
        return rendered.to_string();
    };
    let insert_index = html_index + close_index + 1;
    let mut document = String::with_capacity(rendered.len() + head_content.len() + 16);
    document.push_str(&rendered[..insert_index]);
    document.push_str("<head>");
    document.push_str(head_content);
    document.push_str("</head>");
    document.push_str(&rendered[insert_index..]);
    document
}

pub(crate) fn insert_before_ascii_case(input: &str, needle: &str, insertion: &str) -> String {
    let Some(index) = find_ascii_case(input, needle) else {
        let mut output = input.to_string();
        output.push_str(insertion);
        return output;
    };

    let mut output = String::with_capacity(input.len() + insertion.len());
    output.push_str(&input[..index]);
    output.push_str(insertion);
    output.push_str(&input[index..]);
    output
}

pub(crate) fn contains_ascii_case(input: &str, needle: &str) -> bool {
    find_ascii_case(input, needle).is_some()
}

/// ASCII-case-insensitive substring search.
///
/// `compose_document` runs several of these over the whole rendered document on
/// every SSR response, so this scans in place instead of allocating a lowercased
/// copy of the page per call. ASCII case folding is byte-for-byte, so the
/// returned index is a valid `str` boundary in the original input.
pub(crate) fn find_ascii_case(input: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let haystack = input.as_bytes();
    let needle = needle.as_bytes();
    let first = needle[0].to_ascii_lowercase();

    haystack.windows(needle.len()).position(|window| {
        window[0].to_ascii_lowercase() == first
            && window
                .iter()
                .zip(needle)
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

#[derive(Debug, Deserialize)]
struct ClientAssetManifest {
    routes: Vec<ClientAssetRoute>,
}

#[derive(Debug, Deserialize)]
struct ClientAssetRoute {
    path: String,
    src: String,
    #[serde(rename = "sharedChunks")]
    shared_chunks: Vec<ClientSharedChunk>,
    #[serde(default)]
    hydration: HydrationMode,
    #[serde(default, rename = "hydrationLoader")]
    hydration_loader: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClientSharedChunk {
    src: String,
}

#[derive(Clone)]
pub(crate) struct ClientAssets {
    pub(crate) src: String,
    pub(crate) preloads: Vec<String>,
    pub(crate) hydration: HydrationMode,
    pub(crate) hydration_loader: Option<String>,
}

/// Browser module that defers importing a route bundle until its trigger.
pub fn hydration_loader_source() -> &'static str {
    r#"const params=new URL(import.meta.url).searchParams;
const src=params.get('src');
const strategy=params.get('strategy');
const load=()=>src?import(src):Promise.resolve();
if(strategy==='idle'){
  if(typeof requestIdleCallback==='function')requestIdleCallback(()=>void load());
  else setTimeout(()=>void load(),1);
}else if(strategy==='visible'&&typeof IntersectionObserver==='function'){
  const target=document.body||document.documentElement;
  const observer=new IntersectionObserver((entries)=>{
    if(entries.some((entry)=>entry.isIntersecting)){
      observer.disconnect();
      void load();
    }
  });
  observer.observe(target);
}else{
  void load();
}
"#
}

/// Build the external loader URL for one deferred route bundle.
pub fn hydration_loader_url(loader_src: &str, client_src: &str, mode: HydrationMode) -> String {
    let strategy = match mode {
        HydrationMode::Idle => "idle",
        HydrationMode::Visible => "visible",
        HydrationMode::Load | HydrationMode::None => return client_src.to_string(),
    };
    format!(
        "{loader_src}?strategy={strategy}&src={}",
        url_encode_component(client_src)
    )
}

pub(crate) fn client_hydration_script(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
) -> String {
    // `export const hydrate = false` pages ship zero client JavaScript.
    // CSR routes never reach this branch: the 'use client' directive forces
    // hydrate=true during graph discovery.
    if !route.render.ships_client_bundle() {
        return String::new();
    }
    let params_json = serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string());
    let params_json = safe_json_for_script(&params_json);
    let request_path_json = safe_json_for_script(
        &serde_json::to_string(request_path).unwrap_or_else(|_| "\"/\"".to_string()),
    );
    let assets = if config.watch {
        ClientAssets {
            src: format!(
                "/__ruvyxa/client?path={}",
                url_encode_component(request_path)
            ),
            preloads: Vec::new(),
            hydration: route.render.hydration,
            hydration_loader: Some("/__ruvyxa/hydration-loader.js".to_string()),
        }
    } else {
        prebuilt_client_assets(config, &route.path).unwrap_or_else(|| ClientAssets {
            src: format!(
                "/__ruvyxa/client?path={}",
                url_encode_component(request_path)
            ),
            preloads: Vec::new(),
            hydration: route.render.hydration,
            hydration_loader: Some("/__ruvyxa/hydration-loader.js".to_string()),
        })
    };
    let deferred = matches!(
        assets.hydration,
        HydrationMode::Idle | HydrationMode::Visible
    );
    let preload_links = if deferred {
        String::new()
    } else {
        assets
            .preloads
            .iter()
            .map(|src| {
                let src = escape_html(src);
                format!(r#"<link rel="modulepreload" href="{src}">"#)
            })
            .collect::<String>()
    };
    let script_src = assets.hydration_loader.as_deref().map_or_else(
        || assets.src.clone(),
        |loader| hydration_loader_url(loader, &assets.src, assets.hydration),
    );
    let src = escape_html(&script_src);

    format!(
        r#"{preload_links}<script>globalThis.__RUVYXA_ROUTE_PARAMS__ = {params_json};globalThis.__RUVYXA_REQUEST_PATH__ = {request_path_json};</script><script type="module" src="{src}"></script>"#,
    )
}

/// Parsed client manifest, validated by content hash and short-circuited by a
/// settled `(length, mtime)`.
///
/// The document renderer looks up per-route script/preload assets on every SSR
/// request, and re-deserializing the whole manifest each time is wasted work on
/// a file that only changes on rebuild.
///
/// Correctness rests on the hash, not the metadata. A rebuild commonly rewrites
/// the manifest to the *same* length (only the content hash inside each bundle
/// URL changes, e.g. `home.a1b2c3.js` -> `home.d4e5f6.js`), so a metadata
/// fingerprint on its own can miss a real rebuild whenever the filesystem's
/// mtime resolution is coarser than the gap between writes (FAT, some network
/// and container mounts) and the server would then serve the previous build's
/// bundle URLs.
///
/// `settled_identity` is what makes the steady state cheap without giving that
/// up. It is only ever recorded for a file whose mtime is already older than
/// [`MANIFEST_SETTLE`], which is exactly the condition under which a later write
/// must land in a newer second and therefore cannot be confused with the bytes
/// already cached. Every other case — no metadata, an unsettled file, a changed
/// fingerprint — reads and hashes as before.
struct CachedClientManifest {
    content_hash: blake3::Hash,
    /// Insertion order, used only to choose an eviction victim once
    /// [`MAX_CACHED_MANIFEST_ROOTS`] is exceeded.
    sequence: u64,
    /// `(length, mtime)` the cached bytes were read at, once that pair became
    /// old enough to identify them. See [`manifest_identity`].
    settled_identity: Option<(u64, SystemTime)>,
    routes: Arc<HashMap<String, ClientAssets>>,
}

/// How long the manifest's mtime must be in the past before `(len, mtime)` is
/// allowed to stand in for its bytes.
///
/// This is the same bound, for the same reason, as `ASSET_ETAG_SETTLE` in
/// `static_assets.rs`: several filesystems record mtime to the second, so two
/// writes inside one second can leave an identical `(len, mtime)` for different
/// content. Once a file's mtime is already older than this window, any later
/// write necessarily lands in a newer second and cannot be missed.
const MANIFEST_SETTLE: Duration = Duration::from_secs(2);

/// `(length, mtime)` of the manifest, but only once that pair has settled.
///
/// `None` means "ask the bytes" — either the metadata is unreadable or the file
/// changed too recently for its timestamp to identify what it holds.
fn manifest_identity(manifest_path: &Path) -> Option<(u64, SystemTime)> {
    let metadata = fs::metadata(manifest_path).ok()?;
    let modified = metadata.modified().ok()?;
    SystemTime::now()
        .duration_since(modified)
        .ok()
        .filter(|age| *age >= MANIFEST_SETTLE)
        .map(|_| (metadata.len(), modified))
}

/// How many project roots may keep a parsed manifest at once.
///
/// A server process normally serves exactly one root, so this bound is never
/// reached in production; it exists because the cache is process-global and
/// keyed by path, and nothing in the type stopped a process that saw many roots
/// (a test run, a supervisor reusing one process) from retaining every parse it
/// ever made for the life of the process. 128 is far above the real working set
/// and small enough that the eviction scan below is irrelevant.
pub(crate) const MAX_CACHED_MANIFEST_ROOTS: usize = 128;

/// The manifest cache with the insertion order needed to bound it.
///
/// `next_sequence` is what makes eviction oldest-first: entries carry the
/// sequence they were inserted at, so the entry to drop is the one holding the
/// smallest. Refreshing an entry's `settled_identity` deliberately leaves its
/// sequence alone — that is not a new insertion, and letting a steady stream of
/// metadata refreshes reorder the queue would make eviction depend on read
/// traffic rather than on age.
#[derive(Default)]
struct ClientManifestCache {
    entries: HashMap<PathBuf, CachedClientManifest>,
    next_sequence: u64,
}

impl ClientManifestCache {
    /// Store a parse, evicting the oldest root once the bound is exceeded.
    fn insert(&mut self, manifest_path: &Path, entry: CachedClientManifest) {
        self.entries.insert(manifest_path.to_path_buf(), entry);

        while self.entries.len() > MAX_CACHED_MANIFEST_ROOTS {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, cached)| cached.sequence)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        sequence
    }
}

static CLIENT_MANIFEST_CACHE: OnceLock<Mutex<ClientManifestCache>> = OnceLock::new();

/// How many roots the process-global manifest cache is currently holding.
///
/// Eviction is deliberately invisible through `prebuilt_client_assets` — an
/// evicted root simply re-parses and returns the same assets — so the bound can
/// only be proven by looking at the cache itself.
#[cfg(test)]
pub(crate) fn cached_client_manifest_roots() -> usize {
    CLIENT_MANIFEST_CACHE
        .get()
        .and_then(|cache| cache.lock().ok())
        .map_or(0, |guard| guard.entries.len())
}

pub(crate) fn prebuilt_client_assets(
    config: &ServerConfig,
    route_path: &str,
) -> Option<ClientAssets> {
    let manifest_path = config.client_dir.join("manifest.json");
    let routes = load_client_manifest(&manifest_path)?;
    routes.get(route_path).cloned()
}

/// Load the client manifest's per-route asset lookup, reusing the cached parse
/// when the source file's contents are byte-identical to the cached parse.
fn load_client_manifest(manifest_path: &Path) -> Option<Arc<HashMap<String, ClientAssets>>> {
    let cache = CLIENT_MANIFEST_CACHE.get_or_init(|| Mutex::new(ClientManifestCache::default()));

    // Metadata first. Hashing the bytes is what makes invalidation exact, but
    // it also means reading the whole manifest off disk on every server-rendered
    // request — a blocking read on an async worker thread, for a file that in
    // production is written once by the build and then never again. A settled
    // `(len, mtime)` answers the same question without transferring the
    // content; anything the timestamp cannot vouch for still falls through to
    // the hash below, so no rebuild is ever missed.
    let identity = manifest_identity(manifest_path);
    if let Some(identity) = identity
        && let Ok(guard) = cache.lock()
        && let Some(entry) = guard.entries.get(manifest_path)
        && entry.settled_identity == Some(identity)
    {
        return Some(Arc::clone(&entry.routes));
    }

    let source = fs::read(manifest_path).ok()?;
    let content_hash = blake3::hash(&source);

    if let Ok(mut guard) = cache.lock()
        && let Some(entry) = guard.entries.get_mut(manifest_path)
        && entry.content_hash == content_hash
    {
        // Same bytes, newly settled timestamp: record it so the next request
        // can stop at the metadata.
        entry.settled_identity = identity;
        return Some(Arc::clone(&entry.routes));
    }

    // Cache miss or the file changed since it was parsed: parse once, then
    // rebuild the route lookup for subsequent requests.
    let manifest: ClientAssetManifest = serde_json::from_slice(&source).ok()?;
    let mut routes: HashMap<String, ClientAssets> = HashMap::with_capacity(manifest.routes.len());
    for route in manifest.routes {
        // The build emits unique route paths; keep the first if that ever
        // changes, matching the previous `find`-based first-match behavior.
        routes
            .entry(route.path)
            .or_insert_with(move || ClientAssets {
                src: route.src,
                preloads: route.shared_chunks.into_iter().map(|c| c.src).collect(),
                hydration: route.hydration,
                hydration_loader: route.hydration_loader,
            });
    }
    let routes = Arc::new(routes);

    if let Ok(mut guard) = cache.lock() {
        let sequence = guard.take_sequence();
        guard.insert(
            manifest_path,
            CachedClientManifest {
                content_hash,
                sequence,
                // Re-read rather than reusing the pre-read value: the file may
                // have been rewritten between the metadata call and the read,
                // and storing the older identity next to the newer bytes would
                // pin a stale fast path.
                settled_identity: manifest_identity(manifest_path),
                routes: Arc::clone(&routes),
            },
        );
    }

    Some(routes)
}

/// Make a JSON value safe to embed inside an inline `<script>` element.
///
/// Escaping only `</` is not enough. The HTML tokenizer leaves script-data state
/// on `<!--`, and a following `<script` puts it in "script data double escaped"
/// state where `</script>` no longer closes the element — so a route parameter
/// containing `<!--<script>` swallows the rest of the document.
///
/// U+2028/U+2029 are line terminators in JavaScript but legal raw characters in
/// JSON, so they must be escaped too or they end the statement mid-literal.
/// `\uXXXX` is a legal escape in a JSON string, so the decoded value is
/// unchanged.
///
/// The CLI's prerender writer emits the same `<script>` payloads and calls this
/// function. It used to hold a byte-identical copy of these five replacements,
/// tied to this one by a comment promising they would stay in step — the exact
/// arrangement `AGENTS.md` names as the thing that drifts. Escaping rules for a
/// payload two writers embed in the same element belong in one place.
pub fn safe_json_for_script(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

pub(crate) fn hmr_client_script() -> &'static str {
    r#"<script>
(() => {
  const protocol = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${protocol}://${location.host}/__ruvyxa/hmr`);
  let lastSequence = 0;
  const reload = () => location.reload();
  const acknowledgeTrace = (message) => {
    if (message.traceAck !== true) return;
    void fetch('/__ruvyxa/trace', {
      method: 'POST',
      cache: 'no-store',
      credentials: 'same-origin',
      keepalive: true,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ traceId: message.traceId }),
    }).catch(() => {});
  };
  const applyCss = async (sequence) => {
    let applied = false;
    for (const link of document.querySelectorAll('link[rel="stylesheet"][href]')) {
      const url = new URL(link.href, location.href);
      url.searchParams.set('__ruvyxa_hmr', String(sequence));
      link.href = url.href;
      applied = true;
    }
    const current = document.querySelector('style[data-ruvyxa-css]');
    if (current) {
      const response = await fetch(location.href, { cache: 'no-store', credentials: 'same-origin' });
      if (!response.ok) return false;
      if (sequence !== lastSequence) return false;
      const next = new DOMParser().parseFromString(await response.text(), 'text/html')
        .querySelector('style[data-ruvyxa-css]');
      if (!next) return false;
      if (sequence !== lastSequence) return false;
      current.textContent = next.textContent;
      applied = true;
    }
    return applied;
  };
  socket.addEventListener("message", async (event) => {
    let message;
    try { message = JSON.parse(event.data); } catch { reload(); return; }
    if (message.protocol !== 'ruvyxa.hmr' || message.protocolVersion !== 1 ||
        !/^[a-f0-9]{32}$/.test(message.traceId) ||
        !Number.isSafeInteger(message.sequence)) {
      reload(); return;
    }
    if (message.sequence <= lastSequence) return;
    lastSequence = message.sequence;
    try { performance.mark(`ruvyxa:hmr:${message.traceId}:received`); } catch {}
    acknowledgeTrace(message);
    if (message.type === 'issues') {
      console.error('[ruvyxa] HMR issues', message.issues ?? []);
      if (message.fullReload) reload();
      return;
    }
    if (message.type === 'partial' && message.kind === 'css') {
      try { if (await applyCss(message.sequence)) return; } catch {}
      reload(); return;
    }
    if (message.type === 'partial' && message.kind === 'server-route') {
      const route = globalThis.__RUVYXA_ROUTE_PATTERN__ ?? location.pathname;
      if (Array.isArray(message.affectedRoutes) && !message.affectedRoutes.includes(route)) return;
    }
    if (message.type === 'partial' && message.kind === 'client-boundary') {
      const refresh = globalThis.__RUVYXA_HMR_REFRESH__;
      try {
        if (typeof refresh === 'function' && await refresh(message) === true) return;
      } catch (error) {
        console.error('[ruvyxa] client refresh boundary failed', error);
      }
    }
    // Client-boundary patching is promoted only when the runtime can prove an
    // accepted refresh boundary. Until then correctness wins.
    reload();
  });
})();
</script>"#
}

pub(crate) fn url_encode_component(input: &str) -> String {
    let mut output = String::new();

    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                output.push(byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }

    output
}

pub(crate) fn extract_code_frame(file: &Path, line: Option<u32>) -> Option<String> {
    let line = line?;
    let source = fs::read_to_string(file).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    let total = lines.len();
    let idx = line.saturating_sub(1) as usize;
    if idx >= total {
        return None;
    }
    let start = idx.saturating_sub(2);
    let end = (idx + 3).min(total);
    let mut frame = String::new();
    let max_digits = end.to_string().len().max(2);
    for (i, line_text) in lines[start..end].iter().enumerate() {
        let i = start + i;
        let num = i + 1;
        let prefix = if i == idx { ">" } else { " " };
        let marker = if i == idx { "  ← error" } else { "" };
        frame.push_str(&format!(
            " {prefix} {:>width$} │ {}{}\n",
            num,
            line_text,
            marker,
            width = max_digits
        ));
    }
    Some(frame)
}

pub(crate) fn error_response(
    status: StatusCode,
    diagnostics: &Diagnostic,
    is_dev: bool,
) -> Response {
    if !is_dev {
        return html_response(status, plain_error_page("Internal server error"));
    }
    let code_frame = diagnostics
        .span
        .as_ref()
        .and_then(|span| extract_code_frame(&span.file, span.line));
    let body = dev_diagnostic_overlay(diagnostics, code_frame.as_deref());
    html_response(status, body)
}

pub(crate) fn public_internal_error(config: &ServerConfig, error: &RuvyxaError) -> String {
    if config.watch {
        error.to_string()
    } else {
        "Internal server error".to_string()
    }
}

pub(crate) fn error_page(message: &str, show_overlay: bool) -> String {
    if show_overlay {
        dev_error_overlay(message, None, None, None)
    } else {
        plain_error_page(message)
    }
}

pub(crate) fn plain_error_page(message: &str) -> String {
    let not_found = message.contains("Route not found");
    let code = if not_found { "404" } else { "500" };
    let title = if not_found {
        "This page could not be found."
    } else {
        "Ruvyxa hit an unexpected error."
    };

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>Ruvyxa Error - {code}</title>
<style>
  :root {{ color-scheme: light; --bg: #18181c; --ink: #4c1d95; --muted: #6d4b8f; --accent: #7c3aed; --line: rgba(124,58,237,.28); }}
  *, *::before, *::after {{ box-sizing: border-box; }}
  html, body {{ min-height: 100%; }}
  body {{ display: grid; min-height: 100vh; place-items: center; margin: 0; padding: 28px; color: var(--ink); background: radial-gradient(circle at 50% 38%, rgba(111, 65, 143, .18), transparent 34rem), var(--bg); font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  .error-card {{ width: min(760px, 100%); padding: clamp(30px, 6vw, 66px); border: 1px solid rgba(124,58,237,.16); border-radius: 24px; background: #fff; box-shadow: 0 28px 90px rgba(0,0,0,.38), 0 0 0 1px rgba(255,255,255,.7) inset; text-align: center; }}
  .logo {{ display: block; width: clamp(82px, 15vw, 132px); height: clamp(82px, 15vw, 132px); margin: 0 auto 28px; object-fit: contain; filter: drop-shadow(0 12px 22px rgba(123, 62, 226, .3)); }}
  .status {{ display: inline-flex; align-items: center; justify-content: center; gap: clamp(14px, 3vw, 34px); margin: 0 auto 18px; }}
  .code {{ color: var(--accent); font: 800 clamp(36px, 7vw, 58px)/1 ui-monospace, SFMono-Regular, Consolas, monospace; letter-spacing: -.06em; }}
  .divider {{ width: 1px; height: 62px; background: var(--line); }}
  h1 {{ margin: 0; color: var(--ink); font-size: clamp(22px, 4vw, 34px); font-weight: 520; letter-spacing: -.035em; }}
  .message {{ max-width: 620px; margin: 18px auto 0; color: var(--muted); font: 15px/1.7 ui-monospace, SFMono-Regular, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }}
  .path-label {{ display: inline-block; margin-top: 20px; padding: 6px 12px; border: 1px solid rgba(124,58,237,.2); border-radius: 999px; color: #6d28d9; background: #f4efff; font-size: clamp(13px, 1.8vw, 16px); font-weight: 700; letter-spacing: .06em; text-transform: uppercase; text-shadow: 0 1px 0 rgba(255,255,255,.8); }}
  @media (max-width: 560px) {{ body {{ padding: 16px; }} .error-card {{ padding: 34px 22px; border-radius: 18px; }} .status {{ flex-direction: column; gap: 12px; }} .code {{ font-size: clamp(42px, 14vw, 54px); }} .divider {{ width: 64px; height: 1px; }} h1 {{ max-width: 260px; text-align: center; }} }}
</style>
</head>
<body>
<main class="error-card" aria-labelledby="error-title">
  <img class="logo" src="/ruvyxa.png" alt="Ruvyxa">
  <div class="status" aria-label="Error status">
    <span class="code">{code}</span>
    <span class="divider" aria-hidden="true"></span>
    <h1 id="error-title">{title}</h1>
  </div>
  <pre class="message">{}</pre>
  <div class="path-label">Ruvyxa Error</div>
</main>
</body>
</html>"##,
        escape_html(message)
    )
}

pub(crate) fn dev_error_overlay(
    message: &str,
    code_frame: Option<&str>,
    stack: Option<&str>,
    suggestion: Option<&str>,
) -> String {
    let mut lines = message.lines();
    let title = lines.next().unwrap_or("Unhandled Runtime Error");
    let detail = lines.collect::<Vec<_>>().join("\n");
    render_error_overlay(ErrorOverlayView {
        code: "RUV_RUNTIME",
        title,
        detail: if detail.trim().is_empty() {
            message
        } else {
            &detail
        },
        location: None,
        code_frame,
        stack,
        suggestion,
        import_chain: &[],
        affected_routes: &[],
    })
}

pub(crate) fn dev_diagnostic_overlay(diagnostic: &Diagnostic, code_frame: Option<&str>) -> String {
    let location = diagnostic
        .span
        .as_ref()
        .map(|span| match (span.line, span.column) {
            (Some(line), Some(column)) => format!("{}:{line}:{column}", span.file.display()),
            (Some(line), None) => format!("{}:{line}", span.file.display()),
            _ => span.file.display().to_string(),
        });
    render_error_overlay(ErrorOverlayView {
        code: diagnostic.code,
        title: &diagnostic.title,
        detail: &diagnostic.explanation,
        location: location.as_deref(),
        code_frame,
        stack: None,
        suggestion: diagnostic.suggested_fix.as_deref(),
        import_chain: &diagnostic.import_chain,
        affected_routes: &diagnostic.affected_routes,
    })
}

pub(crate) struct ErrorOverlayView<'a> {
    code: &'a str,
    title: &'a str,
    detail: &'a str,
    location: Option<&'a str>,
    code_frame: Option<&'a str>,
    stack: Option<&'a str>,
    suggestion: Option<&'a str>,
    import_chain: &'a [PathBuf],
    affected_routes: &'a [String],
}

pub(crate) fn render_error_overlay(view: ErrorOverlayView<'_>) -> String {
    let ErrorOverlayView {
        code,
        title,
        detail,
        location,
        code_frame,
        stack,
        suggestion,
        import_chain,
        affected_routes,
    } = view;
    let frame_html = code_frame
        .map(|f| {
            format!(
                r#"<section class="source"><div class="source-head"><span>Source</span><code>{}</code></div><pre>{}</pre></section>"#,
                escape_html(location.unwrap_or("source unavailable")),
                escape_html(f)
            )
        })
        .unwrap_or_default();
    let stack_html = stack
        .map(|s| {
            format!(
                r#"<details><summary>Stack trace</summary><pre>{}</pre></details>"#,
                escape_html(s)
            )
        })
        .unwrap_or_default();
    let suggestion_html = suggestion
        .map(|s| {
            format!(
                r#"<section class="hint"><strong>Suggested fix</strong><p>{}</p></section>"#,
                escape_html(s)
            )
        })
        .unwrap_or_default();
    let location_html = location
        .map(|location| format!(r#"<div class="location">{}</div>"#, escape_html(location)))
        .unwrap_or_default();
    let import_chain_html = if import_chain.is_empty() {
        String::new()
    } else {
        format!(
            r#"<details open><summary>Import chain ({})</summary><ol>{}</ol></details>"#,
            import_chain.len(),
            import_chain
                .iter()
                .map(|path| format!(
                    "<li><code>{}</code></li>",
                    escape_html(&path.display().to_string())
                ))
                .collect::<String>()
        )
    };
    let routes_html = if affected_routes.is_empty() {
        String::new()
    } else {
        format!(
            r#"<details open><summary>Affected routes ({})</summary><ul>{}</ul></details>"#,
            affected_routes.len(),
            affected_routes
                .iter()
                .map(|route| format!("<li><code>{}</code></li>", escape_html(route)))
                .collect::<String>()
        )
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Ruvyxa Error - {title}</title>
<style>
  *, *::before, *::after {{ box-sizing: border-box; }}
  :root {{ color-scheme: light; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  body {{
    margin: 0;
    min-height: 100vh;
    color: #171717;
    background: linear-gradient(135deg, #f1f1f1, #d9d9d9);
  }}
  .backdrop {{
    min-height: 100vh;
    padding: clamp(16px, 5vw, 64px);
    background: rgba(245, 245, 245, .76);
    backdrop-filter: blur(9px);
  }}
  .dialog {{
    width: min(920px, 100%);
    margin: 0 auto;
    background: #fff;
    border: 1px solid #d7d7d7;
    border-top: 3px solid #ef5b5b;
    border-radius: 8px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, .2);
    overflow: hidden;
  }}
  .toolbar {{
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 46px;
    padding: 0 14px;
    border-bottom: 1px solid #ececec;
    color: #6b6b6b;
    font-size: 12px;
  }}
  .toolbar button {{ border: 0; background: transparent; color: #707070; font-size: 22px; cursor: pointer; padding: 4px 8px; }}
  .content {{ padding: clamp(20px, 4vw, 40px); }}
  .eyebrow {{ color: #d53535; font: 700 12px/1.4 ui-monospace, SFMono-Regular, Consolas, monospace; letter-spacing: .06em; }}
  h1 {{ margin: 8px 0 6px; font-size: clamp(20px, 3vw, 28px); line-height: 1.25; }}
  .location {{ color: #b4232d; font: 500 13px/1.5 ui-monospace, SFMono-Regular, Consolas, monospace; overflow-wrap: anywhere; }}
  .detail {{ margin: 18px 0 24px; color: #424242; white-space: pre-wrap; overflow-wrap: anywhere; }}
  .source {{ margin: 20px 0; border: 1px solid #222; border-radius: 6px; overflow: hidden; background: #101010; color: #f5f5f5; }}
  .source-head {{ display: flex; justify-content: space-between; gap: 16px; padding: 8px 12px; border-bottom: 1px solid #333; color: #d7d7d7; font-size: 12px; }}
  .source-head code {{ color: #a8a8a8; overflow-wrap: anywhere; text-align: right; }}
  .source pre {{ margin: 0; padding: 16px; overflow: auto; color: #f3f3f3; font: 13px/1.6 ui-monospace, SFMono-Regular, Consolas, monospace; tab-size: 2; }}
  .hint {{ margin: 18px 0; padding: 14px 16px; border: 1px solid #9dd5ab; border-left: 4px solid #2f9e44; border-radius: 6px; background: #f3fbf5; }}
  .hint strong {{ color: #176b2c; }}
  .hint p {{ margin: 5px 0 0; color: #285b35; white-space: pre-wrap; }}
  details {{ margin-top: 12px; border: 1px solid #e2e2e2; border-radius: 6px; padding: 10px 12px; }}
  summary {{ cursor: pointer; font-weight: 650; }}
  details pre {{ overflow: auto; white-space: pre-wrap; color: #454545; font: 12px/1.55 ui-monospace, SFMono-Regular, Consolas, monospace; }}
  details ol, details ul {{ margin-bottom: 0; padding-left: 24px; }}
  details li {{ margin: 5px 0; overflow-wrap: anywhere; }}
  .footer {{ padding: 12px 20px; border-top: 1px solid #ececec; background: #fafafa; color: #777; font-size: 12px; text-align: center; }}
  @media (max-width: 600px) {{
    .backdrop {{ padding: 0; }}
    .dialog {{ min-height: 100vh; border-radius: 0; border-left: 0; border-right: 0; }}
    .source-head {{ flex-direction: column; }}
    .source-head code {{ text-align: left; }}
  }}
</style>
</head>
<body>
<main class="backdrop">
  <section class="dialog" id="ruvyxa-error-overlay" role="dialog" aria-modal="true" aria-labelledby="ruvyxa-error-title">
    <div class="toolbar"><span>‹ &nbsp; 1 of 1 unhandled error &nbsp; ›</span><button type="button" aria-label="Close error overlay" onclick="document.getElementById('ruvyxa-error-overlay').hidden=true">×</button></div>
    <div class="content">
      <div class="eyebrow">{code}</div>
      <h1 id="ruvyxa-error-title">{title}</h1>
      {location_html}
      <div class="detail">{detail}</div>
      {frame_html}
      {suggestion_html}
      {import_chain_html}
      {routes_html}
      {stack_html}
    </div>
    <div class="footer">Ruvyxa Dev Server — fix the error and save to hot-reload</div>
  </section>
</main>
</body>
</html>"#,
        code = escape_html(code),
        title = escape_html(title),
        detail = escape_html(detail),
    )
}

pub(crate) fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmr_client_rejects_stale_sequences_and_applies_css_without_reload() {
        let script = hmr_client_script();
        assert!(script.contains("ruvyxa.hmr"));
        assert!(script.contains("message.sequence <= lastSequence"));
        assert!(script.contains("ruvyxa:hmr:${message.traceId}:received"));
        assert!(script.contains("if (message.traceAck !== true) return"));
        assert!(script.contains("body: JSON.stringify({ traceId: message.traceId })"));
        assert!(script.contains("if (message.fullReload) reload()"));
        assert!(script.contains("style[data-ruvyxa-css]"));
        assert!(script.contains("await applyCss(message.sequence)"));
        assert!(script.contains("location.reload()"));
    }

    #[test]
    fn ascii_case_search_matches_and_keeps_original_indices() {
        assert_eq!(find_ascii_case("<HTML lang=\"en\">", "<html"), Some(0));
        assert_eq!(find_ascii_case("<p>a</P></BODY>", "</body>"), Some(8));
        assert_eq!(find_ascii_case("abc", "d"), None);
        assert_eq!(find_ascii_case("ab", "abc"), None);
        assert_eq!(find_ascii_case("", "a"), None);

        // Multi-byte text must not shift the reported byte offset.
        let input = "<p>สวัสดี</p></BODY>";
        let index = find_ascii_case(input, "</body>").unwrap();
        assert!(input.is_char_boundary(index));
        assert_eq!(&input[index..], "</BODY>");
    }

    #[test]
    fn script_json_neutralizes_html_comment_and_tag_openers() {
        // `</` alone is not enough: `<!--<script>` moves the tokenizer into
        // script-data-double-escaped state, where `</script>` stops closing the
        // element and the rest of the document is swallowed.
        let payload = serde_json::to_string(&serde_json::json!({
            "slug": "<!--<script>alert(1)</script>"
        }))
        .unwrap();

        let safe = safe_json_for_script(&payload);

        assert!(!safe.contains('<'), "{safe}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&safe).unwrap(),
            serde_json::json!({ "slug": "<!--<script>alert(1)</script>" }),
            "escaping must preserve the decoded value"
        );
    }

    #[test]
    fn composed_document_escapes_untrusted_route_params() {
        let params =
            serde_json::to_string(&serde_json::json!({ "slug": "</script><img>" })).unwrap();
        let script = format!(
            "<script>globalThis.__RUVYXA_ROUTE_PARAMS__ = {};</script>",
            safe_json_for_script(&params)
        );

        assert!(!script.contains("</script><img>"));
        assert_eq!(script.matches("</script>").count(), 1);
    }

    #[test]
    fn localizes_existing_document_language_and_adds_alternates() {
        let config = I18nRouting {
            locales: vec!["en".into(), "th".into()],
            default_locale: "en".into(),
            locale_param: "lang".into(),
            detect_locale: true,
            cookie: "RUVYXA_LOCALE".into(),
        };
        let params = RouteParams::from([("lang".into(), serde_json::json!("th"))]);
        let localized = localize_document(
            "<!doctype html><html lang=\"en\"><head><title>About</title></head><body></body></html>",
            Some(&config),
            "/[lang]/about",
            "/th/about",
            &params,
        );

        assert!(localized.contains("<html lang=\"th\">"));
        assert!(localized.contains("hreflang=\"en\" href=\"/en/about\""));
        assert!(localized.contains("hreflang=\"x-default\" href=\"/en/about\""));
    }
}
