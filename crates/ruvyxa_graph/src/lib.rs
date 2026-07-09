use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteManifest {
    pub app_dir: PathBuf,
    pub routes: Vec<RouteEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteEntry {
    pub id: String,
    pub path: String,
    pub kind: RouteKind,
    pub file: PathBuf,
    pub layout_chain: Vec<String>,
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

/// Metadata that controls the rendering strategy for a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RenderMeta {
    /// The rendering strategy for this route.
    pub strategy: RenderStrategy,
    /// ISR revalidation interval in seconds (only meaningful for `Isr`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revalidate: Option<u64>,
    /// Whether the page exports `getStaticParams` for dynamic SSG routes.
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverOptions {
    pub app_dir: PathBuf,
}

impl DiscoverOptions {
    pub fn new(app_dir: impl Into<PathBuf>) -> Self {
        Self {
            app_dir: app_dir.into(),
        }
    }
}

pub fn discover_routes(options: DiscoverOptions) -> Result<RouteManifest> {
    let app_dir = options.app_dir;

    if !app_dir.exists() {
        return Err(Diagnostic::new("RUV1001", "App directory was not found")
            .explain("Ruvyxa expects an app directory with page.tsx or route.ts files.")
            .at_file(&app_dir)
            .suggest("Create app/page.tsx or set appDir in ruvyxa.config.ts.")
            .into());
    }

    let mut routes = Vec::new();

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
            "page.tsx" | "page.jsx" => RouteKind::Page,
            "route.ts" | "route.js" => RouteKind::Api,
            _ => continue,
        };

        let file = entry.path().to_path_buf();
        let route_dir = file.parent().unwrap_or(&app_dir);
        let relative_dir = route_dir.strip_prefix(&app_dir).unwrap_or(route_dir);
        let path = route_path_from_dir(relative_dir)?;
        let id = route_id(&app_dir, &file);

        routes.push(RouteEntry {
            id,
            path,
            kind,
            file: file.clone(),
            layout_chain: layout_chain(&app_dir, route_dir),
            server_modules: sibling_modules(
                route_dir,
                &["server.ts", "server.js", "action.ts", "action.js"],
            ),
            client_modules: sibling_module(route_dir, "client.tsx"),
            runtime: RuntimeTarget::Node,
            render: if kind == RouteKind::Page {
                detect_render_strategy(&file)
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

    Ok(RouteManifest { app_dir, routes })
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

pub fn read_manifest(manifest_file: &Path) -> Result<RouteManifest> {
    let json = fs::read_to_string(manifest_file)?;
    serde_json::from_str(&json).map_err(|error| RuvyxaError::Message(error.to_string()))
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
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    // Track which modules have already been validated to avoid duplicate reads.
    let mut validated_client: BTreeSet<PathBuf> = BTreeSet::new();
    let mut validated_server: BTreeSet<PathBuf> = BTreeSet::new();

    for route in &manifest.routes {
        match route.kind {
            RouteKind::Page => {
                let source = fs::read_to_string(&route.file)?;
                if !source.contains("export default") {
                    diagnostics.push(
                        Diagnostic::new("RUV1004", "Page is missing a default export")
                            .explain("Every page.tsx file must export a default component.")
                            .at_file(&route.file)
                            .suggest("Add `export default function Page() { return <main /> }`."),
                    );
                }

                let graph = collect_relative_graph(&route.file);
                for module in graph {
                    client_modules.insert(module.clone());
                    // Skip if already validated — avoids redundant fs::read + canonicalize.
                    if validated_client.insert(module.clone()) {
                        validate_client_module(&canonical_root, &module, &mut diagnostics)?;
                    }
                }
            }
            RouteKind::Api => {
                let graph = collect_relative_graph(&route.file);
                for module in graph {
                    server_modules.insert(module.clone());
                    if validated_server.insert(module.clone()) {
                        validate_server_module(&module, &mut diagnostics)?;
                    }
                }
            }
        }

        for module in &route.server_modules {
            let module = PathBuf::from(module);
            let graph = collect_relative_graph(&module);
            for module in graph {
                server_modules.insert(module.clone());
                if validated_server.insert(module.clone()) {
                    validate_server_module(&module, &mut diagnostics)?;
                }
            }
        }

        for module in &route.client_modules {
            let module = PathBuf::from(module);
            client_modules.insert(module.clone());
            if validated_client.insert(module.clone()) {
                validate_client_module(&canonical_root, &module, &mut diagnostics)?;
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
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let Ok(source) = fs::read_to_string(file) else {
        return Ok(());
    };

    let code = code_without_strings_and_comments(&source);

    if import_specifiers(&source)
        .iter()
        .any(|specifier| specifier == "server-only")
    {
        diagnostics.push(
            Diagnostic::new("RUV1007", "Server-only module imported into client graph")
                .explain("This module is reachable from a hydrated page or client module but declares `server-only`.")
                .at_file(file)
                .suggest("Move server-only work behind a route handler/server module and pass serializable data to the client."),
        );
    }

    for env_name in private_env_reads(&code) {
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
        // Paths don't share a prefix — try canonicalizing the file as fallback.
        let canonical_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
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

fn validate_server_module(file: &Path, diagnostics: &mut Vec<Diagnostic>) -> Result<()> {
    let Ok(source) = fs::read_to_string(file) else {
        return Ok(());
    };

    if import_specifiers(&source)
        .iter()
        .any(|specifier| specifier == "client-only")
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

fn collect_relative_graph(entry: &Path) -> BTreeSet<PathBuf> {
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::from([entry.to_path_buf()]);

    while let Some(file) = queue.pop_front() {
        if !visited.insert(file.clone()) {
            continue;
        }

        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };

        for specifier in import_specifiers(&source) {
            if !specifier.starts_with('.') {
                continue;
            }

            if let Some(resolved) = resolve_relative_import(&file, &specifier) {
                queue.push_back(resolved);
            }
        }
    }

    visited
}

fn import_specifiers(source: &str) -> Vec<String> {
    let source = code_for_import_specifiers(source);
    let mut imports = Vec::new();

    for line in source.lines() {
        let line = line.trim();

        if let Some(index) = line.find(" from ") {
            if let Some(specifier) = quoted_value(&line[index + " from ".len()..]) {
                imports.push(specifier);
            }
        } else if line.starts_with("import ") {
            if let Some(specifier) = quoted_value(line.trim_start_matches("import").trim()) {
                imports.push(specifier);
            }
        }
    }

    imports
}

fn quoted_value(input: &str) -> Option<String> {
    let quote = input
        .chars()
        .find(|character| *character == '"' || *character == '\'')?;
    let start = input.find(quote)? + 1;
    let rest = &input[start..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn resolve_relative_import(from: &Path, specifier: &str) -> Option<PathBuf> {
    let base = from.parent()?.join(specifier);
    let candidates = [
        base.clone(),
        base.with_extension("ts"),
        base.with_extension("tsx"),
        base.with_extension("js"),
        base.with_extension("jsx"),
        base.join("index.ts"),
        base.join("index.tsx"),
        base.join("index.js"),
        base.join("index.jsx"),
    ];

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.canonicalize().ok().or(Some(candidate)))
}

fn private_env_reads(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let marker = "process.env.";
    let mut rest = source;

    while let Some(index) = rest.find(marker) {
        rest = &rest[index + marker.len()..];
        let name = rest
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .collect::<String>();

        if !name.is_empty() && !name.starts_with("RUVYXA_PUBLIC_") {
            names.push(name);
        }
    }

    names
}

fn relative_starts_with_server(relative: &Path) -> bool {
    relative
        .components()
        .next()
        .is_some_and(|component| component.as_os_str() == "server")
}

fn code_without_strings_and_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut chars = source.char_indices().peekable();

    while let Some((_, character)) = chars.next() {
        match character {
            '"' | '\'' => {
                output.push(' ');
                skip_quoted_string(character, &mut chars, &mut output);
            }
            '`' => {
                output.push(' ');
                skip_template_literal(&mut chars, &mut output);
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '/') => {
                output.push(' ');
                chars.next();
                output.push(' ');
                skip_line_comment(&mut chars, &mut output);
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '*') => {
                output.push(' ');
                chars.next();
                output.push(' ');
                skip_block_comment(&mut chars, &mut output);
            }
            _ => output.push(character),
        }
    }

    output
}

fn code_for_import_specifiers(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut chars = source.char_indices().peekable();

    while let Some((_, character)) = chars.next() {
        match character {
            '"' | '\'' => {
                if should_preserve_import_string(&output) {
                    output.push(character);
                    copy_quoted_string(character, &mut chars, &mut output);
                } else {
                    output.push(' ');
                    skip_quoted_string(character, &mut chars, &mut output);
                }
            }
            '`' => {
                output.push(' ');
                skip_template_literal(&mut chars, &mut output);
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '/') => {
                output.push(' ');
                chars.next();
                output.push(' ');
                skip_line_comment(&mut chars, &mut output);
            }
            '/' if chars.peek().is_some_and(|(_, next)| *next == '*') => {
                output.push(' ');
                chars.next();
                output.push(' ');
                skip_block_comment(&mut chars, &mut output);
            }
            _ => output.push(character),
        }
    }

    output
}

fn should_preserve_import_string(output: &str) -> bool {
    let trimmed = output.trim_end();
    trimmed.ends_with(" from")
        || trimmed.ends_with("import")
        || trimmed.ends_with("import(")
        || trimmed.ends_with("require(")
}

fn copy_quoted_string(
    quote: char,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    output: &mut String,
) {
    let mut escaped = false;
    for (_, character) in chars.by_ref() {
        output.push(character);

        if escaped {
            escaped = false;
            continue;
        }

        if character == '\\' {
            escaped = true;
            continue;
        }

        if character == quote {
            break;
        }
    }
}

fn skip_quoted_string(
    quote: char,
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    output: &mut String,
) {
    let mut escaped = false;
    for (_, character) in chars.by_ref() {
        if character == '\n' {
            output.push('\n');
        } else {
            output.push(' ');
        }

        if escaped {
            escaped = false;
            continue;
        }

        if character == '\\' {
            escaped = true;
            continue;
        }

        if character == quote {
            break;
        }
    }
}

fn skip_template_literal(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    output: &mut String,
) {
    let mut escaped = false;
    for (_, character) in chars.by_ref() {
        if character == '\n' {
            output.push('\n');
        } else {
            output.push(' ');
        }

        if escaped {
            escaped = false;
            continue;
        }

        if character == '\\' {
            escaped = true;
            continue;
        }

        if character == '`' {
            break;
        }
    }
}

fn skip_line_comment(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    output: &mut String,
) {
    for (_, character) in chars.by_ref() {
        if character == '\n' {
            output.push('\n');
            break;
        }
        output.push(' ');
    }
}

fn skip_block_comment(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    output: &mut String,
) {
    let mut previous = '\0';
    for (_, character) in chars.by_ref() {
        if character == '\n' {
            output.push('\n');
        } else {
            output.push(' ');
        }

        if previous == '*' && character == '/' {
            break;
        }
        previous = character;
    }
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
        return Ok(format!("*{name}?"));
    }

    if segment.starts_with("[...") && segment.ends_with(']') {
        let name = &segment[4..segment.len() - 1];
        validate_dynamic_name(name)?;
        if !is_last {
            return Err(catch_all_must_be_last());
        }
        return Ok(format!("*{name}"));
    }

    if segment.starts_with('[') && segment.ends_with(']') {
        let name = &segment[1..segment.len() - 1];
        validate_dynamic_name(name)?;
        return Ok(format!(":{name}"));
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
    let mut layouts = Vec::new();
    let mut current = app_dir.to_path_buf();

    if current.join("layout.tsx").exists() {
        layouts.push(route_id(app_dir, &current.join("layout.tsx")));
    }

    if let Ok(relative) = route_dir.strip_prefix(app_dir) {
        for component in relative.components() {
            let Component::Normal(segment) = component else {
                continue;
            };
            current.push(segment);
            let layout = current.join("layout.tsx");
            if layout.exists() {
                layouts.push(route_id(app_dir, &layout));
            }
        }
    }

    layouts
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
/// 4. `export function getStaticParams` or `export async function getStaticParams` → SSG
/// 5. Route has no dynamic segments and no data fetching → SSG candidate (static routes)
/// 6. Default → SSR
fn detect_render_strategy(file: &Path) -> RenderMeta {
    let Ok(source) = fs::read_to_string(file) else {
        return RenderMeta::default();
    };

    let code = code_without_strings_and_comments(&source);

    // 1. Check for "use client" directive (must be in original source, at top)
    let trimmed = source.trim_start();
    if trimmed.starts_with("\"use client\"") || trimmed.starts_with("'use client'") {
        return RenderMeta {
            strategy: RenderStrategy::Csr,
            ..Default::default()
        };
    }

    // 2. Check for PPR opt-in: export const ppr = true
    if has_export_const_bool(&code, "ppr", true) {
        return RenderMeta {
            strategy: RenderStrategy::Ppr,
            has_dynamic_slots: true,
            ..Default::default()
        };
    }

    // 3. Check for ISR: export const revalidate = <number>
    if let Some(seconds) = parse_export_const_number(&code, "revalidate") {
        let has_static_params = has_export_function(&code, "getStaticParams");
        return RenderMeta {
            strategy: RenderStrategy::Isr,
            revalidate: Some(seconds),
            has_static_params,
            ..Default::default()
        };
    }

    // 4. Check for SSG: export function getStaticParams / export async function getStaticParams
    if has_export_function(&code, "getStaticParams") {
        return RenderMeta {
            strategy: RenderStrategy::Ssg,
            has_static_params: true,
            ..Default::default()
        };
    }

    // 5. Default: SSR
    RenderMeta::default()
}

/// Check if `export const <name> = true|false` exists.
fn has_export_const_bool(code: &str, name: &str, expected: bool) -> bool {
    let pattern = format!("export const {name}");
    for line in code.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&pattern) {
            let after = trimmed[pattern.len()..].trim();
            if let Some(rest) = after.strip_prefix('=') {
                let value = rest.trim().trim_end_matches(';').trim();
                if expected && value == "true" {
                    return true;
                }
                if !expected && value == "false" {
                    return true;
                }
            }
        }
    }
    false
}

/// Parse `export const <name> = <number>` and return the number.
fn parse_export_const_number(code: &str, name: &str) -> Option<u64> {
    let pattern = format!("export const {name}");
    for line in code.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&pattern) {
            let after = trimmed[pattern.len()..].trim();
            if let Some(rest) = after.strip_prefix('=') {
                let value = rest.trim().trim_end_matches(';').trim();
                if let Ok(n) = value.parse::<u64>() {
                    return Some(n);
                }
            }
        }
    }
    None
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
            if trimmed.starts_with(pattern.as_str()) {
                return true;
            }
        }
    }
    false
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
            if segment.starts_with(':') {
                ":"
            } else if segment.starts_with('*') {
                "*"
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

        assert_eq!(paths, vec!["/", "/about", "/blog/:slug"]);
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

        assert_eq!(paths, vec!["/docs/*slug", "/pricing", "/shop/*category?"]);
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
}
