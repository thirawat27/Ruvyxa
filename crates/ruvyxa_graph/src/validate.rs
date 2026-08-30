use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ruvyxa_diagnostics::{Diagnostic, Result, normalized_canonical_path};
use serde::Serialize;

use crate::discovery::resolve_layout_file;
use crate::graph::{ModuleCache, collect_relative_graph, private_env_reads};
use crate::manifest::{RenderStrategy, RouteKind, RouteManifest, RuntimeTarget};
use crate::render::edge_forbidden_builtin;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub routes: usize,
    pub page_routes: usize,
    pub api_routes: usize,
    pub client_modules: usize,
    pub server_modules: usize,
    /// Server-components routes that ship a browser bundle with nothing in it
    /// to hydrate.
    ///
    /// Not a diagnostic: the page is correct and the cost is a few hundred
    /// kilobytes of React runtime the route never uses. Surfaced so it can be
    /// seen at all — an unused bundle is invisible in every other report — and
    /// left as the project's decision, because `export const hydrate = false`
    /// also drops the route from the client router and turns navigation to it
    /// into a full page load.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inert_hydration_routes: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Routes that would render one of `modules` on the server *and* hydrate it in
/// the browser, paired with the module they reach.
///
/// The question a plugin transform raises. `build.onTransform` is applied by
/// the browser compile and by nothing else: the server render reads the same
/// file through `runtime/compiler.mjs`, which runs no plugin hooks. For a route
/// that only runs in the browser that is harmless, and for a route that ships
/// no client bundle it never comes up — but a route that does both renders the
/// original text into the document and then hydrates against the rewritten one.
/// React discards the server markup and re-renders (#418), which looks like a
/// flicker rather than like a build problem.
///
/// Answers the pairs so a caller can name both halves; empty when nothing is at
/// risk, which is the common case and costs one graph walk.
pub fn hydrated_routes_reaching(
    manifest: &RouteManifest,
    modules: &BTreeSet<PathBuf>,
) -> Vec<(String, PathBuf)> {
    if modules.is_empty() {
        return Vec::new();
    }
    let mut cache = ModuleCache::in_root(&manifest.app_dir);
    let mut found = Vec::new();
    for route in &manifest.routes {
        if route.kind != RouteKind::Page
            || route.render.strategy == RenderStrategy::Csr
            || !route.render.ships_client_bundle()
        {
            continue;
        }
        let mut entries = vec![route.file.clone()];
        for layout in &route.layout_chain {
            entries.extend(resolve_layout_file(&manifest.app_dir, layout));
        }
        for template in &route.template_chain {
            entries.extend(resolve_layout_file(&manifest.app_dir, template));
        }
        entries.extend(route.client_modules.iter().map(PathBuf::from));

        for entry in entries {
            for module in collect_relative_graph(&entry, &mut cache) {
                if modules.contains(&module) {
                    found.push((route.path.clone(), module));
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Every project module the routes reach that does **not** live in `app/`.
///
/// `ruvyxa build` stages the application into `<out>/server/` and `ruvyxa start`
/// compiles pages from that copy, so a module the copy does not contain cannot
/// be resolved at request time. Only `app/` and two hard-coded directory names
/// were staged, and the ordinary layout — `app/` beside `lib/` — therefore
/// answered a request-time render with
/// `RUV1801 cannot resolve '../../lib/x'`, naming a path under `.ruvyxa` that
/// the author never wrote. A page importing the same module through a tsconfig
/// alias worked, because that path resolves from the project root.
///
/// Returned as absolute, normalized paths. Anything under `node_modules` is
/// left out: a deployed function bundles what it needs, and `start` resolves
/// packages from the project's own tree.
pub fn reachable_project_modules(root: &Path, manifest: &RouteManifest) -> BTreeSet<PathBuf> {
    let canonical_root = normalized_canonical_path(root);
    let canonical_app = normalized_canonical_path(&manifest.app_dir);
    let mut cache = ModuleCache::in_root(root);
    let mut modules = BTreeSet::new();

    for route in &manifest.routes {
        let mut entries = vec![route.file.clone()];
        for layout in &route.layout_chain {
            entries.extend(resolve_layout_file(&manifest.app_dir, layout));
        }
        for template in &route.template_chain {
            entries.extend(resolve_layout_file(&manifest.app_dir, template));
        }
        entries.extend(route.server_modules.iter().map(PathBuf::from));
        entries.extend(route.client_modules.iter().map(PathBuf::from));

        for entry in entries {
            for module in collect_relative_graph(&entry, &mut cache) {
                if module.starts_with(&canonical_app) {
                    continue;
                }
                let Ok(relative) = module.strip_prefix(&canonical_root) else {
                    continue;
                };
                if relative
                    .components()
                    .any(|component| component.as_os_str() == "node_modules")
                {
                    continue;
                }
                modules.insert(module);
            }
        }
    }
    modules
}

pub fn validate_app(root: &Path, manifest: &RouteManifest) -> Result<ValidationReport> {
    let mut diagnostics = Vec::new();
    let mut client_modules = BTreeSet::new();
    let mut server_modules = BTreeSet::new();
    let mut inert_hydration_routes = Vec::new();

    // Pre-canonicalize root once instead of per-module (avoids repeated syscalls).
    // Use the verbatim-prefix-free helper so `strip_prefix` against module
    // paths (also normalized) actually matches on Windows.
    let canonical_root = normalized_canonical_path(root);

    // Track which modules have already been validated to avoid duplicate reads.
    let mut validated_client: BTreeSet<PathBuf> = BTreeSet::new();
    let mut validated_server: BTreeSet<PathBuf> = BTreeSet::new();
    // Shared across every route so a layout or component reached from many
    // routes is read and scanned once, not once per route.
    let mut cache = ModuleCache::in_root(root);

    for route in &manifest.routes {
        match route.kind {
            RouteKind::Page => {
                let page = cache.require(&route.file)?;
                let is_content_page = is_markdown_route(&route.file);
                if !is_content_page && !page.ast.has_default_export {
                    diagnostics.push(
                        Diagnostic::new("RUV1004", "Page is missing a default export")
                            .explain(
                                "Every TypeScript/JavaScript page must export a default component. Markdown and MDX pages receive one from the content compiler.",
                            )
                            .at_file(&route.file)
                            .suggest("Add `export default function Page() { return <main /> }`."),
                    );
                }

                let mut graph = collect_relative_graph(&route.file, &mut cache);
                for layout in &route.layout_chain {
                    if let Some(layout) = resolve_layout_file(&manifest.app_dir, layout) {
                        graph.extend(collect_relative_graph(&layout, &mut cache));
                    }
                }

                // Which side of the boundary this route's graph is on.
                //
                // An ordinary page hydrates, so every module it reaches is a
                // client module. A server-components route does not: the client
                // compile stops at `'use client'`, the page itself is
                // serialised into a payload, and nothing above that boundary is
                // in any browser bundle. Validating its whole graph as client
                // code refused exactly the things a server component is for —
                // `import 'server-only'` (RUV1007) and a private
                // `process.env` read (RUV1008) — in the one place they are
                // correct.
                let mut reaches_client_code = false;
                for module in graph {
                    let client_lane =
                        !route.render.server_components || is_client_boundary(&module, &mut cache);
                    reaches_client_code |= client_lane;
                    if !client_lane {
                        server_modules.insert(module.clone());
                        if validated_server.insert(module.clone()) {
                            validate_server_module(&module, &mut cache, &mut diagnostics)?;
                        }
                        continue;
                    }

                    // A `'use client'` module owns its whole dependency
                    // closure: everything it reaches is in the browser bundle
                    // with it.
                    let reachable = if route.render.server_components {
                        collect_relative_graph(&module, &mut cache)
                    } else {
                        BTreeSet::from([module.clone()])
                    };
                    for module in reachable {
                        client_modules.insert(module.clone());
                        // Skip if already validated — the cache makes the
                        // re-read free, but the diagnostics would be emitted
                        // twice.
                        if validated_client.insert(module.clone()) {
                            validate_client_module(
                                &canonical_root,
                                &module,
                                &mut cache,
                                &mut diagnostics,
                            )?;
                        }
                    }
                }

                // A server-components route whose graph never crosses a
                // `'use client'` boundary has nothing for a browser bundle to
                // hydrate: the page is serialised into a payload, and every
                // module above the boundary stays on the server. It still ships
                // the shared React runtime — a few hundred kilobytes on a page
                // that does not use a byte of it.
                //
                // Reported rather than fixed here, because the fix is not free:
                // `export const hydrate = false` also removes the route from
                // the client router's registry, so navigating to it becomes a
                // full page load instead of a soft one. Which of the two costs
                // matters is the project's call, and it cannot be read from the
                // source.
                if route.render.server_components
                    && route.render.ships_client_bundle()
                    && !reaches_client_code
                    && route.client_modules.is_empty()
                {
                    inert_hydration_routes.push(route.path.clone());
                }
            }
            RouteKind::Api => {
                let graph = collect_relative_graph(&route.file, &mut cache);
                for module in graph {
                    server_modules.insert(module.clone());
                    if validated_server.insert(module.clone()) {
                        validate_server_module(&module, &mut cache, &mut diagnostics)?;
                    }
                }
            }
        }

        // A route that declared the edge runtime promises an API surface, and
        // this is where the promise is checked. Without it the declaration is a
        // label: the build succeeds, the adapter places the route on a Worker,
        // and the first request fails there with a module-not-found for
        // something that was never going to exist — on the one host where the
        // developer cannot attach a debugger.
        if route.runtime == RuntimeTarget::Edge {
            let mut graph = collect_relative_graph(&route.file, &mut cache);
            for layout in &route.layout_chain {
                if let Some(layout) = resolve_layout_file(&manifest.app_dir, layout) {
                    graph.extend(collect_relative_graph(&layout, &mut cache));
                }
            }
            for module in graph {
                let Some(scanned) = cache.module(&module) else {
                    continue;
                };
                for specifier in scanned.ast.import_specifiers() {
                    let Some(builtin) = edge_forbidden_builtin(&specifier) else {
                        continue;
                    };
                    let mut diagnostic =
                        Diagnostic::new("RUV1013", "Edge route reaches a Node built-in")
                            .explain(format!(
                                "{} imports `{specifier}`, and `{builtin}` does not exist in a Web-standards runtime. This route declares `export const runtime = 'edge'`.",
                                module.display()
                            ))
                            .at_file(&module)
                            .suggest(
                                "Replace the import with a Web API — `fetch`, `crypto.subtle`, `URL` — or drop `export const runtime = 'edge'` so the route runs on Node.",
                            );
                    diagnostic.affected_routes = vec![route.id.clone()];
                    diagnostics.push(diagnostic);
                }
            }
        }

        for module in &route.server_modules {
            let module = PathBuf::from(module);
            let graph = collect_relative_graph(&module, &mut cache);
            for module in graph {
                server_modules.insert(module.clone());
                if validated_server.insert(module.clone()) {
                    validate_server_module(&module, &mut cache, &mut diagnostics)?;
                }
            }
        }

        for module in &route.client_modules {
            let module = PathBuf::from(module);
            client_modules.insert(module.clone());
            if validated_client.insert(module.clone()) {
                validate_client_module(&canonical_root, &module, &mut cache, &mut diagnostics)?;
            }
        }
    }

    Ok(ValidationReport {
        routes: manifest.routes.len(),
        page_routes: manifest
            .routes
            .iter()
            .filter(|route| route.kind == RouteKind::Page)
            .count(),
        api_routes: manifest
            .routes
            .iter()
            .filter(|route| route.kind == RouteKind::Api)
            .count(),
        client_modules: client_modules.len(),
        server_modules: server_modules.len(),
        inert_hydration_routes,
        diagnostics,
    })
}

pub(crate) fn validate_client_module(
    canonical_root: &Path,
    file: &Path,
    cache: &mut ModuleCache,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let Some(module) = cache.module(file) else {
        return Ok(());
    };

    if module
        .ast
        .imports
        .iter()
        .any(|edge| is_server_only_specifier(&edge.specifier))
    {
        diagnostics.push(
            Diagnostic::new("RUV1007", "Server-only module imported into client graph")
                .explain("This module is reachable from a hydrated page or client module but declares `server-only`.")
                .at_file(file)
                .suggest("Move server-only work behind a route handler/server module and pass serializable data to the client."),
        );
    }

    // One diagnostic per *name*, not per read, and the same guard in the same
    // shape as `crates/ruvyxa_bundler/src/boundary.rs` — the two are the same
    // rule reached by two hosts, and `ruvyxa check`/`ruvyxa dev` disagreeing
    // with `ruvyxa build` about how many problems one file has is what a second
    // implementation of it costs.
    //
    // The deduplication belongs here and not in `private_env_reads`, which
    // reports occurrences in source order and unfiltered because that is the
    // extraction contract `tests/fixtures/env-policy-conformance.json` holds
    // level with `privateEnvReads` in `packages/ruvyxa/runtime/compiler.mjs`.
    // First-seen order is kept rather than sorted, so the list still reads down
    // the file.
    let mut already_reported = BTreeSet::new();
    for env_name in private_env_reads(&module.ast) {
        if !already_reported.insert(env_name) {
            continue;
        }
        diagnostics.push(
            Diagnostic::new("RUV1008", "Private environment variable used in client graph")
                .explain(format!(
                    "`process.env.{env_name}` is reachable from browser code. Only `RUVYXA_PUBLIC_*` env vars may be exposed to client modules."
                ))
                .at_file(file)
                .suggest("Move the env read into server-only code or rename it to `RUVYXA_PUBLIC_*` if it is safe to expose."),
        );
    }

    // Check if file is under the project-level server/ directory.
    // Try strip_prefix first (cheap), only canonicalize the file if needed.
    let is_server_dir = if let Ok(relative) = file.strip_prefix(canonical_root) {
        relative_starts_with_server(relative)
    } else {
        // Paths don't share a prefix — normalize the file the same way as the
        // root before giving up, so Windows verbatim prefixes can't silently
        // skip the server/ boundary check.
        let canonical_file = normalized_canonical_path(file);
        if let Ok(relative) = canonical_file.strip_prefix(canonical_root) {
            relative_starts_with_server(relative)
        } else {
            false
        }
    };

    if is_server_dir {
        diagnostics.push(
            Diagnostic::new("RUV1010", "Server directory module reached by client graph")
                .explain("Files under server/ are reserved for server-only code.")
                .at_file(file)
                .suggest("Move shared browser-safe code outside server/, or import it from a server route only."),
        );
    }

    Ok(())
}

pub(crate) fn is_server_only_specifier(specifier: &str) -> bool {
    matches!(
        specifier,
        "server-only" | "@ruvyxa/auth" | "@ruvyxa/database"
    )
}

/// Whether a route file is a Markdown/MDX content route.
pub(crate) fn is_markdown_route(file: &Path) -> bool {
    matches!(
        file.extension().and_then(|extension| extension.to_str()),
        Some("md" | "mdx")
    )
}

/// Blank out fenced code blocks and inline code spans in Markdown/MDX source.
///
/// Fenced examples are display text, not executable code: a guide that shows
/// `process.env.SECRET` or `import 'server-only'` inside a code block must not
/// trip the boundary validators or flip the route's rendering strategy. MDX
/// ESM (`import`/`export`) lives outside fences and is preserved. Blanked
/// regions keep their newlines so diagnostics retain meaningful positions.
pub(crate) fn markdown_without_code_examples(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut fence: Option<(char, usize)> = None;

    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let fence_marker = ['`', '~'].into_iter().find_map(|marker| {
            let length = trimmed.chars().take_while(|&c| c == marker).count();
            (length >= 3).then_some((marker, length))
        });

        match (&fence, fence_marker) {
            // Opening fence line.
            (None, Some(marker)) => {
                fence = Some(marker);
                output.push('\n');
            }
            // Closing fence: same character, at least the opening length.
            (Some((open_char, open_len)), Some((close_char, close_len)))
                if close_char == *open_char && close_len >= *open_len =>
            {
                fence = None;
                output.push('\n');
            }
            // Inside a fence: keep only the newline.
            (Some(_), _) => output.push('\n'),
            // Regular markdown line: blank inline code spans.
            (None, None) => {
                let mut in_span = false;
                for character in line.chars() {
                    if character == '`' {
                        in_span = !in_span;
                        output.push(' ');
                    } else if in_span && character != '\n' {
                        output.push(' ');
                    } else {
                        output.push(character);
                    }
                }
            }
        }
    }

    output
}

/// Whether a module declares itself the start of the browser's half of a
/// server-components route.
///
/// The same question the compilers ask — `'use client'` is a directive, and the
/// scanner that reads it is the bundler's, not a second text search here.
pub(crate) fn is_client_boundary(file: &Path, cache: &mut ModuleCache) -> bool {
    cache.module(file).is_some_and(|module| {
        ruvyxa_bundler::reference_manifest::has_module_directive(&module.source, "use client")
    })
}

pub(crate) fn validate_server_module(
    file: &Path,
    cache: &mut ModuleCache,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let Some(module) = cache.module(file) else {
        return Ok(());
    };

    if module
        .ast
        .imports
        .iter()
        .any(|edge| edge.specifier == "client-only")
    {
        diagnostics.push(
            Diagnostic::new("RUV1009", "Client-only module imported into server graph")
                .explain(
                    "This module is reachable from server runtime code but declares `client-only`.",
                )
                .at_file(file)
                .suggest("Move browser-only code into a client component or client.tsx module."),
        );
    }

    Ok(())
}

pub(crate) fn relative_starts_with_server(relative: &Path) -> bool {
    relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == "server")
}
