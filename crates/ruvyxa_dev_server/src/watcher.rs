//! The development file watcher and the HMR updates it produces.
//!
//! `dev` is the only mode that runs any of this: a filesystem event arrives,
//! the affected caches are invalidated, the route manifest is reconciled, and
//! one update frame goes out over the HMR socket. It lived in the crate root
//! next to `serve`, which is what actually starts it, and grew to a third of
//! that file — the watcher loop, the update classification, the wire payload,
//! and the four rules deciding which paths are worth waking up for.
//!
//! Split out because none of it is server assembly. `lib.rs` keeps the
//! configuration and the `serve` that owns the watcher's lifetime; the rules
//! for what a change means live here with the tests that pin them.

#[cfg(test)]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ruvyxa_diagnostics::{Result, RuvyxaError};
#[cfg(test)]
use ruvyxa_graph::discover_routes;
use ruvyxa_middleware::PluginHost;
use tokio::sync::broadcast;
use tracing::{info, warn};

#[cfg(test)]
use crate::discover_options;
use crate::hmr_tracker::{HmrEventType, HmrTracker, HmrUpdate};
use crate::render_cache::RenderCache;
use crate::worker_pool::NodeWorkerPool;
use crate::{INSTRUMENTATION_FILES, RuntimeCache, ServerConfig, trace};

pub(crate) struct WatcherRuntime {
    pub(crate) config: ServerConfig,
    pub(crate) reload_tx: broadcast::Sender<String>,
    pub(crate) runtime_cache: Arc<RuntimeCache>,
    pub(crate) worker_pool: Arc<NodeWorkerPool>,
    pub(crate) render_cache: Arc<RenderCache>,
    pub(crate) hmr_tracker: Arc<HmrTracker>,
    pub(crate) plugin_runtime: Option<Arc<PluginHost>>,
    pub(crate) edit_traces: Arc<trace::TraceStore>,
    pub(crate) tokio_handle: tokio::runtime::Handle,
}

async fn refresh_hmr_manifest(
    config: &ServerConfig,
    runtime_cache: &RuntimeCache,
    hmr_tracker: &HmrTracker,
) -> Result<()> {
    let (manifest, _) = runtime_cache.route_snapshot(config).await?;
    hmr_tracker.populate_from_manifest(&manifest.routes);
    Ok(())
}

/// Everything one accepted batch of filesystem events acts on.
///
/// Held by the coalescing thread rather than captured by the `notify` callback:
/// the callback now does nothing but filter and forward, so the work below runs
/// once per batch instead of once per raw OS event.
struct WatchBatchContext {
    config: ServerConfig,
    reload_tx: broadcast::Sender<String>,
    runtime_cache: Arc<RuntimeCache>,
    worker_pool: Arc<NodeWorkerPool>,
    render_cache: Arc<RenderCache>,
    hmr_tracker: Arc<HmrTracker>,
    plugin_runtime: Option<Arc<PluginHost>>,
    edit_traces: Arc<trace::TraceStore>,
    tokio_handle: tokio::runtime::Handle,
    root: PathBuf,
    roots: Arc<WatchRoots>,
}

pub(crate) fn start_watcher(
    root: &Path,
    watch_paths: &[PathBuf],
    runtime: WatcherRuntime,
) -> Result<RecommendedWatcher> {
    let WatcherRuntime {
        config,
        reload_tx,
        runtime_cache,
        worker_pool,
        render_cache,
        hmr_tracker,
        plugin_runtime,
        edit_traces,
        tokio_handle,
    } = runtime;
    let roots = Arc::new(WatchRoots::new(root));
    let context = WatchBatchContext {
        config,
        reload_tx,
        runtime_cache,
        worker_pool,
        render_cache,
        hmr_tracker,
        plugin_runtime,
        edit_traces,
        tokio_handle,
        root: root.to_path_buf(),
        roots: Arc::clone(&roots),
    };

    // The coalescer runs on a thread of its own, not on the one `notify` calls
    // back on and not on the Tokio runtime. The batch handler takes blocking
    // locks and broadcasts to every worker; leaving it on the notify thread is
    // what let one save stall the events behind it, and moving it onto an async
    // worker would block a thread the render pipeline needs.
    let (events_tx, events_rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    std::thread::Builder::new()
        .name("ruvyxa-watch-coalescer".to_string())
        .spawn(move || {
            coalesce_watch_events(
                events_rx,
                WATCH_COALESCE_WINDOW,
                WATCH_COALESCE_MAX_WINDOW,
                |paths| handle_watch_batch(&context, paths),
            );
        })
        .map_err(|error| {
            RuvyxaError::Message(format!("Failed to start the watcher coalescer: {error}"))
        })?;

    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let paths = event
                    .paths
                    .into_iter()
                    .filter(|path| !ignored_watch_path(&roots, path))
                    .collect::<Vec<_>>();
                if paths.is_empty() {
                    return;
                }
                // Dropping the watcher drops this sender, which is what stops
                // the coalescing thread.
                let _ = events_tx.send(paths);
            }
            Err(error) => {
                println!("✖ File watcher failed (0ms)");
                println!("  Reason: {error}");
                println!(
                    "  Watcher remains active; refresh the browser after the next detected change."
                );
                warn!(%error, "file watcher error");
            }
        })
        .map_err(|error| RuvyxaError::Message(format!("Failed to start file watcher: {error}")))?;

    for path in watch_paths {
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|error| {
                RuvyxaError::Message(format!("Failed to watch {}: {error}", path.display()))
            })?;
    }

    Ok(watcher)
}

/// How long the watcher waits for more events before acting on the ones it has.
///
/// One Ctrl-S is not one event. Windows `ReadDirectoryChangesW` reports a single
/// write as several `Modify` notifications, and an atomic-save editor emits a
/// rename pair on top of that, so the callback fires two or three times for one
/// logical edit. Every one of those used to run the whole invalidation — a
/// render-cache flush, an NDJSON broadcast to every worker, an HMR frame — and
/// for an instrumentation file, a full worker recycle.
///
/// Short on purpose: the window is added to every HMR update's latency, and a
/// batch that spans a stylesheet and a component classifies as a component
/// update, losing the hot style swap. 50 ms absorbs the duplicate reports of one
/// save without being long enough to merge two deliberate ones.
const WATCH_COALESCE_WINDOW: Duration = Duration::from_millis(50);

/// The longest one batch may keep growing.
///
/// A continuous stream of events — `git checkout`, a build writing into a
/// directory that is not on the ignore list — would otherwise keep resetting the
/// window and never flush, which is the same stall in a different shape.
const WATCH_COALESCE_MAX_WINDOW: Duration = Duration::from_millis(250);

/// What the coalescer's event source has for it right now.
enum WatchEvents {
    /// Paths that arrived within the window.
    Batch(Vec<PathBuf>),
    /// Nothing arrived before the window expired.
    Idle,
    /// The watcher was dropped; no further events can arrive.
    Closed,
}

/// The stream of accepted watcher paths, as the coalescing loop consumes it.
///
/// A trait so the loop can be driven by a test without waiting on a clock: a
/// debounce window is a timing contract, and a test that sleeps for one is a
/// test that fails on a loaded CI machine.
trait WatchEventSource {
    /// Block until the next batch arrives, or return `None` once the watcher is
    /// gone.
    fn next_batch(&mut self) -> Option<Vec<PathBuf>>;
    /// Wait up to `window` for another batch.
    fn next_batch_within(&mut self, window: Duration) -> WatchEvents;
}

impl WatchEventSource for std::sync::mpsc::Receiver<Vec<PathBuf>> {
    fn next_batch(&mut self) -> Option<Vec<PathBuf>> {
        self.recv().ok()
    }

    fn next_batch_within(&mut self, window: Duration) -> WatchEvents {
        match self.recv_timeout(window) {
            Ok(paths) => WatchEvents::Batch(paths),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => WatchEvents::Idle,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => WatchEvents::Closed,
        }
    }
}

/// Merge the raw events of one edit into a single update, then act on it once.
///
/// Runs until the source closes. Every path that arrives while the window is
/// open joins the batch the handler is called with, so the classification, the
/// cache invalidation, and the HMR frame all see one logical edit — exactly as
/// `hmr_update_kind` already expects to.
fn coalesce_watch_events<S, F>(
    mut source: S,
    window: Duration,
    max_window: Duration,
    mut on_batch: F,
) where
    S: WatchEventSource,
    F: FnMut(Vec<PathBuf>),
{
    while let Some(first) = source.next_batch() {
        let mut batch = first;
        let deadline = std::time::Instant::now() + max_window;
        let mut closed = false;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match source.next_batch_within(window.min(remaining)) {
                WatchEvents::Batch(more) => batch.extend(more),
                WatchEvents::Idle => break,
                WatchEvents::Closed => {
                    closed = true;
                    break;
                }
            }
        }

        // One save reports the same file two or three times, and the batch is
        // the unit every consumer downstream counts in: the trace id, the HMR
        // `paths` array, and the worker invalidation list.
        batch.sort();
        batch.dedup();
        if !batch.is_empty() {
            on_batch(batch);
        }
        if closed {
            return;
        }
    }
}

/// Act on one coalesced edit: invalidate, reconcile, and send one HMR frame.
fn handle_watch_batch(context: &WatchBatchContext, paths: Vec<PathBuf>) {
    let WatchBatchContext {
        config,
        reload_tx,
        runtime_cache,
        worker_pool,
        render_cache,
        hmr_tracker,
        plugin_runtime,
        edit_traces,
        tokio_handle,
        root,
        roots,
    } = context;
    let instrumentation_changed = instrumentation_source_changed(roots, &paths);

    // Use HmrTracker for selective invalidation.
    let mut hmr_update = hmr_tracker.compute_update(&paths);
    if hmr_update.full_reload || instrumentation_changed {
        hmr_update.full_reload = true;
        hmr_update.event_type = HmrEventType::FullReload;
    }
    // Selective cache invalidation based on affected routes.
    let rediscover = hmr_update.full_reload || hmr_update.affected_routes.is_empty();
    invalidate_runtime_caches(runtime_cache, rediscover, &paths);
    if rediscover {
        render_cache.invalidate_all_blocking();
        let refresh_config = config.clone();
        let refresh_cache = runtime_cache.clone();
        let refresh_tracker = hmr_tracker.clone();
        tokio_handle.spawn(async move {
            if let Err(error) =
                refresh_hmr_manifest(&refresh_config, &refresh_cache, &refresh_tracker).await
            {
                warn!(%error, "HMR route manifest refresh failed");
            }
        });
    } else {
        // Selectively invalidate render cache for affected routes only.
        for route_path in &hmr_update.affected_routes {
            render_cache.invalidate_route_blocking(route_path);
        }
    }

    // Invalidate worker bundle caches for changed files.
    let path_strings: Vec<String> = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    let hmr_paths = paths
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    let trace_id = trace::edit_id(&hmr_paths);
    let trace_kind = hmr_update_kind(&hmr_update, &hmr_paths);
    edit_traces.start(
        &trace_id,
        &hmr_paths,
        &hmr_update.affected_routes,
        trace_kind,
    );
    edit_traces.record(
        &trace_id,
        "cache",
        if hmr_update.full_reload {
            "all route and render caches invalidated"
        } else {
            "affected route and style caches invalidated"
        },
    );
    info!(
        trace_id = %trace_id,
        files = hmr_paths.len(),
        routes = hmr_update.affected_routes.len(),
        "HMR edit accepted"
    );
    let worker_result = (!instrumentation_changed)
        .then(|| worker_pool.invalidate_from_watcher(path_strings.clone(), Some(&trace_id)));
    match &worker_result {
        Some(Ok(workers)) => {
            edit_traces.record(
                &trace_id,
                "worker",
                format!("queued invalidation for {workers} workers"),
            );
        }
        Some(Err(error)) => {
            edit_traces.record(&trace_id, "worker", format!("invalidation failed: {error}"));
        }
        None => {
            edit_traces.record(
                &trace_id,
                "worker",
                "instrumentation change requires recycle",
            );
        }
    }
    if worker_result.as_ref().is_some_and(|result| result.is_err()) {
        hmr_update.full_reload = true;
        hmr_update.event_type = HmrEventType::FullReload;
    }

    if let Some(plugin_runtime) = plugin_runtime.clone() {
        let plugin_paths = plugin_watch_paths(root, &paths);
        tokio_handle.spawn(async move {
            if let Err(error) = plugin_runtime.notify_file_change(&plugin_paths).await {
                warn!(%error, "plugin dev.fileChange hook failed");
            }
        });
    }

    if instrumentation_changed {
        let recycle_pool = Arc::clone(worker_pool);
        let recycle_reload = reload_tx.clone();
        let restart_payload = hmr_payload(
            &hmr_update,
            &hmr_paths,
            &trace_id,
            config.debug_traces,
            None,
        );
        let issue_update = hmr_update.clone();
        let issue_paths = hmr_paths.clone();
        let issue_trace = trace_id.clone();
        let trace_store = Arc::clone(edit_traces);
        let trace_ack = config.debug_traces;
        tokio_handle.spawn(async move {
            let payload = match recycle_pool.recycle().await {
                Ok(workers) => {
                    info!(workers, "recycled workers after instrumentation change");
                    trace_store.record(
                        &issue_trace,
                        "worker",
                        format!("recycled {workers} workers"),
                    );
                    restart_payload
                }
                Err(error) => {
                    warn!(%error, "worker recycle after instrumentation change failed");
                    trace_store.record(&issue_trace, "worker", format!("recycle failed: {error}"));
                    hmr_payload(
                        &issue_update,
                        &issue_paths,
                        &issue_trace,
                        trace_ack,
                        Some((
                            "RUV1707",
                            "Worker restart failed; restart the development server.",
                        )),
                    )
                }
            };
            let _ = recycle_reload.send(payload);
            trace_store.record(&issue_trace, "hmr", "message broadcast");
        });
    } else if let Some(Err(error)) = worker_result {
        warn!(%error, "worker invalidation failed; recycling workers");
        let recycle_pool = Arc::clone(worker_pool);
        let recycle_reload = reload_tx.clone();
        let issue_update = hmr_update.clone();
        let issue_paths = hmr_paths.clone();
        let issue_trace = trace_id.clone();
        let trace_store = Arc::clone(edit_traces);
        let trace_ack = config.debug_traces;
        tokio_handle.spawn(async move {
            let issue = match recycle_pool.recycle().await {
                Ok(workers) => {
                    info!(workers, "recycled workers after invalidation failure");
                    trace_store.record(
                        &issue_trace,
                        "worker",
                        format!("recycled {workers} workers"),
                    );
                    (
                        "RUV1706",
                        "Worker cache invalidation failed; workers were restarted.",
                    )
                }
                Err(error) => {
                    warn!(%error, "worker recycle after invalidation failure failed");
                    trace_store.record(&issue_trace, "worker", format!("recycle failed: {error}"));
                    (
                        "RUV1707",
                        "Worker restart failed; restart the development server.",
                    )
                }
            };
            let payload = hmr_payload(
                &issue_update,
                &issue_paths,
                &issue_trace,
                trace_ack,
                Some(issue),
            );
            let _ = recycle_reload.send(payload);
            trace_store.record(&issue_trace, "hmr", "message broadcast");
        });
    } else {
        // Send targeted HMR payload with affected routes.
        let payload = hmr_payload(
            &hmr_update,
            &hmr_paths,
            &trace_id,
            config.debug_traces,
            None,
        );
        let _ = reload_tx.send(payload);
        edit_traces.record(&trace_id, "hmr", "message broadcast");
    }
}

/// Serialize the HMR wire message in one place so the watcher, browser runtime,
/// and shared contract fixture cannot drift independently.
static NEXT_HMR_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn hmr_update_kind(update: &HmrUpdate, paths: &[String]) -> &'static str {
    if update.full_reload {
        return "restart";
    }
    match update.event_type {
        HmrEventType::CssUpdate => "css",
        HmrEventType::ComponentUpdate => {
            let server_route = paths.iter().any(|path| {
                path.split('/').any(|segment| segment == "server")
                    || path.ends_with("/action.ts")
                    || path.ends_with("/action.js")
                    || path.ends_with("/route.ts")
                    || path.ends_with("/route.js")
            });
            if server_route {
                "server-route"
            } else {
                "client-boundary"
            }
        }
        HmrEventType::FullReload => "restart",
    }
}

/// An HMR issues message for a failure that no file change produced.
///
/// A client bundle is built when the browser asks for it, not when a file is
/// saved, so a bundling failure had no watcher event to travel on. It was
/// answered with a 500 on the script URL — which a browser reports as a script
/// that failed to load and nothing more — while the document around it stayed a
/// perfectly ordinary 200. The page then sat there server-rendered and inert,
/// and the only trace of the real error was a line in the terminal.
///
/// Sent over the channel the overlay already listens on, with `fullReload`
/// false: reloading would re-request the same failing bundle and do it again.
pub(crate) fn hmr_issue_payload(code: &str, message: &str, route: &str) -> String {
    let sequence = NEXT_HMR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    serde_json::json!({
        "protocol": "ruvyxa.hmr",
        "protocolVersion": 1,
        "sequence": sequence,
        "traceId": trace::edit_id(std::slice::from_ref(&route.to_string())),
        "traceAck": false,
        "type": "issues",
        "kind": "issues",
        "modules": [],
        "paths": [],
        "affectedRoutes": [route],
        "fullReload": false,
        "issues": [{ "code": code, "message": message }],
    })
    .to_string()
}

fn hmr_payload(
    update: &HmrUpdate,
    paths: &[String],
    trace_id: &str,
    trace_ack: bool,
    issue: Option<(&'static str, &'static str)>,
) -> String {
    let sequence = NEXT_HMR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let (message_type, kind) = if issue.is_some() {
        ("issues", "issues")
    } else {
        let kind = hmr_update_kind(update, paths);
        if kind == "restart" {
            ("restart", kind)
        } else {
            ("partial", kind)
        }
    };
    let mut payload = serde_json::json!({
        "protocol": "ruvyxa.hmr",
        "protocolVersion": 1,
        "sequence": sequence,
        "traceId": trace_id,
        "traceAck": trace_ack,
        "type": message_type,
        "kind": kind,
        "modules": paths,
        "paths": paths,
        "affectedRoutes": update.affected_routes,
        "fullReload": update.full_reload,
    });
    if let Some((code, message)) = issue {
        payload["issues"] = serde_json::json!([{ "code": code, "message": message }]);
    }
    payload.to_string()
}

pub(crate) fn watch_paths(config: &ServerConfig) -> Vec<PathBuf> {
    let mut paths = vec![config.root.clone()];
    paths.retain(|path| path.exists());
    paths.sort();
    paths.dedup();
    paths
}

// Every filesystem canonicalization the watcher performs, counted per thread so
// a test can assert how many syscalls one event costs. Thread-local rather than
// global: the test binary runs its tests in parallel threads, and a shared
// counter would be measuring every other test too.
#[cfg(test)]
thread_local! {
    static PATH_CANONICALIZATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The one canonicalization the watcher performs, funnelled through a single
/// function so its cost is countable.
fn canonicalized(path: &Path) -> PathBuf {
    #[cfg(test)]
    PATH_CANONICALIZATIONS.with(|count| count.set(count.get() + 1));
    ruvyxa_diagnostics::normalized_canonical_path(path)
}

/// The project root in the two forms every watcher event needs it in.
///
/// A filesystem event carries an absolute path that may be reported in either
/// the raw or the canonical spelling of the root, so both are kept and both are
/// tried. Neither changes for the life of the watcher: `notify`'s handle is
/// bound to the directory it was registered on, so a root that is moved, or a
/// root symlink retargeted, stops delivering events entirely rather than
/// delivering them under a new canonical name. Recomputing the canonical form
/// per event could therefore only ever produce the same answer — at the price
/// of a `canonicalize` syscall on the single thread `notify` calls back on.
pub(crate) struct WatchRoots {
    raw: PathBuf,
    canonical: PathBuf,
}

impl WatchRoots {
    fn new(root: &Path) -> Self {
        Self {
            raw: root.to_path_buf(),
            canonical: canonicalized(root),
        }
    }

    /// The path with the project root stripped, whichever spelling of the root
    /// the event used.
    fn relativize<'a>(&self, path: &'a Path) -> &'a Path {
        if path.is_absolute() {
            path.strip_prefix(&self.canonical)
                .or_else(|_| path.strip_prefix(&self.raw))
                .unwrap_or(path)
        } else {
            path.strip_prefix(Path::new(".")).unwrap_or(path)
        }
    }
}

fn ignored_watch_path(roots: &WatchRoots, path: &Path) -> bool {
    let components = roots
        .relativize(path)
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();
    let top_level_ignored = components.first().is_some_and(|component| {
        matches!(
            component.as_ref(),
            ".git" | ".ruvyxa" | "target" | "dist" | ".npm-pack" | ".npm-smoke"
        ) || component.starts_with(".ruvyxa-")
    });
    top_level_ignored
        || components
            .iter()
            .any(|component| matches!(component.as_ref(), ".ruvyxa" | "node_modules"))
}

/// Which cached answers one watcher event invalidates.
///
/// `rediscover` is the event class that may have added or removed a route, and
/// it drops everything. The other class is a component edit: it changes neither
/// the route manifest nor necessarily the collected CSS, so both are kept and
/// only the stylesheets that actually read a changed file are refreshed.
///
/// The client route table is dropped either way, and that is the whole reason
/// this is a function rather than two lines inside the branch. The table carries
/// each route's bundle hash, which is what the client router compares against to
/// decide whether the bundle the browser already holds is current — and a
/// component edit changes exactly that while changing nothing else the selective
/// branch invalidates. Kept across one, the table tells the router that stale
/// bundle is fine and the next soft navigation renders the code from before the
/// save.
fn invalidate_runtime_caches(runtime_cache: &RuntimeCache, rediscover: bool, paths: &[PathBuf]) {
    runtime_cache.invalidate_client_routes();
    if rediscover {
        runtime_cache.invalidate();
    } else {
        runtime_cache.invalidate_styles_for_paths(paths);
    }
}

fn instrumentation_source_changed(roots: &WatchRoots, paths: &[PathBuf]) -> bool {
    let root = &roots.canonical;
    paths.iter().any(|path| {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        if !INSTRUMENTATION_FILES.contains(&file_name) {
            return false;
        }
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        absolute.parent().map(canonicalized).as_deref() == Some(root.as_path())
    })
}

fn plugin_watch_paths(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.strip_prefix(root).unwrap_or(path))
        .map(|path| path.display().to_string().replace('\\', "/"))
        .collect()
}

pub(crate) fn format_update_elapsed(elapsed: Duration) -> String {
    if elapsed >= Duration::from_millis(1) {
        return format!("{}ms", elapsed.as_millis());
    }
    let tenths = elapsed.as_micros().div_ceil(100).max(1);
    format!("{}.{:01}ms", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_hmr_events_by_changed_file_type() {
        assert_eq!(
            classify_hmr_event(&[PathBuf::from("app/global.css")]),
            "css-update"
        );
        assert_eq!(
            classify_hmr_event(&[PathBuf::from("components/Nav.tsx")]),
            "component-update"
        );
        assert_eq!(
            classify_hmr_event(&[PathBuf::from("server/db.ts")]),
            "full-reload"
        );
        assert_eq!(
            classify_hmr_event(&[PathBuf::from("app/docs/page.mdx")]),
            "component-update"
        );
    }

    #[test]
    fn hmr_payload_matches_the_shared_wire_contract() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/hmr-contract.json"))
                .unwrap();
        assert_eq!(fixture["protocol"], "ruvyxa.hmr");
        assert_eq!(fixture["protocolVersion"], 1);
        assert_eq!(fixture["fallback"], "reload");
        let required_fields = fixture["requiredFields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect::<BTreeSet<_>>();

        for event in fixture["messages"].as_array().unwrap() {
            let (event_type, full_reload, path) = match event["event"].as_str().unwrap() {
                "css" => (HmrEventType::CssUpdate, false, "app/global.css"),
                "client" => (HmrEventType::ComponentUpdate, false, "app/Button.tsx"),
                "server" => (HmrEventType::ComponentUpdate, false, "app/server/data.ts"),
                "structural" => (HmrEventType::FullReload, true, "app/layout.tsx"),
                "failure" => (HmrEventType::FullReload, true, "app/page.tsx"),
                kind => panic!("unknown fixture HMR event: {kind}"),
            };
            let update = HmrUpdate {
                affected_routes: vec!["/docs".to_string()],
                full_reload,
                changed_files: vec![PathBuf::from(path)],
                event_type,
            };
            let payload: serde_json::Value = serde_json::from_str(&hmr_payload(
                &update,
                &[path.to_string()],
                "0123456789abcdef0123456789abcdef",
                false,
                (event["event"] == "failure").then_some((
                    "RUV1706",
                    "Worker cache invalidation failed; workers were restarted.",
                )),
            ))
            .unwrap();
            let actual_fields: BTreeSet<&str> = payload
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert!(required_fields.is_subset(&actual_fields));
            assert_eq!(payload["protocol"], fixture["protocol"]);
            assert_eq!(payload["protocolVersion"], fixture["protocolVersion"]);
            assert_eq!(payload["traceId"], "0123456789abcdef0123456789abcdef");
            assert_eq!(payload["type"], event["type"]);
            assert_eq!(payload["kind"], event["kind"]);
            if event["event"] == "failure" {
                assert_eq!(payload["issues"][0]["code"], "RUV1706");
                assert_eq!(payload["fullReload"], true);
            }
            assert!(
                payload["sequence"]
                    .as_u64()
                    .is_some_and(|sequence| sequence > 0)
            );
        }
    }

    /// A component edit takes the selective branch, and the selective branch has
    /// to drop the client route table anyway.
    ///
    /// That branch exists to keep the route manifest and the collected CSS
    /// across an edit that changes neither — but the table it used to keep as
    /// well carries each route's bundle hash, which is exactly what such an edit
    /// changes. The client router reads that hash to decide whether the bundle
    /// the browser already holds is current, so keeping the table told it the
    /// pre-edit bundle was fine and the next soft navigation rendered the code
    /// from before the save.
    #[tokio::test]
    async fn a_selective_watcher_event_still_drops_the_client_route_table() {
        for rediscover in [false, true] {
            let cache = Arc::new(RuntimeCache::default());
            let generation = cache
                .cached_client_routes()
                .await
                .expect_err("a fresh cache holds no table");
            cache
                .store_client_routes(generation, Arc::from(r#"{"routes":[]}"#))
                .await
                .expect("the table installs against its own generation");
            assert!(
                cache.cached_client_routes().await.is_ok(),
                "the table must be readable before the event"
            );

            // The watcher invalidates from its own thread, which is why these
            // take the blocking lock and cannot be called on the async runtime.
            let invalidated = Arc::clone(&cache);
            tokio::task::spawn_blocking(move || {
                invalidate_runtime_caches(&invalidated, rediscover, &[PathBuf::from("a.tsx")]);
            })
            .await
            .unwrap();

            assert!(
                cache.cached_client_routes().await.is_err(),
                "rediscover={rediscover}: the table must not survive a watcher event"
            );
        }
    }

    #[tokio::test]
    async fn refresh_hmr_manifest_reconciles_routes_without_losing_bundle_inputs() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        let home_page = app.join("page.tsx");
        std::fs::write(
            &home_page,
            "export default function Home() { return <main /> }",
        )
        .unwrap();

        let config = ServerConfig::dev(temp.path(), "localhost", 3000);
        let initial = discover_routes(discover_options(&config)).unwrap();
        let cache = RuntimeCache::with_manifest(initial.clone());
        let tracker = HmrTracker::new();
        tracker.populate_from_manifest(&initial.routes);

        let server_dependency = temp.path().join("lib").join("home-data.ts");
        tracker.register_route("/", std::slice::from_ref(&server_dependency));

        let about = app.join("about");
        std::fs::create_dir_all(&about).unwrap();
        let about_page = about.join("page.tsx");
        std::fs::write(
            &about_page,
            "export default function About() { return <main /> }",
        )
        .unwrap();

        cache.invalidate_async().await;
        refresh_hmr_manifest(&config, &cache, &tracker)
            .await
            .unwrap();

        assert_eq!(tracker.tracked_route_count(), 2);
        assert_eq!(
            tracker.compute_update(&[server_dependency]).affected_routes,
            vec!["/".to_string()],
            "refreshing route discovery must preserve a live route's worker graph"
        );
        assert_eq!(
            tracker.compute_update(&[about_page]).affected_routes,
            vec!["/about".to_string()],
            "new routes must become targetable immediately after manifest refresh"
        );
    }

    #[test]
    fn watches_the_project_root_for_imported_modules_and_styles() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let styles = temp.path().join("styles");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(app.join("page.tsx"), "import '../styles/site.css'").unwrap();
        std::fs::write(styles.join("site.css"), "body { color: green; }").unwrap();
        let config = ServerConfig::dev(temp.path(), "localhost", 3000);

        let roots = WatchRoots::new(temp.path());
        assert_eq!(watch_paths(&config), vec![temp.path().to_path_buf()]);
        assert!(!ignored_watch_path(&roots, &styles.join("site.css")));
        assert!(!ignored_watch_path(
            &roots,
            &temp.path().join("lib/utils.ts")
        ));
        assert!(ignored_watch_path(
            &roots,
            &temp.path().join("node_modules/react/index.js")
        ));
        assert!(ignored_watch_path(
            &roots,
            &temp.path().join(".ruvyxa/cache/client.js")
        ));
        assert!(ignored_watch_path(
            &roots,
            &Path::new(".")
                .join(".ruvyxa")
                .join("cache")
                .join("ssr")
                .join("page.mjs")
        ));
        assert!(ignored_watch_path(
            &roots,
            &temp
                .path()
                .join(".ruvyxa-action-test-BW9IHB")
                .join("app/todos/action.ts")
        ));
        assert!(!ignored_watch_path(
            &roots,
            &temp.path().join("app/.ruvyxa-action-test-helper.ts")
        ));
    }

    /// A scripted event source, so the coalescing rules can be asserted without
    /// waiting on a clock.
    ///
    /// `next_batch` blocks in production, which is what an idle gap means to it:
    /// nothing to do yet. The script models that by skipping past `Idle` until
    /// either a batch or the end of the script arrives.
    struct ScriptedEvents(std::collections::VecDeque<WatchEvents>);

    impl ScriptedEvents {
        fn new(script: impl IntoIterator<Item = WatchEvents>) -> Self {
            Self(script.into_iter().collect())
        }
    }

    impl WatchEventSource for ScriptedEvents {
        fn next_batch(&mut self) -> Option<Vec<PathBuf>> {
            loop {
                match self.0.pop_front()? {
                    WatchEvents::Batch(paths) => return Some(paths),
                    WatchEvents::Idle => continue,
                    WatchEvents::Closed => return None,
                }
            }
        }

        fn next_batch_within(&mut self, _window: Duration) -> WatchEvents {
            self.0.pop_front().unwrap_or(WatchEvents::Closed)
        }
    }

    fn batch(path: &str) -> WatchEvents {
        WatchEvents::Batch(vec![PathBuf::from(path)])
    }

    fn coalesced(script: impl IntoIterator<Item = WatchEvents>) -> Vec<Vec<PathBuf>> {
        let mut updates = Vec::new();
        coalesce_watch_events(
            ScriptedEvents::new(script),
            WATCH_COALESCE_WINDOW,
            WATCH_COALESCE_MAX_WINDOW,
            |paths| updates.push(paths),
        );
        updates
    }

    /// One editor save is not one filesystem event.
    ///
    /// Windows reports a single write as several `Modify` notifications and an
    /// atomic-save editor adds a rename pair, so the callback fired two or three
    /// times per Ctrl-S. Each one ran the whole invalidation: a render-cache
    /// flush, an invalidation broadcast to every worker, an HMR frame, and — for
    /// an instrumentation file — a complete worker recycle.
    #[test]
    fn the_repeated_events_of_one_save_become_one_update() {
        let save = || {
            [
                batch("app/page.tsx"),
                batch("app/page.tsx"),
                batch("app/page.tsx"),
                WatchEvents::Idle,
            ]
        };

        // What the watcher did before it coalesced: no window, so every raw
        // report ran the whole invalidation on its own.
        let mut uncoalesced = 0;
        coalesce_watch_events(
            ScriptedEvents::new(save()),
            WATCH_COALESCE_WINDOW,
            Duration::ZERO,
            |_| uncoalesced += 1,
        );
        assert_eq!(uncoalesced, 3);

        assert_eq!(
            coalesced(save()),
            vec![vec![PathBuf::from("app/page.tsx")]],
            "three reports of one save must produce one update carrying that file once"
        );
    }

    /// Coalescing may not swallow a second deliberate edit. The window closes on
    /// the first idle gap, and everything after it is its own update.
    #[test]
    fn edits_separated_by_an_idle_window_stay_separate_updates() {
        let updates = coalesced([
            batch("app/page.tsx"),
            WatchEvents::Idle,
            batch("app/about/page.tsx"),
            WatchEvents::Idle,
        ]);

        assert_eq!(
            updates,
            vec![
                vec![PathBuf::from("app/page.tsx")],
                vec![PathBuf::from("app/about/page.tsx")],
            ]
        );
    }

    /// A batch that spans several files reaches the handler whole, because
    /// `hmr_update_kind` classifies per batch and has to see all of it.
    #[test]
    fn a_batch_carries_every_distinct_path_it_absorbed() {
        let updates = coalesced([
            batch("app/page.tsx"),
            batch("app/global.css"),
            batch("app/page.tsx"),
            WatchEvents::Idle,
        ]);

        assert_eq!(
            updates,
            vec![vec![
                PathBuf::from("app/global.css"),
                PathBuf::from("app/page.tsx"),
            ]],
            "the batch keeps both files, in a deterministic order, without duplicates"
        );
    }

    /// An unbroken stream of events must not hold a batch open forever. Without
    /// the ceiling, a `git checkout` writing continuously would keep resetting
    /// the window and no update would ever be sent.
    #[test]
    fn a_continuous_event_stream_still_flushes() {
        let mut updates = 0;
        coalesce_watch_events(
            ScriptedEvents::new([
                batch("a.tsx"),
                batch("b.tsx"),
                batch("c.tsx"),
                batch("d.tsx"),
            ]),
            WATCH_COALESCE_WINDOW,
            Duration::ZERO,
            |_| updates += 1,
        );

        assert_eq!(
            updates, 4,
            "with no room left in the window every event must flush on its own"
        );
    }

    /// The project root is canonicalized once for the life of the watcher, not
    /// once per path in every event.
    ///
    /// `notify` delivers on a single OS thread, so a syscall inside the
    /// per-path filter is paid before the next event can even be looked at. A
    /// `pnpm install` inside the project produces tens of thousands of
    /// `node_modules` events that are all discarded — after paying for a
    /// `canonicalize` each, on the one thread every real edit also queues
    /// behind.
    #[test]
    fn one_watcher_event_canonicalizes_the_project_root_once() {
        let temp = tempfile::tempdir().unwrap();
        let paths = (0..500)
            .map(|index| {
                temp.path()
                    .join(format!("node_modules/.pnpm/pkg-{index}/index.js"))
            })
            .collect::<Vec<_>>();

        PATH_CANONICALIZATIONS.with(|count| count.set(0));
        let roots = WatchRoots::new(temp.path());
        for path in &paths {
            assert!(
                ignored_watch_path(&roots, path),
                "a node_modules path must be discarded"
            );
        }
        assert!(!instrumentation_source_changed(&roots, &paths));

        assert_eq!(
            PATH_CANONICALIZATIONS.with(std::cell::Cell::get),
            1,
            "one event over {} paths must canonicalize the root once",
            paths.len()
        );
    }

    #[test]
    fn instrumentation_watcher_filenames_match_the_shared_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/instrumentation-files-conformance.json"
        ))
        .unwrap();
        let files = fixture["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(files, INSTRUMENTATION_FILES);

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let roots = WatchRoots::new(root);
        for file in INSTRUMENTATION_FILES {
            assert!(instrumentation_source_changed(&roots, &[root.join(file)]));
        }
        assert!(!instrumentation_source_changed(
            &roots,
            &[root.join("app/instrumentation.ts")]
        ));
        assert!(!instrumentation_source_changed(
            &roots,
            &[root.join("instrumentation.ts.bak")]
        ));
    }

    #[test]
    fn plugin_file_change_paths_are_project_relative_and_portable() {
        let root = PathBuf::from("C:/workspace/app");
        let paths = vec![root.join("content/guide.md"), root.join("app/page.tsx")];

        assert_eq!(
            plugin_watch_paths(&root, &paths),
            vec!["content/guide.md", "app/page.tsx"]
        );
    }

    #[test]
    fn dev_hmr_logs_keep_submillisecond_timing_visible() {
        assert_eq!(format_update_elapsed(Duration::from_micros(42)), "0.1ms");
        assert_eq!(format_update_elapsed(Duration::from_millis(1)), "1ms");
    }

    fn classify_hmr_event(paths: &[PathBuf]) -> &'static str {
        if paths.is_empty() {
            return "full-reload";
        }

        if paths.iter().all(|path| extension_is(path, "css")) {
            return "css-update";
        }

        let has_component = paths.iter().any(|path| {
            ["tsx", "jsx", "ts", "js", "md", "mdx"]
                .into_iter()
                .any(|extension| extension_is(path, extension))
                && path.components().any(|component| {
                    let segment = component.as_os_str().to_string_lossy();
                    segment == "app" || segment == "components"
                })
        });

        if has_component {
            "component-update"
        } else {
            "full-reload"
        }
    }

    fn extension_is(path: &Path, expected: &str) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
    }
}
