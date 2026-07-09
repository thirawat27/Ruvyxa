//! Lightweight AST facts used by the native bundler pipeline.
//!
//! This is intentionally smaller than a full JavaScript parser, but it gives
//! the resolver and transformer a shared structured view of imports, exports,
//! JSX, decorators, and TypeScript-only syntax instead of duplicating ad hoc
//! line scans in each stage.

use serde::{Deserialize, Serialize};

/// Import edge discovered in a source module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportEdge {
    pub specifier: String,
    pub kind: ImportKind,
}

/// The import form that created an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportKind {
    Static,
    Dynamic,
    Require,
    ReExport,
    SideEffect,
}

/// Structured facts for one source module.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleAst {
    pub imports: Vec<ImportEdge>,
    pub exports: Vec<String>,
    pub has_jsx: bool,
    pub has_typescript: bool,
    pub has_decorators: bool,
    pub has_enums: bool,
}

impl ModuleAst {
    pub fn import_specifiers(&self) -> Vec<String> {
        self.imports
            .iter()
            .map(|edge| edge.specifier.clone())
            .collect()
    }

    pub fn dynamic_import_specifiers(&self) -> Vec<String> {
        self.imports
            .iter()
            .filter(|edge| edge.kind == ImportKind::Dynamic)
            .map(|edge| edge.specifier.clone())
            .collect()
    }
}

/// Parse source into the facts the bundler needs.
pub fn parse_module(source: &str) -> ModuleAst {
    let mut ast = ModuleAst::default();

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('@') {
            ast.has_decorators = true;
        }
        if starts_with_keyword(trimmed, "interface")
            || starts_with_keyword(trimmed, "type")
            || trimmed.contains(" satisfies ")
            || trimmed.contains(" as ")
            || trimmed.contains(": ")
        {
            ast.has_typescript = true;
        }
        if starts_with_keyword(trimmed, "enum") || trimmed.starts_with("const enum ") {
            ast.has_enums = true;
        }
        if looks_like_jsx(trimmed) {
            ast.has_jsx = true;
        }

        if let Some(edge) = static_import_edge(trimmed) {
            ast.imports.push(edge);
        }
        if let Some(edge) = re_export_edge(trimmed) {
            ast.imports.push(edge);
        }
        if trimmed.starts_with("export ") {
            if let Some(name) = export_name(trimmed) {
                ast.exports.push(name);
            }
        }

        for specifier in call_specifiers(trimmed, "require(") {
            ast.imports.push(ImportEdge {
                specifier,
                kind: ImportKind::Require,
            });
        }
        for specifier in call_specifiers(trimmed, "import(") {
            ast.imports.push(ImportEdge {
                specifier,
                kind: ImportKind::Dynamic,
            });
        }
    }

    ast
}

fn static_import_edge(line: &str) -> Option<ImportEdge> {
    if !line.starts_with("import ") {
        return None;
    }

    if line.starts_with("import \"") || line.starts_with("import '") {
        return quoted_value(line.strip_prefix("import ")?).map(|specifier| ImportEdge {
            specifier,
            kind: ImportKind::SideEffect,
        });
    }

    split_from_specifier(line).map(|(_, specifier)| ImportEdge {
        specifier,
        kind: ImportKind::Static,
    })
}

fn re_export_edge(line: &str) -> Option<ImportEdge> {
    if !line.starts_with("export ") {
        return None;
    }

    split_from_specifier(line).map(|(_, specifier)| ImportEdge {
        specifier,
        kind: ImportKind::ReExport,
    })
}

fn call_specifiers(line: &str, marker: &str) -> Vec<String> {
    let mut specifiers = Vec::new();
    let mut search_start = 0;

    while let Some(relative_index) = line[search_start..].find(marker) {
        let value_start = search_start + relative_index + marker.len();
        if let Some(specifier) = quoted_value(&line[value_start..]) {
            specifiers.push(specifier);
        }
        search_start = value_start;
    }

    specifiers
}

fn split_from_specifier(line: &str) -> Option<(String, String)> {
    let from_idx = line.rfind(" from ")?;
    let before = line[..from_idx].to_string();
    let after = line[from_idx + 6..].trim();
    let specifier = quoted_value(after)?;
    Some((before, specifier))
}

fn quoted_value(s: &str) -> Option<String> {
    let quote = s.chars().find(|c| *c == '"' || *c == '\'')?;
    let start = s.find(quote)? + 1;
    let rest = &s[start..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn export_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("export ")?;
    let rest = rest.strip_prefix("default ").unwrap_or(rest);
    let rest = rest.strip_prefix("async ").unwrap_or(rest);
    let rest = rest
        .strip_prefix("function* ")
        .or_else(|| rest.strip_prefix("function "))
        .or_else(|| rest.strip_prefix("class "))
        .or_else(|| rest.strip_prefix("const "))
        .or_else(|| rest.strip_prefix("let "))
        .or_else(|| rest.strip_prefix("var "))?;

    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();

    (!name.is_empty()).then_some(name)
}

fn looks_like_jsx(line: &str) -> bool {
    let Some(idx) = line.find('<') else {
        return false;
    };
    let mut chars = line[idx + 1..].chars();
    matches!(
        chars.next(),
        Some(c) if c.is_ascii_alphabetic() || c == '>' || c == '/'
    )
}

fn starts_with_keyword(line: &str, keyword: &str) -> bool {
    let Some(rest) = line.strip_prefix(keyword) else {
        return false;
    };
    rest.is_empty()
        || rest
            .chars()
            .next()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_static_dynamic_and_re_export_imports() {
        let ast = parse_module(
            r#"
import React from "react"
import "./global.css"
export { helper } from "./helper"
const lazy = import("./lazy")
const data = require("./data")
"#,
        );

        assert!(ast
            .imports
            .iter()
            .any(|edge| { edge.specifier == "react" && edge.kind == ImportKind::Static }));
        assert!(ast.imports.iter().any(|edge| {
            edge.specifier == "./global.css" && edge.kind == ImportKind::SideEffect
        }));
        assert!(ast
            .imports
            .iter()
            .any(|edge| { edge.specifier == "./helper" && edge.kind == ImportKind::ReExport }));
        assert_eq!(ast.dynamic_import_specifiers(), vec!["./lazy"]);
        assert!(ast.import_specifiers().contains(&"./data".to_string()));
    }

    #[test]
    fn records_transform_features() {
        let ast = parse_module(
            r#"
@sealed
const enum Mode { A }
export default function Page(props: Props) { return <main /> }
"#,
        );

        assert!(ast.has_decorators);
        assert!(ast.has_enums);
        assert!(ast.has_typescript);
        assert!(ast.has_jsx);
        assert!(ast.exports.contains(&"Page".to_string()));
    }
}
