//! Chunk graph helpers for dynamic imports and future shared chunk loading.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::compiler::CompiledModule;
use crate::{
    ast, linker, minifier, BundleInput, DynamicImportChunk, OutputChunk, OutputChunkKind, Result,
};

/// Build output chunks for project-local dynamic import split points.
pub(crate) fn build_dynamic_output_chunks(
    compiled: &[CompiledModule],
    input: &BundleInput,
) -> Result<Vec<OutputChunk>> {
    let module_map: BTreeMap<PathBuf, &CompiledModule> = compiled
        .iter()
        .filter(|module| !module.is_external)
        .map(|module| (module.path.clone(), module))
        .collect();
    let mut dynamic_roots = BTreeSet::<PathBuf>::new();

    for module in compiled.iter().filter(|module| !module.is_external) {
        let ast = ast::parse_module(&module.js);
        for specifier in ast.dynamic_import_specifiers() {
            if let Some(dep) = linker::find_dep_for_specifier(&specifier, &module.deps) {
                dynamic_roots.insert(dep.clone());
            }
        }
    }

    let mut chunks = Vec::new();
    for root in dynamic_roots {
        let mut selected = BTreeSet::new();
        collect_transitive_modules(&root, &module_map, &mut selected);
        let modules = compiled
            .iter()
            .filter(|module| selected.contains(&module.path))
            .cloned()
            .collect::<Vec<_>>();

        if modules.is_empty() {
            continue;
        }

        let mut linked = linker::link_parallel(&modules, input)?;
        linked.push_str("export default ");
        linked.push_str(&linker::module_id(&root));
        linked.push_str(";\n");

        let code = if input.options.minify {
            minifier::minify_parallel_with_options(&linked, input.target, false)?
        } else {
            linked
        };
        let hash = blake3::hash(code.as_bytes()).to_hex();
        let file_name = format!("chunk.{}.js", &hash[..16]);
        let modules = modules
            .iter()
            .map(|module| module.path.display().to_string().replace('\\', "/"))
            .collect::<Vec<_>>();

        chunks.push(OutputChunk {
            file_name,
            code,
            modules,
            kind: OutputChunkKind::DynamicImport,
        });
    }

    Ok(chunks)
}

pub(crate) fn dynamic_import_chunks(
    compiled: &[CompiledModule],
    output_chunks: &[OutputChunk],
) -> Vec<DynamicImportChunk> {
    let mut dynamic_imports = Vec::new();
    for module in compiled.iter().filter(|module| !module.is_external) {
        let ast = ast::parse_module(&module.js);
        for specifier in ast.dynamic_import_specifiers() {
            if let Some(dep) = linker::find_dep_for_specifier(&specifier, &module.deps) {
                let module_path = dep.display().to_string().replace('\\', "/");
                let file = output_chunks
                    .iter()
                    .find(|chunk| chunk.modules.iter().any(|m| m == &module_path))
                    .map(|chunk| chunk.file_name.clone())
                    .unwrap_or_else(|| {
                        let hash = blake3::hash(module_path.as_bytes()).to_hex();
                        format!("chunk.{}.js", &hash[..16])
                    });
                dynamic_imports.push(DynamicImportChunk {
                    importer: module.path.display().to_string().replace('\\', "/"),
                    module: module_path,
                    file,
                });
            }
        }
    }
    dynamic_imports
}

fn collect_transitive_modules(
    path: &PathBuf,
    module_map: &BTreeMap<PathBuf, &CompiledModule>,
    selected: &mut BTreeSet<PathBuf>,
) {
    if !selected.insert(path.clone()) {
        return;
    }

    let Some(module) = module_map.get(path) else {
        return;
    };

    for dep in &module.deps {
        if module_map.contains_key(dep) {
            collect_transitive_modules(dep, module_map, selected);
        }
    }
}
