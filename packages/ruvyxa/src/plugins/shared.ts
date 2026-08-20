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
