//! TypeScript and JSX compilation for the Ruvyxa Bundler.
//!
//! Ruvyxa owns module resolution, TypeScript build hooks, caching, boundary checks,
//! and linking. Oxc owns parsing, TypeScript stripping, JSX lowering, and
//! code generation for each source module.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{JsxRuntime as OxcJsxRuntime, TransformOptions, Transformer};
use rayon::prelude::*;

use crate::ast;
use crate::cache::{CacheLookup, CompileCache};
use crate::hooks::{BuildHookContext, BuildHookPipeline};
use crate::resolver::ResolvedModule;
use crate::{BundleError, BundleInput, JsxRuntime, Result};

/// A compiled module: TypeScript/JSX has been converted to plain JavaScript.
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// Canonical path (or virtual label for the synthetic entry).
    pub path: PathBuf,
    /// Plain JavaScript source after Oxc transformation.
    pub js: String,
    /// Dependency paths preserved from the resolver stage.
    pub deps: Vec<PathBuf>,
    /// Whether this module comes from `node_modules` (external).
    pub is_external: bool,
    /// Whether this module's compiled output came from the compile cache.
    pub cache_hit: bool,
}

struct CompiledModuleOutput {
    module: CompiledModule,
    hook_source_map: Option<String>,
}

pub(crate) fn compile_graph_with_hooks_and_maps(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
    build_hooks: &BuildHookPipeline,
) -> Result<(Vec<CompiledModule>, BTreeMap<PathBuf, String>)> {
    reject_case_colliding_css_modules(graph, input)?;
    let results: Vec<Result<CompiledModuleOutput>> = graph
        .par_iter()
        .map(|module| compile_module(module, input, cache, build_hooks))
        .collect();

    let mut modules = Vec::with_capacity(results.len());
    let mut source_maps = BTreeMap::new();
    for output in results {
        let output = output?;
        if let Some(source_map) = output.hook_source_map {
            source_maps.insert(output.module.path.clone(), source_map);
        }
        modules.push(output.module);
    }
    Ok((modules, source_maps))
}

/// Scoped class names hash a case-folded project-relative path (so the same
/// file hashes identically across case-insensitive filesystems). Two
/// *distinct* CSS module files whose paths differ only by case would
/// therefore share every generated class name and silently swap styles —
/// reject that graph up front with an actionable error instead.
fn reject_case_colliding_css_modules(graph: &[ResolvedModule], input: &BundleInput) -> Result<()> {
    let mut seen: BTreeMap<String, &Path> = BTreeMap::new();
    for module in graph {
        if !crate::style_module::is_css_module_path(&module.path) {
            continue;
        }
        let key = crate::style_module::normalized_relative_path(&module.path, &input.project_root);
        match seen.get(&key) {
            Some(existing) if *existing != module.path.as_path() => {
                return Err(BundleError::Compiler(format!(
                    "CSS module paths {} and {} differ only by letter case and would generate identical scoped class names; rename one file",
                    existing.display(),
                    module.path.display()
                )));
            }
            Some(_) => {}
            None => {
                seen.insert(key, &module.path);
            }
        }
    }
    Ok(())
}

fn compile_module(
    module: &ResolvedModule,
    input: &BundleInput,
    cache: &CompileCache,
    build_hooks: &BuildHookPipeline,
) -> Result<CompiledModuleOutput> {
    let ext = module
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    if crate::style_module::is_css_module_path(&module.path) {
        let css_module = crate::style_module::compile_css_module(&module.path, &input.project_root)
            .map_err(|error| {
                BundleError::Compiler(format!("{}: {error}", module.path.display()))
            })?;
        let js = crate::style_module::css_module_javascript(&css_module)
            .map_err(|error| BundleError::Compiler(error.to_string()))?;
        return Ok(CompiledModuleOutput {
            module: CompiledModule {
                path: module.path.clone(),
                js,
                deps: module.deps.clone(),
                is_external: false,
                cache_hit: false,
            },
            hook_source_map: None,
        });
    }

    let content_source = if matches!(ext, "md" | "mdx") {
        crate::content::compile_content_module(&module.source, &module.path)
            .map_err(BundleError::Compiler)?
    } else {
        module.source.clone()
    };

    let hook_context = BuildHookContext {
        project_root: input.project_root.clone(),
        importer: Some(module.path.clone()),
        target: input.target,
    };
    let hook_output =
        build_hooks.transform_with_map(&content_source, &module.path, &hook_context)?;
    let source = hook_output.code;
    let hook_source_map = hook_output.map;

    // Virtual entries and plain JavaScript pass through after registered transforms.
    if matches!(ext, "js" | "mjs" | "cjs") || module.path.to_string_lossy().contains("ruvyxa:") {
        return Ok(CompiledModuleOutput {
            module: CompiledModule {
                path: module.path.clone(),
                js: source,
                deps: module.deps.clone(),
                is_external: module.is_external,
                cache_hit: false,
            },
            hook_source_map,
        });
    }

    let transform_plan = ast::parse_module(&source);
    let has_jsx = matches!(ext, "tsx" | "jsx") || transform_plan.has_jsx;
    let jsx_runtime = input.options.jsx_runtime;

    match cache.lookup_with_options(&source, has_jsx, jsx_runtime) {
        CacheLookup::Hit(cached_js) => Ok(CompiledModuleOutput {
            module: CompiledModule {
                path: module.path.clone(),
                js: cached_js,
                deps: module.deps.clone(),
                is_external: module.is_external,
                cache_hit: true,
            },
            hook_source_map,
        }),
        CacheLookup::Miss(key) => {
            let js = transform_with_options(&source, has_jsx, jsx_runtime).map_err(|msg| {
                BundleError::Compiler(format!("{}: {}", module.path.display(), msg))
            })?;

            cache.store(&key, &js);

            Ok(CompiledModuleOutput {
                module: CompiledModule {
                    path: module.path.clone(),
                    js,
                    deps: module.deps.clone(),
                    is_external: module.is_external,
                    cache_hit: false,
                },
                hook_source_map,
            })
        }
    }
}

/// Transform TypeScript/JSX source to plain JavaScript (classic JSX mode).
pub fn transform(source: &str, has_jsx: bool) -> std::result::Result<String, String> {
    transform_with_options(source, has_jsx, JsxRuntime::Classic)
}

/// Transform with explicit JSX runtime selection.
pub fn transform_with_options(
    source: &str,
    has_jsx: bool,
    jsx_runtime: JsxRuntime,
) -> std::result::Result<String, String> {
    // Preserve Ruvyxa's historical decorator contract: decorators are accepted
    // but removed without injecting an external runtime helper.
    let source = strip_decorators(source);
    let allocator = Allocator::default();
    let source_type = SourceType::mjs().with_typescript(true).with_jsx(has_jsx);
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(format!(
            "Oxc could not parse TypeScript/JSX: {} syntax diagnostic(s)",
            parsed.diagnostics.len()
        ));
    }

    let mut program = parsed.program;
    let semantic = SemanticBuilder::new_compiler()
        .with_enum_eval(true)
        .build(&program);
    if !semantic.diagnostics.is_empty() {
        return Err(format!(
            "Oxc semantic analysis failed: {} diagnostic(s)",
            semantic.diagnostics.len()
        ));
    }

    let mut options = TransformOptions::default();
    options.jsx.runtime = match jsx_runtime {
        JsxRuntime::Classic => OxcJsxRuntime::Classic,
        JsxRuntime::Automatic => OxcJsxRuntime::Automatic,
    };
    options.jsx.jsx_plugin = has_jsx;
    options.jsx.throw_if_namespace = false;
    options.jsx.pure = false;
    options.typescript.optimize_const_enums = false;
    options.typescript.optimize_enums = false;

    let transformed = Transformer::new(&allocator, Path::new("ruvyxa:module.tsx"), &options)
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if !transformed.diagnostics.is_empty() {
        return Err(format!(
            "Oxc TypeScript/JSX transform failed: {} diagnostic(s)",
            transformed.diagnostics.len()
        ));
    }

    Ok(Codegen::new().build(&program).code)
}

/// Strip legacy decorators while preserving source line positions.
fn strip_decorators(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if matches!(chars[i], '"' | '\'' | '`') {
            let quote = chars[i];
            out.push(quote);
            i += 1;
            while i < len {
                if chars[i] == '\\' && i + 1 < len {
                    out.push(chars[i]);
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                out.push(chars[i]);
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        if chars[i] == '@' {
            let mut j = i;
            while j > 0 && chars[j - 1] != '\n' {
                if !matches!(chars[j - 1], ' ' | '\t') {
                    break;
                }
                j -= 1;
            }
            if j == i || chars[j..i].iter().all(|c| matches!(c, ' ' | '\t')) {
                i += 1;
                while i < len && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.')) {
                    i += 1;
                }
                if i < len && chars[i] == '(' {
                    let mut depth = 1;
                    i += 1;
                    while i < len && depth > 0 {
                        match chars[i] {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            '"' | '\'' | '`' => {
                                let quote = chars[i];
                                i += 1;
                                while i < len {
                                    if chars[i] == '\\' {
                                        i += 2;
                                        continue;
                                    }
                                    if chars[i] == quote {
                                        break;
                                    }
                                    i += 1;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                out.push('\n');
                continue;
            }
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_interface() {
        let out = transform("interface Foo { bar: string; }\nconst x = 1;", false).unwrap();
        assert!(!out.contains("interface Foo"));
        assert!(out.contains("const x = 1;"));
    }

    #[test]
    fn strips_type_annotation() {
        let out = transform("const x: number = 5;", false).unwrap();
        assert!(!out.contains(": number"));
        assert!(out.contains("const x"));
    }

    #[test]
    fn strips_generic_type_params() {
        let out = transform("const arr = new Array<number>();", false).unwrap();
        assert!(!out.contains("<number>"));
        assert!(out.contains("new Array()"));
    }

    #[test]
    fn transforms_classic_jsx() {
        let out = transform("const el = <Input disabled />;", true).unwrap();
        assert!(out.contains("React.createElement(Input"));
        assert!(out.contains("disabled: true"));
    }

    #[test]
    fn transforms_automatic_jsx() {
        let out = transform_with_options(
            "const el = <div><span/><span/></div>;",
            true,
            JsxRuntime::Automatic,
        )
        .unwrap();
        assert!(out.contains("_jsxs"));
        assert!(out.contains("react/jsx-runtime"));
    }

    #[test]
    fn strips_decorators() {
        let out = strip_decorators("@Injectable()\nclass Service {}");
        assert!(!out.contains("@Injectable"));
        assert!(out.contains("class Service"));
    }

    #[test]
    fn transforms_enums() {
        let out = transform("enum Direction { Up, Down = 5, Left }", false).unwrap();
        assert!(out.contains("Direction"));
        assert!(out.contains("Development") || out.contains("Up"));
    }

    #[test]
    fn strips_satisfies_expression() {
        let out = transform("const config = { port: 3000 } satisfies Config;", false).unwrap();
        assert!(!out.contains("satisfies"));
        assert!(out.contains("const config"));
    }

    #[test]
    fn transforms_fragments_and_nested_expressions() {
        let src = r#"const el = <><p>Rendered from <code>{'[...slug]'}</code></p></>;"#;
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.Fragment"));
        assert!(out.contains("[...slug]"));
    }

    #[test]
    fn strips_destructured_param_type_before_jsx() {
        let src = r#"export default function Page({ params }: { params: { slug: string } }) {
  return <main>{params.slug}</main>
}"#;
        let out = transform(src, true).unwrap();
        assert!(!out.contains(": { params"));
        assert!(out.contains("React.createElement(\"main\""));
    }
}
