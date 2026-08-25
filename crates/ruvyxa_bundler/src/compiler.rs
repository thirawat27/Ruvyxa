//! TypeScript and JSX compilation for the Ruvyxa Bundler.
//!
//! Ruvyxa owns module resolution, TypeScript build hooks, caching, boundary checks,
//! and linking. Oxc owns parsing, TypeScript stripping, JSX lowering, and
//! code generation for each source module.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{JsxRuntime as OxcJsxRuntime, TransformOptions, Transformer};
use rayon::prelude::*;

use crate::ast;
use crate::ast::ModuleAst;
use crate::cache::{CacheLookup, CompileCache};
use crate::hooks::{BuildHookContext, BuildHookPipeline};
use crate::resolver::ResolvedModule;
use crate::{BundleError, BundleInput, EsTarget, JsxRuntime, Result};

/// A compiled module: TypeScript/JSX has been converted to plain JavaScript.
///
/// Both heavy fields are shared rather than owned. A build holds the same module
/// in several collections at once — the full graph, the entry's static closure,
/// and one closure per emitted chunk — and each of those used to carry its own
/// copy of the generated JavaScript. Construct with [`CompiledModule::new`].
#[derive(Debug, Clone)]
pub struct CompiledModule {
    /// Canonical path (or virtual label for the synthetic entry).
    pub path: PathBuf,
    /// Plain JavaScript source after Oxc transformation.
    pub js: Arc<str>,
    /// Facts parsed from [`Self::js`], computed once at construction.
    pub ast: Arc<ast::ModuleAst>,
    /// Dependency paths preserved from the resolver stage.
    pub deps: Arc<[PathBuf]>,
    /// Exact source specifier to resolved path bindings from the resolver.
    pub dependency_aliases: Arc<BTreeMap<String, PathBuf>>,
    /// Whether this module comes from `node_modules` (external).
    pub is_external: bool,
    /// Whether this module's compiled output came from the compile cache.
    pub cache_hit: bool,
}

impl CompiledModule {
    /// Build a compiled module, scanning its output exactly once.
    ///
    /// The AST is parsed here — the single point where a `CompiledModule` comes
    /// into existence — so no later stage has a reason to walk `js` again. Chunk
    /// planning alone re-parsed every module once per dynamic root, once per
    /// emitted chunk, and twice more for the boundary check; that cost scaled
    /// with the number of `import()` sites in the app rather than with its size.
    pub(crate) fn new(
        path: PathBuf,
        js: String,
        deps: Vec<PathBuf>,
        dependency_aliases: BTreeMap<String, PathBuf>,
        is_external: bool,
        cache_hit: bool,
    ) -> Self {
        // External modules are excluded from linking, chunk closures, and the
        // boundary check, so their AST is never read. Scanning node_modules
        // output to fill a field nobody consults is pure cost.
        let ast = if is_external {
            Arc::new(ast::ModuleAst::default())
        } else {
            Arc::new(ast::parse_module(&js))
        };
        Self {
            path,
            js: js.into(),
            ast,
            deps: deps.into(),
            dependency_aliases: Arc::new(dependency_aliases),
            is_external,
            cache_hit,
        }
    }
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

/// Every file extension this bundler knows how to turn into a module.
///
/// Mirrors `MODULE_KIND_EXTENSIONS` in
/// `packages/ruvyxa/runtime/compiler.mjs` — the two module graphs must agree on
/// which files are compilable, or a build passes on one path and fails on the
/// other.
const MODULE_KIND_EXTENSIONS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs", "md", "mdx", "json", "css", "scss",
    "sass",
];

/// Compile a JSON file into a module the linker's CommonJS wrapper can host.
///
/// The document becomes one string literal parsed at runtime rather than an
/// inline object literal, so no JSON text can be misread as code. Mirrors
/// `compileJsonModuleSource()` in `packages/ruvyxa/runtime/compiler.mjs`.
fn compile_json_module(source: &str, path: &Path) -> Result<String> {
    let trimmed = source.strip_prefix('\u{feff}').unwrap_or(source);
    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|error| {
        BundleError::Compiler(format!(
            "RUV1805 Invalid JSON module {}: {error}",
            path.display()
        ))
    })?;
    let serialized = serde_json::to_string(&value)
        .map_err(|error| BundleError::Compiler(format!("{}: {error}", path.display())))?;
    let literal = serde_json::to_string(&serialized)
        .map_err(|error| BundleError::Compiler(format!("{}: {error}", path.display())))?;

    let mut js = format!("module.exports = JSON.parse({literal});\n");
    // The linker reads `<module>.default ?? <module>`, so an object gets a
    // non-enumerable self-reference to make a default import the whole document
    // — but never when the document has its own `default` key, because
    // overwriting it would change data the application can read.
    let attach_default = match &value {
        serde_json::Value::Object(map) => !map.contains_key("default"),
        serde_json::Value::Array(_) => true,
        _ => false,
    };
    if attach_default {
        js.push_str(
            "Object.defineProperty(module.exports, 'default', { value: module.exports, configurable: true });\n",
        );
    }
    Ok(js)
}

/// Reject a resolved file whose extension has no compilation path, by name.
///
/// Without this the file reaches the JavaScript transform and Oxc reports a
/// syntax error inside a dependency the application never wrote.
/// The source as the linker receives it, after the pipeline's re-printing.
///
/// Exposed for the syntax-conformance test, which links dependency sources
/// straight from the shared table: without this it would be linking a form the
/// pipeline never produces, and would report failures the build does not have.
#[cfg(test)]
pub(crate) fn prepared_for_linking(source: &str) -> String {
    // The same two erasures `transform_with_plan` performs, in the same order:
    // a decorator and a shebang are both gone before the linker sees a module.
    let source = strip_decorators(source);
    let source = strip_shebang(&source);
    expand_multi_statement_esm(&source).unwrap_or(source)
}

/// Re-print JavaScript whose ESM statements share a line, one statement per line.
///
/// Returns `None` when nothing needs changing, so a well-formed dependency is
/// not parsed at all. Also returns `None` when the source cannot be parsed or
/// re-printed: this is an optimisation for the linker's benefit, never a reason
/// to fail a build here. The linker's `RUV1612` guard still refuses anything
/// that remains unlinkable, so giving up quietly cannot ship broken output.
fn expand_multi_statement_esm(source: &str) -> Option<String> {
    if !has_esm_statement_sharing_a_line(source) && !has_esm_clause_spanning_lines(source) {
        return None;
    }

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::unambiguous()).parse();
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    // Default codegen, not `minify()`: the point is the newline between
    // statements. Legal comments are kept because a dependency's licence notice
    // travels in one, and this is the only pass that would otherwise drop it.
    let printed = Codegen::new()
        .with_options(oxc::codegen::CodegenOptions {
            comments: oxc::codegen::CommentOptions {
                normal: false,
                jsdoc: false,
                annotation: true,
                legal: oxc::codegen::LegalComment::Inline,
            },
            ..Default::default()
        })
        .build(&parsed.program)
        .code;
    (!printed.is_empty()).then_some(printed)
}

/// Whether any `import`/`export` statement is not the first token on its line.
///
/// Uses [`ast::masked_code`] so the words are only counted where they are code,
/// and so this agrees with the linker about where text begins and ends. The
/// expression forms `import(…)` and `import.meta` are not statements and do not
/// need a line of their own.
pub(crate) fn has_esm_statement_sharing_a_line(source: &str) -> bool {
    if !source.contains("import") && !source.contains("export") {
        return false;
    }
    let masked = ast::masked_code(source);
    masked.lines().any(|line| {
        let bytes = line.as_bytes();
        ["export", "import"].iter().any(|keyword| {
            line.match_indices(keyword).any(|(at, _)| {
                let starts_token = at
                    .checked_sub(1)
                    .is_none_or(|before| !is_ascii_identifier_byte(bytes[before]));
                let ends_token = bytes
                    .get(at + keyword.len())
                    .is_none_or(|byte| !is_ascii_identifier_byte(*byte));
                if !starts_token || !ends_token {
                    return false;
                }
                // Already the first token on this line: the linker handles it.
                if line[..at].trim().is_empty() {
                    return false;
                }
                if *keyword == "import" {
                    let next = line[at + keyword.len()..]
                        .bytes()
                        .find(|byte| !byte.is_ascii_whitespace());
                    if matches!(next, Some(b'(') | Some(b'.')) {
                        return false;
                    }
                }
                true
            })
        })
    })
}

/// Whether an `import`/`export` **clause** is broken across lines.
///
/// The linker rewrites one line at a time and expects a whole ESM statement on
/// each. `has_esm_statement_sharing_a_line` covers the minified direction — two
/// statements on one line — and this covers the other one, which Prettier
/// produces the moment an import or export list outgrows the print width:
///
/// ```js
/// export {
///   readFile,
///   writeFile,
/// }
/// ```
///
/// A `.ts`/`.tsx` module never reaches the linker in that shape because the
/// transform re-prints it. A `.js`/`.mjs`/`.cjs` module is passed through
/// untouched, so it did: the multi-line `import` form failed the build with
/// `RUV1612`, and the `export` form was worse — the clause line matched the
/// local-named-export rewrite and vanished, leaving `readFile,` and a stray `}`
/// loose inside the module IIFE. Nothing said `export`, so `reject_surviving_esm`
/// passed it, and the browser got a bundle that does not parse.
///
/// Only the clause form counts. `export default {` and `export const x = {` also
/// open a brace they do not close, and the linker handles both deliberately —
/// re-printing those would rewrite working output for nothing.
pub(crate) fn has_esm_clause_spanning_lines(source: &str) -> bool {
    if !source.contains("import") && !source.contains("export") {
        return false;
    }
    ast::masked_code(source).lines().any(|line| {
        let trimmed = line.trim_start();
        let Some(rest) = ["import", "export"]
            .iter()
            .find_map(|keyword| trimmed.strip_prefix(keyword))
        else {
            return false;
        };
        // A token boundary, so `exporter` and `imports` are ordinary code.
        if rest.starts_with(is_identifier_char) {
            return false;
        }
        // The clause brace is the first thing after the keyword. Anything else
        // — `default`, `const`, `* from`, a bare specifier — is a form the
        // linker already spans correctly.
        rest.trim_start().starts_with('{') && !rest.contains('}')
    })
}

fn is_identifier_char(character: char) -> bool {
    is_ascii_identifier_byte(character as u8) && character.is_ascii()
}

fn is_ascii_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn reject_unsupported_module_kind(ext: &str, path: &Path) -> Result<()> {
    if ext.is_empty()
        || MODULE_KIND_EXTENSIONS
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    {
        return Ok(());
    }
    Err(BundleError::Compiler(format!(
        "RUV1806 cannot compile {} (.{ext}): Ruvyxa compiles {}. \
         Add the package to `build.external` if it must load this file at runtime.",
        path.display(),
        MODULE_KIND_EXTENSIONS
            .iter()
            .map(|known| format!(".{known}"))
            .collect::<Vec<_>>()
            .join(", ")
    )))
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
            module: CompiledModule::new(
                module.path.clone(),
                js,
                module.deps.clone(),
                module.dependency_aliases.clone(),
                false,
                false,
            ),
            hook_source_map: None,
        });
    }

    if ext.eq_ignore_ascii_case("json") {
        let js = compile_json_module(&module.source, &module.path)?;
        return Ok(CompiledModuleOutput {
            module: CompiledModule::new(
                module.path.clone(),
                js,
                Vec::new(),
                BTreeMap::new(),
                false,
                false,
            ),
            hook_source_map: None,
        });
    }

    let content_source = if let Some(compiled) = &module.compiled_content {
        compiled.to_string()
    } else if matches!(ext, "md" | "mdx") {
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
    let hook_source_map = hook_output.map.or_else(|| module.load_source_map.clone());

    // Virtual entries and plain JavaScript pass through after registered transforms.
    if matches!(ext, "js" | "mjs" | "cjs") || module.path.to_string_lossy().contains("ruvyxa:") {
        // ...but only after its statements are given one line each. The linker
        // rewrites ESM a line at a time, and this is the one path that reaches
        // it without having been through codegen — a `.ts`/`.tsx` module is
        // always re-printed by the transform below. A published `dist` build is
        // routinely minified, so `const a=1;export{a as B};` arrived here
        // verbatim, matched none of the linker's line patterns, and was copied
        // into the module IIFE where `export` is a syntax error: the whole
        // browser bundle stopped parsing, with nothing said at build time.
        //
        // Re-printing is skipped unless the module actually has an ESM
        // statement sharing a line, so the overwhelmingly common well-formed
        // dependency costs one scan and no parse.
        let source = expand_multi_statement_esm(&source).unwrap_or(source);
        return Ok(CompiledModuleOutput {
            module: CompiledModule::new(
                module.path.clone(),
                source,
                module.deps.clone(),
                module.dependency_aliases.clone(),
                module.is_external,
                false,
            ),
            hook_source_map,
        });
    }

    // Everything that reaches here is about to be parsed as JavaScript. Anything
    // whose extension says otherwise is named as unsupported now, rather than
    // surfacing later as a syntax error in a dependency.
    reject_unsupported_module_kind(ext, &module.path)?;

    let transform_plan = ast::parse_module(&source);
    let has_jsx = jsx_is_enabled(ext, transform_plan.has_jsx);
    let jsx_runtime = input.options.jsx_runtime;
    let es_target = input.options.es_target;

    match cache.lookup_with_options(&source, has_jsx, jsx_runtime, es_target) {
        CacheLookup::Hit(cached_js) => Ok(CompiledModuleOutput {
            module: CompiledModule::new(
                module.path.clone(),
                cached_js,
                module.deps.clone(),
                module.dependency_aliases.clone(),
                module.is_external,
                true,
            ),
            hook_source_map,
        }),
        CacheLookup::Miss(key) => {
            let js = transform_with_plan(
                &source,
                has_jsx,
                jsx_runtime,
                es_target,
                Some(&transform_plan),
            )
            .map_err(|msg| BundleError::Compiler(format!("{}: {}", module.path.display(), msg)))?;

            cache.store(&key, &js);

            Ok(CompiledModuleOutput {
                module: CompiledModule::new(
                    module.path.clone(),
                    js,
                    module.deps.clone(),
                    module.dependency_aliases.clone(),
                    module.is_external,
                    false,
                ),
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
    transform_with_plan(source, has_jsx, jsx_runtime, EsTarget::EsNext, None)
}

/// Transform, reusing a [`ModuleAst`] the caller already produced.
///
/// The compile path parses every module before it gets here — that is where
/// `has_jsx` comes from — and [`crate::ast`] is explicit that a fact it already
/// collected should not send a consumer back over the bytes. Passing the plan in
/// lets decorator stripping reuse that walk instead of running a second one, and
/// lets it gate on `has_decorators`, which is the precise answer rather than the
/// line-scan approximation.
pub(crate) fn transform_with_plan(
    source: &str,
    has_jsx: bool,
    jsx_runtime: JsxRuntime,
    es_target: EsTarget,
    plan: Option<&ModuleAst>,
) -> std::result::Result<String, String> {
    // Preserve Ruvyxa's historical decorator contract: decorators are accepted
    // but removed without injecting an external runtime helper.
    let source = match plan {
        Some(plan) => strip_decorators_with_plan(source, plan),
        None => strip_decorators(source),
    };
    let source = strip_shebang(&source);
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

    // `from_target` fills `env` and leaves every other field at its default,
    // so the JSX and TypeScript settings below still apply. `EsNext` keeps the
    // historical `default()` so a project that configures nothing pays no new
    // analysis and emits the bytes it always did.
    let mut options = if es_target.is_default() {
        TransformOptions::default()
    } else {
        TransformOptions::from_target(es_target.as_str())
            .map_err(|error| format!("Oxc rejected build.target `{es_target}`: {error}"))?
    };
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

    let code = Codegen::new().build(&program).code;
    reject_runtime_helpers(&code, es_target)?;
    Ok(substitute_public_env(&code))
}

/// Replace `import.meta.env` with the public environment, as a literal.
///
/// Documented in the configuration chapter and implemented by nothing: the
/// expression was left as written, `import.meta` carries no `env` in Node or in
/// a browser, and the documented client component threw
/// `Cannot read properties of undefined` during the server render of its own
/// page. Substituting at compile time is what makes the value the same in both
/// graphs, and only `RUVYXA_PUBLIC_*` names are in the object, so it can never
/// widen what a browser bundle may read.
///
/// Twin of `substitutePublicEnv` in `packages/ruvyxa/runtime/compiler.mjs`.
/// Whether the parser is given JSX for a module with this extension.
///
/// TypeScript settles this by extension rather than by contents, because the two
/// readings are mutually exclusive: in a `.ts` file `<T>(x)` is a generic
/// parameter list and `<string>v` a type assertion, while the same bytes in a
/// `.tsx` file open an element. Letting the scan's guess switch JSX on for a
/// `.ts` file made both of those ordinary forms fail to parse, and the
/// diagnostic pointed at the author's code rather than at the guess — while
/// `runtime/compiler.mjs`, which picks the dialect from the extension alone,
/// compiled the same file. That is the divergence this shape exists to close.
///
/// A `.js` file keeps the guess: JSX in one is common in the ecosystem, and a
/// comparison such as `a<b` still parses under it, because JSX can only begin
/// where a value is expected.
///
/// Case is folded with the ASCII rule and no other: an upper-case extension is
/// a real thing on a case-insensitive filesystem, and `to_lowercase` would ask
/// the host's locale, which is what the repository bans it for.
///
/// Held level with the JavaScript graph by the `parserDialect` section of
/// `tests/fixtures/module-kind-conformance.json`.
pub(crate) fn jsx_is_enabled(extension: &str, source_looks_like_jsx: bool) -> bool {
    match extension
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "tsx" | "jsx" => true,
        "ts" | "mts" | "cts" => false,

        _ => source_looks_like_jsx,
    }
}

fn substitute_public_env(code: &str) -> String {
    const MARKER: &str = "import.meta.env";
    if !code.contains(MARKER) {
        return code.to_string();
    }

    // Asked of the code, so a mention inside a string or a comment is left as
    // the text it is.
    let masked = crate::ast::masked_code(code);
    let mut out = String::with_capacity(code.len());
    let literal = public_env_literal();
    let mut cursor = 0usize;
    for (at, _) in masked.match_indices(MARKER) {
        out.push_str(&code[cursor..at]);
        out.push_str(&literal);
        cursor = at + MARKER.len();
    }
    out.push_str(&code[cursor..]);
    out
}

/// The project's public environment, as the host loaded it.
///
/// A build reads `.env` and `.env.local` itself and hands the values to the
/// processes that need them; it does not put them in its own environment, so
/// this cannot be answered from `std::env` alone — a client bundle built that
/// way substituted `Object.freeze({})` for every project that keeps its public
/// values in a file, which is all of them.
static PUBLIC_ENV: std::sync::OnceLock<std::collections::BTreeMap<String, String>> =
    std::sync::OnceLock::new();

/// Record the project environment this process compiles against.
///
/// Called once by the host before any bundling; a second call is ignored, so a
/// caller cannot change what earlier output was built from.
pub fn set_public_env(values: std::collections::BTreeMap<String, String>) {
    let _ = PUBLIC_ENV.set(values);
}

/// `Object.freeze({...})` over every `RUVYXA_PUBLIC_*` value, in name order.
fn public_env_literal() -> String {
    let mut entries: Vec<(String, String)> = PUBLIC_ENV
        .get()
        .map(|values| {
            values
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| std::env::vars().collect())
        .into_iter()
        .filter(|(name, _)| name.starts_with("RUVYXA_PUBLIC_"))
        .collect();
    entries.sort();
    let body = entries
        .iter()
        .map(|(name, value)| {
            format!(
                "{}:{}",
                crate::output::js_string(name),
                crate::output::js_string(value)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("Object.freeze({{{body}}})")
}

/// Refuse output that depends on a helper runtime Ruvyxa does not deliver.
///
/// Downlevelling is not free of runtime support: oxc's `HelperLoaderMode`
/// defaults to `Runtime`, so a transform that needs a helper emits
/// `import _x from "@oxc-project/runtime/helpers/x"`. That package is in
/// neither module graph, `Inline` mode is `unreachable!()` inside oxc, and a
/// deployed function bundle resolves no bare specifiers at all — so the import
/// would reach production as a module that cannot be found.
///
/// Which targets need a helper is a property of the *source*, not of the
/// number: ordinary application code compiles helper-free down to about
/// es2022, private class fields pull helpers in from es2021 down, and a single
/// `using` declaration needs one at every target below es2026. So the check is
/// on what was emitted, and it names the helpers so the message says what the
/// module actually used.
///
/// The scan goes through [`crate::ast`] rather than a substring test: a module
/// whose *source* mentions the specifier in a string or a comment has not
/// imported anything.
fn reject_runtime_helpers(code: &str, es_target: EsTarget) -> std::result::Result<(), String> {
    if es_target.is_default() {
        return Ok(());
    }
    let mut helpers = ast::parse_module(code)
        .import_specifiers()
        .into_iter()
        .filter_map(|specifier| {
            specifier
                .strip_prefix(HELPER_RUNTIME_PREFIX)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    if helpers.is_empty() {
        return Ok(());
    }
    helpers.sort_unstable();
    helpers.dedup();
    Err(format!(
        "build.target `{es_target}` needs the runtime helpers {} for this module, and Ruvyxa ships no helper runtime — raise build.target (ordinary application code compiles helper-free at es2022 and above) or remove the syntax that needs downlevelling",
        helpers.join(", ")
    ))
}

/// Specifier prefix oxc emits for every helper import it adds.
const HELPER_RUNTIME_PREFIX: &str = "@oxc-project/runtime/helpers/";

/// True when some line's first non-blank character is `@`.
///
/// A decorator is always the first thing on its line, so a file without such a
/// line has nothing to strip. Answering that with a plain line scan lets the
/// overwhelming majority of modules skip the tokenizer below entirely — they
/// are returned byte-for-byte, and no tokenizer limitation can reach them.
fn has_decorator_candidate(source: &str) -> bool {
    source
        .lines()
        .any(|line| line.trim_start().starts_with('@'))
}

/// Strip legacy decorators while preserving source line positions.
///
/// Oxc rejects legacy decorators, so they are removed before it parses. Finding
/// them means knowing whether an `@` is code, and getting that wrong is
/// expensive: a misplaced `@` deletion produces an unterminated string and a
/// parse failure that names the file rather than the construct that confused
/// the scan.
///
/// That question is not answered here. This used to carry its own tokenizer for
/// strings, template literals, and comments, which made it the crate's second
/// byte scanner — and it had exactly the half of the rules [`crate::ast`] was
/// missing, while missing the half `ast` had. It knew a `'` that does not close
/// before end-of-line is an apostrophe, not a delimiter; it did not know what a
/// regular expression is, so `` /`/ `` opened a template state that never
/// closed and the decorator on the next line survived into Oxc. Both halves now
/// live in the one scanner, and this asks it with [`ModuleAst::is_code_offset`]
/// instead of re-deriving the answer.
///
/// A removed decorator leaves behind exactly the newlines it spanned, so every
/// later line keeps its original number.
fn strip_decorators(source: &str) -> String {
    // Only for callers with no plan of their own. A plain line scan settles the
    // overwhelming majority of modules without parsing anything: a decorator is
    // always the first thing on its line.
    if !has_decorator_candidate(source) {
        return source.to_string();
    }

    strip_decorators_with_plan(source, &ast::parse_module(source))
}

/// Strip decorators using facts the caller already collected.
fn strip_decorators_with_plan(source: &str, ast: &ModuleAst) -> String {
    if !ast.has_decorators {
        return source.to_string();
    }

    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut out = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        if bytes[i] == b'@' && begins_decorator(bytes, i) && ast.is_code_offset(i) {
            let end = skip_decorator(bytes, i, ast);
            // Emit the newlines the decorator spanned and nothing else. A
            // decorator on its own line leaves the line blank; a multi-line
            // `@Component({...})` leaves as many blank lines as it occupied.
            // Every later line then keeps its original number, which is what
            // source maps and Oxc's diagnostics are read against.
            let spanned_lines = bytes[i..end].iter().filter(|byte| **byte == b'\n').count();
            out.resize(out.len() + spanned_lines, b'\n');
            i = end;
            continue;
        }

        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8(out).expect("scanner copies whole UTF-8 sequences verbatim")
}

/// True when only blanks separate `at` from the start of its line.
/// Blank a leading `#!` line, keeping the line itself.
///
/// A shebang is legal at the top of a module and Node removes it when it loads
/// one directly — but a bundled module is wrapped in a function, where `#!` is
/// a syntax error. Packages that double as executables ship one on their entry
/// file, so importing such a package failed the build with a parse error that
/// named a character rather than a cause.
///
/// The line is emptied rather than deleted, so every line below keeps its
/// number and diagnostics still address the line the author wrote.
fn strip_shebang(source: &str) -> String {
    let start = source
        .strip_prefix('\u{feff}')
        .map_or(0, |_| '\u{feff}'.len_utf8());
    if !source[start..].starts_with("#!") {
        return source.to_string();
    }
    match source[start..].find('\n') {
        Some(offset) => format!("{}{}", &source[..start], &source[start + offset..]),
        None => source[..start].to_string(),
    }
}

/// Whether an `@` in a code position begins a decorator.
///
/// `@` is not an operator in JavaScript, so a code-position one is a decorator
/// either way — what matters is only that it is not part of a larger token.
/// Requiring it to start its own line was the earlier rule, and it left
/// `class Svc { @log run() {} }` untouched: the shape a formatter picks for a
/// short member, and the shape every minified dependency has. That decorator
/// then reached the emitted bundle, which is plain JavaScript and has no such
/// syntax.
fn begins_decorator(bytes: &[u8], at: usize) -> bool {
    let Some(previous) = bytes[..at]
        .iter()
        .rev()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    else {
        // Nothing before it in the file: a decorator on the first declaration.
        return true;
    };
    // A decorator follows the start of a class body, the end of a previous
    // member, another decorator, or the keyword before an exported class.
    matches!(previous, b'{' | b'}' | b';' | b')' | b']')
        || previous.is_ascii_alphanumeric()
        || matches!(previous, b'_' | b'$')
}

/// Return the index just past a decorator that begins at `at`.
///
/// Only parentheses in code positions close the argument list. A `)` inside a
/// string, template, comment, or regex within the arguments is text, and `ast`
/// is the one place that already knows which is which.
fn skip_decorator(bytes: &[u8], at: usize, ast: &ModuleAst) -> usize {
    let len = bytes.len();
    let mut i = at + 1;
    // Identifier bytes. Anything above ASCII is a continuation byte of a
    // non-ASCII identifier character, which JavaScript allows in a name.
    while i < len
        && (bytes[i].is_ascii_alphanumeric()
            || matches!(bytes[i], b'_' | b'$' | b'.')
            || bytes[i] >= 0x80)
    {
        i += 1;
    }
    if i >= len || bytes[i] != b'(' {
        return i;
    }

    let mut depth = 1usize;
    i += 1;
    while i < len && depth > 0 {
        if ast.is_code_offset(i) {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both compilers accept the same set of `build.target` values.
    ///
    /// The JavaScript half is
    /// `tests/packages/ruvyxa/es-target-contract.test.mjs` over
    /// `resolveEsTarget` in `packages/ruvyxa/runtime/compiler.mjs`. A value one
    /// graph accepts and the other refuses is a build that succeeds on the
    /// client and fails at prerender.
    #[test]
    fn accepted_targets_match_the_shared_conformance_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/es-target-conformance.json"
        ))
        .unwrap();

        let accepted = fixture["accepted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            accepted,
            EsTarget::ALL
                .iter()
                .map(|target| target.as_str())
                .collect::<Vec<_>>(),
            "the accepted list and its order are part of the diagnostic both hosts print"
        );

        for (written, expected) in fixture["aliases"].as_object().unwrap() {
            assert_eq!(
                EsTarget::parse(written).map(EsTarget::as_str),
                expected.as_str(),
                "alias case disagrees with the shared fixture: {written:?}"
            );
        }

        for value in fixture["rejected"].as_array().unwrap() {
            let written = value.as_str().unwrap();
            assert!(
                EsTarget::parse(written).is_none(),
                "the shared fixture refuses {written:?} and this host accepted it"
            );
        }
    }

    /// Every target this crate advertises is one oxc accepts.
    ///
    /// `EsTarget::ALL` mirrors oxc's `ESTarget`, and a list that mirrors
    /// another construct has to be checked against that construct rather than
    /// against a comment promising the two agree.
    #[test]
    fn every_accepted_es_target_is_one_oxc_accepts() {
        for target in EsTarget::ALL {
            assert_eq!(
                EsTarget::parse(target.as_str()),
                Some(target),
                "{target} does not round-trip through parse"
            );
            assert!(
                TransformOptions::from_target(target.as_str()).is_ok(),
                "oxc rejects `{target}`, which this crate advertises as accepted"
            );
        }
        // The alias oxc accepts, accepted here too so a config that uses it is
        // not told the value is unknown.
        assert_eq!(EsTarget::parse("ES6"), Some(EsTarget::Es2015));
        // oxc does not implement es5, so advertising it would be a target that
        // fails at transform time rather than at config time.
        assert_eq!(EsTarget::parse("es5"), None);
        assert!(TransformOptions::from_target("es5").is_err());
    }

    /// The target reaches the emitted bytes, and helper-dependent output is
    /// refused rather than shipped.
    ///
    /// `es2021` was chosen because a private class field is the cheapest way to
    /// make oxc reach for `@oxc-project/runtime`: nothing resolves that
    /// specifier in either module graph, and a deployed function bundle
    /// resolves no bare specifiers at all.
    #[test]
    fn a_downlevel_target_changes_the_output_and_refuses_helper_imports() {
        let plain = "export const pick = (input) => { input.count ??= 0; return input }";
        let esnext =
            transform_with_plan(plain, false, JsxRuntime::Automatic, EsTarget::EsNext, None)
                .unwrap();
        let downlevel =
            transform_with_plan(plain, false, JsxRuntime::Automatic, EsTarget::Es2020, None)
                .unwrap();
        assert!(esnext.contains("??="), "esnext must emit the source syntax");
        assert!(
            !downlevel.contains("??="),
            "es2020 must downlevel logical assignment: {downlevel}"
        );

        let private_field =
            "export class Counter { #value = 0; get value() { return this.#value } }";
        let error = transform_with_plan(
            private_field,
            false,
            JsxRuntime::Automatic,
            EsTarget::Es2021,
            None,
        )
        .expect_err("helper-dependent output must be refused, not emitted");
        assert!(error.contains("build.target `es2021`"), "{error}");
        assert!(error.contains("classPrivateFieldInitSpec"), "{error}");

        // The same module at the default target is not affected.
        transform_with_plan(
            private_field,
            false,
            JsxRuntime::Automatic,
            EsTarget::EsNext,
            None,
        )
        .expect("esnext needs no helper");
    }

    /// A module that merely *mentions* the helper specifier has imported
    /// nothing.
    ///
    /// The guard reads [`crate::ast`] rather than matching the output text,
    /// because a string literal and an import are the same characters.
    #[test]
    fn the_helper_guard_reads_imports_rather_than_text() {
        let source = "export const note = \"@oxc-project/runtime/helpers/typeof\"";
        transform_with_plan(source, false, JsxRuntime::Automatic, EsTarget::Es2020, None)
            .expect("a quoted specifier is not an import");
    }

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
    fn indented_decorators_are_still_stripped() {
        let out = strip_decorators("class Service {\n  @observable()\n  value = 1;\n}");
        assert!(!out.contains("@observable"));
        assert!(out.contains("value = 1;"));
    }

    /// An apostrophe in prose is not a string delimiter. Reading it as one used
    /// to leave the scanner mis-synchronized for the rest of the file, so the
    /// `@` of the next scoped import was deleted as a decorator and the module
    /// no longer parsed.
    #[test]
    fn apostrophes_in_comments_do_not_shift_the_scan() {
        let source = concat!(
            "/**\n",
            " * React's head hoisting.\n",
            " */\n",
            "import { Seo } from '@ruvyxa/react';\n",
            "export default function Page() { return Seo; }\n",
        );
        let out = strip_decorators(source);
        assert!(
            out.contains("'@ruvyxa/react'"),
            "scoped specifier must survive: {out}"
        );
        assert_eq!(out, source, "a file with no decorator must be unchanged");
    }

    /// Passing a caller's plan must not change the answer. Two paths that
    /// compute the same thing are how the regex/apostrophe split survived this
    /// long, so the reuse path is held to the self-parsing one directly.
    #[test]
    fn a_caller_supplied_plan_matches_self_parsing() {
        for source in [
            "const tick = /`/;\n@Injectable()\nclass S {}\n",
            "const label = <p>don't</p>;\n@Injectable()\nclass S {}\n",
            "@Component({\n  selector: 'x',\n})\nclass S {}\n",
            "@Inject$ed({ a: ')', b: `)` })\nclass S {}\n",
            "class S {\n  css = `\n@media (x) {}\n`;\n}\n",
            "const email = 'user@example.com';\n",
            "// ค่าเริ่มต้น\n@Injectable()\nclass บริการ {}\n",
        ] {
            assert_eq!(
                strip_decorators_with_plan(source, &ast::parse_module(source)),
                strip_decorators(source),
                "reuse path diverged for:\n{source}"
            );
        }
    }

    /// A regex literal is not a template, a comment, or a string. When this
    /// carried its own tokenizer it had no regex state, so a regex body holding
    /// a `` ` ``, a `/*`, or a `//` opened a state that never closed and the
    /// decorator on the next line reached Oxc, which rejects it.
    #[test]
    fn regex_literals_do_not_hide_a_later_decorator() {
        for source in [
            "const tick = /`/;\n@Injectable()\nclass S {}\n",
            "const open = /[/*]/;\n@Injectable()\nclass S {}\n",
            "const line = /[//]/;\n@Injectable()\nclass S {}\n",
        ] {
            let out = strip_decorators(source);
            assert!(!out.contains("@Injectable"), "decorator survived:\n{out}");
            assert!(out.contains("class S {}"), "body lost:\n{out}");
            assert_eq!(
                source.lines().count(),
                out.lines().count(),
                "line drift for:\n{source}"
            );
        }
    }

    #[test]
    fn line_comment_apostrophes_do_not_shift_the_scan() {
        let source = "// doesn't matter\nimport x from '@scope/pkg';\n";
        assert_eq!(strip_decorators(source), source);
    }

    /// `@` inside a string or after other code is never a decorator.
    #[test]
    fn at_signs_that_do_not_start_a_line_are_preserved() {
        let source = "const to = 'user@example.com';\nconst pkg = '@scope/name';\n";
        assert_eq!(strip_decorators(source), source);
    }

    #[test]
    fn transform_accepts_comment_apostrophes_before_scoped_imports() {
        let source = concat!(
            "/* it's fine */\n",
            "import { Seo } from '@ruvyxa/react';\n",
            "export const meta = { title: 'x' };\n",
            "export default function Page() { return Seo; }\n",
        );
        assert!(
            transform(source, false).is_ok(),
            "comment apostrophe must not break parsing"
        );
    }

    /// The overwhelming majority of modules have no decorator. They must come
    /// back byte-for-byte, whatever they contain.
    #[test]
    fn sources_without_a_decorator_are_returned_unchanged() {
        for source in [
            "const label = <p>don't</p>;\nconst re = /['\"]/g;\n",
            "const css = `\n@media (min-width: 40rem) { a { color: red } }\n`;\n",
            "const email = 'user@example.com';\nconst pkg = '@scope/name';\n",
            "// it's fine\nimport x from '@scope/pkg';\n",
            "const s = \"a ` backtick inside a string\";\nconst t = 'and @here too';\n",
        ] {
            assert_eq!(strip_decorators(source), source, "source: {source}");
        }
        assert!(!has_decorator_candidate("const pkg = '@scope/name';\n"));
        assert!(has_decorator_candidate(
            "class S {\n  @observable x = 1;\n}\n"
        ));
    }

    /// An unclosed quote is prose, not a string. It must not carry the scan
    /// past its own line, or the decorator after it goes unstripped.
    #[test]
    fn unclosed_quotes_do_not_hide_a_later_decorator() {
        for source in [
            "const label = <p>don't</p>;\n@Injectable()\nclass S {}\n",
            "const re = /['\"]/g;\n@Injectable()\nclass S {}\n",
            "// TODO: it's broken\n@Injectable()\nclass S {}\n",
        ] {
            let out = strip_decorators(source);
            assert!(!out.contains("@Injectable"), "decorator survived:\n{out}");
            assert!(out.contains("class S {}"), "body lost:\n{out}");
        }
    }

    /// A line-leading `@` inside a template literal is CSS or prose, not a
    /// decorator, including across an interpolation.
    #[test]
    fn line_leading_at_inside_a_template_is_left_alone() {
        let source = concat!(
            "@Injectable()\n",
            "class S {\n",
            "  css = `\n",
            "@media (min-width: ${size}rem) {\n",
            "  a { content: '}' }\n",
            "}\n",
            "`;\n",
            "}\n",
        );
        let out = strip_decorators(source);
        assert!(!out.contains("@Injectable"), "decorator kept:\n{out}");
        assert!(out.contains("@media"), "template text lost:\n{out}");
        assert!(out.contains("${size}"), "interpolation lost:\n{out}");
    }

    #[test]
    fn decorator_arguments_and_dollar_names_are_consumed_whole() {
        let out = strip_decorators("@Inject$ed({ a: ')', b: `)` })\nclass S {}\n");
        assert_eq!(out, "\nclass S {}\n");
    }

    /// Every later line has to keep its original number, or Oxc's diagnostics
    /// and the emitted source map point at the wrong line.
    #[test]
    fn stripping_preserves_the_line_count() {
        for source in [
            "@A()\n@B\nclass S {\n  @C() m() {}\n}\n",
            "@Component({\n  selector: 'x',\n})\nclass S {}\n",
            "class S {\n  @observable()\n  value = 1;\n}\n",
        ] {
            let out = strip_decorators(source);
            assert!(!out.contains('@'), "decorators remain:\n{out}");
            assert_eq!(
                source.lines().count(),
                out.lines().count(),
                "line drift for:\n{source}"
            );
        }
    }

    #[test]
    fn non_ascii_source_survives_the_byte_scan() {
        let source = "// ค่าเริ่มต้น — em dash\n@Injectable()\nclass บริการ {}\n";
        let out = strip_decorators(source);
        assert!(out.contains("ค่าเริ่มต้น"));
        assert!(out.contains("class บริการ {}"));
        assert!(!out.contains("@Injectable"));
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

    /// Regression: a JSON dependency used to reach the JavaScript transform and
    /// fail as a syntax error inside someone else's package.
    #[test]
    fn compiles_json_into_a_commonjs_module() {
        let js = compile_json_module(
            r#"{ "name": "fake-sdk", "version": "4.2.1" }"#,
            Path::new("fake-sdk/package.json"),
        )
        .unwrap();

        assert!(js.contains("module.exports = JSON.parse("));
        assert!(js.contains("Object.defineProperty(module.exports, 'default'"));
        // The payload travels as one string literal, so nothing inside it can be
        // parsed as code.
        assert!(js.contains(r#"\"version\":\"4.2.1\""#), "{js}");
    }

    #[test]
    fn leaves_a_documents_own_default_key_alone() {
        let js = compile_json_module(r#"{ "default": "mine" }"#, Path::new("keyed.json")).unwrap();
        assert!(
            !js.contains("Object.defineProperty"),
            "overwriting a `default` key would change data the application reads: {js}"
        );
    }

    #[test]
    fn a_json_scalar_needs_no_default_self_reference() {
        let js = compile_json_module("42", Path::new("scalar.json")).unwrap();
        assert!(!js.contains("Object.defineProperty"), "{js}");
        assert!(js.contains("JSON.parse(\"42\")"), "{js}");
    }

    #[test]
    fn invalid_json_is_a_json_diagnostic() {
        let error = compile_json_module("{ \"a\": }", Path::new("broken.json"))
            .expect_err("malformed JSON must not compile");
        let message = error.to_string();
        assert!(message.contains("RUV1805"), "{message}");
        assert!(message.contains("broken.json"), "{message}");
    }

    #[test]
    fn an_uncompilable_module_kind_is_named_rather_than_parsed() {
        let error = reject_unsupported_module_kind("node", Path::new("native.node"))
            .expect_err("a native addon has no compilation path");
        let message = error.to_string();
        assert!(message.contains("RUV1806"), "{message}");
        assert!(message.contains("native.node"), "{message}");
    }

    #[test]
    fn known_module_kinds_and_extensionless_entries_pass_through() {
        for ext in MODULE_KIND_EXTENSIONS {
            reject_unsupported_module_kind(ext, Path::new("module")).unwrap();
        }
        // A package entry point without an extension is JavaScript by Node's
        // own rules.
        reject_unsupported_module_kind("", Path::new("bin/cli")).unwrap();
    }

    /// Both module graphs compile the same set of extensions.
    ///
    /// The JavaScript half is
    /// `tests/packages/ruvyxa/module-kind-contract.test.mjs` over
    /// `assertSupportedModuleKind` in `packages/ruvyxa/runtime/compiler.mjs`.
    /// The two lists cannot be shared as code, and the doc comment on
    /// [`MODULE_KIND_EXTENSIONS`] used to be the only thing asking them to
    /// agree — the arrangement `tests/fixtures/` exists to replace. An
    /// extension accepted here and refused there is a build that passes on the
    /// client and fails at prerender.
    /// The dialect half of the same table.
    ///
    /// This graph used to decide JSX from its own scan, which turned ordinary
    /// TypeScript — a generic arrow, an angle-bracket assertion — into a parse
    /// error that the Node graph did not produce for the same file.
    #[test]
    fn parser_dialect_matches_the_shared_conformance_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/module-kind-conformance.json"
        ))
        .unwrap();

        let cases = fixture["parserDialect"]
            .as_array()
            .expect("parserDialect cases");
        assert!(!cases.is_empty(), "the table must carry cases");

        for case in cases {
            let extension = case["extension"].as_str().expect("extension");
            let source = case["source"].as_str().expect("source");
            let expected = case["jsx"].as_bool().expect("jsx");
            let why = case["why"].as_str().unwrap_or_default();
            let scanned = ast::parse_module(source).has_jsx;
            assert_eq!(
                jsx_is_enabled(extension, scanned),
                expected,
                "{extension}: {why}"
            );
        }
    }

    #[test]
    fn compilable_module_kinds_match_the_shared_conformance_table() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/module-kind-conformance.json"
        ))
        .unwrap();

        // The fixture writes extensions the way a file name does, with the
        // leading dot; this side stores them without one.
        let expected = fixture["extensions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().trim_start_matches('.'))
            .collect::<Vec<_>>();
        assert_eq!(
            expected, MODULE_KIND_EXTENSIONS,
            "the compilable extension list disagrees with the shared fixture; the RUV1806 message both hosts print lists it in this order"
        );

        for value in fixture["acceptedCasing"].as_array().unwrap() {
            let written = value.as_str().unwrap().trim_start_matches('.');
            reject_unsupported_module_kind(written, Path::new("module")).unwrap_or_else(|error| {
                panic!("upper-case extension .{written} must compile: {error}")
            });
        }

        for value in fixture["rejected"].as_array().unwrap() {
            let written = value.as_str().unwrap().trim_start_matches('.');
            assert!(
                reject_unsupported_module_kind(written, Path::new("module")).is_err(),
                "the shared fixture refuses .{written}, this graph accepts it"
            );
        }

        assert_eq!(
            fixture["extensionlessIsAccepted"].as_bool(),
            Some(true),
            "the fixture describes the extensionless case both hosts allow"
        );
        reject_unsupported_module_kind("", Path::new("bin/cli")).unwrap();
    }

    /// Sources whose `@` is not a decorator, and decorators that are hard to
    /// measure.
    ///
    /// Stripping deletes bytes, so the failure mode is code that silently loses
    /// a piece of itself — a `@media` block cut out of a styled-components
    /// template still parses and still renders, just without the media query.
    /// Each case lists the fragments that must survive verbatim.
    const DECORATOR_CORPUS: &[(&str, &str, &[&str])] = &[
        (
            "css at-rule inside a styled template",
            "const Box = styled.div`\n  color: red;\n  @media (max-width: 600px) {\n    color: blue;\n  }\n`\nexport const value = Box\n",
            &["@media (max-width: 600px)", "color: blue"],
        ),
        (
            "keyframes and supports inside a template",
            "const css = `\n  @supports (display: grid) { display: grid }\n  @keyframes spin { from { rotate: 0deg } }\n`\nexport const value = css\n",
            &["@supports", "@keyframes spin"],
        ),
        (
            "an email address in a comment",
            "// contact @someone (nobody@example.com)\nexport const value = 1\n",
            &["@someone", "nobody@example.com"],
        ),
        (
            "an at sign in a string",
            "export const handle = \"@ruvyxa/core\"\nexport const scoped = '@scope/pkg'\n",
            &["@ruvyxa/core", "@scope/pkg"],
        ),
        (
            "an at sign in jsx text",
            "export const el = <p>write to @support</p>\n",
            &["@support"],
        ),
        (
            "a jsdoc tag",
            "/**\n * @param {number} a\n * @returns {number}\n */\nexport function twice(a) { return a * 2 }\n",
            &["return a * 2"],
        ),
        (
            "decorator with a string holding a paren",
            "@Route(\"/a)b\")\nclass Service {}\nexport const value = Service\n",
            &["class Service", "export const value"],
        ),
        (
            "decorator with nested parens",
            "@Inject(factory(deps(1, 2), 3))\nclass Service {}\nexport const value = Service\n",
            &["class Service"],
        ),
        (
            "decorator with a template argument",
            "@Query(`select 1`)\nclass Service {}\nexport const value = Service\n",
            &["class Service"],
        ),
        (
            "stacked decorators",
            "@First()\n@Second\n@Third({ a: 1 })\nclass Service {}\nexport const value = Service\n",
            &["class Service"],
        ),
        (
            "member decorators",
            "class Service {\n  @Input() name = \"x\"\n  @Output() changed = 1\n  run() { return this.name }\n}\nexport const value = Service\n",
            &["name = \"x\"", "changed = 1", "run()"],
        ),
        (
            "decorator followed by a comment",
            "@Injectable() // wires the service\nclass Service {}\nexport const value = Service\n",
            &["class Service"],
        ),
        (
            "at sign inside a regex",
            "const p = /@[a-z]+/g\nexport const value = p.source\n",
            &["/@[a-z]+/g"],
        ),
    ];

    #[test]
    fn stripping_decorators_keeps_every_at_sign_that_is_not_one() {
        for (name, source, must_survive) in DECORATOR_CORPUS {
            let stripped = strip_decorators(source);
            for fragment in *must_survive {
                assert!(
                    stripped.contains(fragment),
                    "{name}: lost {fragment:?}\n--- stripped ---\n{stripped}"
                );
            }
            assert!(
                transform(&stripped, true).is_ok(),
                "{name}: stripped output does not parse\n{stripped}"
            );
        }
    }

    #[test]
    fn the_transform_accepts_the_corpus_and_its_own_output() {
        for (name, source, _) in DECORATOR_CORPUS {
            let output = transform(source, true).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(
                transform(&output, true).is_ok(),
                "{name}: transform output does not re-parse\n{output}"
            );
        }
    }

    /// A `.js` module the pass-through path hands to the linker must link.
    ///
    /// The linker rewrites one ESM statement per line and cannot span one. A
    /// `.ts` module always gets that shape from the transform; a `.js` one is
    /// passed through, and `expand_multi_statement_esm` is the only thing that
    /// gives it. The two predicates gating that expansion and the linker's
    /// requirement are three separate pieces of knowledge about the same rule,
    /// and they were not the same: `has_esm_statement_sharing_a_line` covered
    /// the minified direction only, so a Prettier-wrapped `export { … }` reached
    /// the linker intact and left a stray clause loose in the module IIFE — a
    /// bundle that does not parse, with no build error at all.
    ///
    /// This walks the whole path rather than asserting on a predicate, so the
    /// three cannot drift apart again without failing.
    #[test]
    fn every_javascript_shape_survives_expansion_and_linking() {
        for source in [
            // Minified: several statements per line.
            "const a=1;function f(){return a}export{a as A,f};\n",
            "export const value=1;export default value;\n",
            // Prettier-wrapped: one statement across several lines.
            "const a = 1\nconst b = 2\nexport {\n  a,\n  b as second,\n}\n",
            "import {\n  join,\n} from \"node:path\"\nexport const value = join\n",
            // Already one statement per line: expansion must not be needed.
            "export const a = 1\nexport function b() {}\n",
            // Shapes the linker spans deliberately; re-printing them would
            // rewrite working output for nothing.
            "export default {\n  name: \"x\",\n}\n",
            "export const config = {\n  name: \"x\",\n}\n",
            "export default class Page {\n  render() {}\n}\n",
            // Text that reads like a statement.
            "export const doc = `export {\n  a,\n}`\nexport const value = 1\n",
        ] {
            let expanded = expand_multi_statement_esm(source).unwrap_or_else(|| source.to_string());
            let entry = PathBuf::from("/p/dep.js");
            let module = CompiledModule::new(
                entry.clone(),
                expanded.clone(),
                Vec::new(),
                BTreeMap::new(),
                false,
                false,
            );
            // A re-export needs its dependency in the graph to rewrite; that
            // path has its own coverage in `linker.rs`. These cases are about
            // the shape reaching the linker at all.
            let input = BundleInput {
                entry,
                project_root: PathBuf::from("/p"),
                app_dir: PathBuf::from("/p/app"),
                layouts: Vec::new(),
                templates: Vec::new(),
                slots: Vec::new(),
                intercepts: Vec::new(),
                request_path: "/".to_string(),
                target: crate::BundleTarget::Client,
                options: crate::BundleOptions::default(),
                specials: crate::RouteSpecials::default(),
            };

            let linked = crate::linker::link(&[module], &input)
                .unwrap_or_else(|error| panic!("link rejected {source:?}: {error}\n{expanded}"));
            assert!(
                transform(&linked, false).is_ok(),
                "linked output does not parse for {source:?}\n{linked}"
            );
        }
    }
}

#[cfg(test)]
mod decorator_placement_tests {
    use super::*;

    /// A decorator is stripped wherever it is written, not only at the start of
    /// a line.
    ///
    /// Ruvyxa accepts decorators and removes them; the emitted bundle is plain
    /// JavaScript, which has no such syntax. Stripping only the line-leading
    /// form left `class Svc { @log run() {} }` — the shape a formatter produces
    /// for a short member, and the shape a minified dependency always has —
    /// intact in the output, where it is a syntax error.
    fn erased_syntax_fixture() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/source-scanner-conformance.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        serde_json::from_str::<serde_json::Value>(&source).expect("the fixture parses")
            ["erasedSyntax"]
            .clone()
    }

    /// A shebang is legal at the top of a module and illegal inside the wrapper
    /// every module is emitted into. Replayed against the shared table.
    #[test]
    fn shebangs_match_the_shared_conformance_table() {
        let fixture = erased_syntax_fixture();
        for source in fixture["shebang"]["stripped"].as_array().expect("stripped") {
            let source = source.as_str().expect("source");
            let stripped = strip_shebang(source);
            assert!(
                !stripped.contains("#!"),
                "a shebang survived:
{stripped}"
            );
            assert_eq!(
                stripped.lines().count(),
                source.lines().count(),
                "the line must be emptied, not removed:
{stripped}"
            );
        }
        for source in fixture["shebang"]["untouched"]
            .as_array()
            .expect("untouched")
        {
            let source = source.as_str().expect("source");
            assert_eq!(strip_shebang(source), source, "text was rewritten");
        }
    }

    /// The decorator half of the same table.
    #[test]
    fn decorators_match_the_shared_conformance_table() {
        let fixture = erased_syntax_fixture();
        for source in fixture["decorators"]["stripped"]
            .as_array()
            .expect("stripped")
        {
            let source = source.as_str().expect("source");
            let stripped = strip_decorators_with_plan(source, &ast::parse_module(source));
            assert!(
                !stripped.contains('@'),
                "a decorator survived stripping:
{stripped}"
            );
        }
        for source in fixture["decorators"]["untouched"]
            .as_array()
            .expect("untouched")
        {
            let source = source.as_str().expect("source");
            assert_eq!(
                strip_decorators_with_plan(source, &ast::parse_module(source)),
                source,
                "text was rewritten"
            );
        }
    }

    #[test]
    fn decorators_are_stripped_wherever_they_appear() {
        for source in [
            "class Svc {\n  @log\n  run() { return 1 }\n}\n",
            "class Svc { @log run() { return 1 } }\n",
            "class Svc {\n  @log()\n  run() { return 1 }\n}\n",
            "class Svc { @log({ a: 1 }) run() { return 1 } }\n",
            "@tag\nclass Svc { run() { return 1 } }\n",
        ] {
            let stripped = strip_decorators_with_plan(source, &ast::parse_module(source));
            assert!(
                !stripped.contains('@'),
                "a decorator survived stripping:\n{stripped}"
            );
            assert_eq!(
                stripped.lines().count(),
                source.lines().count(),
                "line numbers must survive so diagnostics and source maps still line up:\n{stripped}"
            );
        }
    }

    /// An `@` that is not a decorator is left alone.
    #[test]
    fn an_at_sign_in_text_is_not_a_decorator() {
        for source in [
            "const email = \"a@b.test\"\n",
            "// reach me @someone\n",
            "const pattern = /@handle/\n",
            "const doc = `mention @user here`\n",
        ] {
            let stripped = strip_decorators_with_plan(source, &ast::parse_module(source));
            assert_eq!(stripped, source, "text was rewritten: {stripped}");
        }
    }
}
