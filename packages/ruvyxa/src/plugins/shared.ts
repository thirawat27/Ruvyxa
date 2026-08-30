import path from 'node:path'
import { randomUUID } from 'node:crypto'
import {
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import type { PluginBuildContext } from '@ruvyxa/core/plugin'

// ─── shared helpers ───────────────────────────────────────────────────────────

export function normalizeSiteUrl(value: unknown, plugin: string): string {
  if (typeof value !== 'string') {
    throw new TypeError(`${plugin}: siteUrl must be an absolute http(s) URL`)
  }
  let parsed: URL
  try {
    parsed = new URL(value)
  } catch {
    throw new TypeError(`${plugin}: siteUrl must be an absolute http(s) URL`)
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new TypeError(`${plugin}: siteUrl must be an absolute http(s) URL`)
  }
  if (
    parsed.username ||
    parsed.password ||
    (parsed.pathname !== '/' && parsed.pathname !== '') ||
    parsed.search ||
    parsed.hash
  ) {
    throw new TypeError(`${plugin}: siteUrl must contain only an http(s) origin`)
  }
  return parsed.href.replace(/\/+$/, '')
}
export function isConcreteApplicationPath(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    value.startsWith('/') &&
    !/[\\?#[\]*]|\p{Cc}/u.test(value) &&
    !value.split('/').some((segment) => segment === '.' || segment === '..')
  )
}

export function stringList(value: string | string[] | undefined, field: string): string[] {
  if (value === undefined) return []
  const values = Array.isArray(value) ? value : [value]
  if (values.some((entry) => typeof entry !== 'string' || entry === '')) {
    throw new TypeError(`${field} must be a non-empty string or string array`)
  }
  return values
}

export function validateRobotsAgent(value: string, ruleIndex: number): void {
  if (value !== '*' && !/^[A-Za-z_-]+$/.test(value)) {
    throw new TypeError(`robots.rules[${ruleIndex}].userAgent must be "*" or a crawler token`)
  }
}

export function validateRobotsPath(value: string, ruleIndex: number): void {
  if (!value.startsWith('/') || /[\r\n\0]/.test(value)) {
    throw new TypeError(
      `robots.rules[${ruleIndex}] paths must start with "/" and contain no controls`,
    )
  }
}

export function validateAbsoluteHttpUrl(value: string, field: string): void {
  let parsed: URL
  try {
    parsed = new URL(value)
  } catch {
    throw new TypeError(`${field} must contain absolute http(s) URLs`)
  }
  if (
    (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') ||
    parsed.username ||
    parsed.password ||
    parsed.hash ||
    /[\r\n\0]/.test(value)
  ) {
    throw new TypeError(`${field} must contain absolute http(s) URLs`)
  }
}

export function normalizeRoutes(
  routes: string[] | undefined,
  plugin: string,
): string[] | undefined {
  if (routes === undefined) return undefined
  if (!Array.isArray(routes) || routes.length === 0) {
    throw new TypeError(`${plugin}: routes must be a non-empty array when provided`)
  }
  return uniqueStrings(
    routes.map((route, index) => {
      validateRoutePattern(route, `${plugin}.routes[${index}]`)
      return route
    }),
  )
}

export function validateRoutePattern(value: unknown, field: string): asserts value is string {
  if (
    typeof value !== 'string' ||
    (value !== '*' && !value.startsWith('/')) ||
    (value.includes('*') && value !== '*' && !value.endsWith('*')) ||
    (value !== '*' && value.slice(0, -1).includes('*'))
  ) {
    throw new TypeError(`${field} must be "*", an exact /path, or a /prefix/* pattern`)
  }
}

// ─── fixed-window rate limiting ──────────────────────────────────────────────
//
// The fifth limiter in the workspace. `crates/ruvyxa_middleware/src/lib.rs`
// carries the contract every one of them keeps — bounded memory whatever the
// caller wrote, a slot taken back rather than a refusal at capacity, and one
// identity per bucket — and its table names this one. The algorithm and the
// bucket shape are the deployed host's `consumeFixedWindow` in
// `packages/ruvyxa/runtime/serverless-handler.mjs`, and
// `tests/fixtures/rate-limit-conformance.json` is replayed against this copy
// too, because a limiter that agrees with the others only by inspection is a
// limiter that will stop agreeing.
//
// This is a separate copy rather than an import: `serverless-handler.mjs` is
// the module every adapter copies verbatim into a function bundle, and a plugin
// barrel loaded by `ruvyxa.config.ts` has no business pulling that graph in to
// borrow thirty lines.

export const MAX_TRACKED_PLUGIN_RATE_LIMIT_KEYS = 10_000

/** The widest a tracked key may be, matching the deployed host's bound. */
const MAX_PLUGIN_RATE_LIMIT_KEY_LENGTH = 64

export interface FixedWindowBucket {
  remaining: number
  startedAt: number
}

/**
 * Sixteen hex digits that separate two identities sharing a long prefix.
 *
 * Not a cryptographic hash and not offered as one: two thirty-two-bit FNV-1a
 * passes with different offset bases, spliced. It is only ever appended after
 * the first characters of the identity it describes, so producing the same
 * bounded key as another client still means reproducing that client's prefix.
 */
function identityDigest(value: string): string {
  let low = 0x811c9dc5
  let high = 0x01000193
  const bytes = new TextEncoder().encode(value)
  for (const byte of bytes) {
    low = Math.imul(low ^ byte, 0x01000193)
    high = Math.imul(high ^ byte, 0x85ebca6b)
  }
  return `${(low >>> 0).toString(16).padStart(8, '0')}${(high >>> 0).toString(16).padStart(8, '0')}`
}

/**
 * The fixed-width map key one identity is tracked under.
 *
 * The identity is whatever a project's own resolver returned, so its length is
 * not ours to assume: ten thousand unbounded keys retain as much memory as the
 * caller cares to spend. Truncating alone would collapse two identities that
 * share a long prefix into one bucket, and two clients sharing a bucket means
 * either can limit the other — so a longer identity is truncated *onto* a
 * digest of the whole.
 */
export function boundedRateLimitKey(identity: string): string {
  const value = String(identity)
  if (value.length <= MAX_PLUGIN_RATE_LIMIT_KEY_LENGTH) return value
  return `${value.slice(0, MAX_PLUGIN_RATE_LIMIT_KEY_LENGTH - 17)}#${identityDigest(value)}`
}

/**
 * Spend one unit from a fixed-window bucket.
 *
 * Answers `null` when the request is admitted, and the whole seconds a caller
 * should wait when it is not.
 *
 * At capacity the map sweeps buckets whose window has fully elapsed and then
 * evicts the least recently started bucket rather than refusing the arrival:
 * the limiter is out of slots, not out of answers, and a slot can be taken
 * back. Refusing there would let one caller rotating its identity deny the
 * endpoint to every client the map had not already seen.
 */
export function consumeFixedWindow(
  buckets: Map<string, FixedWindowBucket>,
  key: string,
  max: number,
  windowSeconds: number,
): number | null {
  const now = Date.now()
  const windowMs = windowSeconds * 1000
  let bucket = buckets.get(key)
  if (bucket && now - bucket.startedAt >= windowMs) {
    buckets.delete(key)
    bucket = undefined
  }
  if (!bucket) {
    if (buckets.size >= MAX_TRACKED_PLUGIN_RATE_LIMIT_KEYS) {
      for (const [trackedKey, tracked] of buckets) {
        if (now - tracked.startedAt >= windowMs) buckets.delete(trackedKey)
      }
      while (buckets.size >= MAX_TRACKED_PLUGIN_RATE_LIMIT_KEYS) {
        const oldest = leastRecentlyStartedKey(buckets)
        if (oldest === undefined) break
        buckets.delete(oldest)
      }
    }
    bucket = { remaining: max, startedAt: now }
    buckets.set(key, bucket)
  }
  if (bucket.remaining > 0) {
    bucket.remaining -= 1
    return null
  }
  return Math.max(1, Math.ceil((windowMs - (now - bucket.startedAt)) / 1000))
}

/**
 * The bucket that started longest ago.
 *
 * Scanned rather than read off the Map's insertion order, which stops being the
 * same answer the day a bucket is restarted in place.
 */
function leastRecentlyStartedKey(buckets: Map<string, FixedWindowBucket>): string | undefined {
  let oldestKey: string | undefined
  let oldestStartedAt = Infinity
  for (const [key, bucket] of buckets) {
    if (bucket.startedAt < oldestStartedAt) {
      oldestStartedAt = bucket.startedAt
      oldestKey = key
    }
  }
  return oldestKey
}

export function normalizeHeaderName(value: string, field: string): string {
  try {
    const probe = new Headers()
    probe.set(value, 'value')
    return value.toLowerCase()
  } catch {
    throw new TypeError(`${field} must be a valid HTTP header name`)
  }
}

export function cloneResponse(response: Response, headers: Headers): Response {
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  })
}

export function appendHeaderValue(headers: Headers, name: string, value: string): void {
  const existing = headers.get(name)
  headers.set(name, existing ? `${existing}, ${value}` : value)
}

export function mergeVary(headers: Headers, values: string[]): void {
  if (values.length === 0) return
  const current = (headers.get('vary') ?? '')
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean)
  if (current.includes('*')) return
  const seen = new Set(current.map((value) => value.toLowerCase()))
  for (const value of values) {
    if (value === '*') {
      headers.set('vary', '*')
      return
    }
    const normalized = value.toLowerCase()
    if (!seen.has(normalized)) {
      current.push(value)
      seen.add(normalized)
    }
  }
  headers.set('vary', current.join(', '))
}

export function normalizePublicPath(value: unknown, plugin: string): string {
  let decoded: string | undefined
  try {
    decoded = typeof value === 'string' ? decodeURIComponent(value) : undefined
  } catch {
    decoded = undefined
  }
  if (
    typeof value !== 'string' ||
    decoded === undefined ||
    !value.startsWith('/') ||
    value.startsWith('//') ||
    value.includes('\\') ||
    value.includes('?') ||
    value.includes('#') ||
    /%(?:2f|5c)/i.test(value) ||
    /\p{Cc}/u.test(decoded) ||
    decoded.startsWith('//') ||
    decoded.includes('\\') ||
    decoded.split('/').some((segment) => segment === '..' || segment === '.')
  ) {
    throw new TypeError(
      `${plugin}: public paths must be same-origin absolute paths without traversal`,
    )
  }
  return value
}

export function normalizePublicFilePath(value: unknown, plugin: string): string {
  const normalized = normalizePublicPath(value, plugin)
  if (normalized === '/' || normalized.endsWith('/')) {
    throw new TypeError(`${plugin}: public asset path must identify a file`)
  }
  return normalized
}

/**
 * Byte-order string comparison, stable across machines.
 *
 * Deliberately not `localeCompare`: sitemap, feed, and route listings are build
 * artifacts that have to come out identical everywhere, and `localeCompare`
 * orders by the host's locale.
 */
export function compareStable(left: string, right: string): number {
  if (left < right) return -1
  if (left > right) return 1
  return 0
}

export function normalizeItemUrl(value: string, siteUrl: string, field: string): string {
  let resolved: URL
  try {
    resolved = new URL(value, `${siteUrl}/`)
  } catch {
    throw new TypeError(`${field} must be an absolute URL or site-relative path`)
  }
  if (resolved.protocol !== 'http:' && resolved.protocol !== 'https:') {
    throw new TypeError(`${field} must use http(s)`)
  }
  return resolved.href
}

export function normalizeDate(value: string | Date, field: string): string {
  const date = value instanceof Date ? value : new Date(value)
  if (Number.isNaN(date.getTime())) throw new TypeError(`${field}.publishedAt must be a valid date`)
  return date.toUTCString()
}

export function uniqueStrings(values: string[]): string[] {
  return [...new Set(values)]
}

export function walkFiles(root: string): string[] {
  const files: string[] = []
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const resolved = path.join(root, entry.name)
    if (entry.isDirectory()) files.push(...walkFiles(resolved))
    else if (entry.isFile()) files.push(resolved)
  }
  return files
}

export function isDirectory(value: string): boolean {
  try {
    return statSync(value).isDirectory()
  } catch {
    return false
  }
}

export function manifestPagePaths(context: PluginBuildContext): string[] {
  // The build-complete manifest summarizes the build; the full route list
  // lives in the committed route manifest next to the output.
  let routes = (context.manifest as { routes?: unknown }).routes
  if (!Array.isArray(routes)) {
    try {
      const routeManifest = JSON.parse(
        readFileSync(path.join(context.outDir, 'manifest.json'), 'utf8'),
      ) as { routes?: unknown }
      routes = routeManifest.routes
    } catch {
      return []
    }
  }
  if (!Array.isArray(routes)) return []
  const paths: string[] = []
  for (const route of routes) {
    if (!route || typeof route !== 'object') continue
    const { kind, path: routePath } = route as { kind?: unknown; path?: unknown }
    if (kind !== 'page' || typeof routePath !== 'string') continue
    if (routePath.includes('[')) continue
    paths.push(routePath)
  }
  return paths.sort(compareStable)
}

/** Writes into the directory served as `/` by the production server and adapters. */
export function writePublicAsset(
  context: PluginBuildContext,
  fileName: string,
  contents: string,
): void {
  const normalized = normalizePublicFilePath(
    fileName.startsWith('/') ? fileName : `/${fileName}`,
    'built-in plugin',
  ).slice(1)
  const assetsDir = path.join(context.outDir, 'assets')
  const destination = path.join(assetsDir, ...normalized.split('/'))
  mkdirSync(path.dirname(destination), { recursive: true })
  writeFileAtomic(destination, contents)
}

/** Same placement rules as `writePublicAsset`, for bytes rather than text. */
export function writePublicBinaryAsset(
  context: PluginBuildContext,
  fileName: string,
  contents: Buffer,
): void {
  const normalized = normalizePublicFilePath(
    fileName.startsWith('/') ? fileName : `/${fileName}`,
    'built-in plugin',
  ).slice(1)
  const destination = path.join(context.outDir, 'assets', ...normalized.split('/'))
  mkdirSync(path.dirname(destination), { recursive: true })
  writeFileAtomic(destination, contents)
}

/**
 * Publish `contents` at `destination` so a reader sees the whole file or the
 * previous one, never a partial write.
 *
 * Text and bytes went through two copies of this that differed only in the
 * encoding argument — and `writeFileSync` already infers utf8 for a string, so
 * there was nothing for the second copy to carry.
 */
export function writeFileAtomic(destination: string, contents: string | Buffer): void {
  const temporary = `${destination}.tmp-${process.pid}-${randomUUID()}`
  try {
    writeFileSync(temporary, contents)
    renameSync(temporary, destination)
  } finally {
    rmSync(temporary, { force: true })
  }
}

export function escapeXml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll("'", '&apos;')
    .replaceAll('"', '&quot;')
}

export function escapeHtmlAttribute(value: string): string {
  return value.replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;')
}
