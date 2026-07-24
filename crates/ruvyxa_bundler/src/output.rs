//! Output formatter: builds the virtual entry source and wraps the linked
//! bundle in the appropriate format for each [`BundleTarget`].
//!
//! ## Client (IIFE)
//!
//! ```js
//! (function(React, ReactDOM) {
//!   "use strict";
//!   // … all modules concatenated …
//!   // hydration entry
//!   const params = globalThis.__RUVYXA_ROUTE_PARAMS__ ?? {};
//!   const root = ReactDOM.hydrateRoot(document, React.createElement(Page, { params }));
//!   globalThis.__RUVYXA_ROOT__ = root;
//!   window.__RUVYXA_HYDRATED = true;
//! })(React, ReactDOM);
//! ```
//!
//! ## SSR (ESM)
//!
//! ```js
//! import React from "react";
//! import { renderToString } from "react-dom/server";
//! // … modules …
//! export async function render(ctx) { … }
//! ```

use crate::{BundleInput, BundleTarget};

/// Encode a value as a JavaScript string literal.
///
/// `serde_json` never fails on a `str`, but keep an explicit fallback rather
/// than an `unwrap` so a bundle can never panic the dev server.
pub(crate) fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// Build the virtual entry source that wires layouts + page together.
///
/// Returns `(source_string, virtual_label)`.
pub fn build_entry_source(input: &BundleInput) -> (String, String) {
    let label = "ruvyxa:bundle-entry.tsx".to_string();

    // Every interpolated value is emitted as a JSON literal. A path or route
    // that contains a quote, backslash, newline, or `</script` would otherwise
    // terminate the generated string early and inject arbitrary code into the
    // bundle. JSON string syntax is a subset of JavaScript string syntax, so a
    // JSON literal is always a valid — and correctly escaped — JS literal.
    let page_path = js_string(&input.entry.display().to_string().replace('\\', "/"));

    // Collect layout imports (root-to-leaf order).
    let layout_imports: String = input
        .layouts
        .iter()
        .enumerate()
        .map(|(i, layout)| {
            let lp = js_string(&layout.display().to_string().replace('\\', "/"));
            format!("import Layout{i} from {lp};\n")
        })
        .collect();

    let layout_wrappers: String = (0..input.layouts.len())
        .map(|i| format!("Layout{i}"))
        .collect::<Vec<_>>()
        .join(", ");

    // Special-file imports (error/loading/not-found), each optional. Absent
    // kinds contribute nothing, so a route without them emits the same bundle
    // it always did.
    let (error_import, error_name) = special_import(&input.specials.error, "RouteError");
    let (loading_import, loading_name) = special_import(&input.specials.loading, "RouteLoading");
    let (not_found_import, not_found_name) =
        special_import(&input.specials.not_found, "RouteNotFound");
    let special_imports = format!("{error_import}{loading_import}{not_found_import}");

    // The error/not-found boundary class is only referenced when one of those
    // specials exists; emit it only then so an ordinary route ships no dead code.
    let boundary_prelude = if error_name.is_some() || not_found_name.is_some() {
        format!("\n{ROUTE_BOUNDARY_PRELUDE}\n")
    } else {
        String::new()
    };

    let request_path = js_string(&input.request_path);

    // Client bundles are keyed by route pattern, which is what `request_path`
    // carries on this path — one bundle serves every concrete URL of a dynamic
    // route.
    let route_tree = route_tree_function(
        &request_path,
        &layout_wrappers,
        error_name.as_deref(),
        loading_name.as_deref(),
        not_found_name.as_deref(),
    );

    let source = match input.target {
        BundleTarget::Client => {
            format!(
                r#"import React from "react";
import {{ hydrateRoot }} from "react-dom/client";
import Page from {page_path};
{layout_imports}{special_imports}
{ROUTE_CONTEXT_PRELUDE}{boundary_prelude}

{route_tree}
;(globalThis.__RUVYXA_ROUTES__ ||= {{}})[{request_path}] = __ruvyxaTree;

const __ruvyxaCtx = {{
  path: globalThis.__RUVYXA_REQUEST_PATH__ ?? {request_path},
  params: globalThis.__RUVYXA_ROUTE_PARAMS__ ?? {{}},
}};
const __ruvyxaTreeElement = __ruvyxaTree(__ruvyxaCtx);

if (globalThis.__RUVYXA_ROOT__) {{
  globalThis.__RUVYXA_ROOT__.render(__ruvyxaTreeElement);
}} else {{
  globalThis.__RUVYXA_ROOT__ = hydrateRoot(document, __ruvyxaTreeElement);
}}
window.__RUVYXA_HYDRATED = true;
"#
            )
        }
        BundleTarget::Ssr | BundleTarget::Edge => {
            format!(
                r#"import React from "react";
import {{ renderToString }} from "react-dom/server";
import Page from {page_path};
{layout_imports}{special_imports}
{ROUTE_CONTEXT_PRELUDE}{boundary_prelude}

{route_tree}

export async function render(ctx) {{
  return "<!doctype html>" + renderToString(__ruvyxaTree(ctx));
}}
"#
            )
        }
    };

    (source, label)
}

/// Build an optional `import <ident> from "<path>"` for a special file.
///
/// Returns the import statement (with a trailing newline) and the identifier to
/// reference, or empty string / `None` when the route has no such file.
fn special_import(file: &Option<std::path::PathBuf>, ident: &str) -> (String, Option<String>) {
    match file {
        Some(path) => {
            let literal = js_string(&path.display().to_string().replace('\\', "/"));
            (
                format!("import {ident} from {literal};\n"),
                Some(ident.to_string()),
            )
        }
        None => (String::new(), None),
    }
}

/// Shared routing context binding.
///
/// Created on `globalThis` rather than imported so a generated entry never has
/// to depend on `@ruvyxa/react`; an app may render plain React pages and not
/// install it. The package's hooks reach the same object.
///
/// Mirrors `routeContextPrelude()` in
/// `packages/ruvyxa/runtime/entry-templates.mjs`;
/// `tests/packages/ruvyxa/entry-templates.test.mjs` asserts the two agree.
const ROUTE_CONTEXT_PRELUDE: &str = "const __ruvyxaRouteContext = (globalThis.__RUVYXA_ROUTE_CONTEXT__ ||= React.createContext(null));";

/// Inline error / not-found boundary class.
///
/// Mirrors `routeBoundaryPrelude()` in
/// `packages/ruvyxa/runtime/entry-templates.mjs`. Defined inline rather than
/// imported because a generated entry cannot depend on `@ruvyxa/react`; it
/// tells a `notFound()` signal apart from an ordinary error by the own property
/// `error.__ruvyxaNotFound` that `notFound()` stamps.
const ROUTE_BOUNDARY_PRELUDE: &str = r#"class __ruvyxaBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { error: null };
    this.reset = () => this.setState({ error: null });
  }
  static getDerivedStateFromError(error) {
    return { error };
  }
  render() {
    const error = this.state.error;
    if (error) {
      if (error && error.__ruvyxaNotFound) {
        if (this.props.notFound) return React.createElement(this.props.notFound, null);
        throw error;
      }
      if (this.props.errorFallback) {
        return React.createElement(this.props.errorFallback, { error, reset: this.reset });
      }
      throw error;
    }
    return this.props.children;
  }
}"#;

/// Build the function that composes a route's element tree.
///
/// The page is wrapped, innermost to outermost: the error/not-found boundary
/// when either exists, a `<Suspense>` when a `loading.tsx` is present, the
/// layouts, then the routing context provider. The boundary is nested inside the
/// Suspense so a synchronous throw (error or `notFound()`) renders its UI on the
/// server rather than making React emit the Suspense fallback. Mirrors
/// `routeTreeFunction()` in `packages/ruvyxa/runtime/entry-templates.mjs`.
fn route_tree_function(
    route_path_literal: &str,
    layout_wrappers: &str,
    error_name: Option<&str>,
    loading_name: Option<&str>,
    not_found_name: Option<&str>,
) -> String {
    let mut lines = vec![
        "  let tree = React.createElement(Page, { params: ctx.params ?? {}, requestPath: ctx.path });"
            .to_string(),
    ];
    if error_name.is_some() || not_found_name.is_some() {
        let error_ref = error_name.unwrap_or("null");
        let not_found_ref = not_found_name.unwrap_or("null");
        lines.push(format!(
            "  tree = React.createElement(__ruvyxaBoundary, {{ errorFallback: {error_ref}, notFound: {not_found_ref} }}, tree);"
        ));
    }
    if let Some(loading) = loading_name {
        lines.push(format!(
            "  tree = React.createElement(React.Suspense, {{ fallback: React.createElement({loading}, null) }}, tree);"
        ));
    }
    lines.push(format!(
        "  for (const Layout of [{layout_wrappers}].reverse()) {{\n    tree = React.createElement(Layout, null, tree);\n  }}"
    ));
    lines.push(format!(
        "  return React.createElement(__ruvyxaRouteContext.Provider, {{\n    value: {{ pathname: ctx.path, params: ctx.params ?? {{}}, route: {route_path_literal} }},\n  }}, tree);"
    ));
    format!("function __ruvyxaTree(ctx) {{\n{}\n}}", lines.join("\n"))
}

/// Wrap the fully-linked bundle in the target-specific format.
pub fn wrap(linked: String, input: &BundleInput) -> String {
    match input.target {
        BundleTarget::Client => {
            // Browser hydration is loaded with `<script type="module">`, so
            // external package imports must remain top-level ESM imports.
            linked
        }
        BundleTarget::Ssr | BundleTarget::Edge => {
            // The linker hoists external ESM imports and exposes the virtual
            // entry render function as a top-level ESM export.
            format!("// Ruvyxa SSR bundle\n{linked}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BundleOptions, BundleTarget, RouteSpecials};
    use std::path::PathBuf;

    fn input(entry: &str, layouts: Vec<&str>, request_path: &str) -> BundleInput {
        BundleInput {
            entry: PathBuf::from(entry),
            project_root: PathBuf::from("/project"),
            app_dir: PathBuf::from("/project/app"),
            layouts: layouts.into_iter().map(PathBuf::from).collect(),
            request_path: request_path.to_string(),
            target: BundleTarget::Client,
            options: BundleOptions::default(),
            specials: RouteSpecials::default(),
        }
    }

    #[test]
    fn entry_source_escapes_paths_and_request_paths() {
        // A quote in a route or project path used to close the generated string
        // literal early, producing a broken bundle or injected statements.
        let (source, _) = build_entry_source(&input(
            "/project/app/a\"b/page.tsx",
            vec!["/project/app/l\"1/layout.tsx"],
            "/a\";globalThis.pwned=1;\"",
        ));

        assert!(
            source.contains(r#"import Page from "/project/app/a\"b/page.tsx";"#),
            "{source}"
        );
        assert!(
            source.contains(r#"import Layout0 from "/project/app/l\"1/layout.tsx";"#),
            "{source}"
        );
        assert!(!source.contains("globalThis.pwned=1;\"\n"), "{source}");
        assert!(
            source.contains(r#"?? "/a\";globalThis.pwned=1;\"","#),
            "{source}"
        );
        // The route pattern reaches two more interpolation sites now — the
        // registry key and the routing context — and both must stay escaped.
        assert!(
            source.contains(r#"["/a\";globalThis.pwned=1;\""] = __ruvyxaTree;"#),
            "{source}"
        );
        assert!(
            source.contains(r#"route: "/a\";globalThis.pwned=1;\"" },"#),
            "{source}"
        );
    }

    #[test]
    fn entry_source_keeps_ordinary_paths_readable() {
        let (source, label) = build_entry_source(&input(
            "/project/app/blog/[slug]/page.tsx",
            Vec::new(),
            "/blog/[slug]",
        ));

        assert_eq!(label, "ruvyxa:bundle-entry.tsx");
        assert!(source.contains(r#"import Page from "/project/app/blog/[slug]/page.tsx";"#));
        assert!(source.contains(r#"?? "/blog/[slug]","#));
    }

    #[test]
    fn client_entry_registers_the_route_for_soft_navigation() {
        // Without this registry the client router can only render a route the
        // very first time: `import()` caches by URL, so returning to a visited
        // route would re-resolve the cached module and render nothing.
        let (source, _) = build_entry_source(&input(
            "/project/app/blog/[slug]/page.tsx",
            vec!["/project/app/layout.tsx"],
            "/blog/[slug]",
        ));

        assert!(
            source.contains(
                r#"(globalThis.__RUVYXA_ROUTES__ ||= {})["/blog/[slug]"] = __ruvyxaTree;"#
            ),
            "{source}"
        );
        assert!(source.contains("__ruvyxaRouteContext.Provider"), "{source}");
    }

    #[test]
    fn composes_error_loading_and_not_found_specials_around_the_page() {
        let mut bundle = input(
            "/project/app/blog/[slug]/page.tsx",
            Vec::new(),
            "/blog/[slug]",
        );
        bundle.specials = RouteSpecials {
            error: Some(PathBuf::from("/project/app/error.tsx")),
            loading: Some(PathBuf::from("/project/app/loading.tsx")),
            not_found: Some(PathBuf::from("/project/app/blog/[slug]/not-found.tsx")),
        };
        let (source, _) = build_entry_source(&bundle);

        // Each present special is imported under its well-known identifier.
        assert!(
            source.contains(r#"import RouteError from "/project/app/error.tsx";"#),
            "{source}"
        );
        assert!(
            source.contains(r#"import RouteLoading from "/project/app/loading.tsx";"#),
            "{source}"
        );
        assert!(
            source
                .contains(r#"import RouteNotFound from "/project/app/blog/[slug]/not-found.tsx";"#),
            "{source}"
        );
        // The inline boundary class is emitted and wired to both fallbacks.
        assert!(
            source.contains("class __ruvyxaBoundary extends React.Component"),
            "{source}"
        );
        assert!(
            source.contains(
                "React.createElement(__ruvyxaBoundary, { errorFallback: RouteError, notFound: RouteNotFound }, tree)"
            ),
            "{source}"
        );
        // loading.tsx becomes the Suspense fallback around the page.
        assert!(
            source.contains(
                "React.createElement(React.Suspense, { fallback: React.createElement(RouteLoading, null) }, tree)"
            ),
            "{source}"
        );
    }

    #[test]
    fn omits_the_boundary_when_a_route_has_no_error_or_not_found() {
        // loading.tsx alone needs only React.Suspense; shipping the boundary
        // class would be dead code in the common no-specials case.
        let mut bundle = input("/project/app/page.tsx", Vec::new(), "/");
        bundle.specials = RouteSpecials {
            loading: Some(PathBuf::from("/project/app/loading.tsx")),
            ..RouteSpecials::default()
        };
        let (source, _) = build_entry_source(&bundle);

        assert!(!source.contains("class __ruvyxaBoundary"), "{source}");
        assert!(!source.contains("__ruvyxaBoundary,"), "{source}");
        assert!(source.contains("React.Suspense"), "{source}");
    }

    #[test]
    fn a_route_without_specials_is_unchanged() {
        let (source, _) = build_entry_source(&input("/project/app/page.tsx", Vec::new(), "/"));
        assert!(!source.contains("__ruvyxaBoundary"), "{source}");
        assert!(!source.contains("React.Suspense"), "{source}");
        assert!(!source.contains("RouteError"), "{source}");
    }

    #[test]
    fn server_entries_provide_the_same_routing_context_as_the_client() {
        // A hook that reads the routing context has to see the same value on
        // the server as it does after hydration, or the first client render
        // produces a mismatch.
        for target in [BundleTarget::Ssr, BundleTarget::Edge] {
            let mut bundle = input("/project/app/page.tsx", Vec::new(), "/");
            bundle.target = target;
            let (source, _) = build_entry_source(&bundle);

            assert!(source.contains(ROUTE_CONTEXT_PRELUDE), "{source}");
            assert!(
                source.contains(
                    r#"value: { pathname: ctx.path, params: ctx.params ?? {}, route: "/" },"#
                ),
                "{source}"
            );
            // The server must not publish a client registry: there is no root
            // to re-render into, and the global would leak across requests.
            assert!(!source.contains("__RUVYXA_ROUTES__"), "{source}");
        }
    }
}
