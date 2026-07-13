//! JavaScript minification and Ruvyxa's linker-aware export pruning.
//!
//! Ruvyxa keeps its framework-specific graph, linker, and explicit export
//! pruning pass. The final JavaScript transformation is delegated to Oxc:
//! parse → semantic compression/mangling → code generation. This makes
//! production output safe for syntax that cannot be handled reliably by a
//! text-only compressor (notably regular expressions, templates, ASI, and
//! nested modern JavaScript expressions).

#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use oxc::{
    allocator::Allocator,
    codegen::{Codegen, CodegenOptions},
    minifier::{CompressOptions, Minifier, MinifierOptions},
    parser::Parser,
    span::SourceType,
};

use crate::{BundleError, BundleTarget, Result};

/// Apply all minification passes to `source` and return the result.
pub fn minify(source: &str, _target: BundleTarget) -> Result<String> {
    minify_with_options(source, _target, true)
}

/// Apply minification with explicit tree-shaking control.
pub fn minify_with_options(
    source: &str,
    _target: BundleTarget,
    tree_shaking: bool,
) -> Result<String> {
    let stage0 = if tree_shaking {
        tree_shake(source)
    } else {
        source.to_string()
    };
    minify_javascript(&stage0, tree_shaking)
}

/// Compatibility entry point for callers that previously requested parallel
/// text minification. Oxc needs the complete linked program to build semantic
/// scope information safely, so it performs one whole-program AST pass.
pub fn minify_parallel(source: &str, _target: BundleTarget) -> Result<String> {
    minify_parallel_with_options(source, _target, true)
}

/// Parallel minification with explicit tree-shaking control.
pub fn minify_parallel_with_options(
    source: &str,
    _target: BundleTarget,
    tree_shaking: bool,
) -> Result<String> {
    minify_with_options(source, _target, tree_shaking)
}

fn minify_javascript(source: &str, tree_shaking: bool) -> Result<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::unambiguous()).parse();

    if !parsed.diagnostics.is_empty() {
        return Err(BundleError::Compiler(format!(
            "Oxc could not parse linked JavaScript: {} syntax diagnostic(s)",
            parsed.diagnostics.len()
        )));
    }

    let mut program = parsed.program;
    let options = if tree_shaking {
        MinifierOptions::default()
    } else {
        // `treeShaking: false` must still preserve otherwise-unused bindings.
        // Oxc's safest compression profile keeps those bindings while allowing
        // semantics-preserving whitespace reduction and identifier mangling.
        MinifierOptions {
            mangle: MinifierOptions::default().mangle,
            compress: Some(CompressOptions::safest()),
        }
    };
    let result = Minifier::new(options).minify(&allocator, &mut program);

    Ok(Codegen::new()
        .with_options(CodegenOptions::minify())
        .with_scoping(result.scoping)
        .with_private_member_mappings(result.class_private_mappings)
        .build(&program)
        .code)
}

/// Apply only the tree-shaking pass.
pub fn tree_shake_exports(source: &str) -> String {
    tree_shake(source)
}

/// Fold CommonJS `NODE_ENV` branches while resolving a production client
/// graph. This prevents packages such as React from pulling both development
/// and production implementations into the same browser bundle.
pub(crate) fn fold_production_node_env(source: &str) -> String {
    let mut folded = source.to_string();

    // A bounded loop handles nested guards without allowing malformed input to
    // turn source preprocessing into an unbounded build step.
    for _ in 0..64 {
        let Some((start, end, replacement)) = find_node_env_conditional(&folded) else {
            break;
        };
        folded.replace_range(start..end, &replacement);
    }

    folded
}

fn find_node_env_conditional(source: &str) -> Option<(usize, usize, String)> {
    let bytes = source.as_bytes();
    let mut search = 0;

    while search + 1 < bytes.len() {
        match bytes[search] {
            b'"' | b'\'' | b'`' => {
                search = skip_quoted_bytes(bytes, search);
                continue;
            }
            b'/' if bytes.get(search + 1) == Some(&b'/') => {
                search += 2;
                while search < bytes.len() && !matches!(bytes[search], b'\n' | b'\r') {
                    search += 1;
                }
                continue;
            }
            b'/' if bytes.get(search + 1) == Some(&b'*') => {
                search = skip_block_comment_bytes(bytes, search);
                continue;
            }
            b'/' => {
                if let Some(end) = skip_slash_delimited_bytes(bytes, search) {
                    search = end;
                    continue;
                }
            }
            b'i' if bytes.get(search + 1) == Some(&b'f') => {}
            _ => {
                search += 1;
                continue;
            }
        }

        let start = search;
        if start > 0 && is_ascii_identifier_byte(bytes[start - 1])
            || bytes
                .get(start + 2)
                .is_some_and(|byte| is_ascii_identifier_byte(*byte))
        {
            search = start + 2;
            continue;
        }

        let condition_open = skip_ascii_whitespace(bytes, start + 2);
        if bytes.get(condition_open) != Some(&b'(') {
            search = start + 2;
            continue;
        }
        let condition_close = matching_delimiter(source, condition_open, b'(', b')')?;
        let condition = &source[condition_open + 1..condition_close];
        let Some(condition_result) = production_condition_result(condition) else {
            search = condition_close + 1;
            continue;
        };

        let consequent_open = skip_ascii_whitespace(bytes, condition_close + 1);
        if bytes.get(consequent_open) != Some(&b'{') {
            search = condition_close + 1;
            continue;
        }
        let consequent_close = matching_delimiter(source, consequent_open, b'{', b'}')?;

        let after_consequent = skip_ascii_whitespace(bytes, consequent_close + 1);
        let else_start = source[after_consequent..]
            .starts_with("else")
            .then_some(after_consequent);
        let alternative = else_start.and_then(|else_index| {
            let open = skip_ascii_whitespace(bytes, else_index + 4);
            (bytes.get(open) == Some(&b'{'))
                .then(|| matching_delimiter(source, open, b'{', b'}').map(|close| (open, close)))?
        });

        let end = alternative
            .map(|(_, close)| close + 1)
            .unwrap_or(consequent_close + 1);
        let replacement = if condition_result {
            source[consequent_open + 1..consequent_close].to_string()
        } else if let Some((open, close)) = alternative {
            source[open + 1..close].to_string()
        } else {
            String::new()
        };

        return Some((start, end, replacement));
    }

    None
}

fn production_condition_result(condition: &str) -> Option<bool> {
    let normalized = condition
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .map(|ch| if ch == '\'' { '"' } else { ch })
        .collect::<String>();
    match normalized.as_str() {
        "process.env.NODE_ENV===\"production\""
        | "process.env.NODE_ENV==\"production\""
        | "\"production\"===process.env.NODE_ENV"
        | "\"production\"==process.env.NODE_ENV" => Some(true),
        "process.env.NODE_ENV!==\"production\""
        | "process.env.NODE_ENV!=\"production\""
        | "\"production\"!==process.env.NODE_ENV"
        | "\"production\"!=process.env.NODE_ENV" => Some(false),
        _ => None,
    }
}

fn matching_delimiter(source: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = start;

    while index < bytes.len() {
        match bytes[index] {
            b'"' | b'\'' | b'`' => index = skip_quoted_bytes(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment_bytes(bytes, index);
            }
            b'/' => {
                if let Some(end) = skip_slash_delimited_bytes(bytes, index) {
                    index = end;
                } else {
                    index += 1;
                }
            }
            byte if byte == open => {
                depth += 1;
                index += 1;
            }
            byte if byte == close => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
                index += 1;
            }
            _ => index += 1,
        }
    }

    None
}

fn skip_quoted_bytes(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index = (index + 2).min(bytes.len());
        } else if bytes[index] == quote {
            return index + 1;
        } else {
            index += 1;
        }
    }
    bytes.len()
}

fn skip_block_comment_bytes(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 2;
    while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
        index += 1;
    }
    (index + 2).min(bytes.len())
}

fn skip_slash_delimited_bytes(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    let mut escaped = false;
    let mut in_character_class = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if matches!(byte, b'\n' | b'\r') {
            return None;
        }
        if escaped {
            escaped = false;
        } else {
            match byte {
                b'\\' => escaped = true,
                b'[' => in_character_class = true,
                b']' => in_character_class = false,
                b'/' if !in_character_class => {
                    index += 1;
                    while bytes
                        .get(index)
                        .is_some_and(|byte| is_ascii_identifier_byte(*byte))
                    {
                        index += 1;
                    }
                    return Some(index);
                }
                _ => {}
            }
        }
        index += 1;
    }

    None
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

fn is_ascii_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

// ─────────────────────────────────────────────
// Pass 0 – Tree-shaking (dead-code elimination)
// ─────────────────────────────────────────────

/// Remove unused exports from the linked bundle.
///
/// Strategy:
/// 1. Scan for all `__ruv_<hex16>__.<member>` property accesses across the
///    entire bundle to build a "used set" per module.
/// 2. Remove lines matching `__exports.<name> = <name>;` where `<name>` is
///    not in the used set for that module.
/// 3. Remove variable declarations whose sole purpose was the removed export,
///    if they are not referenced elsewhere in the same module scope.
///
/// This is conservative — if we can't prove an export is unused, we keep it.
fn tree_shake(source: &str) -> String {
    // Step 1: Collect all consumed members: `__ruv_xxx__.member`
    let used_members = collect_used_members(source);

    if used_members.is_empty() {
        return source.to_string();
    }

    // Step 2: Remove unused `__exports.name = name;` assignments.
    let mut out = String::with_capacity(source.len());
    let mut current_module_id: Option<String> = None;

    for line in source.lines() {
        let trimmed = line.trim();

        // Track which module scope we're inside.
        // Module IIFEs start with: `var __ruv_xxx__ = (function() {`
        if trimmed.starts_with("var __ruv_")
            && trimmed.contains("= (function()")
            && let Some(id) = extract_module_id_from_line(trimmed)
        {
            current_module_id = Some(id);
        }

        // End of module IIFE: `})();`
        if trimmed == "})();" || trimmed == "  return __exports;" {
            // Keep these lines regardless.
            out.push_str(line);
            out.push('\n');
            if trimmed == "})();" {
                current_module_id = None;
            }
            continue;
        }

        // Check if this is an export assignment we can remove.
        if let Some(ref mod_id) = current_module_id
            && let Some(export_name) = extract_export_assignment(trimmed)
        {
            let member_key = format!("{mod_id}.{export_name}");
            if !used_members.contains(&member_key) && export_name != "default" {
                // This export is unused — remove the assignment line.
                out.push_str("  // [tree-shaken] ");
                out.push_str(trimmed);
                out.push('\n');
                continue;
            }
        }

        out.push_str(line);
        out.push('\n');
    }

    // Remove trailing newline if source didn't end with one.
    if !source.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    out
}

/// Scan the source for all `__ruv_<hex16>__.<member>` accesses.
///
/// Returns a set of `"__ruv_xxx__.member"` strings.
fn collect_used_members(source: &str) -> BTreeSet<String> {
    let mut members = BTreeSet::new();
    let prefix = "__ruv_";
    let mut search = source;

    while let Some(start) = search.find(prefix) {
        let tail = &search[start..];

        // Find the closing `__` of the module ID.
        let after_prefix = &tail[prefix.len()..];
        let Some(close_offset) = after_prefix.find("__") else {
            search = &search[start + prefix.len()..];
            continue;
        };

        let id_end = prefix.len() + close_offset + 2;
        let module_id = &tail[..id_end];

        // Check if followed by `.member`
        let rest = &tail[id_end..];
        if let Some(after_dot) = rest.strip_prefix('.') {
            // Extract the member name (valid JS identifier chars).
            let member: String = after_dot
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();

            if !member.is_empty() {
                members.insert(format!("{module_id}.{member}"));
            }
        }

        search = &search[start + id_end..];
    }

    members
}

/// Extract the module ID from a line like `var __ruv_abc123__ = (function() {`
fn extract_module_id_from_line(line: &str) -> Option<String> {
    let rest = line.strip_prefix("var ")?;
    let space_idx = rest.find(' ')?;
    let id = &rest[..space_idx];
    if id.starts_with("__ruv_") && id.ends_with("__") {
        Some(id.to_string())
    } else {
        None
    }
}

/// Extract the export name from `__exports.name = …;` lines.
fn extract_export_assignment(line: &str) -> Option<String> {
    let rest = line.strip_prefix("__exports.")?;
    let eq_idx = rest.find(" = ")?;
    let name = &rest[..eq_idx];
    // Validate that it's a simple identifier.
    if name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        Some(name.to_string())
    } else {
        None
    }
}

// ─────────────────────────────────────────────
// Pass 1 – Token-aware compression
// ─────────────────────────────────────────────

#[cfg(test)]
fn compress_javascript(source: &str) -> String {
    let chars = source.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    let mut pending_whitespace = false;
    let mut pending_newline = false;
    let mut previous_word = String::new();

    while index < chars.len() {
        let ch = chars[index];

        if ch.is_whitespace() {
            pending_whitespace = true;
            pending_newline |= matches!(ch, '\n' | '\r');
            index += 1;
            continue;
        }

        if ch == '/' && chars.get(index + 1) == Some(&'/') {
            let comment_start = index;
            index += 2;
            while index < chars.len() && !matches!(chars[index], '\n' | '\r') {
                index += 1;
            }
            if chars.get(comment_start + 2) == Some(&'!') {
                emit_pending_separator(
                    &mut out,
                    &chars,
                    comment_start,
                    pending_whitespace,
                    pending_newline,
                    &previous_word,
                );
                out.extend(chars[comment_start..index].iter());
                out.push('\n');
            }
            pending_whitespace = true;
            pending_newline = true;
            continue;
        }

        if ch == '/' && chars.get(index + 1) == Some(&'*') {
            let comment_start = index;
            let legal_comment = chars.get(index + 2) == Some(&'!');
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                pending_newline |= matches!(chars[index], '\n' | '\r');
                index += 1;
            }
            index = (index + 2).min(chars.len());
            if legal_comment {
                emit_pending_separator(
                    &mut out,
                    &chars,
                    comment_start,
                    pending_whitespace,
                    pending_newline,
                    &previous_word,
                );
                out.extend(chars[comment_start..index].iter());
            }
            pending_whitespace = true;
            continue;
        }

        emit_pending_separator(
            &mut out,
            &chars,
            index,
            pending_whitespace,
            pending_newline,
            &previous_word,
        );
        pending_whitespace = false;
        pending_newline = false;

        if matches!(ch, '"' | '\'' | '`') {
            let end = quoted_literal_end(&chars, index, ch);
            out.extend(chars[index..end].iter());
            previous_word.clear();
            index = end;
            continue;
        }

        if ch == '/'
            && let Some(end) = slash_delimited_end(&chars, index)
        {
            out.extend(chars[index..end].iter());
            previous_word.clear();
            index = end;
            continue;
        }

        if is_identifier_part(ch) {
            let start = index;
            index += 1;
            while index < chars.len() && is_identifier_part(chars[index]) {
                index += 1;
            }
            previous_word = chars[start..index].iter().collect();
            out.push_str(&previous_word);
            continue;
        }

        previous_word.clear();
        out.push(ch);
        index += 1;
    }

    out.trim().to_string()
}

#[cfg(test)]
fn emit_pending_separator(
    out: &mut String,
    chars: &[char],
    next_index: usize,
    pending_whitespace: bool,
    pending_newline: bool,
    previous_word: &str,
) {
    if !pending_whitespace || out.is_empty() || next_index >= chars.len() {
        return;
    }

    let previous = out.chars().next_back().unwrap_or_default();
    let next = chars[next_index];
    let restricted_newline = pending_newline
        && matches!(
            previous_word,
            "async" | "await" | "break" | "continue" | "return" | "throw" | "yield"
        );
    let postfix_newline =
        pending_newline && matches!(next, '+' | '-') && chars.get(next_index + 1) == Some(&next);

    if restricted_newline || postfix_newline {
        out.push('\n');
    } else if tokens_need_separator(previous, next) {
        out.push(' ');
    }
}

#[cfg(test)]
fn tokens_need_separator(previous: char, next: char) -> bool {
    (is_identifier_part(previous) && is_identifier_part(next))
        || (previous == '+' && next == '+')
        || (previous == '-' && next == '-')
        || (previous == '/' && matches!(next, '/' | '*'))
}

#[cfg(test)]
fn is_identifier_part(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '$') || (!ch.is_ascii() && ch.is_alphabetic())
}

#[cfg(test)]
fn quoted_literal_end(chars: &[char], start: usize, quote: char) -> usize {
    let mut index = start + 1;
    let mut escaped = false;
    while index < chars.len() {
        let ch = chars[index];
        index += 1;
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            break;
        }
    }
    index
}

/// Preserve a slash-delimited span verbatim. For a regular expression this
/// protects its body and flags. For a division chain such as `a / b / c`, the
/// conservative span is still valid JavaScript and merely retains a few bytes.
#[cfg(test)]
fn slash_delimited_end(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start + 1;
    let mut escaped = false;
    let mut in_character_class = false;

    while index < chars.len() {
        let ch = chars[index];
        if matches!(ch, '\n' | '\r') {
            return None;
        }
        if escaped {
            escaped = false;
        } else {
            match ch {
                '\\' => escaped = true,
                '[' => in_character_class = true,
                ']' => in_character_class = false,
                '/' if !in_character_class => {
                    index += 1;
                    while index < chars.len() && is_identifier_part(chars[index]) {
                        index += 1;
                    }
                    return Some(index);
                }
                _ => {}
            }
        }
        index += 1;
    }

    None
}

// ─────────────────────────────────────────────
// Pass 3 – Shorten module identifiers
// ─────────────────────────────────────────────

/// Replace `__ruv_<hex16>__` identifiers with short names (`_ra`, `_rb`, …).
#[cfg(test)]
fn shorten_module_ids(source: &str) -> String {
    // Collect all unique `__ruv_…__` identifiers.
    let mut ids: Vec<String> = Vec::new();
    let prefix = "__ruv_";
    let mut search = source;

    while let Some(start_offset) = search.find(prefix) {
        let tail = &search[start_offset..];
        // Find the closing `__` starting AFTER the prefix (position 6).
        if let Some(close_offset) = tail[prefix.len()..].find("__") {
            let end = prefix.len() + close_offset + 2; // include closing `__`
            let id = &tail[..end];
            if !ids.contains(&id.to_string()) {
                ids.push(id.to_string());
            }
            search = &tail[end..];
        } else {
            break;
        }
    }

    if ids.is_empty() {
        return source.to_string();
    }

    // Build a replacement map.
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (i, id) in ids.iter().enumerate() {
        let short = encode_base26(i);
        map.insert(id.clone(), format!("_r{short}"));
    }

    let mut out = source.to_string();
    for (long, short) in &map {
        out = out.replace(long.as_str(), short.as_str());
    }

    out
}

/// Encode a number into a short alphabetic string (`a`, `b`, …, `z`, `aa`, …).
#[cfg(test)]
fn encode_base26(mut n: usize) -> String {
    let mut s = String::new();
    loop {
        s.insert(0, (b'a' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oxc_minifies_modern_literals_without_corrupting_them() {
        let src = r#"// remove this comment
const url = "https://example.test/a//b";
const template = `keep // text and ${url.length}`;
const pattern = /\\n( *(at)?)[a-z/]+/gi;
export { url, template, pattern };"#;
        let out = minify(src, BundleTarget::Ssr).unwrap();

        assert!(out.len() < src.len());
        assert!(out.contains("https://example.test/a//b"));
        assert!(out.contains("keep // text and"), "unexpected output: {out}");
        assert!(out.contains(r#"/\\n( *(at)?)[a-z/]+/gi"#));
        assert!(!out.contains("remove this comment"));
    }

    #[test]
    fn oxc_minifies_esm_without_erasing_module_syntax() {
        let src = r#"import { createElement } from "react";
export const view = createElement("main", null, "Ruvyxa");"#;
        let out = minify(src, BundleTarget::Ssr).unwrap();

        assert!(out.contains("import"));
        assert!(out.contains("from\"react\""));
        assert!(out.contains("export"));
    }

    #[test]
    fn oxc_parse_failures_abort_the_bundle() {
        let error = minify("const = ;", BundleTarget::Client).unwrap_err();
        assert!(
            matches!(error, BundleError::Compiler(message) if message.contains("Oxc could not parse"))
        );
    }

    #[test]
    fn compresses_comments_and_whitespace() {
        let src = "const   x = 1; // this is a comment\nconst y = 2;";
        let out = compress_javascript(src);
        assert!(!out.contains("this is a comment"));
        assert_eq!(out, "const x=1;const y=2;");
    }

    #[test]
    fn preserves_literals_and_removes_only_real_comments() {
        let src = r#"const url = "https://example.test/a//b";
const template = `keep // text and  spaces`;
const pattern = /\n( *(at)?)[a-z/]+/gi; /* remove me */"#;
        let out = compress_javascript(src);
        assert!(out.contains(r#""https://example.test/a//b""#));
        assert!(out.contains("`keep // text and  spaces`"));
        assert!(out.contains(r#"/\n( *(at)?)[a-z/]+/gi"#));
        assert!(!out.contains("remove me"));
    }

    #[test]
    fn preserves_automatic_semicolon_insertion_boundaries() {
        let src = "function value() { return\n{ ok: true }; }\nlet count = 1\n++count;";
        let out = compress_javascript(src);
        assert!(out.contains("return\n{"));
        assert!(out.contains("1\n++count"));
    }

    #[test]
    fn preserves_legal_comments() {
        let src = "/*! library license */ const value = 1; //! directive\nvalue;";
        let out = compress_javascript(src);
        assert!(out.contains("/*! library license */"));
        assert!(out.contains("//! directive"));
    }

    #[test]
    fn folds_commonjs_production_dependency_branch() {
        let src = r#"
'use strict';
if (process.env.NODE_ENV === 'production') {
  module.exports = require('./production.js');
} else {
  module.exports = require('./development.js');
}
"#;
        let out = fold_production_node_env(src);
        assert!(out.contains("require('./production.js')"));
        assert!(!out.contains("development.js"));
        assert!(!out.contains("process.env.NODE_ENV"));
    }

    #[test]
    fn folds_nested_development_only_guard() {
        let src = r#"
function checkDCE() {
  if (process.env.NODE_ENV !== "production") {
    throw new Error("development only");
  }
  return true;
}
"#;
        let out = fold_production_node_env(src);
        assert!(!out.contains("development only"));
        assert!(out.contains("return true"));
    }

    #[test]
    fn node_env_folder_ignores_literals_comments_and_regexes() {
        let src = r#"
const message = "if (process.env.NODE_ENV === 'production') { altered }";
// if (process.env.NODE_ENV === 'production') { altered }
const pattern = /if \(process\.env\.NODE_ENV === 'production'\) \{ altered \}/;
module.exports = message;
"#;
        assert_eq!(fold_production_node_env(src), src);
    }

    #[test]
    fn shortens_module_ids() {
        let src = "var __ruv_abcdef1234567890__ = 1; var __ruv_1111111111111111__ = 2;";
        let out = shorten_module_ids(src);
        assert!(!out.contains("__ruv_abcdef1234567890__"));
        assert!(out.contains("_ra"));
    }

    #[test]
    fn encode_base26_examples() {
        assert_eq!(encode_base26(0), "a");
        assert_eq!(encode_base26(25), "z");
        assert_eq!(encode_base26(26), "aa");
    }

    // ── Tree-shaking tests ──

    #[test]
    fn tree_shake_removes_unused_exports() {
        let src = r#"var __ruv_aaaa1111aaaa1111__ = (function() {
  "use strict";
  var __exports = {};
  const used = 1;
  const unused = 2;
  __exports.used = used;
  __exports.unused = unused;
  return __exports;
})();
var __ruv_bbbb2222bbbb2222__ = (function() {
  "use strict";
  var __exports = {};
  const val = __ruv_aaaa1111aaaa1111__.used;
  __exports.default = val;
  return __exports;
})();
"#;
        let result = tree_shake(src);
        // `used` export should be kept (referenced by module b).
        assert!(result.contains("__exports.used = used;"));
        // `unused` export should be tree-shaken (marked as comment).
        assert!(result.contains("[tree-shaken]"));
        assert!(result.contains("[tree-shaken] __exports.unused = unused;"));
        // The active assignment should NOT exist (only the commented version).
        let active_lines: Vec<&str> = result
            .lines()
            .filter(|l| l.contains("__exports.unused") && !l.contains("[tree-shaken]"))
            .collect();
        assert!(
            active_lines.is_empty(),
            "unused export should not appear as active: {:?}",
            active_lines
        );
    }

    #[test]
    fn tree_shake_keeps_default_always() {
        let src = r#"var __ruv_cccc3333cccc3333__ = (function() {
  "use strict";
  var __exports = {};
  const Page = () => {};
  __exports.default = Page;
  return __exports;
})();
"#;
        let result = tree_shake(src);
        // `default` is never shaken — it's always considered used.
        assert!(result.contains("__exports.default = Page;"));
    }

    #[test]
    fn minify_can_disable_tree_shaking() {
        let src = r#"var __ruv_aaaa1111aaaa1111__ = (function() {
  "use strict";
  var __exports = {};
  const unused = 2;
  __exports.unused = unused;
  return __exports;
})();
"#;
        let result = minify_with_options(src, BundleTarget::Client, false).unwrap();

        assert!(result.contains("unused"));
        assert!(!result.contains("[tree-shaken]"));
    }

    #[test]
    fn tree_shake_no_modules_passthrough() {
        let src = "const x = 1;\nconst y = 2;\n";
        let result = tree_shake(src);
        assert_eq!(result, src);
    }

    #[test]
    fn collect_used_members_finds_references() {
        let src = "const a = __ruv_abc123abc12300__.foo; const b = __ruv_abc123abc12300__.bar;";
        let members = collect_used_members(src);
        assert!(members.contains("__ruv_abc123abc12300__.foo"));
        assert!(members.contains("__ruv_abc123abc12300__.bar"));
        assert_eq!(members.len(), 2);
    }

    #[test]
    fn extract_export_assignment_works() {
        assert_eq!(
            extract_export_assignment("__exports.helper = helper;"),
            Some("helper".into())
        );
        assert_eq!(
            extract_export_assignment("__exports.default = Page;"),
            Some("default".into())
        );
        assert_eq!(extract_export_assignment("const x = 1;"), None);
    }
}
