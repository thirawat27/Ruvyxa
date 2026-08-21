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

        routes.push(RouteEntry {
            id,
            path: path.clone(),
            kind,
            file: file.clone(),
            layout_chain: layout_chain.clone(),
            template_chain,
            slots,
            server_modules: sibling_modules(
                route_dir,
                &["server.ts", "server.js", "action.ts", "action.js"],
            ),
            client_modules: sibling_module(route_dir, "client.tsx"),
            runtime: RuntimeTarget::Node,
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
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

pub fn validate_app(root: &Path, manifest: &RouteManifest) -> Result<ValidationReport> {
    let mut diagnostics = Vec::new();
    let mut client_modules = BTreeSet::new();
    let mut server_modules = BTreeSet::new();

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
                for module in graph {
                    client_modules.insert(module.clone());
                    // Skip if already validated — the cache makes the re-read
                    // free, but the diagnostics would be emitted twice.
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

    candidates.into_iter().find_map(|candidate| {
        [candidate.clone(), candidate.with_extension("tsx")]
            .into_iter()
            .find(|file| file.is_file())
            .and_then(|file| file.canonicalize().ok().or(Some(file)))
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

/// Detect the rendering strategy for a page by scanning its source for known exports/directives.
///
/// Detection rules (first match wins):
/// 1. `"use client"` directive at top → CSR
/// 2. `export const ppr = true` → PPR
/// 3. `export const revalidate = <number>` → ISR with that interval
/// 4. `getStaticParams` or `staticParams` page export → SSG
/// 5. Route has no dynamic segments and no data fetching → SSG candidate (static routes)
/// 6. Default → SSR
fn detect_render_strategy(
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
