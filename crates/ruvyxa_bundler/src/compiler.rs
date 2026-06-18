//! Ruvyxa Native TypeScript/JSX Compiler
//!
//! A pure-Rust, zero-external-dependency TypeScript and JSX transformer
//! designed specifically for the Ruvyxa framework.
//!
//! ## Strategy
//!
//! Rather than pulling in a full AST-based parser (which would conflict with
//! wasmtime's `bumpalo` version pin), this compiler uses a **character-level
//! streaming transformer** inspired by Sucrase.  It handles the TypeScript
//! constructs that appear in real Ruvyxa apps without a full grammar.
//!
//! ### TypeScript stripping
//! - Type annotations: `foo: string`, `bar: Map<string, number>`
//! - Generic type parameters on functions and calls: `<T>`, `<T extends U>`
//! - Interface/type declarations: `interface Foo {}`, `type Bar = …`
//! - `import type` and `export type` declarations
//! - Access modifiers on class members: `public`, `private`, `protected`
//! - `as` casts: `expr as Type`
//! - Non-null assertions: `expr!`
//!
//! ### JSX transformation
//! - `<Component prop={…}>…</Component>` → `React.createElement(Component, {prop: …}, …)`
//! - `<tag>…</tag>` → `React.createElement("tag", null, …)`
//! - `<SelfClosing />` → `React.createElement(SelfClosing, null)`
//! - JSX expressions `{expr}` → `expr`
//! - JSX text nodes → string literals

use std::path::PathBuf;

use rayon::prelude::*;

use crate::cache::{CacheLookup, CompileCache};
use crate::resolver::ResolvedModule;
use crate::{BundleError, BundleInput, Result};
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
    // Use parallel iterator for multi-core compilation.
    // Rayon handles thread pool management and work-stealing automatically.
    let results: Vec<Result<CompiledModule>> = graph
        .par_iter()
        .map(|module| compile_module(module, input, cache))
        .collect();

    // Collect results, propagating the first error if any module failed.
    results.into_iter().collect()
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
///
/// This is useful for dev mode where you want to show all errors at once
/// rather than stopping at the first one.
pub fn compile_graph_resilient(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<CompiledModule> {
    let results: Vec<(usize, Result<CompiledModule>)> = graph
        .par_iter()
        .enumerate()
        .map(|(idx, module)| (idx, compile_module(module, input, cache)))
        .collect();

    let mut compiled = Vec::with_capacity(graph.len());

    for (idx, result) in results {
        match result {
            Ok(module) => compiled.push(module),
            Err(err) => {
                let source_module = &graph[idx];

                // Convert the error to a diagnostic with line/col info.
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

                // Emit a stub module that throws at runtime with useful info.
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
                });
            }
        }
    }

    compiled
}

/// Parse line:col from an error message like "file.tsx:5:12: unexpected token"
/// Falls back to (1, 1) if parsing fails.
fn parse_error_location(msg: &str) -> (u32, u32) {
    // Try pattern: "path:line:col: message" or just extract numbers
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
    // Try "at line X, column Y" pattern
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
    _input: &BundleInput,
    cache: &CompileCache,
) -> Result<CompiledModule> {
    let ext = module
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Virtual entry (label starts with "ruvyxa:") or plain JS: pass through.
    if matches!(ext, "js" | "mjs" | "cjs") || module.path.to_string_lossy().contains("ruvyxa:") {
        return Ok(CompiledModule {
            path: module.path.clone(),
            js: module.source.clone(),
            deps: module.deps.clone(),
            is_external: module.is_external,
        });
    }

    let has_jsx = matches!(ext, "tsx" | "jsx");

    // Check the cache first.
    match cache.lookup(&module.source, has_jsx) {
        CacheLookup::Hit(cached_js) => Ok(CompiledModule {
            path: module.path.clone(),
            js: cached_js,
            deps: module.deps.clone(),
            is_external: module.is_external,
        }),
        CacheLookup::Miss(key) => {
            let js = transform(&module.source, has_jsx).map_err(|msg| {
                BundleError::Compiler(format!("{}: {}", module.path.display(), msg))
            })?;

            // Store in cache for next time.
            cache.store(&key, &js);

            Ok(CompiledModule {
                path: module.path.clone(),
                js,
                deps: module.deps.clone(),
                is_external: module.is_external,
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Core transformer
// ─────────────────────────────────────────────────────────────────────────────

/// Transform TypeScript/JSX source to plain JavaScript.
///
/// Passes:
/// 1. Strip TypeScript-specific syntax.
/// 2. Transform JSX to `React.createElement` calls (if `has_jsx`).
pub fn transform(source: &str, has_jsx: bool) -> std::result::Result<String, String> {
    let stripped = strip_typescript(source)?;
    if has_jsx {
        transform_jsx(&stripped)
    } else {
        Ok(stripped)
    }
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
            let kw_len = 6; // both "import" and "export" are 6 chars
            let j = i + kw_len;
            let after_kw = skip_spaces(&chars, j);
            if is_keyword_at(&chars, after_kw, "type") {
                // Check it's "import type X from …" or "export type { … }"
                let after_type = skip_spaces(&chars, after_kw + 4);
                // "export type { … }" — remove to end of statement
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
            // Emit a blank line to preserve line numbers roughly.
            out.push('\n');
            continue;
        }

        // `type Foo = …;` — remove.
        if is_keyword_at(&chars, i, "type") {
            let j = skip_spaces(&chars, i + 4);
            if j < len && is_ident_start(chars[j]) {
                // Make sure this is `type Foo = …` not `typeof`
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
            i += 8; // skip "abstract"
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
            // Only strip if followed by space + identifier (class member context)
            let after = skip_spaces(&chars, kw_end);
            if after < len
                && (is_ident_start(chars[after]) || chars[after] == '[' || chars[after] == '#')
            {
                i = kw_end; // Skip the modifier keyword.
                continue;
            }
        }

        // `: TypeAnnotation` — strip colon + type, but not in object literals.
        if chars[i] == ':' && i > 0 {
            // Heuristic: if the character before (skipping spaces) is an
            // identifier, `)`, `]`, or `?`, this is a type annotation.
            let prev = prev_non_space(&chars, i);
            if prev
                .map(|c| is_ident_end(c) || c == ')' || c == ']' || c == '?')
                .unwrap_or(false)
            {
                // Peek ahead: if followed by a type expression (not `:`), strip it.
                let after = skip_spaces(&chars, i + 1);
                if after < len && chars[after] != ':' && chars[after] != '/' {
                    let type_end = skip_type_annotation(&chars, after);
                    if type_end > after {
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

        // Non-null assertion `!` (postfix) — only strip when NOT logical NOT.
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
                    // Skip the `!`
                    i += 1;
                    continue;
                }
            }
        }

        // Generic type parameters on calls/new: `foo<T>(` → `foo(`
        if chars[i] == '<' {
            let prev = prev_non_space(&chars, i);
            if prev.map(is_ident_end).unwrap_or(false) {
                // Try to skip balanced `< … >` type args.
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
// Pass 2 – JSX transformation
// ─────────────────────────────────────────────────────────────────────────────

/// Transform JSX syntax to `React.createElement(…)` calls.
///
/// Supports:
/// - `<Component prop={expr}>children</Component>`
/// - `<tag className="x">text</tag>`
/// - `<SelfClosing />`
/// - `<>fragment</>` (React.Fragment)
/// - `{expression}` interpolation
/// - Spread props `{...obj}`
/// - Text nodes as string literals
fn transform_jsx(source: &str) -> std::result::Result<String, String> {
    let mut transformer = JsxTransformer::new(source);
    transformer.run()
}

/// Streaming JSX transformer state machine.
struct JsxTransformer {
    chars: Vec<char>,
    pos: usize,
    out: String,
}

impl JsxTransformer {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            out: String::with_capacity(source.len()),
        }
    }

    fn run(&mut self) -> std::result::Result<String, String> {
        while self.pos < self.chars.len() {
            if self.at_jsx_open() {
                let jsx = self.parse_jsx_element()?;
                self.out.push_str(&jsx);
            } else if self.current() == '`' {
                // Template literal — preserve verbatim (may contain ${} with JSX).
                self.consume_template_literal()?;
            } else if self.current() == '"' || self.current() == '\'' {
                self.consume_string_literal();
            } else if self.current() == '/' && self.peek() == Some('/') {
                // Line comment.
                while self.pos < self.chars.len() && self.current() != '\n' {
                    self.out.push(self.current());
                    self.pos += 1;
                }
            } else if self.current() == '/' && self.peek() == Some('*') {
                // Block comment.
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

    /// Check if the current position starts a JSX element (not a comparison).
    fn at_jsx_open(&self) -> bool {
        if self.pos >= self.chars.len() || self.chars[self.pos] != '<' {
            return false;
        }
        let next_pos = self.pos + 1;
        if next_pos >= self.chars.len() {
            return false;
        }
        let next = self.chars[next_pos];
        // JSX starts with: <Uppercase, <lowercase, <_, <>, </
        // Not JSX: <=, <<, < followed by space/digit (comparison)
        if next.is_alphabetic() || next == '_' || next == '>' || next == '/' {
            // Additional heuristic: check what came before to distinguish
            // from comparison operators. If preceded by an identifier char
            // without an operator, it's likely a comparison.
            if next == '/' {
                return false; // closing tag handled inside element parse
            }
            // Check context: is this return/=/(/, etc.
            return self.is_jsx_context();
        }
        false
    }

    /// Heuristic: JSX appears after `return`, `(`, `=`, `?`, `:`, `,`, `&&`, `||`, `??`, `[`, `{`, `=>`.
    fn is_jsx_context(&self) -> bool {
        let mut i = self.pos.saturating_sub(1);
        // Skip whitespace backwards
        while i > 0
            && (self.chars[i] == ' '
                || self.chars[i] == '\t'
                || self.chars[i] == '\n'
                || self.chars[i] == '\r')
        {
            i -= 1;
        }
        if i == 0 && (self.chars[i] == ' ' || self.chars[i] == '\t' || self.chars[i] == '\n') {
            return true; // beginning of file/line
        }
        let c = self.chars[i];
        // After these chars, `<` is JSX:
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

    /// Parse a full JSX element and return its React.createElement equivalent.
    fn parse_jsx_element(&mut self) -> std::result::Result<String, String> {
        assert_eq!(self.current(), '<');
        self.pos += 1; // skip <

        // Fragment: <>…</>
        if self.current() == '>' {
            self.pos += 1; // skip >
            return self.parse_fragment();
        }

        // Parse tag name (may be dotted: Foo.Bar)
        let tag = self.parse_tag_name()?;
        self.skip_whitespace();

        // Parse props
        let props = self.parse_props()?;
        self.skip_whitespace();

        // Self-closing?
        if self.current() == '/' {
            self.pos += 1; // skip /
            if self.current() != '>' {
                return Err("expected '>' after '/' in self-closing tag".into());
            }
            self.pos += 1; // skip >
            return Ok(format!(
                "React.createElement({}, {})",
                format_tag(&tag),
                format_props(&props)
            ));
        }

        // Opening tag close: >
        if self.current() != '>' {
            return Err(format!(
                "expected '>' to close <{tag}>, found {:?}",
                self.current()
            ));
        }
        self.pos += 1; // skip >

        // Parse children until closing tag </tag>
        let children = self.parse_children(&tag)?;

        // Format output
        if children.is_empty() {
            Ok(format!(
                "React.createElement({}, {})",
                format_tag(&tag),
                format_props(&props)
            ))
        } else {
            Ok(format!(
                "React.createElement({}, {}, {})",
                format_tag(&tag),
                format_props(&props),
                children.join(", ")
            ))
        }
    }

    /// Parse a JSX fragment `<>…</>`
    fn parse_fragment(&mut self) -> std::result::Result<String, String> {
        let children = self.parse_children("")?;

        if children.is_empty() {
            Ok("React.createElement(React.Fragment, null)".into())
        } else {
            Ok(format!(
                "React.createElement(React.Fragment, null, {})",
                children.join(", ")
            ))
        }
    }

    /// Parse tag name: `div`, `MyComponent`, `Foo.Bar`
    fn parse_tag_name(&mut self) -> std::result::Result<String, String> {
        let mut name = String::new();
        while self.pos < self.chars.len()
            && (self.current().is_alphanumeric()
                || self.current() == '_'
                || self.current() == '.'
                || self.current() == '$')
        {
            name.push(self.current());
            self.pos += 1;
        }
        if name.is_empty() {
            return Err("empty JSX tag name".into());
        }
        Ok(name)
    }

    /// Parse JSX props until `>` or `/>`.
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
                self.pos += 1; // skip {
                self.skip_whitespace();
                if self.current() == '.' && self.peek() == Some('.') && self.peek_at(2) == Some('.')
                {
                    self.pos += 3; // skip ...
                    let expr = self.parse_jsx_expression_content()?;
                    props.push(JsxProp::Spread(expr));
                    continue;
                } else {
                    return Err("unexpected { in prop position without spread".into());
                }
            }

            // Named prop
            let name = self.parse_prop_name()?;
            self.skip_whitespace();

            if self.current() == '=' {
                self.pos += 1; // skip =
                self.skip_whitespace();
                let value = self.parse_prop_value()?;
                props.push(JsxProp::Named(name, value));
            } else {
                // Boolean shorthand: <Input disabled />
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
                || self.current() == '$')
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
            // String literal value: "hello" or 'hello'
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
                    self.pos += 1; // skip closing quote
                }
                Ok(format!("{q}{val}{q}"))
            }
            // Expression value: {expr}
            '{' => {
                self.pos += 1; // skip {
                self.parse_jsx_expression_content()
            }
            _ => Err(format!("unexpected prop value char: {:?}", self.current())),
        }
    }

    /// Parse children between open and close tags.
    /// Returns Vec of string expressions for each child.
    fn parse_children(&mut self, parent_tag: &str) -> std::result::Result<Vec<String>, String> {
        let mut children: Vec<String> = Vec::new();

        loop {
            if self.pos >= self.chars.len() {
                if parent_tag.is_empty() {
                    return Err("unexpected EOF in fragment".into());
                }
                return Err(format!("unexpected EOF, expected </{parent_tag}>"));
            }

            // Closing tag?
            if self.current() == '<' && self.peek() == Some('/') {
                self.pos += 2; // skip </
                if parent_tag.is_empty() {
                    // Fragment close: </>
                    if self.current() == '>' {
                        self.pos += 1;
                        return Ok(children);
                    }
                }
                // Read closing tag name
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

            // Nested JSX element
            if self.at_jsx_open() {
                let child = self.parse_jsx_element()?;
                children.push(child);
                continue;
            }

            // Expression child: {expr}
            if self.current() == '{' {
                self.pos += 1; // skip {
                let expr = self.parse_jsx_expression_content()?;
                if !expr.trim().is_empty() {
                    children.push(expr);
                }
                continue;
            }

            // Text node
            let text = self.parse_jsx_text();
            if !text.is_empty() {
                children.push(format!("\"{}\"", escape_jsx_text(&text)));
            }
        }
    }

    /// Parse text content in JSX until `<` or `{`.
    fn parse_jsx_text(&mut self) -> String {
        let mut text = String::new();
        while self.pos < self.chars.len() && self.current() != '<' && self.current() != '{' {
            text.push(self.current());
            self.pos += 1;
        }
        // Trim and collapse whitespace
        let trimmed = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        trimmed
    }

    /// Parse a `{…}` expression content (balanced braces), returns the inner expression.
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
                        self.pos += 1; // consume closing }
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

/// Represents a parsed JSX prop.
#[derive(Debug)]
enum JsxProp {
    /// `name={value}` or `name="value"`
    Named(String, String),
    /// `{...expr}`
    Spread(String),
}

/// Format a tag name for React.createElement: lowercase → string, otherwise identifier.
fn format_tag(tag: &str) -> String {
    if tag.is_empty() {
        return "React.Fragment".into();
    }
    let first = tag.chars().next().unwrap();
    if first.is_lowercase() {
        format!("\"{}\"", tag)
    } else {
        tag.to_string()
    }
}

/// Format props vec into the second argument of React.createElement.
fn format_props(props: &[JsxProp]) -> String {
    if props.is_empty() {
        return "null".into();
    }

    let has_spread = props.iter().any(|p| matches!(p, JsxProp::Spread(_)));
    if has_spread {
        // Use Object.assign for spread
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
            parts.pop(); // remove trailing comma
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
                    Some(format!("{}: {}", key, value))
                } else {
                    None
                }
            })
            .collect();
        format!("{{{}}}", entries.join(", "))
    }
}

/// Check if a prop name needs quoting (contains dashes, etc.)
fn needs_quoting(name: &str) -> bool {
    name.contains('-') || name.starts_with(|c: char| c.is_ascii_digit())
}

/// Escape text for use inside a JS string literal.
fn escape_jsx_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

// ─────────────────────────────────────────────────────────────────────────────
// Helper utilities
// ─────────────────────────────────────────────────────────────────────────────

fn is_keyword_at(chars: &[char], i: usize, kw: &str) -> bool {
    let kw_chars: Vec<char> = kw.chars().collect();
    let kw_len = kw_chars.len();
    if i + kw_len > chars.len() {
        return false;
    }
    // Match keyword
    if chars[i..i + kw_len] != kw_chars[..] {
        return false;
    }
    // Must be followed by a non-identifier char (word boundary).
    let after = i + kw_len;
    if after < chars.len() && (chars[after].is_alphanumeric() || chars[after] == '_') {
        return false;
    }
    // Must be preceded by a non-identifier char (start of token).
    if i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
        return false;
    }
    true
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_end(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn is_value_token(chars: &[char], i: usize) -> bool {
    i < chars.len()
        && (is_ident_start(chars[i]) || chars[i] == '"' || chars[i] == '\'' || chars[i] == '`')
}

fn skip_spaces(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        i += 1;
    }
    i
}

fn skip_ident(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && is_ident_end(chars[i]) {
        i += 1;
    }
    i
}

fn prev_non_space(chars: &[char], from: usize) -> Option<char> {
    let mut i = from.checked_sub(1)?;
    loop {
        if chars[i] != ' ' && chars[i] != '\t' {
            return Some(chars[i]);
        }
        i = i.checked_sub(1)?;
    }
}

/// Skip to end of statement (`;` or newline at top level).
fn find_statement_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    let mut depth = 0i32;
    while i < chars.len() {
        match chars[i] {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => {
                if depth > 0 {
                    depth -= 1;
                } else {
                    return i;
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

/// Skip to end of block `{ … }`.
fn find_block_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
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

/// Skip a type annotation: `string`, `Map<string, number>`, `(A | B)[]`, etc.
fn skip_type_annotation(chars: &[char], start: usize) -> usize {
    let mut i = start;
    // Skip the first type name or keyword.
    if i >= chars.len() {
        return i;
    }

    i = skip_type_atom(chars, i);

    // Handle unions `|` and intersections `&`.
    loop {
        let j = skip_spaces(chars, i);
        if j < chars.len() && (chars[j] == '|' || chars[j] == '&') {
            let k = skip_spaces(chars, j + 1);
            let l = skip_type_atom(chars, k);
            if l > k {
                i = l;
                continue;
            }
        }
        // Handle `[]` suffix.
        if j + 1 < chars.len() && chars[j] == '[' && chars[j + 1] == ']' {
            i = j + 2;
            continue;
        }
        break;
    }

    i
}

fn skip_type_atom(chars: &[char], mut i: usize) -> usize {
    if i >= chars.len() {
        return i;
    }

    // Parenthesised type `(A | B)`.
    if chars[i] == '(' {
        let mut depth = 1;
        i += 1;
        while i < chars.len() && depth > 0 {
            if chars[i] == '(' {
                depth += 1;
            } else if chars[i] == ')' {
                depth -= 1;
            }
            i += 1;
        }
        return i;
    }

    // Identifier type.
    if !is_ident_start(chars[i]) && chars[i] != '"' && chars[i] != '\'' {
        return i;
    }

    // Quoted string literal type `"foo"`.
    if chars[i] == '"' || chars[i] == '\'' {
        let q = chars[i];
        i += 1;
        while i < chars.len() && chars[i] != q {
            i += 1;
        }
        if i < chars.len() {
            i += 1;
        }
        return i;
    }

    i = skip_ident(chars, i);

    // Generic parameters `<A, B>`.
    if i < chars.len() && chars[i] == '<' {
        if let Some(end) = try_skip_type_args(chars, i) {
            i = end;
        }
    }

    i
}

/// Try to skip a balanced `< … >` generic type arguments block.
/// Returns `None` if it doesn't look like type args (e.g. comparison operator).
fn try_skip_type_args(chars: &[char], start: usize) -> Option<usize> {
    debug_assert_eq!(chars.get(start), Some(&'<'));
    let mut depth = 1i32;
    let mut i = start + 1;

    while i < chars.len() && depth > 0 {
        match chars[i] {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            // If we hit something that can't appear in a type arg, bail.
            '=' | ';' | '\n' => return None,
            _ => {}
        }
        i += 1;
    }

    if depth == 0 {
        Some(i)
    } else {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_interface_declaration() {
        let src = "interface Foo { bar: string }\nconst x = 1;";
        let out = strip_typescript(src).unwrap();
        assert!(!out.contains("interface"));
        assert!(out.contains("const x = 1;"));
    }

    #[test]
    fn strips_type_alias() {
        let src = "type MyType = string | number;\nconst x = 1;";
        let out = strip_typescript(src).unwrap();
        assert!(!out.contains("type MyType"));
        assert!(out.contains("const x = 1;"));
    }

    #[test]
    fn strips_import_type() {
        let src = "import type { Foo } from \"./foo\";\nimport { Bar } from \"./bar\";";
        let out = strip_typescript(src).unwrap();
        assert!(!out.contains("import type"));
        assert!(out.contains("import { Bar }"));
    }

    #[test]
    fn strips_declare() {
        let src = "declare const __DEV__: boolean;\nconst x = 1;";
        let out = strip_typescript(src).unwrap();
        assert!(!out.contains("declare"));
        assert!(out.contains("const x = 1;"));
    }

    #[test]
    fn preserves_string_contents() {
        let src = r#"const s = "interface Foo { bar: string }";"#;
        let out = strip_typescript(src).unwrap();
        // String content must be preserved verbatim.
        assert!(out.contains("interface Foo { bar: string }"));
    }

    #[test]
    fn plain_js_passthrough() {
        let src = "const x = 1;\nconst y = 2;";
        let out = transform(src, false).unwrap();
        assert_eq!(out, src);
    }

    // ── JSX transformation tests ──

    #[test]
    fn jsx_simple_element() {
        let src = "const el = (<div>hello</div>);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("React.createElement(\"div\", null, \"hello\")"));
    }

    #[test]
    fn jsx_self_closing() {
        let src = "const el = (<br />);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("React.createElement(\"br\", null)"));
    }

    #[test]
    fn jsx_component_with_props() {
        let src = "const el = (<Button disabled className=\"primary\">Click</Button>);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("React.createElement(Button,"));
        assert!(out.contains("disabled: true"));
        assert!(out.contains("className: \"primary\""));
        assert!(out.contains("\"Click\""));
    }

    #[test]
    fn jsx_expression_child() {
        let src = "const el = (<span>{name}</span>);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("React.createElement(\"span\", null, name)"));
    }

    #[test]
    fn jsx_expression_prop() {
        let src = "const el = (<Input value={state.value} />);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("value: state.value"));
    }

    #[test]
    fn jsx_fragment() {
        let src = "const el = (<>first</>);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("React.createElement(React.Fragment, null, \"first\")"));
    }

    #[test]
    fn jsx_nested_elements() {
        let src = "const el = (<div><span>inner</span></div>);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains(
            "React.createElement(\"div\", null, React.createElement(\"span\", null, \"inner\"))"
        ));
    }

    #[test]
    fn jsx_spread_props() {
        let src = "const el = (<Comp {...rest} id=\"x\" />);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("Object.assign({}"));
        assert!(out.contains("rest"));
        assert!(out.contains("id: \"x\""));
    }

    #[test]
    fn jsx_data_attribute() {
        let src = "const el = (<div data-testid=\"foo\">bar</div>);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("\"data-testid\": \"foo\""));
    }

    #[test]
    fn jsx_dotted_component() {
        let src = "const el = (<Motion.div>x</Motion.div>);";
        let out = transform_jsx(src).unwrap();
        assert!(out.contains("React.createElement(Motion.div, null, \"x\")"));
    }

    #[test]
    fn full_tsx_transform() {
        let src = r#"
interface Props { name: string }
export default function Page({ name }: Props) {
  return (<div className="page">{name}</div>);
}
"#;
        let out = transform(src, true).unwrap();
        assert!(!out.contains("interface Props"));
        assert!(out.contains("React.createElement(\"div\""));
        assert!(out.contains("className: \"page\""));
        assert!(out.contains(", name)"));
    }
}
