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

/// Build the virtual entry source that wires layouts + page together.
///
/// Returns `(source_string, virtual_label)`.
pub fn build_entry_source(input: &BundleInput) -> (String, String) {
    let label = "ruvyxa:bundle-entry.tsx".to_string();

    let page_path = input.entry.display().to_string().replace('\\', "/");

    // Collect layout imports (root-to-leaf order).
    let layout_imports: String = input
        .layouts
        .iter()
        .enumerate()
        .map(|(i, layout)| {
            let lp = layout.display().to_string().replace('\\', "/");
            format!("import Layout{i} from \"{lp}\";\n")
        })
        .collect();

    let layout_wrappers: String = (0..input.layouts.len())
        .map(|i| format!("Layout{i}"))
        .collect::<Vec<_>>()
        .join(", ");

    let request_path = &input.request_path;

    let source = match input.target {
        BundleTarget::Client => {
            format!(
                r#"import React from "react";
import {{ hydrateRoot }} from "react-dom/client";
import Page from "{page_path}";
{layout_imports}
const params = globalThis.__RUVYXA_ROUTE_PARAMS__ ?? {{}};
const currentPath = globalThis.__RUVYXA_REQUEST_PATH__ ?? "{request_path}";
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
import Page from "{page_path}";
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
import Page from "{page_path}";
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
