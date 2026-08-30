//! JavaScript minification and Ruvyxa's linker-aware export pruning.
//!
//! Ruvyxa keeps its framework-specific graph, linker, and explicit export
//! pruning pass. The final JavaScript transformation is delegated to Oxc:
//! parse → semantic compression/mangling → code generation. This makes
//! production output safe for syntax that cannot be handled reliably by a
//! text-only compressor (notably regular expressions, templates, ASI, and
//! nested modern JavaScript expressions).

use std::collections::BTreeSet;

use oxc::{
    allocator::Allocator,
    codegen::{Codegen, CodegenOptions, CommentOptions, LegalComment},
    minifier::{CompressOptions, Minifier, MinifierOptions},
    parser::Parser,
    span::SourceType,
    transformer::EngineTargets,
};

use crate::{BundleError, BundleTarget, EsTarget, Result};

/// Apply all minification passes to `source` and return the result.
pub fn minify(source: &str, _target: BundleTarget, es_target: EsTarget) -> Result<String> {
    minify_with_options(source, _target, true, es_target)
}

/// Apply minification with explicit tree-shaking control.
pub fn minify_with_options(
    source: &str,
    _target: BundleTarget,
    tree_shaking: bool,
    es_target: EsTarget,
) -> Result<String> {
    let stage0 = if tree_shaking {
        tree_shake(source)
    } else {
        source.to_string()
    };
    minify_javascript(&stage0, tree_shaking, es_target)
}

/// Hold the compressor to `es_target` so it cannot reintroduce newer syntax.
///
/// oxc's compressor rewrites toward the shortest equivalent form, and the
/// shortest form is often newer syntax: `a.b ?? (a.b = 0)` compresses to
/// `a.b ??= 0`. Left at its default (every feature available) it undid the
/// transform's work — the client bundle for a project on `build.target: es2020`
/// went out with logical assignment in it, because the two passes had no shared
/// idea of the language level.
fn apply_es_target(options: &mut MinifierOptions, es_target: EsTarget) -> Result<()> {
    if es_target.is_default() {
        return Ok(());
    }
    // With compression off there is no rewriting to constrain: mangling and
    // whitespace removal cannot introduce syntax the source did not have.
    let Some(compress) = options.compress.as_mut() else {
        return Ok(());
    };
    compress.target = EngineTargets::from_target(es_target.as_str()).map_err(|error| {
        BundleError::Compiler(format!("Oxc rejected build.target `{es_target}`: {error}"))
    })?;
    Ok(())
}

/// One generated position and the input position it came from.
///
/// Minification rewrites the text wholesale, so the only thing that can carry a
/// source map across it is the minifier itself. Oxc's codegen emits these; the
/// bundler joins them to the linker's line provenance to reach the module a
/// position started in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MinifiedPosition {
    pub(crate) generated_line: u32,
    pub(crate) generated_column: u32,
    pub(crate) source_line: u32,
}

/// Minify and report where each emitted position came from in `source`.
///
/// Used only when a source map was asked for: producing the mapping costs
/// codegen work that a build without `build.map` should not pay.
pub(crate) fn minify_tracking_positions(
    source: &str,
    target: BundleTarget,
    es_target: EsTarget,
) -> Result<(String, Vec<MinifiedPosition>)> {
    let _ = target;
    let allocator = Allocator::default();
    let program = parse_for_minification(&allocator, source, es_target)?;
    let (mut program, options) = program;
    let result = Minifier::new(options).minify(&allocator, &mut program);

    let mut codegen_options = codegen_options();
    // Any path will do: it only names the `sources` entry of oxc's own map,
    // which is discarded once the positions have been read out of it.
    codegen_options.source_map_path = Some(std::path::PathBuf::from("ruvyxa-linked.js"));
    let generated = Codegen::new()
        .with_options(codegen_options)
        .with_scoping(result.scoping)
        .with_private_member_mappings(result.class_private_mappings)
        .build(&program);

    let positions = generated.map.as_ref().map_or_else(Vec::new, |map| {
        map.get_tokens()
            .map(|token| MinifiedPosition {
                generated_line: token.get_dst_line(),
                generated_column: token.get_dst_col(),
                source_line: token.get_src_line(),
            })
            .collect()
    });
    Ok((generated.code, positions))
}

/// Parse and pick the compression profile the two minify entry points share.
fn parse_for_minification<'a>(
    allocator: &'a Allocator,
    source: &'a str,
    es_target: EsTarget,
) -> Result<(oxc::ast::ast::Program<'a>, MinifierOptions)> {
    let parsed = Parser::new(allocator, source, SourceType::unambiguous()).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(BundleError::Compiler(format!(
            "Oxc could not parse linked JavaScript: {} syntax diagnostic(s)",
            parsed.diagnostics.len()
        )));
    }
    // `treeShaking: false` must still preserve otherwise-unused bindings, and
    // the bundler shakes before this runs, so the map-producing path always
    // takes the safest profile for the same reason `minify_javascript` does.
    let mut options = MinifierOptions {
        compress: Some(CompressOptions::safest()),
        ..MinifierOptions::default()
    };
    apply_es_target(&mut options, es_target)?;
    Ok((parsed.program, options))
}

fn minify_javascript(source: &str, tree_shaking: bool, es_target: EsTarget) -> Result<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::unambiguous()).parse();

    if !parsed.diagnostics.is_empty() {
        return Err(BundleError::Compiler(format!(
            "Oxc could not parse linked JavaScript: {} syntax diagnostic(s)",
            parsed.diagnostics.len()
        )));
    }

    let mut program = parsed.program;
    let mut options = if tree_shaking {
        MinifierOptions::default()
    } else {
        // `treeShaking: false` must still preserve otherwise-unused bindings.
        // Oxc's safest compression profile keeps those bindings while allowing
        // semantics-preserving whitespace reduction and identifier mangling.
        //
        // Spread the defaults rather than listing every field: oxc is pinned to
        // an exact version and adds options between releases (`mangle_properties`
        // arrived in 0.146), so an exhaustive literal turns each bump into a
        // compile error that says nothing about what the new option should be.
        // Only the field this branch actually means to change is named.
        MinifierOptions {
            compress: Some(CompressOptions::safest()),
            ..MinifierOptions::default()
        }
    };
    apply_es_target(&mut options, es_target)?;
    let result = Minifier::new(options).minify(&allocator, &mut program);

    Ok(Codegen::new()
        .with_options(codegen_options())
        .with_scoping(result.scoping)
        .with_private_member_mappings(result.class_private_mappings)
        .build(&program)
        .code)
}

/// Minified codegen that still emits the comments a bundle is not allowed to
/// drop.
///
/// `CodegenOptions::minify()` disables every comment class, legal ones
/// included. That silently changed what this crate ships: the text compressor
/// oxc replaced had an explicit path for `/*!` and `//!`, and dependencies
/// carry their licence notice in exactly that form — MIT and BSD both require
/// the notice to travel with the distributed copy. A minifier is expected to
/// preserve them, and every comparable tool does.
///
/// `Eof` rather than `Inline` so the notices are collected at the end of the
/// bundle instead of interrupting code, which is the placement bundlers
/// conventionally use.
///
/// Normal, JSDoc, and annotation comments stay off — `jsdoc: false` does not
/// affect a JSDoc-shaped banner, because oxc classifies a comment containing
/// `@license` or `@preserve` as legal regardless of how it opens. That matters:
/// React's notice is a `/** @license React … */` block, not a `/*!` one.
/// `#__PURE__` and friends were already consumed by the minifier pass just
/// above, so keeping them would only add bytes to a browser bundle nothing
/// reads them from again.
fn codegen_options() -> CodegenOptions {
    CodegenOptions {
        comments: CommentOptions {
            normal: false,
            jsdoc: false,
            annotation: false,
            legal: LegalComment::Eof,
        },
        ..CodegenOptions::minify()
    }
}

/// Apply only the tree-shaking pass.
pub fn tree_shake_exports(source: &str) -> String {
    tree_shake(source)
}

/// Fold CommonJS `NODE_ENV` branches while resolving a production client
/// graph. This prevents packages such as React from pulling both development
/// and production implementations into the same browser bundle.
///
/// This ran a fixed sixty-four times, and the ceiling bounded cost rather than
/// the number of real guards: each pass called a finder that rescanned the
/// module from its first byte, so sixty-five folds cost sixty-five full scans.
/// The sixty-fifth guard and everything after it therefore survived into the
/// browser bundle, where nothing downstream can remove it — the linker injects
/// `var process = globalThis.process || { env: { NODE_ENV: "production" } }`,
/// which oxc's compressor cannot treat as a constant — and a minified package
/// carries far more than sixty-four.
///
/// The ceiling is gone because the cost it was bounding is gone. One pass
/// collects every *disjoint* guard in the text (the scan continues past a
/// match's end rather than descending into it) and applies them last-first, so
/// an earlier match's offsets stay valid. Nested guards and `else if` chains
/// that fold into another guard need another pass, so passes are bounded by
/// nesting depth rather than by guard count, and each pass is one
/// [`ast::masked_code`] scan.
///
/// Termination needs no counter: every replacement is one of the guard's own
/// inner spans — the surviving block, the `else` block, the rest of an
/// `else if` chain, or nothing — and each of those begins strictly after the
/// `if` it replaces, so a pass that folds anything strictly shortens the text.
pub(crate) fn fold_production_node_env(source: &str) -> String {
    let mut folded = source.to_string();

    loop {
        let folds = find_node_env_conditionals(&folded);
        if folds.is_empty() {
            break;
        }

        let before = folded.len();
        for (start, end, replacement) in folds.iter().rev() {
            folded.replace_range(*start..*end, replacement);
        }

        // The safety net for the invariant above. If a future replacement kind
        // ever broke it, this stops with the source partly unfolded rather than
        // spinning — larger output, which is the direction that still ships
        // working code.
        debug_assert!(
            folded.len() < before,
            "a fold must shorten the source or the scan cannot terminate"
        );
        if folded.len() >= before {
            break;
        }
    }

    folded
}

/// Every foldable guard in `source`, in ascending order and never overlapping.
///
/// A match is `(start, end, replacement)` over `source`'s own byte offsets, so
/// the caller must apply them last-first. Scanning resumes at a match's `end`,
/// which is what keeps them disjoint: a guard nested inside another one is
/// found by the next pass, once the outer fold has decided whether its bytes
/// survive at all.
fn find_node_env_conditionals(source: &str) -> Vec<(usize, usize, String)> {
    // Masked once for the whole pass. Every match's position is decided against
    // it, and the mask is only valid for `source` as it stands — which is why a
    // pass collects before it rewrites rather than rewriting as it goes.
    let masked = crate::ast::masked_code(source);
    let mut folds = Vec::new();
    let mut search_from = 0;

    while let Some((start, end, replacement)) =
        find_node_env_conditional(source, &masked, search_from)
    {
        search_from = end;
        folds.push((start, end, replacement));
    }

    folds
}

/// Locate the first foldable `if (process.env.NODE_ENV …)` guard at or after
/// `search_from`.
///
/// Every position decision is made against [`ast::masked_code`], where string,
/// template, comment, and regex text has been blanked out but every byte offset
/// still names the same place in `source`. Text is then read from `source`,
/// because the condition this has to recognise *is* a string literal
/// (`"production"`) and the folded body must be the real code.
///
/// This used to walk `source` with a private lexer, which is the arrangement
/// `ast`'s module documentation exists to prevent. That lexer treated a
/// backtick like a plain quote, so a nested template literal — `` `${m[`a`]}` ``
/// — left it scanning template text as code; an apostrophe in that text then
/// opened a "string" that ran to the next quote anywhere in the file, and every
/// development-only guard past that point stopped being folded and shipped to
/// the browser. It also read every `/` as a regex with no check for whether a
/// value was even expected there, a decision `ast` keeps private precisely
/// because it is only correct alongside the rest of the scan.
fn find_node_env_conditional(
    source: &str,
    masked: &str,
    search_from: usize,
) -> Option<(usize, usize, String)> {
    let bytes = masked.as_bytes();
    let mut search = search_from;

    while search + 1 < bytes.len() {
        if bytes[search] != b'i' || bytes.get(search + 1) != Some(&b'f') {
            search += 1;
            continue;
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
        let condition_close = matching_delimiter(masked, condition_open, b'(', b')')?;
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
        let consequent_close = matching_delimiter(masked, consequent_open, b'{', b'}')?;

        let after_consequent = skip_ascii_whitespace(bytes, consequent_close + 1);
        let else_start = else_keyword_at(masked, after_consequent).then_some(after_consequent);

        // What the `else` clause is, and where the whole statement ends.
        //
        // A clause that is neither a block nor another `if` is a brace-less
        // statement whose end cannot be found without parsing, so the guard is
        // left alone rather than half-removed.
        let Some(clause) = else_clause(masked, bytes, else_start, consequent_close) else {
            search = consequent_close + 1;
            continue;
        };
        let end = clause.end;
        // The surviving branch keeps its braces and becomes a block statement.
        //
        // Splicing the *body* in discarded the block's lexical scope, which is
        // not cosmetic: `let`, `const`, and `class` belong to that block, so a
        // folded-in `const x` landed in the parent scope and collided with an
        // outer `const x` — turning a dead-code elimination into
        // "Identifier 'x' has already been declared" for the whole bundle. A
        // bare block is valid JavaScript, costs nothing, spans the same bytes,
        // and keeps the branch's bindings exactly where the author put them.
        let replacement = if condition_result {
            source[consequent_open..=consequent_close].to_string()
        } else {
            // The `else` clause survives as written. For a block that is its
            // braces; for an `else if` chain it is the chain itself, which is
            // already a complete statement — so nothing is spliced and the
            // remaining branches keep their own conditions.
            match clause.kind {
                ElseClause::None => String::new(),
                ElseClause::Block { open, close } => source[open..=close].to_string(),
                ElseClause::Chain { at } => source[at..end].to_string(),
            }
        };

        return Some((start, end, replacement));
    }

    None
}

/// What follows the folded `if`, and where the whole statement ends.
struct FoldedElse {
    kind: ElseClause,
    /// One past the last byte of the entire `if`/`else` statement.
    end: usize,
}

enum ElseClause {
    None,
    Block {
        open: usize,
        close: usize,
    },
    /// `else if (…) { … }`, possibly continuing. `at` is the `if`.
    Chain {
        at: usize,
    },
}

/// Whether `else` appears at `index` as a keyword rather than as the start of
/// an identifier such as `elsewhere`.
fn else_keyword_at(masked: &str, index: usize) -> bool {
    masked[index..].starts_with("else")
        && !masked.as_bytes()[index + 4..]
            .first()
            .is_some_and(|byte| is_ascii_identifier_byte(*byte))
}

/// Measure the `else` clause of a guard whose consequent ends at
/// `consequent_close`.
///
/// Returns `None` when the clause is a brace-less statement, whose end cannot
/// be found without parsing. The caller then leaves the guard alone: the fold
/// used to take `consequent_close + 1` as the end whatever followed, so
///
/// ```js
/// if (process.env.NODE_ENV !== "production") { warn() } else if (flag) { run() }
/// ```
///
/// lost its `if` and left a bare `else if` behind — a syntax error that failed
/// the whole production client bundle, from a pass that runs while the graph is
/// being resolved and reports nothing.
fn else_clause(
    masked: &str,
    bytes: &[u8],
    else_start: Option<usize>,
    consequent_close: usize,
) -> Option<FoldedElse> {
    let Some(else_index) = else_start else {
        return Some(FoldedElse {
            kind: ElseClause::None,
            end: consequent_close + 1,
        });
    };

    let clause_start = skip_ascii_whitespace(bytes, else_index + 4);
    if bytes.get(clause_start) == Some(&b'{') {
        let close = matching_delimiter(masked, clause_start, b'{', b'}')?;
        return Some(FoldedElse {
            kind: ElseClause::Block {
                open: clause_start,
                close,
            },
            end: close + 1,
        });
    }

    // `else if …`: walk the rest of the chain so both outcomes know where the
    // statement ends — the kept branch has to delete it, the dropped branch has
    // to leave it intact.
    if !identifier_at(masked, clause_start, "if") {
        return None;
    }
    let end = if_statement_end(masked, bytes, clause_start)?;
    Some(FoldedElse {
        kind: ElseClause::Chain { at: clause_start },
        end,
    })
}

/// Whether `word` appears at `index` as a whole identifier.
fn identifier_at(masked: &str, index: usize, word: &str) -> bool {
    masked[index..].starts_with(word)
        && !masked.as_bytes()[index + word.len()..]
            .first()
            .is_some_and(|byte| is_ascii_identifier_byte(*byte))
}

/// One past the last byte of the `if` statement starting at `index`.
///
/// Iterative rather than recursive: an `else if` chain is arbitrarily long and
/// this walks it a link at a time. `None` for any shape it cannot measure —
/// a brace-less branch, or an unbalanced delimiter — so the caller declines to
/// fold instead of guessing where the statement stopped.
fn if_statement_end(masked: &str, bytes: &[u8], index: usize) -> Option<usize> {
    let mut at = index;
    loop {
        let condition_open = skip_ascii_whitespace(bytes, at + 2);
        if bytes.get(condition_open) != Some(&b'(') {
            return None;
        }
        let condition_close = matching_delimiter(masked, condition_open, b'(', b')')?;
        let body_open = skip_ascii_whitespace(bytes, condition_close + 1);
        if bytes.get(body_open) != Some(&b'{') {
            return None;
        }
        let body_close = matching_delimiter(masked, body_open, b'{', b'}')?;

        let after = skip_ascii_whitespace(bytes, body_close + 1);
        if !else_keyword_at(masked, after) {
            return Some(body_close + 1);
        }
        let clause_start = skip_ascii_whitespace(bytes, after + 4);
        if bytes.get(clause_start) == Some(&b'{') {
            return Some(matching_delimiter(masked, clause_start, b'{', b'}')? + 1);
        }
        if !identifier_at(masked, clause_start, "if") {
            return None;
        }
        at = clause_start;
    }
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

/// Index of the delimiter closing the one at `start`, counted over masked code.
///
/// `masked` must be [`ast::masked_code`] output: a delimiter inside a string,
/// comment, or regex has already been blanked there, so this only has to count.
fn matching_delimiter(masked: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = masked.as_bytes();
    let mut depth = 0usize;
    let mut index = start;

    while index < bytes.len() {
        match bytes[index] {
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
/// Runs [`tree_shake_pass`] to a fixed point (bounded) so that unused
/// re-export chains (`__exports.bar = __ruv_a__.foo;` where `bar` is itself
/// unused) collapse fully instead of leaving one dead hop behind after a
/// single pass.
fn tree_shake(source: &str) -> String {
    tree_shake_tracking_lines(source).0
}

/// Tree-shake, and report which input line each output line came from.
///
/// A pass never deletes a line — a dead export becomes a `// [tree-shaken]`
/// comment, and a line carrying both live and dead exports becomes two — so the
/// output is a line-for-line descendant of the input and the provenance is
/// exact. The source map needs it to survive this stage: without it the map
/// describes a document that only exists before shaking.
pub(crate) fn tree_shake_tracking_lines(source: &str) -> (String, Vec<usize>) {
    let mut current = source.to_string();
    let mut origins: Vec<usize> = (0..current.lines().count()).collect();
    for _ in 0..64 {
        let (next, pass_origins) = tree_shake_pass(&current);
        if next == current {
            break;
        }
        origins = pass_origins
            .into_iter()
            .map(|line| origins.get(line).copied().unwrap_or(line))
            .collect();
        current = next;
    }
    (current, origins)
}

/// Remove unused exports from the linked bundle, one pass.
///
/// Strategy:
/// 1. Scan for all `__ruv_<hex16>__.<member>` property accesses across the
///    entire bundle (ignoring already tree-shaken lines) to build a "used
///    set" per module.
/// 2. Remove lines matching `__exports.<name> = <name>;` where `<name>` is
///    not in the used set for that module.
///
/// This is conservative — if we can't prove an export is unused, we keep it.
/// Chained re-exports need repeated passes; see [`tree_shake`].
fn tree_shake_pass(source: &str) -> (String, Vec<usize>) {
    // Step 1: Collect all consumed members: `__ruv_xxx__.member`. An empty
    // set is meaningful, not a signal to bail — it means every remaining
    // live export in this pass is unreferenced (e.g. the last hop of a
    // cascading dead re-export chain) and should still be shaken.
    let used_members = collect_used_members(source);

    // Step 1b: Collect namespaces read as a whole. Nothing can be proven dead
    // in those modules, because the alias reaches every export.
    let opaque_modules = collect_opaque_modules(source);

    // Step 2: Remove unused `__exports.name = name;` assignments.
    let mut out = String::with_capacity(source.len());
    let mut current_module_id: Option<String> = None;

    let mut origins: Vec<usize> = Vec::new();

    for (index, line) in source.lines().enumerate() {
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
        if trimmed == "})();" {
            // Keep the closer regardless, and leave the module scope.
            out.push_str(line);
            out.push('\n');
            origins.push(index);
            current_module_id = None;
            continue;
        }

        // Check if this line holds export assignments we can remove. A single
        // line can carry several (`export { a, b } from "./mod"`), so each
        // assignment is judged on its own — dropping the whole line because its
        // first export is unused would take live exports down with it.
        if let Some(ref mod_id) = current_module_id
            && !opaque_modules.contains(mod_id)
            && let Some(statements) = split_export_assignments(trimmed)
        {
            let (kept, dropped): (Vec<_>, Vec<_>) =
                statements.into_iter().partition(|(name, _)| {
                    name == "default" || used_members.contains(&format!("{mod_id}.{name}"))
                });

            if !dropped.is_empty() {
                let indent = &line[..line.len() - line.trim_start().len()];
                if !kept.is_empty() {
                    out.push_str(indent);
                    out.push_str(&join_statements(&kept));
                    out.push('\n');
                    origins.push(index);
                }
                out.push_str(indent);
                out.push_str("// [tree-shaken] ");
                out.push_str(&join_statements(&dropped));
                out.push('\n');
                origins.push(index);
                continue;
            }
        }

        out.push_str(line);
        out.push('\n');
        origins.push(index);
    }

    // Remove trailing newline if source didn't end with one.
    if !source.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    (out, origins)
}

/// Scan the source for all `__ruv_<hex16>__.<member>` accesses.
///
/// Lines already marked `// [tree-shaken]` are skipped so a dead export's
/// own now-unreachable references don't keep something else alive forever.
///
/// Returns a set of `"__ruv_xxx__.member"` strings.
fn collect_used_members(source: &str) -> BTreeSet<String> {
    let mut members = BTreeSet::new();
    for line in source.lines() {
        if line.trim_start().starts_with("// [tree-shaken]") {
            continue;
        }
        collect_used_members_in(line, &mut members);
    }
    members
}

/// Scan the source for module namespaces read as a whole rather than through a
/// single member.
///
/// The linker binds `import * as ns from "./mod"` to `const ns = __ruv_xxx__;`,
/// and a default import of a CommonJS package to
/// `__ruv_xxx__ && __ruv_xxx__.__esModule ? __ruv_xxx__.default : __ruv_xxx__`.
/// Both hand the whole namespace to a local alias, so later `ns.member` reads
/// never appear as `__ruv_xxx__.member` and no export of that module can be
/// proven dead. Those modules are left intact.
///
/// A module's own `var __ruv_xxx__ = (function() {` header does not count — it
/// declares the namespace rather than reading it.
fn collect_opaque_modules(source: &str) -> BTreeSet<String> {
    let mut opaque = BTreeSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("// [tree-shaken]")
            || (trimmed.starts_with("var __ruv_") && trimmed.contains("= (function()"))
        {
            continue;
        }
        collect_opaque_modules_in(line, &mut opaque);
    }
    opaque
}

fn collect_opaque_modules_in(source: &str, opaque: &mut BTreeSet<String>) {
    for (module_id, rest) in module_id_references(source) {
        // A `.member` read names exactly one export; anything else (a bare
        // reference, an index, a call) can reach all of them.
        let reaches_one_member = rest
            .strip_prefix('.')
            .is_some_and(|after| after.starts_with(crate::ast::is_identifier_continue_char));
        if !reaches_one_member {
            opaque.insert(module_id);
        }
    }
}

/// Yield every `__ruv_<id>__` occurrence with the text that follows it.
fn module_id_references(source: &str) -> Vec<(String, &str)> {
    let prefix = "__ruv_";
    let mut found = Vec::new();
    let mut search = source;

    while let Some(start) = search.find(prefix) {
        let tail = &search[start..];
        let after_prefix = &tail[prefix.len()..];
        let Some(close_offset) = after_prefix.find("__") else {
            search = &search[start + prefix.len()..];
            continue;
        };

        let id_end = prefix.len() + close_offset + 2;
        found.push((tail[..id_end].to_string(), &tail[id_end..]));
        search = &search[start + id_end..];
    }

    found
}

fn collect_used_members_in(source: &str, members: &mut BTreeSet<String>) {
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
                .take_while(|c| crate::ast::is_identifier_continue_char(*c))
                .collect();

            if !member.is_empty() {
                members.insert(format!("{module_id}.{member}"));
            }
        }

        search = &search[start + id_end..];
    }
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

/// Split a line into its individual `__exports.<name> = <value>;` statements.
///
/// The linker emits one line per source `export` statement, so a barrel's
/// `export { a, b, c } from "./mod"` lands as three assignments on one line.
/// Returns `(name, statement)` pairs, or `None` when the line is not made up
/// entirely of simple export assignments — the caller then leaves it alone,
/// which keeps the pass conservative.
fn split_export_assignments(line: &str) -> Option<Vec<(String, String)>> {
    let mut rest = line.trim();
    if !rest.starts_with("__exports.") {
        return None;
    }

    let mut statements = Vec::new();
    while !rest.is_empty() {
        let after = rest.strip_prefix("__exports.")?;
        let eq_idx = after.find(" = ")?;
        let name = &after[..eq_idx];
        if !crate::ast::is_identifier_name(name) {
            return None;
        }

        let value_start = eq_idx + " = ".len();
        let value_end = value_start + after[value_start..].find(';')?;
        let value = &after[value_start..value_end];
        // Only identifiers and member accesses are safe to split on `;` —
        // anything richer could hide a `;` inside a string or call argument.
        if value.is_empty() || value.contains(['"', '\'', '`', '(', ')', '{', '}', '[', ']', ',']) {
            return None;
        }

        statements.push((name.to_string(), format!("__exports.{name} = {value};")));
        rest = after[value_end + 1..].trim_start();
    }

    if statements.is_empty() {
        None
    } else {
        Some(statements)
    }
}

/// Render `(name, statement)` pairs back onto a single line.
fn join_statements(statements: &[(String, String)]) -> String {
    statements
        .iter()
        .map(|(_, statement)| statement.as_str())
        .collect::<Vec<_>>()
        .join(" ")
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
        let out = minify(src, BundleTarget::Ssr, EsTarget::EsNext).unwrap();

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
        let out = minify(src, BundleTarget::Ssr, EsTarget::EsNext).unwrap();

        assert!(out.contains("import"));
        assert!(out.contains("from\"react\""));
        assert!(out.contains("export"));
    }

    /// The compressor may not put back what the transform took out.
    ///
    /// oxc rewrites toward the shortest equivalent form and the shortest form
    /// is often newer syntax, so `a.b ?? (a.b = 0)` compresses to `a.b ??= 0`.
    /// Left at its default the minifier undid the downlevel: a project on
    /// `build.target: es2020` shipped a client bundle with logical assignment
    /// in it, and only the unminified build looked correct.
    #[test]
    fn the_minifier_does_not_reintroduce_syntax_the_target_excludes() {
        let downlevelled = "export function probe(input) { input.count ?? (input.count = 0); input.count || (input.count = 41); return input.count }";

        let at_default =
            minify_with_options(downlevelled, BundleTarget::Client, false, EsTarget::EsNext)
                .unwrap();
        assert!(
            at_default.contains("??="),
            "the default profile is expected to compress this: {at_default}"
        );

        let at_es2020 =
            minify_with_options(downlevelled, BundleTarget::Client, false, EsTarget::Es2020)
                .unwrap();
        assert!(
            !at_es2020.contains("??=") && !at_es2020.contains("||="),
            "es2020 output must not carry logical assignment: {at_es2020}"
        );
    }

    #[test]
    fn oxc_parse_failures_abort_the_bundle() {
        let error = minify("const = ;", BundleTarget::Client, EsTarget::EsNext).unwrap_err();
        assert!(
            matches!(error, BundleError::Compiler(message) if message.contains("Oxc could not parse"))
        );
    }

    #[test]
    fn compresses_comments_and_whitespace() {
        let src = "const   x = 1; // this is a comment\nconst y = 2;";
        let out = minify(src, BundleTarget::Client, EsTarget::EsNext).unwrap();
        assert!(!out.contains("this is a comment"), "{out}");
        // Oxc merges adjacent declarations; the retired text compressor could
        // only drop whitespace between them.
        assert_eq!(out, "const x=1,y=2;");
    }

    /// A `//` inside a URL, a template, or a regex is not a comment. Literal
    /// *content* must survive; the quoting oxc chooses to re-emit it with is
    /// its own business, so this asserts the text rather than the delimiter.
    #[test]
    fn preserves_literals_and_removes_only_real_comments() {
        let src = r#"const url = "https://example.test/a//b";
const template = `keep // text and  spaces`;
const pattern = /\n( *(at)?)[a-z/]+/gi; /* remove me */"#;
        let out = minify(src, BundleTarget::Client, EsTarget::EsNext).unwrap();
        assert!(out.contains("https://example.test/a//b"), "{out}");
        assert!(out.contains("keep // text and  spaces"), "{out}");
        assert!(out.contains(r#"/\n( *(at)?)[a-z/]+/gi"#), "{out}");
        assert!(!out.contains("remove me"), "{out}");
    }

    /// Automatic semicolon insertion has to be respected, not preserved
    /// verbatim. Oxc parses the newline-sensitive positions and re-emits their
    /// meaning: `return` followed by a newline returns undefined, and `1` on
    /// its own line before `++count` is not `1++`. Asserting the original
    /// newlines instead only described how the old text compressor coped.
    #[test]
    fn preserves_automatic_semicolon_insertion_boundaries() {
        let src = "function value() { return\n{ ok: true }; }\nlet count = 1\n++count;";
        let out = minify(src, BundleTarget::Client, EsTarget::EsNext).unwrap();
        assert!(
            !out.contains("return{") && !out.contains("return {"),
            "the block after `return` must not become its return value: {out}"
        );
        assert!(
            out.contains("count=1;++count") || out.contains("count=1;\n++count"),
            "`1` and `++count` must stay separate statements: {out}"
        );
    }

    /// Licence notices must survive minification.
    ///
    /// `CodegenOptions::minify()` turns every comment class off, legal ones
    /// included, so adopting it silently stopped shipping the notices that MIT
    /// and BSD dependencies require to travel with the code. See
    /// [`codegen_options`].
    #[test]
    fn preserves_legal_comments() {
        let src = "/*! library license */ const value = 1; //! directive\nvalue;";
        let out = minify(src, BundleTarget::Client, EsTarget::EsNext).unwrap();
        assert!(out.contains("library license"), "{out}");
        assert!(out.contains("directive"), "{out}");
    }

    /// The shape that actually matters: a `/** @license … */` banner above a
    /// `"use strict"` directive the minifier deletes.
    ///
    /// This is how React — and most of the ecosystem — ships its MIT notice, so
    /// it is the case the fix has to cover. The banner survives its anchor
    /// being compressed away and lands at the end of the bundle.
    #[test]
    fn a_jsdoc_license_banner_survives_its_anchor_being_compressed_away() {
        let src = "/**\n * @license Example\n * Copyright (c) Example.\n */\n\"use strict\";\nvar x = 1;\nexport default x;";
        let out = minify(src, BundleTarget::Client, EsTarget::EsNext).unwrap();
        assert!(out.contains("@license Example"), "{out:?}");
        assert!(out.contains("Copyright (c) Example."), "{out:?}");
        assert!(
            out.find("export default").unwrap() < out.find("@license").unwrap(),
            "the notice belongs after the code, not in the middle of it: {out:?}"
        );
    }

    /// Ordinary comments are still dropped — restoring legal comments must not
    /// turn into "keep every comment", which would inflate every bundle.
    #[test]
    fn ordinary_and_jsdoc_comments_are_still_dropped() {
        let src = "/** @param {number} n */\nfunction f(n) { /* inner */ return n; }\nf(1);";
        let out = minify(src, BundleTarget::Client, EsTarget::EsNext).unwrap();
        assert!(!out.contains("@param"), "{out}");
        assert!(!out.contains("inner"), "{out}");
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

    /// The kept branch stays a block, so its bindings stay its own.
    ///
    /// Splicing the body in dropped the block scope, and `let`/`const`/`class`
    /// are bound to that block. A folded-in `const x` therefore landed beside an
    /// outer `const x` and the bundle stopped parsing with "Identifier 'x' has
    /// already been declared" — dead-code elimination breaking live code.
    #[test]
    fn node_env_folder_keeps_the_surviving_branch_in_its_own_block() {
        let src =
            "if (process.env.NODE_ENV === \"production\") { const x = 1; use(x); }\nconst x = 2;\n";
        let out = fold_production_node_env(src);
        assert!(
            out.contains("const x = 1"),
            "the live branch survives: {out}"
        );
        assert!(
            out.trim_start().starts_with('{'),
            "the branch keeps its block: {out}"
        );
        // Parsing is the real assertion: a leaked binding is a syntax error.
        minify_javascript(&out, false, EsTarget::EsNext)
            .expect("the folded output must still parse");

        // The `else` branch is kept the same way.
        let with_else = "if (process.env.NODE_ENV !== \"production\") { const y = 1; } else { const y = 2; use(y); }\nconst y = 3;\n";
        let folded = fold_production_node_env(with_else);
        assert!(folded.contains("const y = 2"), "{folded}");
        minify_javascript(&folded, false, EsTarget::EsNext)
            .expect("the folded else branch must still parse");
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

    /// A nested template literal must not blind the folder to everything after
    /// it.
    ///
    /// The private lexer this replaced treated a backtick as a plain quote, so
    /// the inner `` `…` `` closed the outer literal and left the scanner reading
    /// template text as code. The apostrophe in that text then opened a
    /// "string" with no close until the next quote anywhere in the file, and
    /// every development-only guard past it survived into the browser bundle.
    /// Nested template literals are ordinary in minified npm packages, which is
    /// exactly the code this folder runs on.
    #[test]
    fn a_nested_template_literal_does_not_hide_later_guards() {
        let src = r#"
const label = `${map[`don't`]}`;
if (process.env.NODE_ENV !== "production") {
  throw new Error("development only");
}
"#;
        let out = fold_production_node_env(src);
        assert!(
            !out.contains("development only"),
            "a guard after a nested template literal must still fold: {out}"
        );
        assert!(
            out.contains("`${map[`don't`]}`"),
            "the literal itself must survive untouched: {out}"
        );
    }

    /// Division is not a regular expression. Reading `/` as a literal opener
    /// with no check for whether a value is expected there swallowed the text
    /// between two divisions on one line — the shape minified bundles are made
    /// of — and any delimiter inside it stopped being counted.
    #[test]
    fn division_on_one_line_is_not_read_as_a_regex() {
        let src = "function f(a,b){var x=a/2;var y=b/3;if(process.env.NODE_ENV!==\"production\"){throw new Error(\"development only\")}return x+y}";
        let out = fold_production_node_env(src);
        assert!(
            !out.contains("development only"),
            "a guard after two divisions must still fold: {out}"
        );
        assert!(out.contains("var x=a/2;"), "division must survive: {out}");
    }

    /// A guard inside a template interpolation is real code and folds; the
    /// literal text around it is not and must be left alone.
    #[test]
    fn a_guard_inside_a_template_interpolation_still_folds() {
        let src = r#"const html = `<p>${(() => { if (process.env.NODE_ENV !== "production") { return "development only" } return "ok" })()}</p>`;"#;
        let out = fold_production_node_env(src);
        assert!(
            !out.contains("development only"),
            "interpolated code is code: {out}"
        );
        assert!(out.contains("<p>"), "literal text must survive: {out}");
    }

    /// However many guards a module has, all of them fold.
    ///
    /// The loop used to stop after a fixed 64 folds because each call rescanned
    /// from offset 0 and the ceiling was there to bound that cost. The 65th
    /// development-only guard and everything after it therefore shipped to the
    /// browser, and nothing downstream could remove it: the linker injects
    /// `var process = globalThis.process || { env: { NODE_ENV: "production" } }`,
    /// which oxc's compressor cannot constant-fold. Minified npm packages carry
    /// far more than 64 — `react-dom` alone is in the hundreds.
    #[test]
    fn every_development_guard_folds_however_many_there_are() {
        let mut src = String::from("'use strict';\n");
        for index in 0..100 {
            src.push_str(&format!(
                "if (process.env.NODE_ENV !== \"production\") {{ warn(\"development only {index}\"); }}\nkeep({index});\n"
            ));
        }

        let out = fold_production_node_env(&src);
        assert!(
            !out.contains("development only"),
            "no guard may survive, whatever its position: {out}"
        );
        assert!(out.contains("keep(99);"), "live code survives: {out}");
        minify_javascript(&out, false, EsTarget::EsNext)
            .expect("the folded output must still parse");
    }

    /// Nesting is not depth-limited either, and the scan resumes rather than
    /// restarting — an inner guard sits at or after the outer one's start.
    #[test]
    fn deeply_nested_guards_all_fold() {
        let depth = 100;
        let mut src = String::new();
        for _ in 0..depth {
            src.push_str("if (process.env.NODE_ENV === 'production') {\n");
        }
        src.push_str("ship();\n");
        for _ in 0..depth {
            src.push_str("} else { warn(\"development only\"); }\n");
        }

        let out = fold_production_node_env(&src);
        assert!(
            !out.contains("development only"),
            "an inner guard must fold too: {out}"
        );
        assert!(out.contains("ship();"), "the live branch survives: {out}");
        minify_javascript(&out, false, EsTarget::EsNext)
            .expect("the folded output must still parse");
    }

    /// A guard whose `else if` is another guard needs the second pass.
    ///
    /// The dropped branch leaves the chain behind as written, so the surviving
    /// `if` is a fresh foldable guard at the same offset — the one case where a
    /// pass cannot finish the work and the next one has to.
    #[test]
    fn an_else_if_chain_of_guards_folds_all_the_way_down() {
        let src = concat!(
            "if (process.env.NODE_ENV !== \"production\") { warn(\"development only\"); }\n",
            "else if (process.env.NODE_ENV === \"production\") { ship(); }\n",
            "else { other(); }\n"
        );
        let out = fold_production_node_env(src);
        assert!(!out.contains("development only"), "{out}");
        assert!(!out.contains("other()"), "{out}");
        assert!(out.contains("ship();"), "{out}");
        assert!(
            !out.contains("process.env.NODE_ENV"),
            "no guard may be left behind: {out}"
        );
        minify_javascript(&out, false, EsTarget::EsNext)
            .expect("the folded output must still parse");
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
    fn tree_shake_collapses_unused_reexport_chains() {
        // `c` re-exports `b`'s `unused`, which re-exports `a`'s `unused`.
        // Nothing ever reads `__ruv_cccc__.unused`, so all three hops should
        // shake out, not just the outermost one.
        let src = r#"var __ruv_aaaa1111aaaa1111__ = (function() {
  "use strict";
  var __exports = {};
  const unused = 2;
  __exports.unused = unused;
  return __exports;
})();
var __ruv_bbbb2222bbbb2222__ = (function() {
  "use strict";
  var __exports = {};
  __exports.unused = __ruv_aaaa1111aaaa1111__.unused;
  return __exports;
})();
var __ruv_cccc3333cccc3333__ = (function() {
  "use strict";
  var __exports = {};
  __exports.unused = __ruv_bbbb2222bbbb2222__.unused;
  return __exports;
})();
"#;
        let result = tree_shake(src);
        let active_lines: Vec<&str> = result
            .lines()
            .filter(|l| l.contains("__exports.unused") && !l.contains("[tree-shaken]"))
            .collect();
        assert!(
            active_lines.is_empty(),
            "unused re-export chain should fully collapse: {:?}",
            active_lines
        );
        assert!(result.contains("[tree-shaken] __exports.unused = unused;"));
        assert!(
            result.contains("[tree-shaken] __exports.unused = __ruv_aaaa1111aaaa1111__.unused;")
        );
        assert!(
            result.contains("[tree-shaken] __exports.unused = __ruv_bbbb2222bbbb2222__.unused;")
        );
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
        let result =
            minify_with_options(src, BundleTarget::Client, false, EsTarget::EsNext).unwrap();

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
            first_export_assignment("__exports.helper = helper;"),
            Some("helper".into())
        );
        assert_eq!(
            first_export_assignment("__exports.default = Page;"),
            Some("default".into())
        );
        assert_eq!(first_export_assignment("const x = 1;"), None);
    }

    /// Name of the first export assigned on a line, for tests.
    fn first_export_assignment(line: &str) -> Option<String> {
        Some(split_export_assignments(line)?.remove(0).0)
    }

    #[test]
    fn split_export_assignments_separates_statements() {
        let statements = split_export_assignments(
            "__exports.A = __ruv_aaaa1111aaaa1111__.A; __exports.B = __ruv_aaaa1111aaaa1111__.B;",
        )
        .expect("line is made of export assignments");

        assert_eq!(
            statements,
            vec![
                (
                    "A".to_string(),
                    "__exports.A = __ruv_aaaa1111aaaa1111__.A;".to_string()
                ),
                (
                    "B".to_string(),
                    "__exports.B = __ruv_aaaa1111aaaa1111__.B;".to_string()
                ),
            ]
        );
    }

    #[test]
    fn split_export_assignments_keeps_complex_values_intact() {
        // A `;` could hide inside a string or call, so these stay untouched.
        assert_eq!(split_export_assignments("__exports.msg = \"a; b\";"), None);
        assert_eq!(
            split_export_assignments("__exports.run = wrap(a, b);"),
            None
        );
    }

    #[test]
    fn tree_shake_keeps_used_exports_sharing_a_line() {
        // A barrel's `export { a, b, c } from "./mod"` lands as three
        // assignments on one line. Only `Image` is consumed, so the other two
        // must shake out without taking `Image` with them.
        let src = r#"var __ruv_aaaa1111aaaa1111__ = (function() {
  "use strict";
  var __exports = {};
  __exports.DEFAULT_WIDTHS = __ruv_bbbb2222bbbb2222__.DEFAULT_WIDTHS; __exports.Image = __ruv_bbbb2222bbbb2222__.Image; __exports.Picture = __ruv_bbbb2222bbbb2222__.Picture;
  return __exports;
})();
var __ruv_cccc3333cccc3333__ = (function() {
  "use strict";
  var __exports = {};
  const Image = __ruv_aaaa1111aaaa1111__.Image;
  __exports.default = Image;
  return __exports;
})();
"#;
        let result = tree_shake(src);

        // The consumed re-export survives as executable code.
        assert!(result.lines().any(|line| {
            !line.contains("[tree-shaken]")
                && line.contains("__exports.Image = __ruv_bbbb2222bbbb2222__.Image;")
        }));
        // Its unused line-mates do not.
        for dead in ["__exports.DEFAULT_WIDTHS", "__exports.Picture"] {
            let active: Vec<&str> = result
                .lines()
                .filter(|line| line.contains(dead) && !line.contains("[tree-shaken]"))
                .collect();
            assert!(active.is_empty(), "{dead} should be shaken: {active:?}");
        }
    }

    /// Folding a `NODE_ENV` guard must leave parsable JavaScript.
    ///
    /// The fold deletes code while a production client graph is being resolved,
    /// so its failure mode is a bundle that no longer parses and a pass that
    /// reports nothing. It used to take the consequent's closing brace as the
    /// end of the statement whatever followed, which cut the `if` out of an
    /// `else if` chain and left the `else` behind.
    ///
    /// Parsed rather than matched, because "the output does not parse" is the
    /// failure.
    #[test]
    fn every_foldable_shape_leaves_parsable_javascript() {
        for (name, source) in [
            (
                "production guard",
                "if (process.env.NODE_ENV === 'production') { keep() } else { drop() }\n",
            ),
            (
                "development guard",
                "if (process.env.NODE_ENV !== 'production') { drop() } else { keep() }\n",
            ),
            (
                "development guard with no else",
                "if (process.env.NODE_ENV !== 'production') { drop() }\nkeep()\n",
            ),
            (
                "else-if chain after a production guard",
                "if (process.env.NODE_ENV === 'production') { keep() } else if (flag) { drop() }\n",
            ),
            (
                "else-if chain after a development guard",
                "if (process.env.NODE_ENV !== 'production') { drop() } else if (flag) { keep() } else { other() }\n",
            ),
            (
                "long else-if chain",
                "if (process.env.NODE_ENV !== 'production') { drop() } else if (a) { one() } else if (b) { two() } else { three() }\n",
            ),
            (
                "brace-less else, left alone",
                "if (process.env.NODE_ENV !== 'production') { drop() } else keep()\n",
            ),
            (
                "brace-less consequent, left alone",
                "if (process.env.NODE_ENV !== 'production') drop()\nkeep()\n",
            ),
            (
                "identifier that merely starts with else",
                "if (process.env.NODE_ENV === 'production') { keep() }\nelsewhere()\n",
            ),
            (
                "nested guards",
                "if (process.env.NODE_ENV !== 'production') {\n  if (process.env.NODE_ENV !== 'production') { drop() } else { alsoDrop() }\n} else {\n  keep()\n}\n",
            ),
            (
                "guard quoted inside a template literal",
                "const doc = `if (process.env.NODE_ENV !== 'production') { drop() }`\nkeep()\n",
            ),
        ] {
            let folded = fold_production_node_env(source);
            assert!(
                crate::compiler::transform(&folded, false).is_ok(),
                "{name}: folded output does not parse\n--- folded ---\n{folded}"
            );
        }
    }

    /// The fold has to keep the branch a production build would run, and drop
    /// the other one — parsing alone would accept a fold that kept neither.
    #[test]
    fn the_fold_keeps_the_production_branch_and_drops_the_rest() {
        let kept = |source: &str| fold_production_node_env(source);

        let production =
            kept("if (process.env.NODE_ENV === 'production') { keep() } else { drop() }\n");
        assert!(production.contains("keep()"), "{production}");
        assert!(!production.contains("drop()"), "{production}");

        let development =
            kept("if (process.env.NODE_ENV !== 'production') { drop() } else { keep() }\n");
        assert!(development.contains("keep()"), "{development}");
        assert!(!development.contains("drop()"), "{development}");

        // The chain the guard was hiding stays whole, conditions and all.
        let chain = kept(
            "if (process.env.NODE_ENV !== 'production') { drop() } else if (flag) { keep() } else { other() }\n",
        );
        assert!(!chain.contains("drop()"), "{chain}");
        assert!(chain.contains("if (flag) { keep() }"), "{chain}");
        assert!(chain.contains("other()"), "{chain}");

        // A production guard deletes the whole chain behind it.
        let pruned = kept(
            "if (process.env.NODE_ENV === 'production') { keep() } else if (flag) { drop() } else { alsoDrop() }\n",
        );
        assert!(pruned.contains("keep()"), "{pruned}");
        assert!(!pruned.contains("drop()"), "{pruned}");
        assert!(!pruned.contains("alsoDrop()"), "{pruned}");

        // A shape it cannot measure is left exactly as written.
        let untouched = "if (process.env.NODE_ENV !== 'production') { drop() } else keep()\n";
        assert_eq!(kept(untouched), untouched);

        // Text is not code.
        let quoted = "const doc = `if (process.env.NODE_ENV !== 'production') { drop() }`\n";
        assert_eq!(kept(quoted), quoted);
    }
}
