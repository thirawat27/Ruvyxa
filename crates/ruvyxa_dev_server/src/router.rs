//! Radix-trie-based route matcher for O(path_depth) route resolution.
//!
//! Replaces the previous O(n) linear scan over all routes. Routes are compiled
//! into a tree structure at startup (and recompiled on manifest invalidation).
//! Lookup time is proportional to the number of path segments, not the number
//! of registered routes.

use std::collections::BTreeMap;

use ruvyxa_graph::{RouteEntry, RouteManifest};

/// A compiled router that resolves paths in O(depth) time.
#[derive(Debug, Clone)]
pub struct RadixRouter {
    root: TrieNode,
}

#[derive(Debug, Clone)]
struct TrieNode {
    /// Static children keyed by path segment.
    static_children: Vec<(String, TrieNode)>,
    /// Dynamic parameter child (`:param`).
    param_child: Option<Box<ParamChild>>,
    /// Optional parameter child (`:param?`).
    optional_param_child: Option<Box<ParamChild>>,
    /// Catch-all wildcard child (`*rest`).
    wildcard: Option<Box<WildcardChild>>,
    /// Route stored at this node (if a route terminates here).
    route_index: Option<usize>,
}

#[derive(Debug, Clone)]
struct ParamChild {
    name: String,
    node: TrieNode,
}

#[derive(Debug, Clone)]
struct WildcardChild {
    name: String,
    route_index: usize,
}

pub struct RouteMatch<'a> {
    pub route: &'a RouteEntry,
    pub params: BTreeMap<String, String>,
}

impl RadixRouter {
    /// Compile a router from a route manifest.
    pub fn compile(manifest: &RouteManifest) -> Self {
        let mut root = TrieNode::new();

        for (index, route) in manifest.routes.iter().enumerate() {
            let segments = parse_pattern(&route.path);
            root.insert(&segments, index);
        }

        Self { root }
    }

    /// Look up a request path and return the matched route with extracted params.
    pub fn find<'a>(
        &self,
        manifest: &'a RouteManifest,
        request_path: &str,
    ) -> Option<RouteMatch<'a>> {
        let parts = split_path(request_path);
        let mut params = BTreeMap::new();

        let route_index = self.root.lookup(&parts, 0, &mut params)?;
        let route = manifest.routes.get(route_index)?;

        Some(RouteMatch { route, params })
    }
}

impl TrieNode {
    fn new() -> Self {
        Self {
            static_children: Vec::new(),
            param_child: None,
            optional_param_child: None,
            wildcard: None,
            route_index: None,
        }
    }

    fn insert(&mut self, segments: &[PatternSegment], route_index: usize) {
        if segments.is_empty() {
            self.route_index = Some(route_index);
            return;
        }

        match &segments[0] {
            PatternSegment::Static(value) => {
                let child = self.find_or_create_static(value);
                child.insert(&segments[1..], route_index);
            }
            PatternSegment::Param(name) => {
                let child = self.param_child.get_or_insert_with(|| {
                    Box::new(ParamChild {
                        name: name.clone(),
                        node: TrieNode::new(),
                    })
                });
                child.node.insert(&segments[1..], route_index);
            }
            PatternSegment::OptionalParam(name) => {
                // Optional params can match with or without the segment.
                // Store the remaining pattern in the optional child.
                let child = self.optional_param_child.get_or_insert_with(|| {
                    Box::new(ParamChild {
                        name: name.clone(),
                        node: TrieNode::new(),
                    })
                });
                child.node.insert(&segments[1..], route_index);
                // Also register the remaining segments at this level (for when the param is absent)
                if segments.len() == 1 {
                    // If this is the last segment, this node is also a valid endpoint
                    self.route_index = self.route_index.or(Some(route_index));
                } else {
                    self.insert(&segments[1..], route_index);
                }
            }
            PatternSegment::Wildcard(name) => {
                self.wildcard = Some(Box::new(WildcardChild {
                    name: name.clone(),
                    route_index,
                }));
            }
        }
    }

    fn find_or_create_static(&mut self, segment: &str) -> &mut TrieNode {
        // Find existing or create new
        let position = self
            .static_children
            .iter()
            .position(|(key, _)| key == segment);
        match position {
            Some(index) => &mut self.static_children[index].1,
            None => {
                self.static_children
                    .push((segment.to_string(), TrieNode::new()));
                let last = self.static_children.len() - 1;
                &mut self.static_children[last].1
            }
        }
    }

    fn lookup(
        &self,
        parts: &[&str],
        index: usize,
        params: &mut BTreeMap<String, String>,
    ) -> Option<usize> {
        // If we've consumed all path segments, check if this node has a route
        if index >= parts.len() {
            // Check optional param child — it can match zero segments
            if self.route_index.is_none() {
                if let Some(optional) = &self.optional_param_child {
                    if let Some(result) = optional.node.lookup(parts, index, params) {
                        return Some(result);
                    }
                }
            }
            return self.route_index;
        }

        let segment = parts[index];

        // 1. Try static children first (highest priority)
        if let Some(result) = self.lookup_static(parts, index, params) {
            return Some(result);
        }

        // 2. Try dynamic parameter child
        if let Some(param_child) = &self.param_child {
            params.insert(param_child.name.clone(), segment.to_string());
            if let Some(result) = param_child.node.lookup(parts, index + 1, params) {
                return Some(result);
            }
            params.remove(&param_child.name);
        }

        // 3. Try optional parameter child
        if let Some(optional) = &self.optional_param_child {
            // Try with the segment consumed
            params.insert(optional.name.clone(), segment.to_string());
            if let Some(result) = optional.node.lookup(parts, index + 1, params) {
                return Some(result);
            }
            params.remove(&optional.name);

            // Try without consuming the segment (optional skipped)
            if let Some(result) = optional.node.lookup(parts, index, params) {
                return Some(result);
            }
        }

        // 4. Try wildcard (catch-all, lowest priority)
        if let Some(wildcard) = &self.wildcard {
            let rest = parts[index..].join("/");
            params.insert(wildcard.name.clone(), rest);
            return Some(wildcard.route_index);
        }

        None
    }

    fn lookup_static(
        &self,
        parts: &[&str],
        index: usize,
        params: &mut BTreeMap<String, String>,
    ) -> Option<usize> {
        let segment = parts[index];
        for (key, child) in &self.static_children {
            if key == segment {
                return child.lookup(parts, index + 1, params);
            }
        }
        None
    }
}

// --- Pattern Parsing ---

#[derive(Debug, Clone)]
enum PatternSegment {
    Static(String),
    Param(String),
    OptionalParam(String),
    Wildcard(String),
}

fn parse_pattern(pattern: &str) -> Vec<PatternSegment> {
    split_path(pattern)
        .into_iter()
        .map(|segment| {
            if segment.starts_with('*') {
                PatternSegment::Wildcard(segment.trim_start_matches('*').to_string())
            } else if segment.starts_with(':') && segment.ends_with('?') {
                let name = segment
                    .trim_start_matches(':')
                    .trim_end_matches('?')
                    .to_string();
                PatternSegment::OptionalParam(name)
            } else if segment.starts_with(':') {
                PatternSegment::Param(segment.trim_start_matches(':').to_string())
            } else {
                PatternSegment::Static(segment.to_string())
            }
        })
        .collect()
}

fn split_path(path: &str) -> Vec<&str> {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruvyxa_graph::{RouteEntry, RouteKind, RouteManifest, RuntimeTarget};
    use std::path::PathBuf;

    fn make_manifest(routes: Vec<(&str, RouteKind)>) -> RouteManifest {
        let entries = routes
            .into_iter()
            .map(|(path, kind)| RouteEntry {
                id: path.to_string(),
                path: path.to_string(),
                kind,
                file: PathBuf::from(format!("app{path}/page.tsx")),
                layout_chain: Vec::new(),
                server_modules: Vec::new(),
                client_modules: Vec::new(),
                runtime: RuntimeTarget::Node,
            })
            .collect();

        RouteManifest {
            app_dir: PathBuf::from("app"),
            routes: entries,
        }
    }

    #[test]
    fn test_static_routes() {
        let manifest = make_manifest(vec![
            ("/", RouteKind::Page),
            ("/about", RouteKind::Page),
            ("/blog", RouteKind::Page),
            ("/api/health", RouteKind::Api),
        ]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/").unwrap();
        assert_eq!(m.route.path, "/");

        let m = router.find(&manifest, "/about").unwrap();
        assert_eq!(m.route.path, "/about");

        let m = router.find(&manifest, "/api/health").unwrap();
        assert_eq!(m.route.path, "/api/health");

        assert!(router.find(&manifest, "/nonexistent").is_none());
    }

    #[test]
    fn test_dynamic_params() {
        let manifest = make_manifest(vec![
            ("/blog/:slug", RouteKind::Page),
            ("/users/:id/posts/:post_id", RouteKind::Page),
        ]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/blog/hello-world").unwrap();
        assert_eq!(m.route.path, "/blog/:slug");
        assert_eq!(m.params["slug"], "hello-world");

        let m = router.find(&manifest, "/users/42/posts/99").unwrap();
        assert_eq!(m.params["id"], "42");
        assert_eq!(m.params["post_id"], "99");
    }

    #[test]
    fn test_optional_params() {
        let manifest = make_manifest(vec![("/lang/:locale?", RouteKind::Page)]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/lang/en").unwrap();
        assert_eq!(m.params["locale"], "en");

        let m = router.find(&manifest, "/lang").unwrap();
        assert!(!m.params.contains_key("locale") || m.params["locale"].is_empty());
    }

    #[test]
    fn test_wildcard() {
        let manifest = make_manifest(vec![("/docs/*path", RouteKind::Page)]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/docs/getting-started").unwrap();
        assert_eq!(m.params["path"], "getting-started");

        let m = router.find(&manifest, "/docs/guides/routing").unwrap();
        assert_eq!(m.params["path"], "guides/routing");
    }

    #[test]
    fn test_static_takes_priority_over_dynamic() {
        let manifest = make_manifest(vec![
            ("/blog/featured", RouteKind::Page),
            ("/blog/:slug", RouteKind::Page),
        ]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/blog/featured").unwrap();
        assert_eq!(m.route.path, "/blog/featured");

        let m = router.find(&manifest, "/blog/other").unwrap();
        assert_eq!(m.route.path, "/blog/:slug");
    }
}
