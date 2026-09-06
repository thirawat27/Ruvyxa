import path from 'node:path'
import { randomUUID } from 'node:crypto'
import { mkdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'

// ─── shared helpers for the content engine ───────────────────────────────────

export function normalizeSiteUrl(value: unknown, owner: string): string {
  if (typeof value !== 'string') {
    throw new TypeError(`${owner}: siteUrl must be an absolute http(s) URL`)
  }
  let parsed: URL
  try {
    parsed = new URL(value)
  } catch {
    throw new TypeError(`${owner}: siteUrl must be an absolute http(s) URL`)
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new TypeError(`${owner}: siteUrl must be an absolute http(s) URL`)
  }
  if (
    parsed.username ||
    parsed.password ||
    (parsed.pathname !== '/' && parsed.pathname !== '') ||
    parsed.search ||
    parsed.hash
  ) {
    throw new TypeError(`${owner}: siteUrl must contain only an http(s) origin`)
  }
  return parsed.href.replace(/\/+$/, '')
}

export function normalizeRoutes(routes: string[] | undefined, owner: string): string[] | undefined {
  if (routes === undefined) return undefined
  if (!Array.isArray(routes) || routes.length === 0) {
    throw new TypeError(`${owner}: routes must be a non-empty array when provided`)
  }
  return uniqueStrings(
    routes.map((route, index) => {
      validateRoutePattern(route, `${owner}.routes[${index}]`)
      return route
    }),
  )
}

function validateRoutePattern(value: unknown, field: string): asserts value is string {
  if (
    typeof value !== 'string' ||
    (value !== '*' && !value.startsWith('/')) ||
    (value.includes('*') && value !== '*' && !value.endsWith('*')) ||
    (value !== '*' && value.slice(0, -1).includes('*'))
  ) {
    throw new TypeError(`${field} must be "*", an exact /path, or a /prefix/* pattern`)
  }
}

/** Returns the wildcard remainder, `''` for exact matches, or `null` for no match. */
export function matchSource(source: string, pathname: string): string | null {
  if (source === '*') return pathname
  if (source.endsWith('*')) {
    const prefix = source.slice(0, -1)
    return pathname.startsWith(prefix) ? pathname.slice(prefix.length) : null
  }
  return pathname === source ? '' : null
}

function normalizePublicPath(value: unknown, owner: string): string {
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
      `${owner}: public paths must be same-origin absolute paths without traversal`,
    )
  }
  return value
}

export function normalizePublicFilePath(value: unknown, owner: string): string {
  const normalized = normalizePublicPath(value, owner)
  if (normalized === '/' || normalized.endsWith('/')) {
    throw new TypeError(`${owner}: public asset path must identify a file`)
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

export function isDirectory(value: string): boolean {
  try {
    return statSync(value).isDirectory()
  } catch {
    return false
  }
}

/** Writes into the directory served as `/` by the production server and adapters. */
export function writePublicAsset(outDir: string, fileName: string, contents: string): void {
  const normalized = normalizePublicFilePath(
    fileName.startsWith('/') ? fileName : `/${fileName}`,
    'content engine',
  ).slice(1)
  const destination = path.join(outDir, 'assets', ...normalized.split('/'))
  mkdirSync(path.dirname(destination), { recursive: true })
  writeFileAtomic(destination, contents)
}

/**
 * Publish `contents` at `destination` so a reader sees the whole file or the
 * previous one, never a partial write.
 */
function writeFileAtomic(destination: string, contents: string | Buffer): void {
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
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&apos;')
}
