//! Ruvyxa Bundler TypeScript/JSX Compiler
//!
//! The TypeScript and JSX transformer used by Ruvyxa Bundler
//! designed specifically for the Ruvyxa framework.
//!
//! ## Strategy
//!
//! Ruvyxa delegates syntax lowering to Oxc's parser and transformer so the
//! runtime compiler shares one standards-aware Rust pipeline across platforms.
//!
//! Oxc owns TypeScript syntax lowering, enum/decorator handling, JSX lowering in
//! classic or automatic mode, and standards-aware code generation. Ruvyxa
//! retains the framework-specific graph, plugin, boundary, and cache contracts
//! around that transform.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{JsxRuntime as OxcJsxRuntime, TransformOptions, Transformer};
use rayon::prelude::*;

use crate::ast;
use crate::cache::{CacheLookup, CompileCache};
use crate::plugin::{PluginContext, PluginPipeline};
use crate::resolver::ResolvedModule;
use crate::{BundleError, BundleInput, JsxRuntime, Result};

/// A compiled module: TypeScript/JSX has been converted to plain JS.
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// Canonical path (or virtual label for the synthetic entry).
    pub path: PathBuf,
    /// Plain JavaScript source after TS stripping and JSX transform.
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
    plugin_source_map: Option<String>,
}

/// Compile every module in the resolved graph, using the provided cache.
///
/// Modules are compiled in parallel using rayon's work-stealing thread pool.
/// Each module is independent at this stage (deps are resolved in the prior
/// step), so compilation is embarrassingly parallel.
pub fn compile_graph_with_cache(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
) -> Result<Vec<CompiledModule>> {
    compile_graph_with_pipeline(graph, input, cache, &PluginPipeline::empty())
}

/// Compile every module using the provided cache and native plugin pipeline.
pub fn compile_graph_with_pipeline(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
    plugins: &PluginPipeline,
) -> Result<Vec<CompiledModule>> {
    Ok(compile_graph_with_pipeline_and_maps(graph, input, cache, plugins)?.0)
}

pub(crate) fn compile_graph_with_pipeline_and_maps(
    graph: &[ResolvedModule],
    input: &BundleInput,
    cache: &CompileCache,
    plugins: &PluginPipeline,
) -> Result<(Vec<CompiledModule>, BTreeMap<PathBuf, String>)> {
    let results: Vec<Result<CompiledModuleOutput>> = graph
        .par_iter()
        .map(|module| compile_module(module, input, cache, plugins))
        .collect();

    let mut modules = Vec::with_capacity(results.len());
    let mut source_maps = BTreeMap::new();
    for output in results {
        let output = output?;
        if let Some(source_map) = output.plugin_source_map {
            source_maps.insert(output.module.path.clone(), source_map);
        }
        modules.push(output.module);
    }
    Ok((modules, source_maps))
}

fn compile_module(
    module: &ResolvedModule,
    input: &BundleInput,
    cache: &CompileCache,
    plugins: &PluginPipeline,
) -> Result<CompiledModuleOutput> {
    let ext = module
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let content_source = if matches!(ext, "md" | "mdx") {
        crate::content::compile_content_module(&module.source, &module.path)
            .map_err(BundleError::Compiler)?
    } else {
        module.source.clone()
    };

    let plugin_ctx = PluginContext {
        project_root: input.project_root.clone(),
        importer: Some(module.path.clone()),
        target: input.target,
    };
    let plugin_output = plugins.transform_with_map(&content_source, &module.path, &plugin_ctx)?;
    let source = plugin_output.code;
    let plugin_source_map = plugin_output.map;

    // Plain JavaScript needs no TypeScript/JSX transform. Virtual entries use
    // their extension so runtime-generated TS/TSX source goes through the
    // same native compiler as project files.
    if matches!(ext, "js" | "mjs" | "cjs") {
        return Ok(CompiledModuleOutput {
            module: CompiledModule {
                path: module.path.clone(),
                js: source,
                deps: module.deps.clone(),
                is_external: module.is_external,
                cache_hit: false,
            },
            plugin_source_map,
        });
    }

    let transform_plan = ast::parse_module(&source);
    let has_jsx = matches!(ext, "tsx" | "jsx") || transform_plan.has_jsx;
    let jsx_runtime = input.options.jsx_runtime;

    // Cache key includes JSX runtime mode so switching modes invalidates entries.
    match cache.lookup_with_options(&source, has_jsx, jsx_runtime) {
        CacheLookup::Hit(cached_js) => Ok(CompiledModuleOutput {
            module: CompiledModule {
                path: module.path.clone(),
                js: cached_js,
                deps: module.deps.clone(),
                is_external: module.is_external,
                cache_hit: true,
            },
            plugin_source_map,
        }),
        CacheLookup::Miss(key) => {
            let js = transform_for_bundle(&source, has_jsx, jsx_runtime).map_err(|msg| {
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
                plugin_source_map,
            })
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Core transformer entry points
// ─────────────────────────────────────────────────────────────────────────────

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
    transform_source(source, has_jsx, jsx_runtime, false)
}

fn transform_for_bundle(
    source: &str,
    has_jsx: bool,
    jsx_runtime: JsxRuntime,
) -> std::result::Result<String, String> {
    transform_source(source, has_jsx, jsx_runtime, true)
}

fn transform_source(
    source: &str,
    has_jsx: bool,
    jsx_runtime: JsxRuntime,
    bundle_mode: bool,
) -> std::result::Result<String, String> {
    let allocator = Allocator::default();
    let source_type = if has_jsx {
        SourceType::tsx()
    } else {
        SourceType::ts()
    }
    .with_module(true);
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return Err(parsed
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let mut program = parsed.program;
    let semantic = SemanticBuilder::new().with_enum_eval(true).build(&program);
    if !semantic.diagnostics.is_empty() {
        return Err(semantic
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let mut options = TransformOptions::default();
    options.decorator.legacy = true;
    options.jsx.throw_if_namespace = false;
    options.jsx.runtime = match jsx_runtime {
        JsxRuntime::Classic => OxcJsxRuntime::Classic,
        JsxRuntime::Automatic => OxcJsxRuntime::Automatic,
    };
    let transformed = Transformer::new(&allocator, Path::new("module.tsx"), &options)
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if !transformed.diagnostics.is_empty() {
        return Err(transformed
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let mut codegen_options = CodegenOptions::default();
    if bundle_mode {
        // The linker consumes module statements while preserving the source
        // structure needed for readable diagnostics and stable source maps.
        // Drop ordinary comments here; legal and annotation comments remain.
        codegen_options.comments = CommentOptions::disabled();
    }
    Ok(Codegen::new()
        .with_options(codegen_options)
        .build(&program)
        .code)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_interface() {
        let src = "interface Foo { bar: string; }\nconst x = 1;";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("interface Foo"));
        assert!(out.contains("const x = 1;"));
    }

    #[test]
    fn strips_type_annotation() {
        let src = "const x: number = 5;";
        let out = transform(src, false).unwrap();
        assert!(!out.contains(": number"));
        assert!(out.contains("const x"));
        assert!(out.contains("= 5"));
    }

    #[test]
    fn strips_generic_type_params() {
        let src = "const arr = new Array<number>();";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("<number>"));
        assert!(out.contains("new Array()"));
    }

    #[test]
    fn transforms_simple_jsx_classic() {
        let src = "const el = <div className=\"x\">hello</div>;";
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.createElement(\"div\""));
        assert!(out.contains("className"));
    }

    #[test]
    fn transforms_self_closing_jsx_classic() {
        let src = "const el = <Input disabled />;";
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.createElement(Input"));
        assert!(out.contains("disabled: true"));
    }

    #[test]
    fn transforms_jsx_automatic() {
        let src = "const el = <div>hello</div>;";
        let out = transform_with_options(src, true, JsxRuntime::Automatic).unwrap();
        assert!(out.contains("import { jsx as _jsx"));
        assert!(out.contains("_jsx(\"div\""));
    }

    #[test]
    fn transforms_jsx_automatic_multi_child_uses_jsxs() {
        let src = "const el = <div><span/><span/></div>;";
        let out = transform_with_options(src, true, JsxRuntime::Automatic).unwrap();
        assert!(out.contains("_jsxs"));
        assert!(out.contains("import { jsx as _jsx, jsxs as _jsxs"));
    }

    #[test]
    fn strips_decorators() {
        let src = "@Injectable()\nclass Service {}";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("@Injectable"));
        assert!(out.contains("class Service"));
    }

    #[test]
    fn expands_enum() {
        let src = "enum Direction { Up, Down = 5, Left }";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("enum Direction"));
        assert!(out.contains("Direction"));
        assert!(out.contains("Up"));
        assert!(out.contains("Down"));
        assert!(out.contains("Left"));
    }

    #[test]
    fn expands_const_enum() {
        let src = "const enum Color { Red, Green, Blue }";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("const enum"));
        assert!(out.contains("Color"));
        assert!(out.contains("Red"));
        assert!(out.contains("Green"));
        assert!(out.contains("Blue"));
    }

    #[test]
    fn strips_satisfies_expression() {
        let src = "const config = { port: 3000 } satisfies Config;";
        let out = transform(src, false).unwrap();
        assert!(!out.contains("satisfies"));
        assert!(out.contains("const config"));
    }

    #[test]
    fn jsx_fragment_classic() {
        let src = "const el = <><div/><span/></>;";
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.Fragment"));
    }

    #[test]
    fn jsx_fragment_automatic() {
        let src = "const el = <><div/></>;";
        let out = transform_with_options(src, true, JsxRuntime::Automatic).unwrap();
        assert!(out.contains("_Fragment"));
    }

    #[test]
    fn hyphenated_tag_quoted() {
        let src = "const el = <my-element />;";
        let out = transform(src, true).unwrap();
        assert!(out.contains("\"my-element\""));
    }

    #[test]
    fn jsx_text_with_inline_code_element() {
        let src = r#"export default function About() {
  return (
    <main className="page">
      <p>Rendered from <code>app/about/page.tsx</code> - a static page.</p>
      <p>Every <code>page.tsx</code> file becomes a route.</p>
    </main>
  )
}"#;

        let out = transform(src, true).unwrap();
        assert!(out.contains("React.createElement(\"code\""));
        assert!(out.contains("React.createElement(\"main\""));
    }

    #[test]
    fn jsx_text_colon_is_not_type_annotation() {
        let src = r#"export default function About() {
  return (
    <main>
      <p>This demonstrates routing: every <code>page.tsx</code> file becomes a route.</p>
    </main>
  )
}"#;

        let out = transform(src, true).unwrap();
        assert!(out.contains("routing: every"));
        assert!(out.contains("React.createElement(\"code\""));
    }

    #[test]
    fn jsx_code_child_with_expression_and_slashes() {
        let src = r#"const el = (
  <p>Rendered from the <code>catchall/{'[...slug]'}/page.tsx</code> file.</p>
);"#;

        let out = transform(src, true).unwrap();
        assert!(out.contains("catchall/"));
        assert!(out.contains("[...slug]"));
        assert!(out.contains("/page.tsx"));
    }

    #[test]
    fn strips_destructured_param_type_before_jsx() {
        let src = r#"export default function CatchAll({ params }: { params: { slug: string } }) {
  return (
    <main className="page">
      <p>Rendered from the <code>catchall/{'[...slug]'}/page.tsx</code> file.</p>
        <p>The <code>{'[...slug]'}</code> pattern captures all remaining URL segments:</p>
    </main>
  )
}"#;

        let stripped = transform(src, true).unwrap();
        assert!(!stripped.contains(": { params"), "{stripped}");
        let out = transform(src, true).unwrap();
        assert!(out.contains("React.createElement(\"code\""));
        assert!(out.contains("React.createElement(\"main\""));
    }
}
