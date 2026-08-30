use std::collections::{BTreeMap, BTreeSet};

use ruvyxa_diagnostics::{Diagnostic, Result};

use crate::manifest::{RenderStrategy, RouteEntry, RouteKind};

/// Refuse an interception whose target no route serves.
///
/// An interception is an overlay on an ordinary route: a hard load, a refresh,
/// or a shared link has to render the real page, so the real page has to exist.
/// Without this check `app/feed/@modal/(.)phto/page.tsx` — one typo — would be
/// a modal that never opens and a URL that 404s, with nothing said at build
/// time.
///
/// Targets are compared by match shape rather than by text, so
/// `(.)photo/[id]` and `app/feed/photo/[photoId]` are the same URL to this
/// check, exactly as they are to the router.
pub(crate) fn detect_unreachable_intercepts(routes: &[RouteEntry]) -> Result<()> {
    let pages = routes
        .iter()
        .filter(|route| route.kind == RouteKind::Page)
        .map(|route| route_match_shape(&route.path))
        .collect::<BTreeSet<_>>();

    // One route can carry the same interception as another; report the first
    // by sorted file path so two machines name the same file.
    let mut unreachable = routes
        .iter()
        .flat_map(|route| route.intercepts.iter())
        .filter(|intercept| !pages.contains(&route_match_shape(&intercept.target)))
        .collect::<Vec<_>>();
    unreachable
        .sort_by(|left, right| (&left.file, &left.target).cmp(&(&right.file, &right.target)));
    unreachable.dedup_by(|left, right| left.file == right.file && left.target == right.target);

    let Some(intercept) = unreachable.into_iter().next() else {
        return Ok(());
    };
    Err(Diagnostic::new("RUV1006", "Intercepting route has no route to intercept")
        .explain(format!(
            "`{}` intercepts `{}`, and no page answers that URL. An interception is an overlay: a hard load or a shared link still has to render the real page.",
            intercept.marker, intercept.target
        ))
        .at_file(&intercept.file)
        .suggest(format!(
            "Add the page the interception stands in for, at the route `{}`, or correct the folder name.",
            intercept.target
        ))
        .into())
}

/// Refuse the three combinations where `serverComponents` would silently do nothing.
///
/// All three are opt-ins that read as working. A page that is itself
/// `'use client'` has no server half to render, so the export changes nothing
/// and the author is left believing their data fetching moved off the browser.
/// Partial pre-rendering streams its shell through an entry this pipeline does
/// not build. An interception is resolved by the client router from a registry
/// the server-components browser entry does not build, so the modal simply
/// never opens.
///
/// Refusing at discovery rather than at render is deliberate: all three
/// failures are invisible in a working page, and a diagnostic that only fires
/// on the request path would not fire during `ruvyxa check` at all.
///
/// **One code each.** These were `RUV1011` three times over, which made the
/// code useless as a search term and made the SARIF rule table describe all
/// three with whichever the report listed first. `RUV1011` kept the commonest
/// of them — the `'use client'` page — and the other two took `RUV1019` and
/// `RUV1020`.
pub(crate) fn detect_server_component_conflicts(routes: &[RouteEntry]) -> Result<()> {
    for route in routes.iter().filter(|route| route.render.server_components) {
        if route.render.strategy == RenderStrategy::Csr {
            return Err(Diagnostic::new(
                "RUV1011",
                "Page declares both `use client` and server components",
            )
            .explain(
                "A `'use client'` page runs entirely in the browser, so there is no server graph for `export const serverComponents = true` to render. One of the two is not doing what it says.",
            )
            .at_file(&route.file)
            .suggest(
                "Remove the `'use client'` directive from the page and move the interactive parts into their own `'use client'` components, or drop the `serverComponents` export.",
            )
            .into());
        }
        if route.render.strategy == RenderStrategy::Ppr {
            return Err(Diagnostic::new(
                "RUV1019",
                "Server components route also opts into partial pre-rendering",
            )
            .explain(
                "Partial pre-rendering streams a static shell and fills its holes later, through a render entry the server-components pipeline does not build. The route would be pre-rendered as an ordinary shell and the `serverComponents` export would do nothing.",
            )
            .at_file(&route.file)
            .suggest("Remove `export const ppr = true` or `export const serverComponents = true` from this page.")
            .into());
        }
        if let Some(intercept) = route.intercepts.first() {
            return Err(Diagnostic::new(
                "RUV1020",
                "Server components route carries an intercepting route",
            )
            .explain(format!(
                "`{}` intercepts `{}`, and an interception is resolved by the client router from a registry a server-components route does not publish. The overlay would never open.",
                intercept.marker, intercept.target
            ))
            .at_file(&route.file)
            .suggest(
                "Drop `export const serverComponents = true` from this route, or move the interception to a route that does not use server components.",
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn detect_conflicts(routes: &[RouteEntry]) -> Result<()> {
    let mut seen = BTreeMap::<String, &RouteEntry>::new();

    for route in routes {
        let key = route_match_shape(&route.path);
        if let Some(previous) = seen.insert(key, route) {
            let mut diagnostic = Diagnostic::new("RUV1003", "Conflicting route paths")
                .explain(format!(
                    "{} and {} resolve to the same URL match shape. Route parameter names and page/API kinds do not make overlapping routes distinct.",
                    previous.file.display(),
                    route.file.display()
                ))
                .at_file(&route.file)
                .suggest("Keep only one route for this URL shape or move one route to a distinct URL segment.");
            diagnostic.affected_routes = vec![previous.id.clone(), route.id.clone()];
            return Err(diagnostic.into());
        }
    }

    Ok(())
}

pub(crate) fn route_match_shape(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with("[[...") && segment.ends_with("]]") {
                "*?"
            } else if segment.starts_with("[...") && segment.ends_with(']') {
                "*"
            } else if segment.starts_with('[') && segment.ends_with(']') {
                ":"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}
