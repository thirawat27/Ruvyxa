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

    // On the client path `input.request_path` carries the route *pattern*: one
    // bundle serves every concrete URL of a dynamic route, so there is no single
    // request path to embed. The binding is named for what it holds.
    let route_pattern = js_string(&input.request_path);

    // Route metadata is read from namespace re-imports of the same modules: a
    // default import cannot see a sibling `export const meta`. Ordered root
    // layout → leaf layout → page, which the resolver treats as least → most
    // specific. Mirrors `metaSourceImports()` in
    // `packages/ruvyxa/runtime/entry-templates.mjs`.
    let meta_paths: Vec<String> = input
        .layouts
        .iter()
        .chain(std::iter::once(&input.entry))
        .map(|file| js_string(&file.display().to_string().replace('\\', "/")))
        .collect();
    let meta_imports: String = meta_paths
        .iter()
        .enumerate()
        .map(|(i, literal)| format!("import * as __ruvyxaMeta{i} from {literal};\n"))
        .collect();
    let meta_names: String = (0..meta_paths.len())
        .map(|i| format!("__ruvyxaMeta{i}"))
        .collect::<Vec<_>>()
        .join(", ");

    // Client bundles are keyed by route pattern — one bundle serves every
    // concrete URL of a dynamic route.
    let route_tree = route_tree_function(
        &route_pattern,
        &layout_wrappers,
        error_name.as_deref(),
        loading_name.as_deref(),
        not_found_name.as_deref(),
        &meta_names,
    );

    // Only a route with a declared loading state gets a shell; see
    // `route_shell_function`. Emitted for the browser bundle alone — a server
    // render has the data in hand and never shows a loading fallback for it.
    let route_shell = match (input.target, loading_name.as_deref()) {
        (BundleTarget::Client, Some(loading)) => format!(
            "
{}
;(globalThis.__RUVYXA_SHELLS__ ||= {{}})[{route_pattern}] = __ruvyxaShell;
",
            route_shell_function(&route_pattern, &layout_wrappers, loading, &meta_names)
        ),
        _ => String::new(),
    };

    let source = match input.target {
        BundleTarget::Client => {
            format!(
                r#"import React from "react";
import {{ createRoot, hydrateRoot }} from "react-dom/client";
import Page from {page_path};
{layout_imports}{special_imports}{meta_imports}
{ROUTE_CONTEXT_PRELUDE}{boundary_prelude}
{META_PRELUDE}

{route_tree}
;(globalThis.__RUVYXA_ROUTES__ ||= {{}})[{route_pattern}] = __ruvyxaTree;
globalThis.__RUVYXA_ROUTE_PATTERN__ = {route_pattern};
{route_shell}

{CLIENT_BOOTSTRAP_PRELUDE}

const __ruvyxaCtx = {{
  // The registry is keyed by route pattern, so this bundle has no concrete
  // request path of its own to fall back to. Reading the browser's location is
  // correct for every URL the pattern matches; falling back to the pattern
  // itself used to report `/blog/[slug]` as the pathname.
  path: globalThis.__RUVYXA_REQUEST_PATH__ ?? (typeof location === "undefined" ? "/" : location.pathname),
  params: globalThis.__RUVYXA_ROUTE_PARAMS__ ?? {{}},
}};
const __ruvyxaTreeElement = __ruvyxaTree(__ruvyxaCtx);

if (globalThis.__RUVYXA_ROOT__) {{
  globalThis.__RUVYXA_ROOT__.render(__ruvyxaTreeElement);
}} else if (globalThis.__RUVYXA_CSR__) {{
  // A client-rendered route is served as a shell the server never rendered
  // this tree into, so there is no markup to hydrate against and matching one
  // is a guaranteed mismatch — React discards the document and warns (#418).
  // Mounting is what the shell is for.
  globalThis.__RUVYXA_ROOT__ = createRoot(document);
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
{layout_imports}{special_imports}{meta_imports}
{ROUTE_CONTEXT_PRELUDE}{boundary_prelude}
{META_PRELUDE}{META_LANG_PRELUDE}

{route_tree}

export async function render(ctx) {{
  const html = "<!doctype html>" + renderToString(__ruvyxaTree(ctx));
  return __ruvyxaApplyLang(html, __ruvyxaResolveMeta([{meta_names}], ctx).lang);
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

/// Read the bootstrap data block and publish it on `globalThis`.
///
/// The document used to carry these assignments as an executable inline
/// `<script>`. Every page had one, so any `Content-Security-Policy` without
/// `'unsafe-inline'` blocked it and hydration never started — and since the
/// parameters differ per request, a CSP hash could not cover it either.
///
/// `type="application/json"` is a data block rather than executable script, so
/// `script-src` does not apply to it. Publishing the same globals here is what
/// keeps every reader downstream unchanged.
///
/// `??=` rather than `=`: a soft navigation has already written the params for
/// the route it is entering, and this bundle may only be evaluated afterwards.
///
/// Mirrors `clientBootstrapPrelude()` in
/// `packages/ruvyxa/runtime/entry-templates.mjs`; both are replayed against
/// `tests/fixtures/client-bootstrap-conformance.json`.
const CLIENT_BOOTSTRAP_PRELUDE: &str = r#"const __ruvyxaBootstrap = (() => {
  if (typeof document === "undefined") return {}
  const el = document.getElementById("__ruvyxa-bootstrap")
  if (!el) return {}
  try {
    return JSON.parse(el.textContent || "{}")
  } catch {
    return {}
  }
})()
globalThis.__RUVYXA_ROUTE_PARAMS__ ??= __ruvyxaBootstrap.params
globalThis.__RUVYXA_REQUEST_PATH__ ??= __ruvyxaBootstrap.path
if (__ruvyxaBootstrap.csr === true) globalThis.__RUVYXA_CSR__ = true"#;

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
    // Ask the server for this route again, then clear the boundary.
    // A plain reset re-renders the payload that just failed, so it can only
    // recover from a fault in the client tree. A page whose data failed to load
    // needs the request repeated, which is what the router's retry does.
    // Without a mounted router there is nothing to re-fetch from, so this
    // degrades to a plain reset rather than doing nothing at all.
    this.retry = () => {
      const router = globalThis.__RUVYXA_ROUTER_INSTANCE__;
      if (!router || typeof router.retry !== "function") {
        this.reset();
        return Promise.resolve();
      }
      return Promise.resolve(router.retry()).then(
        () => this.reset(),
        (failure) => this.setState({ error: failure }),
      );
    };
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
        return React.createElement(this.props.errorFallback, {
          error,
          reset: this.reset,
          retry: this.retry,
        });
      }
      throw error;
    }
    return this.props.children;
  }
}"#;

/// Route-metadata helpers: merge, element construction, and the `<html lang>`
/// rewrite.
///
/// Mirrors `routeMetaPrelude()` in `packages/ruvyxa/runtime/entry-templates.mjs`
/// — the two must stay in step or a route's `<head>` would differ depending on
/// which bundler produced it. Inlined rather than imported for the same reason
/// as the routing context: a generated entry cannot depend on `@ruvyxa/react`.
///
/// Metadata merges least-specific first (root layout → page). `titleTemplate`
/// applies only to a title declared below the level that set it, so a layout's
/// template formats its pages' titles and not its own. Resolution is
/// synchronous: an async `meta` would resolve after the shell was flushed
/// without a title.
const META_PRELUDE: &str = r#"function __ruvyxaResolveMeta(sources, ctx) {
  const merged = {};
  let template = null;
  let templateDepth = -1;
  let titleDepth = -1;
  for (let depth = 0; depth < sources.length; depth += 1) {
    const source = sources[depth];
    const declared = source && source.meta;
    const resolved = typeof declared === "function" ? declared(ctx) : declared;
    if (!resolved || typeof resolved !== "object") continue;
    if (typeof resolved.titleTemplate === "string") {
      template = resolved.titleTemplate;
      templateDepth = depth;
    }
    for (const key of Object.keys(resolved)) {
      if (resolved[key] !== undefined) merged[key] = resolved[key];
    }
    if (typeof resolved.title === "string") titleDepth = depth;
  }
  if (template && titleDepth > templateDepth && typeof merged.title === "string") {
    merged.title = template.replace("%s", () => merged.title);
  }
  delete merged.titleTemplate;
  return merged;
}

function __ruvyxaMetaElement(meta) {
  if (!meta || typeof meta !== "object") return null;
  const children = [];
  const add = (type, props) => {
    children.push(React.createElement(type, Object.assign({ key: type + children.length }, props)));
  };
  const title = typeof meta.title === "string" && meta.title !== "" ? meta.title : null;
  const description = typeof meta.description === "string" ? meta.description : null;
  const canonical = typeof meta.canonical === "string" ? meta.canonical : null;
  const image = typeof meta.image === "string" ? meta.image : null;
  if (title) add("title", { children: title });
  if (description) add("meta", { name: "description", content: description });
  if (canonical) add("link", { rel: "canonical", href: canonical });
  const robots = typeof meta.robots === "string" ? meta.robots : meta.noindex ? "noindex, nofollow" : null;
  if (robots) add("meta", { name: "robots", content: robots });
  for (const alternate of Array.isArray(meta.alternates) ? meta.alternates : []) {
    if (alternate && alternate.href && alternate.hreflang) {
      add("link", { rel: "alternate", hrefLang: alternate.hreflang, href: alternate.href });
    }
  }
  if (title || description || image) {
    if (title) add("meta", { property: "og:title", content: title });
    if (description) add("meta", { property: "og:description", content: description });
    add("meta", { property: "og:type", content: meta.type || "website" });
    if (canonical) add("meta", { property: "og:url", content: canonical });
    if (meta.siteName) add("meta", { property: "og:site_name", content: meta.siteName });
    if (meta.locale) add("meta", { property: "og:locale", content: meta.locale });
    if (image) add("meta", { property: "og:image", content: image });
    if (image && meta.imageAlt) add("meta", { property: "og:image:alt", content: meta.imageAlt });
    add("meta", { name: "twitter:card", content: meta.card || (image ? "summary_large_image" : "summary") });
    if (title) add("meta", { name: "twitter:title", content: title });
    if (description) add("meta", { name: "twitter:description", content: description });
    if (image) add("meta", { name: "twitter:image", content: image });
  }
  if (children.length === 0) return null;
  return children;
}"#;

/// The `<html lang>` rewrite, appended to [`META_PRELUDE`] on server entries.
///
/// Only a server entry has a finished document string to rewrite; the browser
/// hydrates into a document whose `lang` the server already set, so shipping
/// this to the client would be dead bytes on every route bundle. Mirrors the
/// `lang` option of `routeMetaPrelude()` in
/// `packages/ruvyxa/runtime/entry-templates.mjs`.
const META_LANG_PRELUDE: &str = r#"

function __ruvyxaApplyLang(html, lang) {
  if (typeof html !== "string" || typeof lang !== "string" || lang === "") return html;
  const match = /<html\b[^>]*>/i.exec(html);
  if (!match) return html;
  const value = lang.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
  const attribute = /\slang\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/i;
  const tag = attribute.test(match[0])
    ? match[0].replace(attribute, () => ' lang="' + value + '"')
    : match[0].replace(/^<html/i, () => '<html lang="' + value + '"');
  return html.slice(0, match.index) + tag + html.slice(match.index + match[0].length);
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
    meta_names: &str,
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
    // Metadata is a sibling of the layouts, not a wrapper around them: a layout
    // that suspends must not be able to hold the document title back past the
    // flushed shell. It is passed as an extra child of the provider — an element
    // array carrying its own keys — so no wrapper element is created per render.
    let meta_child = if meta_names.is_empty() {
        String::new()
    } else {
        format!("__ruvyxaMetaElement(__ruvyxaResolveMeta([{meta_names}], ctx)), ")
    };
    lines.push(format!(
        "  return React.createElement(__ruvyxaRouteContext.Provider, {{\n    value: {{ pathname: ctx.path, params: ctx.params ?? {{}}, route: {route_path_literal}, flight: ctx.flight }},\n  }}, {meta_child}tree);"
    ));
    format!("function __ruvyxaTree(ctx) {{\n{}\n}}", lines.join("\n"))
}

/// Build the route's loading shell: its layouts wrapped around `loading.tsx`.
///
/// This is the half of a route that needs no server data. The layouts and the
/// loading component are already in this bundle, so once it has executed the
/// client can paint the shell with no request at all — which is what lets a
/// navigation show the destination immediately instead of leaving the previous
/// page up until the Flight payload lands.
///
/// Emitted only for a route that has a `loading.tsx`; a route without one has
/// no declared loading state, and a blank screen would be worse than the page
/// the user is already looking at. Mirrors `routeShellFunction()` in
/// `packages/ruvyxa/runtime/entry-templates.mjs`.
fn route_shell_function(
    route_path_literal: &str,
    layout_wrappers: &str,
    loading_name: &str,
    meta_names: &str,
) -> String {
    let meta_child = if meta_names.is_empty() {
        String::new()
    } else {
        format!("__ruvyxaMetaElement(__ruvyxaResolveMeta([{meta_names}], ctx)), ")
    };
    format!(
        "function __ruvyxaShell(ctx) {{\n  let tree = React.createElement({loading_name}, null);\n  for (const Layout of [{layout_wrappers}].reverse()) {{\n    tree = React.createElement(Layout, null, tree);\n  }}\n  return React.createElement(__ruvyxaRouteContext.Provider, {{\n    value: {{ pathname: ctx.path, params: ctx.params ?? {{}}, route: {route_path_literal}, flight: undefined }},\n  }}, {meta_child}tree);\n}}"
    )
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

    /// A client-rendered route is served as a shell the server never rendered
    /// the route tree into. Hydrating it is a guaranteed mismatch — React
    /// discarded the document and reported #418 — so the bootstrap has to be
    /// able to mount instead, chosen by the flag the shell sets.
    #[test]
    fn client_entry_mounts_a_csr_shell_and_hydrates_a_rendered_one() {
        let (source, _) = build_entry_source(&input("/project/app/page.tsx", vec![], "/"));

        assert!(
            source.contains("import { createRoot, hydrateRoot } from \"react-dom/client\";"),
            "{source}"
        );
        assert!(source.contains("globalThis.__RUVYXA_CSR__"), "{source}");
        assert!(source.contains("createRoot(document)"), "{source}");
        assert!(
            source.contains("hydrateRoot(document, __ruvyxaTreeElement)"),
            "a server-rendered document must still hydrate: {source}"
        );
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
        // The route pattern reaches three interpolation sites — the registry
        // key, the pattern global the client router reads, and the routing
        // context — and every one must stay escaped.
        assert!(
            source.contains(r#"["/a\";globalThis.pwned=1;\""] = __ruvyxaTree;"#),
            "{source}"
        );
        assert!(
            source
                .contains(r#"globalThis.__RUVYXA_ROUTE_PATTERN__ = "/a\";globalThis.pwned=1;\"";"#),
            "{source}"
        );
        assert!(
            source.contains(r#"route: "/a\";globalThis.pwned=1;\"", flight: ctx.flight },"#),
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
    }

    /// One client bundle serves every URL of a dynamic route, so it has no
    /// concrete request path to fall back to. Falling back to the route pattern
    /// made `usePathname()` return `/blog/[slug]` whenever the server did not
    /// inject `__RUVYXA_REQUEST_PATH__`, which is never a real pathname.
    #[test]
    fn client_entry_falls_back_to_the_browser_location_not_the_route_pattern() {
        let (source, _) = build_entry_source(&input(
            "/project/app/blog/[slug]/page.tsx",
            Vec::new(),
            "/blog/[slug]",
        ));

        assert!(
            source.contains(r#"globalThis.__RUVYXA_REQUEST_PATH__ ?? (typeof location ==="#),
            "{source}"
        );
        assert!(
            !source.contains(r#"__RUVYXA_REQUEST_PATH__ ?? "/blog/[slug]""#),
            "the pattern must never be used as a pathname: {source}"
        );
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
        // The registry is keyed by pattern, so the client router needs the
        // pattern published alongside it to address its own initial route.
        // `packages/ruvyxa/runtime/entry-templates.mjs` emits the same line from
        // `routeRegistration`; the two must stay in lockstep.
        assert!(
            source.contains(r#"globalThis.__RUVYXA_ROUTE_PATTERN__ = "/blog/[slug]";"#),
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

    /// The shell is what makes a navigation paint immediately: it holds the
    /// destination's layouts and its `loading.tsx`, all of which are already in
    /// this bundle, so the client router can render it without waiting for the
    /// Flight payload.
    #[test]
    fn emits_a_loading_shell_the_client_router_can_paint() {
        let mut bundle = input(
            "/project/app/blog/[slug]/page.tsx",
            vec!["/project/app/layout.tsx"],
            "/blog/[slug]",
        );
        bundle.specials = RouteSpecials {
            loading: Some(PathBuf::from("/project/app/loading.tsx")),
            ..RouteSpecials::default()
        };
        let (source, _) = build_entry_source(&bundle);

        assert!(source.contains("function __ruvyxaShell(ctx)"), "{source}");
        assert!(
            source.contains(
                r#";(globalThis.__RUVYXA_SHELLS__ ||= {})["/blog/[slug]"] = __ruvyxaShell;"#
            ),
            "{source}"
        );
        // The shell renders the loading component inside the layouts, with no
        // page and no Flight payload — a stale payload from the page being
        // navigated away from must never reach it.
        assert!(
            source.contains("let tree = React.createElement(RouteLoading, null);"),
            "{source}"
        );
        assert!(source.contains("flight: undefined"), "{source}");
        assert!(
            !source
                .split("function __ruvyxaShell")
                .nth(1)
                .unwrap()
                .starts_with(
                    "(ctx) {
  let tree = React.createElement(Page"
                ),
            "the shell must not render the page: {source}"
        );
    }

    /// A route with no `loading.tsx` has no declared loading state. Painting a
    /// blank shell for it would be worse than leaving the previous page up.
    #[test]
    fn omits_the_shell_when_a_route_declares_no_loading_state() {
        let (source, _) = build_entry_source(&input("/project/app/page.tsx", Vec::new(), "/"));
        assert!(!source.contains("__ruvyxaShell"), "{source}");
        assert!(!source.contains("__RUVYXA_SHELLS__"), "{source}");
    }

    /// A server render has its data in hand and never shows a loading fallback,
    /// so shipping the shell there would be dead bytes on every SSR bundle.
    #[test]
    fn server_entries_carry_no_shell() {
        for target in [BundleTarget::Ssr, BundleTarget::Edge] {
            let mut bundle = input("/project/app/page.tsx", Vec::new(), "/");
            bundle.target = target;
            bundle.specials = RouteSpecials {
                loading: Some(PathBuf::from("/project/app/loading.tsx")),
                ..RouteSpecials::default()
            };
            let (source, _) = build_entry_source(&bundle);
            assert!(!source.contains("__RUVYXA_SHELLS__"), "{source}");
        }
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
    fn entry_reimports_page_and_layouts_as_metadata_namespaces() {
        // A default import cannot see a sibling `export const meta`; the
        // namespace re-import is what makes route metadata readable. Order is
        // root layout -> leaf layout -> page, least specific first.
        let (source, _) = build_entry_source(&input(
            "/project/app/blog/page.tsx",
            vec!["/project/app/layout.tsx", "/project/app/blog/layout.tsx"],
            "/blog",
        ));

        assert!(
            source.contains(r#"import * as __ruvyxaMeta0 from "/project/app/layout.tsx";"#),
            "{source}"
        );
        assert!(
            source.contains(r#"import * as __ruvyxaMeta1 from "/project/app/blog/layout.tsx";"#),
            "{source}"
        );
        assert!(
            source.contains(r#"import * as __ruvyxaMeta2 from "/project/app/blog/page.tsx";"#),
            "{source}"
        );
        assert!(
            source.contains(
                "__ruvyxaMetaElement(__ruvyxaResolveMeta([__ruvyxaMeta0, __ruvyxaMeta1, __ruvyxaMeta2], ctx))"
            ),
            "{source}"
        );
    }

    #[test]
    fn metadata_is_composed_outside_the_layouts() {
        // A layout that suspends must not be able to hold the document title
        // back past the flushed shell.
        let (source, _) = build_entry_source(&input(
            "/project/app/page.tsx",
            vec!["/project/app/layout.tsx"],
            "/",
        ));

        let layouts = source.find("[Layout0].reverse()").expect("layout wrap");
        let meta = source
            .find("__ruvyxaMetaElement(__ruvyxaResolveMeta")
            .expect("metadata composition");
        assert!(meta > layouts, "{source}");
        // A sibling child of the provider, not a wrapper element per render.
        assert!(
            source.contains("}, __ruvyxaMetaElement(__ruvyxaResolveMeta([__ruvyxaMeta0, __ruvyxaMeta1], ctx)), tree);"),
            "{source}"
        );
        assert!(!source.contains("React.Fragment"), "{source}");
    }

    #[test]
    fn server_entries_rewrite_the_document_lang_from_metadata() {
        // `lang` belongs to the `<html>` element the app renders, which no
        // hoisted child element can reach — the finished document is rewritten.
        for target in [BundleTarget::Ssr, BundleTarget::Edge] {
            let mut bundle = input("/project/app/page.tsx", Vec::new(), "/");
            bundle.target = target;
            let (source, _) = build_entry_source(&bundle);

            assert!(
                source.contains(
                    "return __ruvyxaApplyLang(html, __ruvyxaResolveMeta([__ruvyxaMeta0], ctx).lang);"
                ),
                "{source}"
            );
        }

        // The client bundle hydrates into a document whose lang the server
        // already set, so it carries no rewrite call.
        let (client, _) = build_entry_source(&input("/project/app/page.tsx", Vec::new(), "/"));
        assert!(!client.contains("__ruvyxaApplyLang(html"), "{client}");
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
                    r#"value: { pathname: ctx.path, params: ctx.params ?? {}, route: "/", flight: ctx.flight },"#
                ),
                "{source}"
            );
            // The server must not publish a client registry: there is no root
            // to re-render into, and the global would leak across requests.
            assert!(!source.contains("__RUVYXA_ROUTES__"), "{source}");
        }
    }
}
