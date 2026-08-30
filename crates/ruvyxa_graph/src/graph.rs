use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruvyxa_diagnostics::{Result, normalized_canonical_path};

use crate::validate::{is_markdown_route, markdown_without_code_examples};

/// One module's source and the facts derived from a single scan of it.
///
/// `ast.rs` states the contract this upholds: "callers that also need imports
/// should call `parse_module` once and read both facts off the result." Route
/// validation needs three facts per module — imports, env reads, and whether a
/// default export exists — and used to reach each through its own
/// `source -> T` helper that called `parse_module` internally.
pub(crate) struct ParsedModule {
    /// Source as the validators see it: Markdown/MDX already has its fenced
    /// examples blanked out.
    pub(crate) source: Arc<str>,
    pub(crate) ast: ruvyxa_bundler::ast::ModuleAst,
}

/// Per-run cache of everything derived from a module's source text.
///
/// Reading and scanning a file is the expensive part of route discovery and
/// validation, and both walk overlapping graphs: a layout, and every component
/// it pulls in, is reachable from every route beneath it. Keying that work by
/// canonical path collapses it to once per file per run instead of growing as
/// `routes × shared modules`.
///
/// Reading through one place also makes the Markdown decision unskippable.
/// Masking used to be applied by each caller, and the edge walk was the one
/// that forgot: an `import './helpers'` shown inside a fenced example in a
/// `.md` page became a real graph edge, pulling that module into the client
/// graph and raising boundary diagnostics against code the page never runs.
/// Masking now happens at the single point where source is read, so no caller
/// can skip it.
#[derive(Default)]
pub(crate) struct ModuleCache {
    project_root: Option<PathBuf>,
    /// Memoized `normalized_canonical_path`, so the same route file reached
    /// through a walk path and through a resolved import is one entry, and the
    /// canonicalize syscall runs once per distinct spelling.
    canonical: BTreeMap<PathBuf, PathBuf>,
    pub(crate) modules: BTreeMap<PathBuf, Option<Arc<ParsedModule>>>,
    /// Masked code, built lazily: only rendering-strategy detection needs it,
    /// and it is a full second pass over the source.
    masked: BTreeMap<PathBuf, Arc<str>>,
    pub(crate) edges: BTreeMap<PathBuf, Arc<[PathBuf]>>,
    /// `tsconfig.json` path aliases, read once per run on first use.
    aliases: Option<Arc<ruvyxa_bundler::resolver::TsConfigPaths>>,
}

impl ModuleCache {
    pub(crate) fn in_root(root: &Path) -> Self {
        Self {
            project_root: Some(normalized_canonical_path(root)),
            ..Self::default()
        }
    }

    pub(crate) fn canonical(&mut self, file: &Path) -> PathBuf {
        if let Some(canonical) = self.canonical.get(file) {
            return canonical.clone();
        }
        let canonical = normalized_canonical_path(file);
        self.canonical.insert(file.to_path_buf(), canonical.clone());
        canonical
    }

    /// Source and parsed facts for `file`, or `None` when it cannot be read.
    ///
    /// An unreadable file caches the `None`, matching the previous behavior of
    /// skipping it, and stops the retry on every later walk.
    pub(crate) fn module(&mut self, file: &Path) -> Option<Arc<ParsedModule>> {
        let key = self.canonical(file);
        if let Some(cached) = self.modules.get(&key) {
            return cached.clone();
        }

        let parsed = fs::read_to_string(&key).ok().map(|source| {
            let source = if is_markdown_route(&key) {
                markdown_without_code_examples(&source)
            } else {
                source
            };
            Arc::new(ParsedModule {
                ast: ruvyxa_bundler::ast::parse_module(&source),
                source: Arc::from(source),
            })
        });
        self.modules.insert(key, parsed.clone());
        parsed
    }

    /// Like [`ModuleCache::module`], but reports why the read failed.
    ///
    /// A file the manifest lists as a route must exist; treating it as an
    /// empty module would silently drop its diagnostics.
    pub(crate) fn require(&mut self, file: &Path) -> Result<Arc<ParsedModule>> {
        if let Some(module) = self.module(file) {
            return Ok(module);
        }
        // Only reached on the error path, so re-reading to recover the real
        // `io::Error` costs nothing in the common case.
        Err(fs::read_to_string(file).unwrap_err().into())
    }

    /// Source with strings, template text, comments, and regex literals blanked.
    pub(crate) fn masked(&mut self, file: &Path) -> Option<Arc<str>> {
        let key = self.canonical(file);
        if let Some(cached) = self.masked.get(&key) {
            return Some(cached.clone());
        }

        let module = self.module(&key)?;
        let masked: Arc<str> = Arc::from(code_without_strings_and_comments(&module.source));
        self.masked.insert(key, masked.clone());
        Some(masked)
    }

    /// The project's `tsconfig.json` path aliases.
    ///
    /// The bundler's table, not a third one: this walk decides which modules a
    /// route can reach, and a module it cannot see is a module whose data
    /// fetching it reports as absent.
    pub(crate) fn aliases(&mut self) -> Option<Arc<ruvyxa_bundler::resolver::TsConfigPaths>> {
        if let Some(aliases) = &self.aliases {
            return Some(Arc::clone(aliases));
        }
        let root = self.project_root.clone()?;
        let loaded = Arc::new(ruvyxa_bundler::resolver::TsConfigPaths::load(&root));
        self.aliases = Some(Arc::clone(&loaded));
        Some(loaded)
    }

    /// Resolve a non-relative specifier through the project's path aliases.
    ///
    /// Returns `None` for a bare package specifier, which stays outside this
    /// walk — see [`collect_relative_graph`].
    pub(crate) fn aliased_import(&mut self, specifier: &str) -> Option<PathBuf> {
        let resolved = self.aliases()?.resolve(specifier)?;
        Some(self.canonical(&resolved))
    }

    /// Project imports declared by `file`, resolved to paths.
    ///
    /// Relative and aliased specifiers both land here. Only relative ones used
    /// to: `import { load } from '@/lib/data'` produced no edge at all, so a
    /// page whose data fetching lived one alias away looked to
    /// [`detect_render_strategy`] like a page that fetched nothing, and was
    /// pre-rendered at build time. The same import written `../../lib/data`
    /// stayed SSR. Which spelling a project uses is not a rendering decision.
    ///
    /// Both arms answer through the bundler — `resolve_specifier` for relative
    /// paths, `TsConfigPaths` for aliases — because this walk decides what
    /// `ruvyxa build` stages into `<out>/server/`, which strategy a route gets,
    /// and which modules `validate_app` boundary-checks. A specifier the
    /// compiler resolves and this walk does not is a module that is compiled but
    /// never copied, and the failure surfaces as a request-time `RUV1801`.
    ///
    /// There used to be a private probe here that built its candidates with
    /// `Path::with_extension` — the mistake the bundler's own doc comment names:
    /// substitution turns `./util.inspect` into `util.js`, a file nobody wrote,
    /// and never asks for `util.inspect.js`. It was also four extensions short
    /// (`mts`, `cts`, `mjs`, `cjs`). One probe order, one place to change it.
    pub(crate) fn edges(&mut self, file: &Path) -> Arc<[PathBuf]> {
        let key = self.canonical(file);
        if let Some(cached) = self.edges.get(&key) {
            return cached.clone();
        }

        let resolved: Arc<[PathBuf]> = match self.module(&key) {
            Some(module) => {
                let specifiers = module.ast.import_specifiers();
                let mut edges: Vec<PathBuf> = Vec::with_capacity(specifiers.len());
                for specifier in specifiers {
                    let edge = if specifier.starts_with('.') {
                        key.parent().and_then(|directory| {
                            ruvyxa_bundler::resolver::resolve_specifier(directory, &specifier)
                        })
                    } else {
                        self.aliased_import(&specifier)
                    };
                    if let Some(edge) = edge {
                        edges.push(edge);
                    }
                }
                let provider = self.project_root.as_deref().and_then(|root| {
                    ruvyxa_bundler::content::resolve_mdx_components_file_in_root(&key, root)
                });
                if let Some(provider) = provider {
                    edges.push(normalized_canonical_path(&provider));
                }
                edges.into()
            }
            None => Arc::from([] as [PathBuf; 0]),
        };
        self.edges.insert(key, resolved.clone());
        resolved
    }
}

pub(crate) fn collect_relative_graph(entry: &Path, cache: &mut ModuleCache) -> BTreeSet<PathBuf> {
    let mut visited = BTreeSet::new();
    // Normalize the entry exactly like resolved imports so a cycle back to
    // the entry file compares equal instead of being visited twice.
    let mut queue = VecDeque::from([cache.canonical(entry)]);

    while let Some(file) = queue.pop_front() {
        if !visited.insert(file.clone()) {
            continue;
        }

        queue.extend(cache.edges(&file).iter().cloned());
    }

    visited
}

/// Statically-known `process.env` reads that must not reach the browser.
///
/// The reads come from the bundler's scanner and the rule that judges them comes
/// from the bundler's boundary check, so `check` and `build` cannot disagree
/// about either which env vars a module touches or which of them are allowed.
///
/// Both halves used to be local. The scan was a private marker search over a
/// privately-masked copy of the source; the rule was a hand-copied filter that
/// had lost the `NODE_ENV` exemption, so `check` rejected with RUV1008 what
/// `build` compiled without complaint.
pub(crate) fn private_env_reads(
    ast: &ruvyxa_bundler::ast::ModuleAst,
) -> impl Iterator<Item = &str> {
    ast.env_reads
        .iter()
        .map(String::as_str)
        .filter(|name| ruvyxa_bundler::boundary::env_read_is_private(name))
}

/// Blank out strings, template text, comments, and regular-expression literals.
///
/// Rendering-strategy detection matches on code *text* — `export const
/// revalidate`, `fetch(`, `process.env.` — so it needs masked source rather than
/// structured facts, and byte offsets and line breaks are preserved for it.
///
/// The masking is the bundler's, which is the point. This file used to carry a
/// character-wise lexer of its own: a duplicate `regex_can_start`, a duplicate
/// template-literal walk, a duplicate comment skipper. A bug in that copy read
/// `/['"]/` as a division followed by an unterminated string and blanked
/// everything after it, silently disabling RUV1007/RUV1008/RUV1010 for the
/// module. One scanner cannot drift from itself.
pub(crate) fn code_without_strings_and_comments(source: &str) -> String {
    ruvyxa_bundler::ast::masked_code(source)
}
