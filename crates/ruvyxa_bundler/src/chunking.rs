//! Chunk planning and rendering for dynamic imports.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::ast::{self, ImportKind};
use crate::compiler::CompiledModule;
use crate::{
    BundleInput, DynamicImportChunk, OutputChunk, OutputChunkKind, Result, linker, minifier,
};

/// Plan deterministic filenames for every project-local dynamic-import root.
///
/// The graph fingerprint intentionally includes every project module. This makes chunk references
/// change together when a nested dynamic dependency changes, avoiding a stale parent chunk pointing
/// at an obsolete child filename.
pub(crate) fn plan_dynamic_chunk_files(
    compiled: &[CompiledModule],
    entry: &Path,
) -> BTreeMap<PathBuf, String> {
    let module_map = project_module_map(compiled);
    let dynamic_roots = dynamic_roots(compiled, &module_map);
    let graph_fingerprint = graph_fingerprint(compiled);
    let mut entry_modules = BTreeSet::new();
    collect_static_transitive_modules(entry, &module_map, &mut entry_modules);

    // Chunks cannot safely share our closure-scoped module namespaces.  Splitting roots whose
    // static closures overlap would execute a shared module once in each output file.  Keep every
    // overlapping root in the entry instead; this preserves evaluation semantics until a shared
    // chunk runtime exists.
    let closures = dynamic_roots
        .iter()
        .map(|root| {
            let mut closure = BTreeSet::new();
            collect_static_transitive_modules(root, &module_map, &mut closure);
            (root.clone(), closure)
        })
        .collect::<Vec<_>>();
    // A root is split only when its closure is disjoint from the entry and every other dynamic
    // closure. Roots with any overlap are linked into the entry by `entry_modules` below.
    let split_roots = closures
        .iter()
        .filter(|(root, closure)| {
            closure.is_disjoint(&entry_modules)
                && closures.iter().all(|(other_root, other_closure)| {
                    root == other_root || closure.is_disjoint(other_closure)
                })
        })
        .map(|(root, _)| root.clone())
        .collect::<BTreeSet<_>>();

    split_roots
        .into_iter()
        .map(|root| {
            let fingerprint =
                blake3::hash(format!("{graph_fingerprint}\0{}", root.display()).as_bytes())
                    .to_hex();
            (root, format!("chunk.{}.js", &fingerprint[..16]))
        })
        .collect()
}

/// Return the modules that must be evaluated with the entry bundle.
///
/// Only static, re-export, side-effect, and CommonJS require edges are followed. Dynamic edges are
/// intentionally excluded so their code is evaluated only when its runtime import executes.
pub(crate) fn static_entry_modules(
    compiled: &[CompiledModule],
    entry: &Path,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
) -> Vec<CompiledModule> {
    let module_map = project_module_map(compiled);
    let mut selected = BTreeSet::new();
    collect_static_transitive_modules(entry, &module_map, &mut selected);

    // Any dynamic root that was not emitted as an isolated chunk must be present in the entry so
    // its rewritten `import()` can resolve to the existing namespace without duplicating it.
    for root in dynamic_roots(compiled, &module_map) {
        if !dynamic_import_files.contains_key(&root) {
            collect_static_transitive_modules(&root, &module_map, &mut selected);
        }
    }

    compiled
        .iter()
        .filter(|module| selected.contains(&module.path))
        .cloned()
        .collect()
}

/// Build output chunks for project-local dynamic import split points.
pub(crate) fn build_dynamic_output_chunks(
    compiled: &[CompiledModule],
    input: &BundleInput,
    dynamic_import_files: &BTreeMap<PathBuf, String>,
) -> Result<Vec<OutputChunk>> {
    let module_map = project_module_map(compiled);
    let mut chunks = Vec::with_capacity(dynamic_import_files.len());

    for (root, file_name) in dynamic_import_files {
        let mut selected = BTreeSet::new();
        collect_static_transitive_modules(root, &module_map, &mut selected);
        let modules = compiled
            .iter()
            .filter(|module| selected.contains(&module.path))
            .cloned()
            .collect::<Vec<_>>();

        if modules.is_empty() {
            continue;
        }

        let mut linked =
            linker::link_parallel_with_dynamic_imports(&modules, input, dynamic_import_files)?;
        linked.push_str("export default ");
        linked.push_str(&linker::module_id(root));
        linked.push_str(";\n");

        let code = if input.options.minify {
            minifier::minify_with_options(&linked, input.target, false)?
        } else {
            linked
        };
        let modules = modules
            .iter()
            .map(|module| module.path.display().to_string().replace('\\', "/"))
            .collect::<Vec<_>>();

        chunks.push(OutputChunk {
            file_name: file_name.clone(),
            code,
            modules,
            kind: OutputChunkKind::DynamicImport,
        });
    }

    Ok(chunks)
}

pub(crate) fn dynamic_import_chunks(
    compiled: &[CompiledModule],
    dynamic_import_files: &BTreeMap<PathBuf, String>,
) -> Vec<DynamicImportChunk> {
    let mut dynamic_imports = Vec::new();
    for module in compiled.iter().filter(|module| !module.is_external) {
        let ast = ast::parse_module(&module.js);
        for specifier in ast.dynamic_import_specifiers() {
            if let Some(dep) = linker::find_dep_for_specifier(&specifier, &module.deps)
                && let Some(file) = dynamic_import_files.get(dep)
            {
                dynamic_imports.push(DynamicImportChunk {
                    importer: module.path.display().to_string().replace('\\', "/"),
                    module: dep.display().to_string().replace('\\', "/"),
                    file: file.clone(),
                });
            }
        }
    }
    dynamic_imports
}

fn project_module_map(compiled: &[CompiledModule]) -> BTreeMap<PathBuf, &CompiledModule> {
    compiled
        .iter()
        .filter(|module| !module.is_external)
        .map(|module| (module.path.clone(), module))
        .collect()
}

fn dynamic_roots(
    compiled: &[CompiledModule],
    module_map: &BTreeMap<PathBuf, &CompiledModule>,
) -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    for module in compiled.iter().filter(|module| !module.is_external) {
        let ast = ast::parse_module(&module.js);
        for specifier in ast.dynamic_import_specifiers() {
            if let Some(dep) = linker::find_dep_for_specifier(&specifier, &module.deps)
                && module_map.contains_key(dep)
            {
                roots.insert(dep.clone());
            }
        }
    }
    roots
}

fn collect_static_transitive_modules(
    path: &Path,
    module_map: &BTreeMap<PathBuf, &CompiledModule>,
    selected: &mut BTreeSet<PathBuf>,
) {
    if !selected.insert(path.to_path_buf()) {
        return;
    }

    let Some(module) = module_map.get(path) else {
        return;
    };

    let ast = ast::parse_module(&module.js);
    for edge in ast
        .imports
        .iter()
        .filter(|edge| edge.kind != ImportKind::Dynamic)
    {
        if let Some(dep) = linker::find_dep_for_specifier(&edge.specifier, &module.deps)
            && module_map.contains_key(dep)
        {
            collect_static_transitive_modules(dep, module_map, selected);
        }
    }
}

fn graph_fingerprint(compiled: &[CompiledModule]) -> String {
    let mut hasher = blake3::Hasher::new();
    for module in compiled.iter().filter(|module| !module.is_external) {
        hasher.update(module.path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(module.js.as_bytes());
        hasher.update(b"\0");
    }
    hasher.finalize().to_hex().to_string()
}
