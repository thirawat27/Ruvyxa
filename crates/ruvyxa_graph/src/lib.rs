use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError, normalized_canonical_path};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

/// Route parameters passed from the matcher to page and API renderers.
///
/// Values are JSON-shaped because catch-all segments are arrays while an
/// omitted optional catch-all has no entry.
pub type RouteParams = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteManifest {
    pub app_dir: PathBuf,
    pub routes: Vec<RouteEntry>,
    /// Optional file-system locale routing policy copied from project config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i18n: Option<I18nRouting>,
}

/// Validated locale-routing policy shared by discovery, native serving, and
/// deployment runtimes. Validation belongs to the config boundary; consumers
/// can therefore use these values without interpreting raw user input again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct I18nRouting {
    pub locales: Vec<String>,
    pub default_locale: String,
    pub locale_param: String,
    pub detect_locale: bool,
    pub cookie: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteEntry {
    pub id: String,
    pub path: String,
    pub kind: RouteKind,
    pub file: PathBuf,
    pub layout_chain: Vec<String>,
    /// `template.tsx` files on the path to this route, root first.
    ///
    /// Separate from `layout_chain` rather than merged into it because a level
    /// may have either, both, or neither, and composition interleaves them by
    /// directory.
    #[serde(default)]
    pub template_chain: Vec<String>,
    /// Parallel-route slots this route composes into its layouts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<RouteSlot>,
    /// Interceptions reachable from this route, by the URL each one covers.
    ///
    /// Carried on the route the user is *standing on* rather than on the route
    /// being intercepted, because that is the bundle that has to be able to
    /// render the overlay without a round trip. The intercepted route keeps its
    /// own entry untouched, which is what makes a hard load show the real page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intercepts: Vec<RouteIntercept>,
    pub server_modules: Vec<String>,
    pub client_modules: Vec<String>,
    pub runtime: RuntimeTarget,
    /// Rendering strategy and metadata for this route.
    #[serde(default)]
    pub render: RenderMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteKind {
    Page,
    Api,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeTarget {
    Node,
    Edge,
    Static,
}

/// Per-route rendering strategy — determines when and how the HTML is generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RenderStrategy {
    /// Server-Side Rendering: HTML generated on every request (default).
    #[default]
    Ssr,
    /// Static Site Generation: HTML pre-rendered at build time.
    Ssg,
    /// Incremental Static Regeneration: pre-rendered at build time, revalidated
    /// in the background after a TTL expires.
    Isr,
    /// Client-Side Rendering: minimal shell HTML served, full rendering happens
    /// in the browser via hydration without server-rendered content.
    Csr,
    /// Partial Pre-Rendering: static shell pre-rendered at build time with
    /// dynamic "holes" that stream in at request time.
    Ppr,
}

/// When a server-rendered route downloads and starts its client runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HydrationMode {
    /// Load and hydrate as soon as the document parser reaches the module.
    #[default]
    Load,
    /// Download the route bundle when the browser is idle.
    Idle,
    /// Download the route bundle when the document becomes visible.
    Visible,
    /// Ship no client bundle for this route.
    None,
}

/// Metadata that controls the rendering strategy for a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderMeta {
    /// The rendering strategy for this route.
    pub strategy: RenderStrategy,
    /// ISR revalidation interval in seconds (only meaningful for `Isr`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revalidate: Option<u64>,
    /// Whether the page exports `getStaticParams` or `staticParams` for dynamic SSG routes.
    #[serde(default)]
    pub has_static_params: bool,
    /// Static paths discovered from `getStaticParams` at build time.
    /// Empty until the build phase populates them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub static_paths: Vec<String>,
    /// For PPR: whether the page uses `<Suspense>` boundaries that mark
    /// dynamic slots to be streamed at request time.
    #[serde(default)]
    pub has_dynamic_slots: bool,
    /// When the client bundle is scheduled, and whether there is one at all.
    ///
    /// This is the single source of truth for client-side scheduling.
    /// `export const hydrate = false` (or `'none'`) parses to
    /// [`HydrationMode::None`], which is what
    /// [`RenderMeta::ships_client_bundle`] answers from — a page that ships no
    /// bundle runs no interactivity, so `'use client'` islands do not execute
    /// there. A separate `hydrate: bool` field used to be stored beside this
    /// one and was only ever `hydration != None`; because both were public and
    /// independently assignable, callers could and did set one without the
    /// other, leaving the bundler and the document writer disagreeing about
    /// whether a route had JavaScript.
    #[serde(default)]
    pub hydration: HydrationMode,
    /// `export const serverComponents = true`: render this route through the
    /// React Server Components pipeline.
    ///
    /// Orthogonal to [`Self::strategy`] rather than a variant of it, because it
    /// answers a different question. The strategy decides *when* a route is
    /// rendered — at build time, per request, or on a revalidation interval —
    /// while this decides *which two graphs* render it. A server-components
    /// route can still be SSG or ISR, and folding the two would have made
    /// `revalidate` and this mutually exclusive for no reason.
    ///
    /// Opt-in per route rather than a project-wide default: turning it on
    /// changes what reaches the browser for that route, and a framework-wide
    /// switch would change every existing page at once.
    #[serde(default)]
    pub server_components: bool,
    /// `export const dynamic = 'force-dynamic'`: this route asked to be
    /// rendered per request.
    ///
    /// Distinct from an ordinary SSR strategy, which is only the *default*.
    /// Reading the export used to decide one thing — do not pre-render this —
    /// and nothing downstream could tell the two apart afterwards, so the
    /// runtime render cache stored the document and served it unchanged for the
    /// life of the process. The page asked for the opposite of that, and Next,
    /// whose convention this is, also takes it to mean "do not cache".
    #[serde(default)]
    pub force_dynamic: bool,
}

impl RenderMeta {
    /// Whether the served HTML includes a client bundle.
    pub fn ships_client_bundle(&self) -> bool {
        self.hydration != HydrationMode::None
    }
}

impl Default for RenderMeta {
    fn default() -> Self {
        Self {
            strategy: RenderStrategy::default(),
            revalidate: None,
            has_static_params: false,
            static_paths: Vec::new(),
            has_dynamic_slots: false,
            hydration: HydrationMode::Load,
            server_components: false,
            force_dynamic: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverOptions {
    pub app_dir: PathBuf,
    pub default_render_strategy: Option<RenderStrategy>,
    pub default_revalidate: Option<u64>,
    pub i18n: Option<I18nRouting>,
}

impl DiscoverOptions {
    pub fn new(app_dir: impl Into<PathBuf>) -> Self {
        Self {
            app_dir: app_dir.into(),
            default_render_strategy: None,
            default_revalidate: None,
            i18n: None,
        }
    }

    pub fn with_rendering_defaults(
        mut self,
        default_render_strategy: Option<RenderStrategy>,
        default_revalidate: Option<u64>,
    ) -> Self {
        self.default_render_strategy = default_render_strategy;
        self.default_revalidate = default_revalidate;
        self
    }

    pub fn with_i18n(mut self, i18n: Option<I18nRouting>) -> Self {
        self.i18n = i18n;
        self
    }
}

pub fn discover_routes(options: DiscoverOptions) -> Result<RouteManifest> {
    let DiscoverOptions {
        app_dir,
        default_render_strategy,
        default_revalidate,
        i18n,
    } = options;

    if !app_dir.exists() {
        return Err(Diagnostic::new("RUV1001", "App directory was not found")
            .explain("Ruvyxa expects an app directory with page.tsx, page.md, page.mdx, or route.ts files.")
            .at_file(&app_dir)
            .suggest("Create app/page.tsx, app/page.md, or app/page.mdx; or set appDir in ruvyxa.config.ts.")
            .into());
    }

    reject_intercepting_routes(&app_dir)?;

    let mut routes = Vec::new();
    // Shared across every route: layouts and shared components are reachable
    // from many pages, and rendering-strategy detection walks that graph.
    let mut cache = ModuleCache::in_root(app_dir.parent().unwrap_or(&app_dir));

    for entry in WalkDir::new(&app_dir)
        .into_iter()
        .filter_entry(|entry| {
            if !entry.file_type().is_dir() || entry.path() == app_dir {
                return true;
            }

            let name = entry.file_name().to_string_lossy();
            !name.starts_with('_') && !name.starts_with('@')
        })
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy();
        let kind = match file_name.as_ref() {
            "page.tsx" | "page.jsx" | "page.md" | "page.mdx" => RouteKind::Page,
            "route.ts" | "route.js" => RouteKind::Api,
            _ => continue,
        };

        let file = entry.path().to_path_buf();
        let route_dir = file.parent().unwrap_or(&app_dir);
        let relative_dir = route_dir.strip_prefix(&app_dir).unwrap_or(route_dir);
        let path = route_path_from_dir(relative_dir)?;
        let id = route_id(&app_dir, &file);
        let layout_chain = layout_chain(&app_dir, route_dir);
        let template_chain = template_chain(&app_dir, route_dir);
        let slots = route_slots(&app_dir, route_dir);
        let intercepts = route_intercepts(&app_dir, route_dir)?;

        routes.push(RouteEntry {
            id,
            path: path.clone(),
            kind,
            file: file.clone(),
            layout_chain: layout_chain.clone(),
            template_chain,
            slots,
            intercepts,
            server_modules: sibling_modules(
                route_dir,
                &["server.ts", "server.js", "action.ts", "action.js"],
            ),
            client_modules: sibling_module(route_dir, "client.tsx"),
            runtime: detect_runtime_target(&file, &mut cache)?,
            render: if kind == RouteKind::Page {
                apply_rendering_defaults(
                    detect_render_strategy(&app_dir, &file, &path, &layout_chain, &mut cache),
                    default_render_strategy,
                    default_revalidate,
                )
            } else {
                RenderMeta::default()
            },
        });
    }

    routes.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.id.cmp(&right.id))
    });
    detect_conflicts(&routes)?;
    detect_unreachable_intercepts(&routes)?;
    detect_server_component_conflicts(&routes)?;

    Ok(RouteManifest {
        app_dir,
        routes,
        i18n,
    })
}

pub fn write_manifest(manifest: &RouteManifest, output_file: &Path) -> Result<()> {
    if let Some(parent) = output_file.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(manifest)
        .map_err(|error| RuvyxaError::Message(error.to_string()))?;
    fs::write(output_file, json)?;
    Ok(())
}

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

fn validate_client_module(
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

    for env_name in private_env_reads(&module.ast) {
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

fn is_server_only_specifier(specifier: &str) -> bool {
    matches!(
        specifier,
        "server-only" | "@ruvyxa/auth" | "@ruvyxa/database"
    )
}

/// Whether a route file is a Markdown/MDX content route.
fn is_markdown_route(file: &Path) -> bool {
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
fn markdown_without_code_examples(source: &str) -> String {
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
fn is_client_boundary(file: &Path, cache: &mut ModuleCache) -> bool {
    cache.module(file).is_some_and(|module| {
        ruvyxa_bundler::reference_manifest::has_module_directive(&module.source, "use client")
    })
}

fn validate_server_module(
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

/// One module's source and the facts derived from a single scan of it.
///
/// `ast.rs` states the contract this upholds: "callers that also need imports
/// should call `parse_module` once and read both facts off the result." Route
/// validation needs three facts per module — imports, env reads, and whether a
/// default export exists — and used to reach each through its own
/// `source -> T` helper that called `parse_module` internally.
struct ParsedModule {
    /// Source as the validators see it: Markdown/MDX already has its fenced
    /// examples blanked out.
    source: Arc<str>,
    ast: ruvyxa_bundler::ast::ModuleAst,
}

/// Per-run cache of everything derived from a module's source text.
///
/// Reading and scanning a file is the expensive part of route discovery and
/// validation, and both walk overlapping graphs: a layout, and every component
/// it pulls in, is reachable from every route beneath it. Keying that work by
/// canonical path collapses it to once per file per run instead of growing as
/// `routes × shared modules`.
///
/// Reading through one place also makes the Markdown decision unskippable.
/// Masking used to be applied by each caller, and the edge walk was the one
/// that forgot: an `import './helpers'` shown inside a fenced example in a
/// `.md` page became a real graph edge, pulling that module into the client
/// graph and raising boundary diagnostics against code the page never runs.
/// Masking now happens at the single point where source is read, so no caller
/// can skip it.
#[derive(Default)]
struct ModuleCache {
    project_root: Option<PathBuf>,
    /// Memoized `normalized_canonical_path`, so the same route file reached
    /// through a walk path and through a resolved import is one entry, and the
    /// canonicalize syscall runs once per distinct spelling.
    canonical: BTreeMap<PathBuf, PathBuf>,
    modules: BTreeMap<PathBuf, Option<Arc<ParsedModule>>>,
    /// Masked code, built lazily: only rendering-strategy detection needs it,
    /// and it is a full second pass over the source.
    masked: BTreeMap<PathBuf, Arc<str>>,
    edges: BTreeMap<PathBuf, Arc<[PathBuf]>>,
    /// `tsconfig.json` path aliases, read once per run on first use.
    aliases: Option<Arc<ruvyxa_bundler::resolver::TsConfigPaths>>,
}

impl ModuleCache {
    fn in_root(root: &Path) -> Self {
        Self {
            project_root: Some(normalized_canonical_path(root)),
            ..Self::default()
        }
    }

    fn canonical(&mut self, file: &Path) -> PathBuf {
        if let Some(canonical) = self.canonical.get(file) {
            return canonical.clone();
        }
        let canonical = normalized_canonical_path(file);
        self.canonical.insert(file.to_path_buf(), canonical.clone());
        canonical
    }

    /// Source and parsed facts for `file`, or `None` when it cannot be read.
    ///
    /// An unreadable file caches the `None`, matching the previous behavior of
    /// skipping it, and stops the retry on every later walk.
    fn module(&mut self, file: &Path) -> Option<Arc<ParsedModule>> {
        let key = self.canonical(file);
        if let Some(cached) = self.modules.get(&key) {
            return cached.clone();
        }

        let parsed = fs::read_to_string(&key).ok().map(|source| {
            let source = if is_markdown_route(&key) {
                markdown_without_code_examples(&source)
            } else {
                source
            };
            Arc::new(ParsedModule {
                ast: ruvyxa_bundler::ast::parse_module(&source),
                source: Arc::from(source),
            })
        });
        self.modules.insert(key, parsed.clone());
        parsed
    }

    /// Like [`ModuleCache::module`], but reports why the read failed.
    ///
    /// A file the manifest lists as a route must exist; treating it as an
    /// empty module would silently drop its diagnostics.
    fn require(&mut self, file: &Path) -> Result<Arc<ParsedModule>> {
        if let Some(module) = self.module(file) {
            return Ok(module);
        }
        // Only reached on the error path, so re-reading to recover the real
        // `io::Error` costs nothing in the common case.
        Err(fs::read_to_string(file).unwrap_err().into())
    }

    /// Source with strings, template text, comments, and regex literals blanked.
    fn masked(&mut self, file: &Path) -> Option<Arc<str>> {
        let key = self.canonical(file);
        if let Some(cached) = self.masked.get(&key) {
            return Some(cached.clone());
        }

        let module = self.module(&key)?;
        let masked: Arc<str> = Arc::from(code_without_strings_and_comments(&module.source));
        self.masked.insert(key, masked.clone());
        Some(masked)
    }

    /// The project's `tsconfig.json` path aliases.
    ///
    /// The bundler's table, not a third one: this walk decides which modules a
    /// route can reach, and a module it cannot see is a module whose data
    /// fetching it reports as absent.
    fn aliases(&mut self) -> Option<Arc<ruvyxa_bundler::resolver::TsConfigPaths>> {
        if let Some(aliases) = &self.aliases {
            return Some(Arc::clone(aliases));
        }
        let root = self.project_root.clone()?;
        let loaded = Arc::new(ruvyxa_bundler::resolver::TsConfigPaths::load(&root));
        self.aliases = Some(Arc::clone(&loaded));
        Some(loaded)
    }

    /// Resolve a non-relative specifier through the project's path aliases.
    ///
    /// Returns `None` for a bare package specifier, which stays outside this
    /// walk — see [`collect_relative_graph`].
    fn aliased_import(&mut self, specifier: &str) -> Option<PathBuf> {
        let resolved = self.aliases()?.resolve(specifier)?;
        Some(self.canonical(&resolved))
    }

    /// Project imports declared by `file`, resolved to paths.
    ///
    /// Relative and aliased specifiers both land here. Only relative ones used
    /// to: `import { load } from '@/lib/data'` produced no edge at all, so a
    /// page whose data fetching lived one alias away looked to
    /// [`detect_render_strategy`] like a page that fetched nothing, and was
    /// pre-rendered at build time. The same import written `../../lib/data`
    /// stayed SSR. Which spelling a project uses is not a rendering decision.
    fn edges(&mut self, file: &Path) -> Arc<[PathBuf]> {
        let key = self.canonical(file);
        if let Some(cached) = self.edges.get(&key) {
            return cached.clone();
        }

        let resolved: Arc<[PathBuf]> = match self.module(&key) {
            Some(module) => {
                let specifiers = module.ast.import_specifiers();
                let mut edges: Vec<PathBuf> = Vec::with_capacity(specifiers.len());
                for specifier in specifiers {
                    let edge = if specifier.starts_with('.') {
                        resolve_relative_import(&key, &specifier)
                    } else {
                        self.aliased_import(&specifier)
                    };
                    if let Some(edge) = edge {
                        edges.push(edge);
                    }
                }
                let provider = self.project_root.as_deref().and_then(|root| {
                    ruvyxa_bundler::content::resolve_mdx_components_file_in_root(&key, root)
                });
                if let Some(provider) = provider {
                    edges.push(normalized_canonical_path(&provider));
                }
                edges.into()
            }
            None => Arc::from([] as [PathBuf; 0]),
        };
        self.edges.insert(key, resolved.clone());
        resolved
    }
}

fn collect_relative_graph(entry: &Path, cache: &mut ModuleCache) -> BTreeSet<PathBuf> {
    let mut visited = BTreeSet::new();
    // Normalize the entry exactly like resolved imports so a cycle back to
    // the entry file compares equal instead of being visited twice.
    let mut queue = VecDeque::from([cache.canonical(entry)]);

    while let Some(file) = queue.pop_front() {
        if !visited.insert(file.clone()) {
            continue;
        }

        queue.extend(cache.edges(&file).iter().cloned());
    }

    visited
}

fn resolve_relative_import(from: &Path, specifier: &str) -> Option<PathBuf> {
    let base = from.parent()?.join(specifier);
    let candidates = [
        base.clone(),
        base.with_extension("ts"),
        base.with_extension("tsx"),
        base.with_extension("js"),
        base.with_extension("jsx"),
        base.with_extension("md"),
        base.with_extension("mdx"),
        base.join("index.ts"),
        base.join("index.tsx"),
        base.join("index.js"),
        base.join("index.jsx"),
        base.join("index.md"),
        base.join("index.mdx"),
    ];

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| normalized_canonical_path(&candidate))
}

/// Statically-known `process.env` reads that must not reach the browser.
///
/// The reads come from the bundler's scanner and the rule that judges them comes
/// from the bundler's boundary check, so `check` and `build` cannot disagree
/// about either which env vars a module touches or which of them are allowed.
///
/// Both halves used to be local. The scan was a private marker search over a
/// privately-masked copy of the source; the rule was a hand-copied filter that
/// had lost the `NODE_ENV` exemption, so `check` rejected with RUV1008 what
/// `build` compiled without complaint.
fn private_env_reads(ast: &ruvyxa_bundler::ast::ModuleAst) -> impl Iterator<Item = &str> {
    ast.env_reads
        .iter()
        .map(String::as_str)
        .filter(|name| ruvyxa_bundler::boundary::env_read_is_private(name))
}

fn relative_starts_with_server(relative: &Path) -> bool {
    relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == "server")
}

/// Blank out strings, template text, comments, and regular-expression literals.
///
/// Rendering-strategy detection matches on code *text* — `export const
/// revalidate`, `fetch(`, `process.env.` — so it needs masked source rather than
/// structured facts, and byte offsets and line breaks are preserved for it.
///
/// The masking is the bundler's, which is the point. This file used to carry a
/// character-wise lexer of its own: a duplicate `regex_can_start`, a duplicate
/// template-literal walk, a duplicate comment skipper. A bug in that copy read
/// `/['"]/` as a division followed by an unterminated string and blanked
/// everything after it, silently disabling RUV1007/RUV1008/RUV1010 for the
/// module. One scanner cannot drift from itself.
fn code_without_strings_and_comments(source: &str) -> String {
    ruvyxa_bundler::ast::masked_code(source)
}

fn route_path_from_dir(relative_dir: &Path) -> Result<String> {
    let visible_segments = relative_dir
        .components()
        .filter_map(|component| {
            let Component::Normal(segment) = component else {
                return None;
            };
            let segment = segment.to_string_lossy();

            if (segment.starts_with('(') && segment.ends_with(')')) || segment.starts_with('@') {
                None
            } else {
                Some(segment.into_owned())
            }
        })
        .collect::<Vec<_>>();
    let mut segments = Vec::with_capacity(visible_segments.len());

    for (index, segment) in visible_segments.iter().enumerate() {
        segments.push(route_segment(segment, index + 1 == visible_segments.len())?);
    }

    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}

/// Next.js intercepting-route markers, longest first.
///
/// Order is load-bearing: `(..)(..)` also starts with `(..)`, and `(...)` also
/// starts with `(.`, so a shorter marker tested first would name the wrong
/// convention in the diagnostic.
const INTERCEPTING_ROUTE_MARKERS: [&str; 4] = ["(..)(..)", "(...)", "(..)", "(.)"];

/// The intercepting-route marker a directory name opens with, if any.
fn intercepting_route_marker(segment: &str) -> Option<&'static str> {
    INTERCEPTING_ROUTE_MARKERS
        .into_iter()
        .find(|marker| segment.starts_with(marker))
}

/// How many route levels a marker climbs before the segment it names.
///
/// `(...)` is the odd one: it restarts from the app root rather than climbing a
/// fixed number of levels, so it is reported separately.
fn intercept_climb(marker: &str) -> Option<usize> {
    match marker {
        "(.)" => Some(0),
        "(..)" => Some(1),
        "(..)(..)" => Some(2),
        _ => None,
    }
}

/// Whether a project-relative directory sits inside a parallel-route slot.
fn is_inside_slot(relative: &Path) -> bool {
    relative.components().any(|component| {
        matches!(component, Component::Normal(name) if name.to_string_lossy().starts_with('@'))
    })
}

/// Refuse intercepting-route directories that no slot can render.
///
/// An interception is an overlay: it replaces a parallel-route slot while the
/// page underneath stays mounted, so it only means something inside an `@name`
/// folder. Outside one there is nothing to render it into, and the folder used
/// to become a literal URL segment instead — the route-group branch needs a
/// trailing `)`, so `app/feed/(.)photo/page.tsx` passed straight through
/// [`route_segment`] and mounted a real, publicly reachable page at
/// `/feed/(.)photo`, a view the author wrote as an interception and never meant
/// to publish on its own URL.
///
/// This walks directories rather than the segments of discovered routes,
/// because the route walk skips `@slot` folders. `_`-prefixed folders are
/// excluded: they opt out of routing entirely, so nothing there can reach a
/// URL.
fn reject_intercepting_routes(app_dir: &Path) -> Result<()> {
    let mut offenders = WalkDir::new(app_dir)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || entry.path() == app_dir
                || !entry.file_name().to_string_lossy().starts_with('_')
        })
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let marker = intercepting_route_marker(&name)?;
            let relative = entry.path().strip_prefix(app_dir).ok()?;
            // Inside a slot the folder is a real interception, resolved by
            // `route_intercepts`. Everywhere else it is a mistake.
            if is_inside_slot(relative) {
                return None;
            }
            Some((entry.path().to_path_buf(), name, marker))
        })
        .collect::<Vec<_>>();
    // Directory order is filesystem order, so which offender is reported would
    // otherwise differ between machines building the same project.
    offenders.sort_by(|left, right| left.0.cmp(&right.0));

    let Some((path, name, marker)) = offenders.into_iter().next() else {
        return Ok(());
    };
    Err(Diagnostic::new("RUV1005", "Intercepting route is outside a parallel-route slot")
        .explain(format!(
            "`{name}` opens with the intercepting-route marker `{marker}`, but it does not live inside an `@name` folder. An interception replaces a slot while the page underneath stays mounted, so there is nowhere to render this one."
        ))
        .at_file(&path)
        .suggest("Move the folder inside a parallel-route slot beside the layout that should show it, such as `@modal`, or rename it to an ordinary route segment.")
        .into())
}

fn route_segment(segment: &str, is_last: bool) -> Result<String> {
    if segment.starts_with("[[...") && segment.ends_with("]]") {
        let name = &segment[5..segment.len() - 2];
        validate_dynamic_name(name)?;
        if !is_last {
            return Err(catch_all_must_be_last());
        }
        return Ok(segment.to_string());
    }

    if segment.starts_with("[...") && segment.ends_with(']') {
        let name = &segment[4..segment.len() - 1];
        validate_dynamic_name(name)?;
        if !is_last {
            return Err(catch_all_must_be_last());
        }
        return Ok(segment.to_string());
    }

    if segment.starts_with('[') && segment.ends_with(']') {
        let name = &segment[1..segment.len() - 1];
        validate_dynamic_name(name)?;
        return Ok(segment.to_string());
    }

    if segment.contains('[') || segment.contains(']') {
        return Err(Diagnostic::new("RUV1002", "Invalid dynamic route segment")
            .explain("Dynamic route segments must use [name], [...name], or [[...name]].")
            .suggest("Rename the route folder to a valid dynamic segment.")
            .into());
    }

    Ok(segment.to_string())
}

fn validate_dynamic_name(name: &str) -> Result<()> {
    if !name.is_empty() && !name.contains(['[', ']']) && !name.starts_with('.') {
        return Ok(());
    }

    Err(Diagnostic::new("RUV1002", "Invalid dynamic route segment")
        .explain("Dynamic route parameter names must be non-empty and cannot contain brackets or begin with a dot.")
        .suggest("Use [name], [...name], or [[...name]] with a non-empty parameter name.")
        .into())
}

fn catch_all_must_be_last() -> RuvyxaError {
    Diagnostic::new("RUV1002", "Catch-all route must be the final URL segment")
        .explain("Catch-all routes consume every remaining URL segment and cannot have a child URL segment.")
        .suggest("Move the catch-all folder to the end of the route or remove the child segment.")
        .into()
}

fn route_id(app_dir: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(app_dir).unwrap_or(file);
    let without_extension = relative.with_extension("");
    format!(
        "app/{}",
        without_extension
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy().replace('\\', "/")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/")
    )
}

fn layout_chain(app_dir: &Path, route_dir: &Path) -> Vec<String> {
    nested_chain(app_dir, route_dir, "layout.tsx")
}

/// One intercepting route reachable from a particular route.
///
/// `app/feed/@modal/(.)photo/page.tsx` declares that a soft navigation to
/// `/feed/photo` should render `page.tsx` into the `modal` slot of the layout
/// at `app/feed`, leaving whatever is already on screen mounted underneath. A
/// hard load of `/feed/photo` is unaffected: it renders `app/feed/photo`, the
/// ordinary route, which must exist for the interception to be accepted at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteIntercept {
    /// Directory holding the `@name` folder, as a route id (`app/feed`).
    pub level: String,
    /// Slot name without the `@`, which is the prop the layout receives.
    pub name: String,
    /// Route pattern this interception covers, in the same shape as
    /// [`RouteEntry::path`] so one matcher answers both.
    pub target: String,
    /// The marker the author wrote, kept for diagnostics.
    pub marker: String,
    /// File that renders the interception.
    pub file: PathBuf,
}

/// One parallel-route slot resolved for a particular route.
///
/// A `@name` directory beside a `layout.tsx` declares a slot that layout
/// receives as a prop. The slot matches the URL independently of the page: for
/// `/dashboard/reports`, the slot at `app/dashboard/@team` renders
/// `@team/reports/page.tsx` if it exists, and `@team/default.tsx` otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSlot {
    /// Directory holding the `@name` folder, as a route id (`app/dashboard`).
    /// This is the level whose layout receives the slot.
    pub level: String,
    /// Slot name without the `@`, which is the prop name the layout sees.
    pub name: String,
    /// File that renders this slot for this route.
    pub file: PathBuf,
}

/// Parallel-route slots in scope for a route, level order then name order.
///
/// Walks the same directory chain the layout and template chains do, and at
/// each level resolves every `@name` folder against the route's remaining
/// segments. A slot that matches neither a page nor a `default.tsx` is left out
/// entirely — the layout sees no prop, which is the same thing Next.js renders
/// for an unmatched slot with no default.
fn route_slots(app_dir: &Path, route_dir: &Path) -> Vec<RouteSlot> {
    let Ok(relative) = route_dir.strip_prefix(app_dir) else {
        return Vec::new();
    };
    let segments = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut slots = Vec::new();
    let mut level = app_dir.to_path_buf();
    for depth in 0..=segments.len() {
        if depth > 0 {
            level.push(&segments[depth - 1]);
        }
        // The URL below this level is what the slot has to match.
        let remaining = &segments[depth..];
        slots.extend(slots_at_level(app_dir, &level, remaining));
    }
    slots
}

/// Interceptions in scope for a route, level order then slot name then target.
///
/// Walks the same directory chain the layout, template, and slot chains do. At
/// each level, every `@name` folder is searched for children whose first
/// segment carries an intercepting-route marker, and each one is resolved to
/// the URL it covers.
///
/// The target is computed from the *level's* URL rather than from the slot
/// folder, because a slot contributes no URL segment: for
/// `app/feed/@modal/(.)photo`, `(.)` means "the level `app/feed` is on", so the
/// target is `/feed/photo`.
fn route_intercepts(app_dir: &Path, route_dir: &Path) -> Result<Vec<RouteIntercept>> {
    let Ok(relative) = route_dir.strip_prefix(app_dir) else {
        return Ok(Vec::new());
    };
    let segments = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut intercepts = Vec::new();
    let mut level = app_dir.to_path_buf();
    for depth in 0..=segments.len() {
        if depth > 0 {
            level.push(&segments[depth - 1]);
        }
        intercepts.extend(intercepts_at_level(app_dir, &level)?);
    }
    // Directory order is filesystem order, and this list decides the order the
    // generated entry emits its lookup table in.
    intercepts.sort_by(|left, right| {
        (&left.level, &left.name, &left.target).cmp(&(&right.level, &right.name, &right.target))
    });
    intercepts.dedup();
    Ok(intercepts)
}

/// Every interception declared by an `@name` folder directly inside `level`.
fn intercepts_at_level(app_dir: &Path, level: &Path) -> Result<Vec<RouteIntercept>> {
    let Ok(entries) = fs::read_dir(level) else {
        return Ok(Vec::new());
    };
    let level_relative = level.strip_prefix(app_dir).unwrap_or(Path::new(""));
    let level_path = route_path_from_dir(level_relative)?;

    let mut slots = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let raw = entry.file_name();
            let name = raw.to_string_lossy();
            let name = name.strip_prefix('@')?.to_string();
            (!name.is_empty()).then(|| (name, entry.path()))
        })
        .collect::<Vec<_>>();
    slots.sort_by(|left, right| left.0.cmp(&right.0));

    let mut intercepts = Vec::new();
    for (name, slot_dir) in slots {
        for (file, marker, target_segments) in intercept_pages(&slot_dir) {
            let target = intercept_target_path(&level_path, marker, &target_segments)
                .ok_or_else(|| intercept_climbs_past_root(&file, marker, &level_path))?;
            intercepts.push(RouteIntercept {
                level: directory_id(app_dir, level),
                name: name.clone(),
                target,
                marker: marker.to_string(),
                file,
            });
        }
    }
    Ok(intercepts)
}

/// Page files under a slot whose first segment carries a marker.
///
/// Returns the page, the marker, and the URL segments it contributes — the
/// first segment with the marker stripped, then everything below it.
fn intercept_pages(slot_dir: &Path) -> Vec<(PathBuf, &'static str, Vec<String>)> {
    let mut found = Vec::new();
    for entry in WalkDir::new(slot_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if !matches!(
            entry.file_name().to_string_lossy().as_ref(),
            "page.tsx" | "page.jsx" | "page.md" | "page.mdx"
        ) {
            continue;
        }
        let Some(page_dir) = entry.path().parent() else {
            continue;
        };
        let Ok(relative) = page_dir.strip_prefix(slot_dir) else {
            continue;
        };
        let mut segments = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let Some(first) = segments.first().cloned() else {
            continue;
        };
        let Some(marker) = intercepting_route_marker(&first) else {
            continue;
        };
        let head = first[marker.len()..].to_string();
        if head.is_empty() {
            continue;
        }
        segments[0] = head;
        found.push((entry.path().to_path_buf(), marker, segments));
    }
    found
}

/// The URL an interception covers, or `None` when the marker climbs past root.
fn intercept_target_path(level_path: &str, marker: &str, segments: &[String]) -> Option<String> {
    let base = match intercept_climb(marker) {
        // `(...)` restarts from the app root rather than climbing levels.
        None => "/".to_string(),
        Some(climb) => drop_route_segments(level_path, climb)?,
    };
    let mut parts = base
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    parts.extend(segments.iter().cloned());
    Some(format!("/{}", parts.join("/")))
}

/// Drop `count` trailing segments from a route path, or `None` if it cannot.
fn drop_route_segments(path: &str, count: usize) -> Option<String> {
    let mut parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < count {
        return None;
    }
    parts.truncate(parts.len() - count);
    Some(format!("/{}", parts.join("/")))
}

fn intercept_climbs_past_root(file: &Path, marker: &str, level_path: &str) -> RuvyxaError {
    Diagnostic::new("RUV1006", "Intercepting route climbs above the app root")
        .explain(format!(
            "`{marker}` asks for a level above `{level_path}`, and there is nothing there. A marker can only climb as many levels as the slot's own route has."
        ))
        .at_file(file)
        .suggest("Use a shorter marker, or `(...)` to name a path from the app root.")
        .into()
}

/// Every `@name` folder directly inside `level`, resolved against `remaining`.
fn slots_at_level(
    app_dir: &Path,
    level: &Path,
    remaining: &[std::ffi::OsString],
) -> Vec<RouteSlot> {
    let Ok(entries) = fs::read_dir(level) else {
        return Vec::new();
    };
    let mut named = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let raw = entry.file_name();
            let name = raw.to_string_lossy();
            let name = name.strip_prefix('@')?.to_string();
            // A slot needs a name; `@` alone is a directory nobody can address.
            (!name.is_empty()).then(|| (name, entry.path()))
        })
        .collect::<Vec<_>>();
    // Directory order is filesystem order, which differs between machines and
    // decides prop order in the generated entry.
    named.sort_by(|left, right| left.0.cmp(&right.0));

    named
        .into_iter()
        .filter_map(|(name, slot_dir)| {
            let file = slot_page_for(&slot_dir, remaining)?;
            Some(RouteSlot {
                level: directory_id(app_dir, level),
                name,
                file,
            })
        })
        .collect()
}

/// The file a slot renders for the remaining URL segments.
///
/// The slot's own page for that sub-path when it has one, and its
/// `default.tsx` otherwise. `default.tsx` is what a slot falls back to when the
/// URL does not name anything inside it, which is the majority of navigations
/// once more than one slot exists.
fn slot_page_for(slot_dir: &Path, remaining: &[std::ffi::OsString]) -> Option<PathBuf> {
    let mut target = slot_dir.to_path_buf();
    for segment in remaining {
        target.push(segment);
    }
    for name in ["page.tsx", "page.jsx", "page.md", "page.mdx"] {
        let candidate = target.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for name in ["default.tsx", "default.jsx"] {
        let candidate = slot_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Route id for a directory, matching [`route_id`]'s shape for a file.
fn directory_id(app_dir: &Path, directory: &Path) -> String {
    let relative = directory.strip_prefix(app_dir).unwrap_or(directory);
    let segments = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().replace('\\', "/")),
            _ => None,
        })
        .collect::<Vec<_>>();
    if segments.is_empty() {
        "app".to_string()
    } else {
        format!("app/{}", segments.join("/"))
    }
}

/// `template.tsx` files from the app root down to the route, root first.
///
/// A template wraps its level's children the way a layout does, and differs in
/// one respect that is the whole reason it exists: it is given a key derived
/// from the request path, so navigating within the same layout remounts it —
/// state resets and effects run again. Composition interleaves the two, layout
/// outside template at each level; see `route_wrapper_levels` in
/// `crates/ruvyxa_bundler/src/output.rs` and its mirror in
/// `packages/ruvyxa/runtime/entry-templates.mjs`.
fn template_chain(app_dir: &Path, route_dir: &Path) -> Vec<String> {
    nested_chain(app_dir, route_dir, "template.tsx")
}

/// Files named `file_name` on the path from the app root to `route_dir`, root
/// first.
fn nested_chain(app_dir: &Path, route_dir: &Path, file_name: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = app_dir.to_path_buf();

    if current.join(file_name).exists() {
        found.push(route_id(app_dir, &current.join(file_name)));
    }

    if let Ok(relative) = route_dir.strip_prefix(app_dir) {
        for component in relative.components() {
            let Component::Normal(segment) = component else {
                continue;
            };
            current.push(segment);
            let candidate = current.join(file_name);
            if candidate.exists() {
                found.push(route_id(app_dir, &candidate));
            }
        }
    }

    found
}

fn resolve_layout_file(app_dir: &Path, layout_id: &str) -> Option<PathBuf> {
    let layout = PathBuf::from(layout_id);
    let project_root = app_dir.parent().unwrap_or(app_dir);
    let app_relative = layout
        .strip_prefix("app")
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| layout.clone());
    let candidates = [project_root.join(&layout), app_dir.join(app_relative)];

    // `normalized_canonical_path`, not `Path::canonicalize`: the raw call
    // returns the Windows extended-length prefix, and every caller feeds this
    // path into `ModuleCache`, which keys on it. The cache normalizes on the
    // way in, so nothing is wrong today — but handing out a verbatim path is
    // the shape that broke server-component builds once already, and the next
    // caller has no reason to expect it.
    candidates.into_iter().find_map(|candidate| {
        [candidate.clone(), candidate.with_extension("tsx")]
            .into_iter()
            .find(|file| file.is_file())
            .map(|file| normalized_canonical_path(&file))
    })
}

fn sibling_module(route_dir: &Path, name: &str) -> Vec<String> {
    let module = route_dir.join(name);
    if module.exists() {
        vec![module.display().to_string()]
    } else {
        Vec::new()
    }
}

fn sibling_modules(route_dir: &Path, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .flat_map(|name| sibling_module(route_dir, name))
        .collect()
}

/// Detect a page's rendering strategy, and whether it opts into server components.
///
/// The two are read separately because they answer different questions — see
/// [`RenderMeta::server_components`] — and because the strategy rules below
/// return early in six places, each of which would otherwise have to remember
/// to carry the opt-in.
fn detect_render_strategy(
    app_dir: &Path,
    file: &Path,
    route_path: &str,
    layout_chain: &[String],
    cache: &mut ModuleCache,
) -> RenderMeta {
    let mut render = detect_render_meta(app_dir, file, route_path, layout_chain, cache);
    render.server_components = opts_into_server_components(file, cache);
    render
}

/// Whether a page declares `export const serverComponents = true`.
///
/// Read from masked source for the same reason every other route export is: a
/// commented-out or quoted occurrence is not a declaration, and reading raw
/// text turned one into a silent change of rendering pipeline.
fn opts_into_server_components(file: &Path, cache: &mut ModuleCache) -> bool {
    let Some(page) = cache.module(file) else {
        return false;
    };
    let source = Arc::clone(&page.source);
    let Some(code) = cache.masked(file) else {
        return false;
    };
    has_export_const_bool(&source, &code, "serverComponents", true)
}

/// Detect the rendering strategy for a page by scanning its source for known exports/directives.
///
/// Detection rules (first match wins):
/// 1. `"use client"` directive at top → CSR
/// 2. `export const ppr = true` → PPR
/// 3. `export const revalidate = <number>` → ISR with that interval
/// 4. `getStaticParams` or `staticParams` page export → SSG
/// 5. Route has no dynamic segments and no data fetching → SSG candidate (static routes)
/// 6. Default → SSR
fn detect_render_meta(
    app_dir: &Path,
    file: &Path,
    route_path: &str,
    layout_chain: &[String],
    cache: &mut ModuleCache,
) -> RenderMeta {
    let Some(page) = cache.module(file) else {
        return RenderMeta::default();
    };
    let source = Arc::clone(&page.source);
    let Some(code) = cache.masked(file) else {
        return RenderMeta::default();
    };

    // 1. Check for "use client" directive (must be in original source, at top)
    let trimmed = source.trim_start();
    if trimmed.starts_with("\"use client\"") || trimmed.starts_with("'use client'") {
        // CSR pages render entirely in the browser, so the hydration opt-out
        // does not apply — the directive wins.
        return RenderMeta {
            strategy: RenderStrategy::Csr,
            ..Default::default()
        };
    }

    // Boolean false remains the zero-JS contract; string values add deferred
    // route-level hydration without changing the default.
    let hydration = parse_hydration_mode(&source, &code);

    // 1b. `export const dynamic` — the route segment config Next.js uses to
    // override the automatic choice. A page written against that convention
    // used to be read by nothing here: `force-dynamic` on an otherwise-static
    // page was discarded and the page was pre-rendered anyway, which is the
    // opposite of what it asked for and produced no diagnostic. It is checked
    // before the opt-in exports below because that is the precedence Next
    // defines — `force-dynamic` outranks `revalidate`.
    match export_const_value(&source, &code, "dynamic")
        .map(|value| value.trim_end_matches(';').trim().trim_matches(['\'', '"']))
    {
        Some("force-dynamic") => {
            return RenderMeta {
                hydration,
                force_dynamic: true,
                ..Default::default()
            };
        }
        // `error` is `force-static` plus a runtime complaint about dynamic
        // APIs; this graph decides strategy, not runtime behaviour, so the
        // strategy is the part it can honour.
        Some("force-static" | "error") => {
            return RenderMeta {
                strategy: RenderStrategy::Ssg,
                has_static_params: has_static_params_export(&code),
                hydration,
                ..Default::default()
            };
        }
        // `auto` is the default, and anything else is not this export.
        _ => {}
    }

    // 2. Check for PPR opt-in: export const ppr = true
    if has_export_const_bool(&source, &code, "ppr", true) {
        return RenderMeta {
            strategy: RenderStrategy::Ppr,
            has_dynamic_slots: true,
            hydration,
            ..Default::default()
        };
    }

    // 3. Check for ISR: export const revalidate = <number>
    if let Some(seconds) = parse_export_const_number(&source, &code, "revalidate") {
        let has_static_params = has_static_params_export(&code);
        return RenderMeta {
            strategy: RenderStrategy::Isr,
            revalidate: Some(seconds),
            has_static_params,
            hydration,
            ..Default::default()
        };
    }

    // 4. Check for dynamic SSG parameter exports.
    if has_static_params_export(&code) {
        return RenderMeta {
            strategy: RenderStrategy::Ssg,
            has_static_params: true,
            hydration,
            ..Default::default()
        };
    }

    // 5. Static routes with no dynamic data markers can be pre-rendered.
    //
    // The reachable graph is only read here. Rules 1-4 answer from the page's
    // own exports, so reading and masking every dependency ahead of them was
    // work whose result was discarded for every page that opts into a strategy
    // explicitly.
    if !route_has_dynamic_segments(route_path) {
        // A dependency that cannot be read cannot be cleared of data markers,
        // so an unreadable graph leaves the route dynamic rather than guessing
        // it static.
        let reachable_code = render_reachable_code(app_dir, file, layout_chain, cache);
        if reachable_code.is_some_and(|code| !has_dynamic_data_markers(&code)) {
            return RenderMeta {
                strategy: RenderStrategy::Ssg,
                hydration,
                ..Default::default()
            };
        }
    }

    // 6. Default: SSR
    RenderMeta {
        hydration,
        ..Default::default()
    }
}

/// Parse the additive route hydration export while preserving boolean input.
///
/// `hydrate` decides whether a page ships a client bundle at all, so reading it
/// wrongly in either direction is expensive: a missed opt-out ships JavaScript
/// the author disabled, and a false positive drops the hydration a working page
/// depends on. Both happened while this read the raw source — see
/// [`export_const_value`].
/// Node built-ins an edge route may not reach.
///
/// Conservative on purpose: these are the modules that need a filesystem, a
/// process table, or a socket, and no V8 isolate has them. The ones every edge
/// runtime ships a polyfill for — `buffer`, `crypto`, `path`, `stream`, `util`
/// — are deliberately absent, because a false refusal on a module that does
/// work costs more than a missing one. The missing one still fails at deploy,
/// loudly, on the platform that knows its own surface.
///
/// Kept level with `unavailableOnEdge` in
/// `tests/fixtures/edge-runtime-conformance.json`, which the tests replay.
const EDGE_UNAVAILABLE_BUILTINS: &[&str] = &[
    "child_process",
    "cluster",
    "dgram",
    "dns",
    "fs",
    "http2",
    "inspector",
    "module",
    "net",
    "os",
    "readline",
    "repl",
    "tls",
    "trace_events",
    "tty",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
];

/// The forbidden built-in this specifier names, if it names one.
///
/// Matches the segment before the first `/` so `node:fs/promises` and
/// `fs/promises` are the same answer as `fs`.
fn edge_forbidden_builtin(specifier: &str) -> Option<&'static str> {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    let head = bare.split('/').next().unwrap_or(bare);
    EDGE_UNAVAILABLE_BUILTINS
        .iter()
        .find(|name| **name == head)
        .copied()
}

/// The runtime one `export const runtime = …` value names, or `None` when it
/// names nothing this framework has.
///
/// Split from the file walk so the accepted spellings can be replayed against
/// the shared table without a temporary project on disk.
fn runtime_target_from_value(raw: &str) -> Option<RuntimeTarget> {
    let value = raw.trim_end_matches(';').trim();
    let value = value.strip_suffix("as const").unwrap_or(value).trim();
    match value.trim_matches(['\'', '"', '`']) {
        "edge" => Some(RuntimeTarget::Edge),
        "nodejs" | "node" => Some(RuntimeTarget::Node),
        _ => None,
    }
}

/// The runtime a route asks to run on — `export const runtime = 'edge'`.
///
/// Read from the route's own file and from nothing it imports: a dependency
/// cannot move the route that uses it, and the whole value of the declaration
/// is that one glance at the route says where it runs.
///
/// An unrecognised value is an error rather than a fall back to Node. The
/// declaration exists because the author meant somewhere specific; accepting
/// `'Edge'` or `'worker'` silently would put the route on the other runtime
/// with nothing said, and the difference only shows up in production, where an
/// edge route has no filesystem and a Node route has no Worker globals.
///
/// `nodejs` is spelled the way Next.js spells it, so a route moved between the
/// two frameworks does not change meaning.
fn detect_runtime_target(file: &Path, cache: &mut ModuleCache) -> Result<RuntimeTarget> {
    let Some(module) = cache.module(file) else {
        return Ok(RuntimeTarget::Node);
    };
    let source = Arc::clone(&module.source);
    let Some(masked) = cache.masked(file) else {
        return Ok(RuntimeTarget::Node);
    };
    let Some(raw) = export_const_value(&source, &masked, "runtime") else {
        return Ok(RuntimeTarget::Node);
    };

    match runtime_target_from_value(raw) {
        Some(target) => Ok(target),
        None => Err(Diagnostic::new("RUV1012", "Unknown route runtime")
            .explain(format!(
                "`export const runtime = {}` names a runtime this framework does not have. The choices are `'edge'` and `'nodejs'`.",
                raw.trim()
            ))
            .at_file(file)
            .suggest(
                "Use `export const runtime = 'edge'` to run this route on a Web-standards runtime, or remove the export to keep the Node default.",
            )
            .into()),
    }
}

fn parse_hydration_mode(source: &str, masked: &str) -> HydrationMode {
    let Some(value) = export_const_value(source, masked, "hydrate") else {
        return HydrationMode::Load;
    };
    let value = value.trim_end_matches(';').trim();
    let value = value.strip_suffix("as const").unwrap_or(value).trim();
    match value.trim_matches(['\'', '"']) {
        "false" | "none" => HydrationMode::None,
        "idle" => HydrationMode::Idle,
        "visible" => HydrationMode::Visible,
        _ => HydrationMode::Load,
    }
}

/// Return all statically reachable route and layout source after stripping strings/comments.
/// Route-level rendering exports are intentionally handled from the page source only, while data
/// markers in any dependency make automatic SSG unsafe.
fn render_reachable_code(
    app_dir: &Path,
    file: &Path,
    layout_chain: &[String],
    cache: &mut ModuleCache,
) -> Option<String> {
    let mut files = collect_relative_graph(file, cache);
    for layout in layout_chain {
        let layout = resolve_layout_file(app_dir, layout)?;
        files.extend(collect_relative_graph(&layout, cache));
    }

    let mut code = String::new();
    for path in files {
        code.push_str(&cache.masked(&path)?);
        code.push('\n');
    }
    Some(code)
}

fn apply_rendering_defaults(
    mut render: RenderMeta,
    default_strategy: Option<RenderStrategy>,
    default_revalidate: Option<u64>,
) -> RenderMeta {
    if render.strategy != RenderStrategy::Ssr {
        return render;
    }

    let Some(strategy) = default_strategy else {
        return render;
    };

    render.strategy = strategy;
    if strategy == RenderStrategy::Isr {
        render.revalidate = Some(default_revalidate.unwrap_or(60));
    }
    render
}

fn route_has_dynamic_segments(route_path: &str) -> bool {
    route_path
        .split('/')
        .any(|segment| segment.starts_with('[') && segment.ends_with(']'))
}

fn has_dynamic_data_markers(code: &str) -> bool {
    const MARKERS: &[&str] = &[
        "fetch(",
        "headers(",
        "cookies(",
        "searchParams",
        "Date.now(",
        "Math.random(",
        "process.env.",
    ];

    MARKERS
        .iter()
        .any(|marker| contains_marker_identifier(code, marker))
}

/// True when `marker` appears in `code` as its own identifier.
///
/// A plain `contains` reads `router.prefetch('/products')` as a `fetch(` call
/// and takes automatic pre-rendering away from a page that only warms a link —
/// `prefetch` is an API this framework ships, so the collision is the ordinary
/// case rather than a contrived one.
///
/// Only the leading edge is checked. Every marker already ends at a `(` or a
/// `.`, which cannot continue an identifier, and a member access has to keep
/// matching: `globalThis.fetch(` is a fetch.
///
/// A byte that begins a multi-byte character is not an ASCII identifier byte,
/// so an identifier outside ASCII reads as a word boundary and the marker
/// counts. That is the safe direction: this decides whether a route may be
/// pre-rendered, and a false marker costs a static page while a missed one
/// ships stale data.
fn contains_marker_identifier(code: &str, marker: &str) -> bool {
    let bytes = code.as_bytes();
    code.match_indices(marker).any(|(start, _)| {
        !start.checked_sub(1).is_some_and(
            |index| matches!(bytes[index], b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'$'),
        )
    })
}

/// Right-hand side of `export const <name> = …`, taken from the real source.
///
/// Two things have to be true at once, and doing only one of them is how every
/// route-export scanner here used to get the answer wrong.
///
/// *The statement must be code.* Positions are found in `masked` — the shared
/// [`code_without_strings_and_comments`] view, where comment and literal text is
/// blanked but every byte offset still names the same place in `source`. Reading
/// the raw text instead made a commented-out `export const hydrate = false`, or
/// one quoted inside a documentation snippet, switch off the real page's
/// hydration.
///
/// *The value must be the source's.* Masking blanks string contents, so
/// `'idle'` reads as `'    '`. The span is located in `masked` and then sliced
/// out of `source`, which is what lets a string-valued export be recognised at
/// all.
///
/// A TypeScript annotation between the name and `=` is skipped. `has_export_function`
/// already tolerated one; these did not, so `export const revalidate: number = 3600`
/// silently lost its ISR opt-in and `export const ppr: boolean = true` its PPR opt-in.
///
/// Returns `None` when the declaration does not finish on its own line — the
/// scan is line-based and will not guess at a continuation.
fn export_const_value<'a>(source: &'a str, masked: &str, name: &str) -> Option<&'a str> {
    debug_assert_eq!(
        source.len(),
        masked.len(),
        "masked_code preserves length, which is what makes these offsets shared"
    );
    let prefix = format!("export const {name}");
    let mut line_start = 0usize;

    for line in masked.lines() {
        let start = line_start;
        // `masked_code` keeps every `\n` in place and turns a `\r` into a space,
        // so one byte always separates consecutive lines in both strings.
        line_start += line.len() + 1;

        let indent = line.len() - line.trim_start().len();
        let Some(after) = line[indent..].strip_prefix(prefix.as_str()) else {
            continue;
        };
        // `export const hydrateAll` is a different export.
        if after
            .chars()
            .next()
            .is_some_and(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
        {
            continue;
        }
        let Some(equals) = assignment_offset(after) else {
            continue;
        };

        let value_start = indent + prefix.len() + equals + 1;
        let masked_tail = &line[value_start..];
        let raw_tail = &source[start + value_start..start + line.len()];

        // Where the value ends depends on what masking left behind. When any
        // code survives — `false`, `3600`, `'idle' as const` — the last
        // non-blank byte is the end, and a trailing comment is already blank.
        // When nothing survives the value is one string literal, whose own text
        // was blanked; only then is the literal measured directly, over a span
        // masking has already proven holds no code.
        let end = if masked_tail.trim().is_empty() {
            quoted_literal_end(raw_tail)?
        } else {
            masked_tail.trim_end().len()
        };
        let value = raw_tail.get(..end)?.trim();
        return (!value.is_empty()).then_some(value);
    }
    None
}

/// Byte just past the quoted literal that starts `tail`, if one does.
///
/// Only reached for a value masking reported as entirely non-code, so this
/// measures one literal rather than lexing a program — there is no second
/// scanner here to drift from [`code_without_strings_and_comments`].
fn quoted_literal_end(tail: &str) -> Option<usize> {
    let bytes = tail.as_bytes();
    let start = bytes.iter().position(|byte| !byte.is_ascii_whitespace())?;
    let quote = bytes[start];
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            byte if byte == quote => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Offset of the assignment `=` in `after`, skipping any type annotation.
///
/// `=` also appears in `=>`, `==`, `<=`, `>=`, and `!=`, all of which occur
/// inside a type (`: Record<string, () => void>`), so only a bare `=` outside
/// every bracket pair counts.
///
/// `<` and `>` are deliberately not counted as a pair. They are not reliably
/// balanced in TypeScript — `=>` alone would close a depth nothing opened, and
/// comparison operators do the same — so tracking them turned an ordinary
/// annotation into a negative depth and lost the assignment entirely. The three
/// bracket pairs that are always balanced are enough to keep a `=` inside a
/// parameter list or object type from being mistaken for the assignment.
fn assignment_offset(after: &str) -> Option<usize> {
    let bytes = after.as_bytes();
    let mut depth = 0i32;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'=' if depth == 0 => {
                let follows = bytes.get(index + 1);
                let precedes = index.checked_sub(1).map(|previous| bytes[previous]);
                if follows == Some(&b'=')
                    || follows == Some(&b'>')
                    || matches!(precedes, Some(b'=' | b'!' | b'<' | b'>'))
                {
                    continue;
                }
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

/// Check if `export const <name> = true|false` exists.
fn has_export_const_bool(source: &str, masked: &str, name: &str, expected: bool) -> bool {
    export_const_value(source, masked, name)
        .map(|value| value.trim_end_matches(';').trim())
        .is_some_and(|value| value == if expected { "true" } else { "false" })
}

/// Parse `export const <name> = <number>` and return the number.
fn parse_export_const_number(source: &str, masked: &str, name: &str) -> Option<u64> {
    export_const_value(source, masked, name)?
        .trim_end_matches(';')
        .trim()
        .parse::<u64>()
        .ok()
}

/// Check if `export function <name>` or `export async function <name>` exists.
fn has_export_function(code: &str, name: &str) -> bool {
    let patterns = [
        format!("export function {name}"),
        format!("export async function {name}"),
        format!("export const {name}"),
    ];
    for line in code.lines() {
        let trimmed = line.trim();
        for pattern in &patterns {
            let Some(rest) = trimmed.strip_prefix(pattern.as_str()) else {
                continue;
            };
            if rest.chars().next().is_none_or(|character| {
                character.is_whitespace() || matches!(character, '(' | '<' | ':' | '=')
            }) {
                return true;
            }
        }
    }
    false
}

/// Names a page may use to declare its static parameter set.
///
/// `generateStaticParams` is Next.js's name for the same export, with the same
/// contract: return the parameter objects to pre-render. Accepting it costs
/// nothing and removes a silent failure — a page brought over from Next.js
/// declared its parameters, this file did not recognise the name, and the route
/// was served dynamically with no diagnostic anywhere. Mirrored by the resolver
/// in `packages/ruvyxa/runtime/worker-pool.mjs`, which has to read the same
/// names or discovery and execution disagree.
pub const STATIC_PARAMS_EXPORTS: [&str; 3] =
    ["getStaticParams", "staticParams", "generateStaticParams"];

fn has_static_params_export(code: &str) -> bool {
    STATIC_PARAMS_EXPORTS
        .iter()
        .any(|name| has_export_function(code, name))
}

/// Refuse an interception whose target no route serves.
///
/// An interception is an overlay on an ordinary route: a hard load, a refresh,
/// or a shared link has to render the real page, so the real page has to exist.
/// Without this check `app/feed/@modal/(.)phto/page.tsx` — one typo — would be
/// a modal that never opens and a URL that 404s, with nothing said at build
/// time.
///
/// Targets are compared by match shape rather than by text, so
/// `(.)photo/[id]` and `app/feed/photo/[photoId]` are the same URL to this
/// check, exactly as they are to the router.
fn detect_unreachable_intercepts(routes: &[RouteEntry]) -> Result<()> {
    let pages = routes
        .iter()
        .filter(|route| route.kind == RouteKind::Page)
        .map(|route| route_match_shape(&route.path))
        .collect::<BTreeSet<_>>();

    // One route can carry the same interception as another; report the first
    // by sorted file path so two machines name the same file.
    let mut unreachable = routes
        .iter()
        .flat_map(|route| route.intercepts.iter())
        .filter(|intercept| !pages.contains(&route_match_shape(&intercept.target)))
        .collect::<Vec<_>>();
    unreachable
        .sort_by(|left, right| (&left.file, &left.target).cmp(&(&right.file, &right.target)));
    unreachable.dedup_by(|left, right| left.file == right.file && left.target == right.target);

    let Some(intercept) = unreachable.into_iter().next() else {
        return Ok(());
    };
    Err(Diagnostic::new("RUV1006", "Intercepting route has no route to intercept")
        .explain(format!(
            "`{}` intercepts `{}`, and no page answers that URL. An interception is an overlay: a hard load or a shared link still has to render the real page.",
            intercept.marker, intercept.target
        ))
        .at_file(&intercept.file)
        .suggest(format!(
            "Add the page the interception stands in for, at the route `{}`, or correct the folder name.",
            intercept.target
        ))
        .into())
}

/// Refuse the two combinations where `serverComponents` would silently do nothing.
///
/// Both are opt-ins that read as working. A page that is itself `'use client'`
/// has no server half to render, so the export changes nothing and the author
/// is left believing their data fetching moved off the browser. An interception
/// is resolved by the client router from a registry the server-components
/// browser entry does not build, so the modal simply never opens.
///
/// Refusing at discovery rather than at render is deliberate: both failures are
/// invisible in a working page, and a diagnostic that only fires on the request
/// path would not fire during `ruvyxa check` at all.
fn detect_server_component_conflicts(routes: &[RouteEntry]) -> Result<()> {
    for route in routes.iter().filter(|route| route.render.server_components) {
        if route.render.strategy == RenderStrategy::Csr {
            return Err(Diagnostic::new(
                "RUV1011",
                "Page declares both `use client` and server components",
            )
            .explain(
                "A `'use client'` page runs entirely in the browser, so there is no server graph for `export const serverComponents = true` to render. One of the two is not doing what it says.",
            )
            .at_file(&route.file)
            .suggest(
                "Remove the `'use client'` directive from the page and move the interactive parts into their own `'use client'` components, or drop the `serverComponents` export.",
            )
            .into());
        }
        if route.render.strategy == RenderStrategy::Ppr {
            return Err(Diagnostic::new(
                "RUV1011",
                "Server components route also opts into partial pre-rendering",
            )
            .explain(
                "Partial pre-rendering streams a static shell and fills its holes later, through a render entry the server-components pipeline does not build. The route would be pre-rendered as an ordinary shell and the `serverComponents` export would do nothing.",
            )
            .at_file(&route.file)
            .suggest("Remove `export const ppr = true` or `export const serverComponents = true` from this page.")
            .into());
        }
        if let Some(intercept) = route.intercepts.first() {
            return Err(Diagnostic::new(
                "RUV1011",
                "Server components route carries an intercepting route",
            )
            .explain(format!(
                "`{}` intercepts `{}`, and an interception is resolved by the client router from a registry a server-components route does not publish. The overlay would never open.",
                intercept.marker, intercept.target
            ))
            .at_file(&route.file)
            .suggest(
                "Drop `export const serverComponents = true` from this route, or move the interception to a route that does not use server components.",
            )
            .into());
        }
    }
    Ok(())
}

fn detect_conflicts(routes: &[RouteEntry]) -> Result<()> {
    let mut seen = BTreeMap::<String, &RouteEntry>::new();

    for route in routes {
        let key = route_match_shape(&route.path);
        if let Some(previous) = seen.insert(key, route) {
            let mut diagnostic = Diagnostic::new("RUV1003", "Conflicting route paths")
                .explain(format!(
                    "{} and {} resolve to the same URL match shape. Route parameter names and page/API kinds do not make overlapping routes distinct.",
                    previous.file.display(),
                    route.file.display()
                ))
                .at_file(&route.file)
                .suggest("Keep only one route for this URL shape or move one route to a distinct URL segment.");
            diagnostic.affected_routes = vec![previous.id.clone(), route.id.clone()];
            return Err(diagnostic.into());
        }
    }

    Ok(())
}

fn route_match_shape(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with("[[...") && segment.ends_with("]]") {
                "*?"
            } else if segment.starts_with("[...") && segment.ends_with(']') {
                "*"
            } else if segment.starts_with('[') && segment.ends_with(']') {
                ":"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn hydration_of(source: &str) -> HydrationMode {
        parse_hydration_mode(source, &code_without_strings_and_comments(source))
    }

    /// The runtime a source declares, read the way the route walk reads it.
    fn runtime_of(source: &str) -> Option<RuntimeTarget> {
        export_const_value(
            source,
            &code_without_strings_and_comments(source),
            "runtime",
        )
        .map_or(Some(RuntimeTarget::Node), |raw| {
            runtime_target_from_value(&raw)
        })
    }

    fn edge_fixture() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/edge-runtime-conformance.json"
        ))
        .expect("the edge runtime fixture parses")
    }

    /// Every spelling of `export const runtime` the shared table lists.
    ///
    /// The declaration decides where a route is allowed to run and what it may
    /// import, so a spelling read differently here than by the manifest readers
    /// moves a route with nothing said.
    #[test]
    fn route_runtime_declarations_match_the_shared_conformance_table() {
        let fixture = edge_fixture();
        let declaration = &fixture["declaration"];
        assert_eq!(declaration["export"], "runtime");

        let cases = declaration["values"].as_array().expect("values");
        assert!(!cases.is_empty(), "the table must carry cases");
        for case in cases {
            let source = case["source"].as_str().expect("source");
            let expected = case["runtime"].as_str().expect("runtime");
            let actual = match runtime_of(source) {
                Some(RuntimeTarget::Edge) => "edge",
                Some(RuntimeTarget::Node) => "node",
                Some(RuntimeTarget::Static) => "static",
                None => "rejected",
            };
            assert_eq!(actual, expected, "{source}");
        }

        for source in declaration["rejected"].as_array().expect("rejected") {
            let source = source.as_str().expect("rejected source");
            assert_eq!(
                runtime_of(source),
                None,
                "{source} names no runtime this framework has, and defaulting it \
                 to Node would place the route somewhere the author did not ask for"
            );
        }
    }

    /// The built-in list this crate refuses is the list the table publishes.
    #[test]
    fn edge_unavailable_builtins_match_the_shared_conformance_table() {
        let fixture = edge_fixture();

        let expected: Vec<&str> = fixture["unavailableOnEdge"]
            .as_array()
            .expect("unavailableOnEdge")
            .iter()
            .map(|name| name.as_str().expect("name"))
            .collect();
        assert_eq!(EDGE_UNAVAILABLE_BUILTINS, expected.as_slice());

        // Bare, prefixed, and sub-path spellings are one answer.
        assert_eq!(edge_forbidden_builtin("fs"), Some("fs"));
        assert_eq!(edge_forbidden_builtin("node:fs"), Some("fs"));
        assert_eq!(edge_forbidden_builtin("node:fs/promises"), Some("fs"));
        assert_eq!(edge_forbidden_builtin("fs/promises"), Some("fs"));

        // Everything the table calls available must pass, under both spellings:
        // a false refusal costs more than a missing one, because the missing one
        // still fails at deploy on the platform that knows its own surface.
        for name in fixture["availableOnEdge"]
            .as_array()
            .expect("availableOnEdge")
        {
            let name = name.as_str().expect("name");
            assert_eq!(edge_forbidden_builtin(name), None, "{name}");
            assert_eq!(
                edge_forbidden_builtin(&format!("node:{name}")),
                None,
                "{name}"
            );
        }

        // A package whose name merely starts with a built-in's is not that
        // built-in: `os-locale` and `fs-extra` are ordinary dependencies.
        for specifier in ["os-locale", "fs-extra", "net-utils", "@scope/vm"] {
            assert_eq!(edge_forbidden_builtin(specifier), None, "{specifier}");
        }
    }

    /// A route export only counts where it is real code.
    ///
    /// These scanners read the raw source, so an `export const hydrate = false`
    /// sitting in a block comment or quoted inside a documentation snippet
    /// switched off the surrounding page's hydration. The same shape already
    /// broke the linker once, which is why `masked_code` exists.
    #[test]
    fn a_route_export_inside_a_comment_or_literal_is_not_an_export() {
        assert_eq!(
            hydration_of("export const hydrate = false\n"),
            HydrationMode::None,
            "the real declaration must still be read"
        );
        assert_eq!(
            hydration_of("/*\nexport const hydrate = false\n*/\nexport default function P() {}\n"),
            HydrationMode::Load,
            "a commented-out opt-out must not disable hydration"
        );
        assert_eq!(
            hydration_of("const docs = `\nexport const hydrate = false\n`;\n"),
            HydrationMode::Load,
            "a code sample inside a template literal is text, not an export"
        );

        let quoted_ppr = "const docs = `export const ppr = true`;\n";
        assert!(
            !has_export_const_bool(
                quoted_ppr,
                &code_without_strings_and_comments(quoted_ppr),
                "ppr",
                true
            ),
            "a quoted opt-in must not switch the route to PPR"
        );
    }

    /// A TypeScript annotation between the name and `=` is ordinary TS, and
    /// `has_export_function` beside these already tolerated one. These did not,
    /// so an annotated opt-in was read as absent and the route silently fell
    /// back to a different rendering strategy.
    #[test]
    fn an_annotated_route_export_is_still_the_export() {
        assert_eq!(
            hydration_of("export const hydrate: HydrationMode = false\n"),
            HydrationMode::None
        );
        assert_eq!(
            hydration_of("export const hydrate: 'idle' | 'visible' = 'idle'\n"),
            HydrationMode::Idle,
            "a union type contains no assignment; the value after it does"
        );

        let ppr = "export const ppr: boolean = true\n";
        assert!(has_export_const_bool(
            ppr,
            &code_without_strings_and_comments(ppr),
            "ppr",
            true
        ));

        let revalidate = "export const revalidate: number = 3600\n";
        assert_eq!(
            parse_export_const_number(
                revalidate,
                &code_without_strings_and_comments(revalidate),
                "revalidate"
            ),
            Some(3600)
        );

        // An arrow inside the annotation is not the assignment.
        let typed_arrow =
            "export const revalidate: (() => number) extends never ? 1 : number = 60\n";
        assert_eq!(
            parse_export_const_number(
                typed_arrow,
                &code_without_strings_and_comments(typed_arrow),
                "revalidate"
            ),
            Some(60)
        );
    }

    /// A longer identifier that merely starts with the same characters is a
    /// different export, and a trailing comment is not part of the value.
    #[test]
    fn route_export_matching_stops_at_the_identifier_and_the_comment() {
        assert_eq!(
            hydration_of("export const hydrateAll = false\n"),
            HydrationMode::Load,
            "`hydrateAll` is not `hydrate`"
        );
        assert_eq!(
            hydration_of("export const hydrate = false // keep this page static\n"),
            HydrationMode::None
        );

        let commented = "export const revalidate = 120 // refresh every two minutes\n";
        assert_eq!(
            parse_export_const_number(
                commented,
                &code_without_strings_and_comments(commented),
                "revalidate"
            ),
            Some(120)
        );
    }

    /// Scanner tests assert on source text directly. Production reads both
    /// facts off one cached `ModuleAst`; these shadow the module-level helpers
    /// so the assertions stay about the scanner rather than the cache.
    fn private_env_reads(source: &str) -> Vec<String> {
        let ast = ruvyxa_bundler::ast::parse_module(source);
        super::private_env_reads(&ast).map(str::to_owned).collect()
    }

    fn import_specifiers(source: &str) -> Vec<String> {
        ruvyxa_bundler::ast::parse_module(source).import_specifiers()
    }

    /// `check` must allow exactly what `build` allows. A local copy of the rule
    /// had lost the `NODE_ENV` exemption, so the most ordinary line in a React
    /// client component raised RUV1008 while the same file built cleanly.
    #[test]
    fn env_boundary_rule_matches_the_bundler_that_compiles_the_bundle() {
        assert!(
            private_env_reads("const dev = process.env.NODE_ENV !== 'production'").is_empty(),
            "NODE_ENV is substituted at build time and must not be reported as a leak"
        );
        assert!(
            private_env_reads("const url = process.env.RUVYXA_PUBLIC_API_URL").is_empty(),
            "RUVYXA_PUBLIC_* is public by contract"
        );
        assert_eq!(
            private_env_reads("const secret = process.env.DATABASE_URL"),
            vec!["DATABASE_URL".to_string()],
            "a genuinely private read must still be reported"
        );

        // Same names, judged by the bundler's own predicate: the two must agree
        // name for name, or `check` and `build` have drifted again.
        for name in [
            "NODE_ENV",
            "RUVYXA_PUBLIC_API_URL",
            "DATABASE_URL",
            "API_KEY",
        ] {
            let source = format!("const value = process.env.{name}");
            assert_eq!(
                private_env_reads(&source).is_empty(),
                !ruvyxa_bundler::boundary::env_read_is_private(name),
                "check and build disagree about `{name}`"
            );
        }
    }

    #[test]
    fn discovers_static_nested_and_dynamic_pages() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("about")).unwrap();
        fs::create_dir_all(app.join("blog/[slug]")).unwrap();
        fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
        fs::write(
            app.join("about/page.tsx"),
            "export default function About() {}",
        )
        .unwrap();
        fs::write(
            app.join("blog/[slug]/page.tsx"),
            "export default function Post() {}",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let paths = manifest
            .routes
            .iter()
            .map(|route| route.path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["/", "/about", "/blog/[slug]"]);
    }

    #[test]
    fn discovers_markdown_and_mdx_pages_without_default_export_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("docs")).unwrap();
        fs::write(app.join("page.md"), "# Home").unwrap();
        fs::write(
            app.join("docs/page.mdx"),
            "# Docs\n\n<strong>Built in</strong>",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();
        assert_eq!(manifest.routes.len(), 2);
        assert!(report.diagnostics.is_empty());
        assert!(
            manifest
                .routes
                .iter()
                .all(|route| route.render.strategy == RenderStrategy::Ssg)
        );
    }

    #[test]
    fn markdown_code_examples_do_not_create_graph_edges() {
        // A fenced example is display text. It used to reach the edge walk
        // unmasked — every other reader masked it first — so a documented
        // `import './config'` pulled a real module into the page's client
        // graph and raised boundary diagnostics against code the page never
        // runs.
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("config.ts"),
            "export const url = process.env.DATABASE_URL;\n",
        )
        .unwrap();
        fs::write(
            app.join("page.md"),
            "# Guide\n\nConfigure the database:\n\n```ts\nimport './config';\n```\n",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();

        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
        assert_eq!(
            report.client_modules, 1,
            "only the page itself is reachable"
        );
    }

    #[test]
    fn supports_catch_all_optional_catch_all_and_route_groups() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("docs/[...slug]")).unwrap();
        fs::create_dir_all(app.join("shop/[[...category]]")).unwrap();
        fs::create_dir_all(app.join("(marketing)/pricing")).unwrap();
        fs::write(app.join("docs/[...slug]/page.tsx"), "").unwrap();
        fs::write(app.join("shop/[[...category]]/page.tsx"), "").unwrap();
        fs::write(app.join("(marketing)/pricing/page.tsx"), "").unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let paths = manifest
            .routes
            .iter()
            .map(|route| route.path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec!["/docs/[...slug]", "/pricing", "/shop/[[...category]]"]
        );
    }

    #[test]
    fn rejects_non_next_optional_segments_and_non_terminal_catch_all() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("shop/[[category]]")).unwrap();
        fs::write(app.join("shop/[[category]]/page.tsx"), "").unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        assert!(error.to_string().contains("RUV1002"));

        fs::remove_dir_all(app.join("shop")).unwrap();
        fs::create_dir_all(app.join("docs/[...slug]/edit")).unwrap();
        fs::write(app.join("docs/[...slug]/edit/page.tsx"), "").unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        assert!(error.to_string().contains("RUV1002"));
    }

    #[test]
    fn private_folders_and_parallel_slots_do_not_create_routes() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("_private")).unwrap();
        fs::create_dir_all(app.join("@modal")).unwrap();
        fs::write(app.join("page.tsx"), "").unwrap();
        fs::write(app.join("_private/page.tsx"), "").unwrap();
        fs::write(app.join("@modal/page.tsx"), "").unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        assert_eq!(manifest.routes.len(), 1);
        assert_eq!(manifest.routes[0].path, "/");
    }

    /// Both hosts discover the same interceptions from the same tree.
    ///
    /// The JavaScript half is
    /// `tests/packages/ruvyxa/intercepting-routes-contract.test.mjs` over
    /// `collectIntercepts` in `packages/ruvyxa/runtime/worker-pool.mjs`, which
    /// is what `ruvyxa dev` builds its client entries from. An interception one
    /// host composes and the other does not is a modal that opens in
    /// production and does nothing locally.
    #[test]
    fn interception_discovery_matches_the_shared_conformance_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/intercepting-route-conformance.json"
        ))
        .unwrap();

        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let temp = tempfile::tempdir().unwrap();
            let app = temp.path().join("app");
            for file in case["tree"].as_array().unwrap() {
                let path = app.join(file.as_str().unwrap());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, "export default function Fixture() {}").unwrap();
            }
            let route_dir = match case["routeDir"].as_str().unwrap() {
                "" => app.clone(),
                relative => app.join(relative),
            };

            let actual = route_intercepts(&app, &route_dir)
                .unwrap_or_else(|error| panic!("{name} failed discovery: {error}"))
                .into_iter()
                .map(|intercept| {
                    serde_json::json!({
                        "level": intercept.level,
                        "name": intercept.name,
                        "target": intercept.target,
                        "file": intercept
                            .file
                            .strip_prefix(&app)
                            .unwrap_or(&intercept.file)
                            .display()
                            .to_string()
                            .replace('\\', "/"),
                    })
                })
                .collect::<Vec<_>>();
            assert_eq!(
                &serde_json::Value::Array(actual),
                &case["intercepts"],
                "{name} disagrees with the shared fixture"
            );
        }
    }

    /// Each marker resolves to the URL it actually covers.
    ///
    /// The target comes from the *level* the slot sits on, not from the slot
    /// folder, because a slot contributes no URL segment of its own. Getting
    /// that wrong is invisible until a modal silently never opens.
    #[test]
    fn every_marker_resolves_to_the_url_it_covers() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        // The ordinary routes the interceptions stand in for.
        for real in ["photo", "feed/photo", "feed/albums/photo"] {
            fs::create_dir_all(app.join(real)).unwrap();
            fs::write(
                app.join(real).join("page.tsx"),
                "export default function Real() {}",
            )
            .unwrap();
        }
        fs::create_dir_all(app.join("feed/albums")).unwrap();
        fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
        fs::write(
            app.join("feed/albums/page.tsx"),
            "export default function Albums() {}",
        )
        .unwrap();
        fs::write(
            app.join("feed/layout.tsx"),
            "export default function L() {}",
        )
        .unwrap();

        // One slot on `app/feed/albums`, so every climb has somewhere to go.
        for (folder, _expected) in [
            ("(.)photo", "/feed/albums/photo"),
            ("(..)photo", "/feed/photo"),
            ("(..)(..)photo", "/photo"),
            ("(...)photo", "/photo"),
        ] {
            fs::create_dir_all(app.join("feed/albums/@modal").join(folder)).unwrap();
            fs::write(
                app.join("feed/albums/@modal").join(folder).join("page.tsx"),
                "export default function Modal() {}",
            )
            .unwrap();
        }
        fs::create_dir_all(app.join("feed/albums/photo")).unwrap();
        fs::write(
            app.join("feed/albums/photo/page.tsx"),
            "export default function Real() {}",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let albums = manifest
            .routes
            .iter()
            .find(|route| route.path == "/feed/albums")
            .expect("the level's own route is discovered");
        let mut targets = albums
            .intercepts
            .iter()
            .map(|intercept| (intercept.marker.as_str(), intercept.target.as_str()))
            .collect::<Vec<_>>();
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec![
                ("(.)", "/feed/albums/photo"),
                ("(..)", "/feed/photo"),
                ("(..)(..)", "/photo"),
                ("(...)", "/photo"),
            ]
        );
        assert!(
            albums
                .intercepts
                .iter()
                .all(|intercept| intercept.name == "modal"),
            "every interception names the slot it renders into"
        );
    }

    /// An interception is carried by the routes that can show it, not by the
    /// route it covers.
    ///
    /// The intercepted route keeps its own entry untouched, which is what makes
    /// a hard load render the real page instead of the overlay.
    #[test]
    fn an_interception_is_carried_by_the_routes_below_its_level() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("feed/@modal/(.)photo")).unwrap();
        fs::create_dir_all(app.join("feed/photo")).unwrap();
        fs::create_dir_all(app.join("elsewhere")).unwrap();
        fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
        fs::write(
            app.join("feed/layout.tsx"),
            "export default function L() {}",
        )
        .unwrap();
        fs::write(
            app.join("feed/page.tsx"),
            "export default function Feed() {}",
        )
        .unwrap();
        fs::write(
            app.join("feed/photo/page.tsx"),
            "export default function Photo() {}",
        )
        .unwrap();
        fs::write(
            app.join("feed/@modal/(.)photo/page.tsx"),
            "export default function Modal() {}",
        )
        .unwrap();
        fs::write(
            app.join("elsewhere/page.tsx"),
            "export default function Elsewhere() {}",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let by_path = |path: &str| {
            manifest
                .routes
                .iter()
                .find(|route| route.path == path)
                .unwrap_or_else(|| panic!("{path} must be discovered"))
        };

        assert_eq!(by_path("/feed").intercepts.len(), 1, "the level itself");
        assert_eq!(
            by_path("/feed/photo").intercepts.len(),
            1,
            "a route below the level composes the same layout"
        );
        assert!(
            by_path("/").intercepts.is_empty(),
            "a route above the level has no layout to render it into"
        );
        assert!(
            by_path("/elsewhere").intercepts.is_empty(),
            "a sibling route never composes that layout"
        );
    }

    /// An interception with no real route behind it fails the build.
    #[test]
    fn rejects_an_interception_whose_target_no_route_serves() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("feed/@modal/(.)phto")).unwrap();
        fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
        fs::write(
            app.join("feed/layout.tsx"),
            "export default function L() {}",
        )
        .unwrap();
        fs::write(
            app.join("feed/page.tsx"),
            "export default function Feed() {}",
        )
        .unwrap();
        fs::write(
            app.join("feed/@modal/(.)phto/page.tsx"),
            "export default function Modal() {}",
        )
        .unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("RUV1006"), "{text}");
        assert!(text.contains("/feed/phto"), "{text}");
    }

    /// A marker cannot climb above the app root.
    #[test]
    fn rejects_an_interception_that_climbs_past_the_app_root() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("@modal/(..)photo")).unwrap();
        fs::create_dir_all(app.join("photo")).unwrap();
        fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
        fs::write(app.join("photo/page.tsx"), "export default function P() {}").unwrap();
        fs::write(
            app.join("@modal/(..)photo/page.tsx"),
            "export default function Modal() {}",
        )
        .unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        assert!(error.to_string().contains("RUV1006"), "{error}");
    }

    /// Every intercepting-route marker is refused, and refused as itself.
    ///
    /// Before this, none of the four was stripped or reported: the route-group
    /// branch needs a trailing `)`, so the folder became a literal URL segment
    /// and published a page the author wrote as an interception.
    #[test]
    fn rejects_every_intercepting_route_marker() {
        for (folder, marker) in [
            ("(.)photo", "(.)"),
            ("(..)photo", "(..)"),
            ("(..)(..)photo", "(..)(..)"),
            ("(...)photo", "(...)"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let app = temp.path().join("app");
            fs::create_dir_all(app.join("feed").join(folder)).unwrap();
            fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
            fs::write(
                app.join("feed").join(folder).join("page.tsx"),
                "export default function Photo() {}",
            )
            .unwrap();

            let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
            let text = error.to_string();
            assert!(text.contains("RUV1005"), "{folder} was accepted: {text}");
            assert!(
                text.contains(marker),
                "{folder} was reported as some other convention: {text}"
            );
        }
    }

    /// A marker inside a parallel-route slot is an interception, not an error.
    ///
    /// This is the shape the convention exists for — `@modal/(.)photo` is the
    /// canonical Next.js modal — and it is the one place the folder has
    /// somewhere to render into. It used to be rejected along with every other
    /// marker, and before that it silently matched no URL and rendered nothing.
    #[test]
    fn an_intercepting_route_inside_a_parallel_slot_is_resolved() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("@modal/(.)photo")).unwrap();
        fs::create_dir_all(app.join("photo")).unwrap();
        fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
        fs::write(app.join("layout.tsx"), "export default function L() {}").unwrap();
        fs::write(
            app.join("photo/page.tsx"),
            "export default function Photo() {}",
        )
        .unwrap();
        fs::write(
            app.join("@modal/(.)photo/page.tsx"),
            "export default function Modal() {}",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let home = manifest
            .routes
            .iter()
            .find(|route| route.path == "/")
            .expect("the root route is discovered");
        assert_eq!(home.intercepts.len(), 1);
        assert_eq!(home.intercepts[0].target, "/photo");
        assert_eq!(home.intercepts[0].name, "modal");
        assert_eq!(home.intercepts[0].marker, "(.)");
        // The slot folder still contributes no URL of its own.
        let paths = manifest
            .routes
            .iter()
            .map(|route| route.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/", "/photo"]);
    }

    /// The marker scan must not swallow the conventions beside it.
    ///
    /// `(marketing)` opens with `(` and `@modal` is a slot; both were working
    /// before the scan existed and a prefix test that is too loose would take
    /// them away with no test noticing.
    #[test]
    fn route_groups_slots_and_private_folders_survive_the_intercept_scan() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("(marketing)/pricing")).unwrap();
        fs::create_dir_all(app.join("@modal")).unwrap();
        fs::create_dir_all(app.join("_drafts/(.)photo")).unwrap();
        fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
        fs::write(
            app.join("(marketing)/pricing/page.tsx"),
            "export default function Pricing() {}",
        )
        .unwrap();
        fs::write(
            app.join("@modal/default.tsx"),
            "export default function M() {}",
        )
        .unwrap();
        fs::write(
            app.join("_drafts/(.)photo/page.tsx"),
            "export default function Draft() {}",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let paths = manifest
            .routes
            .iter()
            .map(|route| route.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/", "/pricing"]);
    }

    #[test]
    fn detects_duplicate_page_routes() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("pricing")).unwrap();
        fs::create_dir_all(app.join("(marketing)/pricing")).unwrap();
        fs::write(app.join("pricing/page.tsx"), "").unwrap();
        fs::write(app.join("(marketing)/pricing/page.tsx"), "").unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        assert!(error.to_string().contains("RUV1003"));
    }

    #[test]
    fn detects_routes_with_equivalent_dynamic_shapes() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("blog/[slug]")).unwrap();
        fs::create_dir_all(app.join("blog/[id]")).unwrap();
        fs::write(app.join("blog/[slug]/page.tsx"), "").unwrap();
        fs::write(app.join("blog/[id]/page.tsx"), "").unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        assert!(error.to_string().contains("RUV1003"));
    }

    #[test]
    fn rejects_page_and_route_handler_at_the_same_segment() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app/api");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("page.tsx"), "").unwrap();
        fs::write(app.join("route.ts"), "").unwrap();

        let error = discover_routes(DiscoverOptions::new(temp.path().join("app"))).unwrap_err();
        assert!(error.to_string().contains("RUV1003"));
    }

    /// The opt-in is read the way every other route export is: from masked
    /// source. Reading raw text would let a commented-out line, or the same
    /// words inside a template literal, silently change a route's rendering
    /// pipeline — which is how `hydrate` was misread twice before.
    #[test]
    fn reads_the_server_components_opt_in_from_code_only() {
        let cases = [
            ("export const serverComponents = true\n", true),
            ("export const serverComponents: boolean = true\n", true),
            ("export const serverComponents = false\n", false),
            ("// export const serverComponents = true\n", false),
            ("/*\nexport const serverComponents = true\n*/\n", false),
            (
                "const doc = `\nexport const serverComponents = true\n`;\n",
                false,
            ),
            ("export const serverComponentsAll = true\n", false),
            ("", false),
        ];

        for (prologue, expected) in cases {
            let temp = tempfile::tempdir().unwrap();
            let app = temp.path().join("app");
            fs::create_dir_all(&app).unwrap();
            fs::write(
                app.join("page.tsx"),
                format!("{prologue}export default function Page() {{ return null }}\n"),
            )
            .unwrap();

            let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
            assert_eq!(
                manifest.routes[0].render.server_components, expected,
                "{prologue:?}"
            );
        }
    }

    /// The opt-in is orthogonal to the strategy, not a variant of it: a
    /// server-components route can still revalidate on an interval.
    #[test]
    fn server_components_compose_with_a_rendering_strategy() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "export const serverComponents = true\nexport const revalidate = 60\nexport default function Page() { return null }\n",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        assert!(manifest.routes[0].render.server_components);
        assert_eq!(manifest.routes[0].render.strategy, RenderStrategy::Isr);
        assert_eq!(manifest.routes[0].render.revalidate, Some(60));
    }

    /// The pairing that produces a hydration mismatch nobody is told about, and
    /// the two that do not.
    ///
    /// A plugin transform is applied by the browser compile alone. Rendering
    /// the same module on the server and then hydrating against the rewritten
    /// version makes React throw the server markup away (#418) — which shows up
    /// as a flicker, never as a failure. A `'use client'` route has no server
    /// document to disagree with, and a route that ships no bundle never
    /// hydrates, so neither is at risk.
    #[test]
    fn only_a_route_that_both_renders_and_hydrates_diverges_on_a_plugin_transform() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        for route in ["ssr", "csr", "static"] {
            fs::create_dir_all(app.join(route)).unwrap();
        }
        fs::write(
            app.join("marker.ts"),
            "export const MARKER = 'untouched'
",
        )
        .unwrap();
        fs::write(
            app.join("ssr/page.tsx"),
            "import { MARKER } from '../marker'
export default function Page() { return MARKER }
",
        )
        .unwrap();
        fs::write(
            app.join("csr/page.tsx"),
            "'use client'
import { MARKER } from '../marker'
export default function Page() { return MARKER }
",
        )
        .unwrap();
        fs::write(
            app.join("static/page.tsx"),
            "export const hydrate = false
import { MARKER } from '../marker'
export default function Page() { return MARKER }
",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let transformed = BTreeSet::from([normalized_canonical_path(&app.join("marker.ts"))]);
        let at_risk = hydrated_routes_reaching(&manifest, &transformed);

        assert_eq!(
            at_risk
                .iter()
                .map(|(route, _)| route.as_str())
                .collect::<Vec<_>>(),
            vec!["/ssr"],
            "only the route that renders on the server and hydrates can disagree with itself"
        );

        // The shared table, replayed against those three routes. It named the
        // rule and nothing checked it, which is the state a fixture exists to
        // avoid.
        //
        // The rule has two halves and they live apart on purpose. Whether the
        // plugin really produces different text for the two lanes is answered
        // by asking the plugin (`transform_differs_by_environment` in the CLI);
        // whether anything that renders on the server *and* hydrates can reach
        // the module is this function's half. A build warns only when both say
        // yes, which is what the `expect` column describes.
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/plugin-transform-lane-conformance.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let fixture: serde_json::Value =
            serde_json::from_str(&source).expect("the lane fixture parses");
        let cases = fixture["divergence"]["cases"]
            .as_array()
            .expect("divergence cases");
        assert!(!cases.is_empty(), "the fixture must carry cases");

        for case in cases {
            let renders = case["routeRenders"].as_bool().expect("routeRenders");
            let hydrates = case["routeHydrates"].as_bool().expect("routeHydrates");
            let client_only = case["clientOnly"].as_bool().expect("clientOnly");
            let why = case["why"].as_str().unwrap_or_default();
            let route = match (renders, hydrates) {
                (true, true) => "/ssr",
                (false, true) => "/csr",
                (true, false) => "/static",
                (false, false) => continue,
            };

            assert_eq!(
                at_risk.iter().any(|(found, _)| found == route),
                renders && hydrates,
                "{route}: the graph half is reachability alone — {why}"
            );
            assert_eq!(
                case["expect"].as_str().expect("expect") == "diverge",
                client_only && renders && hydrates,
                "{route}: a build warns only when both halves say yes — {why}"
            );
        }
    }

    /// The common case has to stay free: a build with no plugin transforms must
    /// not walk the graph at all.
    #[test]
    fn no_transformed_modules_asks_no_questions() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "export default function Page() { return null }
",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        assert!(hydrated_routes_reaching(&manifest, &BTreeSet::new()).is_empty());
    }

    /// A bundle that hydrates nothing is invisible in every other report: it is
    /// real, referenced, and correct, so no check flags it and the page just
    /// downloads a few hundred kilobytes of React it never uses.
    ///
    /// The signal has to be the boundary walk, not the route's declared client
    /// modules: `client_modules` holds a sibling `client.tsx` by convention and
    /// is empty for a route whose island is any other file, so a check written
    /// against it would tell an interactive page to switch its JavaScript off.
    #[test]
    fn reports_a_server_components_route_whose_bundle_hydrates_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("static")).unwrap();
        fs::create_dir_all(app.join("island")).unwrap();
        fs::write(
            app.join("static/page.tsx"),
            "export const serverComponents = true
export default function Page() { return null }
",
        )
        .unwrap();
        fs::write(
            app.join("island/counter.tsx"),
            "'use client'
export default function Counter() { return null }
",
        )
        .unwrap();
        fs::write(
            app.join("island/page.tsx"),
            "export const serverComponents = true
import Counter from './counter'
export default function Page() { return Counter }
",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();

        assert_eq!(
            report.inert_hydration_routes,
            vec!["/static".to_string()],
            "only the route that reaches no client module ships a bundle for nothing"
        );
    }

    /// A `'use client'` page has no server half, so the export would do nothing
    /// while reading as though it had moved the page's work off the browser.
    #[test]
    fn rejects_server_components_on_a_use_client_page() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "\"use client\"\nexport const serverComponents = true\nexport default function Page() { return null }\n",
        )
        .unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("RUV1011"), "{text}");
        assert!(text.contains("use client"), "{text}");
    }

    /// Partial pre-rendering streams a shell through an entry the
    /// server-components pipeline does not build.
    #[test]
    fn rejects_server_components_with_partial_prerendering() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "export const ppr = true\nexport const serverComponents = true\nexport default function Page() { return null }\n",
        )
        .unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        assert!(error.to_string().contains("RUV1011"), "{error}");
    }

    /// An interception is matched by the client router from a registry a
    /// server-components browser entry never publishes, so the overlay would
    /// simply never open.
    #[test]
    fn rejects_server_components_on_a_route_carrying_an_interception() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("@modal/(.)photo")).unwrap();
        fs::create_dir_all(app.join("photo")).unwrap();
        fs::write(app.join("layout.tsx"), "export default function L() {}").unwrap();
        fs::write(
            app.join("page.tsx"),
            "export const serverComponents = true\nexport default function Page() { return null }\n",
        )
        .unwrap();
        fs::write(app.join("photo/page.tsx"), "export default function P() {}").unwrap();
        fs::write(
            app.join("@modal/(.)photo/page.tsx"),
            "export default function M() {}",
        )
        .unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("RUV1011"), "{text}");
        assert!(text.contains("/photo"), "{text}");
    }

    #[test]
    fn includes_action_files_as_server_modules() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("todos")).unwrap();
        fs::write(
            app.join("todos/page.tsx"),
            "export default function Todos() {}",
        )
        .unwrap();
        fs::write(app.join("todos/action.ts"), "export const createTodo = {}").unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let route = manifest
            .routes
            .iter()
            .find(|route| route.path == "/todos")
            .unwrap();

        assert_eq!(route.server_modules.len(), 1);
        assert!(route.server_modules[0].ends_with("action.ts"));
    }

    #[test]
    fn classifies_static_pages_without_data_markers_as_ssg() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("static-page")).unwrap();
        fs::write(
            app.join("static-page/page.tsx"),
            r#"
                export default function StaticPage() {
                    return <code>.ruvyxa/prerender/static-page/index.html</code>;
                }
            "#,
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let route = manifest
            .routes
            .iter()
            .find(|route| route.path == "/static-page")
            .unwrap();

        assert_eq!(route.render.strategy, RenderStrategy::Ssg);
        assert!(!route.render.has_static_params);
        assert!(route.render.ships_client_bundle());
    }

    #[test]
    fn hydrate_false_export_opts_pages_out_of_hydration() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("no-js")).unwrap();
        fs::write(
            app.join("no-js/page.tsx"),
            r#"
                export const hydrate = false
                export default function NoJsPage() {
                    return <h1>Content only</h1>;
                }
            "#,
        )
        .unwrap();
        fs::create_dir_all(app.join("csr-page")).unwrap();
        fs::write(
            app.join("csr-page/page.tsx"),
            r#""use client"
                export const hydrate = false
                export default function CsrPage() {
                    return <h1>Client page</h1>;
                }
            "#,
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let no_js = manifest
            .routes
            .iter()
            .find(|route| route.path == "/no-js")
            .unwrap();
        assert_eq!(no_js.render.strategy, RenderStrategy::Ssg);
        assert!(!no_js.render.ships_client_bundle());

        // 'use client' wins: CSR pages cannot opt out of client rendering.
        let csr = manifest
            .routes
            .iter()
            .find(|route| route.path == "/csr-page")
            .unwrap();
        assert_eq!(csr.render.strategy, RenderStrategy::Csr);
        assert!(csr.render.ships_client_bundle());
    }

    #[test]
    fn hydration_string_exports_select_deferred_modes() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        for (segment, declaration) in [
            ("idle", "'idle' as const; // wait for idle"),
            ("visible", "'visible'"),
        ] {
            let route = app.join(segment);
            fs::create_dir_all(&route).unwrap();
            fs::write(
                route.join("page.tsx"),
                format!(
                    "export const hydrate = {declaration};\nexport default function Page() {{ return <main>{segment}</main> }}"
                ),
            )
            .unwrap();
        }

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let idle = manifest
            .routes
            .iter()
            .find(|route| route.path == "/idle")
            .unwrap();
        let visible = manifest
            .routes
            .iter()
            .find(|route| route.path == "/visible")
            .unwrap();

        assert_eq!(idle.render.hydration, HydrationMode::Idle);
        assert_eq!(visible.render.hydration, HydrationMode::Visible);
        assert!(idle.render.ships_client_bundle() && visible.render.ships_client_bundle());
    }

    #[test]
    fn classifies_static_params_shorthand_as_dynamic_ssg() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("articles/[slug]")).unwrap();
        fs::write(
            app.join("articles/[slug]/page.tsx"),
            "export const staticParams = ['one', 'two']; export default function Page() {}",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let route = manifest
            .routes
            .iter()
            .find(|route| route.path == "/articles/[slug]")
            .unwrap();

        assert_eq!(route.render.strategy, RenderStrategy::Ssg);
        assert!(route.render.has_static_params);
    }

    #[test]
    fn does_not_treat_prefixed_static_params_names_as_exports() {
        assert!(!has_static_params_export(
            "export const staticParamsHelper = ['one'];"
        ));
        assert!(!has_static_params_export(
            "export function getStaticParamsHelper() {}"
        ));
    }

    #[test]
    fn keeps_dynamic_and_data_fetching_pages_as_ssr_without_static_params() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("blog/[slug]")).unwrap();
        fs::create_dir_all(app.join("latest")).unwrap();
        fs::write(
            app.join("blog/[slug]/page.tsx"),
            "export default function Post() {}",
        )
        .unwrap();
        fs::write(
            app.join("latest/page.tsx"),
            r#"
                export default async function Latest() {
                    const response = await fetch("https://example.com/news");
                    return <main>{response.status}</main>;
                }
            "#,
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let dynamic = manifest
            .routes
            .iter()
            .find(|route| route.path == "/blog/[slug]")
            .unwrap();
        let latest = manifest
            .routes
            .iter()
            .find(|route| route.path == "/latest")
            .unwrap();

        assert_eq!(dynamic.render.strategy, RenderStrategy::Ssr);
        assert_eq!(latest.render.strategy, RenderStrategy::Ssr);
    }

    #[test]
    fn keeps_pages_with_reachable_data_fetching_as_ssr() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("news")).unwrap();
        fs::write(
            app.join("news/page.tsx"),
            "import { load } from './data'; export default function Page() { return <main>{load}</main>; }",
        )
        .unwrap();
        fs::write(
            app.join("news/data.ts"),
            "export const load = fetch('https://example.com/news');",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        assert_eq!(manifest.routes[0].render.strategy, RenderStrategy::Ssr);
    }

    #[test]
    fn keeps_pages_with_data_fetching_layouts_as_ssr() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("docs")).unwrap();
        fs::write(
            app.join("layout.tsx"),
            "export default function Layout({ children }) { headers(); return children; }",
        )
        .unwrap();
        fs::write(
            app.join("docs/page.tsx"),
            "export default function Page() { return <main>Docs</main>; }",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        assert_eq!(manifest.routes[0].render.strategy, RenderStrategy::Ssr);
    }

    /// A server component is server code, and validation has to know that.
    ///
    /// The client compile of a server-components route stops at `'use client'`:
    /// the page is serialised into a payload and never reaches a browser
    /// bundle. Validating its whole graph as client code refused the two things
    /// a server component exists to do — `import 'server-only'` and reading a
    /// private `process.env` value — while the module they were refused in was
    /// provably absent from every emitted chunk. The boundary below is the
    /// dividing line, and what sits under it is still browser code.
    #[test]
    fn a_server_components_route_is_validated_on_the_server_side_of_its_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app").join("rsc");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            r#"
                import "server-only";
                import Island from "./island";

                export const serverComponents = true;

                export default function Page() {
                    return <main>{process.env.DATABASE_URL}<Island /></main>;
                }
            "#,
        )
        .unwrap();
        fs::write(
            app.join("island.tsx"),
            r#"
                'use client'
                import { secret } from "./browser-secret";
                export default function Island() { return <button>{secret}</button> }
            "#,
        )
        .unwrap();
        fs::write(
            app.join("browser-secret.ts"),
            "export const secret = process.env.DATABASE_URL;\n",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&temp.path().join("app"))).unwrap();
        assert!(
            manifest.routes[0].render.server_components,
            "the fixture must opt into server components"
        );
        let report = validate_app(temp.path(), &manifest).unwrap();
        let codes = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        // The page's own `server-only` import and env read are correct here.
        assert!(
            !codes.contains(&"RUV1007"),
            "server-only is what a server component is for: {:?}",
            report.diagnostics
        );
        // The module under the client boundary is still browser code, and its
        // private env read is still a leak.
        assert_eq!(
            codes.iter().filter(|code| **code == "RUV1008").count(),
            1,
            "only the module below the client boundary leaks: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn validates_client_and_server_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let server = temp.path().join("server");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&server).unwrap();
        fs::write(
            app.join("page.tsx"),
            r#"
                import secret from "../server/secret";

                export default function Home() {
                    return <main>{secret}</main>;
                }
            "#,
        )
        .unwrap();
        fs::write(
            server.join("secret.ts"),
            r#"
                import "server-only";

                export default process.env.DATABASE_URL;
            "#,
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();
        let codes = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"RUV1007"));
        assert!(codes.contains(&"RUV1008"));
        assert!(codes.contains(&"RUV1010"));
    }

    #[test]
    fn validates_implicit_mdx_component_providers_in_the_client_graph() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("docs")).unwrap();
        fs::write(app.join("docs/page.mdx"), "# Documentation").unwrap();
        fs::write(
            app.join("mdx-components.tsx"),
            r#"
                import "server-only";
                export function useMDXComponents(components) {
                    return { ...components, secret: process.env.DATABASE_URL };
                }
            "#,
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();
        let codes = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"RUV1007"), "{codes:?}");
        assert!(codes.contains(&"RUV1008"), "{codes:?}");
    }

    /// Route validation used to test for the literal text `export default`, so
    /// every other valid default-export form was reported as RUV1004 and a
    /// commented-out one silently passed. It now shares the bundler's
    /// comment-aware scanner.
    #[test]
    fn accepts_every_valid_default_export_form_and_still_catches_a_missing_one() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("aliased")).unwrap();
        fs::create_dir_all(app.join("reexported")).unwrap();
        fs::create_dir_all(app.join("commented")).unwrap();

        fs::write(
            app.join("page.tsx"),
            "export default function Home() { return <main /> }",
        )
        .unwrap();
        // Valid: a named binding aliased to `default`.
        fs::write(
            app.join("aliased/page.tsx"),
            "function Page() { return <main /> }\nexport { Page as default }",
        )
        .unwrap();
        // Valid: a namespace re-exported as `default`.
        fs::write(
            app.join("reexported/page.tsx"),
            "export * as default from \"../page\"",
        )
        .unwrap();
        // Invalid: the only occurrence is inside a comment.
        fs::write(
            app.join("commented/page.tsx"),
            "// export default function Page() {}\nexport const title = 'Missing'",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();
        let missing_default = report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "RUV1004")
            .count();

        assert_eq!(
            missing_default,
            1,
            "only the commented-out page lacks a default export, got: {:#?}",
            report
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.code, &diagnostic.title))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn validates_layouts_in_the_client_boundary_graph() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("layout.tsx"),
            r#"
                import "server-only";
                export default function Layout({ children }) {
                    return <main>{process.env.DATABASE_URL}{children}</main>;
                }
            "#,
        )
        .unwrap();
        fs::write(
            app.join("page.tsx"),
            "export default function Page() { return <p>Safe page</p>; }",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();
        let codes = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"RUV1007"), "{codes:?}");
        assert!(codes.contains(&"RUV1008"), "{codes:?}");
    }

    /// The edge cache is shared across walks, so the second walk over a module
    /// finds its edges already memoized. It must still return the full
    /// reachable set — caching the edges, not the reachable set, is what keeps
    /// a warm walk identical to a cold one.
    #[test]
    fn a_warm_edge_cache_returns_the_same_reachable_set() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("blog")).unwrap();
        fs::write(app.join("shared.ts"), "export const shared = 1;").unwrap();
        fs::write(
            app.join("layout.tsx"),
            "import { shared } from './shared'; export default function Layout() { return shared; }",
        )
        .unwrap();
        fs::write(
            app.join("page.tsx"),
            "import { shared } from './shared'; export default function Page() { return shared; }",
        )
        .unwrap();
        fs::write(
            app.join("blog/page.tsx"),
            "import { shared } from '../shared'; export default function Blog() { return shared; }",
        )
        .unwrap();

        let mut cache = ModuleCache::default();
        let cold = collect_relative_graph(&app.join("page.tsx"), &mut cache);
        // `shared.ts` is memoized by now; walking a second entry through it must
        // not short-circuit into a partial graph.
        let warm_blog = collect_relative_graph(&app.join("blog/page.tsx"), &mut cache);
        let warm_layout = collect_relative_graph(&app.join("layout.tsx"), &mut cache);
        // Repeating the first entry on a fully warm cache must be idempotent.
        let warm_repeat = collect_relative_graph(&app.join("page.tsx"), &mut cache);

        assert_eq!(cold, warm_repeat, "a warm walk must match the cold walk");
        let shared = normalized_canonical_path(&app.join("shared.ts"));
        for (label, graph) in [
            ("page", &cold),
            ("blog", &warm_blog),
            ("layout", &warm_layout),
        ] {
            assert_eq!(graph.len(), 2, "{label} graph: {graph:?}");
            assert!(
                graph.contains(&shared),
                "{label} graph lost the shared module"
            );
        }

        // Every entry above still resolves through one read of `shared.ts`.
        assert!(cache.edges.contains_key(&shared));
        assert!(cache.modules.contains_key(&shared));
    }

    #[test]
    fn validates_dynamic_imports_and_requires_in_boundary_graphs() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("api")).unwrap();
        fs::write(
            app.join("page.tsx"),
            "export default async function Page() { return (await import('./secret')).default; }",
        )
        .unwrap();
        fs::write(
            app.join("secret.ts"),
            "import 'server-only'; export default 'secret';",
        )
        .unwrap();
        fs::write(
            app.join("api/route.ts"),
            "const browser = require('./browser'); export const GET = () => browser;",
        )
        .unwrap();
        fs::write(
            app.join("api/browser.ts"),
            "import 'client-only'; export default {}; ",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();
        let codes = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect::<Vec<_>>();

        assert!(codes.contains(&"RUV1007"), "{codes:?}");
        assert!(codes.contains(&"RUV1009"), "{codes:?}");
    }

    #[test]
    fn ignores_doc_snippets_when_validating_client_env_and_imports() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            r#"
                const docs = `
                  import secret from "../server/secret";
                  import "server-only";
                  process.env.DATABASE_URL;
                `;

                export default function Docs() {
                    return <main>{docs}</main>;
                }
            "#,
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();

        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    }

    #[test]
    fn regex_literals_do_not_blank_out_the_rest_of_a_module() {
        // A quote inside a regex character class used to open a string that ran
        // to end-of-file, blanking every later import and env read and silently
        // disabling the boundary rules for the module.
        let names =
            private_env_reads(r#"const re = /['"]/g; const secret = process.env.DATABASE_URL;"#);
        assert_eq!(names, vec!["DATABASE_URL"]);

        let specifiers = import_specifiers(
            r#"const re = /['"]/g;
import 'server-only';
"#,
        );
        assert!(
            specifiers.iter().any(|s| s == "server-only"),
            "{specifiers:?}"
        );
    }

    #[test]
    fn division_is_not_mistaken_for_a_regex_literal() {
        let names = private_env_reads(
            "const ratio = total / count; const secret = process.env.DATABASE_URL;",
        );
        assert_eq!(names, vec!["DATABASE_URL"]);

        let names =
            private_env_reads("const ratio = (a + b) / 2 / 4; const secret = process.env.API_KEY;");
        assert_eq!(names, vec!["API_KEY"]);
    }

    #[test]
    fn detects_literal_bracket_private_env_reads() {
        let names = private_env_reads(
            r#"const secret = process.env["DATABASE_URL"]; const docs = "process.env['EXAMPLE']";"#,
        );

        assert_eq!(names, vec!["DATABASE_URL"]);
    }

    #[test]
    fn detects_private_env_reads_inside_template_interpolations() {
        let names = private_env_reads(
            "const label = `db: ${process.env.DATABASE_URL}`;\nconst doc = `plain process.env.IGNORED text`;",
        );

        assert_eq!(names, vec!["DATABASE_URL"]);

        let nested = private_env_reads(
            "const value = `outer ${cond ? `inner ${process.env.API_SECRET}` : \"\"}`;",
        );
        assert_eq!(nested, vec!["API_SECRET"]);
    }

    #[test]
    fn detects_server_only_imports_inside_template_interpolations() {
        let specifiers = import_specifiers(
            "const loader = `${require(\"server-only\")}`;\nconst doc = `import \"ignored-in-text\";`;",
        );

        assert!(
            specifiers.iter().any(|s| s == "server-only"),
            "{specifiers:?}"
        );
        assert!(
            !specifiers.iter().any(|s| s == "ignored-in-text"),
            "{specifiers:?}"
        );
    }

    #[test]
    fn bracket_env_reads_stay_index_accurate_after_multibyte_text() {
        // Thai comment before the read shifts byte offsets; blanking must be
        // byte-width preserving or the bracket lookup reads garbage.
        let names = private_env_reads(
            "// คอมเมนต์ภาษาไทยก่อนหน้า\nconst secret = process.env[\"DATABASE_URL\"];",
        );

        assert_eq!(names, vec!["DATABASE_URL"]);
    }

    #[test]
    fn allows_server_as_a_url_route_segment() {
        let temp = tempfile::tempdir().unwrap();
        let app_server = temp.path().join("app/server");
        fs::create_dir_all(&app_server).unwrap();
        fs::write(
            app_server.join("page.tsx"),
            "export default function ServerDocs() { return <main /> }",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(temp.path().join("app"))).unwrap();
        let report = validate_app(temp.path(), &manifest).unwrap();

        assert_eq!(manifest.routes[0].path, "/server");
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    }

    #[test]
    fn applies_global_isr_defaults_to_ssr_routes() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "export default async function Page() { return <main>{await fetch('https://example.com')}</main> }",
        )
        .unwrap();

        let manifest = discover_routes(
            DiscoverOptions::new(&app).with_rendering_defaults(Some(RenderStrategy::Isr), Some(90)),
        )
        .unwrap();

        assert_eq!(manifest.routes[0].render.strategy, RenderStrategy::Isr);
        assert_eq!(manifest.routes[0].render.revalidate, Some(90));
    }

    /// Which spelling an import uses is not a rendering decision.
    ///
    /// Rule 5 of `detect_render_strategy` pre-renders a static route whose
    /// reachable graph shows no data fetching, and the walk that produces that
    /// graph followed relative specifiers only. An aliased import produced no
    /// edge at all, so `@/lib/data` and `../../lib/data` — the same file, the
    /// same `fetch` — gave the same page two different strategies, and the
    /// aliased one was baked at build time and never refreshed again.
    ///
    /// A bare package specifier is deliberately still outside the walk, and is
    /// asserted here so the boundary stays a decision rather than an accident:
    /// following `node_modules` would find `fetch(` in almost any dependency
    /// and take automatic pre-rendering away from every page.
    #[test]
    fn an_aliased_import_is_followed_like_a_relative_one() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(app.join("news")).unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["./*"]}}}"#,
        )
        .unwrap();
        fs::write(
            root.join("lib/data.ts"),
            "export const load = fetch('https://example.com/news');",
        )
        .unwrap();

        let strategy_for = |specifier: &str| {
            fs::write(
                app.join("news/page.tsx"),
                format!(
                    "import {{ load }} from '{specifier}'; \
                     export default function Page() {{ return <main>{{load}}</main>; }}"
                ),
            )
            .unwrap();
            discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0]
                .render
                .strategy
        };

        assert_eq!(
            strategy_for("../../lib/data"),
            RenderStrategy::Ssr,
            "a relative import of a fetching module keeps the route dynamic"
        );
        assert_eq!(
            strategy_for("@/lib/data"),
            RenderStrategy::Ssr,
            "the same module through an alias must reach the same conclusion"
        );
        assert_eq!(
            strategy_for("my-data-lib"),
            RenderStrategy::Ssg,
            "a bare package specifier stays outside this walk on purpose"
        );
    }

    /// A marker has to be a whole identifier, not a substring of one.
    ///
    /// `prefetch(` contains `fetch(`, and `prefetch` is an API this framework
    /// ships on `useRouter()` — so a page that warmed one link was read as a
    /// page that fetched data and lost automatic pre-rendering. The reverse
    /// direction is the dangerous one and is asserted alongside it: a member
    /// access is still a call, so `globalThis.fetch(` has to keep counting.
    #[test]
    fn a_data_marker_must_be_its_own_identifier() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();

        let strategy_for = |body: &str| {
            fs::write(
                app.join("page.tsx"),
                format!("export default function Page() {{ {body} return <main/>; }}"),
            )
            .unwrap();
            discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0]
                .render
                .strategy
        };

        for (body, why) in [
            ("router.prefetch('/products');", "prefetch is not fetch"),
            ("const value = parseheaders(raw);", "not headers()"),
            ("const value = readcookies(raw);", "not cookies()"),
            ("const value = mysearchParamsHelper;", "not searchParams"),
        ] {
            assert_eq!(strategy_for(body), RenderStrategy::Ssg, "{body} — {why}");
        }

        for (body, why) in [
            ("fetch('/api/data');", "a bare call"),
            (
                "globalThis.fetch('/api/data');",
                "a member access is still a call",
            ),
            ("await headers();", "the framework accessor"),
            ("const now = Date.now();", "a clock read"),
            (
                "const value = props.searchParams;",
                "a request-dependent prop",
            ),
            ("const key = process.env.SECRET;", "an environment read"),
        ] {
            assert_eq!(strategy_for(body), RenderStrategy::Ssr, "{body} — {why}");
        }
    }

    /// `export const dynamic` decides the strategy, as it does in Next.js.
    ///
    /// A page written against that convention used to be read by nothing here:
    /// `force-dynamic` on an otherwise-static page was discarded, the page was
    /// pre-rendered anyway, and no diagnostic said so. Its precedence matters
    /// too — `force-dynamic` outranks `revalidate`, so a page carrying both is
    /// dynamic rather than ISR.
    #[test]
    fn the_dynamic_route_segment_config_decides_the_strategy() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();

        let strategy_for = |body: &str| {
            fs::write(
                app.join("page.tsx"),
                format!("{body}\nexport default function Page() {{ return <main/>; }}"),
            )
            .unwrap();
            let route = discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0].clone();
            (route.render.strategy, route.render.revalidate)
        };

        assert_eq!(
            strategy_for("").0,
            RenderStrategy::Ssg,
            "a page with no markers is still pre-rendered by default"
        );
        assert_eq!(
            strategy_for("export const dynamic = 'force-dynamic';").0,
            RenderStrategy::Ssr,
            "force-dynamic must take the page off the pre-render path"
        );
        assert_eq!(
            strategy_for("export const dynamic = 'force-static';").0,
            RenderStrategy::Ssg
        );
        assert_eq!(
            strategy_for("export const dynamic = 'error';").0,
            RenderStrategy::Ssg,
            "error is force-static plus a runtime complaint this graph cannot make"
        );
        assert_eq!(
            strategy_for("export const dynamic = 'auto';").0,
            RenderStrategy::Ssg,
            "auto is the default and changes nothing"
        );

        // A page that reads request data is dynamic with or without the export.
        assert_eq!(
            strategy_for("export const dynamic = 'force-dynamic';\nconst now = Date.now();").0,
            RenderStrategy::Ssr
        );
        // Precedence: force-dynamic outranks an ISR opt-in.
        assert_eq!(
            strategy_for("export const dynamic = 'force-dynamic';\nexport const revalidate = 60;"),
            (RenderStrategy::Ssr, None),
            "force-dynamic outranks revalidate, as it does in Next.js"
        );
        // Without it, the ISR opt-in still wins.
        assert_eq!(
            strategy_for("export const revalidate = 60;"),
            (RenderStrategy::Isr, Some(60))
        );
        // A commented-out or quoted occurrence is not the export — the same
        // rule every other route export here is held to.
        assert_eq!(
            strategy_for("// export const dynamic = 'force-dynamic';").0,
            RenderStrategy::Ssg
        );
    }

    /// Next.js's name for the static parameter set is accepted.
    ///
    /// The contract is identical — return the parameter objects to pre-render —
    /// so the only thing the unfamiliar name changed was whether anything
    /// noticed. A dynamic route that declared `generateStaticParams` discovered
    /// as SSR and pre-rendered nothing, silently.
    #[test]
    fn a_dynamic_route_accepts_every_static_params_name() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("blog/[slug]")).unwrap();

        for name in STATIC_PARAMS_EXPORTS {
            fs::write(
                app.join("blog/[slug]/page.tsx"),
                format!(
                    "export async function {name}() {{ return [{{ slug: 'a' }}]; }}\n\
                     export default function Page() {{ return <main/>; }}"
                ),
            )
            .unwrap();
            let route = discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0].clone();
            assert_eq!(
                (route.render.strategy, route.render.has_static_params),
                (RenderStrategy::Ssg, true),
                "{name} declares a static parameter set"
            );
        }

        // A name that is not one of them stays dynamic rather than being
        // guessed at.
        fs::write(
            app.join("blog/[slug]/page.tsx"),
            "export async function makeStaticParams() { return []; }\n\
             export default function Page() { return <main/>; }",
        )
        .unwrap();
        assert_eq!(
            discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0]
                .render
                .strategy,
            RenderStrategy::Ssr
        );
    }

    /// `template.tsx` is discovered on the same chain `layout.tsx` is.
    ///
    /// Kept as its own chain rather than folded into `layout_chain`, because a
    /// level may have either, both, or neither and composition interleaves them
    /// by directory. Merging the two lists here would lose which level each
    /// entry belongs to.
    #[test]
    fn a_template_chain_is_discovered_alongside_the_layout_chain() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("dash/reports")).unwrap();
        fs::write(
            app.join("layout.tsx"),
            "export default function L({children}) { return children }",
        )
        .unwrap();
        fs::write(
            app.join("template.tsx"),
            "export default function T({children}) { return children }",
        )
        .unwrap();
        fs::write(
            app.join("dash/layout.tsx"),
            "export default function L({children}) { return children }",
        )
        .unwrap();
        // A level with a template and no layout beside it.
        fs::write(
            app.join("dash/reports/template.tsx"),
            "export default function T({children}) { return children }",
        )
        .unwrap();
        fs::write(
            app.join("dash/reports/page.tsx"),
            "export default function Page() { return <main/>; }",
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let route = &manifest.routes[0];

        assert_eq!(route.path, "/dash/reports");
        assert_eq!(route.layout_chain, vec!["app/layout", "app/dash/layout"]);
        assert_eq!(
            route.template_chain,
            vec!["app/template", "app/dash/reports/template"],
            "root first, and only the levels that have one"
        );

        // A route with no template in scope carries an empty chain, which is
        // what keeps its emitted bundle byte-identical to before the feature.
        fs::write(
            app.join("page.tsx"),
            "export default function Home() { return <main/>; }",
        )
        .unwrap();
        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let home = manifest
            .routes
            .iter()
            .find(|route| route.path == "/")
            .expect("the home route");
        assert_eq!(
            home.template_chain,
            vec!["app/template"],
            "the root template is in scope for the root route too"
        );
    }

    /// A `@name` folder declares a slot the level's layout receives as a prop.
    ///
    /// Slots match the URL independently of the page, which is the whole point:
    /// `/dashboard/reports` renders the page from `reports/page.tsx` and the
    /// team panel from `@team/reports/page.tsx` at the same time. A slot with
    /// nothing for the current URL falls back to its `default.tsx`.
    ///
    /// Before this, a `@name` directory was pruned from the walk and produced
    /// nothing at all — a project that wrote one got no route, no slot, and no
    /// diagnostic.
    #[test]
    fn a_parallel_slot_resolves_against_the_url_below_its_level() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let page = "export default function P() { return <main/>; }";
        fs::create_dir_all(app.join("dashboard/reports")).unwrap();
        fs::create_dir_all(app.join("dashboard/@team/reports")).unwrap();
        fs::create_dir_all(app.join("dashboard/@activity")).unwrap();
        fs::write(
            app.join("dashboard/layout.tsx"),
            "export default function L({children}) { return children }",
        )
        .unwrap();
        fs::write(app.join("dashboard/page.tsx"), page).unwrap();
        fs::write(app.join("dashboard/reports/page.tsx"), page).unwrap();
        // The team slot has a page for both URLs.
        fs::write(app.join("dashboard/@team/page.tsx"), page).unwrap();
        fs::write(app.join("dashboard/@team/reports/page.tsx"), page).unwrap();
        // The activity slot has a page for the index only, and a default for
        // everything else.
        fs::write(app.join("dashboard/@activity/page.tsx"), page).unwrap();
        fs::write(app.join("dashboard/@activity/default.tsx"), page).unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let slots_for = |path: &str| {
            manifest
                .routes
                .iter()
                .find(|route| route.path == path)
                .unwrap_or_else(|| panic!("no route {path}"))
                .slots
                .iter()
                .map(|slot| {
                    (
                        slot.name.clone(),
                        slot.level.clone(),
                        slot.file
                            .strip_prefix(&app)
                            .unwrap_or(&slot.file)
                            .display()
                            .to_string()
                            .replace('\\', "/"),
                    )
                })
                .collect::<Vec<_>>()
        };

        // A `@name` folder is still not a route of its own.
        assert_eq!(
            manifest
                .routes
                .iter()
                .map(|route| route.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/dashboard", "/dashboard/reports"]
        );

        assert_eq!(
            slots_for("/dashboard"),
            vec![
                (
                    "activity".to_string(),
                    "app/dashboard".to_string(),
                    "dashboard/@activity/page.tsx".to_string()
                ),
                (
                    "team".to_string(),
                    "app/dashboard".to_string(),
                    "dashboard/@team/page.tsx".to_string()
                ),
            ],
            "named order, not filesystem order"
        );
        assert_eq!(
            slots_for("/dashboard/reports"),
            vec![
                (
                    "activity".to_string(),
                    "app/dashboard".to_string(),
                    "dashboard/@activity/default.tsx".to_string()
                ),
                (
                    "team".to_string(),
                    "app/dashboard".to_string(),
                    "dashboard/@team/reports/page.tsx".to_string()
                ),
            ],
            "the team slot follows the URL; the activity slot falls back"
        );
    }

    /// A slot with neither a matching page nor a default contributes nothing.
    ///
    /// The layout simply does not receive the prop, which is what Next.js
    /// renders for an unmatched slot with no `default.tsx`. Inventing an empty
    /// element instead would put a wrapper in the tree the author never wrote.
    #[test]
    fn an_unmatched_slot_without_a_default_is_left_out() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        let page = "export default function P() { return <main/>; }";
        fs::create_dir_all(app.join("dashboard/settings")).unwrap();
        fs::create_dir_all(app.join("dashboard/@team")).unwrap();
        fs::write(app.join("dashboard/settings/page.tsx"), page).unwrap();
        fs::write(app.join("dashboard/@team/page.tsx"), page).unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let route = manifest
            .routes
            .iter()
            .find(|route| route.path == "/dashboard/settings")
            .expect("the settings route");
        assert!(
            route.slots.is_empty(),
            "the team slot has nothing for /dashboard/settings: {:?}",
            route.slots
        );
    }
}
