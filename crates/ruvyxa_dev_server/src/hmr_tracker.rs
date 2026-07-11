//! Incremental HMR tracker: maps file dependencies to affected routes.
//!
//! Instead of invalidating all caches on every file change, this module
//! maintains a reverse mapping from source files to the route bundles that
//! include them. When a file changes, only the affected routes are
//! re-rendered, and only their cache entries are evicted.
//!
//! ## Performance impact
//!
//! For a 50-route app where one component file changes:
//! - Without tracking: invalidate all 50 route caches, re-render everything
//! - With tracking: invalidate 2-3 affected routes, re-render only those
//!
//! This reduces HMR latency from O(routes) to O(affected_routes), which
//! for most edits means sub-100ms updates regardless of project size.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

/// Tracks which source files affect which routes for incremental HMR.
#[derive(Debug, Clone)]
pub struct HmrTracker {
    /// Reverse map: source_file → set of route_paths that depend on it.
    file_to_routes: Arc<RwLock<BTreeMap<PathBuf, BTreeSet<String>>>>,
    /// Forward map: route_path → set of source_files in its graph.
    route_to_files: Arc<RwLock<BTreeMap<String, BTreeSet<PathBuf>>>>,
}

impl Default for HmrTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of computing affected routes from changed files.
#[derive(Debug, Clone)]
pub struct HmrUpdate {
    /// Route paths that need re-rendering (e.g., "/", "/blog/:slug").
    pub affected_routes: Vec<String>,
    /// Whether a full reload is needed (e.g., layout change affects all routes).
    pub full_reload: bool,
    /// The specific files that triggered this update.
    pub changed_files: Vec<PathBuf>,
    /// HMR event type for the client WebSocket message.
    pub event_type: HmrEventType,
}

/// Type of HMR event to send to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HmrEventType {
    /// Only CSS changed — can hot-replace stylesheets.
    CssUpdate,
    /// Specific components changed — can attempt React Fast Refresh.
    ComponentUpdate,
    /// Structural change (new route, layout change) — full page reload.
    FullReload,
}

impl HmrEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CssUpdate => "css-update",
            Self::ComponentUpdate => "component-update",
            Self::FullReload => "full-reload",
        }
    }
}

impl HmrTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self {
            file_to_routes: Arc::new(RwLock::new(BTreeMap::new())),
            route_to_files: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// Populate tracker from a route manifest (called at startup and on manifest change).
    pub fn populate_from_manifest(&self, routes: &[ruvyxa_graph::RouteEntry]) {
        let mut file_to_routes = self.file_to_routes.write();
        let mut route_to_files = self.route_to_files.write();
        file_to_routes.clear();
        route_to_files.clear();

        for route in routes {
            let mut files = BTreeSet::new();
            files.insert(normalize_source_path(&route.file));
            for layout in &route.layout_chain {
                files.insert(normalize_source_path(Path::new(layout)));
            }
            for server_module in &route.server_modules {
                files.insert(normalize_source_path(Path::new(server_module)));
            }
            for file in &files {
                file_to_routes
                    .entry(file.clone())
                    .or_default()
                    .insert(route.path.clone());
            }
            route_to_files.insert(route.path.clone(), files);
        }
    }

    /// Register a route's dependency graph.
    ///
    /// Called after a route is successfully rendered to record which source
    /// files contribute to its bundle. On subsequent file changes, the
    /// tracker can identify exactly which routes need re-rendering.
    pub fn register_route(&self, route_path: &str, source_files: &[PathBuf]) {
        let mut file_to_routes = self.file_to_routes.write();
        let mut route_to_files = self.route_to_files.write();

        // Clear previous mappings for this route (in case deps changed).
        if let Some(old_files) = route_to_files.get(route_path) {
            for file in old_files.iter() {
                if let Some(routes) = file_to_routes.get_mut(file) {
                    routes.remove(route_path);
                    if routes.is_empty() {
                        file_to_routes.remove(file);
                    }
                }
            }
        }

        // Insert new mappings.
        let file_set: BTreeSet<PathBuf> = source_files
            .iter()
            .map(|file| normalize_source_path(file))
            .collect();
        for file in &file_set {
            file_to_routes
                .entry(file.clone())
                .or_default()
                .insert(route_path.to_string());
        }
        route_to_files.insert(route_path.to_string(), file_set);
    }

    /// Compute which routes are affected by a set of changed files.
    ///
    /// Returns an `HmrUpdate` with the affected routes and the appropriate
    /// event type for the client.
    pub fn compute_update(&self, changed_paths: &[PathBuf]) -> HmrUpdate {
        if changed_paths.is_empty() {
            return HmrUpdate {
                affected_routes: Vec::new(),
                full_reload: true,
                changed_files: Vec::new(),
                event_type: HmrEventType::FullReload,
            };
        }

        let file_to_routes = self.file_to_routes.read();

        // Determine event type based on file extensions.
        let all_css = changed_paths
            .iter()
            .all(|p| extension_is(p, "css") || extension_is(p, "scss"));
        let has_layout_change = changed_paths.iter().any(|p| is_layout_file(p));

        let event_type = if all_css {
            HmrEventType::CssUpdate
        } else if has_layout_change {
            HmrEventType::FullReload
        } else {
            HmrEventType::ComponentUpdate
        };

        // Collect all affected routes.
        let mut affected_routes: BTreeSet<String> = BTreeSet::new();
        for path in changed_paths {
            let normalized = normalize_source_path(path);
            if let Some(routes) = file_to_routes.get(&normalized) {
                affected_routes.extend(routes.iter().cloned());
            }
        }

        // If a layout changed, all routes using that layout are affected.
        // If we couldn't determine specific routes, trigger full reload.
        let full_reload = has_layout_change
            || (affected_routes.is_empty() && !all_css && !file_to_routes.is_empty());

        HmrUpdate {
            affected_routes: affected_routes.into_iter().collect(),
            full_reload,
            changed_files: changed_paths.to_vec(),
            event_type,
        }
    }

    /// Invalidate all tracking data (called when route manifest changes).
    pub fn clear(&self) {
        self.file_to_routes.write().clear();
        self.route_to_files.write().clear();
    }

    /// Number of tracked files.
    pub fn tracked_file_count(&self) -> usize {
        self.file_to_routes.read().len()
    }

    /// Number of tracked routes.
    pub fn tracked_route_count(&self) -> usize {
        self.route_to_files.read().len()
    }

    /// Get the routes affected by a specific file.
    #[allow(dead_code)]
    pub fn routes_for_file(&self, path: &Path) -> Vec<String> {
        self.file_to_routes
            .read()
            .get(path)
            .map(|routes| routes.iter().cloned().collect())
            .unwrap_or_default()
    }
}

fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

fn normalize_source_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_layout_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with("layout."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_lookup_routes() {
        let tracker = HmrTracker::new();
        let utils = PathBuf::from("/app/utils.ts");
        let button = PathBuf::from("/app/components/Button.tsx");
        let page = PathBuf::from("/app/page.tsx");

        // Route "/" depends on page.tsx, Button.tsx, utils.ts
        tracker.register_route("/", &[page.clone(), button.clone(), utils.clone()]);
        // Route "/blog" depends on its own page + utils
        tracker.register_route(
            "/blog",
            &[PathBuf::from("/app/blog/page.tsx"), utils.clone()],
        );

        assert_eq!(tracker.tracked_route_count(), 2);
        assert_eq!(tracker.tracked_file_count(), 4); // page, button, utils, blog/page

        // Changing utils affects both routes.
        let update = tracker.compute_update(std::slice::from_ref(&utils));
        assert!(update.affected_routes.contains(&"/".to_string()));
        assert!(update.affected_routes.contains(&"/blog".to_string()));
        assert_eq!(update.affected_routes.len(), 2);
        assert_eq!(update.event_type, HmrEventType::ComponentUpdate);

        // Changing Button only affects "/".
        let update = tracker.compute_update(std::slice::from_ref(&button));
        assert_eq!(update.affected_routes, vec!["/"]);
    }

    #[test]
    fn css_only_change() {
        let tracker = HmrTracker::new();
        let css = PathBuf::from("/app/global.css");
        tracker.register_route("/", &[PathBuf::from("/app/page.tsx"), css.clone()]);

        let update = tracker.compute_update(&[css]);
        assert_eq!(update.event_type, HmrEventType::CssUpdate);
        assert!(!update.full_reload);
    }

    #[test]
    fn layout_change_triggers_full_reload() {
        let tracker = HmrTracker::new();
        let layout = PathBuf::from("/app/layout.tsx");
        tracker.register_route("/", &[PathBuf::from("/app/page.tsx"), layout.clone()]);

        let update = tracker.compute_update(&[layout]);
        assert_eq!(update.event_type, HmrEventType::FullReload);
        assert!(update.full_reload);
    }

    #[test]
    fn re_registration_updates_deps() {
        let tracker = HmrTracker::new();
        let old_dep = PathBuf::from("/app/old.ts");
        let new_dep = PathBuf::from("/app/new.ts");
        let page = PathBuf::from("/app/page.tsx");

        tracker.register_route("/", &[page.clone(), old_dep.clone()]);
        assert!(tracker
            .compute_update(std::slice::from_ref(&old_dep))
            .affected_routes
            .contains(&"/".to_string()));

        // Re-register with different deps.
        tracker.register_route("/", &[page.clone(), new_dep.clone()]);

        // Old dep should no longer affect "/".
        let update = tracker.compute_update(&[old_dep]);
        assert!(!update.affected_routes.contains(&"/".to_string()));

        // New dep should affect "/".
        let update = tracker.compute_update(&[new_dep]);
        assert!(update.affected_routes.contains(&"/".to_string()));
    }

    #[test]
    fn unknown_file_with_tracked_routes_triggers_full_reload() {
        let tracker = HmrTracker::new();
        tracker.register_route("/", &[PathBuf::from("/app/page.tsx")]);

        // A file not in any route's deps — triggers full reload since we can't
        // determine if it's safe to skip.
        let update = tracker.compute_update(&[PathBuf::from("/app/unknown.ts")]);
        assert!(update.full_reload);
    }

    #[test]
    fn empty_tracker_does_not_panic() {
        let tracker = HmrTracker::new();
        let update = tracker.compute_update(&[PathBuf::from("/app/page.tsx")]);
        // No routes tracked, so full_reload is false (no routes to reload),
        // but also no affected routes.
        assert!(update.affected_routes.is_empty());
    }

    #[test]
    fn normalizes_relative_and_absolute_existing_paths() {
        let current_dir = std::env::current_dir().unwrap();
        let temp = tempfile::Builder::new()
            .prefix("hmr-path-test-")
            .tempdir_in(&current_dir)
            .unwrap();
        let page = temp.path().join("page.tsx");
        std::fs::write(&page, "export default function Page() {}").unwrap();
        let tracker = HmrTracker::new();
        tracker.register_route("/", std::slice::from_ref(&page));

        let relative = page.strip_prefix(&current_dir).unwrap();
        let update = tracker.compute_update(&[relative.to_path_buf()]);
        assert_eq!(update.affected_routes, vec!["/"]);
        let update = tracker.compute_update(&[page.canonicalize().unwrap()]);
        assert_eq!(update.affected_routes, vec!["/"]);
    }
}
