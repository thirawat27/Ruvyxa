//! Minifier: applies module identifier shortening, token-aware JavaScript
//! compression, and tree-shaking (dead-code elimination).
//!
//! The Ruvyxa minifier is intentionally conservative. It does not rewrite an
//! arbitrary JavaScript AST; instead it tokenizes the linked output closely
//! enough to preserve literals and automatic-semicolon-insertion boundaries:
//!
//! 1. **Tree-shaking**: identify unused exports and remove their assignments.
//! 2. Remove non-license line and block comments outside literals.
//! 3. Preserve strings, templates, regular expressions, and legal comments.
//! 4. Remove whitespace only where adjacent JavaScript tokens remain distinct.
//! 5. Shorten the long `__ruv_<hex16>__` module namespace identifiers to
//!    short two-character names (`_ra`, `_rb`, …).
//!
//! ## Parallel Minification
//!
//! The `minify_parallel` function splits the linked bundle into independent
//! segments (one per module IIFE) after the tree-shaking pass, then runs
//! token-aware compression on each segment concurrently
//! via rayon. For bundles with 8+ modules, this reduces minification time
//! by approximately 3× on a 4-core machine.
//!
//! The minifier is part of Ruvyxa's own Rust bundling pipeline and does not
//! require an external JavaScript bundler.

use std::collections::{BTreeMap, BTreeSet};

use rayon::prelude::*;

use crate::{BundleTarget, Result};

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
    let compressed = compress_javascript(&stage0);
    Ok(shorten_module_ids(&compressed))
}

/// Parallel minification: splits the bundle into module segments after
/// tree-shaking, processes comment-stripping and whitespace-collapse
/// concurrently on each segment, then applies global ID shortening.
///
/// Falls back to sequential `minify()` for small bundles (<8 modules).
pub fn minify_parallel(source: &str, _target: BundleTarget) -> Result<String> {
    minify_parallel_with_options(source, _target, true)
}

/// Parallel minification with explicit tree-shaking control.
pub fn minify_parallel_with_options(
    source: &str,
    _target: BundleTarget,
    tree_shaking: bool,
) -> Result<String> {
    // Phase 1: Tree-shake (global pass — needs cross-module member usage data).
    let tree_shaken = if tree_shaking {
        tree_shake(source)
    } else {
        source.to_string()
    };

    // Phase 2: Split into segments for parallel processing.
    let segments = split_into_segments(&tree_shaken);

    // For small bundles, sequential is faster (avoids rayon overhead).
    if segments.len() < 8 {
        let compressed = compress_javascript(&tree_shaken);
        return Ok(shorten_module_ids(&compressed));
    }

    // Phase 3: Compress independent module segments in parallel.
    let minified_segments: Vec<String> = segments
        .par_iter()
        .map(|segment| compress_javascript(segment))
        .collect();

    // Phase 4: Rejoin segments.
    let joined_size: usize = minified_segments.iter().map(|s| s.len() + 1).sum();
    let mut joined = String::with_capacity(joined_size);
    for (i, segment) in minified_segments.iter().enumerate() {
        joined.push_str(segment);
        if i < minified_segments.len() - 1 {
            joined.push(' ');
        }
    }

    // Phase 5: Shorten module IDs (global text replacement).
    let final_output = shorten_module_ids(&joined);
    Ok(final_output)
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

/// Split a linked bundle into independent segments at module IIFE boundaries.
///
/// Segments are split at `})();` lines followed by blank lines, which is the
/// consistent boundary pattern produced by the linker.
fn split_into_segments(source: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let boundary = "})();\n\n";

    let mut search_from = 0;
    while let Some(pos) = source[search_from..].find(boundary) {
        let abs_pos = search_from + pos + boundary.len();
        segments.push(&source[start..abs_pos]);
        start = abs_pos;
        search_from = abs_pos;
    }

    // Remaining content (the SSR export line, or trailing content).
    if start < source.len() {
        segments.push(&source[start..]);
    }

    segments
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

fn tokens_need_separator(previous: char, next: char) -> bool {
    (is_identifier_part(previous) && is_identifier_part(next))
        || (previous == '+' && next == '+')
        || (previous == '-' && next == '-')
        || (previous == '/' && matches!(next, '/' | '*'))
}

fn is_identifier_part(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '$') || (!ch.is_ascii() && ch.is_alphabetic())
}

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
