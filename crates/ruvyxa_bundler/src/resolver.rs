//! Module resolver: walks `import`/`require` specifiers and produces a
//! topologically-ordered list of (absolute-path, source-code) pairs.
//!
//! ## Performance
//!
//! The resolver maintains a **resolution cache** that maps
//! `(base_dir, specifier)` pairs to their resolved absolute paths.
//! This eliminates redundant filesystem stat calls when the same specifier
//! is imported from the same directory by multiple modules (common with
//! shared utilities like `"./utils"` or `"../components/Button"`).
//!
//! For large module graphs (100+ modules), this can reduce stat syscalls
//! from thousands to hundreds.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{BundleError, Result};

/// A resolved module: its canonical path and raw source text.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    /// Canonical absolute path to the source file.
    pub path: PathBuf,
    /// Raw UTF-8 source (TypeScript/TSX/JS/JSX).
    pub source: String,
    /// Specifiers that this module imports (absolute paths after resolution).
    pub deps: Vec<PathBuf>,
    /// Whether this module is part of `node_modules` (external).
    pub is_external: bool,
}

/// Cache for resolved specifier lookups.
///
/// Keyed by `(base_directory, specifier)` → resolved `PathBuf`.
/// This avoids probing up to 11 filesystem paths per import when the
/// same import has already been resolved from the same directory.
#[derive(Debug, Default)]
struct ResolveCache {
    /// Maps (base_dir string, specifier) → resolved path.
    entries: HashMap<(String, String), Option<PathBuf>>,
}

impl ResolveCache {
    fn get(&self, base_dir: &Path, specifier: &str) -> Option<&Option<PathBuf>> {
        let key = (
            base_dir.to_string_lossy().into_owned(),
            specifier.to_string(),
        );
        self.entries.get(&key)
    }

    fn insert(&mut self, base_dir: &Path, specifier: &str, result: Option<PathBuf>) {
        let key = (
            base_dir.to_string_lossy().into_owned(),
            specifier.to_string(),
        );
        self.entries.insert(key, result);
    }
}

/// Walk the import graph starting from a virtual entry source string.
///
/// Returns an ordered `Vec` of [`ResolvedModule`] values.  The virtual entry
/// is always first; thereafter modules appear in BFS discovery order.
pub fn resolve_graph(
    entry_source: &str,
    entry_label: &str,
    project_root: &Path,
    app_dir: &Path,
) -> Result<Vec<ResolvedModule>> {
    let project_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let app_dir = app_dir
        .canonicalize()
        .unwrap_or_else(|_| app_dir.to_path_buf());
    let mut visited: BTreeMap<PathBuf, ResolvedModule> = BTreeMap::new();
    let mut order: Vec<PathBuf> = Vec::new();
    let mut queue: VecDeque<(PathBuf, String)> = VecDeque::new();
    let mut resolve_cache = ResolveCache::default();

    // Virtual entry — synthetic key that won't collide with real files.
    let entry_key = PathBuf::from(entry_label);
    queue.push_back((entry_key.clone(), entry_source.to_string()));

    while let Some((current_path, source)) = queue.pop_front() {
        if visited.contains_key(&current_path) {
            continue;
        }

        let is_external = current_path
            .components()
            .any(|c| c.as_os_str() == "node_modules");

        let deps = if is_external {
            Vec::new()
        } else {
            let resolve_base = if current_path == entry_key {
                project_root.clone()
            } else {
                current_path.parent().unwrap_or(&project_root).to_path_buf()
            };

            collect_deps_cached(
                &source,
                &resolve_base,
                &project_root,
                &app_dir,
                &mut resolve_cache,
            )?
        };

        // Enqueue unvisited dependencies.
        for dep in &deps {
            if !visited.contains_key(dep) {
                match fs::read_to_string(dep) {
                    Ok(dep_source) => queue.push_back((dep.clone(), dep_source)),
                    Err(err) => {
                        return Err(BundleError::Io(err));
                    }
                }
            }
        }

        order.push(current_path.clone());
        visited.insert(
            current_path,
            ResolvedModule {
                path: if order.len() == 1 {
                    entry_key.clone()
                } else {
                    order[order.len() - 1].clone()
                },
                source,
                deps,
                is_external,
            },
        );
    }

    Ok(order
        .into_iter()
        .filter_map(|path| visited.remove(&path))
        .collect())
}

/// Resolve dependencies using the resolution cache to avoid repeated stat calls.
fn collect_deps_cached(
    source: &str,
    base_dir: &Path,
    project_root: &Path,
    _app_dir: &Path,
    cache: &mut ResolveCache,
) -> Result<Vec<PathBuf>> {
    let mut deps = Vec::new();

    for specifier in extract_specifiers(source) {
        if is_non_js_asset_specifier(&specifier) {
            continue;
        }

        let resolved = if specifier.starts_with('.') {
            // Check cache first.
            if let Some(cached) = cache.get(base_dir, &specifier) {
                cached.clone()
            } else {
                let result = resolve_specifier(base_dir, &specifier);
                cache.insert(base_dir, &specifier, result.clone());
                result
            }
        } else {
            // Absolute or project-root-relative paths are framework-generated
            // local imports. Bare specifiers such as "react" remain external.
            resolve_project_specifier(project_root, &specifier)
        };

        match resolved {
            Some(abs_path) => {
                if is_project_local(&abs_path, project_root) {
                    deps.push(abs_path);
                }
            }
            None => {
                if !specifier.starts_with('.') {
                    continue;
                }
                return Err(BundleError::Unresolved {
                    specifier,
                    importer: base_dir.to_path_buf(),
                });
            }
        }
    }

    Ok(deps)
}

fn is_non_js_asset_specifier(specifier: &str) -> bool {
    let lower = specifier.to_ascii_lowercase();
    matches!(
        Path::new(&lower).extension().and_then(|ext| ext.to_str()),
        Some("css" | "scss" | "sass" | "less")
    )
}

/// Extract all import/export specifier strings from source text.
///
/// This is a lightweight line-oriented scanner — not a full AST parse.  It
/// handles the common patterns used inside Ruvyxa projects.
fn extract_specifiers(source: &str) -> Vec<String> {
    let mut specifiers = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // import … from "…" | export … from "…"
        if let Some(idx) = trimmed.find(" from ") {
            if let Some(spec) = quoted_value(&trimmed[idx + 6..]) {
                specifiers.push(spec);
            }
        }
        // import "…" (side-effect)
        else if let Some(after_import) = trimmed.strip_prefix("import ") {
            if let Some(spec) = quoted_value(after_import) {
                specifiers.push(spec);
            }
        }
    }

    specifiers
}

/// Extract the string value between the first pair of quotes.
fn quoted_value(s: &str) -> Option<String> {
    let quote = s.chars().find(|c| *c == '"' || *c == '\'')?;
    let start = s.find(quote)? + 1;
    let rest = &s[start..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

/// Resolve a relative specifier like `"./utils"` to an absolute file path,
/// probing TypeScript/JavaScript extensions in priority order.
pub fn resolve_specifier(base_dir: &Path, specifier: &str) -> Option<PathBuf> {
    let joined = base_dir.join(specifier);
    resolve_file_candidate(&joined)
}

fn resolve_project_specifier(project_root: &Path, specifier: &str) -> Option<PathBuf> {
    let path = Path::new(specifier);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    resolve_file_candidate(&candidate)
}

fn resolve_file_candidate(joined: &Path) -> Option<PathBuf> {
    // Probe extensions in priority order. Each candidate is a stat syscall.
    let candidates = [
        joined.to_path_buf(),
        joined.with_extension("ts"),
        joined.with_extension("tsx"),
        joined.with_extension("js"),
        joined.with_extension("jsx"),
        joined.with_extension("mts"),
        joined.with_extension("mjs"),
        joined.join("index.ts"),
        joined.join("index.tsx"),
        joined.join("index.js"),
        joined.join("index.jsx"),
    ];

    candidates
        .into_iter()
        .find(|p| p.is_file())
        .and_then(|p| p.canonicalize().ok().or(Some(p)))
}

fn is_project_local(path: &Path, project_root: &Path) -> bool {
    let rel = match path.strip_prefix(project_root) {
        Ok(r) => r,
        Err(_) => return false,
    };
    !rel.starts_with("node_modules")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_import_specifiers() {
        let source = r#"
            import React from "react"
            import { foo } from "./foo"
            import type { Bar } from './bar'
            import "./styles.css"
            export { baz } from "../baz"
        "#;

        let specs = extract_specifiers(source);
        assert!(specs.contains(&"./foo".to_string()));
        assert!(specs.contains(&"./bar".to_string()));
        assert!(specs.contains(&"./styles.css".to_string()));
        assert!(specs.contains(&"../baz".to_string()));
        assert!(specs.contains(&"react".to_string()));
    }

    #[test]
    fn quoted_value_handles_double_and_single_quotes() {
        assert_eq!(quoted_value(r#""hello""#), Some("hello".to_string()));
        assert_eq!(quoted_value("'world'"), Some("world".to_string()));
        assert_eq!(quoted_value("nothing"), None);
    }

    #[test]
    fn resolve_cache_deduplicates() {
        let mut cache = ResolveCache::default();
        let base = PathBuf::from("/project/src");

        // Initially empty
        assert!(cache.get(&base, "./utils").is_none());

        // Insert a result
        cache.insert(
            &base,
            "./utils",
            Some(PathBuf::from("/project/src/utils.ts")),
        );

        // Now cached
        let cached = cache.get(&base, "./utils");
        assert!(cached.is_some());
        assert_eq!(
            cached.unwrap().as_ref().unwrap(),
            &PathBuf::from("/project/src/utils.ts")
        );
    }

    #[test]
    fn resolve_cache_stores_none_for_unresolved() {
        let mut cache = ResolveCache::default();
        let base = PathBuf::from("/project/src");

        cache.insert(&base, "./missing", None);

        let cached = cache.get(&base, "./missing");
        assert!(cached.is_some()); // entry exists
        assert!(cached.unwrap().is_none()); // but value is None
    }

    #[test]
    fn resolves_absolute_project_imports() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        let page = app.join("page.tsx");
        fs::write(&page, "export default function Page() {}").unwrap();

        let import_path = page.display().to_string().replace('\\', "/");
        let source = format!(
            "import Page from {};",
            serde_json::to_string(&import_path).unwrap()
        );
        let root = temp.path().canonicalize().unwrap();
        let deps =
            collect_deps_cached(&source, &root, &root, &app, &mut ResolveCache::default()).unwrap();

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], page.canonicalize().unwrap());
    }

    #[test]
    fn ignores_css_side_effect_imports() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("global.css"), "body { margin: 0; }").unwrap();

        let deps = collect_deps_cached(
            "import \"./global.css\";",
            &app,
            temp.path(),
            &app,
            &mut ResolveCache::default(),
        )
        .unwrap();

        assert!(deps.is_empty());
    }
}
