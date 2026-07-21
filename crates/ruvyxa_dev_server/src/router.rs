//! Radix-trie-based route matcher for O(path_depth) route resolution.
//!
//! Replaces the previous O(n) linear scan over all routes. Routes are compiled
//! into a tree structure at startup (and recompiled on manifest invalidation).
//! Lookup time is proportional to the number of path segments, not the number
//! of registered routes.

use ruvyxa_graph::{RouteEntry, RouteManifest, RouteParams};
use serde_json::Value;

/// A compiled router that resolves paths in O(depth) time.
#[derive(Debug, Clone)]
pub struct RadixRouter {
    root: TrieNode,
    /// Parsed pattern of every manifest route, indexed by route index.
    ///
    /// The trie collapses sibling routes that share a URL shape into one
    /// parameter node, so it can only decide *which* route matched. Parameter
    /// names must come from the matched route's own pattern.
    patterns: Vec<Vec<PatternSegment>>,
}

#[derive(Debug, Clone)]
struct TrieNode {
    /// Static children keyed by path segment.
    static_children: Vec<(String, TrieNode)>,
    /// Dynamic parameter child (`[param]`). The name lives on the route
    /// pattern, not here, because siblings may declare different names.
    param_child: Option<Box<TrieNode>>,
    /// Route reached through a catch-all child (`[...rest]`).
    wildcard: Option<usize>,
    /// Route reached through an optional catch-all child (`[[...rest]]`).
    optional_wildcard: Option<usize>,
    /// Route stored at this node (if a route terminates here).
    route_index: Option<usize>,
}

pub struct RouteMatch<'a> {
    pub route: &'a RouteEntry,
    pub params: RouteParams,
}

impl RadixRouter {
    /// Compile a router from a route manifest.
    pub fn compile(manifest: &RouteManifest) -> Self {
        let mut root = TrieNode::new();
        let mut patterns = Vec::with_capacity(manifest.routes.len());

        for (index, route) in manifest.routes.iter().enumerate() {
            let segments = parse_pattern(&route.path);
            root.insert(&segments, index);
            patterns.push(segments);
        }

        Self { root, patterns }
    }

    /// Look up a request path and return the matched route with extracted params.
    pub fn find<'a>(
        &self,
        manifest: &'a RouteManifest,
        request_path: &str,
    ) -> Option<RouteMatch<'a>> {
        let parts = split_path(request_path);

        let route_index = self.root.lookup(&parts, 0)?;
        let route = manifest.routes.get(route_index)?;
        let params = self
            .patterns
            .get(route_index)
            .map(|pattern| extract_params(pattern, &parts))
            .unwrap_or_default();

        Some(RouteMatch { route, params })
    }
}

/// Bind request segments to the parameter names declared by one route pattern.
///
/// `/users/[id]/posts` and `/users/[userId]/settings` are distinct URL shapes
/// that the manifest accepts, yet they share a trie parameter node. Reading the
/// names from the matched route keeps each route's declared parameter name.
fn extract_params(pattern: &[PatternSegment], parts: &[&str]) -> RouteParams {
    let mut params = RouteParams::new();

    for (index, segment) in pattern.iter().enumerate() {
        match segment {
            PatternSegment::Static(_) => {}
            PatternSegment::Param(name) => {
                let Some(value) = parts.get(index) else {
                    break;
                };
                params.insert(name.clone(), Value::String((*value).to_string()));
            }
            PatternSegment::Wildcard(name) | PatternSegment::OptionalWildcard(name) => {
                let rest = parts.get(index..).unwrap_or_default();
                // An optional catch-all that captured nothing stays absent so
                // pages can distinguish "/shop" from "/shop/<segment>".
                if !rest.is_empty() {
                    params.insert(
                        name.clone(),
                        Value::Array(
                            rest.iter()
                                .map(|segment| Value::String((*segment).to_string()))
                                .collect(),
                        ),
                    );
                }
                break;
            }
        }
    }

    params
}

impl TrieNode {
    fn new() -> Self {
        Self {
            static_children: Vec::new(),
            param_child: None,
            wildcard: None,
            optional_wildcard: None,
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
            PatternSegment::Param(_) => {
                let child = self
                    .param_child
                    .get_or_insert_with(|| Box::new(TrieNode::new()));
                child.insert(&segments[1..], route_index);
            }
            PatternSegment::Wildcard(_) => {
                self.wildcard = Some(route_index);
            }
            PatternSegment::OptionalWildcard(_) => {
                self.optional_wildcard = Some(route_index);
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

    /// Resolve which route owns `parts`. Parameter binding happens afterwards
    /// from the matched route's pattern, so this walk carries no param state.
    fn lookup(&self, parts: &[&str], index: usize) -> Option<usize> {
        // If we've consumed all path segments, check if this node has a route
        if index >= parts.len() {
            if let Some(route_index) = self.route_index {
                return Some(route_index);
            }
            return self.optional_wildcard;
        }

        // 1. Try static children first (highest priority)
        if let Some(result) = self.lookup_static(parts, index) {
            return Some(result);
        }

        // 2. Try dynamic parameter child
        if let Some(param_child) = &self.param_child
            && let Some(result) = param_child.lookup(parts, index + 1)
        {
            return Some(result);
        }

        // 3. Required catch-all, then optional catch-all (lowest priority).
        self.wildcard.or(self.optional_wildcard)
    }

    fn lookup_static(&self, parts: &[&str], index: usize) -> Option<usize> {
        let segment = parts[index];
        for (key, child) in &self.static_children {
            if key == segment {
                return child.lookup(parts, index + 1);
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
    Wildcard(String),
    OptionalWildcard(String),
}

fn parse_pattern(pattern: &str) -> Vec<PatternSegment> {
    split_path(pattern)
        .into_iter()
        .map(|segment| {
            if segment.starts_with("[[...") && segment.ends_with("]]") {
                PatternSegment::OptionalWildcard(
                    segment
                        .trim_start_matches("[[...")
                        .trim_end_matches("]]")
                        .to_string(),
                )
            } else if segment.starts_with("[...") && segment.ends_with(']') {
                PatternSegment::Wildcard(
                    segment
                        .trim_start_matches("[...")
                        .trim_end_matches(']')
                        .to_string(),
                )
            } else if segment.starts_with('[') && segment.ends_with(']') {
                PatternSegment::Param(
                    segment
                        .trim_start_matches('[')
                        .trim_end_matches(']')
                        .to_string(),
                )
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
    use ruvyxa_graph::{RenderMeta, RouteEntry, RouteKind, RouteManifest, RuntimeTarget};
    use serde_json::json;
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
                render: RenderMeta::default(),
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
            ("/blog/[slug]", RouteKind::Page),
            ("/users/[id]/posts/[post_id]", RouteKind::Page),
        ]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/blog/hello-world").unwrap();
        assert_eq!(m.route.path, "/blog/[slug]");
        assert_eq!(m.params["slug"], json!("hello-world"));

        let m = router.find(&manifest, "/blog/hello world").unwrap();
        assert_eq!(m.params["slug"], json!("hello world"));

        let m = router.find(&manifest, "/users/42/posts/99").unwrap();
        assert_eq!(m.params["id"], json!("42"));
        assert_eq!(m.params["post_id"], json!("99"));
    }

    #[test]
    fn test_optional_catch_all() {
        let manifest = make_manifest(vec![("/shop/[[...slug]]", RouteKind::Page)]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/shop/clothes/tops").unwrap();
        assert_eq!(m.params["slug"], json!(["clothes", "tops"]));

        let m = router.find(&manifest, "/shop").unwrap();
        assert!(m.params.is_empty());
    }

    #[test]
    fn test_wildcard() {
        let manifest = make_manifest(vec![("/docs/[...path]", RouteKind::Page)]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/docs/getting-started").unwrap();
        assert_eq!(m.params["path"], json!(["getting-started"]));

        let m = router.find(&manifest, "/docs/guides/routing").unwrap();
        assert_eq!(m.params["path"], json!(["guides", "routing"]));

        assert!(router.find(&manifest, "/docs").is_none());
    }

    #[test]
    fn test_static_takes_priority_over_dynamic() {
        let manifest = make_manifest(vec![
            ("/blog/featured", RouteKind::Page),
            ("/blog/[slug]", RouteKind::Page),
        ]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/blog/featured").unwrap();
        assert_eq!(m.route.path, "/blog/featured");

        let m = router.find(&manifest, "/blog/other").unwrap();
        assert_eq!(m.route.path, "/blog/[slug]");
    }

    #[test]
    fn sibling_routes_keep_their_own_parameter_names() {
        // Both routes are accepted by the manifest conflict check because their
        // URL shapes differ, but they share one trie parameter node. Each must
        // still receive the parameter name it declared.
        let manifest = make_manifest(vec![
            ("/users/[id]/posts", RouteKind::Page),
            ("/users/[userId]/settings", RouteKind::Page),
        ]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/users/7/posts").unwrap();
        assert_eq!(m.route.path, "/users/[id]/posts");
        assert_eq!(m.params["id"], json!("7"));

        let m = router.find(&manifest, "/users/7/settings").unwrap();
        assert_eq!(m.route.path, "/users/[userId]/settings");
        assert_eq!(m.params["userId"], json!("7"));
        assert!(!m.params.contains_key("id"));
    }

    #[test]
    fn backtracking_does_not_leak_parameters_from_rejected_branches() {
        let manifest = make_manifest(vec![
            ("/[locale]/docs", RouteKind::Page),
            ("/blog/[slug]", RouteKind::Page),
        ]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/blog/intro").unwrap();
        assert_eq!(m.route.path, "/blog/[slug]");
        assert_eq!(m.params["slug"], json!("intro"));
        assert!(!m.params.contains_key("locale"));

        let m = router.find(&manifest, "/th/docs").unwrap();
        assert_eq!(m.route.path, "/[locale]/docs");
        assert_eq!(m.params["locale"], json!("th"));
        assert!(!m.params.contains_key("slug"));
    }

    #[test]
    fn test_static_takes_priority_over_optional_catch_all() {
        let manifest = make_manifest(vec![
            ("/shop", RouteKind::Page),
            ("/shop/[[...slug]]", RouteKind::Page),
        ]);
        let router = RadixRouter::compile(&manifest);

        let m = router.find(&manifest, "/shop").unwrap();
        assert_eq!(m.route.path, "/shop");

        let m = router.find(&manifest, "/shop/clothes").unwrap();
        assert_eq!(m.route.path, "/shop/[[...slug]]");
        assert_eq!(m.params["slug"], json!(["clothes"]));
    }
}
