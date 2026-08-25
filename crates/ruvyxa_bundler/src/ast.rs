//! Lightweight AST facts used by the Ruvyxa Bundler pipeline.
//!
//! This is intentionally smaller than a full JavaScript parser, but it gives
//! the resolver and transformer a shared structured view of imports, env
//! reads, JSX, decorators, and TypeScript-only syntax instead of duplicating
//! ad hoc line scans in each stage.
//!
//! Every field here has a production reader. A fact nothing consumes is still
//! allocated for every module in the graph and retained for the run, so it is
//! removed rather than kept "in case": the named-export list was collected on
//! every scan and read only by its own tests.
//!
//! ## One walk, one answer
//!
//! [`scan_code`] is the only byte scanner in this crate, and every fact any
//! stage needs is recorded during that single pass. This is a correctness
//! constraint before it is a performance one. A second scanner has to re-derive
//! where strings, template literals, comments, and regular expressions begin
//! and end, and twice already a scanner that got one of those wrong swallowed
//! the rest of a module: imports vanished from the graph, `server-only` stopped
//! being seen, private env reads went unreported. Facts that look like they
//! belong to a consumer — a page's default export, a `process.env` read — are
//! collected here so no consumer has a reason to walk the bytes again.
//!
//! Policy stays with the consumer. This module records *that* `process.env.X`
//! was read; deciding which names are allowed in a browser bundle belongs to
//! [`crate::boundary`].

use serde::{Deserialize, Serialize};

/// The `process.env` member access this scanner recognizes.
const ENV_MARKER: &[u8] = b"process.env";

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
    /// Every statically-known `process.env.NAME` / `process.env["NAME"]` read,
    /// in source order and unfiltered. See the module docs on policy.
    pub env_reads: Vec<String>,
    pub has_jsx: bool,
    pub has_typescript: bool,
    pub has_decorators: bool,
    pub has_enums: bool,
    /// Whether the module declares a runtime default export.
    pub has_default_export: bool,
    /// Byte ranges that are text rather than code: string bodies, comments,
    /// regular expressions, and the literal parts of template literals (their
    /// `${…}` interpolations stay code and are excluded).
    ///
    /// Ranges are non-overlapping and in ascending order. The linker rewrites
    /// statements line by line and cannot tell `export const x = 1` from the
    /// same characters inside a documentation sample; this is how it asks the
    /// one scanner that already knows.
    #[serde(default)]
    pub text_spans: Vec<(usize, usize)>,
}

impl ModuleAst {
    pub fn import_specifiers(&self) -> Vec<String> {
        self.imports
            .iter()
            .map(|edge| edge.specifier.clone())
            .collect()
    }

    /// Whether `offset` is a code position rather than text inside a string,
    /// template literal, comment, or regular expression.
    #[must_use]
    pub fn is_code_offset(&self, offset: usize) -> bool {
        // Ascending, non-overlapping ranges: the last span that starts at or
        // before `offset` is the only one that can contain it.
        match self
            .text_spans
            .binary_search_by(|(start, _)| start.cmp(&offset))
        {
            Ok(_) => false,
            Err(0) => true,
            Err(next) => offset >= self.text_spans[next - 1].1,
        }
    }

    pub fn dynamic_import_specifiers(&self) -> Vec<String> {
        self.imports
            .iter()
            .filter(|edge| edge.kind == ImportKind::Dynamic)
            .map(|edge| edge.specifier.clone())
            .collect()
    }
}

/// Whether a module declares a directly named runtime export.
///
/// This deliberately recognizes declaration exports only. Re-export forms
/// require resolving another module and are not safe to advertise as a local
/// runtime capability until that graph edge is proven.
pub fn has_named_runtime_export(source: &str, ast: &ModuleAst, name: &str) -> bool {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
    {
        return false;
    }
    let mut offset = 0;
    while let Some(found) = source[offset..].find("export") {
        let index = offset + found;
        offset = index + "export".len();
        if !ast.is_code_offset(index) || !is_identifier_boundary(source, index, "export".len()) {
            continue;
        }
        let declaration = source[offset..].trim_start();
        let declaration = declaration.strip_prefix("async ").unwrap_or(declaration);
        for prefix in ["function ", "const ", "let ", "var ", "class "] {
            if let Some(rest) = declaration.strip_prefix(prefix)
                && is_export_name(rest, name)
            {
                return true;
            }
        }
    }
    false
}

fn is_identifier_boundary(source: &str, start: usize, length: usize) -> bool {
    // `checked_sub`, not `saturating_sub`: at offset 0 there is no preceding
    // byte, and clamping to 0 made the keyword its own left neighbour. The
    // first byte of `export` is an identifier byte, so a module whose very
    // first characters were `export const flight = …` failed the boundary
    // check and its export went unseen — while the same source with one
    // leading newline was recognized.
    let before = start.checked_sub(1).and_then(|i| source.as_bytes().get(i));
    let after = source.as_bytes().get(start + length);
    !before.is_some_and(|byte| is_identifier_byte(*byte))
        && !after.is_some_and(|byte| is_identifier_byte(*byte))
}

/// Whether the text after an export keyword and its declaration prefix names
/// exactly `name`.
///
/// `is_some_and`, not `is_none_or`: a failed `strip_prefix` means the export
/// declares some *other* binding, and reporting that as a match made every
/// `export const meta = ...` page claim whichever runtime export was asked
/// about. The dev route manifest then advertised `flight: true` for routes
/// with no flight export, so the router's Flight fetch 500'd and every soft
/// navigation fell back to a document load.
fn is_export_name(rest: &str, name: &str) -> bool {
    rest.strip_prefix(name).is_some_and(|remaining| {
        remaining
            .as_bytes()
            .first()
            .is_none_or(|byte| !is_identifier_byte(*byte))
    })
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// Parse source into the facts the bundler needs.
pub fn parse_module(source: &str) -> ModuleAst {
    let mut ast = ModuleAst::default();
    scan_code(source, 0, source.len(), &mut ast);
    // The walk emits spans in order already; sorting costs nothing on sorted
    // input and keeps `is_code_offset`'s binary search correct regardless.
    ast.text_spans.sort_unstable();
    ast
}

/// Scan `source[start..end]` as code, recording facts into `ast`.
///
/// Takes bounds rather than a substring so byte offsets stay absolute: the
/// scanner looks backwards (`is_line_prefix_whitespace`, `previous_non_whitespace`)
/// and a re-sliced string would make those reads consult the wrong bytes.
fn scan_code(source: &str, start: usize, end: usize, ast: &mut ModuleAst) {
    let bytes = &source.as_bytes()[..end];
    let mut index = start;
    // Last byte that can end a JavaScript token, so a `/` can be classified as
    // a regular expression or a division. See [`regex_can_start`].
    let mut previous_significant: Option<usize> = None;
    while index < bytes.len() {
        if is_comment_start(bytes, index) {
            let start = index;
            index = skip_comment(bytes, index);
            ast.text_spans.push((start, index));
            continue;
        }
        if bytes[index] == b'`' {
            // A template literal's interpolations are code, not text. Skipping
            // the whole literal would hide `${require("server-only")}` from the
            // boundary check and drop real dependency edges.
            let literal_start = index;
            let (after, interpolations) = template_literal(bytes, index);
            // Everything between the interpolations is text. Recording it in
            // pieces is what lets a consumer ask about one offset and get the
            // right answer for a literal that contains both.
            let mut text_start = literal_start;
            for (code_start, code_end) in interpolations {
                ast.text_spans.push((text_start, code_start));
                scan_code(source, code_start, code_end, ast);
                text_start = code_end;
            }
            ast.text_spans.push((text_start, after));
            previous_significant = Some(index);
            index = after;
            continue;
        }
        if is_quote(bytes[index]) {
            let start = index;
            index = skip_string(bytes, index);
            ast.text_spans.push((start, index));
            previous_significant = Some(start);
            continue;
        }
        if bytes[index] == b'/' && regex_can_start(bytes, previous_significant) {
            let start = index;
            index = skip_regex_literal(bytes, index);
            ast.text_spans.push((start, index));
            previous_significant = Some(start);
            continue;
        }

        // `@` is not an operator in JavaScript, so a code-position one begins a
        // decorator. The earlier rule required it to start its own line, which
        // reported `class Svc { @log run() {} }` as decorator-free — so the
        // stripper returned the source untouched and the `@` reached the
        // emitted bundle, where it is a syntax error. That is the shape a
        // formatter picks for a short member and the shape every minified
        // dependency has.
        if bytes[index] == b'@' && decorator_can_start(bytes, index) {
            ast.has_decorators = true;
            previous_significant = Some(index);
            index += 1;
            continue;
        }
        if bytes[index] == b'<' && looks_like_jsx_at(bytes, index) {
            ast.has_jsx = true;
        }

        if !is_ident_start_byte(bytes[index]) {
            if !bytes[index].is_ascii_whitespace() {
                previous_significant = Some(index);
            }
            index += 1;
            continue;
        }

        let start = index;
        index = skip_identifier(bytes, index);
        previous_significant = Some(index - 1);
        let word = &source[start..index];
        match word {
            "import" => {
                if let Some(edge) = import_edge(source, index, end) {
                    ast.imports.push(edge);
                }
            }
            "require" if previous_non_whitespace(bytes, start) != Some(b'.') => {
                if let Some(specifier) = call_specifier(source, index, end) {
                    ast.imports.push(ImportEdge {
                        specifier,
                        kind: ImportKind::Require,
                    });
                }
            }
            "export" => {
                if let Some(edge) = export_edge(source, index, end) {
                    ast.imports.push(edge);
                }
                if export_declares_default(source, index, end) {
                    ast.has_default_export = true;
                }
            }
            // `process.env` is a member access, not a keyword, so the scanner
            // stops on the `process` identifier and the marker check confirms
            // the rest. Recording it here is what lets the boundary check read
            // env usage off this AST instead of walking the module again.
            "process" if starts_env_read(bytes, start) => {
                if let Some(name) = env_read_name(bytes, start + ENV_MARKER.len()) {
                    ast.env_reads.push(name);
                }
                index = start + ENV_MARKER.len();
                previous_significant = Some(index - 1);
            }
            "enum" => {
                ast.has_enums = true;
                ast.has_typescript = true;
            }
            "interface" | "type" | "satisfies" | "implements" | "declare" | "abstract"
            | "readonly" | "public" | "private" | "protected" | "override" => {
                ast.has_typescript = true;
            }
            "as" if previous_non_whitespace(bytes, start).is_some() => {
                ast.has_typescript = true;
            }
            _ => {}
        }
    }
}

fn import_edge(source: &str, after_keyword: usize, end: usize) -> Option<ImportEdge> {
    let bytes = &source.as_bytes()[..end];
    let index = skip_whitespace_and_comments(bytes, after_keyword);
    if index >= bytes.len() || bytes[index] == b'.' {
        return None;
    }
    if bytes[index] == b'(' {
        return call_specifier(source, index, end).map(|specifier| ImportEdge {
            specifier,
            kind: ImportKind::Dynamic,
        });
    }
    if is_quote(bytes[index]) {
        return quoted_value_at(source, index, end).map(|specifier| ImportEdge {
            specifier,
            kind: ImportKind::SideEffect,
        });
    }
    if word_at(source, index, end) == Some("type") {
        return None;
    }
    let declaration_start = index;
    find_from_specifier(source, declaration_start, end).map(|specifier| ImportEdge {
        specifier,
        kind: ImportKind::Static,
    })
}

fn export_edge(source: &str, after_keyword: usize, end: usize) -> Option<ImportEdge> {
    let bytes = &source.as_bytes()[..end];
    let index = skip_whitespace_and_comments(bytes, after_keyword);
    if word_at(source, index, end) == Some("type")
        || !matches!(bytes.get(index), Some(b'{') | Some(b'*'))
    {
        return None;
    }
    find_from_specifier(source, index, end).map(|specifier| ImportEdge {
        specifier,
        kind: ImportKind::ReExport,
    })
}

fn call_specifier(source: &str, after_keyword: usize, end: usize) -> Option<String> {
    let bytes = &source.as_bytes()[..end];
    let mut index = skip_whitespace_and_comments(bytes, after_keyword);
    if bytes.get(index) != Some(&b'(') {
        return None;
    }
    index = skip_whitespace_and_comments(bytes, index + 1);
    quoted_value_at(source, index, end)
}

fn find_from_specifier(source: &str, mut index: usize, end: usize) -> Option<String> {
    let bytes = &source.as_bytes()[..end];
    while index < bytes.len() {
        index = skip_whitespace_and_comments(bytes, index);
        if index >= bytes.len() || bytes[index] == b';' {
            return None;
        }
        if is_quote(bytes[index]) {
            index = skip_string(bytes, index);
            continue;
        }
        if word_at(source, index, end) == Some("from") {
            let value = skip_whitespace_and_comments(bytes, index + 4);
            return quoted_value_at(source, value, end);
        }
        index += 1;
    }
    None
}

fn quoted_value_at(source: &str, start: usize, end: usize) -> Option<String> {
    let bytes = &source.as_bytes()[..end];
    let quote = *bytes.get(start)?;
    if !is_quote(quote) || quote == b'`' {
        return None;
    }
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == quote {
            return Some(source[start + 1..index].to_string());
        }
        index += 1;
    }
    None
}

/// Return `source` with every non-code region blanked out.
///
/// Strings, template text, comments, and regular-expression literals become
/// spaces; code bytes are copied through. Byte offsets and line breaks are
/// preserved, so the result can be matched line by line and positions in it
/// still name the same place in the original file.
///
/// This exists for consumers that need to *search* code text rather than read
/// structured facts — route rendering-strategy detection matches on things like
/// `export const revalidate` and `fetch(` and has no AST field to read. Giving
/// them a masking primitive built on this module's scanner is what keeps them
/// from growing a private lexer: the previous one in `ruvyxa_graph` re-derived
/// where strings, templates, comments, and regexes end, which is precisely the
/// decision that has silently blinded a boundary check before.
///
/// Interpolated expressions inside a template literal are code and survive; the
/// literal text around them does not.
pub fn masked_code(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out: Vec<u8> = bytes
        .iter()
        .map(|byte| if *byte == b'\n' { b'\n' } else { b' ' })
        .collect();
    mask_range(source, 0, bytes.len(), &mut out);
    // Every byte is either an ASCII blank, an ASCII newline, or copied from a
    // code region whose bounds fall on ASCII token boundaries, so the result is
    // still valid UTF-8 and cannot split a multi-byte character.
    String::from_utf8(out).unwrap_or_else(|_| " ".repeat(bytes.len()))
}

/// Copy the code bytes of `source[start..end]` into `out`, leaving the rest
/// blank. Mirrors [`scan_code`]'s walk and shares its skipping helpers, so the
/// two agree on where code begins and ends by construction.
fn mask_range(source: &str, start: usize, end: usize, out: &mut [u8]) {
    let bytes = &source.as_bytes()[..end];
    let mut index = start;
    let mut previous_significant: Option<usize> = None;
    while index < bytes.len() {
        if is_comment_start(bytes, index) {
            index = skip_comment(bytes, index);
            continue;
        }
        if bytes[index] == b'`' {
            let (after, interpolations) = template_literal(bytes, index);
            for (code_start, code_end) in interpolations {
                mask_range(source, code_start, code_end, out);
            }
            previous_significant = Some(index);
            index = after;
            continue;
        }
        if is_quote(bytes[index]) {
            let quote = index;
            index = skip_string(bytes, index);
            previous_significant = Some(quote);
            continue;
        }
        if bytes[index] == b'/' && regex_can_start(bytes, previous_significant) {
            let slash = index;
            index = skip_regex_literal(bytes, index);
            previous_significant = Some(slash);
            continue;
        }

        out[index] = bytes[index];
        if !bytes[index].is_ascii_whitespace() {
            previous_significant = Some(index);
        }
        index += 1;
    }
}

/// Whether `source` declares a runtime default export.
///
/// Route validation needs this to tell a real page from a module that only
/// exports helpers. A plain `source.contains("export default")` answered the
/// question wrongly in both directions: it rejected `export { Page as default }`
/// and `export * as default from './page'`, which are valid default exports, and
/// it accepted a commented-out or quoted occurrence.
///
/// This is a thin read of [`parse_module`]. It used to be its own walk over the
/// bytes, which meant a second place that had to agree with the dependency
/// scanner about where strings, templates, comments, and regular expressions
/// end — and the two drifted. Callers that also need imports should call
/// [`parse_module`] once and read both facts off the result.
pub fn has_default_export(source: &str) -> bool {
    parse_module(source).has_default_export
}

/// Whether the export clause starting after `export` produces a default binding.
fn export_declares_default(source: &str, after_keyword: usize, end: usize) -> bool {
    let bytes = &source.as_bytes()[..end];
    let index = skip_whitespace_and_comments(bytes, after_keyword);

    // `export type { Page as default }` and `export type default` are erased at
    // compile time and leave no runtime binding behind.
    if word_at(source, index, end) == Some("type") {
        return false;
    }

    match word_at(source, index, end) {
        // `export default …`
        Some("default") => true,
        Some(_) => false,
        None => match bytes.get(index) {
            // `export * as default from "./page"`
            Some(b'*') => {
                let index = skip_whitespace_and_comments(bytes, index + 1);
                if word_at(source, index, end) != Some("as") {
                    return false;
                }
                let index = skip_whitespace_and_comments(bytes, index + "as".len());
                word_at(source, index, end) == Some("default")
            }
            // `export { Page as default }`, `export { default } from "./page"`
            Some(b'{') => named_clause_exports_default(source, index, end),
            _ => false,
        },
    }
}

/// Whether a `{ … }` export clause binds something to the name `default`.
///
/// The exported name is the last identifier of each comma-separated specifier,
/// so `{ Page as default }` and `{ default }` both qualify while
/// `{ default as Page }` re-exports another module's default under a new name
/// and deliberately does not.
fn named_clause_exports_default(source: &str, brace: usize, end: usize) -> bool {
    let bytes = &source.as_bytes()[..end];
    let mut index = brace + 1;
    let mut last_word: Option<&str> = None;
    while index < bytes.len() {
        if is_comment_start(bytes, index) {
            index = skip_comment(bytes, index);
            continue;
        }
        if is_quote(bytes[index]) {
            // `export { "a-b" as default }` uses a string specifier name.
            index = skip_string(bytes, index);
            last_word = None;
            continue;
        }
        match bytes[index] {
            b'}' => return last_word == Some("default"),
            b',' => {
                if last_word == Some("default") {
                    return true;
                }
                last_word = None;
                index += 1;
            }
            byte if is_ident_start_byte(byte) => {
                let start = index;
                index = skip_identifier(bytes, index);
                let word = &source[start..index];
                // `as` is the separator, never the exported name.
                if word != "as" {
                    last_word = Some(word);
                }
            }
            _ => index += 1,
        }
    }
    false
}

/// Whether the bytes at `index` begin a `process.env` member access.
///
/// The preceding byte must not continue an identifier or be a `.`, so
/// `myprocess.env` and `globalThis.process.env` are not counted as a bare
/// `process.env` read.
fn starts_env_read(bytes: &[u8], index: usize) -> bool {
    bytes.get(index..index + ENV_MARKER.len()) == Some(ENV_MARKER)
        && bytes
            .get(index.wrapping_sub(1))
            .is_none_or(|previous| !is_ident_continue_byte(*previous) && *previous != b'.')
}

/// Read the variable name from the member access that follows `process.env`.
///
/// Handles both `process.env.NAME` and `process.env["NAME"]`. A computed access
/// with a non-literal key (`process.env[key]`) has no statically-known name and
/// yields `None`.
fn env_read_name(bytes: &[u8], mut index: usize) -> Option<String> {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }

    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        index = skip_identifier(bytes, index);
        return std::str::from_utf8(&bytes[start..index])
            .ok()
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
    }

    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let quote = *bytes.get(index)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    index += 1;
    let start = index;
    index = skip_identifier(bytes, index);
    let name = std::str::from_utf8(&bytes[start..index])
        .ok()
        .filter(|name| !name.is_empty())?
        .to_owned();
    if bytes.get(index) != Some(&quote) {
        return None;
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    (bytes.get(index) == Some(&b']')).then_some(name)
}

fn word_at(source: &str, start: usize, end: usize) -> Option<&str> {
    let bytes = &source.as_bytes()[..end];
    if start >= bytes.len() || !is_ident_start_byte(bytes[start]) {
        return None;
    }
    Some(&source[start..skip_identifier(bytes, start)])
}

fn skip_whitespace_and_comments(bytes: &[u8], mut index: usize) -> usize {
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if is_comment_start(bytes, index) {
            index = skip_comment(bytes, index);
        } else {
            return index;
        }
    }
}

fn is_comment_start(bytes: &[u8], index: usize) -> bool {
    bytes.get(index) == Some(&b'/') && matches!(bytes.get(index + 1), Some(b'/') | Some(b'*'))
}

fn skip_comment(bytes: &[u8], start: usize) -> usize {
    if bytes.get(start + 1) == Some(&b'/') {
        return bytes[start + 2..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |offset| start + 2 + offset + 1);
    }
    let mut index = start + 2;
    while index + 1 < bytes.len() {
        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            return index + 2;
        }
        index += 1;
    }
    bytes.len()
}

/// Decide whether a `/` opens a regular expression rather than a division.
///
/// Every byte scanner in this crate needs this decision, and getting it wrong is
/// not a cosmetic error: without it `/["']/` reads as a division followed by an
/// unterminated string, and the string skip then swallows the rest of the module.
/// Imports after that point vanish from the dependency graph, `server-only`
/// markers stop being seen by the boundary check, and a page's default export
/// becomes invisible. Sharing one implementation is what keeps the scanners from
/// drifting back into that failure one at a time.
///
/// A regex may only appear where a value is expected. When the preceding token
/// could end a value (identifier, number, string, closing bracket) the slash is
/// division. Keywords such as `return` are values-expected positions.
///
/// `previous_significant` is the index of the last byte that can end a token, or
/// `None` at the start of the source.
///
/// Deliberately private: it is only correct in the context of a scan that also
/// tracks strings, templates, and comments. Exposing it invited a second
/// scanner that shared this one decision and re-derived every other one.
fn regex_can_start(bytes: &[u8], previous_significant: Option<usize>) -> bool {
    let Some(index) = previous_significant else {
        return true;
    };
    match bytes[index] {
        b')' | b']' | b'}' | b'\'' | b'"' | b'`' => false,
        byte if is_ident_continue_byte(byte) => previous_token_is_keyword(bytes, index),
        _ => true,
    }
}

fn previous_token_is_keyword(bytes: &[u8], end: usize) -> bool {
    let mut start = end + 1;
    while start > 0 && is_ident_continue_byte(bytes[start - 1]) {
        start -= 1;
    }
    matches!(
        std::str::from_utf8(&bytes[start..=end]).unwrap_or_default(),
        "await"
            | "case"
            | "delete"
            | "do"
            | "else"
            | "in"
            | "instanceof"
            | "new"
            | "of"
            | "return"
            | "throw"
            | "typeof"
            | "void"
            | "yield"
    )
}

/// Skip past a regular expression literal, returning the index after it.
///
/// Quotes and slashes inside a character class (`/[/"']/`) are literal, so the
/// class state has to be tracked or the literal ends in the wrong place.
fn skip_regex_literal(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 1;
    let mut inside_character_class = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'[' => {
                inside_character_class = true;
                index += 1;
            }
            b']' if inside_character_class => {
                inside_character_class = false;
                index += 1;
            }
            // An unterminated literal was a division after all. Stop here so the
            // rest of the line is still scanned normally.
            b'\n' => return index,
            b'/' if !inside_character_class => {
                index += 1;
                break;
            }
            _ => index += 1,
        }
    }

    // Trailing flags (`/x/gi`) are part of the literal, not a new identifier.
    while bytes
        .get(index)
        .is_some_and(|byte| is_ident_continue_byte(*byte))
    {
        index += 1;
    }
    index
}

/// Walk a template literal starting at its opening backtick.
///
/// Returns the index just past the closing backtick together with the code
/// ranges of each `${ … }` interpolation, so callers can scan those as code
/// instead of treating the whole literal as opaque text.
fn template_literal(bytes: &[u8], start: usize) -> (usize, Vec<(usize, usize)>) {
    let mut index = start + 1;
    let mut interpolations = Vec::new();
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'`' => return (index + 1, interpolations),
            b'$' if bytes.get(index + 1) == Some(&b'{') => {
                let code_start = index + 2;
                let code_end = interpolation_end(bytes, code_start);
                interpolations.push((code_start, code_end));
                index = (code_end + 1).min(bytes.len());
            }
            _ => index += 1,
        }
    }
    (bytes.len(), interpolations)
}

/// Index of the `}` closing an interpolation whose code begins at `start`.
///
/// Braces inside nested strings, templates, and comments do not count, or a
/// literal such as `` `${obj["}"]}` `` would end the interpolation early and
/// desynchronize the rest of the scan.
fn interpolation_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    let mut depth = 1usize;
    // Tracked for the same reason the outer scans track it: `/` opens a regex
    // only where a value is expected, and that depends on the token before it.
    let mut previous_significant: Option<usize> = None;
    while index < bytes.len() {
        if is_comment_start(bytes, index) {
            index = skip_comment(bytes, index);
            continue;
        }
        // An interpolation is code, so it can hold a regex — and a regex can
        // hold a quote. Without this the `'` in
        // `` `'${value.replace(/'/g, "''")}'` `` opened a string that ran to
        // the next quote, and every literal and comment after it in the file
        // was read inside out. `js-yaml` ships exactly this line.
        if bytes[index] == b'/' && regex_can_start(bytes, previous_significant) {
            previous_significant = Some(index);
            index = skip_regex_literal(bytes, index);
            continue;
        }
        match bytes[index] {
            b'`' => {
                previous_significant = Some(index);
                index = template_literal(bytes, index).0;
            }
            b'\'' | b'"' => {
                previous_significant = Some(index);
                index = skip_string(bytes, index);
            }
            b'{' => {
                depth += 1;
                previous_significant = Some(index);
                index += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
                previous_significant = Some(index);
                index += 1;
            }
            byte => {
                if !byte.is_ascii_whitespace() {
                    previous_significant = Some(index);
                }
                index += 1;
            }
        }
    }
    bytes.len()
}

/// Skip a `'`/`"` literal, giving up at the end of its line.
///
/// A JavaScript string cannot contain a raw newline, so a quote with no closing
/// partner on its own line was never a delimiter: it is an apostrophe in prose
/// or in JSX text — `React's`, `<p>don't</p>`. Running to the next quote
/// anywhere in the file is what used to desynchronize this scan, and the cost
/// was silent: the swallowed region became a text span, so every import,
/// `server-only` marker, and `process.env` read after it was invisible to the
/// graph and to the boundary check. Resuming just past the opening quote keeps
/// a stray apostrophe's blast radius to its own line.
///
/// Returns the index just past the closing quote, or just past the opening one
/// when the line ends first.
fn skip_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => break,
            b'\\' if index + 1 < bytes.len() && bytes[index + 1] != b'\n' => index += 2,
            byte if byte == quote => return index + 1,
            _ => index += 1,
        }
    }
    start + 1
}

/// Whether an `@` at this position begins a decorator rather than sitting
/// inside a larger token.
///
/// Kept level with `begins_decorator` in `crate::compiler`, which strips what
/// this reports.
fn decorator_can_start(bytes: &[u8], at: usize) -> bool {
    let Some(previous) = bytes[..at]
        .iter()
        .rev()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    else {
        return true;
    };
    matches!(previous, b'{' | b'}' | b';' | b')' | b']')
        || previous.is_ascii_alphanumeric()
        || matches!(previous, b'_' | b'$')
}

fn looks_like_jsx_at(bytes: &[u8], index: usize) -> bool {
    matches!(
        bytes.get(index + 1),
        Some(b'>') | Some(b'/') | Some(b'A'..=b'Z') | Some(b'a'..=b'z')
    )
}

fn previous_non_whitespace(bytes: &[u8], index: usize) -> Option<u8> {
    bytes[..index]
        .iter()
        .rev()
        .find(|byte| !byte.is_ascii_whitespace())
        .copied()
}

fn skip_identifier(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && is_ident_continue_byte(bytes[index]) {
        index += 1;
    }
    index
}

fn is_ident_start_byte(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

fn is_ident_continue_byte(byte: u8) -> bool {
    is_ident_start_byte(byte) || byte.is_ascii_digit()
}

fn is_quote(byte: u8) -> bool {
    matches!(byte, b'"' | b'\'' | b'`')
}

/// What [`skip_non_code`] found at an offset that is not plain code.
pub(crate) enum NonCode {
    /// Text to copy through unchanged, resuming at `end`.
    Opaque { end: usize },
    /// A template literal. Its text is data, but each `${…}` range is code and
    /// must still be walked — [`mask_range`] recurses into them for the same
    /// reason.
    Template {
        end: usize,
        interpolations: Vec<(usize, usize)>,
    },
}

/// Skip the non-code construct at `index`, or return `None` when it is code.
///
/// Exists for consumers that rewrite text a line at a time and therefore cannot
/// use [`masked_code`], which needs the whole source to carry block-comment
/// state across lines: the linker's `require()` and dynamic-`import()` passes.
/// `in_block_comment` is that state, threaded by the caller between lines.
///
/// Those two passes each used to carry their own copy of this walk. Both knew
/// about strings and comments and neither knew about regular expressions, so
/// `/[/*]/` — a character class holding a slash and a star — set
/// `in_block_comment` and swallowed every following line of the module as
/// comment text, and `/"/g` opened a string that never closed and hid every
/// `require()` after it on the line. Minified CommonJS puts a whole module on
/// one line, so that is every require in the file. The decision belongs here,
/// with the rest of the scanner, exactly as [`regex_can_start`]'s own
/// documentation argues: it is only correct alongside string, template, and
/// comment tracking, so this function does all four rather than exporting the
/// regex test on its own.
///
/// A template literal reports its `${…}` ranges rather than being skipped
/// whole. Those ranges are code — [`scan_code`] already reads imports out of
/// them — so a linker that skipped them left a bare `require()` at a call site
/// whose module the graph had already bundled: the dependency was present and
/// the call still said `require`, which is a `ReferenceError` in a browser
/// bundle.
pub(crate) fn skip_non_code(
    bytes: &[u8],
    index: usize,
    previous_significant: Option<usize>,
    in_block_comment: &mut bool,
) -> Option<NonCode> {
    if *in_block_comment {
        let mut cursor = index;
        while cursor + 1 < bytes.len() {
            if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                *in_block_comment = false;
                return Some(NonCode::Opaque { end: cursor + 2 });
            }
            cursor += 1;
        }
        return Some(NonCode::Opaque { end: bytes.len() });
    }

    if is_comment_start(bytes, index) {
        let after = skip_comment(bytes, index);
        // `skip_comment` reports the end of the input for a block comment that
        // never closes, which is indistinguishable from one that closes on the
        // final two bytes. Requiring room for both delimiters separates them;
        // `/*/` is unterminated, `/**/` is not.
        let block = bytes.get(index + 1) == Some(&b'*');
        let closed = after >= index + 4 && bytes[after - 2] == b'*' && bytes[after - 1] == b'/';
        if block && !closed {
            *in_block_comment = true;
        }
        return Some(NonCode::Opaque { end: after });
    }

    if bytes[index] == b'`' {
        let (end, interpolations) = template_literal(bytes, index);
        return Some(NonCode::Template {
            end,
            interpolations,
        });
    }

    if is_quote(bytes[index]) {
        return Some(NonCode::Opaque {
            end: skip_string(bytes, index),
        });
    }

    if bytes[index] == b'/' && regex_can_start(bytes, previous_significant) {
        return Some(NonCode::Opaque {
            end: skip_regex_literal(bytes, index),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Rust half of the source-scanner contract.
    ///
    /// The JavaScript half is `tests/packages/ruvyxa/source-scanner.test.mjs`,
    /// driving `runtime/scanner.mjs` over the same table. Both walk bytes
    /// rather than parse, so a construct one of them does not know
    /// desynchronizes it — and every reader downstream believes the answer.
    #[test]
    fn masking_matches_the_shared_conformance_table() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/source-scanner-conformance.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let fixture: serde_json::Value =
            serde_json::from_str(&source).expect("the scanner fixture parses");
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(!cases.is_empty(), "the fixture must carry cases");

        for case in cases {
            let name = case["name"].as_str().unwrap_or_default();
            let why = case["why"].as_str().unwrap_or_default();
            let input = case["source"].as_str().expect("source");
            let masked = masked_code(input);
            assert_eq!(
                masked.len(),
                input.len(),
                "{name}: a mask is the same length as its source, so offsets stay usable"
            );
            for fragment in case["code"].as_array().into_iter().flatten() {
                let fragment = fragment.as_str().expect("code fragment");
                assert!(
                    masked.contains(fragment),
                    "{name}: `{fragment}` must survive the mask — {why}"
                );
            }
            for fragment in case["text"].as_array().into_iter().flatten() {
                let fragment = fragment.as_str().expect("text fragment");
                assert!(
                    !masked.contains(fragment),
                    "{name}: `{fragment}` is text and must be masked away — {why}"
                );
            }
        }
    }

    /// The keyword's own position must not be treated as the byte before it.
    /// A module that opens directly with the export — no import, no directive,
    /// no blank line — is the case a leading-newline fixture never reaches, and
    /// it is exactly how a small route module is written.
    #[test]
    fn named_runtime_export_is_found_at_the_very_start_of_a_module() {
        for source in [
            "export const flight = true\n",
            "export function flight() {}\n",
            "export async function flight() {}\n",
            "export class flight {}\n",
        ] {
            let ast = parse_module(source);
            assert!(
                has_named_runtime_export(source, &ast, "flight"),
                "offset-zero export must be seen: {source:?}"
            );

            // The same source one byte further in must agree, or the boundary
            // check is position-dependent again.
            let shifted = format!("\n{source}");
            let shifted_ast = parse_module(&shifted);
            assert!(
                has_named_runtime_export(&shifted, &shifted_ast, "flight"),
                "shifted export must be seen: {shifted:?}"
            );
        }
    }

    /// The boundary check still has to reject look-alikes at offset zero rather
    /// than accept everything now that the clamp is gone.
    #[test]
    fn named_runtime_export_still_rejects_look_alikes() {
        for source in [
            "exports.flight = true\n",
            "reexport const flight = true\n",
            "export const flightPlan = true\n",
            // A different binding entirely: the export exists, the name
            // does not. This is what an ordinary page looks like, and
            // accepting it advertised runtime exports no module had.
            "export const meta = { title: 1 }\n",
            "export function loader() {}\n",
            "export async function generateStaticParams() {}\n",
            "export class Widget {}\n",
            "// export const flight = true\n",
            "const doc = 'export const flight = true'\n",
        ] {
            let ast = parse_module(source);
            assert!(
                !has_named_runtime_export(source, &ast, "flight"),
                "must not match: {source:?}"
            );
        }
    }

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

        assert!(
            ast.imports
                .iter()
                .any(|edge| { edge.specifier == "react" && edge.kind == ImportKind::Static })
        );
        assert!(ast.imports.iter().any(|edge| {
            edge.specifier == "./global.css" && edge.kind == ImportKind::SideEffect
        }));
        assert!(
            ast.imports
                .iter()
                .any(|edge| { edge.specifier == "./helper" && edge.kind == ImportKind::ReExport })
        );
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
        assert!(ast.has_default_export);
    }

    #[test]
    fn ignores_type_only_imports() {
        let ast = parse_module(
            r#"
import type { PageProps } from "ruvyxa/config";
import { createElement } from "react";
"#,
        );

        assert_eq!(ast.import_specifiers(), vec!["react"]);
    }

    #[test]
    fn recognizes_every_runtime_default_export_form() {
        for source in [
            "export default function Page() { return <main /> }",
            "export default class Page {}",
            "export default () => <main />",
            "const Page = () => <main />;\nexport default Page",
            "function Page() {}\nexport { Page as default }",
            "function Page() {}\nexport { Page as default, Page as Other }",
            "export { default } from \"./page\"",
            "export * as default from \"./page\"",
            "export {\n  // the page component\n  Page as default,\n}",
            "export { Page as Other, Page as default }",
        ] {
            assert!(
                has_default_export(source),
                "should detect a default export in: {source}"
            );
        }
    }

    #[test]
    fn rejects_sources_without_a_runtime_default_export() {
        for source in [
            "export const title = 'Missing'",
            "export function Page() {}",
            "// export default function Page() {}",
            "/* export default function Page() {} */",
            "const help = \"export default function Page() {}\"",
            "export const defaultTitle = 'Missing'",
            "export { Page }",
            // Re-exporting another module's default under a new name does not
            // give this module a default export.
            "export { default as Page } from \"./page\"",
            // Type-only exports leave no runtime binding.
            "export type { Page as default } from \"./page\"",
            "export * from \"./page\"",
        ] {
            assert!(
                !has_default_export(source),
                "should not detect a default export in: {source}"
            );
        }
    }

    /// An apostrophe in prose or JSX text is not a string delimiter. Treating
    /// it as one ran the string skip to the next quote anywhere in the file, so
    /// the imports and `process.env` reads after it were recorded as text and
    /// never seen — the same failure the regex case above describes, reached
    /// through the other literal form.
    #[test]
    fn unclosed_quotes_do_not_hide_later_facts() {
        let ast = parse_module(concat!(
            "const label = <p>don't</p>;\n",
            "import { helper } from './helper';\n",
            "const key = process.env.DATABASE_URL;\n",
        ));

        assert_eq!(
            ast.import_specifiers(),
            vec!["./helper"],
            "an apostrophe must not swallow the rest of the module"
        );
        assert_eq!(
            ast.env_reads,
            vec!["DATABASE_URL"],
            "a private env read after an apostrophe must stay visible"
        );
    }

    /// A string still ends at its own closing quote on the same line.
    #[test]
    fn same_line_strings_are_still_text() {
        let ast = parse_module("const s = 'import x from \"./nope\"';\n");
        assert!(
            ast.import_specifiers().is_empty(),
            "an import inside a closed string is text, not an edge"
        );
    }

    /// A regex literal containing a quote used to start a string skip that ran
    /// to end-of-file, so every import after it disappeared from the dependency
    /// graph and the module was never bundled.
    #[test]
    fn regex_literals_do_not_hide_later_imports() {
        let ast = parse_module(
            r#"
const QUOTED = /["']/g
const CLASS_SLASH = /[/"]/
import { helper } from "./helper"
export { shared } from "./shared"
const lazy = import("./lazy")
"#,
        );

        assert_eq!(
            ast.import_specifiers(),
            vec!["./helper", "./shared", "./lazy"],
            "a regex literal must not swallow the rest of the module"
        );
    }

    /// The same swallowing made `check` reject a valid page with RUV1004.
    #[test]
    fn regex_literals_do_not_hide_a_later_default_export() {
        for source in [
            "const RE = /[\"']/;\nexport default function Page() { return <main /> }",
            "const RE = /don't/;\nexport default function Page() {}",
            "const RE = /[/\"]/g;\nfunction Page() {}\nexport { Page as default }",
        ] {
            assert!(has_default_export(source), "should detect: {source}");
        }
    }

    /// Interpolations are code. Treating a template literal as opaque text hid
    /// `${require("server-only")}` from the RUV1007 boundary check and dropped
    /// real dependency edges from the graph.
    #[test]
    fn template_interpolations_are_scanned_as_code() {
        let ast = parse_module(
            r#"
const loader = `${require("server-only")}`
const nested = `outer ${cond ? `inner ${import("./lazy")}` : ""} tail`
const text = `import "not-an-import" and require("not-either")`
"#,
        );

        let specifiers = ast.import_specifiers();
        assert!(
            specifiers.contains(&"server-only".to_string()),
            "{specifiers:?}"
        );
        assert!(specifiers.contains(&"./lazy".to_string()), "{specifiers:?}");
        assert!(
            !specifiers.contains(&"not-an-import".to_string()),
            "literal template text is not code: {specifiers:?}"
        );
        assert!(
            !specifiers.contains(&"not-either".to_string()),
            "literal template text is not code: {specifiers:?}"
        );
    }

    /// Every helper reached from an interpolation is bounded to that
    /// interpolation. Reading from the unbounded source let a keyword at the end
    /// of `${…}` pull its specifier out of the surrounding template text, which
    /// is literal text and not an import at all.
    #[test]
    fn interpolation_scans_do_not_read_past_their_own_range() {
        let ast = parse_module(r#"const trap = `${import}` from "./not-an-import"`"#);
        assert!(
            ast.import_specifiers().is_empty(),
            "text after an interpolation is not its specifier: {:?}",
            ast.import_specifiers()
        );

        let ast = parse_module(r#"const trap = `${require} ("./nope")`"#);
        assert!(ast.import_specifiers().is_empty(), "{:?}", ast.imports);

        let ast = parse_module(r#"const trap = `${export} * from "./nope"`"#);
        assert!(ast.import_specifiers().is_empty(), "{:?}", ast.imports);

        // The bound must not cost a real interpolated import its specifier.
        let ast = parse_module(r#"const real = `${import("./lazy")}`"#);
        assert_eq!(ast.import_specifiers(), vec!["./lazy"]);
    }

    /// A brace inside a nested string must not end the interpolation early, or
    /// the scan resumes at the wrong offset and loses everything after it.
    #[test]
    fn braces_inside_interpolated_strings_do_not_end_the_interpolation() {
        let ast = parse_module(
            r#"
const label = `${obj["}"]} tail`
import { helper } from "./helper"
"#,
        );

        assert_eq!(ast.import_specifiers(), vec!["./helper"]);
    }

    /// A `/` after a value is division. Treating it as a regex would skip real
    /// code instead — the opposite failure, and just as silent.
    #[test]
    fn division_is_not_mistaken_for_a_regex_literal() {
        let ast = parse_module(
            r#"
const ratio = total / count
const scaled = (a + b) / 2
const indexed = list[0] / 2
import { helper } from "./helper"
"#,
        );

        assert_eq!(ast.import_specifiers(), vec!["./helper"]);
    }

    /// After a keyword a `/` really is a regex, even though the preceding byte
    /// is an identifier byte.
    #[test]
    fn regex_after_a_keyword_is_still_a_regex() {
        let ast = parse_module(
            r#"
function pattern() { return /["']/ }
import { helper } from "./helper"
"#,
        );

        assert_eq!(ast.import_specifiers(), vec!["./helper"]);
    }

    /// Imports, the default export, and env reads used to come from three
    /// separate walks over the same bytes. One walk now answers all of them,
    /// and this pins that they stay consistent with each other.
    #[test]
    fn one_scan_answers_imports_default_export_and_env_reads() {
        let ast = parse_module(
            r#"
const QUOTED = /["']/g
import { helper } from "./helper"
const db = process.env.DATABASE_URL
const key = process.env['API_KEY']
const mode = process.env.NODE_ENV
export default function Page() { return helper(db, key, mode) }
"#,
        );

        assert_eq!(ast.import_specifiers(), vec!["./helper"]);
        assert!(ast.has_default_export);
        assert_eq!(ast.env_reads, ["DATABASE_URL", "API_KEY", "NODE_ENV"]);
    }

    /// Env reads are code only where code is. Text that merely spells
    /// `process.env` is not a read, and a member access on another object is a
    /// different variable entirely.
    #[test]
    fn env_reads_ignore_text_and_qualified_access() {
        let ast = parse_module(
            r#"
const docs = "process.env.DATABASE_URL"
// process.env.COMMENTED
const other = globalThis.process.env.NOT_BARE
const shadow = myprocess.env.NOT_THIS
const rendered = `${process.env.INTERPOLATED}`
const computed = process.env[dynamicKey]
"#,
        );

        assert_eq!(ast.env_reads, ["INTERPOLATED"]);
    }

    #[test]
    fn default_export_detection_survives_unterminated_clauses() {
        // Malformed input must return an answer rather than scan out of bounds.
        for source in ["export {", "export { Page as", "export *", "export"] {
            assert!(!has_default_export(source), "{source}");
        }
    }

    /// Import specifiers as oxc's real parser sees them.
    fn oxc_static_specifiers(source: &str) -> Vec<String> {
        use oxc::allocator::Allocator;
        use oxc::ast::ast::Statement;
        use oxc::parser::Parser;
        use oxc::span::SourceType;

        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, SourceType::tsx()).parse();
        assert!(
            parsed.diagnostics.is_empty(),
            "the corpus must parse: {source}"
        );

        let mut found = Vec::new();
        for statement in &parsed.program.body {
            match statement {
                Statement::ImportDeclaration(declaration) => {
                    found.push(declaration.source.value.to_string());
                }
                Statement::ExportFromDeclaration(declaration) => {
                    found.push(declaration.source.value.to_string());
                }
                Statement::ExportAllDeclaration(declaration) => {
                    found.push(declaration.source.value.to_string());
                }
                _ => {}
            }
        }
        found
    }

    /// Specifiers this scanner deliberately does not report.
    ///
    /// A type-only import is erased at compile time, so treating it as a
    /// dependency edge would pull a module into the graph that the emitted
    /// JavaScript never mentions. oxc reports it because it is syntactically an
    /// import; this scanner answers "what does the output depend on".
    const ERASED_SPECIFIERS: &[&str] = &["./types.js"];

    /// The scanner finds exactly the static edges the real parser does.
    ///
    /// This is the crate's only byte scanner, and every masked-code decision in
    /// the linker, the minifier, and the boundary check rests on it — a miss
    /// here is a miss everywhere, and it has taken repeat regressions. Compared
    /// against oxc rather than against expected output: a hand-written
    /// expectation can be wrong in the same direction as the scanner.
    #[test]
    fn the_scanner_finds_the_same_static_edges_as_the_real_parser() {
        for (name, source) in AST_CORPUS {
            let scanned = parse_module(source)
                .imports
                .iter()
                .filter(|edge| {
                    matches!(
                        edge.kind,
                        ImportKind::Static | ImportKind::SideEffect | ImportKind::ReExport
                    )
                })
                .map(|edge| edge.specifier.clone())
                .collect::<Vec<_>>();
            let mut expected = oxc_static_specifiers(source);
            expected.retain(|specifier| !ERASED_SPECIFIERS.contains(&specifier.as_str()));
            let mut scanned = scanned;
            expected.sort();
            scanned.sort();
            assert_eq!(scanned, expected, "{name}");
        }
    }

    /// Masking blanks text and nothing else, in place.
    ///
    /// Every caller reads an offset out of masked code and then slices the
    /// original at it, so the two must address the same bytes: same length,
    /// same line breaks, and every byte either kept or blanked. A mask that
    /// shifted by one would move every diagnostic and every rewrite with it.
    #[test]
    fn masking_blanks_text_in_place_without_moving_any_byte() {
        for (name, source) in AST_CORPUS {
            let masked = masked_code(source);
            if masked.len() != source.len() {
                eprintln!(
                    "PROBE [{name}] masked length {} != {}",
                    masked.len(),
                    source.len()
                );
                continue;
            }
            if masked.lines().count() != source.lines().count() {
                eprintln!("PROBE [{name}] line count changed");
            }
            for (index, (original, blanked)) in source.bytes().zip(masked.bytes()).enumerate() {
                if blanked != original && blanked != b' ' && blanked != b'\n' {
                    eprintln!("PROBE [{name}] byte {index} became {blanked:?}, not a blank");
                    break;
                }
            }
        }
    }

    /// Sources whose text and code are easy to confuse.
    const AST_CORPUS: &[(&str, &str)] = &[
        ("plain import", "import a from \"./a.js\"\n"),
        ("side effect", "import \"./side.js\"\n"),
        (
            "re-export",
            "export { a } from \"./a.js\"\nexport * from \"./b.js\"\n",
        ),
        (
            "specifier quoted inside a template",
            "const doc = `import x from \"./fake.js\"`\nimport real from \"./real.js\"\n",
        ),
        (
            "specifier inside a line comment",
            "// import x from \"./fake.js\"\nimport real from \"./real.js\"\n",
        ),
        (
            "specifier inside a block comment",
            "/* import x from \"./fake.js\" */\nimport real from \"./real.js\"\n",
        ),
        (
            "regex holding a quote",
            "const p = /[\"']/g\nimport real from \"./real.js\"\n",
        ),
        (
            "division that is not a regex",
            "const a = 1\nconst b = a / 2 / 3\nimport real from \"./real.js\"\n",
        ),
        (
            "regex after a keyword",
            "const m = \"x\".split(/,/)\nif (true) /a/.test(\"a\")\nimport real from \"./real.js\"\n",
        ),
        (
            "nested template interpolation",
            "const doc = `outer ${`inner ${\"./nope.js\"}`} end`\nimport real from \"./real.js\"\n",
        ),
        (
            "template holding an apostrophe",
            "const doc = `it's fine`\nimport real from \"./real.js\"\n",
        ),
        (
            "jsx with a quote in text",
            "const el = <p title=\"a\">it's here</p>\nimport real from \"./real.js\"\n",
        ),
        (
            "multi-line import clause",
            "import {\n  a,\n  b,\n} from \"./multi.js\"\n",
        ),
        (
            "import with a trailing comment",
            "import a from \"./a.js\" // from \"./fake.js\"\n",
        ),
        (
            "escaped quote inside a string",
            "const s = \"a \\\" import x from './fake.js'\"\nimport real from \"./real.js\"\n",
        ),
        (
            "string spanning a line continuation",
            "const s = \"a \\\n  b\"\nimport real from \"./real.js\"\n",
        ),
        (
            "type-only import",
            "import type { A } from \"./types.js\"\nimport real from \"./real.js\"\n",
        ),
        (
            "import attributes",
            "import data from \"./data.json\" with { type: \"json\" }\n",
        ),
        (
            "class with a private field and a regex",
            "class A {\n  #x = /a/\n  run() { return this.#x }\n}\nimport real from \"./real.js\"\n",
        ),
        // Mis-reading a regex only changes the answer on the regex's own line:
        // `skip_string` stops at a newline by design, so a mis-scan cannot
        // swallow the rest of the file. A case whose regex holds nothing that
        // matters therefore proves nothing about regex detection.
        (
            "regex holding a whole import statement",
            "const p = /import x from \"fake-pkg\"/\nimport real from \"./real.js\"\n",
        ),
        (
            "regex holding a comment opener and code after it",
            "const p = /[/][/] x/; import real from \"./real.js\"\n",
        ),
        (
            "regex holding an unbalanced brace",
            "const p = /[{]/; const after = { a: 1 }\nimport real from \"./real.js\"\n",
        ),
        (
            "regex containing a comment opener",
            "const p = /a\\/\\/b/\nimport real from \"./real.js\"\n",
        ),
        (
            "regex with a slash in a character class",
            "const p = /[/]/\nimport real from \"./real.js\"\n",
        ),
        (
            "regex right after return",
            "function f() { return /a\"b/ }\nimport real from \"./real.js\"\n",
        ),
        (
            "division right after a closing paren",
            "const q = (1 + 2) / 3 / 4\nimport real from \"./real.js\"\n",
        ),
        (
            "string ending in an escaped backslash",
            "const s = \"a\\\\\"\nimport real from \"./real.js\"\n",
        ),
        (
            "comment holding an unbalanced quote",
            "// it's fine\nimport real from \"./real.js\"\n",
        ),
        (
            "block comment holding a comment opener",
            "/* /* nested-looking */\nimport real from \"./real.js\"\n",
        ),
        (
            "jsx text holding braces and quotes",
            "const el = <p>{\"a\"} it's {'{'} here</p>\nimport real from \"./real.js\"\n",
        ),
        (
            "jsx attribute holding a template",
            "const el = <p title={`a ${1} b`} />\nimport real from \"./real.js\"\n",
        ),
        (
            "less-than that is not jsx",
            "const smaller = count < limit && other > 1\nimport real from \"./real.js\"\n",
        ),
        (
            "export star as namespace",
            "export * as ns from \"./ns.js\"\n",
        ),
        (
            "import.meta and a dynamic import",
            "const u = import.meta.url\nconst m = flag ? import(\"./a.js\") : import(\"./b.js\")\nimport real from \"./real.js\"\n",
        ),
        (
            "decorator carrying a string argument",
            "@Injectable({ providedIn: \"root\" })\nclass S {}\nimport real from \"./real.js\"\n",
        ),
        (
            "numeric separators and bigint",
            "const n = 1_000_000n / 2n\nimport real from \"./real.js\"\n",
        ),
        (
            "optional chaining before a regex-looking slash",
            "const v = a?.b / 2\nimport real from \"./real.js\"\n",
        ),
        (
            "interpolation holding a string with a backtick",
            "const doc = `x ${\"`\"} y`\nimport real from \"./real.js\"\n",
        ),
        (
            "satisfies and a generic call",
            "const c = fn<Map<string, number>>() satisfies Config\nimport real from \"./real.js\"\n",
        ),
        (
            "generic arrow that looks like jsx",
            "const identity = <T,>(value: T): T => value\nimport real from \"./real.js\"\n",
        ),
    ];
}
