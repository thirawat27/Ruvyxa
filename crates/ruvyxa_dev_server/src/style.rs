//! Dependency-driven global style collection for development and production rendering.

use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ruvyxa_bundler::ast::parse_module;
use ruvyxa_bundler::resolver::{TsConfigPaths, resolve_specifier};
use ruvyxa_bundler::style_module::{
    compile_sass_file, is_css_module_path, is_sass_path, scope_css_module,
};
use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use walkdir::WalkDir;

use crate::JavaScriptRuntime;

const SCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"];
const PREPROCESSOR_EXTENSIONS: &[&str] = &["scss", "sass", "less"];

/// Styles and source files that contributed to a rendered document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleCollection {
    pub css: String,
    pub files: Vec<PathBuf>,
}

/// Collect imported and explicitly configured global stylesheet entries.
///
/// Runs the project's PostCSS chain in development mode. Use
/// [`collect_styles_for_build`] for production output.
///
/// `runtime` is the project's JavaScript runtime, and it is a parameter rather
/// than a default because the PostCSS chain is a JavaScript program: a project
/// on Bun or Deno without Node installed had its stylesheet fail while every
/// other JavaScript stage of the same build ran fine.
pub fn collect_styles(
    root: &Path,
    app_dir: &Path,
    entries: &[PathBuf],
    runtime: JavaScriptRuntime,
) -> Result<StyleCollection> {
    collect_styles_in_mode(root, app_dir, entries, runtime, false)
}

/// [`collect_styles`] with PostCSS plugins told they are running a production
/// build, which is what selects a plugin's production behavior — minification,
/// dead-rule removal, `NODE_ENV`-conditional plugin lists.
pub fn collect_styles_for_build(
    root: &Path,
    app_dir: &Path,
    entries: &[PathBuf],
    runtime: JavaScriptRuntime,
) -> Result<StyleCollection> {
    collect_styles_in_mode(root, app_dir, entries, runtime, true)
}

fn collect_styles_in_mode(
    root: &Path,
    app_dir: &Path,
    entries: &[PathBuf],
    runtime: JavaScriptRuntime,
    production: bool,
) -> Result<StyleCollection> {
    let root = absolute_path(root)?;
    let app_dir = absolute_path(app_dir)?;
    let tsconfig = TsConfigPaths::load(&root);
    let postcss = crate::PostcssRunner::detect(&root, production, runtime)?;
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
            if is_css_specifier(specifier) || is_sass_specifier(specifier) {
                let resolved = resolve_style_import(&root, base_dir, specifier, &tsconfig)
                    .ok_or_else(|| {
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

    let mut walk = StyleWalk::new(&root, &tsconfig);
    walk.postcss = postcss.is_some();
    for style in style_seeds {
        // PostCSS runs per entry, over the CSS that entry contributed once its
        // local `@import`s are inlined. Per entry rather than per file, because a
        // plugin chain applied to every imported partial would run Tailwind once
        // per partial; over the whole collection would lose the entry path that
        // plugins resolve their content globs against.
        let start = walk.css.len();
        append_style(&mut walk, &style)?;
        let Some(runner) = &postcss else { continue };
        if walk.css.len() == start {
            continue;
        }
        let transformed = runner.run(&walk.css[start..], &style)?;
        walk.css.truncate(start);
        walk.css.push_str(&transformed.css);
        walk.css.push('\n');
        for dependency in transformed.dependencies {
            walk.record_file(dependency);
        }
    }
    if let Some(runner) = &postcss {
        // The configuration itself is a build input: changing the plugin list
        // has to invalidate the stylesheet in development.
        walk.record_file(runner.config_file().to_path_buf());
    }

    Ok(StyleCollection {
        css: escape_style_end_tags(&walk.css),
        files: walk.files,
    })
}

/// Accumulator shared by every stylesheet reached during one collection.
///
/// The seed loop, the CSS `@import` recursion, and the Sass dependency walk all
/// contribute to the same output, so the deduplication state has to be shared
/// between them rather than rebuilt per stylesheet.
struct StyleWalk<'a> {
    root: &'a Path,
    /// Loaded once by the caller. Resolving a bare stylesheet specifier used to
    /// re-read and re-parse `tsconfig.json` from disk on every occurrence.
    tsconfig: &'a TsConfigPaths,
    /// Stylesheets already appended to `css`.
    visited: BTreeSet<PathBuf>,
    /// Membership index for `files`. A linear `Vec::contains` per contributing
    /// file made recording the file list quadratic in the number of
    /// stylesheets.
    file_index: BTreeSet<PathBuf>,
    /// Sass files whose import graph has already been walked.
    sass_walked: BTreeSet<PathBuf>,
    files: Vec<PathBuf>,
    css: String,
    /// Whether the caller runs a PostCSS chain over each entry afterwards. The
    /// built-in Tailwind CLI shortcut stands down when it does.
    postcss: bool,
}

impl<'a> StyleWalk<'a> {
    fn new(root: &'a Path, tsconfig: &'a TsConfigPaths) -> Self {
        Self {
            root,
            tsconfig,
            visited: BTreeSet::new(),
            file_index: BTreeSet::new(),
            sass_walked: BTreeSet::new(),
            files: Vec::new(),
            css: String::new(),
            postcss: false,
        }
    }

    /// Record a file as contributing to the collection, at most once.
    fn record_file(&mut self, file: PathBuf) {
        if self.file_index.insert(file.clone()) {
            self.files.push(file);
        }
    }

    /// Record every file reachable through a Sass entry's import graph.
    ///
    /// `sass_walked` spans the whole collection rather than one entry. Shared
    /// partials are the normal shape of a Sass project, and re-reading each of
    /// them once per importing stylesheet was the cost this walk was paying. A
    /// partial skipped here as already walked was already recorded by whichever
    /// entry reached it first, so the recorded set is unchanged.
    fn collect_sass_dependencies(&mut self, entry: &Path) {
        let root = self.root;
        let mut pending = vec![canonical_or_original(entry.to_path_buf())];
        let mut discovered = BTreeSet::new();

        while let Some(file) = pending.pop() {
            if !self.sass_walked.insert(file.clone()) {
                continue;
            }
            discovered.insert(file.clone());
            let Ok(source) = fs::read_to_string(&file) else {
                continue;
            };
            let base_dir = file.parent().unwrap_or(root);
            for specifier in sass_imports(&source) {
                if specifier.starts_with("sass:") || is_remote_style(&specifier) {
                    continue;
                }
                if let Some(dependency) = resolve_sass_import(root, base_dir, &specifier) {
                    pending.push(dependency);
                }
            }
        }

        for file in discovered {
            self.record_file(file);
        }
    }
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
            .filter(|item| {
                item.file_type().is_file() && has_extension(item.path(), &["css", "scss", "sass"])
            })
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
    if !has_extension(&entry, &["css", "scss", "sass"]) {
        return Err(unsupported_preprocessor(
            &entry,
            entry.to_string_lossy().as_ref(),
        ));
    }
    styles.push(entry);
    Ok(())
}

fn append_style(walk: &mut StyleWalk<'_>, file: &Path) -> Result<()> {
    let root = walk.root;
    let tsconfig = walk.tsconfig;
    let file = canonical_or_original(file.to_path_buf());
    if !walk.visited.insert(file.clone()) {
        return Ok(());
    }

    let source = fs::read_to_string(&file)?;
    // The Tailwind CLI path predates the PostCSS stage and stays for projects
    // that install `@tailwindcss/cli` without a PostCSS config. A project with a
    // config gets Tailwind through its own `@tailwindcss/postcss` plugin, and
    // running both would compile the stylesheet twice.
    if !walk.postcss && imports_tailwind(&source) {
        let compiled = compile_tailwind_css(root, &file)?;
        walk.css.push_str(&compiled);
        walk.css.push('\n');
        walk.record_file(file);
        return Ok(());
    }

    let source = if is_sass_path(&file) {
        walk.collect_sass_dependencies(&file);
        compile_sass_file(&file, root).map_err(|error| {
            Diagnostic::new("RUV1402", "Sass compilation failed")
                .explain(error)
                .at_file(&file)
                .suggest("Check Sass syntax and imported partial paths.")
        })?
    } else {
        source
    };

    let source = if is_css_module_path(&file) {
        scope_css_module(&source, &file, root).css
    } else {
        source
    };

    // One comment scan feeds both the import collection and the removal below.
    let mask = css_code_mask(&source);
    let imports = css_imports(&source, &mask);
    for specifier in &imports {
        if is_remote_style(specifier) {
            continue;
        } else if is_css_specifier(specifier) {
            let base_dir = file.parent().unwrap_or(root);
            let dependency =
                resolve_style_import(root, base_dir, specifier, tsconfig).ok_or_else(|| {
                    Diagnostic::new("RUV1403", "CSS @import could not be resolved")
                        .explain(format!(
                            "`{specifier}` is imported from {}.",
                            file.display()
                        ))
                        .at_file(&file)
                })?;
            append_style(walk, &dependency)?;
        } else if is_preprocessor_specifier(specifier) {
            return Err(unsupported_preprocessor(&file, specifier));
        }
    }

    walk.css
        .push_str(&remove_local_css_imports(&source, &mask, &imports));
    walk.css.push('\n');
    walk.record_file(file);
    Ok(())
}

fn sass_imports(source: &str) -> Vec<String> {
    let characters = source.chars().collect::<Vec<_>>();
    let mut imports = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] == '/' && characters.get(index + 1) == Some(&'/') {
            index += 2;
            while index < characters.len() && characters[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if characters[index] == '/' && characters.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < characters.len()
                && !(characters[index] == '*' && characters[index + 1] == '/')
            {
                index += 1;
            }
            index = (index + 2).min(characters.len());
            continue;
        }
        let directive = ["@forward", "@import", "@use"]
            .into_iter()
            .find(|directive| {
                let expected = directive.chars().collect::<Vec<_>>();
                characters[index..].starts_with(&expected)
                    && characters
                        .get(index + expected.len())
                        .is_none_or(|character| character.is_whitespace())
            });
        let Some(directive) = directive else {
            index += 1;
            continue;
        };
        index += directive.len();
        while index < characters.len() && characters[index] != ';' {
            if matches!(characters[index], '\'' | '"') {
                let quote = characters[index];
                index += 1;
                let start = index;
                while index < characters.len() && characters[index] != quote {
                    if characters[index] == '\\' {
                        index = (index + 2).min(characters.len());
                    } else {
                        index += 1;
                    }
                }
                if index <= characters.len() {
                    imports.push(characters[start..index].iter().collect());
                }
            }
            index += 1;
        }
        index += usize::from(index < characters.len());
    }
    imports
}

fn resolve_sass_import(root: &Path, base_dir: &Path, specifier: &str) -> Option<PathBuf> {
    let base = if specifier.starts_with('.') {
        base_dir.join(specifier)
    } else if specifier.starts_with('/') {
        root.join(specifier.trim_start_matches('/'))
    } else {
        root.join("node_modules").join(specifier)
    };
    let parent = base.parent().unwrap_or(base_dir);
    let name = base.file_name()?.to_string_lossy();
    let mut candidates = vec![base.clone()];
    if base.extension().is_none() {
        candidates.extend([
            base.with_extension("scss"),
            base.with_extension("sass"),
            parent.join(format!("_{name}.scss")),
            parent.join(format!("_{name}.sass")),
            base.join("index.scss"),
            base.join("_index.scss"),
            base.join("index.sass"),
            base.join("_index.sass"),
        ]);
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(canonical_or_original)
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

/// Resolve a stylesheet specifier.
///
/// `tsconfig` is passed in rather than loaded here: this runs once per import
/// occurrence, and loading it locally re-read and re-parsed the project's
/// `tsconfig.json` every time.
fn resolve_style_import(
    root: &Path,
    base_dir: &Path,
    specifier: &str,
    tsconfig: &TsConfigPaths,
) -> Option<PathBuf> {
    let candidate = if specifier.starts_with('.') {
        base_dir.join(specifier)
    } else if specifier.starts_with('/') {
        root.join(specifier.trim_start_matches('/'))
    } else {
        if let Some(mapped) = tsconfig.resolve(specifier)
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

/// `@import` specifiers declared by this stylesheet.
///
/// `mask` gates each line on being real code. Without it a block-commented
/// import was followed like a live one: the target got inlined, and when the
/// file it named had since been deleted the build failed with `RUV1403`
/// pointing inside a comment. Commenting a block of imports out and removing
/// the files is ordinary work; it must not break the build.
fn css_imports(source: &str, mask: &[bool]) -> Vec<String> {
    css_lines_with_offsets(source)
        .filter(|(line, start)| line_is_code(mask, *start, line))
        .filter_map(|(line, _)| {
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

/// Drop the `@import` lines whose target was inlined above.
///
/// Judges "is this line an import" with the same `mask` [`css_imports`] used, so
/// the two cannot disagree about which lines are code. A line only the remover
/// recognised would delete a commented-out import out of its comment; a line
/// only the collector recognised would inline a stylesheet and leave the
/// browser-level `@import` behind to fetch it a second time.
fn remove_local_css_imports(source: &str, mask: &[bool], imports: &[String]) -> String {
    css_lines_with_offsets(source)
        .filter(|(line, start)| {
            let trimmed = line.trim_start();
            !line_is_code(mask, *start, line)
                || !trimmed.starts_with("@import")
                || !imports.iter().any(|specifier| {
                    !is_remote_style(specifier)
                        && (is_css_specifier(specifier) || is_preprocessor_specifier(specifier))
                        && trimmed.contains(specifier)
                })
        })
        .map(|(line, _)| line)
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
    let mut command = Command::new(tailwind);
    command
        .current_dir(root)
        .arg("-i")
        .arg(input_arg)
        .arg("--minify");
    let output =
        crate::process::output_with_timeout(&mut command, crate::process::STYLE_TOOL_TIMEOUT)
            .map_err(|error| match error {
                crate::process::ProcessError::Io(source) => RuvyxaError::Io {
                    message: "Failed to run Tailwind CSS CLI".to_string(),
                    source,
                },
                timed_out => RuvyxaError::Message(format!("Tailwind CSS compilation {timed_out}")),
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

fn is_sass_specifier(specifier: &str) -> bool {
    Path::new(specifier)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("scss") || extension.eq_ignore_ascii_case("sass")
        })
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
    ruvyxa_diagnostics::normalized_canonical_path(&path)
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

/// Byte ranges of `source` that are CSS code rather than comment text.
///
/// The one place this file decides where a `/* … */` begins and ends. Both
/// consumers — comment stripping for minification and the `@import` scan — read
/// the answer from here, because they disagreed before: the Sass scanner skipped
/// comments and the CSS one did not, so a commented-out `@import` was still
/// followed. Every delimiter involved is ASCII, and a UTF-8 continuation byte is
/// always `>= 0x80`, so scanning bytes cannot mistake part of a character for
/// one of them.
fn css_code_spans(source: &str) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;

    while index < len {
        // A `/*` inside a string is content, so strings are skipped whole.
        if bytes[index] == b'"' || bytes[index] == b'\'' {
            let quote = bytes[index];
            index += 1;
            while index < len && bytes[index] != quote {
                index += if bytes[index] == b'\\' { 2 } else { 1 };
            }
            index = (index + 1).min(len);
            continue;
        }

        if index + 1 < len && bytes[index] == b'/' && bytes[index + 1] == b'*' {
            if index > start {
                spans.push((start, index));
            }
            index += 2;
            while index + 1 < len && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(len);
            start = index;
            continue;
        }

        index += 1;
    }

    if start < len {
        spans.push((start, len));
    }
    spans
}

/// Per-byte "this is code" view of `source`, for the line-oriented scans below.
fn css_code_mask(source: &str) -> Vec<bool> {
    let mut mask = vec![false; source.len()];
    for (start, end) in css_code_spans(source) {
        mask[start..end].fill(true);
    }
    mask
}

/// Whether the first non-space byte of the line starting at `line_start` is code.
fn line_is_code(mask: &[bool], line_start: usize, line: &str) -> bool {
    let indent = line.len() - line.trim_start().len();
    mask.get(line_start + indent).copied().unwrap_or(false)
}

/// Each line of `source` with the byte offset it starts at.
///
/// `str::lines` strips a trailing `\r`, so a CRLF file advances by two bytes
/// between lines and a bare `\n` file by one. Getting that wrong shifts every
/// offset after the first CRLF and the mask lookups stop naming the right byte.
fn css_lines_with_offsets(source: &str) -> impl Iterator<Item = (&str, usize)> {
    let mut offset = 0usize;
    source.lines().map(move |line| {
        let start = offset;
        offset += line.len()
            + if source[offset + line.len()..].starts_with("\r\n") {
                2
            } else {
                1
            };
        (line, start)
    })
}

/// Remove `/* ... */` block comments from CSS, respecting string literals.
///
/// Copies byte *slices* rather than casting each byte to a `char`. The previous
/// `bytes[i] as char` reinterpreted every UTF-8 continuation byte as a Latin-1
/// code point and re-encoded it, so `content: "→"` came out of a production
/// build as `content: "â†’"` — every non-ASCII character in every minified
/// stylesheet, including arrows, bullets, and non-Latin font names.
fn strip_css_comments(source: &str) -> String {
    let spans = css_code_spans(source);
    let mut out = String::with_capacity(source.len());
    for (start, end) in spans {
        out.push_str(&source[start..end]);
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

    /// Minification must not rewrite the characters it is compressing.
    ///
    /// The comment stripper copied `bytes[i] as char`, which reads each UTF-8
    /// continuation byte as a Latin-1 code point and re-encodes it. Every
    /// non-ASCII character in every minified stylesheet came out as mojibake —
    /// `content: "→"` shipped as `content: "â†’"`, and non-Latin font names with
    /// it. Only production builds minify, so it was invisible in dev.
    #[test]
    fn minification_preserves_non_ascii_content() {
        let source = ".a::after { content: \"\u{2192}\" }\n.b { font-family: \"\u{0e44}\u{0e17}\u{0e22}\" }\n";

        let minified = minify_css(source);

        assert!(
            minified.contains('\u{2192}'),
            "an arrow in `content` must survive: {minified:?}"
        );
        assert!(
            minified.contains("\u{0e44}\u{0e17}\u{0e22}"),
            "a non-Latin font name must survive: {minified:?}"
        );
    }

    /// A comment survives minification's removal but must keep its bytes intact
    /// on the way through everything else.
    #[test]
    fn comment_stripping_keeps_surrounding_bytes_and_respects_strings() {
        assert_eq!(
            strip_css_comments(".a{content:\"/* not a comment */\"}/* real */\n.b{}"),
            ".a{content:\"/* not a comment */\"}\n.b{}",
            "a comment sequence inside a string is content"
        );
        assert_eq!(
            strip_css_comments(".a{content:\"\u{2192}\"}"),
            ".a{content:\"\u{2192}\"}"
        );
    }

    /// A commented-out `@import` is not an import.
    ///
    /// The CSS scanner matched any line starting with `@import`, including one
    /// inside a block comment, while the Sass scanner beside it skipped comments
    /// correctly. Following the dead import inlined a stylesheet nobody asked
    /// for, and — once the file it named was deleted — failed the build with
    /// `RUV1403` pointing at a line inside a comment.
    #[test]
    fn a_commented_out_import_is_neither_followed_nor_removed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("page.tsx"), "import './global.css'").unwrap();
        fs::write(
            app.join("global.css"),
            "/*\n@import \"./deleted.css\";\n*/\n@import \"./live.css\";\nbody { margin: 0 }\n",
        )
        .unwrap();
        fs::write(app.join("live.css"), ".live { color: red }").unwrap();

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node)
            .expect("a dead import must not fail");

        assert!(
            collection.css.contains(".live { color: red }"),
            "the live import must still be inlined: {}",
            collection.css
        );
        assert!(
            !collection.css.contains("@import \"./live.css\""),
            "the live import line must be removed once inlined: {}",
            collection.css
        );
        assert!(
            collection.css.contains("@import \"./deleted.css\""),
            "the commented line is not an import and must be left alone: {}",
            collection.css
        );
    }

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

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();

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

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();

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

        let collection = collect_styles(
            root,
            &app,
            &[PathBuf::from("themes")],
            JavaScriptRuntime::Node,
        )
        .unwrap();

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

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();

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

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();

        assert!(collection.css.contains(".theme { color: navy; }"));
    }

    /// The nested `@import` path resolves aliases through the `TsConfigPaths`
    /// carried by the walk. Only the top-level script-import path was covered
    /// before, so a regression in the carried config would have gone unnoticed.
    #[test]
    fn resolves_aliased_css_imports_nested_inside_a_stylesheet() {
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
        fs::write(app.join("page.tsx"), "import './entry.css'").unwrap();
        fs::write(
            app.join("entry.css"),
            "@import '@styles/tokens.css';\n.entry { color: navy; }",
        )
        .unwrap();
        fs::write(styles.join("tokens.css"), ".tokens { color: teal; }").unwrap();

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();

        assert!(collection.css.contains(".tokens { color: teal; }"));
        assert!(collection.css.contains(".entry { color: navy; }"));
        assert!(
            collection
                .files
                .iter()
                .any(|file| file.ends_with("tokens.css"))
        );
    }

    /// A partial shared by several entries is walked once and recorded once.
    /// Sharing the Sass traversal state across entries must not drop it from
    /// the dependency list, and must not let it be recorded twice.
    #[test]
    fn records_a_shared_sass_partial_exactly_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "import './left.scss'; import './right.scss'",
        )
        .unwrap();
        fs::write(app.join("_shared.scss"), "$accent: rebeccapurple;").unwrap();
        fs::write(
            app.join("left.scss"),
            "@use './shared' as s; .left { color: s.$accent; }",
        )
        .unwrap();
        fs::write(
            app.join("right.scss"),
            "@use './shared' as s; .right { color: s.$accent; }",
        )
        .unwrap();

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();

        assert!(collection.css.contains(".left"));
        assert!(collection.css.contains(".right"));

        let shared = collection
            .files
            .iter()
            .filter(|file| file.ends_with("_shared.scss"))
            .count();
        assert_eq!(shared, 1, "shared partial must be recorded once");

        let unique: BTreeSet<&PathBuf> = collection.files.iter().collect();
        assert_eq!(
            unique.len(),
            collection.files.len(),
            "dependency list must not contain duplicates"
        );
    }

    #[test]
    fn compiles_scss_and_scopes_css_module_selectors() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "import styles from './card.module.scss'; export default styles.card",
        )
        .unwrap();
        fs::write(app.join("_tokens.scss"), "$accent: rebeccapurple;").unwrap();
        let module_path = app.join("card.module.scss");
        fs::write(
            &module_path,
            "@use './tokens' as t; .card { color: t.$accent; .title { font-weight: 700; } }",
        )
        .unwrap();

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();
        let expected = scope_css_module(
            &compile_sass_file(&module_path, root).unwrap(),
            &module_path,
            root,
        );

        assert!(
            collection
                .css
                .contains(&format!(".{}", expected.classes["card"]))
        );
        assert!(
            collection
                .css
                .contains(&format!(".{}", expected.classes["title"]))
        );
        assert!(collection.css.contains("rebeccapurple"));
        assert!(
            collection
                .files
                .iter()
                .any(|file| file.ends_with("_tokens.scss"))
        );
    }

    /// A project fixture inside the repository.
    ///
    /// The PostCSS stage resolves `postcss` and every plugin from the *project*,
    /// walking up from its root — the same resolution an installed application
    /// gets. A fixture in the OS temp directory would find nothing.
    fn in_repo_project(label: &str) -> tempfile::TempDir {
        let target = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .canonicalize()
            .expect("the cargo target directory exists during a test run");
        tempfile::Builder::new()
            .prefix(&format!("ruvyxa-postcss-{label}-"))
            .tempdir_in(target)
            .unwrap()
    }

    /// The reported incident, end to end: a project declares PostCSS, and the
    /// collected global stylesheet reaches the document transformed.
    #[test]
    fn a_declared_postcss_chain_transforms_the_collected_stylesheet() {
        let temp = in_repo_project("runs");
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("layout.tsx"), "import './globals.css'").unwrap();
        // `@import "theme.css"` is inlined by this pipeline before PostCSS runs,
        // so the plugin must see the partial's rules too.
        fs::write(
            app.join("globals.css"),
            "@import \"./theme.css\";\n.from { color: red }\n",
        )
        .unwrap();
        fs::write(app.join("theme.css"), ".from-theme { color: blue }\n").unwrap();
        fs::write(
            root.join("postcss.config.mjs"),
            "export default {\n  plugins: [{\n    postcssPlugin: 'rename',\n    \
             Rule(rule) { rule.selector = rule.selector.replace('.from', '.renamed') },\n  }],\n}\n",
        )
        .unwrap();

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();

        assert!(collection.css.contains(".renamed"), "{}", collection.css);
        assert!(
            collection.css.contains(".renamed-theme"),
            "an inlined @import must reach the plugin chain: {}",
            collection.css
        );
        assert!(!collection.css.contains(".from "), "{}", collection.css);
        assert!(
            collection
                .files
                .iter()
                .any(|file| file.ends_with("postcss.config.mjs")),
            "the config is a build input, so a plugin change must invalidate the stylesheet"
        );
    }

    /// A project with no PostCSS configuration must get exactly the pipeline it
    /// had before this stage existed.
    #[test]
    fn a_project_without_postcss_is_left_alone() {
        let temp = in_repo_project("absent");
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("layout.tsx"), "import './globals.css'").unwrap();
        fs::write(app.join("globals.css"), ".from { color: red }\n").unwrap();

        let collection = collect_styles(root, &app, &[], JavaScriptRuntime::Node).unwrap();

        assert_eq!(collection.css.trim(), ".from { color: red }");
    }

    /// A plugin failure stops the build. Emitting the untransformed stylesheet
    /// instead is what shipped an unstyled page to production.
    #[test]
    fn a_failing_plugin_fails_the_collection_rather_than_emitting_raw_css() {
        let temp = in_repo_project("fails");
        let root = temp.path();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("layout.tsx"), "import './globals.css'").unwrap();
        fs::write(app.join("globals.css"), ".a { color: red }\n").unwrap();
        fs::write(
            root.join("postcss.config.mjs"),
            "export default { plugins: [{ postcssPlugin: 'explode', \
             Once() { throw new Error('plugin exploded') } }] }\n",
        )
        .unwrap();

        let error = collect_styles(root, &app, &[], JavaScriptRuntime::Node)
            .expect_err("a plugin failure must not be swallowed into raw CSS");
        let message = error.to_string();
        assert!(message.contains("RUV1406"), "{message}");
        assert!(message.contains("plugin exploded"), "{message}");
    }
}
