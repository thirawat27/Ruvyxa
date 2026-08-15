//! Module linker: concatenates compiled modules into a single JS string.
//!
//! Each project-local module is wrapped in a closure-style IIFE namespace so
//! that top-level declarations do not leak across module boundaries.
//! Circular dependencies are detected before linking and reported as a
//! [`BundleError::CircularDependency`] with the full cycle path.
//!
//! ```js
//! // ── module.tsx ──
//! var __ruv_abc123__ = (function() {
//!   "use strict";
//!   var __exports = {};
//!   // … compiled JS with imports rewritten …
//!   __exports.default = MyComponent;
//!   __exports.helper = helper;
//!   return module.exports;
//! })();
//! ```
//!
//! Import/export rewrites handle all ES module patterns:
//! - `import Default from "./mod"`       → `const Default = __ruv_xxx__.default`
//! - `import { a, b } from "./mod"`      → `const a = __ruv_xxx__.a; const b = __ruv_xxx__.b`
//! - `import * as ns from "./mod"`       → `const ns = __ruv_xxx__`
//! - `import Default, { a } from "./mod"`→ `const Default = __ruv_xxx__.default; const a = __ruv_xxx__.a`
//! - `export { a } from "./mod"`         → re-exported via `__exports.a = __ruv_xxx__.a`
//! - `export * from "./mod"`             → `Object.assign(__exports, __ruv_xxx__)`
//! - `export default expr`              → `__exports.default = expr`
//! - `export const/function name`       → declaration + `__exports.name = name`
//!
//! ## Performance: Parallel Linking
//!
//! The `link_parallel` function computes topological layers and rewrites
//! modules within each layer concurrently using rayon. Since import rewrites
//! only reference the deterministic `module_id` (blake3 hash of the dep's
//! path), each module's rewrite is independent and embarrassingly parallel.
//!
//! For a 100-module graph with 5 layers, this cuts link time by ~4× on
//! a 4-core machine.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use blake3::hash;
use rayon::prelude::*;

use crate::compiler::CompiledModule;
use crate::{BundleError, BundleInput, BundleTarget, Result};

/// Detect circular dependencies in the module graph.
///
/// If a cycle is found, returns `Err(BundleError::CircularDependency)` with a
/// human-readable path: `a -> b -> c -> a`.
pub fn detect_cycles(modules: &[CompiledModule]) -> Result<()> {
    let module_map: BTreeMap<PathBuf, &CompiledModule> = modules
        .iter()
        .filter(|m| !m.is_external)
        .map(|m| (m.path.clone(), m))
        .collect();

    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    let mut stack: Vec<PathBuf> = Vec::new();

    for module in modules.iter().filter(|m| !m.is_external) {
        if !visited.contains(&module.path) {
            dfs_detect_cycle(&module.path, &module_map, &mut visited, &mut stack)?;
        }
    }

    Ok(())
}

fn dfs_detect_cycle(
    path: &PathBuf,
    module_map: &BTreeMap<PathBuf, &CompiledModule>,
    visited: &mut BTreeSet<PathBuf>,
    stack: &mut Vec<PathBuf>,
) -> Result<()> {
    if stack.contains(path) {
        let cycle_start = stack.iter().position(|p| p == path).unwrap_or(0);
        let mut parts: Vec<String> = stack[cycle_start..]
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string())
            })
            .collect();
        // Close the cycle by repeating the start.
        let start_name = parts[0].clone();
        parts.push(start_name);
        let cycle_str = parts.join(" -> ");
        return Err(BundleError::CircularDependency { cycle: cycle_str });
    }

    if visited.contains(path) {
        return Ok(());
    }

    stack.push(path.clone());

    if let Some(module) = module_map.get(path) {
        for dep in module.deps.iter() {
            if module_map.contains_key(dep) {
                dfs_detect_cycle(dep, module_map, visited, stack)?;
            }
        }
    }

    stack.pop();
    visited.insert(path.clone());

    Ok(())
}

/// Link all compiled modules into a single concatenated JS string.
///
/// Detects circular dependencies first; returns
/// [`BundleError::CircularDependency`] if a cycle is found.
pub fn link(modules: &[CompiledModule], input: &BundleInput) -> Result<String> {
    detect_cycles(modules)?;
    link_inner(modules, input, &BTreeMap::new(), &BTreeSet::new())
}

/// Inner link implementation — does NOT check for cycles.
/// Called by `link` and `link_parallel` after cycle detection.
fn link_inner(
    modules: &[CompiledModule],
    input: &BundleInput,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
    shared_modules: &BTreeSet<PathBuf>,
) -> Result<String> {
    let project_modules = ordered_project_modules(modules);

    // Pre-calculate output capacity to avoid reallocations.
    // Each module contributes: its JS source + ~200 bytes of wrapper overhead.
    let estimated_size: usize = project_modules
        .iter()
        .map(|m| m.js.len() + 200)
        .sum::<usize>()
        + 64; // header

    let mut out = String::with_capacity(estimated_size);

    let external_imports = collect_external_imports(&project_modules, input.target);
    for import in external_imports {
        out.push_str(&import);
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }

    // Header comment
    out.push_str("// Generated by ruvyxa_bundler \u{2014} do not edit\n");
    out.push_str("\"use strict\";\n\n");

    write_shared_module_bindings(&mut out, shared_modules);

    for module in &project_modules {
        let id = module_id(&module.path);
        let label = module.path.to_string_lossy().into_owned();

        out.push_str("// \u{2500}\u{2500} ");
        out.push_str(&label);
        out.push_str(" \u{2500}\u{2500}\n");

        out.push_str("var ");
        out.push_str(&id);
        out.push_str(" = (function() {\n");
        out.push_str("  \"use strict\";\n");
        out.push_str("  var __exports = {};\n");
        out.push_str("  var module = { exports: __exports };\n");
        out.push_str("  var exports = module.exports;\n");
        out.push_str(
            "  var process = globalThis.process || { env: { NODE_ENV: \"production\" } };\n",
        );

        rewrite_module_into(
            &module.js,
            &DepIndex::new(&module.deps, &module.dependency_aliases),
            dynamic_import_files,
            &mut out,
            true,
            true,
            &label,
        )?;

        out.push_str("  return module.exports;\n");
        out.push_str("})();\n\n");
    }

    if matches!(input.target, BundleTarget::Ssr | BundleTarget::Edge) {
        let entry_id = module_id(&PathBuf::from("ruvyxa:bundle-entry.tsx"));
        out.push_str("export const render = ");
        out.push_str(&entry_id);
        out.push_str(".render;\n");
    }

    Ok(out)
}

/// Link modules using parallel import/export rewriting.
///
/// Computes topological layers from the dependency graph. Modules in the same
/// layer have no dependencies on each other (only on earlier layers), so their
/// import/export rewriting can proceed concurrently via rayon.
///
/// For small graphs (<8 modules), falls back to sequential linking to avoid
/// rayon scheduling overhead. Circular dependencies are detected before linking.
pub fn link_parallel(modules: &[CompiledModule], input: &BundleInput) -> Result<String> {
    link_parallel_with_dynamic_imports(modules, input, &BTreeMap::new())
}

/// Link modules while preserving selected dynamic imports as relative ESM chunk loads.
///
/// The map is internal to chunk planning: keys are resolved module paths and values are emitted
/// chunk filenames. Imports not present in the map keep the existing inline namespace behavior.
pub(crate) fn link_parallel_with_dynamic_imports(
    modules: &[CompiledModule],
    input: &BundleInput,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
) -> Result<String> {
    link_parallel_with_dynamic_imports_and_shared_modules(
        modules,
        input,
        dynamic_import_files,
        &BTreeSet::new(),
    )
}

/// Link a route bundle while resolving selected modules from an executable
/// shared-route registry. The registry chunk must run before this bundle.
pub(crate) fn link_parallel_with_dynamic_imports_and_shared_modules(
    modules: &[CompiledModule],
    input: &BundleInput,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
    shared_modules: &BTreeSet<PathBuf>,
) -> Result<String> {
    // Cycle detection runs regardless of graph size — cheap O(V+E) DFS.
    detect_cycles(modules)?;

    let project_modules = ordered_project_modules(modules);

    // For small graphs, sequential is faster (avoids rayon overhead).
    // Note: we already detected cycles above so pass directly to `link` internals.
    if project_modules.len() < 8 {
        return link_inner(modules, input, dynamic_import_files, shared_modules);
    }

    // Phase 1: Compute external imports (sequential — cheap BTreeSet scan).
    let external_imports = collect_external_imports(&project_modules, input.target);

    // Phase 2: Parallel rewrite — each module's IIFE body is independent.
    // The rewrite only references `module_id(dep)` which is deterministic.
    let rewritten_segments: Vec<String> = project_modules
        .par_iter()
        .map(|module| {
            let id = module_id(&module.path);
            let label = module.path.to_string_lossy().into_owned();

            // Pre-size the segment buffer.
            let mut segment = String::with_capacity(module.js.len() + 200);

            segment.push_str("// \u{2500}\u{2500} ");
            segment.push_str(&label);
            segment.push_str(" \u{2500}\u{2500}\n");

            segment.push_str("var ");
            segment.push_str(&id);
            segment.push_str(" = (function() {\n");
            segment.push_str("  \"use strict\";\n");
            segment.push_str("  var __exports = {};\n");
            segment.push_str("  var module = { exports: __exports };\n");
            segment.push_str("  var exports = module.exports;\n");
            segment.push_str(
                "  var process = globalThis.process || { env: { NODE_ENV: \"production\" } };\n",
            );

            rewrite_module_into(
                &module.js,
                &DepIndex::new(&module.deps, &module.dependency_aliases),
                dynamic_import_files,
                &mut segment,
                true,
                true,
                &label,
            )?;

            segment.push_str("  return module.exports;\n");
            segment.push_str("})();\n\n");

            Ok(segment)
        })
        .collect::<Result<_>>()?;

    // Phase 3: Assemble the final output from segments (sequential concat).
    let total_size: usize = external_imports.iter().map(|s| s.len() + 1).sum::<usize>()
        + 64
        + rewritten_segments.iter().map(|s| s.len()).sum::<usize>()
        + 64;

    let mut out = String::with_capacity(total_size);

    for import in &external_imports {
        out.push_str(import);
        out.push('\n');
    }
    if !external_imports.is_empty() {
        out.push('\n');
    }

    out.push_str("// Generated by ruvyxa_bundler \u{2014} do not edit\n");
    out.push_str("\"use strict\";\n\n");

    write_shared_module_bindings(&mut out, shared_modules);

    for segment in &rewritten_segments {
        out.push_str(segment);
    }

    if matches!(input.target, BundleTarget::Ssr | BundleTarget::Edge) {
        let entry_id = module_id(&PathBuf::from("ruvyxa:bundle-entry.tsx"));
        out.push_str("export const render = ");
        out.push_str(&entry_id);
        out.push_str(".render;\n");
    }

    Ok(out)
}

/// Link project-local modules into an executable registry used by route
/// bundles. Dependency-first ordering ensures each shared module evaluates once.
pub(crate) fn link_shared_route_modules(
    modules: &[CompiledModule],
    input: &BundleInput,
) -> Result<String> {
    detect_cycles(modules)?;
    let project_modules = ordered_project_modules(modules);
    let mut out = String::new();
    for import in collect_external_imports(&project_modules, input.target) {
        out.push_str(&import);
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str("// Generated shared route runtime\n");
    out.push_str("\"use strict\";\n");
    out.push_str(
        "const __ruvyxa_shared_modules__ = globalThis.__RUVYXA_SHARED_MODULES__ ??= Object.create(null);\n\n",
    );

    for module in project_modules {
        let id = module_id(&module.path);
        let label = module.path.to_string_lossy().into_owned();
        out.push_str("var ");
        out.push_str(&id);
        out.push_str(" = __ruvyxa_shared_modules__[\"");
        out.push_str(&id);
        out.push_str("\"] = (function() {\n");
        out.push_str("  \"use strict\";\n");
        out.push_str("  var __exports = {};\n");
        out.push_str("  var module = { exports: __exports };\n");
        out.push_str("  var exports = module.exports;\n");
        out.push_str(
            "  var process = globalThis.process || { env: { NODE_ENV: \"production\" } };\n",
        );
        rewrite_module_into(
            &module.js,
            &DepIndex::new(&module.deps, &module.dependency_aliases),
            &BTreeMap::new(),
            &mut out,
            true,
            true,
            &label,
        )?;
        out.push_str("  return module.exports;\n})();\n\n");
    }

    let _ = input;
    Ok(out)
}

fn write_shared_module_bindings(out: &mut String, shared_modules: &BTreeSet<PathBuf>) {
    if shared_modules.is_empty() {
        return;
    }
    out.push_str("var __ruvyxa_shared_modules__ = globalThis.__RUVYXA_SHARED_MODULES__;\n");
    for path in shared_modules {
        let id = module_id(path);
        out.push_str("var ");
        out.push_str(&id);
        out.push_str(" = __ruvyxa_shared_modules__ && __ruvyxa_shared_modules__[\"");
        out.push_str(&id);
        out.push_str("\"];\nif (!");
        out.push_str(&id);
        out.push_str(") throw new Error(\"RUV1602 shared route module was not loaded: ");
        out.push_str(&id);
        out.push_str("\");\n");
    }
    out.push('\n');
}

/// Return project-local modules in dependency-first order.
///
/// The resolver discovers modules breadth-first from the virtual entry, which
/// means importers can appear before their dependencies.  IIFE module wrappers
/// execute eagerly, so dependencies must be linked before any module that reads
/// their namespace object.
pub fn ordered_project_modules(modules: &[CompiledModule]) -> Vec<&CompiledModule> {
    let module_map: BTreeMap<PathBuf, &CompiledModule> = modules
        .iter()
        .filter(|module| !module.is_external)
        .map(|module| (module.path.clone(), module))
        .collect();

    let mut ordered = Vec::with_capacity(module_map.len());
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();

    for module in modules.iter().filter(|module| !module.is_external) {
        visit_module(
            &module.path,
            &module_map,
            &mut visiting,
            &mut visited,
            &mut ordered,
        );
    }

    ordered
}

fn visit_module<'a>(
    path: &PathBuf,
    module_map: &BTreeMap<PathBuf, &'a CompiledModule>,
    visiting: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
    ordered: &mut Vec<&'a CompiledModule>,
) {
    if visited.contains(path) {
        return;
    }

    if !visiting.insert(path.clone()) {
        return;
    }

    let Some(module) = module_map.get(path).copied() else {
        visiting.remove(path);
        return;
    };

    for dep in module.deps.iter() {
        if module_map.contains_key(dep) {
            visit_module(dep, module_map, visiting, visited, ordered);
        }
    }

    visiting.remove(path);
    visited.insert(path.clone());
    ordered.push(module);
}

/// Deterministic identifier for a module based on its path.
///
/// Format: `__ruv_<hex16>__`
pub fn module_id(path: &Path) -> String {
    let hex = hash(path.to_string_lossy().as_bytes()).to_hex();
    format!("__ruv_{:}__", &hex[..16])
}

// ─────────────────────────────────────────────────────────────────────────────
// Import/Export rewriting engine
// ─────────────────────────────────────────────────────────────────────────────

/// Rewrite all import/export statements in a module's source.
///
/// - Project-local imports → namespace variable references
/// - Exports → `__exports.name = …` assignments
/// - External imports (not in deps) → left as-is (handled by the runtime)
fn rewrite_module_into(
    source: &str,
    deps: &DepIndex<'_>,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
    out: &mut String,
    indent: bool,
    drop_external_imports: bool,
    importer: &str,
) -> Result<()> {
    let mut pending_exports = Vec::new();
    let mut in_block_comment = false;
    let mut in_commonjs_block_comment = false;
    let module_ast = crate::ast::parse_module(source);

    if declares_esm_syntax(source, &module_ast) {
        write_rewritten_line(out, ESM_NAMESPACE_MARKER, indent);
    }

    for (line, statement_start) in lines_with_statement_offsets(source) {
        let trimmed = line.trim();

        // A line whose first non-whitespace byte sits inside a string,
        // template literal, or comment is text the module means to keep, not a
        // statement to rewrite. Rewriting it edits the literal's contents.
        let rewritten = if module_ast.is_code_offset(statement_start) {
            try_rewrite_import(trimmed, deps, drop_external_imports)?
                .map(Rewrite::Inline)
                .or_else(|| try_rewrite_export_statement(trimmed, deps))
        } else {
            None
        };

        let content = match rewritten {
            Some(Rewrite::Inline(ref content)) => content.as_str(),
            Some(Rewrite::Pending {
                ref line,
                ref assignment,
            }) => {
                pending_exports.push(assignment.clone());
                line.as_str()
            }
            None => line,
        };

        let dynamic_rewritten =
            rewrite_dynamic_imports(content, deps, dynamic_import_files, &mut in_block_comment);
        let commonjs_rewritten = rewrite_commonjs_requires_with_state(
            &dynamic_rewritten,
            deps,
            &mut in_commonjs_block_comment,
            drop_external_imports,
            importer,
        );
        write_rewritten_line(out, &commonjs_rewritten, indent);
    }

    for assignment in pending_exports {
        write_rewritten_line(out, &assignment, indent);
    }

    Ok(())
}

#[cfg(test)]
fn rewrite_commonjs_requires(line: &str, deps: &[PathBuf]) -> String {
    rewrite_commonjs_requires_with_state(
        line,
        &DepIndex::without_aliases(deps),
        &mut false,
        false,
        "<test>",
    )
}

fn rewrite_commonjs_requires_with_state(
    line: &str,
    deps: &DepIndex<'_>,
    in_block_comment: &mut bool,
    drop_unresolved: bool,
    importer: &str,
) -> String {
    let mut out = String::with_capacity(line.len());
    rewrite_requires_in_range(
        line,
        0,
        line.len(),
        deps,
        in_block_comment,
        drop_unresolved,
        importer,
        &mut out,
    );
    out
}

/// Rewrite `require()` calls in `line[start..end]`, appending to `out`.
///
/// Ranged rather than whole-line so a template literal's `${…}` interpolations
/// can be walked with the same pass that walks the statement around them.
#[allow(clippy::too_many_arguments)]
fn rewrite_requires_in_range(
    line: &str,
    start: usize,
    end: usize,
    deps: &DepIndex<'_>,
    in_block_comment: &mut bool,
    drop_unresolved: bool,
    importer: &str,
    out: &mut String,
) {
    let bytes = line.as_bytes();
    let mut index = start;
    let mut previous_significant: Option<usize> = None;

    while index < end {
        // Strings, comments, and regular expressions are copied through
        // untouched; a template's interpolations are walked. The decision lives
        // in `ast` so this pass and the dynamic-import pass below cannot
        // disagree about where a literal ends — which is how a regex holding
        // `/*` or a quote used to hide every `require()` that followed it.
        if let Some(found) =
            crate::ast::skip_non_code(bytes, index, previous_significant, in_block_comment)
        {
            index = copy_non_code(line, index, end, found, out, |code_start, code_end, out| {
                // A comment cannot escape an interpolation, so the nested walk
                // starts with its own state rather than borrowing this one.
                let mut nested = false;
                rewrite_requires_in_range(
                    line,
                    code_start,
                    code_end,
                    deps,
                    &mut nested,
                    drop_unresolved,
                    importer,
                    out,
                );
            });
            previous_significant = Some(index.saturating_sub(1));
            continue;
        }

        if bytes[index..].starts_with(b"require")
            && is_import_boundary(bytes, index)
            && let Some((specifier, after_call)) = require_call(line, index + "require".len())
            && after_call <= end
        {
            if let Some(dep_path) = deps.resolve(&specifier) {
                out.push_str(&module_id(dep_path));
                index = after_call;
                previous_significant = Some(index.saturating_sub(1));
                continue;
            }
            // Unresolved require in a client bundle: replace with a runtime
            // error so the bundle does not ship a bare `require()` call that
            // browsers cannot execute.
            //
            // The message names the importer because the stack trace cannot:
            // bundle frames point at a content-hashed chunk, so without it the
            // only clue is a specifier with no indication of who asked for it.
            if drop_unresolved {
                let escaped = escape_js_string(&specifier);
                let importer = escape_js_string(importer);
                out.push_str(&format!(
                    "(function(){{throw new Error(\"RUV1610: Cannot require \\\"{escaped}\\\" in a browser bundle (imported by {importer}). The package could not be resolved from node_modules; check that it is installed.\")}})() /* require removed */"
                ));
                index = after_call;
                previous_significant = Some(index.saturating_sub(1));
                continue;
            }
        }

        if !bytes[index].is_ascii_whitespace() {
            previous_significant = Some(index);
        }
        push_next_char(line, out, &mut index);
    }
}

/// Copy a non-code construct into `out` and return the index after it.
///
/// Template literals are copied text-first: everything outside `${…}` is data,
/// and each interpolation is handed to `rewrite_code` so the caller's own pass
/// runs inside it. Both rewriters need exactly this, and both need it to agree.
fn copy_non_code(
    line: &str,
    index: usize,
    limit: usize,
    found: crate::ast::NonCode,
    out: &mut String,
    mut rewrite_code: impl FnMut(usize, usize, &mut String),
) -> usize {
    match found {
        crate::ast::NonCode::Opaque { end } => {
            let end = end.min(limit).max(index);
            out.push_str(&line[index..end]);
            end
        }
        crate::ast::NonCode::Template {
            end,
            interpolations,
        } => {
            let end = end.min(limit).max(index);
            let mut cursor = index;
            for (code_start, code_end) in interpolations {
                // An interpolation that runs past this range belongs to a
                // literal the caller did not hand us; stop rather than reach
                // outside it.
                if code_start >= end || code_start < cursor {
                    break;
                }
                out.push_str(&line[cursor..code_start]);
                let code_end = code_end.min(end).max(code_start);
                rewrite_code(code_start, code_end, out);
                cursor = code_end;
            }
            out.push_str(&line[cursor..end]);
            end
        }
    }
}

/// Escape a value for embedding in a double-quoted JavaScript string literal.
/// Windows importer paths are full of backslashes, so this is not optional.
fn escape_js_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn require_call(line: &str, mut index: usize) -> Option<(String, usize)> {
    let bytes = line.as_bytes();
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'(') {
        return None;
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let (specifier, consumed) = quoted_value_with_len(&line[index..])?;
    index += consumed;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    (bytes.get(index) == Some(&b')')).then_some((specifier, index + 1))
}

fn rewrite_dynamic_imports(
    line: &str,
    deps: &DepIndex<'_>,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
    in_block_comment: &mut bool,
) -> String {
    let mut out = String::with_capacity(line.len());
    rewrite_dynamic_imports_in_range(
        line,
        0,
        line.len(),
        deps,
        dynamic_import_files,
        in_block_comment,
        &mut out,
    );
    out
}

/// Rewrite dynamic `import()` calls in `line[start..end]`, appending to `out`.
#[allow(clippy::too_many_arguments)]
fn rewrite_dynamic_imports_in_range(
    line: &str,
    start: usize,
    end: usize,
    deps: &DepIndex<'_>,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
    in_block_comment: &mut bool,
    out: &mut String,
) {
    let bytes = line.as_bytes();
    let mut index = start;
    let mut previous_significant: Option<usize> = None;

    while index < end {
        if let Some(found) =
            crate::ast::skip_non_code(bytes, index, previous_significant, in_block_comment)
        {
            index = copy_non_code(line, index, end, found, out, |code_start, code_end, out| {
                let mut nested = false;
                rewrite_dynamic_imports_in_range(
                    line,
                    code_start,
                    code_end,
                    deps,
                    dynamic_import_files,
                    &mut nested,
                    out,
                );
            });
            previous_significant = Some(index.saturating_sub(1));
            continue;
        }

        if bytes[index..].starts_with(b"import")
            && is_import_boundary(bytes, index)
            && let Some((specifier, after_call)) = dynamic_import_call(line, index + "import".len())
            && after_call <= end
            && let Some(dep_path) = deps.resolve(&specifier)
        {
            if let Some(file_name) = dynamic_import_files.get(dep_path) {
                // Chunks export their original module namespace as the default export, keeping
                // `await import()` observably equivalent to the inline linker path.
                out.push_str(&format!(
                    "import(\"./{file_name}\").then((module) => module.default)"
                ));
            } else {
                out.push_str("Promise.resolve(");
                out.push_str(&module_id(dep_path));
                out.push(')');
            }
            index = after_call;
            previous_significant = Some(index.saturating_sub(1));
            continue;
        }

        if !bytes[index].is_ascii_whitespace() {
            previous_significant = Some(index);
        }
        push_next_char(line, out, &mut index);
    }
}

fn is_import_boundary(bytes: &[u8], index: usize) -> bool {
    index == 0
        || !matches!(
            bytes[index - 1],
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$' | b'.'
        )
}

fn dynamic_import_call(line: &str, mut index: usize) -> Option<(String, usize)> {
    let bytes = line.as_bytes();
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    if bytes.get(index) != Some(&b'(') {
        return None;
    }
    index += 1;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let (specifier, consumed) = quoted_value_with_len(&line[index..])?;
    index += consumed;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    (bytes.get(index) == Some(&b')')).then_some((specifier, index + 1))
}

fn push_next_char(line: &str, out: &mut String, index: &mut usize) {
    let character = line[*index..]
        .chars()
        .next()
        .expect("index always points at a char boundary");
    out.push(character);
    *index += character.len_utf8();
}

fn write_rewritten_line(out: &mut String, content: &str, indent: bool) {
    if indent {
        if content.is_empty() {
            out.push('\n');
        } else {
            out.push_str("  ");
            out.push_str(content);
            out.push('\n');
        }
    } else {
        out.push_str(content);
        out.push('\n');
    }
}

enum Rewrite {
    Inline(String),
    Pending { line: String, assignment: String },
}

/// Try to rewrite an import statement. Returns None if the line is not an import.
fn try_rewrite_import(
    line: &str,
    deps: &DepIndex<'_>,
    drop_external_imports: bool,
) -> Result<Option<String>> {
    if !line.starts_with("import ") {
        return Ok(None);
    }

    // Side-effect import: `import "./styles.css"` → remove (CSS handled separately)
    if line.starts_with("import \"") || line.starts_with("import '") {
        return Ok(Some(format!("// [bundled] {line}")));
    }

    // Extract the `from "specifier"` part.
    let Some((before_from, specifier)) = split_from_specifier(line) else {
        return Ok(None);
    };

    // Find the matching dep by specifier.
    let Some(dep_path) = deps.resolve(&specifier) else {
        return Ok(if drop_external_imports {
            Some(String::new())
        } else {
            None
        });
    };
    let dep_id = module_id(dep_path);

    // Parse the import clause (the part between `import` and `from`).
    let Some(clause) = before_from.strip_prefix("import ") else {
        return Ok(None);
    };
    let clause = clause.trim();

    Ok(Some(rewrite_import_clause(clause, &dep_id)?))
}

/// Hoist the import statements that stay external to the bundle.
///
/// On the server a bare specifier left here is correct: Node resolves it at
/// load time. In a browser bundle it is not — nothing resolves `"scheduler"`
/// from a `<script type="module">`, so hoisting it verbatim makes the whole
/// chunk fail to load with a message that names neither the package nor the
/// file that wanted it. Client bundles therefore replace unresolvable bare
/// imports with bindings that throw a `RUV1611` naming both, which keeps the
/// rest of the page alive and the failure attributable. This mirrors what the
/// CommonJS path does with `RUV1610`.
fn collect_external_imports(modules: &[&CompiledModule], target: BundleTarget) -> Vec<String> {
    let mut imports = BTreeSet::new();
    // specifier -> (local bindings, importing modules). Collected across all
    // modules before emitting so one stub covers every importer of a package:
    // emitting per-module would redeclare shared binding names and produce a
    // bundle that does not parse.
    let mut unresolved: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();

    for module in modules {
        // One index per module: every import line in the module resolves
        // against the same dependency list.
        let deps = DepIndex::new(&module.deps, &module.dependency_aliases);
        let module_ast = crate::ast::parse_module(&module.js);
        for (line, statement_start) in lines_with_statement_offsets(&module.js) {
            let trimmed = line.trim();
            if !trimmed.starts_with("import ") || !module_ast.is_code_offset(statement_start) {
                continue;
            }

            let side_effect_only =
                trimmed.starts_with("import \"") || trimmed.starts_with("import '");
            let specifier = if side_effect_only {
                extract_quoted_string(trimmed.strip_prefix("import ").unwrap_or(trimmed))
            } else {
                split_from_specifier(trimmed).map(|(_, specifier)| specifier)
            };

            let Some(specifier) = specifier else {
                continue;
            };

            if is_non_js_asset_specifier(&specifier) {
                continue;
            }

            if deps.resolve(&specifier).is_some() {
                continue;
            }

            if target != BundleTarget::Client || !is_bare_specifier(&specifier) {
                imports.insert(ensure_semicolon(trimmed));
                continue;
            }

            let entry = unresolved.entry(specifier).or_default();
            entry.1.insert(module.path.to_string_lossy().into_owned());
            if side_effect_only {
                continue;
            }
            let Some((before_from, _)) = split_from_specifier(trimmed) else {
                continue;
            };
            let Some(clause) = before_from.strip_prefix("import ") else {
                continue;
            };
            // An unparsable clause is not worth failing the build over here:
            // the statement is already broken, and the importer is still
            // recorded so the stub reports it.
            if let Ok(bindings) = parse_import_clause(clause.trim()) {
                entry.0.extend(bindings.into_iter().map(|(local, _)| local));
            }
        }
    }

    let mut out: Vec<String> = imports.into_iter().collect();
    out.extend(unresolved_import_stubs(&unresolved));
    out
}

/// Emit throwing bindings for bare specifiers a browser bundle cannot resolve.
fn unresolved_import_stubs(
    unresolved: &BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)>,
) -> Vec<String> {
    if unresolved.is_empty() {
        return Vec::new();
    }

    let mut out = vec![UNRESOLVED_IMPORT_HELPER.to_string()];
    // One declaration per name across the whole bundle. Two missing packages
    // that introduce the same local name would otherwise emit a duplicate
    // `const` and break parsing — the first declaration wins, and its message
    // still names a package that has to be installed.
    let mut declared = BTreeSet::new();

    for (specifier, (bindings, importers)) in unresolved {
        let importers = importers
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let specifier_literal = crate::output::js_string(specifier);
        let importers_literal = crate::output::js_string(&importers);

        for binding in bindings {
            if !declared.insert(binding.clone()) {
                continue;
            }
            out.push(format!(
                "const {binding} = __ruvyxaMissingImport__({specifier_literal}, {}, {importers_literal});",
                crate::output::js_string(binding)
            ));
        }

        if bindings.is_empty() {
            // A side-effect-only import has no binding to defer the failure
            // onto, so `null` makes it report at load — no worse than the
            // module-resolution error it replaces, and it names the package and
            // the importer.
            out.push(format!(
                "__ruvyxaMissingImport__({specifier_literal}, null, {importers_literal});"
            ));
        }
    }

    out
}

/// Runtime half of the `RUV1611` stub.
///
/// Every trap throws, so a named binding reports the first time the missing
/// value is actually touched rather than at load — code paths that never reach
/// it keep working. The binding is a function so a missing React component
/// still reaches the render that calls it. A `null` binding has nothing to
/// defer onto and throws immediately.
const UNRESOLVED_IMPORT_HELPER: &str = concat!(
    "const __ruvyxaMissingImport__ = function(specifier, binding, importers) {",
    "const fail = function() {",
    "throw new Error(\"RUV1611: Cannot import \" + (binding ? '\"' + binding + '\" from ' : \"\") + '\"' + specifier + '\"' + \" in a browser bundle (imported by \" + importers + \"). \" + ",
    "\"The package could not be resolved from node_modules; check that it is installed.\")",
    "};",
    "if (binding === null) fail();",
    "return new Proxy(function() {}, { get: fail, apply: fail, construct: fail });",
    "};"
);

/// Each line of `source` with the byte offset of its first non-whitespace byte.
///
/// The linker decides line by line, which cannot tell an `import` or `export`
/// statement from the same characters inside a template literal — and a
/// documentation snippet in a `<pre>{`…`}</pre>` block is exactly that. Left
/// ungated, the demo's own todos page had the import line deleted out of its
/// code sample and the quoted package hoisted into the bundle as a real
/// dependency. Pairing each line with an offset is what lets the caller ask
/// [`crate::ast::ModuleAst::is_code_offset`], the one scanner that knows where
/// text begins and ends.
fn lines_with_statement_offsets(source: &str) -> impl Iterator<Item = (&str, usize)> {
    let mut offset = 0;
    source.lines().map(move |line| {
        let line_start = offset;
        // `lines()` strips the terminator; `\r\n` therefore costs two bytes.
        offset += line.len()
            + if source[offset + line.len()..].starts_with("\r\n") {
                2
            } else {
                1
            };
        let indent = line.len() - line.trim_start().len();
        (line, line_start + indent)
    })
}

/// Whether a specifier names a package rather than a file or a URL.
fn is_bare_specifier(specifier: &str) -> bool {
    !specifier.starts_with('.')
        && !specifier.starts_with('/')
        && !specifier.contains("://")
        && !specifier.starts_with("data:")
}

fn is_non_js_asset_specifier(specifier: &str) -> bool {
    let lower = specifier.to_ascii_lowercase();
    matches!(
        Path::new(&lower).extension().and_then(|ext| ext.to_str()),
        Some("css" | "scss" | "sass" | "less")
    )
}

fn ensure_semicolon(line: &str) -> String {
    if line.ends_with(';') {
        line.to_string()
    } else {
        format!("{line};")
    }
}

/// Try to rewrite an export statement. Returns None if the line is not an export.
#[cfg(test)]
fn try_rewrite_export(line: &str, deps: &[PathBuf]) -> Option<String> {
    try_rewrite_export_statement(line, &DepIndex::without_aliases(deps)).map(
        |rewrite| match rewrite {
            Rewrite::Inline(line) => line,
            Rewrite::Pending { line, assignment } => format!("{line}\n{assignment}"),
        },
    )
}

fn try_rewrite_export_statement(line: &str, deps: &DepIndex<'_>) -> Option<Rewrite> {
    if !line.starts_with("export ") {
        return None;
    }

    // `export default function/class name` or `export default expr`
    if line.starts_with("export default ") {
        let expr = line.strip_prefix("export default ")?.trim();
        // If it's a function/class declaration, keep the declaration and assign.
        if expr.starts_with("function ") || expr.starts_with("class ") {
            // `export default function Foo() {}` → `function Foo() {} __exports.default = Foo;`
            let name = extract_declaration_name(expr);
            if let Some(name) = name {
                return Some(Rewrite::Pending {
                    line: expr.to_string(),
                    assignment: format!("__exports.default = {name};"),
                });
            }
        }
        // `export default expr` → `__exports.default = expr`
        //
        // The line's own terminator is carried through untouched. Stripping it
        // and appending `;` assumed the expression ended on this line, and an
        // object or array literal usually does not: `export default {` became
        // `__exports.default = {;`, a syntax error that failed the entire
        // bundle at parse time with a message pointing at the linked output
        // rather than at the module that wrote it. Leaving the terminator alone
        // keeps the single-line form identical and lets a multi-line literal
        // finish on the lines that follow.
        return Some(Rewrite::Inline(format!("__exports.default = {expr}")));
    }

    // `export { a, b } from "./mod"` — re-export from another module
    if line.contains(" from ") {
        let (before_from, specifier) = split_from_specifier(line)?;
        let dep_path = deps.resolve(&specifier)?;
        let dep_id = module_id(dep_path);

        let clause = before_from.strip_prefix("export ")?.trim();

        // `export * from "./mod"` → `Object.assign(__exports, __ruv_xxx__)`
        if clause == "*" {
            return Some(Rewrite::Inline(format!(
                "Object.assign(__exports, {dep_id});"
            )));
        }

        // `export { a, b as c } from "./mod"` → `__exports.a = dep.a; __exports.c = dep.b;`
        if clause.starts_with('{') {
            let names = parse_named_bindings(clause);
            let assignments: Vec<String> = names
                .iter()
                .map(|(local, alias)| {
                    // `export { default as X } from "cjs-pkg"` re-exports the
                    // same value `import X from "cjs-pkg"` would bind, so it
                    // needs the same interop.
                    if local == "default" {
                        format!("__exports.{alias} = {};", interop_default(&dep_id))
                    } else {
                        format!("__exports.{alias} = {dep_id}.{local};")
                    }
                })
                .collect();
            return Some(Rewrite::Inline(assignments.join(" ")));
        }

        return None;
    }

    // `export const name = …` / `export let name = …` / `export var name = …`
    if line.starts_with("export const ")
        || line.starts_with("export let ")
        || line.starts_with("export var ")
    {
        let decl = line.strip_prefix("export ")?;
        let name = extract_var_declaration_name(decl);
        if let Some(name) = name {
            return Some(Rewrite::Pending {
                line: decl.to_string(),
                assignment: format!("__exports.{name} = {name};"),
            });
        }
        // `export const { a, b } = source` binds names too. Only a plain
        // identifier was recognised before, so a destructured export produced
        // the declaration and no `__exports` assignment at all: the names were
        // live inside the module and absent from its namespace, and an importer
        // silently received `undefined` instead of a resolution error.
        let names = destructured_binding_names(decl);
        if !names.is_empty() {
            return Some(Rewrite::Pending {
                line: decl.to_string(),
                assignment: names
                    .iter()
                    .map(|name| format!("__exports.{name} = {name};"))
                    .collect::<Vec<_>>()
                    .join(" "),
            });
        }
        return Some(Rewrite::Inline(decl.to_string()));
    }

    // `export function name(…)` / `export class name`
    if line.starts_with("export function ")
        || line.starts_with("export class ")
        || line.starts_with("export async function ")
    {
        let decl = line.strip_prefix("export ").unwrap_or(line);
        let name = extract_declaration_name(decl);
        if let Some(name) = name {
            return Some(Rewrite::Pending {
                line: decl.to_string(),
                assignment: format!("__exports.{name} = {name};"),
            });
        }
        return Some(Rewrite::Inline(decl.to_string()));
    }

    // `export { a, b }` — named exports from current module (no `from`)
    if line.starts_with("export {") && !line.contains(" from ") {
        let clause = line.strip_prefix("export ")?.trim().trim_end_matches(';');
        let names = parse_named_bindings(clause);
        let assignments: Vec<String> = names
            .iter()
            .map(|(local, alias)| format!("__exports.{alias} = {local};"))
            .collect();
        return Some(Rewrite::Inline(assignments.join(" ")));
    }

    None
}

/// What a local binding in an import clause reads from the module namespace.
#[derive(Debug, PartialEq, Eq)]
enum ImportBinding {
    Default,
    Named(String),
    Namespace,
}

/// Parse an import clause into its local bindings, in source order.
///
/// Both consumers go through here: the rewriter that points bindings at a
/// bundled module, and the client stub that replaces bindings whose package
/// could not be resolved. Parsing the clause in one place is what keeps the two
/// from disagreeing about which names a statement introduces.
fn parse_import_clause(clause: &str) -> Result<Vec<(String, ImportBinding)>> {
    let clause = clause.trim();

    // `* as ns`
    if let Some(namespace) = clause.strip_prefix("* as ") {
        return Ok(vec![(
            namespace.trim().to_string(),
            ImportBinding::Namespace,
        )]);
    }

    // `{ a, b as c }` — named imports only
    if clause.starts_with('{') {
        return Ok(parse_named_bindings(clause)
            .into_iter()
            .map(|(original, alias)| (alias, ImportBinding::Named(original)))
            .collect());
    }

    // `Default, { a, b }` — default + named
    if clause.contains(',') && clause.contains('{') {
        let comma_idx = clause.find(',').unwrap();
        let default_name = clause[..comma_idx].trim();
        let rest = clause[comma_idx + 1..].trim();

        let mut bindings = vec![(default_name.to_string(), ImportBinding::Default)];
        if rest.starts_with('{') {
            bindings.extend(
                parse_named_bindings(rest)
                    .into_iter()
                    .map(|(original, alias)| (alias, ImportBinding::Named(original))),
            );
        }
        return Ok(bindings);
    }

    // `Default, * as ns` — default + namespace import.
    if let Some((default_name, namespace_clause)) = clause.split_once(',') {
        let default_name = default_name.trim();
        let namespace_clause = namespace_clause.trim();
        if let Some(namespace) = namespace_clause.strip_prefix("* as ")
            && is_identifier(default_name)
            && is_identifier(namespace.trim())
        {
            return Ok(vec![
                (default_name.to_string(), ImportBinding::Default),
                (namespace.trim().to_string(), ImportBinding::Namespace),
            ]);
        }
    }

    // `Default` — plain default import
    if is_identifier(clause) {
        return Ok(vec![(clause.to_string(), ImportBinding::Default)]);
    }

    Err(BundleError::Compiler(format!(
        "unsupported static import clause `{clause}`"
    )))
}

/// Marker the linker writes into every namespace it compiles from ES module
/// source, so an importer can tell one from a CommonJS package at runtime.
const ESM_NAMESPACE_MARKER: &str = "__exports.__esModule = true;";

/// The value `import X from "…"` binds, for either module system.
///
/// `default` means two different things depending on what the module is. For an
/// ES module it is the `default` export. For a CommonJS package it is
/// `module.exports` itself — `require("react")` returns an object with
/// `Component` and `createContext` on it and no `default` at all. Reading
/// `.default` unconditionally is why `import React from "react"` bound
/// `undefined` and the first `React.Component` in the bundle threw.
fn interop_default(dep_id: &str) -> String {
    format!("{dep_id} && {dep_id}.__esModule ? {dep_id}.default : {dep_id}")
}

/// Whether `source` is an ES module, and so needs the namespace marker.
///
/// Import or export syntax is the signal, exactly as it is for Node: a file
/// with neither is CommonJS and its namespace stays `module.exports`.
fn declares_esm_syntax(source: &str, module_ast: &crate::ast::ModuleAst) -> bool {
    let has_esm_import = module_ast.imports.iter().any(|edge| {
        matches!(
            edge.kind,
            crate::ast::ImportKind::Static
                | crate::ast::ImportKind::SideEffect
                | crate::ast::ImportKind::ReExport
        )
    });
    has_esm_import
        || lines_with_statement_offsets(source).any(|(line, offset)| {
            line.trim_start().starts_with("export ") && module_ast.is_code_offset(offset)
        })
}

/// Rewrite an import clause given the resolved module namespace ID.
///
/// Handles:
/// - `Default` → `const Default = dep.default;`
/// - `{ a, b as c }` → `const a = dep.a; const c = dep.b;`
/// - `* as ns` → `const ns = dep;`
/// - `Default, { a, b }` and `Default, * as ns` → combined
fn rewrite_import_clause(clause: &str, dep_id: &str) -> Result<String> {
    let bindings = parse_import_clause(clause)?
        .into_iter()
        .map(|(local, source)| match source {
            ImportBinding::Default => {
                format!("const {local} = {};", interop_default(dep_id))
            }
            ImportBinding::Named(original) => format!("const {local} = {dep_id}.{original};"),
            ImportBinding::Namespace => format!("const {local} = {dep_id};"),
        })
        .collect::<Vec<_>>();

    Ok(bindings.join(" "))
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
}

/// Parse `{ a, b as c, d }` into a vec of (original, alias) pairs.
fn parse_named_bindings(clause: &str) -> Vec<(String, String)> {
    let inner = clause
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim_end_matches(';')
        .trim();

    if inner.is_empty() {
        return Vec::new();
    }

    inner
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            if let Some((original, alias)) = part.split_once(" as ") {
                Some((original.trim().to_string(), alias.trim().to_string()))
            } else {
                Some((part.to_string(), part.to_string()))
            }
        })
        .collect()
}

/// Split a line at `from "specifier"` or `from 'specifier'`.
/// Returns (everything before "from", the specifier string).
fn split_from_specifier(line: &str) -> Option<(String, String)> {
    let from_idx = line.rfind(" from ")?;
    let before = line[..from_idx].to_string();
    let after = line[from_idx + 6..].trim();

    // Extract quoted specifier.
    let specifier = extract_quoted_string(after)?;
    Some((before, specifier))
}

/// Extract a quoted string value: `"foo"` → `foo`, `'bar'` → `bar`.
fn extract_quoted_string(s: &str) -> Option<String> {
    quoted_value_with_len(s).map(|(value, _)| value)
}

fn quoted_value_with_len(s: &str) -> Option<(String, usize)> {
    let leading_ws = s.len() - s.trim_start().len();
    let s = s.trim_start().trim_end_matches(';');
    let quote = s.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = s[1..].find(quote)?;
    Some((s[1..1 + end].to_string(), leading_ws + end + 2))
}

/// Find the dependency path that matches a given specifier.
///
/// Matches by checking if the dep path ends with the specifier (after
/// stripping extensions and normalizing separators).
/// Empty alias table for [`DepIndex::without_aliases`].
#[cfg(test)]
static NO_ALIASES: BTreeMap<String, PathBuf> = BTreeMap::new();

/// One dependency's precomputed match keys.
struct DepEntry {
    /// `dep.display()` with backslashes normalized to `/`.
    normalized: String,
    /// File stem, for extensionless specifiers (`./foo` -> `/app/foo.tsx`).
    stem: String,
    /// Parent directory name, for index specifiers (`./utils` -> `.../utils/index.tsx`).
    parent_name: String,
    /// Whether this dep is a directory index module.
    is_index: bool,
    /// For deps under `node_modules`, the package-relative path with any
    /// JS/TS extension stripped.
    package_path: Option<String>,
}

impl DepEntry {
    fn new(dep: &Path) -> Self {
        let normalized = dep.display().to_string().replace('\\', "/");
        let is_index = normalized.ends_with("/index.ts")
            || normalized.ends_with("/index.tsx")
            || normalized.ends_with("/index.js")
            || normalized.ends_with("/index.jsx");
        let package_path = normalized.find("/node_modules/").map(|at| {
            let rest = &normalized[at + "/node_modules/".len()..];
            rest.strip_suffix(".tsx")
                .or_else(|| rest.strip_suffix(".ts"))
                .or_else(|| rest.strip_suffix(".jsx"))
                .or_else(|| rest.strip_suffix(".js"))
                .or_else(|| rest.strip_suffix(".mjs"))
                .or_else(|| rest.strip_suffix(".cjs"))
                .unwrap_or(rest)
                .to_string()
        });
        Self {
            stem: dep
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            parent_name: dep
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            normalized,
            is_index,
            package_path,
        }
    }
}

/// Specifier resolver over one module's dependency list.
///
/// Resolution used to re-derive, for every candidate dep on every specifier
/// lookup, that dep's forward-slash path string, its file stem, and its parent
/// directory name — values that depend only on the dep list, which is fixed for
/// the whole of a module's rewrite. Because a module is scanned once per import
/// line, per `require(` call, and per dynamic `import(`, the cost compounded
/// with both module count and import count. Measured on a release build, one
/// lookup cost 4.0 us against 50 deps and 100.9 us against 1000 — roughly two
/// seconds of pure string re-derivation for a thousand-module link pass.
///
/// Deriving each dep's keys once, when the index is built, turns resolution
/// into a scan over borrowed data: the same measurement falls to 0.43 us and
/// 2.63 us, a 9x to 38x reduction that widens with dep count. Entries stay
/// parallel to `deps`, so first-match order is unchanged.
pub(crate) struct DepIndex<'a> {
    deps: &'a [PathBuf],
    entries: Vec<DepEntry>,
    aliases: &'a BTreeMap<String, PathBuf>,
}

impl<'a> DepIndex<'a> {
    pub(crate) fn new(deps: &'a [PathBuf], aliases: &'a BTreeMap<String, PathBuf>) -> Self {
        Self {
            entries: deps.iter().map(|dep| DepEntry::new(dep)).collect(),
            deps,
            aliases,
        }
    }

    #[cfg(test)]
    pub(crate) fn without_aliases(deps: &'a [PathBuf]) -> Self {
        Self::new(deps, &NO_ALIASES)
    }

    /// Resolve `specifier` to one of this module's dependencies, consulting the
    /// alias table first.
    pub(crate) fn resolve(&self, specifier: &str) -> Option<&'a PathBuf> {
        if let Some(path) = self.aliases.get(specifier) {
            return Some(path);
        }
        self.resolve_by_path(specifier)
    }

    fn resolve_by_path(&self, specifier: &str) -> Option<&'a PathBuf> {
        let owned;
        let normalized = if specifier.contains('\\') {
            owned = specifier.replace('\\', "/");
            owned.as_str()
        } else {
            specifier
        };
        let direct_suffix = normalized.strip_prefix("./").unwrap_or(normalized);
        let spec_file = normalized.rsplit('/').next().unwrap_or(normalized);
        let spec_dir = normalized
            .rsplit_once('/')
            .map(|(dir, _)| dir)
            .unwrap_or("");

        let at = self.entries.iter().position(|entry| {
            // Direct path match. The suffix must start at a path-segment
            // boundary: a bare `ends_with` would let "./Button.module.css"
            // match ".../IconButton.module.css" and bind the wrong module.
            if entry.normalized == direct_suffix
                || entry
                    .normalized
                    .strip_suffix(direct_suffix)
                    .is_some_and(|prefix| prefix.ends_with('/'))
            {
                return true;
            }

            // Without extension: "./foo" matches "/project/app/foo.tsx", but
            // only when the directory context also matches.
            if entry.stem == spec_file
                && (spec_dir.is_empty() || entry.normalized.contains(spec_dir))
            {
                return true;
            }

            // Index file: "./utils" matches "/project/app/utils/index.tsx".
            if entry.is_index && spec_file == entry.parent_name {
                return true;
            }

            if let Some(package_path) = &entry.package_path {
                return package_path == normalized
                    || package_path
                        .strip_suffix("/index")
                        .is_some_and(|base| base == normalized)
                    || package_path
                        .strip_suffix("/client")
                        .is_some_and(|base| base == normalized);
            }

            false
        })?;

        Some(&self.deps[at])
    }
}

#[cfg(test)]
pub(crate) fn find_dep_for_specifier<'a>(
    specifier: &str,
    deps: &'a [PathBuf],
) -> Option<&'a PathBuf> {
    DepIndex::without_aliases(deps).resolve(specifier)
}

/// Extract the declared name from `function Name(…)` or `class Name …`.
fn extract_declaration_name(decl: &str) -> Option<String> {
    let decl = decl.trim();

    // Skip `async` prefix.
    let decl = decl.strip_prefix("async ").unwrap_or(decl);

    let rest = decl
        .strip_prefix("function* ")
        .or_else(|| decl.strip_prefix("function "))
        .or_else(|| decl.strip_prefix("class "))?;

    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();

    if name.is_empty() { None } else { Some(name) }
}

/// Names bound by a destructuring declaration: `const { a, b: c } = …`.
///
/// Returns an empty vector for anything that is not a destructuring pattern, or
/// for a pattern whose closing delimiter is not on this line — the linker
/// rewrites line by line and cannot see the rest. Callers treat empty as "no
/// export assignments to emit", which is the behaviour every destructured
/// export used to get.
fn destructured_binding_names(decl: &str) -> Vec<String> {
    let Some(rest) = decl
        .strip_prefix("const ")
        .or_else(|| decl.strip_prefix("let "))
        .or_else(|| decl.strip_prefix("var "))
    else {
        return Vec::new();
    };
    let rest = rest.trim_start();
    let Some(pattern) = balanced_pattern(rest) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    collect_pattern_names(pattern, &mut names);
    names
}

/// The leading `{…}` or `[…]` of `source`, when it closes within the text.
///
/// Delimiters are counted over [`crate::ast::masked_code`] so a brace inside a
/// string or a comment cannot close the pattern early.
fn balanced_pattern(source: &str) -> Option<&str> {
    let (open, close) = match source.as_bytes().first()? {
        b'{' => (b'{', b'}'),
        b'[' => (b'[', b']'),
        _ => return None,
    };
    let masked = crate::ast::masked_code(source);
    let bytes = masked.as_bytes();
    let mut depth = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == open {
            depth += 1;
        } else if *byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(&source[..=index]);
            }
        }
    }
    None
}

/// Collect the identifiers a destructuring pattern introduces.
///
/// Object elements bind the target after `:` when there is one and the key
/// otherwise; array elements bind their own target; `...rest` binds `rest`; a
/// default (`= expr`) belongs to the target, not to the names. Nested patterns
/// recurse, which is why `{ a: { b } }` reports `b` and not `a`.
fn collect_pattern_names(pattern: &str, names: &mut Vec<String>) {
    let inner = &pattern[1..pattern.len().saturating_sub(1)];
    for element in split_top_level(inner) {
        let element = element.trim();
        if element.is_empty() {
            continue;
        }
        let element = element.strip_prefix("...").unwrap_or(element).trim();
        // `key: target` — the binding is the target. Split before any default,
        // so `{ a: b = 1 }` reads `b` rather than `b = 1`.
        let target = match split_top_level_once(element, b':') {
            Some((_, target)) => target.trim(),
            None => element,
        };
        let target = match split_top_level_once(target, b'=') {
            Some((before, _)) => before.trim(),
            None => target,
        };
        if target.starts_with('{') || target.starts_with('[') {
            if let Some(nested) = balanced_pattern(target) {
                collect_pattern_names(nested, names);
            }
            continue;
        }
        if is_identifier(target) {
            names.push(target.to_string());
        }
    }
}

/// Split on commas that sit at depth zero of `source`.
fn split_top_level(source: &str) -> Vec<&str> {
    let masked = crate::ast::masked_code(source);
    let bytes = masked.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&source[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&source[start..]);
    parts
}

/// Split `source` at the first depth-zero occurrence of `separator`.
///
/// `=` is matched only as assignment: `==`, `=>`, `<=`, `>=`, and `!=` all
/// contain one and none of them opens a default value.
fn split_top_level_once(source: &str, separator: u8) -> Option<(&str, &str)> {
    let masked = crate::ast::masked_code(source);
    let bytes = masked.as_bytes();
    let mut depth = 0i32;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => depth -= 1,
            byte if *byte == separator && depth == 0 => {
                if separator == b'='
                    && (bytes.get(index + 1) == Some(&b'=')
                        || bytes.get(index + 1) == Some(&b'>')
                        || matches!(
                            index.checked_sub(1).map(|i| bytes[i]),
                            Some(b'=' | b'!' | b'<' | b'>')
                        ))
                {
                    continue;
                }
                return Some((&source[..index], &source[index + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// Extract the variable name from `const name = …` / `let name = …` / `var name = …`.
fn extract_var_declaration_name(decl: &str) -> Option<String> {
    let rest = decl
        .strip_prefix("const ")
        .or_else(|| decl.strip_prefix("let "))
        .or_else(|| decl.strip_prefix("var "))?;

    let name: String = rest
        .trim()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();

    if name.is_empty() { None } else { Some(name) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a compiled module the way the pipeline does, so tests exercise the
    /// same construction path — including the single AST parse — as a real build.
    fn fixture(path: PathBuf, js: impl Into<String>, deps: Vec<PathBuf>) -> CompiledModule {
        CompiledModule::new(path, js.into(), deps, BTreeMap::new(), false, false)
    }

    fn client_input(entry: PathBuf) -> BundleInput {
        BundleInput {
            entry,
            project_root: PathBuf::from("/p"),
            app_dir: PathBuf::from("/p/app"),
            layouts: Vec::new(),
            request_path: "/".to_string(),
            target: BundleTarget::Client,
            options: crate::BundleOptions::default(),
            specials: crate::RouteSpecials::default(),
        }
    }

    /// An object or array literal after `export default` normally starts on the
    /// `export` line and ends on a later one. Appending `;` to the rewritten
    /// first line produced `__exports.default = {;` and the whole bundle
    /// stopped parsing — the failure surfaced as an Oxc diagnostic about linked
    /// output, naming nothing the author wrote.
    #[test]
    fn a_multiline_default_export_still_parses() {
        let entry = PathBuf::from("/p/a.tsx");
        let module = fixture(
            entry.clone(),
            "export default {\n\tname: \"x\"\n};\n",
            Vec::new(),
        );

        let linked = link(&[module], &client_input(entry)).unwrap();

        assert!(
            linked.contains("__exports.default = {"),
            "the literal must open where the export did: {linked}"
        );
        assert!(
            !linked.contains("= {;"),
            "the terminator must not be moved inside the literal: {linked}"
        );
        crate::minifier::minify(&linked, BundleTarget::Client)
            .expect("the linked bundle has to be parseable JavaScript");
    }

    /// `export const { a, b } = source` binds names, and they belong in the
    /// module namespace. Only a plain identifier was recognised, so these were
    /// declared inside the IIFE and never assigned to `__exports` — importers
    /// got `undefined` with no error anywhere.
    #[test]
    fn destructured_exports_reach_the_module_namespace() {
        let entry = PathBuf::from("/p/a.tsx");
        let module = fixture(
            entry.clone(),
            "export const { alpha, beta: renamed, gamma = 1, ...rest } = source;\nexport const [first, , third] = list;\n",
            Vec::new(),
        );

        let linked = link(&[module], &client_input(entry)).unwrap();

        for name in ["alpha", "renamed", "gamma", "rest", "first", "third"] {
            assert!(
                linked.contains(&format!("__exports.{name} = {name};")),
                "`{name}` must be exported: {linked}"
            );
        }
        assert!(
            !linked.contains("__exports.beta ="),
            "the source key is not the binding; only the local name is: {linked}"
        );
    }

    /// A nested pattern binds the inner names, not the outer keys, and a
    /// default value is not a binding of its own.
    #[test]
    fn nested_and_defaulted_patterns_report_the_bound_names() {
        assert_eq!(
            destructured_binding_names("const { outer: { inner } } = source;"),
            vec!["inner".to_string()]
        );
        assert_eq!(
            destructured_binding_names("const { a = compute(1, 2), b } = source;"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(
            destructured_binding_names("const [[deep], ...tail] = source;"),
            vec!["deep".to_string(), "tail".to_string()]
        );
        // A brace inside a string cannot close the pattern early.
        assert_eq!(
            destructured_binding_names("const { a = \"}\", b } = source;"),
            vec!["a".to_string(), "b".to_string()]
        );
        // Not destructuring at all.
        assert!(destructured_binding_names("const plain = source;").is_empty());
        // The pattern does not close on this line; the linker cannot see the
        // rest, so it reports nothing rather than guessing.
        assert!(destructured_binding_names("const { a,").is_empty());
    }

    /// Every resolution rule the index precomputes for, exercised against one
    /// dep list. The keys are derived once at build time now instead of per
    /// lookup, so a mistake in that derivation silently changes which module a
    /// specifier binds to rather than failing loudly.
    #[test]
    fn dep_index_resolves_every_specifier_form() {
        let deps = vec![
            PathBuf::from("/project/app/routes/home.tsx"),
            PathBuf::from("/project/app/utils/index.tsx"),
            PathBuf::from("/project/node_modules/lodash/debounce.js"),
            PathBuf::from("/project/node_modules/preact/index.js"),
            PathBuf::from("/project/node_modules/@scope/ui/client.js"),
            PathBuf::from("/project/app/styles/theme.css"),
        ];
        let index = DepIndex::without_aliases(&deps);

        // Direct path match, at a path-segment boundary.
        assert_eq!(index.resolve("./styles/theme.css"), Some(&deps[5]));
        // Extensionless, same directory. The directory-context rule compares the
        // specifier's directory against the dep's absolute path, so a bare name
        // is what matches here.
        assert_eq!(index.resolve("./home"), Some(&deps[0]));
        // Directory index module.
        assert_eq!(index.resolve("./utils"), Some(&deps[1]));
        // Bare package subpath, extension stripped.
        assert_eq!(index.resolve("lodash/debounce"), Some(&deps[2]));
        // Package root resolving through `<pkg>/index`.
        assert_eq!(index.resolve("preact"), Some(&deps[3]));
        // Package root resolving through `<pkg>/client`.
        assert_eq!(index.resolve("@scope/ui"), Some(&deps[4]));
        // Backslash specifiers normalize before matching.
        assert_eq!(index.resolve(".\\home"), Some(&deps[0]));

        assert_eq!(index.resolve("./nothing-here"), None);
    }

    /// The alias table wins over path matching, and the index keeps that
    /// precedence.
    #[test]
    fn dep_index_prefers_an_alias_over_a_path_match() {
        let deps = vec![PathBuf::from("/project/app/routes/home.tsx")];
        let aliased = PathBuf::from("/project/vendor/home.tsx");
        let aliases = BTreeMap::from([("./routes/home".to_string(), aliased.clone())]);
        let index = DepIndex::new(&deps, &aliases);

        assert_eq!(index.resolve("./routes/home"), Some(&aliased));
    }

    #[test]
    fn find_dep_requires_a_path_segment_boundary() {
        let deps = vec![
            PathBuf::from("/app/components/IconButton.module.css"),
            PathBuf::from("/app/components/Button.module.css"),
        ];

        let resolved = find_dep_for_specifier("./Button.module.css", &deps).unwrap();
        assert_eq!(
            resolved,
            &PathBuf::from("/app/components/Button.module.css"),
            "suffix match must not bind IconButton for the Button specifier"
        );

        let icon = find_dep_for_specifier("./IconButton.module.css", &deps).unwrap();
        assert_eq!(
            icon,
            &PathBuf::from("/app/components/IconButton.module.css")
        );
    }

    #[test]
    fn module_id_is_deterministic() {
        let path = PathBuf::from("/app/foo/bar.tsx");
        let id1 = module_id(&path);
        let id2 = module_id(&path);
        assert_eq!(id1, id2);
        assert!(id1.starts_with("__ruv_"));
        assert!(id1.ends_with("__"));
    }

    /// A default import binds the `default` export of an ES module but
    /// `module.exports` of a CommonJS package, and the bundle contains both.
    #[test]
    fn rewrite_default_import_interops_with_commonjs() {
        let dep_id = "__ruv_test1234567890__";
        let result = rewrite_import_clause("React", dep_id).unwrap();
        assert_eq!(
            result,
            format!("const React = {};", interop_default(dep_id))
        );
    }

    #[test]
    fn rewrite_named_imports() {
        let dep_id = "__ruv_abc__";
        let result = rewrite_import_clause("{ useState, useEffect }", dep_id).unwrap();
        assert!(result.contains("const useState = __ruv_abc__.useState;"));
        assert!(result.contains("const useEffect = __ruv_abc__.useEffect;"));
    }

    #[test]
    fn rewrite_named_import_with_alias() {
        let dep_id = "__ruv_abc__";
        let result = rewrite_import_clause("{ foo as bar }", dep_id).unwrap();
        assert_eq!(result, "const bar = __ruv_abc__.foo;");
    }

    #[test]
    fn rewrite_namespace_import() {
        let dep_id = "__ruv_abc__";
        let result = rewrite_import_clause("* as utils", dep_id).unwrap();
        assert_eq!(result, "const utils = __ruv_abc__;");
    }

    #[test]
    fn rewrite_default_plus_named() {
        let dep_id = "__ruv_abc__";
        let result = rewrite_import_clause("React, { useState }", dep_id).unwrap();
        assert!(result.contains(&format!("const React = {};", interop_default(dep_id))));
        assert!(result.contains("const useState = __ruv_abc__.useState;"));
    }

    #[test]
    fn rewrite_default_plus_namespace() {
        let result = rewrite_import_clause("React, * as ReactNamespace", "__ruv_abc__").unwrap();
        assert_eq!(
            result,
            format!(
                "const React = {}; const ReactNamespace = __ruv_abc__;",
                interop_default("__ruv_abc__")
            )
        );
    }

    #[test]
    fn rejects_unsupported_import_clauses() {
        let error = rewrite_import_clause("React, invalid", "__ruv_abc__").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported static import clause")
        );
    }

    #[test]
    fn parse_named_bindings_basic() {
        let names = parse_named_bindings("{ a, b, c }");
        assert_eq!(names.len(), 3);
        assert_eq!(names[0], ("a".into(), "a".into()));
        assert_eq!(names[1], ("b".into(), "b".into()));
        assert_eq!(names[2], ("c".into(), "c".into()));
    }

    #[test]
    fn parse_named_bindings_with_aliases() {
        let names = parse_named_bindings("{ foo as bar, baz }");
        assert_eq!(names[0], ("foo".into(), "bar".into()));
        assert_eq!(names[1], ("baz".into(), "baz".into()));
    }

    #[test]
    fn export_default_expression() {
        let result = try_rewrite_export("export default MyComponent;", &[]);
        assert_eq!(result, Some("__exports.default = MyComponent;".into()));
    }

    #[test]
    fn export_default_function() {
        let result = try_rewrite_export("export default function Page() {}", &[]);
        assert!(result.as_ref().unwrap().contains("function Page() {}"));
        assert!(
            result
                .as_ref()
                .unwrap()
                .contains("__exports.default = Page;")
        );
    }

    #[test]
    fn export_const() {
        let result = try_rewrite_export("export const helper = () => {};", &[]);
        let r = result.unwrap();
        assert!(r.contains("const helper = () => {};"));
        assert!(r.contains("__exports.helper = helper;"));
    }

    #[test]
    fn export_named_bindings() {
        let result = try_rewrite_export("export { foo, bar };", &[]);
        let r = result.unwrap();
        assert!(r.contains("__exports.foo = foo;"));
        assert!(r.contains("__exports.bar = bar;"));
    }

    #[test]
    fn export_star_from() {
        let dep = PathBuf::from("/app/utils.ts");
        let dep_id = module_id(&dep);
        let result = try_rewrite_export("export * from \"./utils\"", std::slice::from_ref(&dep));
        assert_eq!(result, Some(format!("Object.assign(__exports, {dep_id});")));
    }

    #[test]
    fn export_named_from() {
        let dep = PathBuf::from("/app/helpers.ts");
        let dep_id = module_id(&dep);
        let result = try_rewrite_export(
            "export { foo, bar as baz } from \"./helpers\"",
            std::slice::from_ref(&dep),
        );
        let r = result.unwrap();
        assert!(r.contains(&format!("__exports.foo = {dep_id}.foo;")));
        assert!(r.contains(&format!("__exports.baz = {dep_id}.bar;")));
    }

    #[test]
    fn extract_declaration_names() {
        assert_eq!(
            extract_declaration_name("function Foo() {}"),
            Some("Foo".into())
        );
        assert_eq!(
            extract_declaration_name("class Bar extends Base {}"),
            Some("Bar".into())
        );
        assert_eq!(
            extract_declaration_name("async function fetch() {}"),
            Some("fetch".into())
        );
        assert_eq!(
            extract_declaration_name("function* gen() {}"),
            Some("gen".into())
        );
    }

    #[test]
    fn extract_var_names() {
        assert_eq!(
            extract_var_declaration_name("const foo = 1;"),
            Some("foo".into())
        );
        assert_eq!(
            extract_var_declaration_name("let bar = 'x';"),
            Some("bar".into())
        );
        assert_eq!(
            extract_var_declaration_name("var baz = {};"),
            Some("baz".into())
        );
    }

    #[test]
    fn side_effect_import_commented() {
        let result = try_rewrite_import(
            "import \"./styles.css\"",
            &DepIndex::without_aliases(&[]),
            false,
        );
        assert!(result.unwrap().unwrap().starts_with("// [bundled]"));
    }

    /// Statement offsets are what tell the line scanner which lines are code.
    /// `lines()` strips the terminator, so a `\r\n` source that is counted one
    /// byte per line drifts and starts pointing at the wrong text.
    #[test]
    fn statement_offsets_survive_indentation_and_crlf() {
        let source = "const a = 1;\r\n  import x from \"y\";\r\nlast";
        let offsets: Vec<_> = lines_with_statement_offsets(source).collect();

        assert_eq!(offsets.len(), 3);
        for (line, offset) in &offsets {
            assert!(
                source[*offset..].starts_with(line.trim()),
                "offset {offset} does not point at {line:?}"
            );
        }
    }

    /// A code sample in a template literal is string content, not a statement.
    /// Rewriting it edited the sample the page was trying to display and
    /// hoisted the quoted package into the bundle as a real dependency.
    #[test]
    fn statements_inside_a_template_literal_are_not_code() {
        let source = "const sample = `\nimport { action } from \"ruvyxa/server\"\nexport const createTodo = 1\n`;\nexport default sample;";
        let ast = crate::ast::parse_module(source);

        let offsets: Vec<_> = lines_with_statement_offsets(source).collect();
        let code: Vec<_> = offsets
            .iter()
            .filter(|(_, offset)| ast.is_code_offset(*offset))
            .map(|(line, _)| line.trim())
            .collect();

        assert!(
            !code.contains(&"import { action } from \"ruvyxa/server\""),
            "{code:?}"
        );
        assert!(!code.contains(&"export const createTodo = 1"), "{code:?}");
        assert!(code.contains(&"export default sample;"), "{code:?}");
    }

    #[test]
    fn client_link_keeps_template_literal_samples_out_of_hoisted_imports() {
        let entry = PathBuf::from("/app/docs.tsx");
        let input = BundleInput {
            entry: entry.clone(),
            project_root: PathBuf::from("/app"),
            app_dir: PathBuf::from("/app/app"),
            layouts: Vec::new(),
            request_path: "/".to_string(),
            target: BundleTarget::Client,
            options: crate::BundleOptions::default(),
            specials: crate::RouteSpecials::default(),
        };
        let module = fixture(
            entry,
            "export default function Docs() { return `\nimport { action } from \"ruvyxa/server\"\n`; }",
            Vec::new(),
        );

        let output = link(&[module], &input).unwrap();

        assert!(!output.contains("RUV1611"), "{output}");
        assert!(!output.contains("__ruvyxaMissingImport__"), "{output}");
        assert!(
            output.contains("import { action } from \"ruvyxa/server\""),
            "the sample must survive verbatim inside the literal: {output}"
        );
    }

    #[test]
    fn rewrites_local_dynamic_import_to_module_namespace_promise() {
        let dep = PathBuf::from("/app/lazy.ts");
        let dep_id = module_id(&dep);
        let mut in_block_comment = false;
        let result = rewrite_dynamic_imports(
            "const mod = await import(\"./lazy\");",
            &DepIndex::without_aliases(std::slice::from_ref(&dep)),
            &BTreeMap::new(),
            &mut in_block_comment,
        );

        assert_eq!(
            result,
            format!("const mod = await Promise.resolve({dep_id});")
        );
    }

    #[test]
    fn rewrites_planned_dynamic_import_to_an_emitted_chunk() {
        let dep = PathBuf::from("/app/lazy.ts");
        let files = BTreeMap::from([(dep.clone(), "chunk.lazy.js".to_string())]);
        let mut in_block_comment = false;
        let result = rewrite_dynamic_imports(
            "const mod = await import(\"./lazy\");",
            &DepIndex::without_aliases(std::slice::from_ref(&dep)),
            &files,
            &mut in_block_comment,
        );

        assert_eq!(
            result,
            "const mod = await import(\"./chunk.lazy.js\").then((module) => module.default);"
        );
    }

    #[test]
    fn does_not_rewrite_dynamic_import_text_in_strings_or_comments() {
        let dep = PathBuf::from("/app/lazy.ts");
        let mut in_block_comment = false;
        let lines = [
            "const example = 'import(\"./lazy\")'; // import(\"./lazy\")",
            "/* import(\"./lazy\")",
            "   import(\"./lazy\") */",
            "const mod = import(\"./lazy\");",
        ];
        let deps = DepIndex::without_aliases(std::slice::from_ref(&dep));
        let output = lines
            .iter()
            .map(|line| {
                rewrite_dynamic_imports(line, &deps, &BTreeMap::new(), &mut in_block_comment)
            })
            .collect::<Vec<_>>();

        assert!(output[0].contains("'import(\"./lazy\")'"));
        assert!(output[1].contains("import(\"./lazy\")"));
        assert!(output[2].contains("import(\"./lazy\")"));
        assert!(output[3].contains("Promise.resolve("));
    }

    #[test]
    fn split_from_specifier_works() {
        let (before, spec) = split_from_specifier("import React from \"react\"").unwrap();
        assert_eq!(before, "import React");
        assert_eq!(spec, "react");

        let (before, spec) = split_from_specifier("import { a } from './foo'").unwrap();
        assert_eq!(before, "import { a }");
        assert_eq!(spec, "./foo");
    }

    fn link_unresolved_import(target: BundleTarget, source: &str) -> String {
        let entry = PathBuf::from("/app/page.tsx");
        let input = BundleInput {
            entry: entry.clone(),
            project_root: PathBuf::from("/app"),
            app_dir: PathBuf::from("/app/app"),
            layouts: Vec::new(),
            request_path: "/".to_string(),
            target,
            options: crate::BundleOptions::default(),
            specials: crate::RouteSpecials::default(),
        };
        link(&[fixture(entry, source, Vec::new())], &input).unwrap()
    }

    /// Node resolves bare specifiers at load time, so server bundles must keep
    /// hoisting them untouched.
    #[test]
    fn server_link_hoists_external_imports() {
        let output = link_unresolved_import(
            BundleTarget::Ssr,
            "import React from \"react\";\nexport default function Page() {}",
        );

        assert!(output.starts_with("import React from \"react\";"));
        assert!(!output.contains("  import React from \"react\";"));
    }

    /// Nothing resolves a bare specifier inside a `<script type="module">`, and
    /// Ruvyxa emits no import map, so hoisting one into a browser bundle used
    /// to kill the entire chunk with a message naming neither the package nor
    /// the importer. The binding must survive as a deferred, attributable
    /// failure instead.
    #[test]
    fn client_link_replaces_unresolvable_bare_imports_with_throwing_bindings() {
        let output = link_unresolved_import(
            BundleTarget::Client,
            "import React from \"react\";\nexport default function Page() {}",
        );

        assert!(
            !output.contains("import React from \"react\";"),
            "a bare specifier must not reach a browser bundle: {output}"
        );
        assert!(output.contains("RUV1611"), "{output}");
        assert!(
            output.contains("const React = __ruvyxaMissingImport__(\"react\", \"React\""),
            "{output}"
        );
        assert!(
            output.contains("/app/page.tsx"),
            "the stub must name the importer: {output}"
        );
    }

    /// Relative specifiers are how emitted chunks reference each other. They
    /// resolve fine in the browser and must keep being hoisted.
    #[test]
    fn client_link_keeps_relative_external_imports() {
        let output = link_unresolved_import(
            BundleTarget::Client,
            "import \"./shared.chunk.js\";\nexport default function Page() {}",
        );

        assert!(
            output.starts_with("import \"./shared.chunk.js\";"),
            "{output}"
        );
        assert!(!output.contains("RUV1611"), "{output}");
    }

    /// Two modules importing the same missing package must produce one
    /// declaration per name. Emitting per importer would redeclare the binding
    /// and leave a bundle that does not parse.
    #[test]
    fn client_link_declares_each_missing_binding_once() {
        let input = BundleInput {
            entry: PathBuf::from("/app/page.tsx"),
            project_root: PathBuf::from("/app"),
            app_dir: PathBuf::from("/app/app"),
            layouts: Vec::new(),
            request_path: "/".to_string(),
            target: BundleTarget::Client,
            options: crate::BundleOptions::default(),
            specials: crate::RouteSpecials::default(),
        };
        let modules = [
            fixture(
                PathBuf::from("/app/page.tsx"),
                "import { icon } from \"ghost\";\nexport default function Page() {}",
                Vec::new(),
            ),
            fixture(
                PathBuf::from("/app/layout.tsx"),
                "import { icon } from \"ghost\";\nexport function Layout() {}",
                Vec::new(),
            ),
        ];

        let output = link(&modules, &input).unwrap();

        assert_eq!(
            output
                .matches("const icon = __ruvyxaMissingImport__")
                .count(),
            1,
            "{output}"
        );
        assert!(
            output.contains("/app/page.tsx, /app/layout.tsx")
                || output.contains("/app/layout.tsx, /app/page.tsx"),
            "both importers must be named: {output}"
        );
    }

    #[test]
    fn rewrites_commonjs_requires_for_bundled_packages() {
        let dependency = PathBuf::from("/app/node_modules/example/index.js");
        let linked = rewrite_commonjs_requires(
            "module.exports = require(\"example\");",
            std::slice::from_ref(&dependency),
        );
        assert_eq!(
            linked,
            format!("module.exports = {};", module_id(&dependency))
        );
    }

    #[test]
    fn commonjs_rewrite_preserves_string_and_comment_examples() {
        let dependency = PathBuf::from("/app/node_modules/example/index.js");
        let source = concat!(
            "const actual = require(\"example\"); ",
            "const example = 'require(\"example\")'; ",
            "const template = `require(\"example\")`; ",
            "// require(\"example\") must stay documentation"
        );

        let linked = rewrite_commonjs_requires(source, std::slice::from_ref(&dependency));

        assert!(linked.contains(&format!("const actual = {};", module_id(&dependency))));
        assert!(linked.contains("const example = 'require(\"example\")';"));
        assert!(linked.contains("const template = `require(\"example\")`;"));
        assert!(linked.contains("// require(\"example\") must stay documentation"));
    }

    #[test]
    fn a_regex_literal_does_not_open_a_comment_or_a_string() {
        // Both rewriters walk a line looking for `require(` and `import(`, and
        // both used to track strings and comments without tracking regular
        // expressions. These two literals are the exact inputs that broke it.
        let deps = [PathBuf::from("/app/node_modules/example/index.js")];
        let index = DepIndex::without_aliases(&deps);
        let expected = module_id(Path::new("/app/node_modules/example/index.js"));

        // A character class holding a slash and a star reads as `/*` to a
        // scanner with no regex state, which turned block-comment mode on and
        // carried it to every following line of the module.
        let mut in_block_comment = false;
        let rewritten = rewrite_commonjs_requires_with_state(
            "const re = /[/*]/; const x = require(\"example\");",
            &index,
            &mut in_block_comment,
            false,
            "<test>",
        );
        assert!(
            !in_block_comment,
            "a regex literal must not leave the scanner inside a block comment"
        );
        assert!(
            rewritten.contains(&expected),
            "require() after a regex literal must still be rewritten: {rewritten}"
        );
        assert!(
            rewritten.contains("/[/*]/"),
            "the literal survives: {rewritten}"
        );

        // A regex holding a quote opened a string that never closed, hiding
        // every require() later on the line. Minified CommonJS is one line.
        let mut in_block_comment = false;
        let rewritten = rewrite_commonjs_requires_with_state(
            "const q = /\"/g; const y = require(\"example\");",
            &index,
            &mut in_block_comment,
            false,
            "<test>",
        );
        assert!(
            rewritten.contains(&expected),
            "require() after a quote-bearing regex must still be rewritten: {rewritten}"
        );

        // The same hazard on the dynamic-import pass.
        let mut in_block_comment = false;
        let rewritten = rewrite_dynamic_imports(
            "const re = /[/*]/; const p = import(\"example\");",
            &index,
            &BTreeMap::new(),
            &mut in_block_comment,
        );
        assert!(!in_block_comment, "dynamic-import pass must agree");
        assert!(
            rewritten.contains("Promise.resolve("),
            "import() after a regex literal must still be rewritten: {rewritten}"
        );
    }

    #[test]
    fn a_require_inside_a_template_interpolation_is_rewritten() {
        // `${…}` is code, not template text. Skipping the template whole left a
        // bare `require()` in a browser bundle, which is a ReferenceError at
        // load — the same failure an unresolved require produces, but silent at
        // build time because nothing was looking inside the literal.
        let deps = [PathBuf::from("/app/node_modules/example/index.js")];
        let index = DepIndex::without_aliases(&deps);
        let expected = module_id(Path::new("/app/node_modules/example/index.js"));
        let mut in_block_comment = false;

        let rewritten = rewrite_commonjs_requires_with_state(
            "const banner = `built with ${require(\"example\").name}`;",
            &index,
            &mut in_block_comment,
            false,
            "<test>",
        );
        assert!(
            rewritten.contains(&expected),
            "require() inside an interpolation must be rewritten: {rewritten}"
        );
        assert!(
            rewritten.starts_with("const banner = `built with ${"),
            "the surrounding template text is preserved: {rewritten}"
        );
    }

    #[test]
    fn template_text_that_looks_like_code_is_left_alone() {
        // The other half: text outside `${…}` is data even when it reads like a
        // call, and a nested template must not be mistaken for the end of the
        // outer one.
        let deps = [PathBuf::from("/app/node_modules/example/index.js")];
        let index = DepIndex::without_aliases(&deps);
        let mut in_block_comment = false;

        let source = "const doc = `see require(\"example\") and ${inner(`${nested}`)} end`;";
        let rewritten = rewrite_commonjs_requires_with_state(
            source,
            &index,
            &mut in_block_comment,
            false,
            "<test>",
        );
        assert_eq!(
            rewritten, source,
            "template text and nested templates stay untouched"
        );
    }

    #[test]
    fn division_after_a_value_is_not_treated_as_a_regex() {
        // The other half of the decision: `/` after something that ends a value
        // is division. Reading it as a regex would swallow real code, so this
        // guards the fix from overcorrecting.
        let deps = [PathBuf::from("/app/node_modules/example/index.js")];
        let index = DepIndex::without_aliases(&deps);
        let expected = module_id(Path::new("/app/node_modules/example/index.js"));
        let mut in_block_comment = false;
        let rewritten = rewrite_commonjs_requires_with_state(
            "const n = total / count; const x = require(\"example\");",
            &index,
            &mut in_block_comment,
            false,
            "<test>",
        );
        assert!(!in_block_comment);
        assert!(
            rewritten.contains(&expected),
            "division must not hide the require: {rewritten}"
        );
    }

    #[test]
    fn a_block_comment_still_carries_across_lines() {
        // Regex handling must not cost the cross-line state the rewriters
        // genuinely need: an unterminated `/*` still owns the next line.
        let deps = [PathBuf::from("/app/node_modules/example/index.js")];
        let index = DepIndex::without_aliases(&deps);
        let expected = module_id(Path::new("/app/node_modules/example/index.js"));
        let mut in_block_comment = false;

        let first = rewrite_commonjs_requires_with_state(
            "/* opening a comment",
            &index,
            &mut in_block_comment,
            false,
            "<test>",
        );
        assert_eq!(first, "/* opening a comment");
        assert!(in_block_comment, "the comment is still open");

        let second = rewrite_commonjs_requires_with_state(
            "still comment: require(\"example\") */ const x = require(\"example\");",
            &index,
            &mut in_block_comment,
            false,
            "<test>",
        );
        assert!(!in_block_comment, "the comment closed on this line");
        assert!(
            second.contains("still comment: require(\"example\")"),
            "the commented require is untouched: {second}"
        );
        assert!(
            second.contains(&expected),
            "the real require after the comment is rewritten: {second}"
        );
    }

    #[test]
    fn unresolved_require_replaced_with_runtime_error_when_drop_enabled() {
        // Simulates a client bundle where an unresolved require() must not
        // pass through — it would cause "ReferenceError: require is not defined"
        // in the browser.
        let result = rewrite_commonjs_requires_with_state(
            "const x = require(\"unknown-pkg\");",
            &DepIndex::without_aliases(&[]),
            &mut false,
            true,
            "C:\\app\\node_modules\\dependent\\index.js",
        );
        assert!(
            result.contains("RUV1610"),
            "should emit RUV1610 error code: {result}"
        );
        assert!(
            result.contains("unknown-pkg"),
            "should mention the specifier: {result}"
        );
        assert!(
            result.contains("dependent"),
            "should name the importer, which the stack trace cannot: {result}"
        );
        assert!(
            !result.contains("\\app\\node"),
            "importer backslashes must be escaped for the JS literal: {result}"
        );
        assert!(
            !result.contains("require(\"unknown-pkg\")"),
            "bare require() must not survive in client bundle: {result}"
        );
    }

    #[test]
    fn unresolved_require_left_intact_when_drop_disabled() {
        // SSR/Edge bundles run on Node.js — unresolved require() is valid.
        let result = rewrite_commonjs_requires_with_state(
            "const x = require(\"fs\");",
            &DepIndex::without_aliases(&[]),
            &mut false,
            false,
            "/app/server.js",
        );
        assert!(
            result.contains("require(\"fs\")"),
            "require should be preserved for SSR: {result}"
        );
    }

    #[test]
    fn small_graph_commonjs_modules_return_reassigned_module_exports() {
        let path = PathBuf::from("/app/node_modules/example/index.js");
        let input = BundleInput {
            entry: path.clone(),
            project_root: PathBuf::from("/app"),
            app_dir: PathBuf::from("/app/app"),
            layouts: Vec::new(),
            request_path: "/".to_string(),
            target: BundleTarget::Client,
            options: crate::BundleOptions::default(),
            specials: crate::RouteSpecials::default(),
        };
        let module = fixture(path, "module.exports = { answer: 42 };", Vec::new());

        let output = link_parallel(&[module], &input).unwrap();

        assert!(output.contains("var module = { exports: __exports };"));
        assert!(output.contains("var exports = module.exports;"));
        assert!(output.contains("return module.exports;"));
        assert!(!output.contains("return __exports;"));
    }

    #[test]
    fn link_orders_dependencies_before_importers() {
        let page = PathBuf::from("/app/app/page.tsx");
        let helper = PathBuf::from("/app/app/helper.ts");
        let input = BundleInput {
            entry: page.clone(),
            project_root: PathBuf::from("/app"),
            app_dir: PathBuf::from("/app/app"),
            layouts: Vec::new(),
            request_path: "/".to_string(),
            target: BundleTarget::Client,
            options: crate::BundleOptions::default(),
            specials: crate::RouteSpecials::default(),
        };
        let modules = vec![
            fixture(
                page.clone(),
                "import { label } from \"./helper\";\nexport default function Page() { return label; }",
                vec![helper.clone()],
            ),
            fixture(
                helper.clone(),
                "export const label = \"ready\";",
                Vec::new(),
            ),
        ];

        let output = link(&modules, &input).unwrap();
        let helper_pos = output.find(&module_id(&helper)).unwrap();
        let page_pos = output.find(&module_id(&page)).unwrap();

        assert!(helper_pos < page_pos);
    }

    #[test]
    fn link_appends_multiline_export_assignments_after_module_body() {
        let page = PathBuf::from("/app/app/layout.tsx");
        let input = BundleInput {
            entry: page.clone(),
            project_root: PathBuf::from("/app"),
            app_dir: PathBuf::from("/app/app"),
            layouts: Vec::new(),
            request_path: "/".to_string(),
            target: BundleTarget::Client,
            options: crate::BundleOptions::default(),
            specials: crate::RouteSpecials::default(),
        };
        let module = fixture(
            page,
            r#"export const meta = {
  title: "Ruvyxa",
};
export default function Layout({ children }) {
  return children;
}"#,
            Vec::new(),
        );

        let output = link(&[module], &input).unwrap();
        let object_end = output.find("  };").unwrap();
        let meta_export = output.find("  __exports.meta = meta;").unwrap();
        let function_end = output.rfind("  }").unwrap();
        let default_export = output.find("  __exports.default = Layout;").unwrap();

        assert!(object_end < meta_export);
        assert!(function_end < default_export);
    }

    #[test]
    fn ssr_link_exports_virtual_entry_render() {
        let entry = PathBuf::from("ruvyxa:bundle-entry.tsx");
        let input = BundleInput {
            entry: PathBuf::from("/app/app/page.tsx"),
            project_root: PathBuf::from("/app"),
            app_dir: PathBuf::from("/app/app"),
            layouts: Vec::new(),
            request_path: "/".to_string(),
            target: BundleTarget::Ssr,
            options: crate::BundleOptions::default(),
            specials: crate::RouteSpecials::default(),
        };
        let module = fixture(
            entry.clone(),
            "export async function render(ctx) {\n  return String(ctx.path);\n}",
            Vec::new(),
        );

        let output = link(&[module], &input).unwrap();

        assert!(output.contains(&format!(
            "export const render = {}.render;",
            module_id(&entry)
        )));
    }

    #[test]
    fn detect_cycles_finds_simple_cycle() {
        let a = PathBuf::from("/app/a.ts");
        let b = PathBuf::from("/app/b.ts");

        let modules = vec![
            fixture(a.clone(), "import B from './b';", vec![b.clone()]),
            fixture(b.clone(), "import A from './a';", vec![a.clone()]),
        ];

        let result = detect_cycles(&modules);
        assert!(result.is_err(), "circular dep should be an error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("circular dependency"),
            "error message should mention circular dependency: {err}"
        );
    }

    #[test]
    fn detect_cycles_no_false_positive_on_diamond() {
        // Diamond: page → A, page → B, A → shared, B → shared
        let page = PathBuf::from("/app/page.ts");
        let a = PathBuf::from("/app/a.ts");
        let b = PathBuf::from("/app/b.ts");
        let shared = PathBuf::from("/app/shared.ts");

        let modules = vec![
            fixture(page.clone(), String::new(), vec![a.clone(), b.clone()]),
            fixture(a.clone(), String::new(), vec![shared.clone()]),
            fixture(b.clone(), String::new(), vec![shared.clone()]),
            fixture(shared.clone(), String::new(), vec![]),
        ];

        // Diamond graph is NOT circular.
        assert!(detect_cycles(&modules).is_ok(), "diamond is not circular");
    }
}
