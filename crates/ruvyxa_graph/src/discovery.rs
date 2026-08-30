use std::path::{Component, Path, PathBuf};

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError, normalized_canonical_path};
use walkdir::WalkDir;

use crate::conflicts::{
    detect_conflicts, detect_server_component_conflicts, detect_unreachable_intercepts,
};
use crate::graph::ModuleCache;
use crate::manifest::{
    I18nRouting, RenderMeta, RenderStrategy, RouteEntry, RouteKind, RouteManifest,
};
use crate::parallel::{reject_intercepting_routes, route_intercepts, route_slots};
use crate::render::{apply_rendering_defaults, detect_render_strategy, detect_runtime_target};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverOptions {
    pub app_dir: PathBuf,
    pub default_render_strategy: Option<RenderStrategy>,
    pub default_revalidate: Option<u64>,
    pub i18n: Option<I18nRouting>,
}

impl DiscoverOptions {
    pub fn new(app_dir: impl Into<PathBuf>) -> Self {
        Self {
            app_dir: app_dir.into(),
            default_render_strategy: None,
            default_revalidate: None,
            i18n: None,
        }
    }

    pub fn with_rendering_defaults(
        mut self,
        default_render_strategy: Option<RenderStrategy>,
        default_revalidate: Option<u64>,
    ) -> Self {
        self.default_render_strategy = default_render_strategy;
        self.default_revalidate = default_revalidate;
        self
    }

    pub fn with_i18n(mut self, i18n: Option<I18nRouting>) -> Self {
        self.i18n = i18n;
        self
    }
}

/// Extensions a route file written as a component may carry, in probe order.
///
/// One table, because this set used to be spelled at five independent sites in
/// this file and two of them — the layout chain and the template chain — spelled
/// it `tsx` only. A project written in `.jsx`, which is a shape this same file
/// accepts for `page` and for a slot's `default`, therefore lost *every* layout
/// and template: no diagnostic, a successful build, and a page rendered without
/// its `<html>`/`<body>` shell.
///
/// `.tsx` is first everywhere this is probed, so a project holding a stray
/// `layout.jsx` beside its `layout.tsx` composes the file it always did.
/// `tests/fixtures/route-chain-conformance.json` holds that order across both
/// hosts.
pub(crate) const COMPONENT_EXTENSIONS: &[&str] = &["tsx", "jsx"];

/// Extensions only a `page` may carry: a Markdown document compiles to a page
/// and to nothing else, so it is not part of [`COMPONENT_EXTENSIONS`].
pub(crate) const PAGE_MARKUP_EXTENSIONS: &[&str] = &["md", "mdx"];

/// Extensions a request handler may carry. No JSX, so not a component.
pub(crate) const HANDLER_EXTENSIONS: &[&str] = &["ts", "js"];

/// The file names `stem` may take, in probe order.
pub(crate) fn route_file_names(stem: &str, extensions: &[&str]) -> Vec<String> {
    extensions
        .iter()
        .map(|extension| format!("{stem}.{extension}"))
        .collect()
}

/// Whether `file_name` is `stem` under one of `extensions`.
pub(crate) fn is_route_file(file_name: &str, stem: &str, extensions: &[&str]) -> bool {
    file_name
        .strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|extension| extensions.contains(&extension))
}

/// Every file name that declares a page, in probe order.
pub(crate) fn page_file_names() -> Vec<String> {
    let mut names = route_file_names("page", COMPONENT_EXTENSIONS);
    names.extend(route_file_names("page", PAGE_MARKUP_EXTENSIONS));
    names
}

/// Whether `file_name` declares a page.
pub(crate) fn is_page_file(file_name: &str) -> bool {
    is_route_file(file_name, "page", COMPONENT_EXTENSIONS)
        || is_route_file(file_name, "page", PAGE_MARKUP_EXTENSIONS)
}

/// A directory in the route tree that could not be read.
///
/// `WalkDir` reports a per-entry failure as an `Err` item and `fs::read_dir`
/// reports one as an `Err` return, and every walk in this crate used to drop
/// them — `.filter_map(Result::ok)`, `let Ok(entries) = … else { return … }`,
/// `file_type().is_ok_and(…)`. All three read "I could not look" as "there is
/// nothing there", which is a different answer with the same shape:
///
/// - in [`discover_routes`] the routes below the directory simply do not exist
///   as far as the build is concerned — no diagnostic, and a 404 in production;
/// - in [`crate::parallel::reject_intercepting_routes`] the *refusal* is what
///   is skipped, so an intercepting-route folder in an unreadable subtree
///   passes validation and mounts a publicly reachable page at a URL the author
///   wrote as an overlay and never meant to publish;
/// - in [`crate::parallel::intercept_pages`] and the two slot walks an
///   interception or a parallel-route slot silently disappears from the entry.
///
/// So it is reported. `app/` is the project's own source tree, not an ambient
/// part of the file system: a directory the build cannot look inside is a
/// question about the project, and answering it with silence is what the three
/// outcomes above have in common.
pub(crate) fn unreadable_route_directory(path: Option<&Path>, reason: &str) -> RuvyxaError {
    let named = path.map_or_else(
        || "a directory under the app directory".to_string(),
        |path| format!("`{}`", path.display()),
    );
    let mut diagnostic = Diagnostic::new("RUV1021", "Route directory could not be read")
        .explain(format!(
            "Ruvyxa could not read {named} while walking the app directory: {reason}. A directory the build cannot look inside is not an empty one: every route below it would be missing with nothing said, and an intercepting route below it would never be refused."
        ))
        .suggest("Grant the build read access to the directory, or move it out of the app directory if it holds no routes. A folder whose name starts with `_` is excluded from routing and is not walked.");
    if let Some(path) = path {
        diagnostic = diagnostic.at_file(path);
    }
    diagnostic.into()
}

/// One item from a [`WalkDir`], or the diagnostic for what it could not read.
pub(crate) fn walk_entry(item: walkdir::Result<walkdir::DirEntry>) -> Result<walkdir::DirEntry> {
    item.map_err(|error| {
        // `walkdir::Error`'s own `Display` repeats the path this diagnostic
        // already names, so the operating system's sentence is the reason and
        // the path is carried structurally.
        let reason = match error.io_error() {
            Some(io_error) => io_error.to_string(),
            None => error.to_string(),
        };
        unreadable_route_directory(error.path(), &reason)
    })
}

/// The entries of `directory`, or the diagnostic for why it could not be read.
pub(crate) fn read_route_directory(directory: &Path) -> Result<Vec<std::fs::DirEntry>> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| unreadable_route_directory(Some(directory), &error.to_string()))?;
    entries
        .map(|entry| {
            entry.map_err(|error| unreadable_route_directory(Some(directory), &error.to_string()))
        })
        .collect()
}

/// Whether one directory entry is itself a directory.
///
/// The `file_type` read is a `stat` and can fail on its own, which
/// `is_ok_and` answered with "not a directory" — the same silent skip one level
/// down from the walk.
pub(crate) fn entry_is_directory(entry: &std::fs::DirEntry) -> Result<bool> {
    entry
        .file_type()
        .map(|kind| kind.is_dir())
        .map_err(|error| unreadable_route_directory(Some(&entry.path()), &error.to_string()))
}

pub fn discover_routes(options: DiscoverOptions) -> Result<RouteManifest> {
    let DiscoverOptions {
        app_dir,
        default_render_strategy,
        default_revalidate,
        i18n,
    } = options;

    if !app_dir.exists() {
        return Err(Diagnostic::new("RUV1001", "App directory was not found")
            .explain("Ruvyxa expects an app directory with page.tsx, page.md, page.mdx, or route.ts files.")
            .at_file(&app_dir)
            .suggest("Create app/page.tsx, app/page.md, or app/page.mdx; or set appDir in ruvyxa.config.ts.")
            .into());
    }

    reject_intercepting_routes(&app_dir)?;

    let mut routes = Vec::new();
    // Shared across every route: layouts and shared components are reachable
    // from many pages, and rendering-strategy detection walks that graph.
    let mut cache = ModuleCache::in_root(app_dir.parent().unwrap_or(&app_dir));

    for entry in WalkDir::new(&app_dir).into_iter().filter_entry(|entry| {
        if !entry.file_type().is_dir() || entry.path() == app_dir {
            return true;
        }

        let name = entry.file_name().to_string_lossy();
        !name.starts_with('_') && !name.starts_with('@')
    }) {
        let entry = walk_entry(entry)?;
        if !entry.file_type().is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy();
        let kind = if is_page_file(file_name.as_ref()) {
            RouteKind::Page
        } else if is_route_file(file_name.as_ref(), "route", HANDLER_EXTENSIONS) {
            RouteKind::Api
        } else {
            continue;
        };

        let file = entry.path().to_path_buf();
        let route_dir = file.parent().unwrap_or(&app_dir);
        let relative_dir = route_dir.strip_prefix(&app_dir).unwrap_or(route_dir);
        let path = route_path_from_dir(relative_dir)?;
        let id = route_id(&app_dir, &file);
        let layout_chain = layout_chain(&app_dir, route_dir);
        let template_chain = template_chain(&app_dir, route_dir);
        let slots = route_slots(&app_dir, route_dir)?;
        let intercepts = route_intercepts(&app_dir, route_dir)?;

        routes.push(RouteEntry {
            id,
            path: path.clone(),
            kind,
            file: file.clone(),
            layout_chain: layout_chain.clone(),
            template_chain,
            slots,
            intercepts,
            server_modules: sibling_modules(
                route_dir,
                &["server.ts", "server.js", "action.ts", "action.js"],
            ),
            client_modules: sibling_module(
                route_dir,
                &route_file_names("client", COMPONENT_EXTENSIONS),
            ),
            runtime: detect_runtime_target(&file, &mut cache)?,
            render: if kind == RouteKind::Page {
                apply_rendering_defaults(
                    detect_render_strategy(&app_dir, &file, &path, &layout_chain, &mut cache),
                    default_render_strategy,
                    default_revalidate,
                )
            } else {
                RenderMeta::default()
            },
        });
    }

    routes.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.id.cmp(&right.id))
    });
    detect_conflicts(&routes)?;
    detect_unreachable_intercepts(&routes)?;
    detect_server_component_conflicts(&routes)?;

    Ok(RouteManifest {
        app_dir,
        routes,
        i18n,
    })
}

pub(crate) fn route_path_from_dir(relative_dir: &Path) -> Result<String> {
    let visible_segments = relative_dir
        .components()
        .filter_map(|component| {
            let Component::Normal(segment) = component else {
                return None;
            };
            let segment = segment.to_string_lossy();

            if (segment.starts_with('(') && segment.ends_with(')')) || segment.starts_with('@') {
                None
            } else {
                Some(segment.into_owned())
            }
        })
        .collect::<Vec<_>>();
    let mut segments = Vec::with_capacity(visible_segments.len());

    for (index, segment) in visible_segments.iter().enumerate() {
        segments.push(route_segment(
            relative_dir,
            segment,
            index + 1 == visible_segments.len(),
        )?);
    }

    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}

/// Turn one `app/` folder name into its URL segment.
///
/// `folder` is the route directory this segment belongs to, relative to `app/`.
/// It is carried only so a rejection can name the folder an author has to go
/// and rename, rather than the bare segment, which may appear several levels up
/// from the `page.tsx` that triggered discovery.
pub(crate) fn route_segment(folder: &Path, segment: &str, is_last: bool) -> Result<String> {
    if segment.starts_with("[[...") && segment.ends_with("]]") {
        let name = &segment[5..segment.len() - 2];
        validate_dynamic_name(folder, segment, name)?;
        if !is_last {
            return Err(catch_all_must_be_last());
        }
        return Ok(segment.to_string());
    }

    if segment.starts_with("[...") && segment.ends_with(']') {
        let name = &segment[4..segment.len() - 1];
        validate_dynamic_name(folder, segment, name)?;
        if !is_last {
            return Err(catch_all_must_be_last());
        }
        return Ok(segment.to_string());
    }

    if segment.starts_with('[') && segment.ends_with(']') {
        let name = &segment[1..segment.len() - 1];
        validate_dynamic_name(folder, segment, name)?;
        return Ok(segment.to_string());
    }

    if segment.contains('[') || segment.contains(']') {
        return Err(Diagnostic::new("RUV1002", "Invalid dynamic route segment")
            .explain(format!(
                "`{segment}` in `{}` uses brackets, but it is not one of the dynamic forms [name], [...name], or [[...name]].",
                display_route_folder(folder)
            ))
            .suggest("Rename the route folder to a valid dynamic segment, or remove the brackets to make it an ordinary URL segment.")
            .into());
    }

    Ok(segment.to_string())
}

/// A dynamic route parameter name is one or more ASCII letters, digits, or `_`.
///
/// **This is one rule with two implementations.** `compilePattern` in
/// `packages/@ruvyxa/core/src/route-match.ts` recognises a dynamic segment with
/// `^\[(\w+)\]$`, and JavaScript's `\w` without the `u` flag is exactly
/// `[A-Za-z0-9_]`. Anything outside it is not a parameter to the matcher — it is
/// a *literal* URL component.
///
/// Discovery used to accept any non-empty name that held no bracket and did not
/// begin with a dot, so `app/blog/[post-id]/page.tsx` was written into the
/// manifest as a dynamic route while every JavaScript host compiled the same
/// segment to the literal path `/blog/[post-id]`. The route passed `ruvyxa
/// check`, appeared in the route table, and 404'd on every request, with no
/// diagnostic anywhere: each half was behaving exactly as written.
///
/// Discovery is the narrower of the two on purpose. It is the only place a
/// folder name enters the system, so refusing it here turns a silent 404 into a
/// build error that names the folder — and every consumer downstream (the Rust
/// router, the prerender path expander, the serverless dispatch table, all of
/// which parse brackets permissively) then only ever sees a name the matcher
/// agrees with.
///
/// `tests/fixtures/route-pattern-conformance.json` holds the two halves level.
pub(crate) fn validate_dynamic_name(folder: &Path, segment: &str, name: &str) -> Result<()> {
    if !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Ok(());
    }

    let folder = display_route_folder(folder);
    let explain = if name.is_empty() {
        format!(
            "`{segment}` in `{folder}` declares a dynamic route segment with an empty parameter name."
        )
    } else {
        format!(
            "`{segment}` in `{folder}` names the route parameter `{name}`, which uses characters outside the ASCII letters, digits, and `_`. The route matcher recognises only that character set, so this segment is compiled as a literal URL component: the route is discovered and written into the manifest, and then never matches a request."
        )
    };

    let suggest = match suggested_dynamic_name(name) {
        Some(replacement) => format!(
            "Rename the folder to `{}`. A parameter name has to be usable as `params.{replacement}`, so it may contain only ASCII letters, digits, and `_`.",
            segment.replace(name, &replacement)
        ),
        None => "Use [name], [...name], or [[...name]] with a parameter name of ASCII letters, digits, and `_` — the same characters you would write after `params.`.".to_string(),
    };

    Err(Diagnostic::new("RUV1002", "Invalid dynamic route segment")
        .explain(explain)
        .suggest(suggest)
        .into())
}

/// The nearest legal parameter name, so the diagnostic can offer a rename.
///
/// Separators are dropped and the following letter is upper-cased, which turns
/// the two shapes that actually get written — `post-id` and `post.id` — into
/// `postId`. `to_ascii_uppercase` is deliberate: the case-mapping must not
/// depend on the host's locale, and every surviving byte is ASCII by
/// construction.
pub(crate) fn suggested_dynamic_name(name: &str) -> Option<String> {
    let mut suggestion = String::with_capacity(name.len());
    let mut capitalize = false;
    for byte in name.bytes() {
        if byte == b'_' || byte.is_ascii_alphanumeric() {
            let character = char::from(byte);
            suggestion.push(if capitalize {
                character.to_ascii_uppercase()
            } else {
                character
            });
            capitalize = false;
        } else {
            capitalize = !suggestion.is_empty();
        }
    }

    (!suggestion.is_empty() && suggestion != name).then_some(suggestion)
}

/// A route folder as an author sees it in the project: `app/blog/[post-id]`.
///
/// The walk carries the directory relative to `app/`, and on Windows its
/// components are joined with `\`, which is not how the folder is written in
/// documentation, in the route table, or in any other diagnostic.
pub(crate) fn display_route_folder(relative_dir: &Path) -> String {
    let joined = relative_dir
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().replace('\\', "/")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");

    if joined.is_empty() {
        "app".to_string()
    } else {
        format!("app/{joined}")
    }
}

/// A catch-all with a child segment after it.
///
/// `RUV1017` and not `RUV1002`: this is a well-formed segment in the wrong
/// place, not a malformed one, and the two answered to the same number until
/// SARIF started describing every result of either kind with whichever the
/// report happened to list first. `RUV1002` kept the commoner meaning.
pub(crate) fn catch_all_must_be_last() -> RuvyxaError {
    Diagnostic::new("RUV1017", "Catch-all route must be the final URL segment")
        .explain("Catch-all routes consume every remaining URL segment and cannot have a child URL segment.")
        .suggest("Move the catch-all folder to the end of the route or remove the child segment.")
        .into()
}

pub(crate) fn route_id(app_dir: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(app_dir).unwrap_or(file);
    let without_extension = relative.with_extension("");
    format!(
        "app/{}",
        without_extension
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy().replace('\\', "/")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/")
    )
}

pub(crate) fn layout_chain(app_dir: &Path, route_dir: &Path) -> Vec<String> {
    nested_chain(
        app_dir,
        route_dir,
        &route_file_names("layout", COMPONENT_EXTENSIONS),
    )
}

/// Route id for a directory, matching [`route_id`]'s shape for a file.
pub(crate) fn directory_id(app_dir: &Path, directory: &Path) -> String {
    let relative = directory.strip_prefix(app_dir).unwrap_or(directory);
    let segments = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().replace('\\', "/")),
            _ => None,
        })
        .collect::<Vec<_>>();
    if segments.is_empty() {
        "app".to_string()
    } else {
        format!("app/{}", segments.join("/"))
    }
}

/// `template.tsx` files from the app root down to the route, root first.
///
/// A template wraps its level's children the way a layout does, and differs in
/// one respect that is the whole reason it exists: it is given a key derived
/// from the request path, so navigating within the same layout remounts it —
/// state resets and effects run again. Composition interleaves the two, layout
/// outside template at each level; see `route_wrapper_levels` in
/// `crates/ruvyxa_bundler/src/output.rs` and its mirror in
/// `packages/ruvyxa/runtime/entry-templates.mjs`.
pub(crate) fn template_chain(app_dir: &Path, route_dir: &Path) -> Vec<String> {
    nested_chain(
        app_dir,
        route_dir,
        &route_file_names("template", COMPONENT_EXTENSIONS),
    )
}

/// Files named by one of `file_names` on the path from the app root to
/// `route_dir`, root first.
///
/// One level contributes at most one entry: the names are one module spelled in
/// several extensions, and the first that exists wins. Mirrored by
/// `collectNested` in `packages/ruvyxa/runtime/compiler.mjs` and held level with
/// it by `tests/fixtures/route-chain-conformance.json`.
pub(crate) fn nested_chain(app_dir: &Path, route_dir: &Path, file_names: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = app_dir.to_path_buf();

    found.extend(nested_file_id(app_dir, &current, file_names));

    if let Ok(relative) = route_dir.strip_prefix(app_dir) {
        for component in relative.components() {
            let Component::Normal(segment) = component else {
                continue;
            };
            current.push(segment);
            found.extend(nested_file_id(app_dir, &current, file_names));
        }
    }

    found
}

/// Route id of the first of `file_names` present in `directory`.
pub(crate) fn nested_file_id(
    app_dir: &Path,
    directory: &Path,
    file_names: &[String],
) -> Option<String> {
    file_names
        .iter()
        .map(|name| directory.join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| route_id(app_dir, &candidate))
}

/// The file a layout or template id names.
///
/// A chain entry is an id with the extension stripped, so `app/layout` alone
/// does not say which file it came from and this probe has to offer every
/// extension [`nested_chain`] walked — in the same order, so both answer with
/// the same file. Probing fewer of them is silent: the chain names a layout
/// nothing can load, so its imports are never staged into `<out>/server/` and
/// `render_reachable_code` gives up, leaving the route SSR.
pub(crate) fn resolve_layout_file(app_dir: &Path, layout_id: &str) -> Option<PathBuf> {
    let layout = PathBuf::from(layout_id);
    let project_root = app_dir.parent().unwrap_or(app_dir);
    let app_relative = layout
        .strip_prefix("app")
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| layout.clone());
    let candidates = [project_root.join(&layout), app_dir.join(app_relative)];

    // `normalized_canonical_path`, not `Path::canonicalize`: the raw call
    // returns the Windows extended-length prefix, and every caller feeds this
    // path into `ModuleCache`, which keys on it. The cache normalizes on the
    // way in, so nothing is wrong today — but handing out a verbatim path is
    // the shape that broke server-component builds once already, and the next
    // caller has no reason to expect it.
    // The extension is *appended*, never substituted: `Path::with_extension`
    // would turn an id whose last segment carries a dot into a file nobody
    // wrote, which is the mistake the bundler's resolver documents and this
    // crate's module walk already stopped making.
    candidates.into_iter().find_map(|candidate| {
        std::iter::once(candidate.clone())
            .chain(COMPONENT_EXTENSIONS.iter().map(|extension| {
                let mut named = candidate.clone().into_os_string();
                named.push(".");
                named.push(extension);
                PathBuf::from(named)
            }))
            .find(|file| file.is_file())
            .map(|file| normalized_canonical_path(&file))
    })
}

/// The first of `names` that exists beside the route, as a one-element list.
///
/// First match rather than every match, because these names are one module
/// spelled in several extensions — `client.tsx` and `client.jsx` are the same
/// module, not two — and the probe order is [`COMPONENT_EXTENSIONS`]'s.
pub(crate) fn sibling_module(route_dir: &Path, names: &[String]) -> Vec<String> {
    names
        .iter()
        .map(|name| route_dir.join(name))
        .find(|module| module.is_file())
        .map(|module| vec![module.display().to_string()])
        .unwrap_or_default()
}

/// Every one of `names` that exists beside the route, in the order given.
///
/// Unlike [`sibling_module`] these are distinct modules — a route may have both
/// a `server.ts` and an `action.ts` — so all of them are collected.
pub(crate) fn sibling_modules(route_dir: &Path, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| route_dir.join(name))
        .filter(|module| module.is_file())
        .map(|module| module.display().to_string())
        .collect()
}
