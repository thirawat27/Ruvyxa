/**
 * Intercepting-route discovery for the dev server's client entries.
 *
 * `ruvyxa dev` builds its entries from the filesystem in a worker process,
 * while `ruvyxa build` builds them from the route manifest the Rust graph
 * produced. Both have to find the same interceptions: one that only one host
 * composes is a modal that opens in production and does nothing locally, or
 * the reverse.
 *
 * Mirrors `route_intercepts()` in `crates/ruvyxa_graph/src/parallel.rs`, and the two
 * are held to `tests/fixtures/intercepting-route-conformance.json`.
 *
 * A module of its own rather than a function inside `worker-pool.mjs`, because
 * that file starts a worker on import and cannot be loaded by a test.
 */

import { readdirSync } from 'node:fs'
import path from 'node:path'

import { compareCodePoints } from './order.mjs'

/**
 * Intercepting routes in scope for a route, level order then slot name.
 *
 * Walks the same directory chain the slot chain does. At each level every
 * `@name` folder is searched for children whose first segment carries a
 * marker, and each is resolved to the URL it covers — from the *level's* URL
 * rather than the slot folder's, because a slot contributes no URL segment.
 *
 * Mirrors `route_intercepts()` in `crates/ruvyxa_graph/src/parallel.rs`, and the two
 * are held to `tests/fixtures/intercepting-route-conformance.json`: an
 * interception one host composes and the other does not is a modal that opens
 * under `ruvyxa build` and does nothing under `ruvyxa dev`.
 */
export function collectIntercepts(appDir, routeDir) {
  const relative = path.relative(appDir, routeDir)
  if (relative.startsWith('..')) return []
  const segments = relative ? relative.split(path.sep).filter(Boolean) : []

  const intercepts = []
  let level = appDir
  for (let depth = 0; depth <= segments.length; depth += 1) {
    if (depth > 0) level = path.join(level, segments[depth - 1])
    intercepts.push(...interceptsAtLevel(appDir, level))
  }
  intercepts.sort((left, right) =>
    compareCodePoints(
      `${left.levelId}\u0000${left.name}\u0000${left.target}`,
      `${right.levelId}\u0000${right.name}\u0000${right.target}`,
    ),
  )
  return intercepts
}

/** Markers longest-first, so `(..)(..)` is never read as `(..)`. */
const INTERCEPT_MARKERS = ['(..)(..)', '(...)', '(..)', '(.)']

/** How many route levels a marker climbs, or null for the from-root marker. */
function interceptClimb(marker) {
  if (marker === '(.)') return 0
  if (marker === '(..)') return 1
  if (marker === '(..)(..)') return 2
  return null
}

/**
 * The entries of one directory, or the error that says why they are unknown.
 *
 * `catch { return [] }` used to stand here and in `interceptPages` below, which
 * reads "I could not look" as "there is nothing there" — an interception that
 * silently disappears from the dev entry. The Rust half refuses the same
 * condition with `RUV1021`, so swallowing it here would make `ruvyxa dev`
 * quietly drop a modal that `ruvyxa build` refuses to build at all.
 */
function readRouteDirectory(directory) {
  try {
    return readdirSync(directory, { withFileTypes: true })
  } catch (error) {
    throw new Error(
      `RUV1021: Route directory could not be read: ${directory}: ${error.message}. ` +
        'A directory the build cannot look inside is not an empty one — an interception below it ' +
        'would silently disappear from the entry.',
      { cause: error },
    )
  }
}

function interceptsAtLevel(appDir, level) {
  const names = readRouteDirectory(level)
    .filter((entry) => entry.isDirectory() && entry.name.startsWith('@') && entry.name.length > 1)
    .map((entry) => entry.name.slice(1))
    .sort(compareCodePoints)
  const levelPath = routePathFromDir(path.relative(appDir, level))
  const levelId = directoryId(appDir, level)
  const found = []
  for (const name of names) {
    for (const page of interceptPages(path.join(level, `@${name}`))) {
      const target = interceptTargetPath(levelPath, page.marker, page.segments)
      if (target === null) continue
      found.push({ levelDir: level, levelId, name, target, file: page.file })
    }
  }
  return found
}

/** Page files under a slot whose first segment carries a marker. */
function interceptPages(slotDir) {
  const found = []
  const walk = (dir, segments) => {
    for (const entry of readRouteDirectory(dir)) {
      if (entry.isDirectory()) {
        walk(path.join(dir, entry.name), [...segments, entry.name])
        continue
      }
      if (!['page.tsx', 'page.jsx', 'page.md', 'page.mdx'].includes(entry.name)) continue
      const first = segments[0]
      if (first === undefined) continue
      const marker = INTERCEPT_MARKERS.find((candidate) => first.startsWith(candidate))
      if (!marker) continue
      const head = first.slice(marker.length)
      if (head === '') continue
      found.push({
        file: path.join(dir, entry.name),
        marker,
        segments: [head, ...segments.slice(1)],
      })
    }
  }
  walk(slotDir, [])
  return found
}

/** The URL an interception covers, or null when the marker climbs past root. */
function interceptTargetPath(levelPath, marker, segments) {
  const climb = interceptClimb(marker)
  let base
  if (climb === null) {
    base = []
  } else {
    base = levelPath.split('/').filter(Boolean)
    if (base.length < climb) return null
    base = base.slice(0, base.length - climb)
  }
  return `/${[...base, ...segments].join('/')}`
}

/** A directory's URL path, with route groups and slots contributing nothing. */
function routePathFromDir(relative) {
  const visible = relative
    .split(path.sep)
    .filter(Boolean)
    .filter(
      (segment) => !(segment.startsWith('(') && segment.endsWith(')')) && !segment.startsWith('@'),
    )
  return visible.length === 0 ? '/' : `/${visible.join('/')}`
}

/** A directory as a route id (`app/feed`), the spelling both hosts emit. */
function directoryId(appDir, dir) {
  const relative = path.relative(appDir, dir)
  const segments = relative ? relative.split(path.sep).filter(Boolean) : []
  return ['app', ...segments].join('/')
}
