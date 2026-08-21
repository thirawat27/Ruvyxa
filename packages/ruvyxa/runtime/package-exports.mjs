/**
 * Which file a bare package specifier names — the JavaScript half of one rule.
 *
 * Ruvyxa resolves imports twice. `crates/ruvyxa_bundler/src/resolver.rs` walks
 * the graph for `ruvyxa build`, and `packages/ruvyxa/runtime/compiler.mjs`
 * walks it again for the dev server, the prerender workers, and every function
 * artifact an adapter assembles. See the two-module-graphs problem: a specifier
 * taught to one and not the other breaks the half nobody is looking at.
 *
 * This module is the `exports`-field decision, written once. The Rust side
 * cannot import it, so both are held to
 * `tests/fixtures/module-resolution-conformance.json`.
 *
 * **The divergence this exists to close.** `compiler.mjs` used to answer bare
 * specifiers with `createRequire(...).resolve(specifier)`, which is Node's
 * *CommonJS* resolver: it matches the conditions `["node", "require"]` and
 * nothing else. So for any dual package the two graphs picked different files —
 * the Rust client bundler took `browser`/`import` while the JavaScript graph
 * inlined the `require` build into the very same browser bundle, and an edge
 * function artifact never saw `worker` or `edge-light` at all. Neither
 * disagreement produced an error; they produced a bundle that ran different
 * code than the one the build reported.
 *
 * Two departures from Node are deliberate and pinned by the shared table:
 *
 * 1. `require` is a **second pass**, not a peer of `import`. Node takes the
 *    first supported condition in author order, so a package writing
 *    `{ "require": "./cjs.js", "import": "./esm.mjs" }` hands CommonJS to a
 *    bundler that emits ESM and had the ESM build sitting right beside it.
 *    Within each pass the author's order still decides, so conditions that
 *    legitimately compete (`browser` before `import`) are unaffected.
 * 2. A subpath an `exports` map does not cover falls through to the legacy
 *    `browser`/`module`/`main` fields instead of failing outright, and only an
 *    explicit `null` blocks.
 *
 * This module must stay dependency-free and free of Node and DOM APIs: it is
 * read by `compiler.mjs`, which is copied into worker and function directories
 * where nothing else is resolvable.
 */

/**
 * Condition names each bundle target accepts, in two passes.
 *
 * Mirrors `resolve_exports_value` in `crates/ruvyxa_bundler/src/resolver.rs`.
 * `edge` has no fallback pass on purpose: a Worker cannot run a CommonJS build,
 * so falling back to `require` there would ship something that cannot load.
 */
export const PACKAGE_EXPORT_CONDITIONS = Object.freeze({
  client: Object.freeze({
    preferred: Object.freeze(['browser', 'import', 'module', 'default']),
    fallback: Object.freeze(['require']),
  }),
  ssr: Object.freeze({
    preferred: Object.freeze(['node', 'import', 'module', 'default']),
    fallback: Object.freeze(['require']),
  }),
  edge: Object.freeze({
    preferred: Object.freeze(['worker', 'edge-light', 'import', 'module', 'default']),
    fallback: Object.freeze([]),
  }),
  // `react-server` first, ahead of `node`, because a package that ships a
  // server-components build lists it as a narrower case of the same runtime —
  // React's own `exports` does exactly that, and taking `node` instead would
  // load the build with `useState` in it and make every server component throw.
  'react-server': Object.freeze({
    preferred: Object.freeze(['react-server', 'node', 'import', 'module', 'default']),
    fallback: Object.freeze(['require']),
  }),
})

/** Every bundle target this rule knows, for validation at the call site. */
export const PACKAGE_EXPORT_TARGETS = Object.freeze(Object.keys(PACKAGE_EXPORT_CONDITIONS))

const UNMATCHED = Object.freeze({ kind: 'unmatched' })
const BLOCKED = Object.freeze({ kind: 'blocked' })

function targets(list) {
  return list.length === 0 ? UNMATCHED : { kind: 'targets', targets: list }
}

/**
 * Split a package specifier into its directory name and `exports` key.
 *
 * `react` → `{ name: 'react', key: '.' }`,
 * `react/jsx-runtime` → `{ name: 'react', key: './jsx-runtime' }`,
 * `@scope/pkg/sub` → `{ name: '@scope/pkg', key: './sub' }`.
 */
export function packageNameAndExportKey(specifier) {
  if (typeof specifier !== 'string') return null
  if (specifier === '' || specifier.startsWith('.') || specifier.startsWith('/')) return null

  if (specifier.startsWith('@')) {
    const [scope, name, ...rest] = specifier.split('/')
    if (!scope || !name) return null
    const subpath = rest.join('/')
    return { name: `${scope}/${name}`, key: subpath === '' ? '.' : `./${subpath}` }
  }

  const slash = specifier.indexOf('/')
  if (slash < 0) return { name: specifier, key: '.' }
  return { name: specifier.slice(0, slash), key: `./${specifier.slice(slash + 1)}` }
}

/**
 * Resolve one `exports` value for a target, substituting a wildcard match.
 *
 * Returns `{kind:'targets',targets}` with package-relative paths in preference
 * order, `{kind:'blocked'}` for an explicit `null`, or `{kind:'unmatched'}`.
 */
function resolveExportsValue(value, target, wildcard) {
  if (value === null) return BLOCKED
  if (typeof value === 'string') {
    const path = wildcard === undefined ? value : value.split('*').join(wildcard)
    return path.startsWith('./') ? targets([path]) : UNMATCHED
  }
  if (Array.isArray(value)) {
    const collected = []
    for (const entry of value) {
      const resolved = resolveExportsValue(entry, target, wildcard)
      if (resolved.kind === 'targets') collected.push(...resolved.targets)
      else if (resolved.kind === 'blocked' && collected.length === 0) return BLOCKED
    }
    return targets(collected)
  }
  if (typeof value !== 'object') return UNMATCHED

  const conditions = PACKAGE_EXPORT_CONDITIONS[target]
  for (const pass of [conditions.preferred, conditions.fallback]) {
    for (const [condition, entry] of Object.entries(value)) {
      if (!pass.includes(condition)) continue
      const resolved = resolveExportsValue(entry, target, wildcard)
      if (resolved.kind !== 'unmatched') return resolved
    }
  }
  return UNMATCHED
}

/** Match `key` against the subpath patterns of an `exports` map. */
function resolveExportsSubpath(map, key, target) {
  if (Object.hasOwn(map, key)) return resolveExportsValue(map[key], target, undefined)

  let best = null
  for (const [pattern, value] of Object.entries(map)) {
    const star = pattern.indexOf('*')
    if (star < 0 || pattern.indexOf('*', star + 1) >= 0) continue
    const prefix = pattern.slice(0, star)
    const suffix = pattern.slice(star + 1)
    if (!key.startsWith(prefix) || !key.endsWith(suffix)) continue
    if (key.length < prefix.length + suffix.length) continue
    // Longest prefix wins, then longest suffix — the same tie-break Node uses,
    // so `./*` never shadows `./feature/*`.
    if (
      best === null ||
      prefix.length > best.prefixLength ||
      (prefix.length === best.prefixLength && suffix.length > best.suffixLength)
    ) {
      best = {
        prefixLength: prefix.length,
        suffixLength: suffix.length,
        wildcard: key.slice(prefix.length, key.length - suffix.length),
        value,
      }
    }
  }
  return best === null ? UNMATCHED : resolveExportsValue(best.value, target, best.wildcard)
}

/**
 * Walk a package's `exports` field for one subpath and bundle target.
 *
 * Mirrors `resolve_exports_entry`. An `exports` map whose keys all start with
 * `.` is a subpath map; anything else is a bare condition map that only answers
 * the `.` key.
 */
export function resolveExportsEntry(exports, key, target) {
  if (!Object.hasOwn(PACKAGE_EXPORT_CONDITIONS, target)) {
    throw new TypeError(
      `RUV1810 unknown bundle target "${target}"; expected one of ${PACKAGE_EXPORT_TARGETS.join(', ')}`,
    )
  }
  if (exports === null) return BLOCKED
  if (exports === undefined) return UNMATCHED
  if (typeof exports === 'string' || Array.isArray(exports)) {
    return key === '.' ? resolveExportsValue(exports, target, undefined) : UNMATCHED
  }
  if (typeof exports !== 'object') return UNMATCHED

  const keys = Object.keys(exports)
  if (keys.some((entry) => entry.startsWith('.')))
    return resolveExportsSubpath(exports, key, target)
  // A map with no `.`-prefixed key is sugar for `{ ".": <this map> }`, so it
  // defines the root entry and nothing else. Answering a subpath from it
  // resolved `pkg/sub` to the package's *root* file — a wrong file, silently,
  // where leaving it unmatched falls through to the legacy branch and probes
  // `pkg/sub` itself. Node refuses the subpath outright here
  // (ERR_PACKAGE_PATH_NOT_EXPORTED); falling through is the documented
  // divergence, taking the wrong file was a defect.
  return key === '.' ? resolveExportsValue(exports, target, undefined) : UNMATCHED
}

/**
 * Package-relative candidates for a package with no usable `exports` entry.
 *
 * Mirrors `resolve_legacy_entry`. `browser` only competes for a browser bundle:
 * on the server it swaps in a build that assumes `window`. The trailing
 * `index` is the directory-layout fallback, not a field.
 */
export function legacyEntryCandidates(manifest, key, target) {
  if (key !== '.') return [key.slice('./'.length)]
  const candidates = []
  // Only the string form of `browser` names an entry point; the object form is
  // a per-file substitution map and is deliberately ignored rather than
  // half-honoured.
  if (target === 'client' && typeof manifest?.browser === 'string')
    candidates.push(manifest.browser)
  if (typeof manifest?.module === 'string') candidates.push(manifest.module)
  if (typeof manifest?.main === 'string') candidates.push(manifest.main)
  candidates.push('index')
  return candidates.map((candidate) =>
    candidate.startsWith('./') ? candidate.slice('./'.length) : candidate,
  )
}

/**
 * Whether a package-relative path may be joined onto the package directory.
 *
 * Refuses anything that could climb out of the package: a `..` segment, an
 * absolute path, or a backslash that a POSIX join would treat as an ordinary
 * character while Windows treats it as a separator.
 */
export function isSafePackageRelativePath(relative) {
  if (typeof relative !== 'string' || relative === '') return false
  if (relative.includes('\\')) return false
  if (relative.startsWith('/')) return false
  return relative
    .split('/')
    .every((segment) => segment !== '' && segment !== '.' && segment !== '..')
}
