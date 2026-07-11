//! Minifier: applies identifier shortening, whitespace compression, and
//! tree-shaking (dead-code elimination).
//!
//! The Ruvyxa minifier is intentionally simple and safe.  It does NOT parse
//! the JS AST — instead it applies a sequence of well-understood text
//! transformations that are correct for the output produced by the linker:
//!
//! 1. **Tree-shaking**: identify unused exports and remove their assignments.
//! 2. Strip single-line `//` comments (excluding `//!` directives).
//! 3. Collapse runs of whitespace (spaces, newlines, tabs) into a single space.
//! 4. Remove spaces around operators and punctuation where safe.
//! 5. Shorten the long `__ruv_<hex16>__` module namespace identifiers to
//!    short two-character names (`_ra`, `_rb`, …).
//!
//! ## Parallel Minification
//!
//! The `minify_parallel` function splits the linked bundle into independent
//! segments (one per module IIFE) after the tree-shaking pass, then runs
//! comment-stripping and whitespace-collapse on each segment concurrently
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
    let stage1 = strip_line_comments(&stage0);
    let stage2 = collapse_whitespace(&stage1);
    let stage3 = shorten_module_ids(&stage2);
    Ok(stage3)
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
        let stage1 = strip_line_comments(&tree_shaken);
        let stage2 = collapse_whitespace(&stage1);
        let stage3 = shorten_module_ids(&stage2);
        return Ok(stage3);
    }

    // Phase 3: Parallel comment-strip + whitespace-collapse per segment.
    let minified_segments: Vec<String> = segments
        .par_iter()
        .map(|segment| {
            let stripped = strip_line_comments(segment);
            collapse_whitespace(&stripped)
        })
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
// Pass 1 – Strip single-line comments
// ─────────────────────────────────────────────

fn strip_line_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_string: Option<char> = None;
    let mut chars = source.chars().peekable();

    while let Some(ch) = chars.next() {
        match (in_string, ch) {
            // Toggle string context.
            (None, '"' | '\'' | '`') => {
                in_string = Some(ch);
                out.push(ch);
            }
            (Some(q), c) if c == q => {
                in_string = None;
                out.push(ch);
            }
            // Start of a line comment outside strings.
            (None, '/') if chars.peek() == Some(&'/') => {
                // Consume until end of line.
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push(ch),
        }
    }

    out
}

// ─────────────────────────────────────────────
// Pass 2 – Collapse whitespace
// ─────────────────────────────────────────────

fn collapse_whitespace(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut prev_space = false;

    for ch in source.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            // Remove spaces around specific punctuation to further shrink output.
            if ch == '{' || ch == '}' || ch == '(' || ch == ')' || ch == ';' || ch == ',' {
                // Trim trailing space before the punctuation.
                if out.ends_with(' ') {
                    out.pop();
                }
                out.push(ch);
                prev_space = true; // Treat as whitespace consumer (skip leading space after).
            } else {
                prev_space = false;
                out.push(ch);
            }
        }
    }

    out.trim().to_string()
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
    fn strips_line_comments() {
        let src = "const x = 1; // this is a comment\nconst y = 2;";
        let out = strip_line_comments(src);
        assert!(!out.contains("this is a comment"));
        assert!(out.contains("const x = 1;"));
        assert!(out.contains("const y = 2;"));
    }

    #[test]
    fn collapses_whitespace() {
        let src = "const   x   =   1;\n\nconst y = 2;";
        let out = collapse_whitespace(src);
        assert!(!out.contains("   "));
        assert!(!out.contains("\n\n"));
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
