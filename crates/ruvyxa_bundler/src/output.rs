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

    let request_path = js_string(&input.request_path);

    let source = match input.target {
        BundleTarget::Client => {
            format!(
                r#"import React from "react";
import {{ hydrateRoot }} from "react-dom/client";
import Page from {page_path};
{layout_imports}
const params = globalThis.__RUVYXA_ROUTE_PARAMS__ ?? {{}};
const currentPath = globalThis.__RUVYXA_REQUEST_PATH__ ?? {request_path};
let tree = React.createElement(Page, {{ params, requestPath: currentPath }});
for (const Layout of [{layout_wrappers}].reverse()) {{
  tree = React.createElement(Layout, null, tree);
}}
if (globalThis.__RUVYXA_ROOT__) {{
  globalThis.__RUVYXA_ROOT__.render(tree);
}} else {{
  globalThis.__RUVYXA_ROOT__ = hydrateRoot(document, tree);
}}
window.__RUVYXA_HYDRATED = true;
"#
            )
        }
        BundleTarget::Ssr => {
            format!(
                r#"import React from "react";
import {{ renderToString }} from "react-dom/server";
import Page from {page_path};
{layout_imports}
export async function render(ctx) {{
  let tree = React.createElement(Page, {{ params: ctx.params ?? {{}}, requestPath: ctx.path }});
  for (const Layout of [{layout_wrappers}].reverse()) {{
    tree = React.createElement(Layout, null, tree);
  }}
  return "<!doctype html>" + renderToString(tree);
}}
"#
            )
        }
        BundleTarget::Edge => {
            format!(
                r#"import React from "react";
import {{ renderToString }} from "react-dom/server";
import Page from {page_path};
{layout_imports}
export async function render(ctx) {{
  let tree = React.createElement(Page, {{ params: ctx.params ?? {{}}, requestPath: ctx.path }});
  for (const Layout of [{layout_wrappers}].reverse()) {{
    tree = React.createElement(Layout, null, tree);
  }}
  return "<!doctype html>" + renderToString(tree);
}}
"#
            )
        }
    };

    (source, label)
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
    use crate::{BundleOptions, BundleTarget};
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
            source.contains(r#"?? "/a\";globalThis.pwned=1;\"";"#),
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
        assert!(source.contains(r#"?? "/blog/[slug]";"#));
    }
}
