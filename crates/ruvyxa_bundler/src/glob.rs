//! Compile-time `import.meta.glob` expansion.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ast;
use crate::resolver::ResolveGraphCache;
use crate::{BundleError, Result};

#[derive(Debug, Default)]
pub(crate) struct GlobExpansion {
    pub source: String,
    pub matches: Vec<PathBuf>,
    pub watch_roots: Vec<PathBuf>,
}

#[cfg(test)]
thread_local! {
    /// How many times this thread has parsed a module inside
    /// [`expand_import_meta_glob`].
    ///
    /// The pass reads the scanner only to ask `is_code_offset` about a marker
    /// position, so a module with no marker in it never needs the parse at
    /// all — and essentially no module has one. Without the guard every module
    /// in every route's graph paid a full byte scan for nothing, on top of the
    /// scan `collect_deps_uncached` already performs on the same text.
    ///
    /// A skipped parse leaves no trace in the pass's output, so removing the
    /// guard again would be silent. This counter is what makes it loud:
    /// `the_marker_guard_skips_the_parse_for_a_module_without_one` fails the
    /// moment the parse moves back above the search.
    ///
    /// Thread-local rather than a process-global counter, because the harness
    /// runs tests on concurrent threads and a shared counter would answer with
    /// whatever a sibling test happened to expand.
    static PARSED_MODULES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn parsed_modules_on_this_thread() -> usize {
    PARSED_MODULES.with(std::cell::Cell::get)
}

/// Parse a module for this pass, recording the parse in test builds.
fn parse_counted(source: &str) -> ast::ModuleAst {
    #[cfg(test)]
    PARSED_MODULES.with(|count| count.set(count.get() + 1));
    ast::parse_module(source)
}

#[cfg(test)]
thread_local! {
    /// How many directory walks this thread has started for a glob pattern.
    ///
    /// The walk itself is memoized on the resolver cache, and a memo hit leaves
    /// no trace in the expansion it hands back — the source, the matches, and
    /// the watch roots are identical either way. This counter is what makes the
    /// memo's removal loud, the same way `PARSED_MODULES` holds the marker
    /// guard.
    static DIRECTORY_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn directory_walks_on_this_thread() -> usize {
    DIRECTORY_WALKS.with(std::cell::Cell::get)
}

pub(crate) fn expand_import_meta_glob(
    source: &str,
    importer_dir: &Path,
    project_root: &Path,
    cache: &ResolveGraphCache,
    resolve_pattern: impl Fn(&str) -> Option<PathBuf>,
) -> Result<GlobExpansion> {
    const MARKER: &str = "import.meta.glob";
    // Search before parsing. The scanner is consulted only to ask
    // `is_code_offset` about a marker position, so a module with no marker in
    // its bytes has nothing to ask — and that is essentially every module, on
    // every route, including every `node_modules` file a client graph reaches.
    // The two neighbouring passes over the same scanner already guard this way.
    if !source.contains(MARKER) {
        return Ok(GlobExpansion {
            source: source.to_string(),
            ..Default::default()
        });
    }
    let ast = parse_counted(source);
    let mut replacements = Vec::new();
    let mut hoisted_imports: Vec<String> = Vec::new();
    let mut all_matches = Vec::new();
    let mut watch_roots = Vec::new();
    let mut cursor = 0;
    let mut call_index = 0usize;

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
        // Walked once per (pattern, watch root) for the life of this cache. The
        // ordered, deduplicated list is what is memoized, not the raw walk: the
        // sort is part of the answer every caller wants, and repeating it per
        // module would be work the memo exists to remove.
        let matches = cache.matched_glob_files(&absolute_pattern, &watch_root, || {
            let mut matches = collect_matches(&absolute_pattern, &watch_root)?;
            // Sort and dedup on the *same* key. `slash` normalizes separators,
            // so two paths that differ only by `\` vs `/` sort adjacent but are
            // not `PathBuf`-equal — a plain `dedup()` let them both through and
            // emitted the same specifier twice in the generated object literal.
            // `sort_by_cached_key` also computes each key once instead of once
            // per comparison.
            matches.sort_by_cached_key(|path| slash(path));
            matches.dedup_by_key(|path| slash(path));
            Ok(matches)
        })?;

        let entries = matches
            .iter()
            .enumerate()
            .map(|(match_index, file)| {
                let specifier = relative_specifier(importer_dir, file);
                let key = serde_json::to_string(&specifier).expect("path string serializes");
                let import = serde_json::to_string(&specifier).expect("path string serializes");
                if parsed.eager {
                    // Eager matches belong in the static dependency graph, so
                    // they lower to a namespace import rather than a
                    // `require()` call. `require` is undefined in an ES module
                    // and only this crate's linker rewrites it, so emitting it
                    // made eager globs throw at runtime on the JavaScript graph.
                    let binding = format!("__ruvyxaGlob{call_index}_{match_index}");
                    hoisted_imports.push(format!("import * as {binding} from {import}"));
                    format!("{key}: {binding}")
                } else {
                    format!("{key}: () => import({import})")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        replacements.push((start, parsed.end, format!("{{{entries}}}")));
        all_matches.extend(matches.iter().cloned());
        watch_roots.push(watch_root);
        cursor = parsed.end;
        call_index += 1;
    }

    let mut expanded = source.to_string();
    for (start, end, replacement) in replacements.into_iter().rev() {
        expanded.replace_range(start..end, &replacement);
    }
    if !hoisted_imports.is_empty() {
        // Insert after the directive prologue: above it would demote a
        // `'use client'` directive to a plain string, and below every use would
        // put the linker's rewritten `const` binding in the temporal dead zone.
        // One import per line keeps the line-based linker able to see them.
        let insert_at = crate::reference_manifest::directive_prologue_end(&expanded);
        expanded.insert_str(insert_at, &format!("\n{}\n", hoisted_imports.join("\n")));
    }
    all_matches.sort_by_key(|path| slash(path));
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
    #[cfg(test)]
    DIRECTORY_WALKS.with(|count| count.set(count.get() + 1));
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

    /// The ordering and lowering halves of the cross-language glob contract.
    ///
    /// This replays `tests/fixtures/glob-contract.json` directly rather than
    /// asserting a hand-written expectation, so the Rust expander and the
    /// JavaScript expander in `packages/ruvyxa/runtime/glob.mjs` cannot drift.
    #[test]
    fn replays_the_cross_language_ordering_and_lowering_contract() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/glob-contract.json"))
                .unwrap();
        assert_eq!(contract["contract"], "ruvyxa.glob");
        assert_eq!(contract["schemaVersion"], 2);
        let ordering = &contract["ordering"];

        let root = temp_directory("ordering-contract");
        let directory = root.join(ordering["directory"].as_str().unwrap());
        fs::create_dir_all(&directory).unwrap();
        for file in ordering["files"].as_array().unwrap() {
            fs::write(
                directory.join(file.as_str().unwrap()),
                "export const value = 1;",
            )
            .unwrap();
        }

        let pattern = ordering["pattern"].as_str().unwrap();
        let expansion = expand_import_meta_glob(
            &format!("export const all = import.meta.glob('{pattern}');"),
            &root,
            &root,
            &ResolveGraphCache::new(),
            |_| None,
        )
        .unwrap();

        let expected: Vec<String> = ordering["expectedKeyOrder"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect();
        let actual = expected
            .iter()
            .map(|key| {
                expansion
                    .source
                    .find(&format!("\"{key}\""))
                    .unwrap_or_else(|| panic!("missing key {key} in {}", expansion.source))
            })
            .collect::<Vec<_>>();
        let mut sorted = actual.clone();
        sorted.sort_unstable();
        assert_eq!(
            actual, sorted,
            "glob keys must follow code-unit order, not locale order: {}",
            expansion.source
        );

        // Eager matches lower to namespace imports; `require(` must never appear.
        let eager = expand_import_meta_glob(
            &format!("export const all = import.meta.glob('{pattern}', {{ eager: true }});"),
            &root,
            &root,
            &ResolveGraphCache::new(),
            |_| None,
        )
        .unwrap();
        for forbidden in contract["lowering"]["forbiddenInOutput"]
            .as_array()
            .unwrap()
        {
            assert!(
                !eager.source.contains(forbidden.as_str().unwrap()),
                "eager lowering must not emit {}: {}",
                forbidden,
                eager.source
            );
        }
        assert!(
            eager.source.contains("import * as __ruvyxaGlob0_0 from"),
            "eager lowering must hoist a namespace import: {}",
            eager.source
        );

        // A `'use client'` directive must stay the first statement in the
        // module: generated imports placed above it would demote it to a plain
        // string and the server/client boundary check would stop seeing it.
        let directive = expand_import_meta_glob(
            &format!(
                "'use client'\nexport const all = import.meta.glob('{pattern}', {{ eager: true }});"
            ),
            &root,
            &root,
            &ResolveGraphCache::new(),
            |_| None,
        )
        .unwrap();
        assert!(
            directive.source.trim_start().starts_with("'use client'"),
            "generated imports must not displace the directive prologue: {}",
            directive.source
        );
        assert!(
            crate::reference_manifest::has_module_directive(&directive.source, "use client"),
            "the expanded module must still declare its client boundary: {}",
            directive.source
        );

        // A regex literal containing a quote must not hide the call after it.
        let guarded = format!(
            "{}\nexport const all = import.meta.glob('{pattern}');",
            contract["scanning"]["mustExpandAfter"].as_str().unwrap()
        );
        let scanned =
            expand_import_meta_glob(&guarded, &root, &root, &ResolveGraphCache::new(), |_| None)
                .unwrap();
        assert!(
            !scanned.source.contains("import.meta.glob"),
            "a regex literal must not hide a later glob call: {}",
            scanned.source
        );

        fs::remove_dir_all(root).unwrap();
    }

    /// The pass does no work for a module that carries no glob call.
    ///
    /// Every non-JSON, non-content, non-external module in every route's graph
    /// reaches this function, which for a client bundle includes every
    /// `node_modules` file. The scanner's facts are needed only to ask
    /// `is_code_offset` about a marker position, so parsing before searching was
    /// one full byte scan of every module for a marker essentially none of them
    /// carry.
    #[test]
    fn the_marker_guard_skips_the_parse_for_a_module_without_one() {
        let root = temp_directory("marker-guard");
        let source = "export const value = 1;\nexport function render() { return null }\n";

        let before = parsed_modules_on_this_thread();
        let expansion =
            expand_import_meta_glob(source, &root, &root, &ResolveGraphCache::new(), |_| None)
                .unwrap();
        assert_eq!(
            parsed_modules_on_this_thread(),
            before,
            "a module with no `import.meta.glob` must not be parsed"
        );

        // Behaviour-preserving: the guard returns the same expansion the walk
        // would have produced for a module with no call in it.
        assert_eq!(expansion.source, source);
        assert!(expansion.matches.is_empty());
        assert!(expansion.watch_roots.is_empty());

        // And a module that *does* carry the marker still parses, so the guard
        // cannot be "passed" by never parsing at all.
        let directory = root.join("content");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("a.ts"), "export const value = 1;").unwrap();
        let before = parsed_modules_on_this_thread();
        expand_import_meta_glob(
            "export const all = import.meta.glob('./content/*.ts');",
            &root,
            &root,
            &ResolveGraphCache::new(),
            |_| None,
        )
        .unwrap();
        assert_eq!(
            parsed_modules_on_this_thread(),
            before + 1,
            "a module carrying the marker still needs the scanner"
        );

        fs::remove_dir_all(root).unwrap();
    }

    /// One build walks a glob's directory tree once, however many modules and
    /// routes name the pattern.
    ///
    /// The expander runs per module per route, so a content site globbing
    /// `./content/**/*.md` re-walked that whole tree once per route and paid it
    /// again on every incremental rebuild — a non-empty `watch_roots` disables
    /// persistent dependency-edge reuse, so the pass never gets to skip.
    #[test]
    fn a_glob_pattern_is_walked_once_for_the_life_of_one_cache() {
        let root = temp_directory("glob-memo");
        let directory = root.join("content");
        fs::create_dir_all(&directory).unwrap();
        for name in ["a.ts", "b.ts"] {
            fs::write(directory.join(name), "export const value = 1;").unwrap();
        }
        let source = "export const all = import.meta.glob('./content/*.ts');";
        let cache = ResolveGraphCache::new();

        let before = directory_walks_on_this_thread();
        let first = expand_import_meta_glob(source, &root, &root, &cache, |_| None).unwrap();
        assert_eq!(
            directory_walks_on_this_thread(),
            before + 1,
            "the first module to name a pattern walks it"
        );

        let second = expand_import_meta_glob(source, &root, &root, &cache, |_| None).unwrap();
        assert_eq!(
            directory_walks_on_this_thread(),
            before + 1,
            "a second module naming the same pattern must reuse the walk"
        );
        // A memo hit is indistinguishable from a walk in what it produces.
        assert_eq!(first.source, second.source);
        assert_eq!(first.matches, second.matches);
        assert_eq!(first.watch_roots, second.watch_roots);
        assert_eq!(first.matches.len(), 2);

        // A different cache is a different build. Keying the memo here rather
        // than on a process global is what keeps `dev` able to see a file that
        // appeared between two routes.
        expand_import_meta_glob(source, &root, &root, &ResolveGraphCache::new(), |_| None).unwrap();
        assert_eq!(
            directory_walks_on_this_thread(),
            before + 2,
            "a fresh cache must walk again rather than answer from another build"
        );

        fs::remove_dir_all(root).unwrap();
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
