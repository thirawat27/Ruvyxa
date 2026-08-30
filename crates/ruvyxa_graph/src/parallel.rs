use std::path::{Component, Path, PathBuf};

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use walkdir::WalkDir;

use crate::discovery::{
    COMPONENT_EXTENSIONS, directory_id, entry_is_directory, is_page_file, page_file_names,
    read_route_directory, route_file_names, route_path_from_dir, walk_entry,
};
use crate::manifest::{RouteIntercept, RouteSlot};

/// Next.js intercepting-route markers, longest first.
///
/// Order is load-bearing: `(..)(..)` also starts with `(..)`, and `(...)` also
/// starts with `(.`, so a shorter marker tested first would name the wrong
/// convention in the diagnostic.
pub(crate) const INTERCEPTING_ROUTE_MARKERS: [&str; 4] = ["(..)(..)", "(...)", "(..)", "(.)"];

/// The intercepting-route marker a directory name opens with, if any.
pub(crate) fn intercepting_route_marker(segment: &str) -> Option<&'static str> {
    INTERCEPTING_ROUTE_MARKERS
        .into_iter()
        .find(|marker| segment.starts_with(marker))
}

/// How many route levels a marker climbs before the segment it names.
///
/// `(...)` is the odd one: it restarts from the app root rather than climbing a
/// fixed number of levels, so it is reported separately.
pub(crate) fn intercept_climb(marker: &str) -> Option<usize> {
    match marker {
        "(.)" => Some(0),
        "(..)" => Some(1),
        "(..)(..)" => Some(2),
        _ => None,
    }
}

/// Whether a project-relative directory sits inside a parallel-route slot.
pub(crate) fn is_inside_slot(relative: &Path) -> bool {
    relative.components().any(|component| {
        matches!(component, Component::Normal(name) if name.to_string_lossy().starts_with('@'))
    })
}

/// Refuse intercepting-route directories that no slot can render.
///
/// An interception is an overlay: it replaces a parallel-route slot while the
/// page underneath stays mounted, so it only means something inside an `@name`
/// folder. Outside one there is nothing to render it into, and the folder used
/// to become a literal URL segment instead — the route-group branch needs a
/// trailing `)`, so `app/feed/(.)photo/page.tsx` passed straight through
/// [`route_segment`] and mounted a real, publicly reachable page at
/// `/feed/(.)photo`, a view the author wrote as an interception and never meant
/// to publish on its own URL.
///
/// This walks directories rather than the segments of discovered routes,
/// because the route walk skips `@slot` folders. `_`-prefixed folders are
/// excluded: they opt out of routing entirely, so nothing there can reach a
/// URL.
pub(crate) fn reject_intercepting_routes(app_dir: &Path) -> Result<()> {
    let mut offenders = Vec::new();
    for entry in WalkDir::new(app_dir).into_iter().filter_entry(|entry| {
        !entry.file_type().is_dir()
            || entry.path() == app_dir
            || !entry.file_name().to_string_lossy().starts_with('_')
    }) {
        // Not `.filter_map(Result::ok)`: a subtree the walk cannot read is
        // exactly where an unrefused interception would hide, so dropping the
        // error here skips the refusal this function exists to make.
        let entry = walk_entry(entry)?;
        if !entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(marker) = intercepting_route_marker(&name) else {
            continue;
        };
        let Ok(relative) = entry.path().strip_prefix(app_dir) else {
            continue;
        };
        // Inside a slot the folder is a real interception, resolved by
        // `route_intercepts`. Everywhere else it is a mistake.
        if is_inside_slot(relative) {
            continue;
        }
        offenders.push((entry.path().to_path_buf(), name, marker));
    }
    // Directory order is filesystem order, so which offender is reported would
    // otherwise differ between machines building the same project.
    offenders.sort_by(|left, right| left.0.cmp(&right.0));

    let Some((path, name, marker)) = offenders.into_iter().next() else {
        return Ok(());
    };
    Err(Diagnostic::new("RUV1005", "Intercepting route is outside a parallel-route slot")
        .explain(format!(
            "`{name}` opens with the intercepting-route marker `{marker}`, but it does not live inside an `@name` folder. An interception replaces a slot while the page underneath stays mounted, so there is nowhere to render this one."
        ))
        .at_file(&path)
        .suggest("Move the folder inside a parallel-route slot beside the layout that should show it, such as `@modal`, or rename it to an ordinary route segment.")
        .into())
}

/// Parallel-route slots in scope for a route, level order then name order.
///
/// Walks the same directory chain the layout and template chains do, and at
/// each level resolves every `@name` folder against the route's remaining
/// segments. A slot that matches neither a page nor a `default.tsx` is left out
/// entirely — the layout sees no prop, which is the same thing Next.js renders
/// for an unmatched slot with no default.
pub(crate) fn route_slots(app_dir: &Path, route_dir: &Path) -> Result<Vec<RouteSlot>> {
    let Ok(relative) = route_dir.strip_prefix(app_dir) else {
        return Ok(Vec::new());
    };
    let segments = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut slots = Vec::new();
    let mut level = app_dir.to_path_buf();
    for depth in 0..=segments.len() {
        if depth > 0 {
            level.push(&segments[depth - 1]);
        }
        // The URL below this level is what the slot has to match.
        let remaining = &segments[depth..];
        slots.extend(slots_at_level(app_dir, &level, remaining)?);
    }
    Ok(slots)
}

/// Interceptions in scope for a route, level order then slot name then target.
///
/// Walks the same directory chain the layout, template, and slot chains do. At
/// each level, every `@name` folder is searched for children whose first
/// segment carries an intercepting-route marker, and each one is resolved to
/// the URL it covers.
///
/// The target is computed from the *level's* URL rather than from the slot
/// folder, because a slot contributes no URL segment: for
/// `app/feed/@modal/(.)photo`, `(.)` means "the level `app/feed` is on", so the
/// target is `/feed/photo`.
pub(crate) fn route_intercepts(app_dir: &Path, route_dir: &Path) -> Result<Vec<RouteIntercept>> {
    let Ok(relative) = route_dir.strip_prefix(app_dir) else {
        return Ok(Vec::new());
    };
    let segments = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut intercepts = Vec::new();
    let mut level = app_dir.to_path_buf();
    for depth in 0..=segments.len() {
        if depth > 0 {
            level.push(&segments[depth - 1]);
        }
        intercepts.extend(intercepts_at_level(app_dir, &level)?);
    }
    // Directory order is filesystem order, and this list decides the order the
    // generated entry emits its lookup table in.
    intercepts.sort_by(|left, right| {
        (&left.level, &left.name, &left.target).cmp(&(&right.level, &right.name, &right.target))
    });
    intercepts.dedup();
    Ok(intercepts)
}

/// Every interception declared by an `@name` folder directly inside `level`.
pub(crate) fn intercepts_at_level(app_dir: &Path, level: &Path) -> Result<Vec<RouteIntercept>> {
    let level_relative = level.strip_prefix(app_dir).unwrap_or(Path::new(""));
    let level_path = route_path_from_dir(level_relative)?;

    let mut slots = named_slot_directories(level)?;
    slots.sort_by(|left, right| left.0.cmp(&right.0));

    let mut intercepts = Vec::new();
    for (name, slot_dir) in slots {
        for (file, marker, target_segments) in intercept_pages(&slot_dir)? {
            let target = intercept_target_path(&level_path, marker, &target_segments)
                .ok_or_else(|| intercept_climbs_past_root(&file, marker, &level_path))?;
            intercepts.push(RouteIntercept {
                level: directory_id(app_dir, level),
                name: name.clone(),
                target,
                marker: marker.to_string(),
                file,
            });
        }
    }
    Ok(intercepts)
}

/// Page files under a slot whose first segment carries a marker.
///
/// Returns the page, the marker, and the URL segments it contributes — the
/// first segment with the marker stripped, then everything below it.
pub(crate) fn intercept_pages(
    slot_dir: &Path,
) -> Result<Vec<(PathBuf, &'static str, Vec<String>)>> {
    let mut found = Vec::new();
    for entry in WalkDir::new(slot_dir).into_iter() {
        let entry = walk_entry(entry)?;
        if !entry.file_type().is_file() {
            continue;
        }
        if !is_page_file(entry.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        let Some(page_dir) = entry.path().parent() else {
            continue;
        };
        let Ok(relative) = page_dir.strip_prefix(slot_dir) else {
            continue;
        };
        let mut segments = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let Some(first) = segments.first().cloned() else {
            continue;
        };
        let Some(marker) = intercepting_route_marker(&first) else {
            continue;
        };
        let head = first[marker.len()..].to_string();
        if head.is_empty() {
            continue;
        }
        segments[0] = head;
        found.push((entry.path().to_path_buf(), marker, segments));
    }
    Ok(found)
}

/// The URL an interception covers, or `None` when the marker climbs past root.
pub(crate) fn intercept_target_path(
    level_path: &str,
    marker: &str,
    segments: &[String],
) -> Option<String> {
    let base = match intercept_climb(marker) {
        // `(...)` restarts from the app root rather than climbing levels.
        None => "/".to_string(),
        Some(climb) => drop_route_segments(level_path, climb)?,
    };
    let mut parts = base
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    parts.extend(segments.iter().cloned());
    Some(format!("/{}", parts.join("/")))
}

/// Drop `count` trailing segments from a route path, or `None` if it cannot.
pub(crate) fn drop_route_segments(path: &str, count: usize) -> Option<String> {
    let mut parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < count {
        return None;
    }
    parts.truncate(parts.len() - count);
    Some(format!("/{}", parts.join("/")))
}

/// A marker that asks for more levels than the app has.
///
/// `RUV1018` and not `RUV1006`: a marker with no target and a marker with
/// nowhere to climb are different mistakes with different fixes, and sharing a
/// code meant a SARIF rule described one of them for results about the other.
/// `RUV1006` kept the commoner meaning — a target no page answers, usually a
/// typo in the folder name.
pub(crate) fn intercept_climbs_past_root(
    file: &Path,
    marker: &str,
    level_path: &str,
) -> RuvyxaError {
    Diagnostic::new("RUV1018", "Intercepting route climbs above the app root")
        .explain(format!(
            "`{marker}` asks for a level above `{level_path}`, and there is nothing there. A marker can only climb as many levels as the slot's own route has."
        ))
        .at_file(file)
        .suggest("Use a shorter marker, or `(...)` to name a path from the app root.")
        .into()
}

/// Every `@name` folder directly inside `level`, resolved against `remaining`.
pub(crate) fn slots_at_level(
    app_dir: &Path,
    level: &Path,
    remaining: &[std::ffi::OsString],
) -> Result<Vec<RouteSlot>> {
    let mut named = named_slot_directories(level)?;
    // Directory order is filesystem order, which differs between machines and
    // decides prop order in the generated entry.
    named.sort_by(|left, right| left.0.cmp(&right.0));

    Ok(named
        .into_iter()
        .filter_map(|(name, slot_dir)| {
            let file = slot_page_for(&slot_dir, remaining)?;
            Some(RouteSlot {
                level: directory_id(app_dir, level),
                name,
                file,
            })
        })
        .collect())
}

/// Every `@name` directory directly inside `level`, unsorted.
///
/// One reader for the two walks that used to spell it twice, so a directory
/// neither of them can read is reported the same way in both. A slot needs a
/// name: `@` alone is a directory nobody can address, and it is skipped.
fn named_slot_directories(level: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut named = Vec::new();
    for entry in read_route_directory(level)? {
        if !entry_is_directory(&entry)? {
            continue;
        }
        let raw = entry.file_name();
        let name = raw.to_string_lossy();
        let Some(name) = name.strip_prefix('@') else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        named.push((name.to_string(), entry.path()));
    }
    Ok(named)
}

/// The file a slot renders for the remaining URL segments.
///
/// The slot's own page for that sub-path when it has one, and its
/// `default.tsx` otherwise. `default.tsx` is what a slot falls back to when the
/// URL does not name anything inside it, which is the majority of navigations
/// once more than one slot exists.
pub(crate) fn slot_page_for(slot_dir: &Path, remaining: &[std::ffi::OsString]) -> Option<PathBuf> {
    let mut target = slot_dir.to_path_buf();
    for segment in remaining {
        target.push(segment);
    }
    for name in page_file_names() {
        let candidate = target.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for name in route_file_names("default", COMPONENT_EXTENSIONS) {
        let candidate = slot_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
