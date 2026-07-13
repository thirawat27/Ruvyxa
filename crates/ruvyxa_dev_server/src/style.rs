//! Dependency-driven global style collection for development and production rendering.

use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ruvyxa_bundler::ast::parse_module;
use ruvyxa_bundler::resolver::{TsConfigPaths, resolve_specifier};
use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use walkdir::WalkDir;

const SCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"];
const PREPROCESSOR_EXTENSIONS: &[&str] = &["scss", "sass", "less"];

/// Styles and source files that contributed to a rendered document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleCollection {
    pub css: String,
    pub files: Vec<PathBuf>,
}

/// Collect imported and explicitly configured global stylesheet entries.
pub fn collect_styles(root: &Path, app_dir: &Path, entries: &[PathBuf]) -> Result<StyleCollection> {
    let root = absolute_path(root)?;
    let app_dir = absolute_path(app_dir)?;
    let tsconfig = TsConfigPaths::load(&root);
    let mut scripts = VecDeque::new();
    let mut style_seeds = Vec::new();

    collect_application_seeds(&app_dir, &mut scripts);
    for entry in entries {
        collect_explicit_entry(&root, entry, &mut style_seeds)?;
    }

    let mut visited_scripts = BTreeSet::new();
    while let Some(script) = scripts.pop_front() {
        let script = canonical_or_original(script);
        if !visited_scripts.insert(script.clone()) {
            continue;
        }
        let source = fs::read_to_string(&script)?;
        let base_dir = script.parent().unwrap_or(&root);
        for import in parse_module(&source).imports {
            let specifier = strip_import_suffix(&import.specifier);
            if is_css_specifier(specifier) {
                let resolved =
                    resolve_style_import(&root, base_dir, specifier).ok_or_else(|| {
                        Diagnostic::new("RUV1403", "Stylesheet import could not be resolved")
                            .explain(format!(
                                "`{specifier}` is imported from {}.",
                                script.display()
                            ))
                            .at_file(&script)
                            .suggest(
                                "Check the path or add a project-relative `css.entries` value.",
                            )
                    })?;
                style_seeds.push(resolved);
            } else if is_preprocessor_specifier(specifier) {
                return Err(unsupported_preprocessor(&script, specifier));
            } else if let Some(resolved) =
                resolve_script_import(&root, base_dir, specifier, &tsconfig)
                && is_within_project(&root, &resolved)
            {
                scripts.push_back(resolved);
            }
        }
    }

    let mut visited_styles = BTreeSet::new();
    let mut files = Vec::new();
    let mut css = String::new();
    for style in style_seeds {
        append_style(&root, &style, &mut visited_styles, &mut files, &mut css)?;
    }

    Ok(StyleCollection {
        css: escape_style_end_tags(&css),
        files,
    })
}

fn collect_application_seeds(app_dir: &Path, scripts: &mut VecDeque<PathBuf>) {
    let mut files = WalkDir::new(app_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    files.sort();

    for file in files {
        if has_extension(&file, SCRIPT_EXTENSIONS) {
            scripts.push_back(file);
        }
    }
}

fn collect_explicit_entry(root: &Path, entry: &Path, styles: &mut Vec<PathBuf>) -> Result<()> {
    let entry = if entry.is_absolute() {
        entry.to_path_buf()
    } else {
        root.join(entry)
    };
    if !is_within_project(root, &entry) {
        return Err(
            Diagnostic::new("RUV1404", "CSS entry must stay inside the project root")
                .at_file(&entry)
                .suggest("Use a project-relative file or directory in `css.entries`.")
                .into(),
        );
    }
    if entry.is_dir() {
        let mut files = WalkDir::new(&entry)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|item| item.file_type().is_file() && has_extension(item.path(), &["css"]))
            .map(|item| item.into_path())
            .collect::<Vec<_>>();
        files.sort();
        styles.extend(files);
        return Ok(());
    }
    if !entry.is_file() {
        return Err(
            Diagnostic::new("RUV1403", "Configured CSS entry was not found")
                .at_file(&entry)
                .suggest("Point `css.entries` at an existing .css file or directory.")
                .into(),
        );
    }
    if !has_extension(&entry, &["css"]) {
        return Err(unsupported_preprocessor(
            &entry,
            entry.to_string_lossy().as_ref(),
        ));
    }
    styles.push(entry);
    Ok(())
}

fn append_style(
    root: &Path,
    file: &Path,
    visited: &mut BTreeSet<PathBuf>,
    files: &mut Vec<PathBuf>,
    output: &mut String,
) -> Result<()> {
    let file = canonical_or_original(file.to_path_buf());
    if !visited.insert(file.clone()) {
        return Ok(());
    }

    let source = fs::read_to_string(&file)?;
    if imports_tailwind(&source) {
        output.push_str(&compile_tailwind_css(root, &file)?);
        output.push('\n');
        files.push(file);
        return Ok(());
    }

    let imports = css_imports(&source);
    for specifier in &imports {
        if is_remote_style(specifier) {
            continue;
        } else if is_css_specifier(specifier) {
            let base_dir = file.parent().unwrap_or(root);
            let dependency = resolve_style_import(root, base_dir, specifier).ok_or_else(|| {
                Diagnostic::new("RUV1403", "CSS @import could not be resolved")
                    .explain(format!(
                        "`{specifier}` is imported from {}.",
                        file.display()
                    ))
                    .at_file(&file)
            })?;
            append_style(root, &dependency, visited, files, output)?;
        } else if is_preprocessor_specifier(specifier) {
            return Err(unsupported_preprocessor(&file, specifier));
        }
    }

    output.push_str(&remove_local_css_imports(&source, &imports));
    output.push('\n');
    files.push(file);
    Ok(())
}

fn resolve_script_import(
    root: &Path,
    base_dir: &Path,
    specifier: &str,
    tsconfig: &TsConfigPaths,
) -> Option<PathBuf> {
    if specifier.starts_with('.') {
        return resolve_specifier(base_dir, specifier);
    }
    if specifier.starts_with('/') {
        return resolve_specifier(root, specifier.trim_start_matches('/'));
    }
    tsconfig
        .resolve(specifier)
        .or_else(|| resolve_specifier(root, specifier))
}

fn resolve_style_import(root: &Path, base_dir: &Path, specifier: &str) -> Option<PathBuf> {
    let candidate = if specifier.starts_with('.') {
        base_dir.join(specifier)
    } else if specifier.starts_with('/') {
        root.join(specifier.trim_start_matches('/'))
    } else {
        if let Some(mapped) = TsConfigPaths::load(root).resolve(specifier)
            && mapped.is_file()
        {
            return Some(canonical_or_original(mapped));
        }
        let project_file = root.join(specifier);
        if project_file.is_file() {
            project_file
        } else {
            root.join("node_modules").join(specifier)
        }
    };
    candidate
        .is_file()
        .then(|| canonical_or_original(candidate))
}

fn css_imports(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("@import") {
                return None;
            }
            let rest = trimmed.trim_start_matches("@import").trim_start();
            let rest = rest
                .strip_prefix("url(")
                .map(str::trim_start)
                .unwrap_or(rest);
            let quote = rest.chars().next()?;
            if quote != '\'' && quote != '"' {
                return None;
            }
            let end = rest[1..].find(quote)? + 1;
            Some(rest[1..end].to_string())
        })
        .collect()
}

fn remove_local_css_imports(source: &str, imports: &[String]) -> String {
    source
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("@import")
                || !imports.iter().any(|specifier| {
                    !is_remote_style(specifier)
                        && (is_css_specifier(specifier) || is_preprocessor_specifier(specifier))
                        && trimmed.contains(specifier)
                })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn imports_tailwind(source: &str) -> bool {
    source.contains("@import \"tailwindcss\"") || source.contains("@import 'tailwindcss'")
}

fn compile_tailwind_css(root: &Path, input: &Path) -> Result<String> {
    let tailwind = find_tailwind_cli(root).ok_or_else(|| {
        Diagnostic::new("RUV1401", "Tailwind CSS CLI was not found")
            .explain("A CSS file imports `tailwindcss`, but Ruvyxa could not find `@tailwindcss/cli` in node_modules.")
            .at_file(input)
            .suggest("Install Tailwind support with `pnpm add tailwindcss && pnpm add -D @tailwindcss/cli`.")
    })?;
    let input_arg = input.strip_prefix(root).unwrap_or(input);
    let output = Command::new(tailwind)
        .current_dir(root)
        .arg("-i")
        .arg(input_arg)
        .arg("--minify")
        .output()
        .map_err(|source| RuvyxaError::Io {
            message: "Failed to run Tailwind CSS CLI".to_string(),
            source,
        })?;

    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map_err(|error| RuvyxaError::Message(error.to_string()));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(Diagnostic::new("RUV1400", "Tailwind CSS compilation failed")
        .explain(stderr.trim())
        .at_file(input)
        .suggest("Check Tailwind directives, content sources, and installed Tailwind package versions.")
        .into())
}

fn find_tailwind_cli(root: &Path) -> Option<PathBuf> {
    let binary = if cfg!(windows) {
        "tailwindcss.cmd"
    } else {
        "tailwindcss"
    };
    [
        root.join("node_modules/.bin").join(binary),
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join("node_modules/.bin").join(binary))
            .unwrap_or_default(),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn unsupported_preprocessor(file: &Path, specifier: &str) -> RuvyxaError {
    Diagnostic::new(
        "RUV1402",
        "CSS preprocessor requires an explicit transform plugin",
    )
    .explain(format!(
        "Ruvyxa cannot safely treat `{specifier}` as plain CSS."
    ))
    .at_file(file)
    .suggest("Compile Sass/Less to CSS first, or add a Ruvyxa transform plugin for that syntax.")
    .into()
}

fn strip_import_suffix(specifier: &str) -> &str {
    specifier.split(['?', '#']).next().unwrap_or(specifier)
}

fn is_css_specifier(specifier: &str) -> bool {
    Path::new(specifier)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
}

fn is_preprocessor_specifier(specifier: &str) -> bool {
    Path::new(specifier)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            PREPROCESSOR_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
}

fn is_remote_style(specifier: &str) -> bool {
    specifier.starts_with("http://")
        || specifier.starts_with("https://")
        || specifier.starts_with("//")
        || specifier.starts_with("data:")
}

fn escape_style_end_tags(css: &str) -> String {
    let mut escaped = css.to_string();
    let mut offset = 0;
    while let Some(index) = escaped[offset..].to_ascii_lowercase().find("</style") {
        let index = offset + index;
        escaped.replace_range(index..index + 2, "<\\/");
        offset = index + 3;
    }
    escaped
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension.to_ascii_lowercase().as_str()))
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(canonical_or_original(path.to_path_buf()))
    } else {
        Ok(canonical_or_original(std::env::current_dir()?.join(path)))
    }
}

fn canonical_or_original(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn is_within_project(root: &Path, path: &Path) -> bool {
    let root = canonical_or_original(root.to_path_buf());
    let path = canonical_or_original(path.to_path_buf());
    path.strip_prefix(root).is_ok_and(|relative| {
        !relative.starts_with("node_modules")
            && !relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
    })
}

// ─────────────────────────────────────────────
// CSS Minification
// ─────────────────────────────────────────────

/// Minify CSS by stripping comments, collapsing whitespace, and removing
/// unnecessary spaces around selectors and punctuation.
///
/// This is intentionally conservative — it preserves content inside strings
/// and `url()` values, and does not attempt shorthand merging or selector
/// optimisation.
pub fn minify_css(source: &str) -> String {
    let no_comments = strip_css_comments(source);
    collapse_css_whitespace(&no_comments)
}

/// Remove `/* ... */` block comments from CSS, respecting string literals.
fn strip_css_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // String literal: preserve contents verbatim.
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            out.push(quote as char);
            i += 1;
            while i < len && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < len {
                    out.push(bytes[i] as char);
                    i += 1;
                }
                out.push(bytes[i] as char);
                i += 1;
            }
            if i < len {
                out.push(bytes[i] as char);
                i += 1;
            }
            continue;
        }

        // Block comment start.
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Skip until closing `*/`.
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < len {
                i += 2; // skip `*/`
            }
            continue;
        }

        out.push(bytes[i] as char);
        i += 1;
    }

    out
}

/// Collapse runs of whitespace and remove spaces around CSS punctuation.
fn collapse_css_whitespace(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut prev_space = false;
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Preserve string literals verbatim.
        if ch == '"' || ch == '\'' {
            // Flush pending space only if output doesn't already end with punctuation.
            if prev_space && !out.is_empty() && !ends_with_css_punct(&out) {
                out.push(' ');
            }
            prev_space = false;
            out.push(ch);
            i += 1;
            while i < len && chars[i] != ch {
                if chars[i] == '\\' && i + 1 < len {
                    out.push(chars[i]);
                    i += 1;
                }
                out.push(chars[i]);
                i += 1;
            }
            if i < len {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }

        if ch == ' ' || ch == '\n' || ch == '\r' || ch == '\t' {
            prev_space = true;
            i += 1;
            continue;
        }

        // CSS punctuation: remove surrounding spaces.
        if is_css_punct(ch) {
            if prev_space && !out.is_empty() && !ends_with_css_punct(&out) {
                // Only keep the space if removing it would merge identifiers/values
                // incorrectly — but for CSS punctuation it's always safe to drop.
            }
            prev_space = false;
            // Trim trailing space before punctuation.
            if out.ends_with(' ') {
                out.pop();
            }
            out.push(ch);
            i += 1;
            continue;
        }

        // Normal character.
        if prev_space && !out.is_empty() && !ends_with_css_punct(&out) {
            out.push(' ');
        }
        prev_space = false;
        out.push(ch);
        i += 1;
    }

    out
}

fn is_css_punct(ch: char) -> bool {
    matches!(ch, '{' | '}' | ':' | ';' | ',' | '(' | ')')
}

fn ends_with_css_punct(s: &str) -> bool {
    s.chars().last().is_some_and(is_css_punct)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_imported_css_outside_app_and_nested_css_imports() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        let components = root.join("components");
        let styles = root.join("styles");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&components).unwrap();
        fs::create_dir_all(&styles).unwrap();
        fs::write(
            app.join("page.tsx"),
            "import '../components/card'\nexport default 1",
        )
        .unwrap();
        fs::write(components.join("card.ts"), "import '../styles/site.css'").unwrap();
        fs::write(
            styles.join("site.css"),
            "@import './tokens.css';\n.card { color: red; }",
        )
        .unwrap();
        fs::write(styles.join("tokens.css"), ":root { --space: 1rem; }").unwrap();

        let collection = collect_styles(root, &app, &[]).unwrap();

        assert!(collection.css.contains("--space: 1rem"));
        assert!(collection.css.contains(".card { color: red; }"));
        assert!(!collection.css.contains("@import './tokens.css'"));
        assert_eq!(collection.files.len(), 2);
    }

    #[test]
    fn collects_only_imported_app_css() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("page.tsx"), "import './global.css'").unwrap();
        fs::write(app.join("global.css"), "body { margin: 0; }").unwrap();
        fs::write(app.join("unused.css"), ".unused { display: none; }").unwrap();

        let collection = collect_styles(root, &app, &[]).unwrap();

        assert_eq!(collection.css.matches("body { margin: 0; }").count(), 1);
        assert!(!collection.css.contains(".unused"));
        assert_eq!(collection.files.len(), 1);
    }

    #[test]
    fn collects_explicit_file_and_directory_entries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        let themes = root.join("themes");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&themes).unwrap();
        fs::write(app.join("page.tsx"), "export default 1").unwrap();
        fs::write(themes.join("dark.css"), "html { color-scheme: dark; }").unwrap();

        let collection = collect_styles(root, &app, &[PathBuf::from("themes")]).unwrap();

        assert!(collection.css.contains("color-scheme: dark"));
        assert_eq!(collection.files.len(), 1);
    }

    #[test]
    fn preserves_remote_imports_and_escapes_inline_style_end_tags() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "import './global.css'\nexport default 1",
        )
        .unwrap();
        fs::write(
            app.join("global.css"),
            "@import \"https://example.com/theme.css\";\n.bad::after { content: \"</STYLE>\"; }",
        )
        .unwrap();

        let collection = collect_styles(root, &app, &[]).unwrap();

        assert!(collection.css.contains("https://example.com/theme.css"));
        assert!(collection.css.contains("<\\/STYLE>"));
        assert!(!collection.css.to_ascii_lowercase().contains("</style"));
    }

    #[test]
    fn resolves_css_imports_through_tsconfig_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        let styles = root.join("styles");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&styles).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@styles/*":["styles/*"]}}}"#,
        )
        .unwrap();
        fs::write(app.join("page.tsx"), "import '@styles/theme.css'").unwrap();
        fs::write(styles.join("theme.css"), ".theme { color: navy; }").unwrap();

        let collection = collect_styles(root, &app, &[]).unwrap();

        assert!(collection.css.contains(".theme { color: navy; }"));
    }
}
