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

/// Group the modules that sit in an import cycle, by Tarjan's algorithm.
///
/// A cycle is legal ESM and common inside published packages — `zod` has one
/// between `schemas.js` and `iso.js` — so the linker links around it rather
/// than refusing the graph, which is what it used to do. Only modules in a
/// strongly connected component of more than one module, or importing
/// themselves, appear in the result; every other bundle links byte for byte as
/// it did.
pub fn cycle_groups(modules: &[CompiledModule]) -> BTreeMap<PathBuf, u32> {
    let module_map: BTreeMap<PathBuf, &CompiledModule> = modules
        .iter()
        .filter(|module| !module.is_external)
        .map(|module| (module.path.clone(), module))
        .collect();

    let mut state = TarjanState {
        index: BTreeMap::new(),
        low_link: BTreeMap::new(),
        on_stack: BTreeSet::new(),
        stack: Vec::new(),
        groups: BTreeMap::new(),
        counter: 0,
        next_group: 0,
    };

    for module in modules.iter().filter(|module| !module.is_external) {
        if !state.index.contains_key(&module.path) {
            strong_connect(&module.path, &module_map, &mut state);
        }
    }

    state.groups
}

struct TarjanState {
    index: BTreeMap<PathBuf, u32>,
    low_link: BTreeMap<PathBuf, u32>,
    on_stack: BTreeSet<PathBuf>,
    stack: Vec<PathBuf>,
    groups: BTreeMap<PathBuf, u32>,
    counter: u32,
    next_group: u32,
}

fn strong_connect(
    path: &PathBuf,
    module_map: &BTreeMap<PathBuf, &CompiledModule>,
    state: &mut TarjanState,
) {
    state.index.insert(path.clone(), state.counter);
    state.low_link.insert(path.clone(), state.counter);
    state.counter += 1;
    state.stack.push(path.clone());
    state.on_stack.insert(path.clone());

    let mut self_referential = false;
    if let Some(module) = module_map.get(path) {
        for dep in module.deps.iter() {
            if !module_map.contains_key(dep) {
                continue;
            }
            if dep == path {
                self_referential = true;
            }
            if !state.index.contains_key(dep) {
                strong_connect(dep, module_map, state);
                let dep_low = state.low_link.get(dep).copied().unwrap_or(u32::MAX);
                let own = state.low_link.get(path).copied().unwrap_or(u32::MAX);
                state.low_link.insert(path.clone(), own.min(dep_low));
            } else if state.on_stack.contains(dep) {
                let dep_index = state.index.get(dep).copied().unwrap_or(u32::MAX);
                let own = state.low_link.get(path).copied().unwrap_or(u32::MAX);
                state.low_link.insert(path.clone(), own.min(dep_index));
            }
        }
    }

    if state.low_link.get(path) != state.index.get(path) {
        return;
    }

    let mut component = Vec::new();
    while let Some(member) = state.stack.pop() {
        state.on_stack.remove(&member);
        let done = &member == path;
        component.push(member);
        if done {
            break;
        }
    }
    if component.len() > 1 || self_referential {
        state.next_group += 1;
        for member in component {
            state.groups.insert(member, state.next_group);
        }
    }
}

/// Link all compiled modules into a single concatenated JS string.
///
/// Detects circular dependencies first; returns
/// [`BundleError::CircularDependency`] if a cycle is found.
pub fn link(modules: &[CompiledModule], input: &BundleInput) -> Result<String> {
    Ok(link_with_origins(modules, input, &BTreeMap::new(), &BTreeSet::new())?.code)
}

/// Where one line of linked output came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineOrigin {
    /// Index into [`LinkedBundle::modules`].
    pub(crate) module: u32,
    /// 0-based line in that module's compiled source.
    pub(crate) line: u32,
}

/// Linked output together with the provenance of every line in it.
///
/// The source map has to know which module each generated line came from and
/// which line of that module it was. Deriving that from the emit format — how
/// many lines the header takes, how many the per-module preamble adds — is what
/// the bundler used to do, with three constants, and it could not have been
/// right: the external-import block above the header has no fixed length, the
/// preamble is longer than the constant said, and `rewrite_module_into` can
/// write more lines than it reads. Recording the positions while the text is
/// being built is the only way they are exact.
pub(crate) struct LinkedBundle {
    pub(crate) code: String,
    /// Modules in emit order.
    pub(crate) modules: Vec<PathBuf>,
    /// One entry per line of `code`; `None` for lines the linker wrote itself.
    pub(crate) line_origins: Vec<Option<LineOrigin>>,
}

/// One module's IIFE, with the provenance of each of its lines.
struct ModuleSegment {
    text: String,
    origins: Vec<Option<u32>>,
}

/// Lines held by a buffer every writer newline-terminates.
fn count_lines(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count()
}

/// Emit one module's IIFE.
///
/// The wrapper was written out twice, byte for byte, once in the sequential
/// path and once in the parallel one. The source map reads its shape, so a
/// third reader of the same format is precisely what this file did not need.
fn write_module_segment(
    module: &CompiledModule,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
    cyclic_deps: &BTreeSet<PathBuf>,
    in_cycle: bool,
) -> Result<ModuleSegment> {
    let id = module_id(&module.path);
    let label = module.path.to_string_lossy().into_owned();

    // The body is rewritten first, because the wrapper's own declarations
    // depend on what it binds. A module that binds `module`, `exports`, or
    // `process` itself — `zod` imports a function called `process` from a
    // sibling — would otherwise have the wrapper redeclare that name in the
    // same scope, and the whole chunk fails to parse in the browser with
    // "Identifier 'process' has already been declared", naming a line the
    // author never wrote.
    let mut body = String::with_capacity(module.js.len() + 64);
    let body_origins = rewrite_module_into(
        &module.js,
        &DepIndex::new(&module.deps, &module.dependency_aliases),
        &mut body,
        &ModuleRewrite {
            dynamic_import_files,
            indent: true,
            drop_external_imports: true,
            importer: &label,
            cyclic_deps,
        },
    )?;
    let bound = top_level_bound_names(&body);
    let owns_module = bound.contains("module");
    // A cycle member publishes its exports object before its body runs and is
    // invoked in place, so there is no call expression to await. A module that
    // both awaits and closes a cycle is left to fail loudly rather than be
    // linked into something subtly wrong.
    let awaits = !in_cycle && has_top_level_await(&body);
    let exports_expression = if owns_module {
        "__exports"
    } else {
        "module.exports"
    };

    let mut text = String::with_capacity(body.len() + 200);
    text.push_str("// \u{2500}\u{2500} ");
    text.push_str(&label);
    text.push_str(" \u{2500}\u{2500}\n");
    if in_cycle {
        // A module in a cycle publishes its exports object before its body
        // runs, so the module that closes the cycle has something to hold. Its
        // `var` was declared with the rest of the group above.
        text.push_str("(function() {\n");
        text.push_str("  \"use strict\";\n");
        text.push_str("  var __exports = ");
        text.push_str(&id);
        text.push_str(";\n");
    } else if awaits {
        // A module that awaits in its own body needs an async wrapper, and the
        // bundle's own top level — where this call sits — may await it.
        text.push_str("var ");
        text.push_str(&id);
        text.push_str(" = await (async function() {\n");
        text.push_str("  \"use strict\";\n");
        text.push_str("  var __exports = {};\n");
    } else {
        text.push_str("var ");
        text.push_str(&id);
        text.push_str(" = (function() {\n");
        text.push_str("  \"use strict\";\n");
        text.push_str("  var __exports = {};\n");
    }
    if !owns_module {
        text.push_str("  var module = { exports: __exports };\n");
    }
    if !bound.contains("exports") {
        text.push_str("  var exports = ");
        text.push_str(exports_expression);
        text.push_str(";\n");
    }
    if !bound.contains("process") {
        text.push_str(
            "  var process = globalThis.process || { env: { NODE_ENV: \"production\" } };\n",
        );
    }

    let mut origins = vec![None; count_lines(&text)];
    text.push_str(&body);
    origins.extend(body_origins);

    if in_cycle {
        // A CommonJS module in the cycle may have replaced `module.exports`
        // wholesale; the identity its importers hold is the one published above.
        text.push_str("  if (");
        text.push_str(exports_expression);
        text.push_str(" !== __exports) Object.assign(__exports, ");
        text.push_str(exports_expression);
        text.push_str(");\n");
        text.push_str("})();\n\n");
    } else {
        text.push_str("  return ");
        text.push_str(exports_expression);
        text.push_str(";\n");
        text.push_str("})();\n\n");
    }
    origins.resize(count_lines(&text), None);
    Ok(ModuleSegment { text, origins })
}

/// Assemble the header, the module segments, and the target's footer.
fn assemble_linked(
    project_modules: &[&CompiledModule],
    segments: &[ModuleSegment],
    input: &BundleInput,
    shared_modules: &BTreeSet<PathBuf>,
    cycle_groups: &BTreeMap<PathBuf, u32>,
) -> LinkedBundle {
    let estimated = segments
        .iter()
        .map(|segment| segment.text.len())
        .sum::<usize>()
        + 256;
    let mut code = String::with_capacity(estimated);

    let external_imports = collect_external_imports(project_modules, input.target);
    for import in &external_imports {
        code.push_str(import);
        code.push('\n');
    }
    if !external_imports.is_empty() {
        code.push('\n');
    }
    code.push_str("// Generated by ruvyxa_bundler \u{2014} do not edit\n");
    code.push_str("\"use strict\";\n\n");
    write_shared_module_bindings(&mut code, shared_modules);
    write_cycle_prelude(&mut code, cycle_groups);

    // Where each cycle starts and ends among the emitted modules. Neither is
    // always the module beside the group: an acyclic dependency of one member
    // is emitted between two of them.
    let mut first_of_group: BTreeMap<u32, usize> = BTreeMap::new();
    let mut last_of_group: BTreeMap<u32, usize> = BTreeMap::new();
    let mut members_of_group: BTreeMap<u32, Vec<&CompiledModule>> = BTreeMap::new();
    for (position, module) in project_modules.iter().enumerate() {
        let Some(group) = cycle_groups.get(&module.path).copied() else {
            continue;
        };
        first_of_group.entry(group).or_insert(position);
        last_of_group.insert(group, position);
        members_of_group.entry(group).or_default().push(module);
    }

    let mut line_origins: Vec<Option<LineOrigin>> = vec![None; count_lines(&code)];
    for (index, segment) in segments.iter().enumerate() {
        let group = project_modules
            .get(index)
            .and_then(|module| cycle_groups.get(&module.path).copied());

        // Every namespace in the group is declared before the first body runs:
        // the member that closes the cycle reads the one that opened it.
        if let Some(group) = group
            && first_of_group.get(&group) == Some(&index)
        {
            for member in members_of_group.get(&group).into_iter().flatten() {
                code.push_str("var ");
                code.push_str(&module_id(&member.path));
                code.push_str(" = {};\n");
            }
            code.push('\n');
            line_origins.resize(count_lines(&code), None);
        }

        code.push_str(&segment.text);
        line_origins.extend(segment.origins.iter().map(|origin| {
            origin.map(|line| LineOrigin {
                module: index as u32,
                line,
            })
        }));

        // The moment a cycle is complete is the moment its members can read
        // each other's named bindings.
        if let Some(group) = group
            && last_of_group.get(&group) == Some(&index)
        {
            code.push_str("__ruvyxaRebind.splice(0).forEach(function (rebind) { rebind(); });\n\n");
            line_origins.resize(count_lines(&code), None);
        }
    }

    if matches!(input.target, BundleTarget::Ssr | BundleTarget::Edge) {
        let entry_id = module_id(&PathBuf::from("ruvyxa:bundle-entry.tsx"));
        code.push_str("export const render = ");
        code.push_str(&entry_id);
        code.push_str(".render;\n");
    }
    line_origins.resize(count_lines(&code), None);

    LinkedBundle {
        code,
        modules: project_modules
            .iter()
            .map(|module| module.path.clone())
            .collect(),
        line_origins,
    }
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
    Ok(link_with_origins(modules, input, dynamic_import_files, shared_modules)?.code)
}

/// Link a route graph and report where every emitted line came from.
///
/// Segment building is the parallel half — each module's IIFE body is
/// independent, and the rewrite only references `module_id(dep)`, which is
/// deterministic. Assembly is sequential because the output order is the whole
/// point. Below a small graph size rayon costs more than it saves, so the same
/// writer runs in a plain loop; the two paths produce identical bytes because
/// they are now the same code, which they were not when the wrapper was
/// written out twice.
pub(crate) fn link_with_origins(
    modules: &[CompiledModule],
    input: &BundleInput,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
    shared_modules: &BTreeSet<PathBuf>,
) -> Result<LinkedBundle> {
    // Which modules sit in a cycle — cheap O(V+E) — and, for each of them, the
    // dependencies that close it. An import that closes a cycle reads a
    // namespace whose body has not run yet, so the binding is re-read once the
    // cycle finishes rather than copied while it is empty.
    let groups = cycle_groups(modules);
    let cyclic_deps = |module: &CompiledModule| -> BTreeSet<PathBuf> {
        let Some(group) = groups.get(&module.path) else {
            return BTreeSet::new();
        };
        module
            .deps
            .iter()
            .filter(|dep| groups.get(*dep) == Some(group))
            .cloned()
            .collect()
    };

    let project_modules = ordered_project_modules(modules);
    const PARALLEL_SEGMENT_THRESHOLD: usize = 8;

    let segments: Vec<ModuleSegment> = if project_modules.len() < PARALLEL_SEGMENT_THRESHOLD {
        project_modules
            .iter()
            .map(|module| {
                write_module_segment(
                    module,
                    dynamic_import_files,
                    &cyclic_deps(module),
                    groups.contains_key(&module.path),
                )
            })
            .collect::<Result<_>>()?
    } else {
        project_modules
            .par_iter()
            .map(|module| {
                write_module_segment(
                    module,
                    dynamic_import_files,
                    &cyclic_deps(module),
                    groups.contains_key(&module.path),
                )
            })
            .collect::<Result<_>>()?
    };

    Ok(assemble_linked(
        &project_modules,
        &segments,
        input,
        shared_modules,
        &groups,
    ))
}

/// Link project-local modules into an executable registry used by route
/// bundles. Dependency-first ordering ensures each shared module evaluates once.
pub(crate) fn link_shared_route_modules(
    modules: &[CompiledModule],
    input: &BundleInput,
) -> Result<String> {
    // The shared registry links the way a route bundle does, cycles included:
    // a group's namespaces are declared before its first body runs, and a
    // binding that closes the cycle is re-read once it finishes.
    let groups = cycle_groups(modules);
    let project_modules = ordered_project_modules(modules);
    let mut first_of_group: BTreeMap<u32, usize> = BTreeMap::new();
    let mut last_of_group: BTreeMap<u32, usize> = BTreeMap::new();
    let mut members_of_group: BTreeMap<u32, Vec<&CompiledModule>> = BTreeMap::new();
    for (position, module) in project_modules.iter().enumerate() {
        let Some(group) = groups.get(&module.path).copied() else {
            continue;
        };
        first_of_group.entry(group).or_insert(position);
        last_of_group.insert(group, position);
        members_of_group.entry(group).or_default().push(module);
    }
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

    write_cycle_prelude(&mut out, &groups);

    for (position, module) in project_modules.iter().enumerate() {
        let id = module_id(&module.path);
        let label = module.path.to_string_lossy().into_owned();
        let group = groups.get(&module.path).copied();

        if let Some(group) = group
            && first_of_group.get(&group) == Some(&position)
        {
            for member in members_of_group.get(&group).into_iter().flatten() {
                let member_id = module_id(&member.path);
                out.push_str("var ");
                out.push_str(&member_id);
                out.push_str(" = __ruvyxa_shared_modules__[\"");
                out.push_str(&member_id);
                out.push_str("\"] = {};\n");
            }
            out.push('\n');
        }

        if group.is_some() {
            out.push_str("(function() {\n");
            out.push_str("  \"use strict\";\n");
            out.push_str("  var __exports = ");
            out.push_str(&id);
            out.push_str(";\n");
        } else {
            out.push_str("var ");
            out.push_str(&id);
            out.push_str(" = __ruvyxa_shared_modules__[\"");
            out.push_str(&id);
            out.push_str("\"] = (function() {\n");
            out.push_str("  \"use strict\";\n");
            out.push_str("  var __exports = {};\n");
        }
        out.push_str("  var module = { exports: __exports };\n");
        out.push_str("  var exports = module.exports;\n");
        out.push_str(
            "  var process = globalThis.process || { env: { NODE_ENV: \"production\" } };\n",
        );
        let cyclic_deps: BTreeSet<PathBuf> = match group {
            Some(group) => module
                .deps
                .iter()
                .filter(|dep| groups.get(*dep) == Some(&group))
                .cloned()
                .collect(),
            None => BTreeSet::new(),
        };
        rewrite_module_into(
            &module.js,
            &DepIndex::new(&module.deps, &module.dependency_aliases),
            &mut out,
            &ModuleRewrite {
                dynamic_import_files: &BTreeMap::new(),
                indent: true,
                drop_external_imports: true,
                importer: &label,
                cyclic_deps: &cyclic_deps,
            },
        )?;
        if group.is_some() {
            out.push_str(
                "  if (module.exports !== __exports) Object.assign(__exports, module.exports);\n})();\n\n",
            );
        } else {
            out.push_str("  return module.exports;\n})();\n\n");
        }

        if let Some(group) = group
            && last_of_group.get(&group) == Some(&position)
        {
            out.push_str("__ruvyxaRebind.splice(0).forEach(function (rebind) { rebind(); });\n\n");
        }
    }

    let _ = input;
    Ok(out)
}

/// Names a rewritten module body binds at its own top level.
///
/// Every import has already become a `const` by the time this runs, so one walk
/// answers for declarations and imported names alike. Depth is counted over
/// [`crate::ast::masked_code`], so a brace inside a string or a comment cannot
/// close a block early and make an inner declaration look top-level.
/// Whether a module's own body awaits — as opposed to awaiting inside one of
/// its functions.
///
/// Every module is emitted as an immediately-invoked function, and `await` is
/// illegal in a synchronous one. A dependency that uses top-level await (an
/// ESM-only package initialising itself at import time, a route awaiting a
/// dynamic import) therefore produced a bundle that would not parse.
///
/// Depth-counted rather than pattern-matched: `await` inside a function body is
/// ordinary and must not count. The one shape this over-reports is a
/// brace-less async arrow (`async () => await x`), where the token sits at
/// depth zero inside a function; the cost of being wrong that way is a wrapper
/// that awaits a promise it did not need to, which changes nothing an
/// application can observe.
///
/// Kept level with `hasTopLevelAwait` in
/// `packages/ruvyxa/runtime/compiler.mjs`.
fn has_top_level_await(body: &str) -> bool {
    let masked = crate::ast::masked_code(body);
    let bytes = masked.as_bytes();
    let mut depth = 0i32;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b'a' if depth == 0 && masked[index..].starts_with("await") => {
                let before = index.checked_sub(1).map(|at| bytes[at]);
                let after = bytes.get(index + 5).copied();
                let boundary = |byte: Option<u8>| {
                    byte.is_none_or(|byte| {
                        !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'$')
                    })
                };
                if boundary(before) && boundary(after) {
                    return true;
                }
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn top_level_bound_names(body: &str) -> BTreeSet<String> {
    const KEYWORDS: [&str; 5] = ["var ", "let ", "const ", "function ", "class "];
    let masked = crate::ast::masked_code(body);
    let bytes = masked.as_bytes();
    let mut names = BTreeSet::new();
    let mut depth = 0i32;

    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            _ => {}
        }
        if depth != 0 {
            continue;
        }
        let previous = index.checked_sub(1).map(|at| bytes[at]);
        if previous.is_some_and(is_linker_identifier_byte) {
            continue;
        }
        // The walk is over bytes and the slice is over a `str`, so a position
        // inside a multi-byte character has to be skipped rather than sliced:
        // `&masked[index..]` panics there, and a module declaring `const café`
        // aborted the build with a Rust backtrace instead of any diagnostic.
        // A continuation byte cannot begin a keyword, so nothing is lost.
        if !masked.is_char_boundary(index) {
            continue;
        }
        let rest = &masked[index..];
        let Some(keyword) = KEYWORDS.iter().find(|keyword| rest.starts_with(**keyword)) else {
            continue;
        };
        let after = rest[keyword.len()..].trim_start().trim_start_matches('*');
        let name: String = after
            .trim_start()
            .chars()
            .take_while(|character| crate::ast::is_identifier_continue_char(*character))
            .collect();
        if !name.is_empty() {
            names.insert(name);
        }
    }
    names
}

/// The two helpers a bundle with an import cycle needs, and nothing when it has
/// none — an acyclic bundle keeps the bytes it always had.
///
/// `__ruvyxaRebind` holds the bindings a cyclic import could not read yet;
/// `__ruvyxaCycleTdz` is what such a binding holds until then. ESM answers a
/// read of a binding whose module has not finished with a ReferenceError, and
/// a copied `undefined` would be the same wrong value with nothing to trace it
/// back to.
fn write_cycle_prelude(out: &mut String, cycle_groups: &BTreeMap<PathBuf, u32>) {
    if cycle_groups.is_empty() {
        return;
    }
    out.push_str("var __ruvyxaRebind = [];\n");
    out.push_str(
        "var __ruvyxaCycleTdz = function (name, from) { return new Proxy(function () {}, { get: function (target, key) { if (key === Symbol.toStringTag) return \"Uninitialized\"; throw new ReferenceError(\"Cannot access '\" + name + \"' before initialization: it is imported from \" + from + \", which imports this module back, and the value is read while that cycle is still running.\"); }, apply: function () { throw new ReferenceError(\"Cannot call '\" + name + \"' before initialization (import cycle with \" + from + \").\"); }, construct: function () { throw new ReferenceError(\"Cannot construct '\" + name + \"' before initialization (import cycle with \" + from + \").\"); } }); };\n\n",
    );
}

fn write_shared_module_bindings(out: &mut String, shared_modules: &BTreeSet<PathBuf>) {
    if shared_modules.is_empty() {
        return;
    }
    out.push_str("var __ruvyxa_shared_modules__ = globalThis.__RUVYXA_SHARED_MODULES__;\n");
    for path in shared_modules {
        let id = module_id(path);
        // Presence, not truthiness. A module is allowed to export a falsy
        // value: lodash's `_WeakMap.js` is `module.exports = getNative(root,
        // "WeakMap")`, which is `undefined` wherever that native check fails.
        // Asking `if (!binding)` called that "not loaded" — the chunk had run,
        // the key was there, and the value was exactly what the module meant to
        // export. The route bundle threw while loading, so a page that rendered
        // correctly on the server went blank in the browser and blamed a loader
        // problem that did not exist.
        out.push_str("if (!__ruvyxa_shared_modules__ || !(\"");
        out.push_str(&id);
        out.push_str(
            "\" in __ruvyxa_shared_modules__)) throw new Error(\"RUV1602 shared route module was not loaded: ",
        );
        out.push_str(&id);
        out.push_str("\");\nvar ");
        out.push_str(&id);
        out.push_str(" = __ruvyxa_shared_modules__[\"");
        out.push_str(&id);
        out.push_str("\"];\n");
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

/// Rewrite all import/export statements in a module's source into `out`, and
/// report where each written line came from.
///
/// - Project-local imports → namespace variable references
/// - Exports → `__exports.name = …` assignments
/// - External imports (not in deps) → left as-is (handled by the runtime)
///
/// The returned vector has one entry per line written: `Some(n)` for a line
/// produced from line `n` of `source`, `None` for a line this function added.
/// It is counted rather than assumed, because a rewrite is not always one line
/// in and one line out — the namespace marker goes in front, deferred export
/// assignments go at the end, and a rewriter may return text containing a
/// newline of its own.
/// Everything a module rewrite needs besides the source and its dependency
/// index.
///
/// Grouped rather than passed one by one: they are read together, they are
/// decided together by the caller, and four of them are booleans and paths that
/// read identically at a call site.
struct ModuleRewrite<'a> {
    dynamic_import_files: &'a BTreeMap<PathBuf, String>,
    /// Indent the rewritten body, because it is emitted inside a wrapper.
    indent: bool,
    /// Replace an import of a module this bundle inlined with a reference to it.
    drop_external_imports: bool,
    /// The importing module, named in any error this rewrite raises.
    importer: &'a str,
    /// Dependencies that close an import cycle with this module.
    cyclic_deps: &'a BTreeSet<PathBuf>,
}

fn rewrite_module_into(
    source: &str,
    deps: &DepIndex<'_>,
    out: &mut String,
    rewrite: &ModuleRewrite<'_>,
) -> Result<Vec<Option<u32>>> {
    let ModuleRewrite {
        dynamic_import_files,
        indent,
        drop_external_imports,
        importer,
        cyclic_deps,
    } = *rewrite;
    let mut pending_exports = Vec::new();
    let mut in_block_comment = false;
    let mut in_commonjs_block_comment = false;
    let module_ast = crate::ast::parse_module(source);

    // Built separately from `out` so the whole rewritten body can be checked
    // before any of it is committed to the bundle.
    let body = &mut String::with_capacity(source.len() + 64);
    let mut origins: Vec<Option<u32>> = Vec::new();

    if declares_esm_syntax(source, &module_ast) {
        write_tracked_line(body, ESM_NAMESPACE_MARKER, indent, &mut origins, None);
    }

    for (index, (line, statement_start)) in lines_with_statement_offsets(source).enumerate() {
        let trimmed = line.trim();

        // A line whose first non-whitespace byte sits inside a string,
        // template literal, or comment is text the module means to keep, not a
        // statement to rewrite. Rewriting it edits the literal's contents.
        let normalized = if module_ast.is_code_offset(statement_start) {
            normalize_esm_statement(trimmed)
        } else {
            None
        };
        let statement = normalized.as_deref().unwrap_or(trimmed);

        let rewritten = if module_ast.is_code_offset(statement_start) {
            try_rewrite_import(statement, deps, drop_external_imports, cyclic_deps)?
                .map(Rewrite::Inline)
                .or_else(|| try_rewrite_export_statement(statement, deps))
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
        write_tracked_line(
            body,
            &commonjs_rewritten,
            indent,
            &mut origins,
            Some(index as u32),
        );
    }

    for assignment in pending_exports {
        write_tracked_line(body, &assignment, indent, &mut origins, None);
    }

    reject_surviving_esm(body, importer)?;
    out.push_str(body);
    Ok(origins)
}

/// Write one rewritten line and record which source line produced it.
///
/// `write_rewritten_line` appends `content` plus a newline, so the lines it
/// adds are one more than the newlines `content` already carries — including
/// the empty-content case, which still ends a line.
fn write_tracked_line(
    body: &mut String,
    content: &str,
    indent: bool,
    origins: &mut Vec<Option<u32>>,
    origin: Option<u32>,
) {
    let added = 1 + content.matches('\n').count();
    write_rewritten_line(body, content, indent);
    origins.resize(origins.len() + added, origin);
}

/// Fail when an ESM statement survived rewriting into the module body.
///
/// Every module is wrapped in an IIFE, where `import`/`export` are syntax
/// errors. The rewriters work a line at a time, so a module that puts more than
/// one statement on a line — which is what a minified `dist` file is —
/// keeps its `export` and the whole bundle stops parsing in the browser. That
/// used to happen with no build-time signal at all: the bad bytes were copied
/// through and only the browser complained, about a generated file.
///
/// Naming the module here turns that into an actionable build failure. It is a
/// guard, not the fix: linking minified ESM would mean the rewriters working on
/// statements rather than lines.
fn reject_surviving_esm(body: &str, module_path: &str) -> Result<()> {
    let masked = crate::ast::masked_code(body);
    let bytes = masked.as_bytes();
    for (keyword, offset) in masked
        .match_indices("export")
        .map(|(at, _)| ("export", at))
        .chain(masked.match_indices("import").map(|(at, _)| ("import", at)))
    {
        let starts_token = offset
            .checked_sub(1)
            .is_none_or(|before| !is_linker_identifier_byte(bytes[before]));
        let after = offset + keyword.len();
        let ends_token = bytes
            .get(after)
            .is_none_or(|byte| !is_linker_identifier_byte(*byte));
        if !starts_token || !ends_token {
            continue;
        }
        // A reserved word is still a legal property name, and neither position
        // can begin a statement:
        //
        //   const conditions = { import: "./index.mjs", export: "./index.js" }
        //   const entry = conditions.import
        //
        // Both are ordinary code the rewriter correctly left alone, and both
        // used to fail the build here — this guard asked only whether the word
        // appeared as a token anywhere, so it was strictly broader than the
        // rewriter it was checking. A package.json `exports` conditions object
        // written inline is exactly that shape.
        let preceding = masked[..offset]
            .bytes()
            .rev()
            .find(|byte| !byte.is_ascii_whitespace());
        let following = masked[after..]
            .bytes()
            .find(|byte| !byte.is_ascii_whitespace());
        if preceding == Some(b'.') || following == Some(b':') {
            continue;
        }
        // `import(…)` and `import.meta` are expressions and are legal here.
        if keyword == "import" {
            let next = masked[after..]
                .bytes()
                .find(|byte| !byte.is_ascii_whitespace());
            if matches!(next, Some(b'(') | Some(b'.')) {
                continue;
            }
        }
        // Say which of the two causes this is, rather than asserting the rarer
        // one. The message used to claim the module "could not be parsed for
        // re-printing" in every case; when the real reason was an unresolved
        // specifier that sent a whole investigation at the dependency's syntax,
        // which was fine, instead of at the resolver, which was not.
        let line_start = body[..offset].rfind('\n').map_or(0, |at| at + 1);
        let line_end = body[offset..]
            .find('\n')
            .map_or(body.len(), |at| offset + at);
        let hint = match split_from_specifier(body[line_start..line_end].trim()) {
            Some((_, specifier)) => format!(
                "Its specifier `{specifier}` did not resolve to a module in this bundle, \
                 so the statement was left as it was written. Check that the file it names \
                 exists and is reachable from the entry, or add the package to \
                 `build.external`."
            ),
            None => "Modules whose statements share a line are normally re-printed one \
                     statement per line before linking, so this one may have failed to \
                     parse for re-printing. Check it for syntax this build does not \
                     support, or add the package to `build.external`."
                .to_string(),
        };
        return Err(BundleError::Compiler(format!(
            "RUV1612 {module_path} still contains a top-level `{keyword}` after linking, \
             so the bundle would not parse in a browser. {hint}"
        )));
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

/// `export /*hint*/ function f()` without the comment, or `None` for a
/// statement that has none.
///
/// Only block comments directly after `import`/`export` are removed, and only
/// up to the next token: a comment anywhere else in the statement is left where
/// the author put it.
fn strip_comment_after_module_keyword(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let keyword = ["import", "export"].into_iter().find(|keyword| {
        bytes.starts_with(keyword.as_bytes())
            && bytes
                .get(keyword.len())
                .is_none_or(|byte| !is_linker_identifier_byte(*byte))
    })?;

    let rest = &line[keyword.len()..];
    let mut at = 0usize;
    let mut removed = false;
    loop {
        at += rest[at..]
            .bytes()
            .take_while(u8::is_ascii_whitespace)
            .count();
        if !rest[at..].starts_with("/*") {
            break;
        }
        let end = rest[at + 2..].find("*/")?;
        at = at + 2 + end + 2;
        removed = true;
    }

    if !removed {
        return None;
    }
    Some(format!("{keyword} {}", rest[at..].trim_start()))
}

/// Give a minified ESM statement the spacing every rewriter below expects.
///
/// The rewriters detect statements with `starts_with("export ")` /
/// `starts_with("import ")` and locate the specifier with `" from "`. That is a
/// *space* test standing in for a token-boundary test, and a minifier does not
/// emit the space: `export{a as B};` and `import{x}from"./m"` are what a
/// package's published `dist` actually contains. `compile_module` hands `.js` /
/// `.mjs` / `.cjs` through untouched, so those bytes reach the linker exactly as
/// written — and, matching nothing, were copied verbatim into the module IIFE.
/// An `export` inside a function body is a syntax error, so a single minified
/// ESM dependency made the whole bundle fail to parse in the browser, with no
/// build-time complaint.
///
/// Returns `None` when the line needs no change, so the common already-spaced
/// path allocates nothing.
fn normalize_esm_statement(line: &str) -> Option<String> {
    // A comment may sit between the keyword and what it exports. `zod` ships
    // `export /*@__NO_SIDE_EFFECTS__*/ function $constructor(…)`, and every
    // rewriter below reads the token straight after `export ` — so the
    // declaration branch was never reached, the `export` survived into the
    // module wrapper, and `RUV1612` blamed a minified dependency. The hint the
    // comment carries is a tree-shaking annotation for a bundler that is not
    // this one.
    if let Some(stripped) = strip_comment_after_module_keyword(line) {
        return Some(normalize_esm_statement(&stripped).unwrap_or(stripped));
    }

    let bytes = line.as_bytes();
    let keyword = ["import", "export"].into_iter().find(|keyword| {
        bytes.starts_with(keyword.as_bytes())
            && bytes
                .get(keyword.len())
                .is_none_or(|byte| !is_linker_identifier_byte(*byte))
    })?;

    // Masked so a `from` inside a string or comment is not treated as the
    // specifier separator. Blanking preserves byte offsets, so positions found
    // here index the original line.
    let masked = crate::ast::masked_code(line);

    let mut insertions: Vec<usize> = Vec::new();
    if bytes
        .get(keyword.len())
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        insertions.push(keyword.len());
    }

    // `import*as ns from…`: the namespace star is a token of its own, and the
    // clause parser reads `*as` as one word. Only the star that opens the
    // clause is spaced, so multiplication anywhere else in the statement is
    // untouched.
    let star = keyword.len()
        + line[keyword.len()..]
            .bytes()
            .take_while(u8::is_ascii_whitespace)
            .count();
    if bytes.get(star) == Some(&b'*')
        && bytes
            .get(star + 1)
            .is_some_and(|byte| is_linker_identifier_byte(*byte))
    {
        insertions.push(star + 1);
    }

    let mut cursor = keyword.len();
    while let Some(found) = masked[cursor..].find("from") {
        let at = cursor + found;
        cursor = at + "from".len();
        // The mask decides *where* a real `from` is; every adjacency question is
        // asked of the original bytes. A masked string is all blanks, so asking
        // the mask whether a space already separates `from` from its specifier
        // always answered yes and `from"./m"` kept its missing space.
        let starts_token = at
            .checked_sub(1)
            .is_none_or(|before| !is_linker_identifier_byte(bytes[before]));
        let ends_token = bytes
            .get(at + "from".len())
            .is_none_or(|byte| !is_linker_identifier_byte(*byte));
        if !starts_token || !ends_token {
            continue;
        }
        if at
            .checked_sub(1)
            .is_some_and(|before| !bytes[before].is_ascii_whitespace())
        {
            insertions.push(at);
        }
        if bytes
            .get(at + "from".len())
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            insertions.push(at + "from".len());
        }
    }

    if insertions.is_empty() {
        return None;
    }
    insertions.sort_unstable();
    // `export*from"./m"` reaches the same gap from the star rule and the `from`
    // rule; one space is wanted, not two.
    insertions.dedup();
    let mut out = String::with_capacity(line.len() + insertions.len());
    let mut previous = 0;
    for at in insertions {
        out.push_str(&line[previous..at]);
        out.push(' ');
        previous = at;
    }
    out.push_str(&line[previous..]);
    Some(out)
}

fn is_linker_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

/// Try to rewrite an import statement. Returns None if the line is not an import.
fn try_rewrite_import(
    line: &str,
    deps: &DepIndex<'_>,
    drop_external_imports: bool,
    cyclic_deps: &BTreeSet<PathBuf>,
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

    Ok(Some(rewrite_import_clause(
        clause,
        &dep_id,
        cyclic_deps.contains(dep_path),
    )?))
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
/// Packages whose import is a declaration for the boundary checker and nothing
/// else, so no emitted bundle may import them.
///
/// A client bundle never reaches this: importing `server-only` there is
/// RUV1007 and the build fails before output exists. Replayed against
/// `tests/fixtures/module-lane-conformance.json` alongside
/// `packages/ruvyxa/runtime/compiler.mjs`, which drops the same two.
pub(crate) fn is_marker_package(specifier: &str) -> bool {
    matches!(specifier, "server-only" | "client-only")
}

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

            // A marker package is a declaration, not a dependency.
            //
            // `server-only` and `client-only` ship no runtime behaviour: the
            // whole point of importing one is the boundary check, which has
            // already run by the time anything is emitted. Carrying the import
            // through meant a deployed function directory — which has no
            // `node_modules` of its own — failed to start with
            // ERR_MODULE_NOT_FOUND for a package whose only job was to not be
            // there. See `markerPackages` in
            // tests/fixtures/module-lane-conformance.json.
            if is_marker_package(&specifier) {
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
    //
    // Asked of the code, not of the text: a quoted ` from ` used to send an
    // ordinary `export const` down this branch, which returns `None` on every
    // path that is not a resolvable re-export and so never falls through to the
    // declaration branch below. See `code_from_keyword`.
    // `split_from_specifier` decides, rather than the keyword search alone.
    // `from` is a keyword only where a specifier follows it, and it is also an
    // ordinary binding name: `export { source as from }` renames a binding *to*
    // `from`. Entering this branch on the keyword and bailing out of it on the
    // missing specifier meant the declaration branch below was never reached,
    // and the export was dropped with no diagnostic — the importer saw
    // `undefined`.
    if let Some((before_from, specifier)) = split_from_specifier(line) {
        let dep_path = deps.resolve(&specifier)?;
        let dep_id = module_id(dep_path);

        let clause = before_from.strip_prefix("export ")?.trim();

        // `export * from "./mod"` → `Object.assign(__exports, __ruv_xxx__)`
        if clause == "*" {
            return Some(Rewrite::Inline(format!(
                "Object.assign(__exports, {dep_id});"
            )));
        }

        // `export * as ns from "./mod"` names the namespace object. Read as a
        // clause it matched nothing, so the `export` survived the link and
        // `RUV1612` blamed the dependency — `zod` re-exports two of its own
        // modules exactly this way.
        if let Some(alias) = clause
            .strip_prefix('*')
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix("as "))
            .map(str::trim)
            && is_identifier(alias)
        {
            return Some(Rewrite::Inline(format!("__exports.{alias} = {dep_id};")));
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

    // `export function name(…)` / `export class name` / `export function* gen(…)`
    //
    // Matched against the same forms `extract_declaration_name` decodes, rather
    // than against a list of literal prefixes. The list had a trailing space in
    // every entry, so a generator — `export function* stream()`, where `*`
    // follows the keyword with no space — matched nothing and fell through to
    // the end of this function. `extract_declaration_name` had known about
    // `function* ` all along; only the dispatcher above it did not, so the
    // `export` survived the link and `RUV1612` failed the build.
    if is_exported_declaration(line) {
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
    //
    // Unreachable with a `from` today, because the branch above claims those
    // first; asked of the code anyway so the two questions cannot answer
    // differently if that order ever changes.
    // The specifier decides here too: a bare list may legally alias a binding
    // to `from`, which the keyword search alone reads as a re-export.
    if line.starts_with("export {") && split_from_specifier(line).is_none() {
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
fn rewrite_import_clause(clause: &str, dep_id: &str, cyclic: bool) -> Result<String> {
    // An import that closes a cycle reads a namespace whose body has not run
    // yet, so a copied binding would hold `undefined` for the life of the
    // bundle. It is re-read once the cycle finishes instead, and refuses to be
    // used before that. A namespace import needs none of this: it holds the
    // object itself, which is published before the group runs.
    let bind = |local: &str, expression: String, namespace: bool| {
        if !cyclic || namespace {
            return format!("const {local} = {expression};");
        }
        format!(
            "let {local} = {expression} ?? __ruvyxaCycleTdz(\"{local}\", \"{dep_id}\"); __ruvyxaRebind.push(function () {{ {local} = {expression}; }});"
        )
    };

    let bindings = parse_import_clause(clause)?
        .into_iter()
        .map(|(local, source)| match source {
            ImportBinding::Default => bind(&local, interop_default(dep_id), false),
            ImportBinding::Named(original) => bind(&local, format!("{dep_id}.{original}"), false),
            ImportBinding::Namespace => bind(&local, dep_id.to_string(), true),
        })
        .collect::<Vec<_>>();

    Ok(bindings.join(" "))
}

fn is_identifier(value: &str) -> bool {
    crate::ast::is_identifier_name(value)
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
/// Byte offset of the ESM `from` keyword on this line, if it has one.
///
/// Searched over [`crate::ast::masked_code`] rather than the raw line, because
/// ` from ` is ordinary English and appears in strings people write:
/// `export const note = "copied from here"` used to be read as a re-export.
/// `try_rewrite_export_statement` then took the re-export branch, found no
/// specifier, and returned `None` without ever reaching the declaration branch
/// below it — so the `export` survived the link and `RUV1612` failed the whole
/// build, with a message about minified dependencies that named nothing the
/// author had written.
///
/// Masking preserves byte offsets, so the returned index addresses the original
/// line.
fn code_from_keyword(line: &str) -> Option<usize> {
    crate::ast::masked_code(line).rfind(" from ")
}

fn split_from_specifier(line: &str) -> Option<(String, String)> {
    let from_idx = code_from_keyword(line)?;
    let before = line[..from_idx].to_string();
    let after = line[from_idx + 6..].trim();

    // Extract quoted specifier.
    let specifier = extract_quoted_string(after)?;
    // `from` is a keyword only where a specifier follows it. It is also an
    // ordinary binding name: `export { source as from }` renames a binding *to*
    // `from`, and reading that as a re-export claimed a line that names no
    // module — the declaration branch below was never reached, and the export
    // was dropped with no diagnostic.
    if before.trim_end().ends_with(" as") || before.trim_end().ends_with(',') {
        return None;
    }
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
        // Drop every leading `./` and `../` before matching. A dependency path
        // is absolute and holds no `..`, so a specifier that climbs out of its
        // own directory matched nothing at all: `../locale/en-US.js` failed to
        // resolve while `./en-US.js` beside it worked. The re-export branch
        // returns `None` when this does, which leaves the statement in the
        // bundle for `RUV1612` to blame on the dependency's syntax. What the
        // climb means has already been decided — these are this module's own
        // resolved dependencies — so only the tail is still in question.
        let mut relative = normalized;
        while let Some(rest) = relative
            .strip_prefix("./")
            .or_else(|| relative.strip_prefix("../"))
        {
            relative = rest;
        }
        let direct_suffix = relative;
        let spec_file = relative.rsplit('/').next().unwrap_or(relative);
        let spec_dir = relative.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");

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
/// Whether the line is `export` followed by a named function or class.
///
/// Kept beside [`extract_declaration_name`] and accepting exactly what it
/// decodes: a generator's `*` binds to the keyword with no space, so a prefix
/// list written with trailing spaces silently excluded every generator export.
fn is_exported_declaration(line: &str) -> bool {
    let Some(decl) = line.strip_prefix("export ") else {
        return false;
    };
    let decl = decl
        .trim_start()
        .strip_prefix("async ")
        .unwrap_or(decl.trim_start());
    ["function", "class"].iter().any(|keyword| {
        decl.strip_prefix(keyword)
            .is_some_and(|rest| rest.starts_with([' ', '*']))
    })
}

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
        .take_while(|c| crate::ast::is_identifier_continue_char(*c))
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
        .take_while(|c| crate::ast::is_identifier_continue_char(*c))
        .collect();

    if name.is_empty() { None } else { Some(name) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minifier drops the space the rewriters used to require. These forms
    /// must reach the same rewrite as their spaced equivalents, not be copied
    /// verbatim into a module IIFE where `export` does not parse.
    #[test]
    fn minified_esm_statements_are_normalized_before_rewriting() {
        for (minified, spaced) in [
            ("export{a as Mini};", "export {a as Mini};"),
            ("export*from\"./m\";", "export * from \"./m\";"),
            ("export{a}from\"./m\";", "export {a} from \"./m\";"),
            ("import{x}from\"./m\";", "import {x} from \"./m\";"),
            ("import*as ns from\"./m\";", "import * as ns from \"./m\";"),
        ] {
            assert_eq!(
                normalize_esm_statement(minified).as_deref(),
                Some(spaced),
                "{minified}"
            );
        }

        // Already-spaced statements are left alone, so the common path does no
        // work and no existing output shifts.
        for spaced in [
            "export { a as Mini };",
            "import { x } from \"./m\";",
            "export default function Page() {}",
        ] {
            assert_eq!(normalize_esm_statement(spaced), None, "{spaced}");
        }

        // A `from` inside a string is not the specifier separator.
        assert_eq!(
            normalize_esm_statement("export const note = \"copied from here\";"),
            None
        );
    }

    /// Re-spacing is only worth anything if the result then rewrites. This walks
    /// the same two steps `rewrite_module_into` does.
    #[test]
    fn a_minified_named_export_rewrites_into_exports_assignments() {
        let normalized =
            normalize_esm_statement("export{a as Mini};").expect("the minified form needs spacing");
        let rewritten = try_rewrite_export(&normalized, &[]);
        assert_eq!(rewritten.as_deref(), Some("__exports.Mini = a;"));
    }

    /// The guard is what keeps a module the line-based rewriters cannot handle
    /// out of the bundle. Several statements on one line — a minified `dist`
    /// build — leave the `export` in place, and an `export` inside the module
    /// IIFE is a syntax error that used to reach the browser unannounced.
    #[test]
    fn a_surviving_top_level_export_fails_the_link_and_names_the_module() {
        let error = reject_surviving_esm("const a=1;export{a as Mini};", "node_modules/m/index.js")
            .expect_err("a surviving export must fail the link");
        let message = error.to_string();
        assert!(message.contains("RUV1612"), "{message}");
        assert!(message.contains("node_modules/m/index.js"), "{message}");
    }

    /// The guard must not fire on the expression forms of `import`, on rewritten
    /// bodies, or on the words appearing in text.
    #[test]
    fn the_surviving_esm_guard_accepts_legal_module_bodies() {
        for body in [
            "const m = await import(\"./lazy.js\");",
            "const url = import.meta.url;",
            "__exports.Mini = a;",
            "const exported = 1; const important = 2;",
            "// export { a } from \"./m\"",
            "const doc = 'export { a }';",
        ] {
            assert!(
                reject_surviving_esm(body, "app/page.tsx").is_ok(),
                "must not fire: {body}"
            );
        }
    }

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
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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
        crate::minifier::minify(&linked, BundleTarget::Client, crate::EsTarget::EsNext)
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
        let result = rewrite_import_clause("React", dep_id, false).unwrap();
        assert_eq!(
            result,
            format!("const React = {};", interop_default(dep_id))
        );
    }

    #[test]
    fn rewrite_named_imports() {
        let dep_id = "__ruv_abc__";
        let result = rewrite_import_clause("{ useState, useEffect }", dep_id, false).unwrap();
        assert!(result.contains("const useState = __ruv_abc__.useState;"));
        assert!(result.contains("const useEffect = __ruv_abc__.useEffect;"));
    }

    #[test]
    fn rewrite_named_import_with_alias() {
        let dep_id = "__ruv_abc__";
        let result = rewrite_import_clause("{ foo as bar }", dep_id, false).unwrap();
        assert_eq!(result, "const bar = __ruv_abc__.foo;");
    }

    #[test]
    fn rewrite_namespace_import() {
        let dep_id = "__ruv_abc__";
        let result = rewrite_import_clause("* as utils", dep_id, false).unwrap();
        assert_eq!(result, "const utils = __ruv_abc__;");
    }

    #[test]
    fn rewrite_default_plus_named() {
        let dep_id = "__ruv_abc__";
        let result = rewrite_import_clause("React, { useState }", dep_id, false).unwrap();
        assert!(result.contains(&format!("const React = {};", interop_default(dep_id))));
        assert!(result.contains("const useState = __ruv_abc__.useState;"));
    }

    #[test]
    fn rewrite_default_plus_namespace() {
        let result =
            rewrite_import_clause("React, * as ReactNamespace", "__ruv_abc__", false).unwrap();
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
        let error = rewrite_import_clause("React, invalid", "__ruv_abc__", false).unwrap_err();
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

    /// Marker packages, held to the table the Node compiler replays.
    ///
    /// A marker is a declaration for the boundary checker and nothing else, so
    /// no emitted bundle may import one. Both linkers used to carry it through,
    /// and a deployed function directory — which has no `node_modules` — then
    /// failed to start with ERR_MODULE_NOT_FOUND for a package whose only job
    /// was to not be there.
    #[test]
    fn marker_packages_match_the_shared_conformance_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/module-lane-conformance.json"
        ))
        .unwrap();
        let markers = fixture["markerPackages"].as_array().unwrap();
        assert!(!markers.is_empty(), "the fixture must name the markers");
        for marker in markers {
            assert!(
                is_marker_package(marker.as_str().unwrap()),
                "{marker} must be dropped from emitted output"
            );
        }
        assert!(!is_marker_package("react"), "an ordinary package is a dep");
    }

    /// A server bundle that declares `server-only` emits no import of it.
    ///
    /// The end the deployment sees: the module the boundary marker was written
    /// in still compiles, and the bundle it lands in imports nothing that has
    /// to exist at run time.
    #[test]
    fn a_server_bundle_drops_the_marker_import_it_declares() {
        let module = CompiledModule::new(
            PathBuf::from("C:/project/app/page.tsx"),
            "import 'server-only';
import { readFile } from 'node:fs';
export const q = readFile;
"
            .to_string(),
            Vec::new(),
            BTreeMap::new(),
            false,
            false,
        );
        let imports = collect_external_imports(&[&module], BundleTarget::Ssr);
        assert!(
            !imports.iter().any(|line| line.contains("server-only")),
            "a marker must not reach the bundle: {imports:?}"
        );
        assert!(
            imports.iter().any(|line| line.contains("node:fs")),
            "an ordinary external import still has to be emitted: {imports:?}"
        );
    }

    #[test]
    fn side_effect_import_commented() {
        let result = try_rewrite_import(
            "import \"./styles.css\"",
            &DepIndex::without_aliases(&[]),
            false,
            &BTreeSet::new(),
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
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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

    /// A declared name survives its own combining marks.
    ///
    /// `char::is_alphanumeric` is false for a Thai tone mark, a Devanagari
    /// matra, an Arabic harakat, and a Vietnamese diacritic, so every name here
    /// was captured only as far as its first one — `หน่วย` became `หน`. The
    /// declaration kept its real name while references to it were rewritten to
    /// the truncated one, so the browser threw `ReferenceError: หน is not
    /// defined` and hydration died on a page that had rendered correctly.
    /// Nothing in the build said a word.
    #[test]
    fn a_declared_name_is_not_cut_at_a_combining_mark() {
        let names = top_level_bound_names(
            "const \u{e2b}\u{e19}\u{e48}\u{e27}\u{e22} = 1;\n\
             let \u{939}\u{93f}\u{928}\u{94d}\u{926}\u{940} = 2;\n\
             function ti\u{1ebf}ng() {}\n\
             class \u{639}\u{64e}\u{631}\u{628}\u{64a} {}\n\
             var plain = 3;\n",
        );

        assert!(
            names.contains("\u{e2b}\u{e19}\u{e48}\u{e27}\u{e22}"),
            "Thai: {names:?}"
        );
        assert!(
            names.contains("\u{939}\u{93f}\u{928}\u{94d}\u{926}\u{940}"),
            "Devanagari: {names:?}"
        );
        assert!(names.contains("ti\u{1ebf}ng"), "Vietnamese: {names:?}");
        assert!(
            names.contains("\u{639}\u{64e}\u{631}\u{628}\u{64a}"),
            "Arabic: {names:?}"
        );
        assert!(names.contains("plain"), "ASCII still works: {names:?}");
        // The truncations this guards against must not appear as names of
        // their own — that is what made the failure look like a missing export.
        assert!(
            !names.contains("\u{e2b}\u{e19}"),
            "truncated Thai present: {names:?}"
        );
        assert!(
            !names.contains("ti"),
            "truncated Vietnamese present: {names:?}"
        );
    }

    /// A specifier that climbs out of its own directory still names a dep.
    ///
    /// `./foo.js` resolved because the leading `./` was stripped before the
    /// suffix match; `../locale/en-US.js` was compared verbatim, and no real
    /// path contains `..`, so it matched nothing. The re-export branch then
    /// returned `None`, the statement survived the link, and `RUV1612` blamed
    /// the dependency for syntax the build "does not support" — the actual
    /// cause being that its neighbour was never found. `date-fns` ships
    /// exactly this line, and so does most of npm.
    #[test]
    fn a_specifier_that_climbs_a_directory_resolves() {
        let deps = vec![
            PathBuf::from("/project/node_modules/date-fns/locale/en-US.js"),
            PathBuf::from("/project/node_modules/date-fns/_lib/other.js"),
        ];
        let index = DepIndex::without_aliases(&deps);

        assert_eq!(index.resolve("../locale/en-US.js"), Some(&deps[0]));
        assert_eq!(index.resolve("../locale/en-US"), Some(&deps[0]));
        assert_eq!(index.resolve("./other.js"), Some(&deps[1]));
        // Two levels up, and a specifier naming nothing in the list.
        assert_eq!(
            index.resolve("../../date-fns/locale/en-US.js"),
            Some(&deps[0])
        );
        assert_eq!(index.resolve("../locale/fr.js"), None);
    }

    fn link_unresolved_import(target: BundleTarget, source: &str) -> String {
        let entry = PathBuf::from("/app/page.tsx");
        let input = BundleInput {
            entry: entry.clone(),
            project_root: PathBuf::from("/app"),
            app_dir: PathBuf::from("/app/app"),
            layouts: Vec::new(),
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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
            templates: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
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

    /// A cycle is grouped, not refused.
    ///
    /// The linker used to answer a cycle with `RUV1803 circular dependency
    /// detected`, which made every package carrying one impossible to bundle;
    /// `zod` carries one between two of its own files. Both halves of the group
    /// have to be found, because the emitted form depends on it: their exports
    /// objects are published before their bodies run.
    #[test]
    fn a_cycle_is_grouped_rather_than_refused() {
        let a = PathBuf::from("/app/a.ts");
        let b = PathBuf::from("/app/b.ts");

        let modules = vec![
            fixture(a.clone(), "import B from './b';", vec![b.clone()]),
            fixture(b.clone(), "import A from './a';", vec![a.clone()]),
        ];

        let groups = cycle_groups(&modules);
        assert_eq!(groups.len(), 2, "both modules are in the cycle: {groups:?}");
        assert_eq!(
            groups.get(&a),
            groups.get(&b),
            "the two sides of one cycle share a group"
        );
    }

    #[test]
    fn a_diamond_is_not_a_cycle() {
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

        assert!(
            cycle_groups(&modules).is_empty(),
            "a diamond shares a dependency; it does not close a cycle"
        );
    }

    /// Link a source and prove the result is still JavaScript.
    ///
    /// The linker rewrites line by line over masked code, so its failure mode is
    /// output that no longer parses — an Oxc diagnostic naming linked bytes the
    /// author never wrote. Every regression recorded in this file was found that
    /// way. Compiling the output is what turns "the rewrite looked right" into
    /// "the browser can run it".
    fn link_and_parse(source: &str) -> String {
        let entry = PathBuf::from("/p/a.tsx");
        let linked = link(
            &[fixture(entry.clone(), source, Vec::new())],
            &client_input(entry),
        )
        .unwrap_or_else(|error| panic!("link rejected the source: {error}\n---\n{source}"));
        crate::compiler::transform(&linked, false)
            .unwrap_or_else(|error| panic!("linked output does not parse: {error}\n---\n{linked}"));
        linked
    }

    /// Every dependency shape in the shared syntax table links and parses.
    ///
    /// `tests/packages/ruvyxa/module-syntax.test.mjs` runs the same table
    /// through the JavaScript linker and checks what each case evaluates to.
    /// This half cannot execute the result, so it asks the question this linker
    /// can answer: does the rewrite leave anything behind?
    ///
    /// That is not a lesser question. Two of the four defects the table found on
    /// its first run were exactly this — an `export` copied through verbatim,
    /// and a decorator the stripper skipped — and both showed up here as output
    /// that would not parse.
    ///
    /// TypeScript-only shapes are skipped: they arrive at this linker already
    /// transformed, so replaying their source here would test the wrong stage.
    #[test]
    fn dependency_shapes_in_the_shared_table_link_and_parse() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/module-syntax-conformance.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let fixture: serde_json::Value =
            serde_json::from_str(&source).expect("the syntax fixture parses");
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(!cases.is_empty(), "the fixture must carry cases");

        let mut checked = 0;
        for case in cases {
            let entry = case["entry"].as_str().expect("entry");
            if entry.contains("./dep.ts") {
                continue;
            }
            let dependency = case["dependency"].as_str().expect("dependency");
            let name = case["name"].as_str().unwrap_or_default();
            let why = case["why"].as_str().unwrap_or_default();
            // Through the same preparation the pipeline does. A module whose
            // ESM statements share a line, or whose clause spans several, is
            // re-printed one statement per line before it reaches the linker;
            // linking the raw source instead would test a stage that never runs
            // on its own.
            let prepared = crate::compiler::prepared_for_linking(dependency);
            let linked = link_and_parse(&prepared);
            assert!(
                !linked.contains(
                    "
export "
                ),
                "{name}: an export survived the link — {why}
{linked}"
            );
            checked += 1;
        }
        assert!(checked > 20, "most of the table should reach this linker");
    }

    /// A cycle links, and the emitted form is the one that can run.
    ///
    /// Three things have to be true at once and none of them is visible in a
    /// text match: both namespaces are declared before either body runs, the
    /// binding that closes the cycle is re-read rather than copied out of an
    /// empty object, and the result parses. The linker used to refuse the graph
    /// outright, which took every package with an internal cycle — `zod` among
    /// them — out of reach of a browser bundle.
    #[test]
    fn a_cycle_links_with_late_bound_bindings() {
        let first = PathBuf::from("/p/first.ts");
        let second = PathBuf::from("/p/second.ts");
        let modules = vec![
            fixture(
                first.clone(),
                "import { second } from './second'\nexport const firstValue = 'first'\nexport function useIt() { return second() }\n",
                vec![second.clone()],
            ),
            fixture(
                second.clone(),
                "import { firstValue } from './first'\nexport function second() { return firstValue }\n",
                vec![first.clone()],
            ),
        ];

        let linked = link(&modules, &client_input(first.clone()))
            .unwrap_or_else(|error| panic!("link refused a cycle: {error}"));
        crate::compiler::transform(&linked, false)
            .unwrap_or_else(|error| panic!("linked output does not parse: {error}\n{linked}"));

        let first_id = module_id(&first);
        let second_id = module_id(&second);
        assert!(
            linked.contains(&format!("var {first_id} = {{}};"))
                && linked.contains(&format!("var {second_id} = {{}};")),
            "both namespaces are declared before the group runs:\n{linked}"
        );
        assert!(
            linked.contains("__ruvyxaRebind.push("),
            "the binding that closes the cycle is re-read:\n{linked}"
        );
        assert!(
            linked.contains("__ruvyxaRebind.splice(0)"),
            "the cycle flushes its re-reads when it completes:\n{linked}"
        );
    }

    /// An acyclic graph keeps the bytes it always had.
    ///
    /// The cycle support is invisible unless a cycle is present: no helper
    /// declarations, no `let` bindings, no flush. Reproducible output and every
    /// content-addressed cache downstream depend on that.
    #[test]
    fn an_acyclic_graph_is_untouched_by_cycle_support() {
        let entry = PathBuf::from("/p/a.ts");
        let dep = PathBuf::from("/p/b.ts");
        let modules = vec![
            fixture(
                entry.clone(),
                "import { value } from './b'\nexport const doubled = value * 2\n",
                vec![dep.clone()],
            ),
            fixture(dep.clone(), "export const value = 21\n", Vec::new()),
        ];

        let linked = link(&modules, &client_input(entry)).unwrap();
        assert!(!linked.contains("__ruvyxaRebind"), "{linked}");
        assert!(!linked.contains("__ruvyxaCycleTdz"), "{linked}");
        assert!(linked.contains("const value = "), "{linked}");
    }

    /// `from` is ordinary English, and a quoted one is not a re-export.
    ///
    /// `export const note = "copied from here"` failed the whole build. The
    /// re-export branch was chosen by `line.contains(" from ")` over the raw
    /// line, and every path out of that branch that is not a resolvable
    /// re-export returns `None` — so the declaration branch below it was never
    /// reached, the `export` survived the link, and `RUV1612` reported a
    /// minified-dependency problem that named nothing the author wrote. The
    /// question is now asked of `masked_code`, where a string holds no keywords.
    #[test]
    fn a_quoted_from_does_not_turn_a_declaration_into_a_re_export() {
        for source in [
            "export const note = \"copied from here\"\n",
            "export const note = 'copied from here'\n",
            "export const snippet = `import { readFile } from \"node:fs/promises\"`\n",
            "export function label() {\n  return \"read from disk\"\n}\n",
            "export class Reader {\n  origin = \"loaded from cache\"\n}\n",
            "export let mutable = \"switched from A\"\n",
            "export const help = \"pick from: a, b\" // from the docs\n",
        ] {
            let linked = link_and_parse(source);
            assert!(
                !linked.contains("\nexport "),
                "an export survived the link:\n{linked}"
            );
            assert!(
                linked.contains("__exports."),
                "the declaration was never published:\n{linked}"
            );
        }
    }

    /// `from` is also a perfectly ordinary binding name.
    ///
    /// Masking settled the case where `from` sits inside a string. It cannot
    /// settle this one: `export { source as from }` renames a binding *to*
    /// `from`, so the keyword search finds real code and the re-export branch
    /// claims a line that names no module. The JavaScript linker dropped the
    /// export silently and the importer saw `undefined`.
    #[test]
    fn an_export_aliased_to_from_is_not_a_re_export() {
        let linked = link_and_parse(
            "const source = 1
export { source as from }
",
        );
        assert!(
            !linked.contains(
                "
export "
            ),
            "an export survived the link:
{linked}"
        );
        assert!(
            linked.contains("__exports.from = source;"),
            "the alias was never published:
{linked}"
        );
    }

    /// A real re-export still resolves, so the fix above narrowed nothing.
    #[test]
    fn a_real_re_export_still_resolves_through_the_masked_check() {
        let entry = PathBuf::from("/p/a.tsx");
        let dep = PathBuf::from("/p/m.ts");
        let modules = [
            fixture(dep.clone(), "export const a = 1\n", Vec::new()),
            fixture(
                entry.clone(),
                "export { a } from \"./m\"\nexport * from \"./m\"\n",
                vec![dep.clone()],
            ),
        ];
        let linked = link(&modules, &client_input(entry)).unwrap();

        assert!(
            linked.contains(&format!("__exports.a = {}.a;", module_id(&dep))),
            "{linked}"
        );
        assert!(
            linked.contains(&format!("Object.assign(__exports, {});", module_id(&dep))),
            "{linked}"
        );
    }

    /// Constructs that have broken a line-based rewriter before, or would.
    ///
    /// The linker asks `ModuleAst::is_code_offset` before touching a line, so
    /// text that merely reads like a statement has to survive untouched while
    /// the statement beside it is still rewritten. Each case is one way a line
    /// can lie about what it is.
    #[test]
    fn adversarial_module_shapes_survive_linking_as_parsable_javascript() {
        for (name, source) in [
            (
                "import-line inside a template literal",
                "export const doc = `\nimport { thing } from \"pkg\"\nexport default thing\n`\nexport const value = 1\n",
            ),
            (
                "export-line inside a block comment",
                "/*\nexport default nothing\nimport \"pkg\"\n*/\nexport const value = 1\n",
            ),
            (
                "regular expression holding a quote and a slash",
                "const pattern = /[\"'\\/]/g\nexport const value = pattern.source\n",
            ),
            (
                "regular expression that looks like a division",
                "const a = 1\nconst b = 2\nconst quotient = a / b / 2\nexport const value = quotient\n",
            ),
            (
                "multi-line default export of an array",
                "export default [\n  1,\n  2,\n]\n",
            ),
            (
                "multi-line default export of a class",
                "export default class Page {\n  render() {\n    return null\n  }\n}\n",
            ),
            (
                "declaration whose value spans lines and holds a brace",
                "export const config = {\n  pattern: \"}\",\n  nested: { deep: `}` },\n}\n",
            ),
            (
                "template literal holding a dynamic import",
                "export const doc = `await import(\"./other.js\")`\nexport const value = 1\n",
            ),
            (
                "template literal holding a require call",
                "export const doc = `require(\"./other.js\")`\nexport const value = 1\n",
            ),
            (
                "nested template interpolation carrying a backtick",
                "export const doc = `outer ${`inner ${1 + 1}`} end`\n",
            ),
            (
                "string holding the sequence that ends a comment",
                "export const doc = \"*/ export default 1\"\nexport const value = 1\n",
            ),
            (
                "export keyword appearing as a property name",
                "const registry = { export: 1, import: 2 }\nexport const value = registry.export\n",
            ),
            (
                "line comment trailing a rewritten export",
                "export const value = 1 // export default 2\n",
            ),
            (
                "async generator yielding text that reads like a statement",
                "export async function* stream() {\n  yield `export const x = 1`\n}\n",
            ),
        ] {
            let linked = link_and_parse(source);
            assert!(
                !linked.contains("__ruvyxaMissingImport__"),
                "{name}: text was mistaken for an import\n{linked}"
            );
        }
    }

    /// Text that reads like a statement must not become a dependency edge.
    ///
    /// The demo's own todos page carried a `<pre>` code sample; the import line
    /// inside it was deleted out of the sample and its quoted package hoisted
    /// into the bundle as a real dependency. Parsing alone would not have caught
    /// that — the output still parsed, it just no longer said what the author
    /// wrote and pulled in a package the project never installed.
    #[test]
    fn a_statement_quoted_inside_text_is_neither_rewritten_nor_hoisted() {
        let sample = "import { readFile } from \"node:fs/promises\"";
        let linked = link_and_parse(&format!(
            "export const snippet = `{sample}`\nexport const value = 1\n"
        ));

        assert!(
            linked.contains(sample),
            "the quoted sample was rewritten out of the template literal:\n{linked}"
        );
        assert!(
            !linked.contains("\nimport { readFile } from \"node:fs/promises\""),
            "the quoted sample was hoisted as a real import:\n{linked}"
        );
    }
}
