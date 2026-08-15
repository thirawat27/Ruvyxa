//! Compile-time `import.meta.glob` expansion.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ast;
use crate::{BundleError, Result};

#[derive(Debug, Default)]
pub(crate) struct GlobExpansion {
    pub source: String,
    pub matches: Vec<PathBuf>,
    pub watch_roots: Vec<PathBuf>,
}

pub(crate) fn expand_import_meta_glob(
    source: &str,
    importer_dir: &Path,
    project_root: &Path,
    resolve_pattern: impl Fn(&str) -> Option<PathBuf>,
) -> Result<GlobExpansion> {
    const MARKER: &str = "import.meta.glob";
    let ast = ast::parse_module(source);
    let mut replacements = Vec::new();
    let mut all_matches = Vec::new();
    let mut watch_roots = Vec::new();
    let mut cursor = 0;

    while let Some(relative) = source[cursor..].find(MARKER) {
        let start = cursor + relative;
        cursor = start + MARKER.len();
        if !ast.is_code_offset(start) {
            continue;
        }
        let parsed = parse_call(source, start)?;
        let absolute_pattern = if parsed.pattern.starts_with('.') {
            importer_dir.join(&parsed.pattern)
        } else {
            resolve_pattern(&parsed.pattern).ok_or_else(|| {
                glob_error(format!("cannot resolve glob pattern {:?}", parsed.pattern))
            })?
        };
        let absolute_pattern = normalize_without_io(&absolute_pattern);
        if !path_is_inside_lexically(&absolute_pattern, project_root) {
            return Err(glob_error(format!(
                "glob pattern {:?} escapes the project root",
                parsed.pattern
            )));
        }
        let watch_root = glob_watch_root(&absolute_pattern, project_root);
        let mut matches = collect_matches(&absolute_pattern, &watch_root)?;
        matches.sort_by_key(|path| slash(path));
        matches.dedup();

        let entries = matches
            .iter()
            .map(|file| {
                let specifier = relative_specifier(importer_dir, file);
                let key = serde_json::to_string(&specifier).expect("path string serializes");
                let import = serde_json::to_string(&specifier).expect("path string serializes");
                if parsed.eager {
                    format!("{key}: require({import})")
                } else {
                    format!("{key}: () => import({import})")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        replacements.push((start, parsed.end, format!("{{{entries}}}")));
        all_matches.extend(matches);
        watch_roots.push(watch_root);
        cursor = parsed.end;
    }

    let mut expanded = source.to_string();
    for (start, end, replacement) in replacements.into_iter().rev() {
        expanded.replace_range(start..end, &replacement);
    }
    all_matches.sort();
    all_matches.dedup();
    watch_roots.sort();
    watch_roots.dedup();
    Ok(GlobExpansion {
        source: expanded,
        matches: all_matches,
        watch_roots,
    })
}

struct ParsedCall {
    pattern: String,
    eager: bool,
    end: usize,
}

fn parse_call(source: &str, start: usize) -> Result<ParsedCall> {
    let bytes = source.as_bytes();
    let mut cursor = start + "import.meta.glob".len();
    skip_whitespace(bytes, &mut cursor);
    if bytes.get(cursor) != Some(&b'(') {
        return Err(glob_error("import.meta.glob must be called directly"));
    }
    cursor += 1;
    skip_whitespace(bytes, &mut cursor);
    let quote = *bytes
        .get(cursor)
        .filter(|quote| **quote == b'\'' || **quote == b'"')
        .ok_or_else(|| glob_error("glob pattern must be a string literal"))?;
    cursor += 1;
    let mut pattern = String::new();
    let mut escaped = false;
    let mut closed = false;
    while cursor < source.len() {
        let character = source[cursor..]
            .chars()
            .next()
            .expect("cursor is in bounds");
        cursor += character.len_utf8();
        if escaped {
            pattern.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character as u32 == u32::from(quote) {
            closed = true;
            break;
        } else {
            pattern.push(character);
        }
    }
    if !closed {
        return Err(glob_error("unterminated glob pattern"));
    }
    skip_whitespace(bytes, &mut cursor);
    let eager = if bytes.get(cursor) == Some(&b',') {
        cursor += 1;
        let option_start = cursor;
        while let Some(&byte) = bytes.get(cursor) {
            if byte == b')' {
                break;
            }
            cursor += 1;
        }
        let compact = source[option_start..cursor]
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        match compact.as_str() {
            "{eager:true}" => true,
            "{eager:false}" => false,
            _ => {
                return Err(glob_error(
                    "glob options must be the literal `{ eager: true }`",
                ));
            }
        }
    } else {
        false
    };
    skip_whitespace(bytes, &mut cursor);
    if bytes.get(cursor) != Some(&b')') {
        return Err(glob_error(
            "glob call must contain one literal pattern and optional eager flag",
        ));
    }
    Ok(ParsedCall {
        pattern,
        eager,
        end: cursor + 1,
    })
}

fn collect_matches(pattern: &Path, watch_root: &Path) -> Result<Vec<PathBuf>> {
    if !watch_root.is_dir() {
        return Ok(Vec::new());
    }
    let pattern = slash(pattern);
    let mut pending = vec![watch_root.to_path_buf()];
    let mut matches = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if !is_ignored_directory(&path) {
                    pending.push(path);
                }
            } else if glob_matches(&pattern, &slash(&path)) {
                matches.push(path);
            }
        }
    }
    Ok(matches)
}

fn glob_watch_root(pattern: &Path, project_root: &Path) -> PathBuf {
    let text = pattern.to_string_lossy();
    let wildcard = text.find(['*', '?']).unwrap_or(text.len());
    let prefix_text = &text[..wildcard];
    let ends_at_directory = prefix_text.ends_with(['/', '\\']);
    let prefix = PathBuf::from(prefix_text.trim_end_matches(['/', '\\']));
    let mut candidate = if ends_at_directory {
        prefix
    } else {
        prefix.parent().unwrap_or(project_root).to_path_buf()
    };
    while !candidate.is_dir() && path_is_inside_lexically(&candidate, project_root) {
        if !candidate.pop() {
            break;
        }
    }
    if path_is_inside_lexically(&candidate, project_root) {
        candidate
    } else {
        project_root.to_path_buf()
    }
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    fn matches(pattern: &[u8], value: &[u8]) -> bool {
        match pattern.first() {
            None => value.is_empty(),
            Some(b'*') if pattern.get(1) == Some(&b'*') && pattern.get(2) == Some(&b'/') => {
                matches(&pattern[3..], value)
                    || (!value.is_empty() && matches(pattern, &value[1..]))
            }
            Some(b'*') if pattern.get(1) == Some(&b'*') => {
                let rest = &pattern[2..];
                matches(rest, value) || (!value.is_empty() && matches(pattern, &value[1..]))
            }
            Some(b'*') => {
                matches(&pattern[1..], value)
                    || (!value.is_empty() && value[0] != b'/' && matches(pattern, &value[1..]))
            }
            Some(b'?') => {
                !value.is_empty() && value[0] != b'/' && matches(&pattern[1..], &value[1..])
            }
            Some(expected) => {
                value.first() == Some(expected) && matches(&pattern[1..], &value[1..])
            }
        }
    }
    matches(pattern.as_bytes(), value.as_bytes())
}

fn relative_specifier(importer_dir: &Path, file: &Path) -> String {
    let importer = normalize_without_io(importer_dir);
    let file = normalize_without_io(file);
    let importer_components = importer.components().collect::<Vec<_>>();
    let file_components = file.components().collect::<Vec<_>>();
    let common = importer_components
        .iter()
        .zip(file_components.iter())
        .take_while(|(left, right)| left == right)
        .count();

    // Both paths are project-contained and should therefore share their root.
    // Keep a safe absolute fallback for synthetic unit-test paths.
    if common == 0 {
        return slash(&file);
    }
    let mut parts = vec!["..".to_string(); importer_components.len() - common];
    parts.extend(
        file_components[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    let relative = parts.join("/");
    if relative.starts_with('.') {
        relative
    } else {
        format!("./{relative}")
    }
}

fn normalize_without_io(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn path_is_inside_lexically(path: &Path, root: &Path) -> bool {
    let path = normalize_without_io(path);
    let root = normalize_without_io(root);
    path == root || path.starts_with(root)
}

fn is_ignored_directory(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | ".ruvyxa" | "node_modules" | "target" | "dist")
    )
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn skip_whitespace(bytes: &[u8], cursor: &mut usize) {
    while bytes.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
        *cursor += 1;
    }
}

fn glob_error(message: impl Into<String>) -> BundleError {
    BundleError::Compiler(format!("RUV1810 import.meta.glob: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "ruvyxa-glob-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn wildcard_matching_distinguishes_one_and_many_segments() {
        assert!(glob_matches("/a/*.ts", "/a/x.ts"));
        assert!(!glob_matches("/a/*.ts", "/a/n/x.ts"));
        assert!(glob_matches("/a/**/*.ts", "/a/n/x.ts"));
        assert!(glob_matches("/a/**/*.ts", "/a/x.ts"));
    }

    #[test]
    fn relative_specifier_reaches_sibling_directories() {
        let root = Path::new("C:/project");
        assert_eq!(
            relative_specifier(&root.join("features/blog"), &root.join("shared/post.ts")),
            "../../shared/post.ts"
        );
    }

    #[test]
    fn dotted_directory_is_the_watch_root() {
        let root = temp_directory("dotted-watch-root");
        let directory = root.join("content.v1");
        fs::create_dir_all(&directory).unwrap();
        let pattern = directory.join("*.ts");
        assert_eq!(glob_watch_root(&pattern, &root), directory);
        fs::remove_dir_all(root).unwrap();
    }
}
