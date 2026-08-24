//! Post-emit audit of what a build actually wrote.
//!
//! Every other check in `ruvyxa build` asks about the *inputs*: does the graph
//! resolve, does the boundary hold, does the render succeed. Nothing read the
//! **output**, and two production incidents came through that gap:
//!
//! - a client chunk shipped `from "../lib/cn"` — a source-level specifier the
//!   linker never rewrote. The browser resolved it against the chunk's own URL,
//!   asked for `/__ruvyxa/lib/cn`, got a 404, and stopped loading the module
//!   graph. The page was already server-rendered, so it looked completely
//!   normal while every button, input, and link did nothing.
//! - a deployed build referenced a stylesheet that was in no output directory,
//!   and the whole site rendered with browser default styling.
//!
//! Both are cheap to see once the files exist: read what was emitted, resolve
//! every specifier and every asset URL the documents reference, and fail the
//! build when one names nothing. A red build is what either of those should
//! have been.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use ruvyxa_diagnostics::Diagnostic;

/// One reference in emitted output that resolves to no emitted file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DanglingReference {
    /// The emitted file that carries the reference, relative to the output.
    pub(crate) from: String,
    /// The specifier or URL exactly as it was written.
    pub(crate) reference: String,
}

/// Audit the emitted client chunks and pre-rendered documents.
///
/// `client_dir` and `prerender_dir` are the staged directories, and
/// `assets_dir` is where `public/` was published to; a URL may name a file in
/// either. Returns every dangling reference, sorted, so one build reports all of
/// them rather than the first.
pub(crate) fn audit_emitted_output(
    client_dir: &Path,
    prerender_dir: &Path,
    assets_dir: &Path,
) -> Vec<DanglingReference> {
    let mut dangling = BTreeSet::new();
    audit_client_chunks(client_dir, &mut dangling);
    audit_documents(prerender_dir, client_dir, assets_dir, &mut dangling);
    dangling.into_iter().collect()
}

/// Turn a dangling reference list into the diagnostic a build fails with.
pub(crate) fn dangling_reference_diagnostic(dangling: &[DanglingReference]) -> Option<Diagnostic> {
    let first = dangling.first()?;
    let detail = dangling
        .iter()
        .take(5)
        .map(|entry| format!("  {} references {}", entry.from, entry.reference))
        .collect::<Vec<_>>()
        .join("\n");
    let more = if dangling.len() > 5 {
        format!("\n  … and {} more", dangling.len() - 5)
    } else {
        String::new()
    };

    Some(
        Diagnostic::new(
            "RUV1213",
            format!(
                "emitted output references {} file(s) the build did not write",
                dangling.len()
            ),
        )
        .explain(format!(
            "A reference that resolves to nothing is a 404 in the browser, and the failure reads as \
             a broken page rather than a broken build — an unresolved module specifier stops \
             hydration, and a missing stylesheet renders the site unstyled.\n{detail}{more}"
        ))
        .suggest(
            "Check the importing module for a specifier the bundler could not resolve, and check \
             that every asset the document references is emitted into the build output.",
        )
        .at_file(PathBuf::from(&first.from)),
    )
}

/// Every emitted `.js` chunk, checked for specifiers that name no emitted file.
fn audit_client_chunks(client_dir: &Path, dangling: &mut BTreeSet<DanglingReference>) {
    for file in emitted_files(client_dir, "js") {
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        // The bundler's own scanner, not a second one: a specifier inside a
        // string or a comment is not an edge, and this file is the last place
        // that distinction should be re-implemented.
        let ast = ruvyxa_bundler::ast::parse_module(&source);
        for edge in &ast.imports {
            let specifier = edge.specifier.as_str();
            let Some(target) = emitted_target(client_dir, &file, specifier) else {
                continue;
            };
            if !target.is_file() {
                dangling.insert(DanglingReference {
                    from: display_name(client_dir, &file),
                    reference: specifier.to_string(),
                });
            }
        }
    }
}

/// The emitted file a specifier names, or `None` when it names something this
/// audit cannot resolve — a bare package specifier or an absolute URL, both of
/// which are resolved by something other than this output directory.
fn emitted_target(client_dir: &Path, from: &Path, specifier: &str) -> Option<PathBuf> {
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let base = from.parent()?;
        return Some(normalize(&base.join(specifier)));
    }
    let rest = specifier.strip_prefix("/__ruvyxa/client/")?;
    Some(client_dir.join(rest))
}

/// Every pre-rendered document, checked for asset URLs that name no file.
fn audit_documents(
    prerender_dir: &Path,
    client_dir: &Path,
    assets_dir: &Path,
    dangling: &mut BTreeSet<DanglingReference>,
) {
    for file in emitted_files(prerender_dir, "html") {
        let Ok(html) = fs::read_to_string(&file) else {
            continue;
        };
        for url in document_asset_urls(&html) {
            let target = if let Some(rest) = url.strip_prefix("/__ruvyxa/client/") {
                client_dir.join(rest)
            } else {
                // A `public/` file, published into the assets directory.
                assets_dir.join(url.trim_start_matches('/'))
            };
            if !target.is_file() {
                dangling.insert(DanglingReference {
                    from: display_name(prerender_dir, &file),
                    reference: url,
                });
            }
        }
    }
}

/// Same-origin `src`/`href` values a document expects the build to have written.
///
/// Only absolute paths are considered: a `data:` URI carries its own bytes, an
/// external origin is not this build's to publish, and a relative URL in a
/// document is resolved against a request path this audit does not know.
fn document_asset_urls(html: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for attribute in ["src=\"", "href=\""] {
        let mut rest = html;
        while let Some(at) = rest.find(attribute) {
            rest = &rest[at + attribute.len()..];
            let Some(end) = rest.find('"') else { break };
            let url = &rest[..end];
            rest = &rest[end..];
            if !url.starts_with('/') || url.starts_with("//") {
                continue;
            }
            // A route URL is a page, not a file; only the two directories a
            // build publishes are checked.
            if url.starts_with("/__ruvyxa/client/") || looks_like_a_file(url) {
                urls.push(url.split(['?', '#']).next().unwrap_or(url).to_string());
            }
        }
    }
    urls
}

/// Whether a same-origin URL names a file rather than a route.
fn looks_like_a_file(url: &str) -> bool {
    let last = url.rsplit('/').next().unwrap_or_default();
    last.contains('.') && !last.ends_with('.')
}

/// Every file under `directory` with the given extension.
fn emitted_files(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// `path` relative to `root`, with forward slashes, for a message a reader can
/// paste into a terminal.
fn display_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Apply `.` and `..` lexically; the target need not exist yet.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A specifier the linker left behind is a 404 in the browser, and the page
    /// it breaks looks fine: it is already server-rendered, and only hydration
    /// is lost. The build has the files in hand and can say so instead.
    #[test]
    fn an_unresolved_specifier_in_an_emitted_chunk_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        let prerender = temp.path().join("prerender");
        let assets = temp.path().join("assets");
        fs::create_dir_all(&client).unwrap();
        fs::create_dir_all(&prerender).unwrap();
        fs::create_dir_all(&assets).unwrap();

        fs::write(client.join("shared.js"), "export const value = 1\n").unwrap();
        fs::write(
            client.join("route.js"),
            "import { value } from './shared.js'\nimport { cn } from '../lib/cn'\nexport default value + cn\n",
        )
        .unwrap();

        let dangling = audit_emitted_output(&client, &prerender, &assets);
        assert_eq!(
            dangling,
            vec![DanglingReference {
                from: "route.js".to_string(),
                reference: "../lib/cn".to_string(),
            }],
            "the resolvable sibling import must not be reported"
        );
        assert!(dangling_reference_diagnostic(&dangling).is_some());
    }

    /// A document that references a stylesheet no directory holds renders
    /// unstyled, which is the most visible failure a deployment can have.
    #[test]
    fn a_document_referencing_a_missing_asset_is_reported() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        let prerender = temp.path().join("prerender");
        let assets = temp.path().join("assets");
        fs::create_dir_all(client.join("nested")).unwrap();
        fs::create_dir_all(prerender.join("about")).unwrap();
        fs::create_dir_all(&assets).unwrap();

        fs::write(client.join("nested/page.js"), "export default 1\n").unwrap();
        fs::write(assets.join("logo.png"), "png").unwrap();
        fs::write(
            prerender.join("about/index.html"),
            r#"<!doctype html><html><head>
               <link rel="stylesheet" href="/__ruvyxa/client/styles.abc.css">
               <link rel="icon" href="/logo.png">
               <a href="/blog/post-one">a route, not a file</a>
               <img src="https://cdn.example/x.png">
               </head><body><script type="module" src="/__ruvyxa/client/nested/page.js"></script></body></html>"#,
        )
        .unwrap();

        let dangling = audit_emitted_output(&client, &prerender, &assets);
        assert_eq!(
            dangling,
            vec![DanglingReference {
                from: "about/index.html".to_string(),
                reference: "/__ruvyxa/client/styles.abc.css".to_string(),
            }],
            "only the missing stylesheet is dangling: {dangling:?}"
        );
    }

    /// An ordinary build has nothing to report, and the audit must not invent
    /// work for a bare specifier or an external URL.
    #[test]
    fn a_clean_build_reports_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        let prerender = temp.path().join("prerender");
        let assets = temp.path().join("assets");
        fs::create_dir_all(&client).unwrap();
        fs::create_dir_all(&prerender).unwrap();
        fs::create_dir_all(&assets).unwrap();

        fs::write(client.join("shared.js"), "export const value = 1\n").unwrap();
        fs::write(
            client.join("route.js"),
            "import 'react'\nimport { value } from './shared.js'\nimport('https://cdn.example/m.js')\nexport default value\n",
        )
        .unwrap();
        fs::write(
            prerender.join("index.html"),
            r#"<!doctype html><html><body><script type="module" src="/__ruvyxa/client/route.js"></script></body></html>"#,
        )
        .unwrap();

        assert_eq!(
            audit_emitted_output(&client, &prerender, &assets),
            Vec::new()
        );
    }
}
