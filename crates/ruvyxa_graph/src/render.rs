use std::path::Path;
use std::sync::Arc;

use ruvyxa_diagnostics::{Diagnostic, Result};

use crate::discovery::resolve_layout_file;
use crate::exports::{
    export_const_value, has_export_const_bool, has_static_params_export, parse_export_const_number,
};
use crate::graph::{ModuleCache, collect_relative_graph};
use crate::manifest::{HydrationMode, RenderMeta, RenderStrategy, RuntimeTarget};

/// Detect a page's rendering strategy, and whether it opts into server components.
///
/// The two are read separately because they answer different questions — see
/// [`RenderMeta::server_components`] — and because the strategy rules below
/// return early in six places, each of which would otherwise have to remember
/// to carry the opt-in.
pub(crate) fn detect_render_strategy(
    app_dir: &Path,
    file: &Path,
    route_path: &str,
    layout_chain: &[String],
    cache: &mut ModuleCache,
) -> RenderMeta {
    let mut render = detect_render_meta(app_dir, file, route_path, layout_chain, cache);
    render.server_components = opts_into_server_components(file, cache);
    render
}

/// Whether a page declares `export const serverComponents = true`.
///
/// Read from masked source for the same reason every other route export is: a
/// commented-out or quoted occurrence is not a declaration, and reading raw
/// text turned one into a silent change of rendering pipeline.
pub(crate) fn opts_into_server_components(file: &Path, cache: &mut ModuleCache) -> bool {
    let Some(page) = cache.module(file) else {
        return false;
    };
    let source = Arc::clone(&page.source);
    let Some(code) = cache.masked(file) else {
        return false;
    };
    has_export_const_bool(&source, &code, "serverComponents", true)
}

/// Detect the rendering strategy for a page by scanning its source for known exports/directives.
///
/// Detection rules (first match wins):
/// 1. `"use client"` directive at top → CSR
/// 2. `export const ppr = true` → PPR
/// 3. `export const revalidate = <number>` → ISR with that interval
/// 4. `getStaticParams` or `staticParams` page export → SSG
/// 5. Route has no dynamic segments and no data fetching → SSG candidate (static routes)
/// 6. Default → SSR
pub(crate) fn detect_render_meta(
    app_dir: &Path,
    file: &Path,
    route_path: &str,
    layout_chain: &[String],
    cache: &mut ModuleCache,
) -> RenderMeta {
    let Some(page) = cache.module(file) else {
        return RenderMeta::default();
    };
    let source = Arc::clone(&page.source);
    let Some(code) = cache.masked(file) else {
        return RenderMeta::default();
    };

    // 1. Check for the `'use client'` directive — through the bundler's
    // scanner, the same call `is_client_boundary` makes above, because there is
    // one answer to this question and it is not a text search. A hand-rolled
    // `trim_start().starts_with(…)` here saw neither a UTF-8 BOM (`Cf`, so
    // `trim_start` keeps it) nor a leading comment, so a page the compilers and
    // `is_client_boundary` all call a client component was classified SSG and
    // pre-rendered — browser-only code run in the build's server renderer, with
    // RUV1011 gated on `Csr` and therefore silent.
    if ruvyxa_bundler::reference_manifest::has_module_directive(&source, "use client") {
        // CSR pages render entirely in the browser, so the hydration opt-out
        // does not apply — the directive wins.
        return RenderMeta {
            strategy: RenderStrategy::Csr,
            ..Default::default()
        };
    }

    // Boolean false remains the zero-JS contract; string values add deferred
    // route-level hydration without changing the default.
    let hydration = parse_hydration_mode(&source, &code);

    // 1b. `export const dynamic` — the route segment config Next.js uses to
    // override the automatic choice. A page written against that convention
    // used to be read by nothing here: `force-dynamic` on an otherwise-static
    // page was discarded and the page was pre-rendered anyway, which is the
    // opposite of what it asked for and produced no diagnostic. It is checked
    // before the opt-in exports below because that is the precedence Next
    // defines — `force-dynamic` outranks `revalidate`.
    match export_const_value(&source, &code, "dynamic")
        .map(|value| value.trim_end_matches(';').trim().trim_matches(['\'', '"']))
    {
        Some("force-dynamic") => {
            return RenderMeta {
                hydration,
                force_dynamic: true,
                ..Default::default()
            };
        }
        // `error` is `force-static` plus a runtime complaint about dynamic
        // APIs; this graph decides strategy, not runtime behaviour, so the
        // strategy is the part it can honour.
        Some("force-static" | "error") => {
            return RenderMeta {
                strategy: RenderStrategy::Ssg,
                has_static_params: has_static_params_export(&code),
                hydration,
                ..Default::default()
            };
        }
        // `auto` is the default, and anything else is not this export.
        _ => {}
    }

    // 2. Check for PPR opt-in: export const ppr = true
    if has_export_const_bool(&source, &code, "ppr", true) {
        return RenderMeta {
            strategy: RenderStrategy::Ppr,
            has_dynamic_slots: true,
            hydration,
            ..Default::default()
        };
    }

    // 3. Check for ISR: export const revalidate = <number>
    if let Some(seconds) = parse_export_const_number(&source, &code, "revalidate") {
        let has_static_params = has_static_params_export(&code);
        return RenderMeta {
            strategy: RenderStrategy::Isr,
            revalidate: Some(seconds),
            has_static_params,
            hydration,
            ..Default::default()
        };
    }

    // 4. Check for dynamic SSG parameter exports.
    if has_static_params_export(&code) {
        return RenderMeta {
            strategy: RenderStrategy::Ssg,
            has_static_params: true,
            hydration,
            ..Default::default()
        };
    }

    // 5. Static routes with no dynamic data markers can be pre-rendered.
    //
    // The reachable graph is only read here. Rules 1-4 answer from the page's
    // own exports, so reading and masking every dependency ahead of them was
    // work whose result was discarded for every page that opts into a strategy
    // explicitly.
    if !route_has_dynamic_segments(route_path) {
        // A dependency that cannot be read cannot be cleared of data markers,
        // so an unreadable graph leaves the route dynamic rather than guessing
        // it static.
        let reachable_code = render_reachable_code(app_dir, file, layout_chain, cache);
        if reachable_code.is_some_and(|code| !has_dynamic_data_markers(&code)) {
            return RenderMeta {
                strategy: RenderStrategy::Ssg,
                hydration,
                ..Default::default()
            };
        }
    }

    // 6. Default: SSR
    RenderMeta {
        hydration,
        ..Default::default()
    }
}

/// Node built-ins an edge route may not reach.
///
/// Conservative on purpose: these are the modules that need a filesystem, a
/// process table, or a socket, and no V8 isolate has them. The ones every edge
/// runtime ships a polyfill for — `buffer`, `crypto`, `path`, `stream`, `util`
/// — are deliberately absent, because a false refusal on a module that does
/// work costs more than a missing one. The missing one still fails at deploy,
/// loudly, on the platform that knows its own surface.
///
/// Kept level with `unavailableOnEdge` in
/// `tests/fixtures/edge-runtime-conformance.json`, which the tests replay.
pub(crate) const EDGE_UNAVAILABLE_BUILTINS: &[&str] = &[
    "child_process",
    "cluster",
    "dgram",
    "dns",
    "fs",
    "http2",
    "inspector",
    "module",
    "net",
    "os",
    "readline",
    "repl",
    "tls",
    "trace_events",
    "tty",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
];

/// The forbidden built-in this specifier names, if it names one.
///
/// Matches the segment before the first `/` so `node:fs/promises` and
/// `fs/promises` are the same answer as `fs`.
pub(crate) fn edge_forbidden_builtin(specifier: &str) -> Option<&'static str> {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    let head = bare.split('/').next().unwrap_or(bare);
    EDGE_UNAVAILABLE_BUILTINS
        .iter()
        .find(|name| **name == head)
        .copied()
}

/// The runtime one `export const runtime = …` value names, or `None` when it
/// names nothing this framework has.
///
/// Split from the file walk so the accepted spellings can be replayed against
/// the shared table without a temporary project on disk.
pub(crate) fn runtime_target_from_value(raw: &str) -> Option<RuntimeTarget> {
    let value = raw.trim_end_matches(';').trim();
    let value = value.strip_suffix("as const").unwrap_or(value).trim();
    match value.trim_matches(['\'', '"', '`']) {
        "edge" => Some(RuntimeTarget::Edge),
        "nodejs" | "node" => Some(RuntimeTarget::Node),
        _ => None,
    }
}

/// The runtime a route asks to run on — `export const runtime = 'edge'`.
///
/// Read from the route's own file and from nothing it imports: a dependency
/// cannot move the route that uses it, and the whole value of the declaration
/// is that one glance at the route says where it runs.
///
/// An unrecognised value is an error rather than a fall back to Node. The
/// declaration exists because the author meant somewhere specific; accepting
/// `'Edge'` or `'worker'` silently would put the route on the other runtime
/// with nothing said, and the difference only shows up in production, where an
/// edge route has no filesystem and a Node route has no Worker globals.
///
/// `nodejs` is spelled the way Next.js spells it, so a route moved between the
/// two frameworks does not change meaning.
pub(crate) fn detect_runtime_target(file: &Path, cache: &mut ModuleCache) -> Result<RuntimeTarget> {
    let Some(module) = cache.module(file) else {
        return Ok(RuntimeTarget::Node);
    };
    let source = Arc::clone(&module.source);
    let Some(masked) = cache.masked(file) else {
        return Ok(RuntimeTarget::Node);
    };
    let Some(raw) = export_const_value(&source, &masked, "runtime") else {
        return Ok(RuntimeTarget::Node);
    };

    match runtime_target_from_value(raw) {
        Some(target) => Ok(target),
        None => Err(Diagnostic::new("RUV1012", "Unknown route runtime")
            .explain(format!(
                "`export const runtime = {}` names a runtime this framework does not have. The choices are `'edge'` and `'nodejs'`.",
                raw.trim()
            ))
            .at_file(file)
            .suggest(
                "Use `export const runtime = 'edge'` to run this route on a Web-standards runtime, or remove the export to keep the Node default.",
            )
            .into()),
    }
}

/// Parse the additive route hydration export while preserving boolean input.
///
/// `hydrate` decides whether a page ships a client bundle at all, so reading it
/// wrongly in either direction is expensive: a missed opt-out ships JavaScript
/// the author disabled, and a false positive drops the hydration a working page
/// depends on. Both happened while this read the raw source — see
/// [`export_const_value`].
pub(crate) fn parse_hydration_mode(source: &str, masked: &str) -> HydrationMode {
    let Some(value) = export_const_value(source, masked, "hydrate") else {
        return HydrationMode::Load;
    };
    let value = value.trim_end_matches(';').trim();
    let value = value.strip_suffix("as const").unwrap_or(value).trim();
    match value.trim_matches(['\'', '"']) {
        "false" | "none" => HydrationMode::None,
        "idle" => HydrationMode::Idle,
        "visible" => HydrationMode::Visible,
        _ => HydrationMode::Load,
    }
}

/// Return all statically reachable route and layout source after stripping strings/comments.
/// Route-level rendering exports are intentionally handled from the page source only, while data
/// markers in any dependency make automatic SSG unsafe.
pub(crate) fn render_reachable_code(
    app_dir: &Path,
    file: &Path,
    layout_chain: &[String],
    cache: &mut ModuleCache,
) -> Option<String> {
    let mut files = collect_relative_graph(file, cache);
    for layout in layout_chain {
        let layout = resolve_layout_file(app_dir, layout)?;
        files.extend(collect_relative_graph(&layout, cache));
    }

    let mut code = String::new();
    for path in files {
        code.push_str(&cache.masked(&path)?);
        code.push('\n');
    }
    Some(code)
}

pub(crate) fn apply_rendering_defaults(
    mut render: RenderMeta,
    default_strategy: Option<RenderStrategy>,
    default_revalidate: Option<u64>,
) -> RenderMeta {
    if render.strategy != RenderStrategy::Ssr {
        return render;
    }

    let Some(strategy) = default_strategy else {
        return render;
    };

    render.strategy = strategy;
    if strategy == RenderStrategy::Isr {
        render.revalidate = Some(default_revalidate.unwrap_or(60));
    }
    render
}

pub(crate) fn route_has_dynamic_segments(route_path: &str) -> bool {
    route_path
        .split('/')
        .any(|segment| segment.starts_with('[') && segment.ends_with(']'))
}

pub(crate) fn has_dynamic_data_markers(code: &str) -> bool {
    const MARKERS: &[&str] = &[
        "fetch(",
        "headers(",
        "cookies(",
        "searchParams",
        "Date.now(",
        "Math.random(",
        "process.env.",
    ];

    MARKERS
        .iter()
        .any(|marker| contains_marker_identifier(code, marker))
}

/// True when `marker` appears in `code` as its own identifier.
///
/// A plain `contains` reads `router.prefetch('/products')` as a `fetch(` call
/// and takes automatic pre-rendering away from a page that only warms a link —
/// `prefetch` is an API this framework ships, so the collision is the ordinary
/// case rather than a contrived one.
///
/// Only the leading edge is checked. Every marker already ends at a `(` or a
/// `.`, which cannot continue an identifier, and a member access has to keep
/// matching: `globalThis.fetch(` is a fetch.
///
/// A byte that begins a multi-byte character is not an ASCII identifier byte,
/// so an identifier outside ASCII reads as a word boundary and the marker
/// counts. That is the safe direction: this decides whether a route may be
/// pre-rendered, and a false marker costs a static page while a missed one
/// ships stale data.
pub(crate) fn contains_marker_identifier(code: &str, marker: &str) -> bool {
    let bytes = code.as_bytes();
    code.match_indices(marker).any(|(start, _)| {
        !start.checked_sub(1).is_some_and(
            |index| matches!(bytes[index], b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'$'),
        )
    })
}
