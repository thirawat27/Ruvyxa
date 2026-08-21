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

use std::path::PathBuf;

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

    // `template.tsx` on the path to this route, imported alongside the layouts
    // and interleaved with them by directory during composition.
    let template_imports: String = input
        .templates
        .iter()
        .enumerate()
        .map(|(i, template)| {
            let path = js_string(&template.display().to_string().replace('\\', "/"));
            format!("import Template{i} from {path};\n")
        })
        .collect();
    let slot_imports: String = input
        .slots
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            let path = js_string(&slot.file.display().to_string().replace('\\', "/"));
            format!("import Slot{i} from {path};\n")
        })
        .collect();
    let intercept_imports: String = input
        .intercepts
        .iter()
        .enumerate()
        .map(|(i, intercept)| {
            let path = js_string(&intercept.file.display().to_string().replace('\\', "/"));
            format!("import Intercept{i} from {path};\n")
        })
        .collect();
    let wrapper_levels = route_wrapper_levels(
        &input.layouts,
        &input.templates,
        &input.slots,
        &input.intercepts,
    );
    // A route with no interceptions emits neither the resolver nor the table,
    // so its bundle stays byte-identical to the one it produced before the
    // feature existed. Both are client-only: an interception is a soft
    // navigation, and a server render has no previous page to overlay.
    let slot_prelude = if input.intercepts.is_empty() {
        String::new()
    } else {
        ROUTE_SLOT_PRELUDE.to_string()
    };

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
    let intercept_registry = intercept_registry_statement(&route_pattern, &input.intercepts);

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
        &wrapper_levels,
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
            route_shell_function(
                &route_pattern,
                &layout_wrappers,
                &wrapper_levels,
                loading,
                &meta_names
            )
        ),
        _ => String::new(),
    };

    let source = match input.target {
        BundleTarget::Client => {
            format!(
                r#"import React from "react";
import {{ createRoot, hydrateRoot }} from "react-dom/client";
import Page from {page_path};
{layout_imports}{template_imports}{slot_imports}{intercept_imports}{special_imports}{meta_imports}
{ROUTE_CONTEXT_PRELUDE}{boundary_prelude}{slot_prelude}
{META_PRELUDE}

{route_tree}
;(globalThis.__RUVYXA_ROUTES__ ||= {{}})[{route_pattern}] = __ruvyxaTree;
globalThis.__RUVYXA_ROUTE_PATTERN__ = {route_pattern};
{intercept_registry}
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
        BundleTarget::Ssr | BundleTarget::Edge | BundleTarget::ReactServer => {
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
/// `tests/packages/ruvyxa/entry-prelude-parity.test.mjs` executes both copies
/// against one stand-in React and asks them the same questions. It compares
/// behaviour rather than bytes because the two are formatted by different
/// tools — this literal carries statement terminators the Prettier-formatted
/// template does not — and a byte comparison would fail on the formatting while
/// passing on a prelude that published the wrong context.
///
/// This comment used to name `entry-templates.test.mjs`, which only ever
/// exercised the JavaScript half; nothing read this file, so the gate it
/// promised did not exist.
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
/// `packages/ruvyxa/runtime/entry-templates.mjs`, and is held to it by
/// `tests/packages/ruvyxa/entry-prelude-parity.test.mjs`, which runs both
/// copies through the same boundary behaviour. Defined inline rather than
/// imported because a generated entry cannot depend on `@ruvyxa/react`; it
/// tells a `notFound()` signal apart from an ordinary error by the own property
/// `error.__ruvyxaNotFound` that `notFound()` stamps.
/// The slot resolver a route with interceptions emits.
///
/// A slot normally renders one fixed component. An interception replaces that
/// content for as long as the URL names it, while the page underneath stays
/// mounted — so the slot has to be decided per render rather than baked in.
/// `ctx.intercept` is set by the client router and is absent on the server and
/// on a hard load, which is what makes a refresh show the real page.
///
/// A slot may carry an interception without having a default of its own, so
/// `Default` is nullable rather than assumed.
///
/// Mirrored by `ROUTE_SLOT_PRELUDE` in
/// `packages/ruvyxa/runtime/entry-templates.mjs` and held to it by
/// `tests/packages/ruvyxa/entry-prelude-parity.test.mjs`.
const ROUTE_SLOT_PRELUDE: &str = r#"function __ruvyxaSlot(ctx, level, name, Default, intercepts) {
  const active = ctx.intercept;
  if (active && active.level === level && active.name === name) {
    for (const entry of intercepts) {
      if (entry[0] === active.target) {
        return React.createElement(entry[1], {
          params: active.params ?? {},
          requestPath: active.path ?? ctx.path,
        });
      }
    }
  }
  return Default
    ? React.createElement(Default, { params: ctx.params ?? {}, requestPath: ctx.path })
    : null;
}
"#;

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

/// Emit the loop that wraps the page in its layouts and templates.
///
/// A route with no `template.tsx` emits exactly the loop it always did: nothing
/// about an ordinary route's bundle changes because the feature exists.
///
/// With templates, the loop walks levels instead of layouts, because the two
/// interleave — Next.js nests `layout > template > children` at each level, and
/// a level may have either, both, or neither. Flattening that into "every
/// template inside every layout" would put a layout outside a template that
/// should have contained it, which is observable the moment a template provides
/// context. `levels` is that interleaved list, already ordered root-first.
///
/// The template's `key` is the whole reason the file exists: React remounts a
/// keyed element when the key changes, so navigating within the same layout
/// resets the template's state and re-runs its effects, while the layout above
/// it stays mounted.
///
/// Mirrors `wrapperLoop()` in `packages/ruvyxa/runtime/entry-templates.mjs`;
/// both are pinned by `tests/fixtures/entry-composition-conformance.json`.
fn wrapper_loop(layout_wrappers: &str, levels: &[WrapperLevel]) -> String {
    if levels.iter().all(|level| {
        level.template.is_none() && level.slots.is_empty() && level.intercepts.is_empty()
    }) {
        return format!(
            "  for (const Layout of [{layout_wrappers}].reverse()) {{\n    tree = React.createElement(Layout, null, tree);\n  }}"
        );
    }

    let triples = levels
        .iter()
        .map(|level| {
            // Slots are built here rather than hoisted, because the elements
            // depend on `ctx` and this loop runs once per render.
            let slots = wrapper_level_slots(level);
            format!(
                "[{}, {}, {slots}]",
                level.layout.as_deref().unwrap_or("null"),
                level.template.as_deref().unwrap_or("null")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "  for (const [Layout, Template, slots] of [{triples}].reverse()) {{\n    if (Template) tree = React.createElement(Template, {{ key: ctx.path }}, tree);\n    if (Layout) tree = React.createElement(Layout, slots, tree);\n  }}"
    )
}

/// One directory level of a route's wrapper chain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WrapperLevel {
    pub(crate) layout: Option<String>,
    pub(crate) template: Option<String>,
    /// Parallel-route slots this level's layout receives, as
    /// `(prop name, component identifier)` in name order.
    pub(crate) slots: Vec<(String, String)>,
    /// Interceptions this level's slots can show, as
    /// `(prop name, route id, target pattern, component identifier)`.
    ///
    /// A slot can carry an interception without having a default page of its
    /// own — `@modal` holding nothing but `(.)photo` is the ordinary shape — so
    /// this is not a lookup into `slots`.
    pub(crate) intercepts: Vec<WrapperIntercept>,
}

/// One interception a level's slot can render instead of its default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WrapperIntercept {
    pub(crate) name: String,
    pub(crate) level_id: String,
    pub(crate) target: String,
    pub(crate) component: String,
}

/// Publish what this route can intercept, for the client router to match.
///
/// Only the metadata travels: the router needs to know *whether* a URL is
/// intercepted from here, and the component that answers it is already in this
/// bundle behind `__ruvyxaSlot`. Putting components in a global would duplicate
/// references the route's own tree already holds.
///
/// Mirrors `interceptRegistryStatement()` in
/// `packages/ruvyxa/runtime/entry-templates.mjs`.
fn intercept_registry_statement(
    route_pattern: &str,
    intercepts: &[crate::RouteInterceptInput],
) -> String {
    if intercepts.is_empty() {
        return String::new();
    }
    let entries = intercepts
        .iter()
        .map(|intercept| {
            format!(
                "{{ level: {}, name: {}, target: {} }}",
                js_string(&intercept.level_id),
                js_string(&intercept.name),
                js_string(&intercept.target)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(";(globalThis.__RUVYXA_INTERCEPTS__ ||= {{}})[{route_pattern}] = [{entries}];")
}

/// The `slots` prop object one level's layout receives, or `null`.
///
/// A slot with no interception emits exactly the element it always did, so a
/// project that uses parallel routes and nothing else produces the bundle it
/// produced before interceptions existed. A slot that *can* be intercepted goes
/// through `__ruvyxaSlot`, which decides per render — and a slot that exists
/// only to hold an interception has no default at all, which is the ordinary
/// `@modal` shape.
///
/// Mirrors `levelSlots()` in `packages/ruvyxa/runtime/entry-templates.mjs`.
fn wrapper_level_slots(level: &WrapperLevel) -> String {
    if level.slots.is_empty() && level.intercepts.is_empty() {
        return "null".to_string();
    }

    let mut names: Vec<&str> = level
        .slots
        .iter()
        .map(|(name, _)| name.as_str())
        .chain(level.intercepts.iter().map(|entry| entry.name.as_str()))
        .collect();
    names.sort_unstable();
    names.dedup();

    let props = names
        .into_iter()
        .map(|name| {
            let default = level
                .slots
                .iter()
                .find(|(slot_name, _)| slot_name == name)
                .map(|(_, component)| component.as_str());
            let intercepts = level
                .intercepts
                .iter()
                .filter(|entry| entry.name == name)
                .collect::<Vec<_>>();
            if intercepts.is_empty() {
                let component = default.unwrap_or("null");
                return format!(
                    "{}: React.createElement({component}, {{ params: ctx.params ?? {{}}, requestPath: ctx.path }})",
                    js_string(name)
                );
            }
            let table = intercepts
                .iter()
                .map(|entry| format!("[{}, {}]", js_string(&entry.target), entry.component))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{}: __ruvyxaSlot(ctx, {}, {}, {}, [{table}])",
                js_string(name),
                js_string(&intercepts[0].level_id),
                js_string(name),
                default.unwrap_or("null")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{ {props} }}")
}

/// Interleave the layout and template chains into one root-first level list.
///
/// Both chains are ordered root-first and each entry names a file, so the
/// directory holding it is the level. Merging on that directory is what keeps
/// `layout > template` correct at every level even when only one of the two
/// exists there.
///
/// Mirrors `wrapperLevels()` in `packages/ruvyxa/runtime/entry-templates.mjs`.
pub(crate) fn route_wrapper_levels(
    layouts: &[PathBuf],
    templates: &[PathBuf],
    slots: &[crate::RouteSlotInput],
    intercepts: &[crate::RouteInterceptInput],
) -> Vec<WrapperLevel> {
    let directory = |file: &PathBuf| {
        file.parent()
            .map(|parent| parent.display().to_string().replace('\\', "/"))
            .unwrap_or_default()
    };

    let mut levels: Vec<(String, WrapperLevel)> = Vec::new();
    let mut push = |key: String, assign: &dyn Fn(&mut WrapperLevel)| match levels
        .iter_mut()
        .find(|(existing, _)| *existing == key)
    {
        Some((_, level)) => assign(level),
        None => {
            let mut level = WrapperLevel::default();
            assign(&mut level);
            levels.push((key, level));
        }
    };

    for (index, layout) in layouts.iter().enumerate() {
        let name = format!("Layout{index}");
        push(directory(layout), &|level: &mut WrapperLevel| {
            level.layout = Some(name.clone());
        });
    }
    for (index, template) in templates.iter().enumerate() {
        let name = format!("Template{index}");
        push(directory(template), &|level: &mut WrapperLevel| {
            level.template = Some(name.clone());
        });
    }
    // A slot names the directory holding its `@name` folder, which is the level
    // whose layout receives it — the same key the two chains merge on.
    for (index, slot) in slots.iter().enumerate() {
        let entry = (slot.name.clone(), format!("Slot{index}"));
        let key = slot
            .level
            .display()
            .to_string()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string();
        push(key, &|level: &mut WrapperLevel| {
            level.slots.push(entry.clone());
        });
    }
    // An interception merges on the same key as a slot: it replaces that slot's
    // content while the page underneath stays mounted, so it belongs to the
    // level whose layout receives the prop.
    for (index, intercept) in intercepts.iter().enumerate() {
        let entry = WrapperIntercept {
            name: intercept.name.clone(),
            level_id: intercept.level_id.clone(),
            target: intercept.target.clone(),
            component: format!("Intercept{index}"),
        };
        let key = intercept
            .level
            .display()
            .to_string()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string();
        push(key, &|level: &mut WrapperLevel| {
            level.intercepts.push(entry.clone());
        });
    }

    // Root-first. A shorter directory is always an ancestor here, because every
    // entry lies on one path from the app root to the route.
    levels.sort_by_key(|(key, _)| key.matches('/').count());
    levels.into_iter().map(|(_, level)| level).collect()
}

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
    template_levels: &[WrapperLevel],
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
    lines.push(wrapper_loop(layout_wrappers, template_levels));
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
    levels: &[WrapperLevel],
    loading_name: &str,
    meta_names: &str,
) -> String {
    let meta_child = if meta_names.is_empty() {
        String::new()
    } else {
        format!("__ruvyxaMetaElement(__ruvyxaResolveMeta([{meta_names}], ctx)), ")
    };
    let wrappers = wrapper_loop(layout_wrappers, levels);
    format!(
        "function __ruvyxaShell(ctx) {{\n  let tree = React.createElement({loading_name}, null);\n{wrappers}\n  return React.createElement(__ruvyxaRouteContext.Provider, {{\n    value: {{ pathname: ctx.path, params: ctx.params ?? {{}}, route: {route_path_literal}, flight: undefined }},\n  }}, {meta_child}tree);\n}}"
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
        BundleTarget::Ssr | BundleTarget::Edge | BundleTarget::ReactServer => {
            // The linker hoists external ESM imports and exposes the virtual
            // entry render function as a top-level ESM export.
            format!("// Ruvyxa SSR bundle\n{linked}")
        }
    }
}

#[cfg(test)]
mod tests {
    /// Read a fixture level's `intercepts` list.
    ///
    /// The fixture carries the shape the JavaScript generator consumes
    /// directly, so one spelling feeds both replays.
    fn fixture_intercepts(value: &serde_json::Value) -> Vec<WrapperIntercept> {
        value
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| WrapperIntercept {
                        name: entry["name"].as_str().unwrap_or_default().to_string(),
                        level_id: entry["level"].as_str().unwrap_or_default().to_string(),
                        target: entry["target"].as_str().unwrap_or_default().to_string(),
                        component: entry["component"].as_str().unwrap_or_default().to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    use super::*;
    use crate::{BundleOptions, BundleTarget, RouteSpecials};
    use std::path::PathBuf;

    fn input(entry: &str, layouts: Vec<&str>, request_path: &str) -> BundleInput {
        BundleInput {
            entry: PathBuf::from(entry),
            project_root: PathBuf::from("/project"),
            app_dir: PathBuf::from("/project/app"),
            layouts: layouts.into_iter().map(PathBuf::from).collect(),
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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

    /// Strip statement terminators so the two generators can be compared.
    ///
    /// These literals are written with semicolons and the Node templates
    /// without them — Prettier owns the second and neither reaches a reader, so
    /// a byte comparison would fail on formatting while passing on a reordered
    /// composition. Only a `;` that ends a line goes, which is why the fixture
    /// forbids one inside a route path.
    fn without_terminators(source: &str) -> String {
        source
            .lines()
            .map(|line| line.strip_suffix(';').unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn joined(value: &serde_json::Value) -> String {
        value
            .as_array()
            .expect("fixture list")
            .iter()
            .map(|entry| entry.as_str().expect("fixture string"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Read a case's `wrapperLevels`, absent on every layout-only case.
    ///
    /// The levels are spelled out rather than derived from the two chains,
    /// because deriving them is a separate rule with its own test on each side
    /// — `route_wrapper_levels` here and `wrapperLevels()` in the Node
    /// templates. This table pins what the levels *emit*.
    fn fixture_wrapper_levels(value: &serde_json::Value) -> Vec<WrapperLevel> {
        value
            .as_array()
            .map(|levels| {
                levels
                    .iter()
                    .map(|level| WrapperLevel {
                        layout: level["layout"].as_str().map(str::to_string),
                        template: level["template"].as_str().map(str::to_string),
                        intercepts: fixture_intercepts(&level["intercepts"]),
                        slots: level["slots"]
                            .as_object()
                            .map(|slots| {
                                slots
                                    .iter()
                                    .map(|(name, component)| {
                                        (
                                            name.clone(),
                                            component.as_str().expect("slot component").to_string(),
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Composition order, held to the table the Node entry templates replay.
    ///
    /// Both bundlers emit a route's element tree, and a project renders through
    /// whichever one built it. The order is the contract: the boundary inside
    /// the Suspense so a synchronous throw renders its own UI rather than the
    /// loading fallback, the layouts wrapping outward, and the metadata as a
    /// sibling rather than a wrapper so a suspended layout cannot hold the
    /// document title past the flushed shell.
    #[test]
    fn route_composition_matches_the_shared_conformance_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/entry-composition-conformance.json"
        ))
        .unwrap();
        let names = &fixture["names"];

        // The Node generator takes these as arguments while this one writes
        // them into its format strings; the fixture is where the two meet.
        assert_eq!(names["tree"], "__ruvyxaTree");
        assert_eq!(names["shell"], "__ruvyxaShell");
        assert_eq!(names["page"], "Page");

        for case in fixture["cases"].as_array().unwrap() {
            let input = &case["input"];
            let route_path = input["routePath"].as_str().unwrap();
            assert!(
                !route_path.contains(';'),
                "a route path with a semicolon breaks the comparison"
            );
            let route_literal = js_string(route_path);
            let layouts = joined(&input["layoutNames"]);
            let meta_names = joined(&input["metaNames"]);
            let levels = fixture_wrapper_levels(&input["wrapperLevels"]);

            let generated = match case["kind"].as_str().unwrap() {
                "tree" => route_tree_function(
                    &route_literal,
                    &layouts,
                    &levels,
                    input["errorName"].as_str(),
                    input["loadingName"].as_str(),
                    input["notFoundName"].as_str(),
                    &meta_names,
                ),
                "shell" => route_shell_function(
                    &route_literal,
                    &layouts,
                    &levels,
                    input["loadingName"].as_str().unwrap(),
                    &meta_names,
                ),
                other => panic!("unknown composition kind {other}"),
            };

            let expected = joined_lines(&case["source"]);
            assert_eq!(
                without_terminators(&generated),
                expected,
                "{}",
                case["$why"].as_str().unwrap_or_default()
            );
        }
    }

    fn joined_lines(value: &serde_json::Value) -> String {
        value
            .as_array()
            .expect("fixture source lines")
            .iter()
            .map(|entry| entry.as_str().expect("fixture string"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Layouts and templates merge on the directory that holds them.
    ///
    /// This is the rule that keeps `layout > template` correct at every level.
    /// Flattening it into "every template inside every layout" is the tempting
    /// shortcut and it is wrong: `Layout1` below would end up outside
    /// `Template0`, when it belongs inside it, and a template that provides
    /// context would stop reaching the layout beneath it.
    #[test]
    fn wrapper_levels_merge_a_layout_and_a_template_on_their_directory() {
        let levels = route_wrapper_levels(
            &[
                PathBuf::from("/p/app/layout.tsx"),
                PathBuf::from("/p/app/dash/layout.tsx"),
            ],
            &[
                PathBuf::from("/p/app/template.tsx"),
                PathBuf::from("/p/app/dash/reports/template.tsx"),
            ],
            &[],
            &[],
        );

        assert_eq!(
            levels,
            vec![
                WrapperLevel {
                    layout: Some("Layout0".to_string()),
                    template: Some("Template0".to_string()),
                    slots: Vec::new(),
                    intercepts: Vec::new(),
                },
                WrapperLevel {
                    layout: Some("Layout1".to_string()),
                    template: None,
                    slots: Vec::new(),
                    intercepts: Vec::new(),
                },
                WrapperLevel {
                    layout: None,
                    template: Some("Template1".to_string()),
                    slots: Vec::new(),
                    intercepts: Vec::new(),
                },
            ]
        );
    }

    /// A route with no template emits exactly the loop it always did.
    ///
    /// The feature existing must not change one byte of an ordinary route's
    /// bundle — every project that has never heard of `template.tsx` keeps the
    /// output it had.
    #[test]
    fn a_route_without_templates_emits_the_layout_only_loop() {
        let levels = route_wrapper_levels(
            &[
                PathBuf::from("/p/app/layout.tsx"),
                PathBuf::from("/p/app/dash/layout.tsx"),
            ],
            &[],
            &[],
            &[],
        );
        assert_eq!(
            wrapper_loop("Layout0, Layout1", &levels),
            "  for (const Layout of [Layout0, Layout1].reverse()) {\n    tree = React.createElement(Layout, null, tree);\n  }"
        );
    }

    /// A `template.tsx` reaches the emitted entry, keyed by request path.
    ///
    /// The key is the whole point of the file: React remounts a keyed element
    /// when the key changes, so navigating within the same layout resets the
    /// template while the layout above it stays mounted. Without it a template
    /// would be a layout with a different filename.
    #[test]
    fn a_template_is_imported_and_keyed_by_the_request_path() {
        let mut input = BundleInput {
            entry: PathBuf::from("/p/app/dash/page.tsx"),
            project_root: PathBuf::from("/p"),
            app_dir: PathBuf::from("/p/app"),
            layouts: vec![PathBuf::from("/p/app/layout.tsx")],
            templates: vec![PathBuf::from("/p/app/dash/template.tsx")],
            slots: Vec::new(),
            intercepts: Vec::new(),
            request_path: "/dash".to_string(),
            target: BundleTarget::Client,
            options: BundleOptions::default(),
            specials: RouteSpecials::default(),
        };
        let (source, _) = build_entry_source(&input);

        assert!(
            source.contains("import Template0 from \"/p/app/dash/template.tsx\""),
            "{source}"
        );
        assert!(
            source.contains("React.createElement(Template, { key: ctx.path }, tree)"),
            "{source}"
        );
        assert!(
            source.contains("[[Layout0, null, null], [null, Template0, null]].reverse()"),
            "the template's own level has no layout: {source}"
        );

        // And the same route without the template is untouched. Matched on the
        // emitted identifier rather than the word: `titleTemplate` lives in the
        // metadata prelude and is not this.
        input.templates.clear();
        let (plain, _) = build_entry_source(&input);
        assert!(!plain.contains("Template0"), "{plain}");
        assert!(
            plain.contains("for (const Layout of [Layout0].reverse())"),
            "{plain}"
        );
    }
}
