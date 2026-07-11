//! Ruvyxa Native TypeScript/JSX Compiler
//!
//! A pure-Rust, zero-external-dependency TypeScript and JSX transformer
//! designed specifically for the Ruvyxa framework.
//!
//! ## Strategy
//!
//! Rather than pulling in a full AST-based parser this compiler uses a
//! **character-level streaming transformer** inspired by Sucrase. It handles
//! the TypeScript constructs that appear in real Ruvyxa apps without a full
//! grammar.
//!
//! ### TypeScript stripping
//! - Type annotations: `foo: string`, `bar: Map<string, number>`
//! - Generic type parameters on functions and calls: `<T>`, `<T extends U>`
//! - Interface/type declarations: `interface Foo {}`, `type Bar = …`
//! - `import type` and `export type` declarations
//! - Access modifiers on class members: `public`, `private`, `protected`
//! - `as` casts: `expr as Type`
//! - Non-null assertions: `expr!`
//! - `satisfies` expressions: `expr satisfies Type`
//! - TypeScript decorators: `@Decorator`
//! - Enum declarations: `enum Foo { A, B }`  → `const Foo = { A: 0, B: 1 }`
//!
//! ### JSX transformation (two modes)
//!
//! **Classic mode** (`jsxRuntime = "classic"`, default):
//! - `<Component prop={…}>…</Component>` → `React.createElement(Component, {prop: …}, …)`
//!
//! **Automatic mode** (`jsxRuntime = "automatic"`, React 17+):
//! - `<Component prop={…}>…</Component>` → `_jsx(Component, { prop: …, children: … })`
//! - Automatically injects `import { jsx as _jsx, jsxs as _jsxs, Fragment as _Fragment } from "react/jsx-runtime"`

use std::collections::BTreeMap;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::ast;
use crate::cache::{CacheLookup, CompileCache};
use crate::plugin::{PluginContext, PluginPipeline};
use crate::resolver::ResolvedModule;
use crate::{BundleError, BundleInput, JsxRuntime, Result};
use ruvyxa_diagnostics::{Diagnostic, SourceSpan};

/// A compiled module: TypeScript/JSX has been converted to plain JS.
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// Canonical path (or virtual label for the synthetic entry).
    pub path: PathBuf,
    /// Plain JavaScript source after TS stripping and JSX transform.
    pub js: String,
    /// Dependency paths preserved from the resolver stage.
    pub deps: Vec<PathBuf>,
    /// Whether this module comes from `node_modules` (external).
    pub is_external: bool,
    /// Whether this module's compiled output came from the compile cache.
    pub cache_hit: bool,
}

struct CompiledModuleOutput {
    module: CompiledModule,
    plugin_source_map: Option<String>,
}

/// Compile every module in the resolved graph.
pub fn compile_graph(graph: &[ResolvedModule], input: &BundleInput) -> Result<Vec<CompiledModule>> {
    let cache = CompileCache::new(&input.project_root, true);
    compile_graph_with_cache(graph, input, &cache)
}

/// Compile every module in the resolved graph, using the provided cache.
///
/// Modules are compiled in parallel using rayon's work-stealing thread pool.
/// Each module is independent at this stage (deps are resolved in the prior
/// step), so compilation is embarrassingly parallel.
pub fn compile_graph_with_cache(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
) -> Result<Vec<CompiledModule>> {
    compile_graph_with_pipeline(graph, input, cache, &PluginPipeline::empty())
}

/// Compile every module using the provided cache and native plugin pipeline.
pub fn compile_graph_with_pipeline(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
    plugins: &PluginPipeline,
) -> Result<Vec<CompiledModule>> {
    Ok(compile_graph_with_pipeline_and_maps(graph, input, cache, plugins)?.0)
}

pub(crate) fn compile_graph_with_pipeline_and_maps(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
    plugins: &PluginPipeline,
) -> Result<(Vec<CompiledModule>, BTreeMap<PathBuf, String>)> {
    let results: Vec<Result<CompiledModuleOutput>> = graph
        .par_iter()
        .map(|module| compile_module(module, input, cache, plugins))
        .collect();

    let mut modules = Vec::with_capacity(results.len());
    let mut source_maps = BTreeMap::new();
    for output in results {
        let output = output?;
        if let Some(source_map) = output.plugin_source_map {
            source_maps.insert(output.module.path.clone(), source_map);
        }
        modules.push(output.module);
    }
    Ok((modules, source_maps))
}

/// A compiler error with source location information.
#[derive(Debug, Clone)]
pub struct CompilerError {
    /// Human-readable error message.
    pub message: String,
    /// 1-based line number in the source file.
    pub line: u32,
    /// 1-based column number in the source file.
    pub column: u32,
    /// Path to the file that caused the error.
    pub file: PathBuf,
}

impl CompilerError {
    /// Convert this compiler error into a structured [`Diagnostic`].
    pub fn to_diagnostic(&self) -> Diagnostic {
        Diagnostic::new("RUV1300", format!("Compile error: {}", self.message))
            .explain(format!(
                "The TypeScript/JSX compiler encountered an error at line {}, column {}.",
                self.line, self.column
            ))
            .at_file(&self.file)
            .suggest("Check the syntax at the indicated position.")
    }
}

/// Compile the graph with error recovery: failed modules are replaced with
/// stub modules that emit a runtime error, and compile errors are collected
/// as diagnostics instead of aborting the build.
pub fn compile_graph_resilient(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<CompiledModule> {
    let results: Vec<(usize, Result<CompiledModule>)> = graph
        .par_iter()
        .enumerate()
        .map(|(idx, module)| {
            (
                idx,
                compile_module(module, input, cache, &PluginPipeline::empty())
                    .map(|output| output.module),
            )
        })
        .collect();

    let mut compiled = Vec::with_capacity(graph.len());

    for (idx, result) in results {
        match result {
            Ok(module) => compiled.push(module),
            Err(err) => {
                let source_module = &graph[idx];

                let diagnostic = match &err {
                    BundleError::Compiler(msg) => {
                        let (line, col) = parse_error_location(msg);
                        let mut diag = Diagnostic::new(
                            "RUV1300",
                            format!("Compile error in {}", source_module.path.display()),
                        )
                        .explain(msg.clone())
                        .suggest("Fix the syntax error and rebuild.");

                        diag.span = Some(SourceSpan {
                            file: source_module.path.clone(),
                            line: Some(line),
                            column: Some(col),
                        });
                        diag
                    }
                    BundleError::Unresolved { specifier, importer } => {
                        Diagnostic::new(
                            "RUV1302",
                            format!("Cannot resolve '{specifier}' from {}", importer.display()),
                        )
                        .at_file(&source_module.path)
                        .suggest(format!(
                            "Check that the module '{specifier}' exists and the import path is correct."
                        ))
                    }
                    other => {
                        Diagnostic::new("RUV1301", format!("Module error: {other}"))
                            .at_file(&source_module.path)
                    }
                };

                diagnostics.push(diagnostic);

                let error_msg = format!(
                    "Ruvyxa compile error in {}: {}",
                    source_module.path.display(),
                    err
                );
                let stub_js = format!(
                    "console.error({}); throw new Error({});",
                    serde_json::to_string(&error_msg)
                        .unwrap_or_else(|_| "\"compile error\"".into()),
                    serde_json::to_string(&error_msg)
                        .unwrap_or_else(|_| "\"compile error\"".into()),
                );

                compiled.push(CompiledModule {
                    path: source_module.path.clone(),
                    js: stub_js,
                    deps: Vec::new(),
                    is_external: source_module.is_external,
                    cache_hit: false,
                });
            }
        }
    }

    compiled
}

/// Parse line:col from an error message like "file.tsx:5:12: unexpected token"
fn parse_error_location(msg: &str) -> (u32, u32) {
    let parts: Vec<&str> = msg.splitn(4, ':').collect();
    if parts.len() >= 3 {
        if let (Ok(line), Ok(col)) = (
            parts[1].trim().parse::<u32>(),
            parts[2].trim().parse::<u32>(),
        ) {
            if line > 0 && col > 0 {
                return (line, col);
            }
        }
    }
    if let Some(line_idx) = msg.find("line ") {
        let after_line = &msg[line_idx + 5..];
        let line_str: String = after_line
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(line) = line_str.parse::<u32>() {
            if let Some(col_idx) = msg.find("column ") {
                let after_col = &msg[col_idx + 7..];
                let col_str: String = after_col
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(col) = col_str.parse::<u32>() {
                    return (line, col);
                }
            }
            return (line, 1);
        }
    }
    (1, 1)
}

fn compile_module(
    module: &ResolvedModule,
    input: &BundleInput,
    cache: &CompileCache,
    plugins: &PluginPipeline,
) -> Result<CompiledModuleOutput> {
    let ext = module
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let content_source = if matches!(ext, "md" | "mdx") {
        crate::content::compile_content_module(&module.source, &module.path)
            .map_err(BundleError::Compiler)?
    } else {
        module.source.clone()
    };

    let plugin_ctx = PluginContext {
        project_root: input.project_root.clone(),
        importer: Some(module.path.clone()),
        target: input.target,
    };
    let plugin_output = plugins.transform_with_map(&content_source, &module.path, &plugin_ctx)?;
    let source = plugin_output.code;
    let plugin_source_map = plugin_output.map;

    // Virtual entry (label starts with "ruvyxa:") or plain JS: pass through
    // after native plugin transforms.
    if matches!(ext, "js" | "mjs" | "cjs") || module.path.to_string_lossy().contains("ruvyxa:") {
        return Ok(CompiledModuleOutput {
            module: CompiledModule {
                path: module.path.clone(),
                js: source,
                deps: module.deps.clone(),
                is_external: module.is_external,
                cache_hit: false,
            },
            plugin_source_map,
        });
    }

    let transform_plan = ast::parse_module(&source);
    let has_jsx = matches!(ext, "tsx" | "jsx") || transform_plan.has_jsx;
    let jsx_runtime = input.options.jsx_runtime;

    // Cache key includes JSX runtime mode so switching modes invalidates entries.
    match cache.lookup_with_options(&source, has_jsx, jsx_runtime) {
        CacheLookup::Hit(cached_js) => Ok(CompiledModuleOutput {
            module: CompiledModule {
                path: module.path.clone(),
                js: cached_js,
                deps: module.deps.clone(),
                is_external: module.is_external,
                cache_hit: true,
            },
            plugin_source_map,
        }),
        CacheLookup::Miss(key) => {
            let js = transform_with_options(&source, has_jsx, jsx_runtime).map_err(|msg| {
                BundleError::Compiler(format!("{}: {}", module.path.display(), msg))
            })?;

            cache.store(&key, &js);

            Ok(CompiledModuleOutput {
                module: CompiledModule {
                    path: module.path.clone(),
                    js,
                    deps: module.deps.clone(),
                    is_external: module.is_external,
                    cache_hit: false,
                },
                plugin_source_map,
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Core transformer entry points
// ─────────────────────────────────────────────────────────────────────────────

/// Transform TypeScript/JSX source to plain JavaScript (classic JSX mode).
pub fn transform(source: &str, has_jsx: bool) -> std::result::Result<String, String> {
    transform_with_options(source, has_jsx, JsxRuntime::Classic)
}

/// Transform with explicit JSX runtime selection.
pub fn transform_with_options(
    source: &str,
    has_jsx: bool,
    jsx_runtime: JsxRuntime,
) -> std::result::Result<String, String> {
    // Step 1: strip decorators (they appear before class/method declarations).
    let step0 = strip_decorators(source);
    // Step 2: expand `const enum` / `enum` to object literals.
    let step1 = expand_enums(&step0)?;
    // Step 3: strip TypeScript-specific syntax.
    let stripped = strip_typescript(&step1)?;
    if has_jsx {
        match jsx_runtime {
            JsxRuntime::Classic => transform_jsx_classic(&stripped),
            JsxRuntime::Automatic => transform_jsx_automatic(&stripped),
        }
    } else {
        Ok(stripped)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pre-pass: decorator stripping
// ─────────────────────────────────────────────────────────────────────────────

/// Strip TypeScript/TC39 decorators: `@Foo`, `@Foo(args)`, `@ns.Bar`.
/// Decorators appear on their own lines before class and method declarations.
fn strip_decorators(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Inside strings – preserve verbatim.
        if chars[i] == '"' || chars[i] == '\'' || chars[i] == '`' {
            let q = chars[i];
            out.push(q);
            i += 1;
            while i < len {
                if chars[i] == '\\' && i + 1 < len {
                    out.push(chars[i]);
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                out.push(chars[i]);
                if chars[i] == q {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // `@` at the start of a line (after optional whitespace) is a decorator.
        if chars[i] == '@' {
            // Check that this is a decorator and not the `@` in an email or comment.
            // Heuristic: preceded only by whitespace/newlines on this line.
            let is_decorator = {
                let mut j = i.wrapping_sub(1);
                let mut ok = true;
                while j < i {
                    if chars[j] == '\n' {
                        break;
                    }
                    if chars[j] != ' ' && chars[j] != '\t' {
                        ok = false;
                        break;
                    }
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                }
                ok
            };

            if is_decorator {
                // Consume `@identifier(.…)` including optional call arguments.
                i += 1; // skip @
                        // Skip identifier (may be dotted).
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
                {
                    i += 1;
                }
                // Skip optional arguments `(…)`.
                if i < len && chars[i] == '(' {
                    let mut depth = 1i32;
                    i += 1;
                    while i < len && depth > 0 {
                        match chars[i] {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            '"' | '\'' | '`' => {
                                let q = chars[i];
                                i += 1;
                                while i < len {
                                    if chars[i] == '\\' && i + 1 < len {
                                        i += 2;
                                        continue;
                                    }
                                    if chars[i] == q {
                                        break;
                                    }
                                    i += 1;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                // Emit a blank line to preserve line numbers.
                out.push('\n');
                continue;
            }
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Pre-pass: enum expansion
// ─────────────────────────────────────────────────────────────────────────────

/// Expand `enum Foo { A, B = 5, C }` → `const Foo = { A: 0, B: 5, C: 6 };`.
/// Also handles `const enum` (same output — we don't inline at call sites).
fn expand_enums(source: &str) -> std::result::Result<String, String> {
    let mut out = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // String literals — preserve verbatim.
        if chars[i] == '"' || chars[i] == '\'' || chars[i] == '`' {
            let q = chars[i];
            out.push(q);
            i += 1;
            while i < len {
                if chars[i] == '\\' && i + 1 < len {
                    out.push(chars[i]);
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                out.push(chars[i]);
                if chars[i] == q {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // `const enum` or `enum`
        let is_const_enum = is_keyword_at(&chars, i, "const") && {
            let j = skip_spaces(&chars, i + 5);
            is_keyword_at(&chars, j, "enum")
        };
        let is_plain_enum = !is_const_enum && is_keyword_at(&chars, i, "enum");

        if is_const_enum || is_plain_enum {
            // Skip `const ` if present.
            if is_const_enum {
                i += 5; // "const"
                i = skip_all_whitespace(&chars, i);
            }
            i += 4; // "enum"
            i = skip_all_whitespace(&chars, i);

            // Read enum name.
            let name_start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let name: String = chars[name_start..i].iter().collect();

            i = skip_all_whitespace(&chars, i);
            if i >= len || chars[i] != '{' {
                return Err(format!("expected '{{' after enum name '{name}'"));
            }
            i += 1; // skip {

            // Parse members.
            let mut members: Vec<(String, Option<i64>)> = Vec::new();
            let mut next_value: i64 = 0;

            loop {
                i = skip_all_whitespace(&chars, i);
                if i >= len || chars[i] == '}' {
                    break;
                }
                // Skip trailing comma.
                if chars[i] == ',' {
                    i += 1;
                    continue;
                }
                // Skip line comments.
                if chars[i] == '/' && i + 1 < len && chars[i + 1] == '/' {
                    while i < len && chars[i] != '\n' {
                        i += 1;
                    }
                    continue;
                }
                // Member name.
                let mname_start = i;
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '"')
                {
                    i += 1;
                }
                let mname: String = chars[mname_start..i].iter().collect();
                if mname.is_empty() {
                    return Err(format!("expected enum member near character {i}"));
                }
                i = skip_all_whitespace(&chars, i);

                let value = if i < len && chars[i] == '=' {
                    i += 1;
                    i = skip_all_whitespace(&chars, i);
                    // Parse a simple integer or negative integer literal.
                    let neg = i < len && chars[i] == '-';
                    if neg {
                        i += 1;
                    }
                    let num_start = i;
                    while i < len && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    let num_str: String = chars[num_start..i].iter().collect();
                    num_str
                        .parse::<i64>()
                        .ok()
                        .map(|v| if neg { -v } else { v })
                } else {
                    Some(next_value)
                };

                let v = value.unwrap_or(next_value);
                next_value = v + 1;
                members.push((mname, Some(v)));

                i = skip_all_whitespace(&chars, i);
                if i < len && chars[i] == ',' {
                    i += 1;
                }
            }

            if i < len && chars[i] == '}' {
                i += 1;
            }

            // Emit `const Name = { A: 0, B: 1, … };`
            out.push_str("const ");
            out.push_str(&name);
            out.push_str(" = {");
            for (idx, (mname, mval)) in members.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                out.push_str(mname);
                out.push_str(": ");
                if let Some(v) = mval {
                    out.push_str(&v.to_string());
                } else {
                    out.push_str("undefined");
                }
            }
            out.push_str("};");
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 1 – TypeScript stripping
// ─────────────────────────────────────────────────────────────────────────────

fn strip_typescript(source: &str) -> std::result::Result<String, String> {
    let mut out = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Skip string literals (preserve content verbatim).
        if chars[i] == '"' || chars[i] == '\'' || chars[i] == '`' {
            let q = chars[i];
            out.push(chars[i]);
            i += 1;
            while i < len {
                if chars[i] == '\\' && i + 1 < len {
                    out.push(chars[i]);
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if chars[i] == q {
                    out.push(chars[i]);
                    i += 1;
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }

        // Skip single-line comments.
        if chars[i] == '/' && i + 1 < len && chars[i + 1] == '/' {
            while i < len && chars[i] != '\n' {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }

        // Skip block comments.
        if chars[i] == '/' && i + 1 < len && chars[i + 1] == '*' {
            out.push('/');
            out.push('*');
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                out.push(chars[i]);
                i += 1;
            }
            if i + 1 < len {
                out.push('*');
                out.push('/');
                i += 2;
            }
            continue;
        }

        // `import type …` and `export type …` — remove entire statement.
        if is_keyword_at(&chars, i, "import") || is_keyword_at(&chars, i, "export") {
            let kw_len = 6;
            let j = i + kw_len;
            let after_kw = skip_spaces(&chars, j);
            if is_keyword_at(&chars, after_kw, "type") {
                let after_type = skip_spaces(&chars, after_kw + 4);
                if after_type < len
                    && (chars[after_type] == '{' || !is_value_token(&chars, after_type))
                {
                    let end = find_statement_end(&chars, i);
                    i = end;
                    continue;
                }
            }
        }

        // `interface Foo { … }` — remove entirely.
        if is_keyword_at(&chars, i, "interface") {
            let end = find_block_end(&chars, i);
            i = end;
            out.push('\n');
            continue;
        }

        // `type Foo = …;` — remove.
        if is_keyword_at(&chars, i, "type") {
            let j = skip_spaces(&chars, i + 4);
            if j < len && is_ident_start(chars[j]) {
                let end = find_statement_end(&chars, i);
                i = end;
                out.push('\n');
                continue;
            }
        }

        // `declare …` — remove entire statement or block.
        if is_keyword_at(&chars, i, "declare") {
            let j = skip_spaces(&chars, i + 7);
            if j < len && chars[j] == '{' {
                let end = find_block_end(&chars, i);
                i = end;
            } else {
                let end = find_statement_end(&chars, i);
                i = end;
            }
            out.push('\n');
            continue;
        }

        // `abstract class` — keep `class`, drop `abstract`.
        if is_keyword_at(&chars, i, "abstract") {
            i += 8;
            continue;
        }

        // Class heritage type list: `class Service implements A, B<T> {`.
        if is_keyword_at(&chars, i, "implements") {
            i += 10;
            let mut angle_depth = 0i32;
            while i < len {
                match chars[i] {
                    '<' => angle_depth += 1,
                    '>' => angle_depth = (angle_depth - 1).max(0),
                    '{' if angle_depth == 0 => break,
                    _ => {}
                }
                i += 1;
            }
            continue;
        }

        // Class member access modifiers.
        if is_keyword_at(&chars, i, "public")
            || is_keyword_at(&chars, i, "private")
            || is_keyword_at(&chars, i, "protected")
            || is_keyword_at(&chars, i, "readonly")
            || is_keyword_at(&chars, i, "override")
        {
            let kw_end = skip_ident(&chars, i);
            let after = skip_spaces(&chars, kw_end);
            if after < len
                && (is_ident_start(chars[after]) || chars[after] == '[' || chars[after] == '#')
            {
                i = kw_end;
                continue;
            }
        }

        // `satisfies TypeExpr` — strip the entire `satisfies Type` suffix.
        if is_keyword_at(&chars, i, "satisfies") {
            let after = skip_spaces(&chars, i + 9);
            if after < len && is_ident_start(chars[after]) {
                let type_end = skip_type_annotation(&chars, after);
                if type_end > after {
                    i = type_end;
                    continue;
                }
            }
        }

        // `: TypeAnnotation` — strip colon + type.
        if chars[i] == ':' && i > 0 && !inside_jsx_tag(&chars, i) {
            let prev = prev_non_space(&chars, i);
            if prev
                .map(|c| is_ident_end(c) || c == ')' || c == ']' || c == '}' || c == '?')
                .unwrap_or(false)
            {
                let after = skip_spaces(&chars, i + 1);
                let object_type_allowed =
                    prev.map(|c| matches!(c, ')' | ']' | '}')).unwrap_or(false);
                if after < len
                    && chars[after] != ':'
                    && chars[after] != '/'
                    && (!is_likely_object_literal_value(chars[after]) || object_type_allowed)
                {
                    let type_end = skip_type_annotation(&chars, after);
                    if type_end > after && has_type_annotation_boundary(&chars, type_end) {
                        i = type_end;
                        continue;
                    }
                }
            }
        }

        // `as TypeCast` — strip `as Type`.
        if is_keyword_at(&chars, i, "as") {
            let after = skip_spaces(&chars, i + 2);
            if after < len && is_ident_start(chars[after]) {
                let type_end = skip_type_annotation(&chars, after);
                if type_end > after {
                    i = type_end;
                    continue;
                }
            }
        }

        // Non-null assertion `!` (postfix).
        if chars[i] == '!' && i > 0 {
            let prev = prev_non_space(&chars, i);
            if prev
                .map(|c| is_ident_end(c) || c == ')' || c == ']')
                .unwrap_or(false)
            {
                let after = i + 1;
                if after >= len
                    || chars[after] == '='
                    || chars[after] == ' '
                    || chars[after] == '\n'
                    || chars[after] == '.'
                    || chars[after] == '('
                    || chars[after] == ')'
                    || chars[after] == ','
                    || chars[after] == ';'
                {
                    i += 1;
                    continue;
                }
            }
        }

        // Generic type parameters on calls/new: `foo<T>(` → `foo(`
        if chars[i] == '<' {
            let prev = prev_non_space(&chars, i);
            if prev.map(is_ident_end).unwrap_or(false) {
                if let Some(end) = try_skip_type_args(&chars, i) {
                    i = end;
                    continue;
                }
            }
        }

        out.push(chars[i]);
        i += 1;
    }

    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 2a – JSX transformation (Classic: React.createElement)
// ─────────────────────────────────────────────────────────────────────────────

fn transform_jsx_classic(source: &str) -> std::result::Result<String, String> {
    let mut transformer = JsxTransformer::new(source, JsxRuntime::Classic);
    transformer.run()
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 2b – JSX transformation (Automatic: _jsx/_jsxs from react/jsx-runtime)
// ─────────────────────────────────────────────────────────────────────────────

fn transform_jsx_automatic(source: &str) -> std::result::Result<String, String> {
    let mut transformer = JsxTransformer::new(source, JsxRuntime::Automatic);
    let result = transformer.run()?;

    // Only inject the import if JSX was actually encountered.
    if transformer.jsx_encountered {
        let runtime_import = if transformer.jsxs_encountered {
            "import { jsx as _jsx, jsxs as _jsxs, Fragment as _Fragment } from \"react/jsx-runtime\";\n"
        } else {
            "import { jsx as _jsx, Fragment as _Fragment } from \"react/jsx-runtime\";\n"
        };
        Ok(format!("{runtime_import}{result}"))
    } else {
        Ok(result)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared JSX transformer state machine
// ─────────────────────────────────────────────────────────────────────────────

struct JsxTransformer {
    chars: Vec<char>,
    pos: usize,
    out: String,
    mode: JsxRuntime,
    /// Whether any JSX was encountered (used for automatic import injection).
    jsx_encountered: bool,
    /// Whether any multi-child JSX element was encountered (needs `_jsxs`).
    jsxs_encountered: bool,
}

impl JsxTransformer {
    fn new(source: &str, mode: JsxRuntime) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            out: String::with_capacity(source.len()),
            mode,
            jsx_encountered: false,
            jsxs_encountered: false,
        }
    }

    fn run(&mut self) -> std::result::Result<String, String> {
        while self.pos < self.chars.len() {
            if self.at_jsx_open() {
                let jsx = self.parse_jsx_element()?;
                self.out.push_str(&jsx);
            } else if self.current() == '`' {
                self.consume_template_literal()?;
            } else if self.current() == '"' || self.current() == '\'' {
                self.consume_string_literal();
            } else if self.current() == '/' && self.peek() == Some('/') {
                while self.pos < self.chars.len() && self.current() != '\n' {
                    self.out.push(self.current());
                    self.pos += 1;
                }
            } else if self.current() == '/' && self.peek() == Some('*') {
                self.out.push(self.current());
                self.pos += 1;
                self.out.push(self.current());
                self.pos += 1;
                while self.pos + 1 < self.chars.len() {
                    if self.current() == '*' && self.peek() == Some('/') {
                        self.out.push('*');
                        self.out.push('/');
                        self.pos += 2;
                        break;
                    }
                    self.out.push(self.current());
                    self.pos += 1;
                }
            } else {
                self.out.push(self.current());
                self.pos += 1;
            }
        }

        Ok(self.out.clone())
    }

    fn at_jsx_open(&self) -> bool {
        if self.pos >= self.chars.len() || self.chars[self.pos] != '<' {
            return false;
        }
        let next_pos = self.pos + 1;
        if next_pos >= self.chars.len() {
            return false;
        }
        let next = self.chars[next_pos];
        if next.is_alphabetic() || next == '_' || next == '>' || next == '/' {
            if next == '/' {
                return false;
            }
            return self.is_jsx_context();
        }
        false
    }

    fn is_jsx_context(&self) -> bool {
        let mut i = self.pos.saturating_sub(1);
        while i > 0
            && (self.chars[i] == ' '
                || self.chars[i] == '\t'
                || self.chars[i] == '\n'
                || self.chars[i] == '\r')
        {
            i -= 1;
        }
        if i == 0 && (self.chars[i] == ' ' || self.chars[i] == '\t' || self.chars[i] == '\n') {
            return true;
        }
        let c = self.chars[i];
        matches!(
            c,
            '(' | '='
                | '?'
                | ':'
                | ','
                | '['
                | '{'
                | '&'
                | '|'
                | '!'
                | ';'
                | '>'
                | '+'
                | '-'
                | '*'
                | '/'
                | '%'
                | '\n'
                | '\r'
        ) || (c == 'n' && i >= 5 && self.keyword_at(i - 5, "return"))
            || (c == 't' && i >= 5 && self.keyword_at(i - 5, "export"))
            || (c == 't' && i >= 4 && self.keyword_at(i - 4, "const"))
            || (c == 't' && i >= 2 && self.keyword_at(i - 2, "let"))
            || self.pos == 0
    }

    fn keyword_at(&self, start: usize, kw: &str) -> bool {
        let kw_chars: Vec<char> = kw.chars().collect();
        if start + kw_chars.len() > self.chars.len() {
            return false;
        }
        self.chars[start..start + kw_chars.len()]
            .iter()
            .zip(kw_chars.iter())
            .all(|(a, b)| a == b)
    }

    fn parse_jsx_element(&mut self) -> std::result::Result<String, String> {
        assert_eq!(self.current(), '<');
        self.pos += 1;

        self.jsx_encountered = true;

        // Fragment: <>…</>
        if self.current() == '>' {
            self.pos += 1;
            return self.parse_fragment();
        }

        let tag = self.parse_tag_name()?;
        self.skip_whitespace();
        let props = self.parse_props()?;
        self.skip_whitespace();

        // Self-closing?
        if self.current() == '/' {
            self.pos += 1;
            if self.current() != '>' {
                return Err("expected '>' after '/' in self-closing tag".into());
            }
            self.pos += 1;
            return Ok(self.emit_element(&tag, &props, &[]));
        }

        if self.current() != '>' {
            return Err(format!(
                "expected '>' to close <{tag}>, found {:?}",
                self.current()
            ));
        }
        self.pos += 1;

        let children = self.parse_children(&tag)?;

        Ok(self.emit_element(&tag, &props, &children))
    }

    /// Emit a React element call in the appropriate runtime format.
    fn emit_element(&mut self, tag: &str, props: &[JsxProp], children: &[String]) -> String {
        match self.mode {
            JsxRuntime::Classic => {
                let tag_expr = format_tag(tag);
                let props_expr = format_props(props);
                if children.is_empty() {
                    format!("React.createElement({tag_expr}, {props_expr})")
                } else {
                    format!(
                        "React.createElement({}, {}, {})",
                        tag_expr,
                        props_expr,
                        children.join(", ")
                    )
                }
            }
            JsxRuntime::Automatic => {
                let tag_expr = format_tag_automatic(tag);
                let multi = children.len() > 1;
                if multi {
                    self.jsxs_encountered = true;
                }
                let fn_name = if multi { "_jsxs" } else { "_jsx" };

                // Build props object including children.
                let mut prop_parts: Vec<String> = Vec::new();
                for prop in props {
                    match prop {
                        JsxProp::Named(name, value) => {
                            let key = if needs_quoting(name) {
                                format!("\"{name}\"")
                            } else {
                                name.clone()
                            };
                            prop_parts.push(format!("{key}: {value}"));
                        }
                        JsxProp::Spread(expr) => {
                            prop_parts.push(format!("...{expr}"));
                        }
                    }
                }
                match children.len() {
                    0 => {}
                    1 => prop_parts.push(format!("children: {}", children[0])),
                    _ => {
                        prop_parts.push(format!("children: [{}]", children.join(", ")));
                    }
                }

                let props_obj = if prop_parts.is_empty() {
                    "{}".to_string()
                } else {
                    format!("{{{}}}", prop_parts.join(", "))
                };

                format!("{fn_name}({tag_expr}, {props_obj})")
            }
        }
    }

    fn parse_fragment(&mut self) -> std::result::Result<String, String> {
        let children = self.parse_children("")?;
        match self.mode {
            JsxRuntime::Classic => {
                if children.is_empty() {
                    Ok("React.createElement(React.Fragment, null)".into())
                } else {
                    Ok(format!(
                        "React.createElement(React.Fragment, null, {})",
                        children.join(", ")
                    ))
                }
            }
            JsxRuntime::Automatic => {
                self.jsxs_encountered = children.len() > 1;
                let fn_name = if children.len() > 1 { "_jsxs" } else { "_jsx" };
                match children.len() {
                    0 => Ok(format!("{fn_name}(_Fragment, {{}})").to_string()),
                    1 => Ok(format!(
                        "{fn_name}(_Fragment, {{children: {}}})",
                        children[0]
                    )),
                    _ => Ok(format!(
                        "{fn_name}(_Fragment, {{children: [{}]}})",
                        children.join(", ")
                    )),
                }
            }
        }
    }

    fn parse_tag_name(&mut self) -> std::result::Result<String, String> {
        let mut name = String::new();
        while self.pos < self.chars.len()
            && (self.current().is_alphanumeric()
                || self.current() == '_'
                || self.current() == '.'
                || self.current() == ':'
                || self.current() == '$'
                || self.current() == '-')
        {
            name.push(self.current());
            self.pos += 1;
        }
        if name.is_empty() {
            return Err("empty JSX tag name".into());
        }
        Ok(name)
    }

    fn parse_props(&mut self) -> std::result::Result<Vec<JsxProp>, String> {
        let mut props = Vec::new();

        loop {
            self.skip_whitespace();
            if self.pos >= self.chars.len() {
                break;
            }
            if self.current() == '>' || self.current() == '/' {
                break;
            }

            // Spread: {...expr}
            if self.current() == '{' {
                self.pos += 1;
                self.skip_whitespace();
                if self.current() == '.' && self.peek() == Some('.') && self.peek_at(2) == Some('.')
                {
                    self.pos += 3;
                    let expr = self.parse_jsx_expression_content()?;
                    props.push(JsxProp::Spread(expr));
                    continue;
                } else {
                    return Err("unexpected { in prop position without spread".into());
                }
            }

            let name = self.parse_prop_name()?;
            self.skip_whitespace();

            if self.current() == '=' {
                self.pos += 1;
                self.skip_whitespace();
                let value = self.parse_prop_value()?;
                props.push(JsxProp::Named(name, value));
            } else {
                props.push(JsxProp::Named(name, "true".into()));
            }
        }

        Ok(props)
    }

    fn parse_prop_name(&mut self) -> std::result::Result<String, String> {
        let mut name = String::new();
        while self.pos < self.chars.len()
            && (self.current().is_alphanumeric()
                || self.current() == '_'
                || self.current() == '-'
                || self.current() == '$'
                || self.current() == ':')
        {
            name.push(self.current());
            self.pos += 1;
        }
        if name.is_empty() {
            return Err(format!("empty prop name at char {:?}", self.safe_current()));
        }
        Ok(name)
    }

    fn parse_prop_value(&mut self) -> std::result::Result<String, String> {
        if self.pos >= self.chars.len() {
            return Err("unexpected EOF in prop value".into());
        }

        match self.current() {
            '"' | '\'' => {
                let q = self.current();
                self.pos += 1;
                let mut val = String::new();
                while self.pos < self.chars.len() && self.current() != q {
                    if self.current() == '\\' {
                        val.push(self.current());
                        self.pos += 1;
                        if self.pos < self.chars.len() {
                            val.push(self.current());
                            self.pos += 1;
                        }
                    } else {
                        val.push(self.current());
                        self.pos += 1;
                    }
                }
                if self.pos < self.chars.len() {
                    self.pos += 1;
                }
                Ok(format!("{q}{val}{q}"))
            }
            '{' => {
                self.pos += 1;
                self.parse_jsx_expression_content()
            }
            _ => Err(format!("unexpected prop value char: {:?}", self.current())),
        }
    }

    fn parse_children(&mut self, parent_tag: &str) -> std::result::Result<Vec<String>, String> {
        let mut children: Vec<String> = Vec::new();

        loop {
            if self.pos >= self.chars.len() {
                if parent_tag.is_empty() {
                    return Err("unexpected EOF in fragment".into());
                }
                return Err(format!("unexpected EOF, expected </{parent_tag}>"));
            }

            if self.current() == '<' && self.peek() == Some('/') {
                self.pos += 2;
                if parent_tag.is_empty() && self.current() == '>' {
                    self.pos += 1;
                    return Ok(children);
                }
                let close_tag = self.parse_tag_name().unwrap_or_default();
                self.skip_whitespace();
                if self.current() == '>' {
                    self.pos += 1;
                }
                if close_tag != parent_tag && !parent_tag.is_empty() {
                    return Err(format!(
                        "mismatched closing tag: expected </{parent_tag}>, found </{close_tag}>"
                    ));
                }
                return Ok(children);
            }

            if self.at_jsx_child_open() {
                let child = self.parse_jsx_element()?;
                children.push(child);
                continue;
            }

            if self.current() == '{' {
                self.pos += 1;
                let expr = self.parse_jsx_expression_content()?;
                if !expr.trim().is_empty() {
                    children.push(expr);
                }
                continue;
            }

            let text = self.parse_jsx_text();
            if !text.is_empty() {
                children.push(format!("\"{}\"", escape_jsx_text(&text)));
            }
        }
    }

    fn parse_jsx_text(&mut self) -> String {
        let mut text = String::new();
        while self.pos < self.chars.len() && self.current() != '<' && self.current() != '{' {
            text.push(self.current());
            self.pos += 1;
        }
        let trimmed = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        trimmed
    }

    fn at_jsx_child_open(&self) -> bool {
        if self.pos >= self.chars.len() || self.chars[self.pos] != '<' {
            return false;
        }
        matches!(
            self.chars.get(self.pos + 1),
            Some(next) if next.is_alphabetic() || *next == '_' || *next == '>'
        )
    }

    fn parse_jsx_expression_content(&mut self) -> std::result::Result<String, String> {
        let mut expr = String::new();
        let mut depth = 0i32;

        while self.pos < self.chars.len() {
            match self.current() {
                '{' => {
                    depth += 1;
                    expr.push('{');
                    self.pos += 1;
                }
                '}' => {
                    if depth == 0 {
                        self.pos += 1;
                        return Ok(expr.trim().to_string());
                    }
                    depth -= 1;
                    expr.push('}');
                    self.pos += 1;
                }
                '"' | '\'' => {
                    let q = self.current();
                    expr.push(q);
                    self.pos += 1;
                    while self.pos < self.chars.len() && self.current() != q {
                        if self.current() == '\\' {
                            expr.push(self.current());
                            self.pos += 1;
                            if self.pos < self.chars.len() {
                                expr.push(self.current());
                                self.pos += 1;
                            }
                        } else {
                            expr.push(self.current());
                            self.pos += 1;
                        }
                    }
                    if self.pos < self.chars.len() {
                        expr.push(self.current());
                        self.pos += 1;
                    }
                }
                '`' => {
                    expr.push('`');
                    self.pos += 1;
                    while self.pos < self.chars.len() && self.current() != '`' {
                        if self.current() == '\\' {
                            expr.push(self.current());
                            self.pos += 1;
                            if self.pos < self.chars.len() {
                                expr.push(self.current());
                                self.pos += 1;
                            }
                        } else if self.current() == '$' && self.peek() == Some('{') {
                            expr.push('$');
                            expr.push('{');
                            self.pos += 2;
                            let mut td = 1i32;
                            while self.pos < self.chars.len() && td > 0 {
                                if self.current() == '{' {
                                    td += 1;
                                } else if self.current() == '}' {
                                    td -= 1;
                                }
                                if td > 0 {
                                    expr.push(self.current());
                                    self.pos += 1;
                                }
                            }
                            if self.pos < self.chars.len() {
                                expr.push('}');
                                self.pos += 1;
                            }
                        } else {
                            expr.push(self.current());
                            self.pos += 1;
                        }
                    }
                    if self.pos < self.chars.len() {
                        expr.push('`');
                        self.pos += 1;
                    }
                }
                '<' if self.at_jsx_open() => {
                    let jsx = self.parse_jsx_element()?;
                    expr.push_str(&jsx);
                }
                _ => {
                    expr.push(self.current());
                    self.pos += 1;
                }
            }
        }

        Err("unexpected EOF inside JSX expression".into())
    }

    fn consume_string_literal(&mut self) {
        let q = self.current();
        self.out.push(q);
        self.pos += 1;
        while self.pos < self.chars.len() {
            if self.current() == '\\' && self.pos + 1 < self.chars.len() {
                self.out.push(self.current());
                self.pos += 1;
                self.out.push(self.current());
                self.pos += 1;
            } else if self.current() == q {
                self.out.push(self.current());
                self.pos += 1;
                return;
            } else {
                self.out.push(self.current());
                self.pos += 1;
            }
        }
    }

    fn consume_template_literal(&mut self) -> std::result::Result<(), String> {
        self.out.push('`');
        self.pos += 1;
        while self.pos < self.chars.len() {
            if self.current() == '\\' && self.pos + 1 < self.chars.len() {
                self.out.push(self.current());
                self.pos += 1;
                self.out.push(self.current());
                self.pos += 1;
            } else if self.current() == '$' && self.peek() == Some('{') {
                self.out.push('$');
                self.out.push('{');
                self.pos += 2;
                let mut depth = 1i32;
                while self.pos < self.chars.len() && depth > 0 {
                    if self.current() == '{' {
                        depth += 1;
                    } else if self.current() == '}' {
                        depth -= 1;
                        if depth == 0 {
                            self.out.push('}');
                            self.pos += 1;
                            break;
                        }
                    }
                    self.out.push(self.current());
                    self.pos += 1;
                }
            } else if self.current() == '`' {
                self.out.push('`');
                self.pos += 1;
                return Ok(());
            } else {
                self.out.push(self.current());
                self.pos += 1;
            }
        }
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.chars.len()
            && (self.current() == ' '
                || self.current() == '\t'
                || self.current() == '\n'
                || self.current() == '\r')
        {
            self.pos += 1;
        }
    }

    fn current(&self) -> char {
        self.chars[self.pos]
    }

    fn safe_current(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JSX prop/tag helpers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum JsxProp {
    Named(String, String),
    Spread(String),
}

fn format_tag(tag: &str) -> String {
    if tag.is_empty() {
        return "React.Fragment".into();
    }
    let first = tag.chars().next().unwrap();
    if first.is_lowercase() || tag.contains('-') {
        format!("\"{}\"", tag)
    } else {
        tag.to_string()
    }
}

fn format_tag_automatic(tag: &str) -> String {
    if tag.is_empty() {
        return "_Fragment".into();
    }
    let first = tag.chars().next().unwrap();
    if first.is_lowercase() || tag.contains('-') {
        format!("\"{}\"", tag)
    } else {
        tag.to_string()
    }
}

fn format_props(props: &[JsxProp]) -> String {
    if props.is_empty() {
        return "null".into();
    }

    let has_spread = props.iter().any(|p| matches!(p, JsxProp::Spread(_)));
    if has_spread {
        let mut parts = vec!["Object.assign({}, ".to_string()];
        let mut current_obj = Vec::new();

        for prop in props {
            match prop {
                JsxProp::Named(name, value) => {
                    let key = if needs_quoting(name) {
                        format!("\"{}\"", name)
                    } else {
                        name.clone()
                    };
                    current_obj.push(format!("{}: {}", key, value));
                }
                JsxProp::Spread(expr) => {
                    if !current_obj.is_empty() {
                        parts.push(format!("{{{}}}", current_obj.join(", ")));
                        parts.push(", ".into());
                        current_obj.clear();
                    }
                    parts.push(expr.clone());
                    parts.push(", ".into());
                }
            }
        }
        if !current_obj.is_empty() {
            parts.push(format!("{{{}}}", current_obj.join(", ")));
        } else if parts.last().map(|s| s.as_str()) == Some(", ") {
            parts.pop();
        }
        parts.push(")".into());
        parts.join("")
    } else {
        let entries: Vec<String> = props
            .iter()
            .filter_map(|p| {
                if let JsxProp::Named(name, value) = p {
                    let key = if needs_quoting(name) {
                        format!("\"{}\"", name)
                    } else {
                        name.clone()
                    };
                    Some(format!("{key}: {value}"))
                } else {
                    None
                }
            })
            .collect();
        format!("{{{}}}", entries.join(", "))
    }
}

fn needs_quoting(name: &str) -> bool {
    name.contains('-') || name.contains(':') || name.starts_with(|c: char| c.is_ascii_digit())
}

fn escape_jsx_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

// ─────────────────────────────────────────────────────────────────────────────
// TypeScript-stripping helpers (shared with strip_typescript)
// ─────────────────────────────────────────────────────────────────────────────

fn is_keyword_at(chars: &[char], i: usize, kw: &str) -> bool {
    let kw_chars: Vec<char> = kw.chars().collect();
    let kw_len = kw_chars.len();
    if i + kw_len > chars.len() {
        return false;
    }
    if chars[i..i + kw_len] != kw_chars[..] {
        return false;
    }
    // Must not be followed by an identifier character.
    let after = i + kw_len;
    if after < chars.len() && (chars[after].is_alphanumeric() || chars[after] == '_') {
        return false;
    }
    // Must not be preceded by an identifier character.
    if i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
        return false;
    }
    true
}

fn skip_spaces(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        i += 1;
    }
    i
}

fn skip_all_whitespace(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

fn skip_ident(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
        i += 1;
    }
    i
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_end(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn prev_non_space(chars: &[char], i: usize) -> Option<char> {
    let mut j = i.wrapping_sub(1);
    while j < i {
        if chars[j] != ' ' && chars[j] != '\t' {
            return Some(chars[j]);
        }
        if j == 0 {
            break;
        }
        j -= 1;
    }
    None
}

fn next_non_space(chars: &[char], mut i: usize) -> Option<char> {
    while i < chars.len()
        && (chars[i] == ' ' || chars[i] == '\t' || chars[i] == '\n' || chars[i] == '\r')
    {
        i += 1;
    }
    chars.get(i).copied()
}

fn has_type_annotation_boundary(chars: &[char], i: usize) -> bool {
    matches!(
        next_non_space(chars, i),
        Some('=' | ',' | ')' | ';' | '{' | '}' | '[' | ']') | None
    )
}

fn is_likely_object_literal_value(c: char) -> bool {
    // After `:` in `{ key: value }`, the value starts with one of these.
    matches!(c, '"' | '\'' | '`' | '{' | '[' | '(' | '-' | '+')
}

fn is_value_token(chars: &[char], i: usize) -> bool {
    if i >= chars.len() {
        return false;
    }
    chars[i].is_alphanumeric() || chars[i] == '"' || chars[i] == '\'' || chars[i] == '{'
}

fn inside_jsx_tag(chars: &[char], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 {
        cursor -= 1;
        match chars[cursor] {
            '>' => return false,
            '<' => {
                return chars
                    .get(cursor + 1)
                    .is_some_and(|next| next.is_alphabetic() || matches!(next, '_' | '/'));
            }
            _ => {}
        }
    }
    false
}

fn find_statement_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    let mut depth = 0i32;
    while i < chars.len() {
        match chars[i] {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                } else {
                    return i + 1;
                }
            }
            ';' if depth == 0 => return i + 1,
            '\n' if depth == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    i
}

fn find_block_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    // Skip to the opening brace.
    while i < chars.len() && chars[i] != '{' {
        i += 1;
    }
    if i >= chars.len() {
        return i;
    }
    let mut depth = 1i32;
    i += 1;
    while i < chars.len() && depth > 0 {
        match chars[i] {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    i
}

fn skip_type_annotation(chars: &[char], start: usize) -> usize {
    let mut i = start;
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;
    let mut depth_angle = 0i32;

    while i < chars.len() {
        match chars[i] {
            '(' => {
                depth_paren += 1;
                i += 1;
            }
            ')' => {
                if depth_paren > 0 {
                    depth_paren -= 1;
                    i += 1;
                } else {
                    break;
                }
            }
            '[' => {
                depth_bracket += 1;
                i += 1;
            }
            ']' => {
                if depth_bracket > 0 {
                    depth_bracket -= 1;
                    i += 1;
                } else {
                    break;
                }
            }
            '<' => {
                depth_angle += 1;
                i += 1;
            }
            '>' => {
                if depth_angle > 0 {
                    depth_angle -= 1;
                    i += 1;
                } else {
                    break;
                }
            }
            '{' => {
                // Object type — skip balanced braces.
                let mut d = 1i32;
                i += 1;
                while i < chars.len() && d > 0 {
                    if chars[i] == '{' {
                        d += 1;
                    } else if chars[i] == '}' {
                        d -= 1;
                    }
                    i += 1;
                }
            }
            '|' | '&' if depth_paren == 0 && depth_bracket == 0 && depth_angle == 0 => {
                // Union/intersection — keep consuming.
                i += 1;
                i = skip_spaces(chars, i);
            }
            ';' | ',' | '=' | '}' | '\n' => break,
            _ => {
                i += 1;
            }
        }
    }

    // Back off trailing whitespace.
    while i > start && (chars[i - 1] == ' ' || chars[i - 1] == '\t') {
        i -= 1;
    }

    i
}

fn try_skip_type_args(chars: &[char], start: usize) -> Option<usize> {
    if start >= chars.len() || chars[start] != '<' {
        return None;
    }

    // A `</` pattern is always a JSX closing tag, never a type argument.
    // Similarly, `<>` is a JSX fragment opener.
    if start + 1 < chars.len() && (chars[start + 1] == '/' || chars[start + 1] == '>') {
        return None;
    }

    // Only attempt type-arg skipping if the content looks like an identifier
    // (type parameter starts with a letter or `_`), not JSX tag content.
    if start + 1 < chars.len() {
        let first = chars[start + 1];
        // Type arguments start with: letter, `_`, `$`, `[`, `(`, or `...`
        // JSX tag names also start with letters — we need extra context.
        // Heuristic: if what follows the potential `>` closer is whitespace or
        // JSX-looking characters, it is NOT a type argument.
        if !first.is_alphabetic() && first != '_' && first != '$' && first != '[' && first != '(' {
            return None;
        }
    }

    let mut i = start + 1;
    let mut depth = 1i32;

    while i < chars.len() && depth > 0 {
        match chars[i] {
            '<' => {
                // Nested `<` only makes sense in nested type args, not JSX.
                depth += 1;
                i += 1;
            }
            '>' => {
                depth -= 1;
                if depth == 0 {
                    i += 1;
                    // Must be followed by `(`, `.`, `[`, `)`, `,`, `;`, `=`, or
                    // end-of-expression context — NOT by a JSX identifier or `>`.
                    if i < chars.len() {
                        let next = chars[i];
                        if matches!(next, '(' | '.' | '[' | ')' | ',' | ';' | '=') {
                            return Some(i);
                        }
                        // Also allow end-of-statement / whitespace only if NOT
                        // followed by JSX-opening patterns.
                        if next == '\n' || next == ' ' || next == '\t' {
                            // Ensure this isn't part of JSX: after a type arg close
                            // we expect `(` for a call, not a JSX child.
                            // Be conservative and only accept if next non-space is `(`.
                            let mut j = i;
                            while j < chars.len()
                                && (chars[j] == ' ' || chars[j] == '\t' || chars[j] == '\n')
                            {
                                j += 1;
                            }
                            if j < chars.len() && chars[j] == '(' {
                                return Some(i);
                            }
                        }
                    } else {
                        return Some(i);
                    }
                    return None;
                }
                i += 1;
            }
            '"' | '\'' => {
                let q = chars[i];
                i += 1;
                while i < chars.len() && chars[i] != q {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i < chars.len() {
                    i += 1;
                }
            }
            // Operators that cannot appear inside a generic type expression.
            '+' | '-' | '*' | '%' | '!' | '~' | ';' => return None,
            // JSX attribute `=` only appears inside tags, not in type args.
            '=' => {
                // Allow `=` as part of `extends` constraint default `<T = string>`.
                i += 1;
            }
            _ => i += 1,
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_interface() {
        let src = "interface Foo { bar: string; }\nconst x = 1;";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("interface Foo"));
        assert!(out.contains("const x = 1;"));
    }

    #[test]
    fn strips_type_annotation() {
        let src = "const x: number = 5;";
        let out = transform(src, false).unwrap();
        assert!(!out.contains(": number"));
        assert!(out.contains("const x"));
        assert!(out.contains("= 5"));
    }

    #[test]
    fn strips_generic_type_params() {
        let src = "const arr = new Array<number>();";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("<number>"));
        assert!(out.contains("new Array()"));
    }

    #[test]
    fn transforms_simple_jsx_classic() {
        let src = "const el = <div className=\"x\">hello</div>;";
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.createElement(\"div\""));
        assert!(out.contains("className"));
    }

    #[test]
    fn transforms_self_closing_jsx_classic() {
        let src = "const el = <Input disabled />;";
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.createElement(Input"));
        assert!(out.contains("disabled: true"));
    }

    #[test]
    fn transforms_jsx_automatic() {
        let src = "const el = <div>hello</div>;";
        let out = transform_with_options(src, true, JsxRuntime::Automatic).unwrap();
        assert!(out.contains("import { jsx as _jsx"));
        assert!(out.contains("_jsx(\"div\""));
    }

    #[test]
    fn transforms_jsx_automatic_multi_child_uses_jsxs() {
        let src = "const el = <div><span/><span/></div>;";
        let out = transform_with_options(src, true, JsxRuntime::Automatic).unwrap();
        assert!(out.contains("_jsxs"));
        assert!(out.contains("import { jsx as _jsx, jsxs as _jsxs"));
    }

    #[test]
    fn strips_decorators() {
        let src = "@Injectable()\nclass Service {}";
        let out = strip_decorators(src);
        assert!(!out.contains("@Injectable"));
        assert!(out.contains("class Service"));
    }

    #[test]
    fn expands_enum() {
        let src = "enum Direction { Up, Down = 5, Left }";
        let out = expand_enums(src).unwrap();
        assert!(out.contains("const Direction"));
        assert!(out.contains("Up: 0"));
        assert!(out.contains("Down: 5"));
        assert!(out.contains("Left: 6"));
    }

    #[test]
    fn expands_const_enum() {
        let src = "const enum Color { Red, Green, Blue }";
        let out = expand_enums(src).unwrap();
        assert!(out.contains("const Color"));
        assert!(out.contains("Red: 0"));
        assert!(out.contains("Green: 1"));
        assert!(out.contains("Blue: 2"));
    }

    #[test]
    fn strips_satisfies_expression() {
        let src = "const config = { port: 3000 } satisfies Config;";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("satisfies"));
        assert!(out.contains("const config"));
    }

    #[test]
    fn jsx_fragment_classic() {
        let src = "const el = <><div/><span/></>;";
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.Fragment"));
    }

    #[test]
    fn jsx_fragment_automatic() {
        let src = "const el = <><div/></>;";
        let out = transform_with_options(src, true, JsxRuntime::Automatic).unwrap();
        assert!(out.contains("_Fragment"));
    }

    #[test]
    fn hyphenated_tag_quoted() {
        let src = "const el = <my-element />;";
        let out = transform(src, true).unwrap();
        assert!(out.contains("\"my-element\""));
    }

    #[test]
    fn jsx_text_with_inline_code_element() {
        let src = r#"export default function About() {
  return (
    <main className="page">
      <p>Rendered from <code>app/about/page.tsx</code> - a static page.</p>
      <p>Every <code>page.tsx</code> file becomes a route.</p>
    </main>
  )
}"#;

        let out = transform(src, true).unwrap();
        assert!(out.contains("React.createElement(\"code\""));
        assert!(out.contains("React.createElement(\"main\""));
    }

    #[test]
    fn jsx_text_colon_is_not_type_annotation() {
        let src = r#"export default function About() {
  return (
    <main>
      <p>This demonstrates routing: every <code>page.tsx</code> file becomes a route.</p>
    </main>
  )
}"#;

        let out = transform(src, true).unwrap();
        assert!(out.contains("routing: every"));
        assert!(out.contains("React.createElement(\"code\""));
    }

    #[test]
    fn jsx_code_child_with_expression_and_slashes() {
        let src = r#"const el = (
  <p>Rendered from the <code>catchall/{'[...slug]'}/page.tsx</code> file.</p>
);"#;

        let out = transform(src, true).unwrap();
        assert!(out.contains("catchall/"));
        assert!(out.contains("'[...slug]'"));
        assert!(out.contains("/page.tsx"));
    }

    #[test]
    fn strips_destructured_param_type_before_jsx() {
        let src = r#"export default function CatchAll({ params }: { params: { slug: string } }) {
  return (
    <main className="page">
      <p>Rendered from the <code>catchall/{'[...slug]'}/page.tsx</code> file.</p>
        <p>The <code>{'[...slug]'}</code> pattern captures all remaining URL segments:</p>
    </main>
  )
}"#;

        let stripped = strip_typescript(src).unwrap();
        assert!(!stripped.contains(": { params"), "{stripped}");
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.createElement(\"code\""));
        assert!(out.contains("React.createElement(\"main\""));
    }
}
