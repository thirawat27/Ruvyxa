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
    let root = root.to_path_buf();
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let paths = event
                    .paths
                    .into_iter()
                    .filter(|path| !ignored_watch_path(&root, path))
                    .collect::<Vec<_>>();
                if paths.is_empty() {
                    return;
                }
                let instrumentation_changed = instrumentation_source_changed(&root, &paths);

                // Use HmrTracker for selective invalidation.
                let mut hmr_update = hmr_tracker.compute_update(&paths);
                if hmr_update.full_reload || instrumentation_changed {
                    hmr_update.full_reload = true;
                    hmr_update.event_type = HmrEventType::FullReload;
                }
                // Selective cache invalidation based on affected routes.
                if hmr_update.full_reload || hmr_update.affected_routes.is_empty() {
                    // Full invalidation: manifest may have changed (new/deleted routes).
                    runtime_cache.invalidate();
                    render_cache.invalidate_all_blocking();
                    let refresh_config = config.clone();
                    let refresh_cache = runtime_cache.clone();
                    let refresh_tracker = hmr_tracker.clone();
                    tokio_handle.spawn(async move {
                        if let Err(error) =
                            refresh_hmr_manifest(&refresh_config, &refresh_cache, &refresh_tracker)
                                .await
                        {
                            warn!(%error, "HMR route manifest refresh failed");
                        }
                    });
                } else {
                    // Selective invalidation: only evict affected route caches.
                    // Refresh styles only when the current CSS dependency graph
                    // intersects a changed path. Component-only updates retain it.
                    runtime_cache.invalidate_styles_for_paths(&paths);

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
                        path.strip_prefix(&root)
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
                let worker_result = (!instrumentation_changed).then(|| {
                    worker_pool.invalidate_from_watcher(path_strings.clone(), Some(&trace_id))
                });
                match &worker_result {
                    Some(Ok(workers)) => {
                        edit_traces.record(
                            &trace_id,
                            "worker",
                            format!("queued invalidation for {workers} workers"),
                        );
                    }
                    Some(Err(error)) => {
                        edit_traces.record(
                            &trace_id,
                            "worker",
                            format!("invalidation failed: {error}"),
                        );
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
                    let plugin_paths = plugin_watch_paths(&root, &paths);
                    tokio_handle.spawn(async move {
                        if let Err(error) = plugin_runtime.notify_file_change(&plugin_paths).await {
                            warn!(%error, "plugin dev.fileChange hook failed");
                        }
                    });
                }

                if instrumentation_changed {
                    let recycle_pool = Arc::clone(&worker_pool);
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
                    let trace_store = Arc::clone(&edit_traces);
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
                                trace_store.record(
                                    &issue_trace,
                                    "worker",
                                    format!("recycle failed: {error}"),
                                );
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
                    let recycle_pool = Arc::clone(&worker_pool);
                    let recycle_reload = reload_tx.clone();
                    let issue_update = hmr_update.clone();
                    let issue_paths = hmr_paths.clone();
                    let issue_trace = trace_id.clone();
                    let trace_store = Arc::clone(&edit_traces);
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
                                trace_store.record(
                                    &issue_trace,
                                    "worker",
                                    format!("recycle failed: {error}"),
                                );
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

fn ignored_watch_path(root: &Path, path: &Path) -> bool {
    let canonical_root = ruvyxa_diagnostics::normalized_canonical_path(root);
    let relative = if path.is_absolute() {
        path.strip_prefix(&canonical_root)
            .or_else(|_| path.strip_prefix(root))
            .unwrap_or(path)
    } else {
        path.strip_prefix(Path::new(".")).unwrap_or(path)
    };
    let components = relative
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

fn instrumentation_source_changed(root: &Path, paths: &[PathBuf]) -> bool {
    let root = ruvyxa_diagnostics::normalized_canonical_path(root);
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
        absolute
            .parent()
            .map(ruvyxa_diagnostics::normalized_canonical_path)
            .as_deref()
            == Some(root.as_path())
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

        assert_eq!(watch_paths(&config), vec![temp.path().to_path_buf()]);
        assert!(!ignored_watch_path(temp.path(), &styles.join("site.css")));
        assert!(!ignored_watch_path(
            temp.path(),
            &temp.path().join("lib/utils.ts")
        ));
        assert!(ignored_watch_path(
            temp.path(),
            &temp.path().join("node_modules/react/index.js")
        ));
        assert!(ignored_watch_path(
            temp.path(),
            &temp.path().join(".ruvyxa/cache/client.js")
        ));
        assert!(ignored_watch_path(
            temp.path(),
            &Path::new(".")
                .join(".ruvyxa")
                .join("cache")
                .join("ssr")
                .join("page.mjs")
        ));
        assert!(ignored_watch_path(
            temp.path(),
            &temp
                .path()
                .join(".ruvyxa-action-test-BW9IHB")
                .join("app/todos/action.ts")
        ));
        assert!(!ignored_watch_path(
            temp.path(),
            &temp.path().join("app/.ruvyxa-action-test-helper.ts")
        ));
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
        for file in INSTRUMENTATION_FILES {
            assert!(instrumentation_source_changed(root, &[root.join(file)]));
        }
        assert!(!instrumentation_source_changed(
            root,
            &[root.join("app/instrumentation.ts")]
        ));
        assert!(!instrumentation_source_changed(
            root,
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
