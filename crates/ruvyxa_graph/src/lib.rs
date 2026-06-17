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
                    validate_client_module(root, &module, &mut diagnostics)?;
                }
            }
            RouteKind::Api => {
                let graph = collect_relative_graph(&route.file);
                for module in graph {
                    server_modules.insert(module.clone());
                    validate_server_module(&module, &mut diagnostics)?;
                }
            }
        }

        for module in &route.server_modules {
            let module = PathBuf::from(module);
            let graph = collect_relative_graph(&module);
            for module in graph {
                server_modules.insert(module.clone());
                validate_server_module(&module, &mut diagnostics)?;
            }
        }

        for module in &route.client_modules {
            let module = PathBuf::from(module);
            client_modules.insert(module.clone());
            validate_client_module(root, &module, &mut diagnostics)?;
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
    root: &Path,
    file: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    let Ok(source) = fs::read_to_string(file) else {
        return Ok(());
    };

    if source.contains("\"server-only\"") || source.contains("'server-only'") {
        diagnostics.push(
            Diagnostic::new("RUV1007", "Server-only module imported into client graph")
                .explain("This module is reachable from a hydrated page or client module but declares `server-only`.")
                .at_file(file)
                .suggest("Move server-only work behind a route handler/server module and pass serializable data to the client."),
        );
    }

    for env_name in private_env_reads(&source) {
        diagnostics.push(
            Diagnostic::new("RUV1008", "Private environment variable used in client graph")
                .explain(format!(
                    "`process.env.{env_name}` is reachable from browser code. Only `RUVYXA_PUBLIC_*` env vars may be exposed to client modules."
                ))
                .at_file(file)
                .suggest("Move the env read into server-only code or rename it to `RUVYXA_PUBLIC_*` if it is safe to expose."),
        );
    }

    let normalized_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let normalized_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());

    if let Ok(relative) = normalized_file.strip_prefix(&normalized_root) {
        if relative
            .components()
            .any(|component| component.as_os_str() == "server")
        {
            diagnostics.push(
                Diagnostic::new("RUV1010", "Server directory module reached by client graph")
                    .explain("Files under server/ are reserved for server-only code.")
                    .at_file(file)
                    .suggest("Move shared browser-safe code outside server/, or import it from a server route only."),
            );
        }
    }

    Ok(())
}

fn validate_server_module(file: &Path, diagnostics: &mut Vec<Diagnostic>) -> Result<()> {
    let Ok(source) = fs::read_to_string(file) else {
        return Ok(());
    };

    if source.contains("\"client-only\"") || source.contains("'client-only'") {
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

fn route_path_from_dir(relative_dir: &Path) -> Result<String> {
    let mut segments = Vec::new();

    for component in relative_dir.components() {
        let Component::Normal(segment) = component else {
            continue;
        };
        let segment = segment.to_string_lossy();

        if segment.starts_with('(') && segment.ends_with(')') {
            continue;
        }

        if segment.starts_with('@') {
            continue;
        }

        segments.push(route_segment(&segment)?);
    }

    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}

fn route_segment(segment: &str) -> Result<String> {
    if segment.starts_with("[[") && segment.ends_with("]]") {
        let name = &segment[2..segment.len() - 2];
        return Ok(format!(":{name}?"));
    }

    if segment.starts_with("[...") && segment.ends_with(']') {
        let name = &segment[4..segment.len() - 1];
        return Ok(format!("*{name}"));
    }

    if segment.starts_with('[') && segment.ends_with(']') {
        let name = &segment[1..segment.len() - 1];
        return Ok(format!(":{name}"));
    }

    if segment.contains('[') || segment.contains(']') {
        return Err(Diagnostic::new("RUV1002", "Invalid dynamic route segment")
            .explain("Dynamic route segments must use [name], [...name], or [[name]].")
            .suggest("Rename the route folder to a valid dynamic segment.")
            .into());
    }

    Ok(segment.to_string())
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

fn detect_conflicts(routes: &[RouteEntry]) -> Result<()> {
    let mut seen = BTreeMap::<(&str, RouteKind), &RouteEntry>::new();
    let mut conflicts = BTreeSet::new();

    for route in routes {
        let key = (route.path.as_str(), route.kind);
        if let Some(previous) = seen.insert(key, route) {
            conflicts.insert(previous.path.clone());
            conflicts.insert(route.path.clone());
        }
    }

    if !conflicts.is_empty() {
        return Err(Diagnostic::new("RUV1003", "Duplicate route paths")
            .explain("Two or more route files resolve to the same URL path and route kind.")
            .suggest("Rename one of the route directories or move it into a route group.")
            .into());
    }

    Ok(())
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
    fn supports_catch_all_optional_and_route_groups() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("docs/[...slug]")).unwrap();
        fs::create_dir_all(app.join("shop/[[category]]")).unwrap();
        fs::create_dir_all(app.join("(marketing)/pricing")).unwrap();
        fs::write(app.join("docs/[...slug]/page.tsx"), "").unwrap();
        fs::write(app.join("shop/[[category]]/page.tsx"), "").unwrap();
        fs::write(app.join("(marketing)/pricing/page.tsx"), "").unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        let paths = manifest
            .routes
            .iter()
            .map(|route| route.path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["/docs/*slug", "/pricing", "/shop/:category?"]);
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
}
