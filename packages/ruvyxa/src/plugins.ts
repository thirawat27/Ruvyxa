/**
 * First-party Ruvyxa plugins, ready to drop into `ruvyxa.config.ts`:
 *
 * ```ts
 * import { redirects, headers, sitemap, robots, alias } from 'ruvyxa/plugins'
 *
 * export default config({
 *   plugins: [
 *     redirects([{ source: '/old-blog/*', destination: '/blog/*', permanent: true }]),
 *     headers([{ source: '/api/*', headers: { 'cache-control': 'no-store' } }]),
 *     sitemap({ siteUrl: 'https://example.com', robots: true }),
 *   ],
 * })
 * ```
 *
 * Every plugin uses only the public plugin API, so custom plugins can compose
 * with them freely. Route patterns follow middleware semantics: `*` matches
 * everything, a trailing `*` matches by prefix, anything else matches exactly.
 */

import { mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import path from 'node:path'

import { definePlugin } from '@ruvyxa/core/config'
import type { PluginBuildContext, RuvyxaPlugin } from '@ruvyxa/core/config'

// ─── redirects ────────────────────────────────────────────────────────────────

export interface RedirectRule {
  /** Exact path or prefix pattern ending in `*`, e.g. `/old-blog/*`. */
  source: string
  /**
   * Destination path or absolute URL. When `source` ends in `*` and the
   * destination also ends in `*`, the matched remainder is appended.
   */
  destination: string
  /** Respond with 308 (cached by browsers) instead of 307. @default false */
  permanent?: boolean
}

/**
 * Declarative route redirects served before rendering, Next.js-style.
 * Sources are reported as middleware routes, so non-matching requests skip
 * the plugin round-trip entirely.
 */
export function redirects(rules: RedirectRule[]): RuvyxaPlugin {
  const normalized = rules.map((rule, index) => {
    if (!rule || typeof rule.source !== 'string' || !rule.source.startsWith('/')) {
      throw new TypeError(`redirects: rules[${index}].source must be a path starting with "/"`)
    }
    if (typeof rule.destination !== 'string' || rule.destination.length === 0) {
      throw new TypeError(`redirects: rules[${index}].destination must be a non-empty string`)
    }
    return { ...rule, permanent: rule.permanent === true }
  })

  return definePlugin({
    name: 'ruvyxa:redirects',
    setup({ addMiddleware }) {
      addMiddleware({
        routes: normalized.map((rule) => rule.source),
        onRequest(request) {
          const url = new URL(request.url)
          for (const rule of normalized) {
            const remainder = matchSource(rule.source, url.pathname)
            if (remainder === null) continue
            let destination = rule.destination
            if (destination.endsWith('*')) {
              destination = destination.slice(0, -1) + (remainder ?? '')
            }
            const location = destination.includes('://') ? destination : destination + url.search
            return new Response(null, {
              status: rule.permanent ? 308 : 307,
              headers: { location },
            })
          }
          return undefined
        },
      })
    },
  })
}

/** Returns the wildcard remainder, `''` for exact matches, or `null` for no match. */
function matchSource(source: string, pathname: string): string | null {
  if (source === '*') return pathname
  if (source.endsWith('*')) {
    const prefix = source.slice(0, -1)
    return pathname.startsWith(prefix) ? pathname.slice(prefix.length) : null
  }
  return pathname === source ? '' : null
}

// ─── headers ──────────────────────────────────────────────────────────────────

export interface HeaderRule {
  /** Exact path or prefix pattern ending in `*`. Omit to match every route. */
  source?: string
  /** Header names and values set on matching responses. */
  headers: Record<string, string>
}

/**
 * Declarative response headers per route, Next.js-style. Rules with a
 * `source` are route-scoped, so unmatched responses stream through untouched.
 */
export function headers(rules: HeaderRule[]): RuvyxaPlugin {
  const normalized = rules.map((rule, index) => {
    if (!rule || typeof rule.headers !== 'object' || rule.headers === null) {
      throw new TypeError(`headers: rules[${index}].headers must be an object`)
    }
    if (rule.source !== undefined && (typeof rule.source !== 'string' || rule.source === '')) {
      throw new TypeError(`headers: rules[${index}].source must be a non-empty string`)
    }
    return { source: rule.source, headers: Object.entries(rule.headers) }
  })
  const scoped = normalized.every((rule) => rule.source !== undefined)

  return definePlugin({
    name: 'ruvyxa:headers',
    setup({ addMiddleware }) {
      addMiddleware({
        ...(scoped ? { routes: normalized.map((rule) => rule.source as string) } : {}),
        onResponse(request, response) {
          const pathname = new URL(request.url).pathname
          let output: Headers | undefined
          for (const rule of normalized) {
            if (rule.source !== undefined && matchSource(rule.source, pathname) === null) continue
            output ??= new Headers(response.headers)
            for (const [name, value] of rule.headers) output.set(name, value)
          }
          if (!output) return undefined
          return new Response(response.body, {
            status: response.status,
            statusText: response.statusText,
            headers: output,
          })
        },
      })
    },
  })
}

// ─── sitemap / robots ─────────────────────────────────────────────────────────

export interface SitemapOptions {
  /** Absolute site origin, e.g. `https://example.com`. Required. */
  siteUrl: string
  /** Route paths or trailing-`*` patterns excluded from the sitemap. */
  exclude?: string[]
  /** Also write a `robots.txt` referencing the sitemap. @default false */
  robots?: boolean
}

/**
 * Generates `sitemap.xml` (and optionally `robots.txt`) into the build's
 * public asset directory after every production build, using the route
 * manifest. Dynamic route patterns and non-page routes are skipped.
 */
export function sitemap(options: SitemapOptions): RuvyxaPlugin {
  const siteUrl = normalizeSiteUrl(options?.siteUrl, 'sitemap')
  const exclude = options.exclude ?? []

  return definePlugin({
    name: 'ruvyxa:sitemap',
    setup({ onBuildComplete }) {
      onBuildComplete((context) => {
        const paths = manifestPagePaths(context).filter(
          (routePath) => !exclude.some((pattern) => matchSource(pattern, routePath) !== null),
        )
        const entries = paths
          .map((routePath) => `  <url><loc>${escapeXml(siteUrl + routePath)}</loc></url>`)
          .join('\n')
        const xml = `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${entries}\n</urlset>\n`
        writePublicAsset(context, 'sitemap.xml', xml)
        if (options.robots === true) {
          writePublicAsset(
            context,
            'robots.txt',
            `User-agent: *\nAllow: /\n\nSitemap: ${siteUrl}/sitemap.xml\n`,
          )
        }
      })
    },
  })
}

export interface RobotsRule {
  /** @default "*" */
  userAgent?: string
  allow?: string[]
  disallow?: string[]
}

export interface RobotsOptions {
  /** Access rules per user agent. Defaults to allowing everything. */
  rules?: RobotsRule[]
  /** Absolute sitemap URL appended as a `Sitemap:` line. */
  sitemap?: string
}

/** Generates `robots.txt` into the build's public asset directory. */
export function robots(options: RobotsOptions = {}): RuvyxaPlugin {
  const rules = options.rules?.length ? options.rules : [{ userAgent: '*', allow: ['/'] }]

  return definePlugin({
    name: 'ruvyxa:robots',
    setup({ onBuildComplete }) {
      onBuildComplete((context) => {
        const blocks = rules.map((rule) => {
          const lines = [`User-agent: ${rule.userAgent ?? '*'}`]
          for (const value of rule.allow ?? []) lines.push(`Allow: ${value}`)
          for (const value of rule.disallow ?? []) lines.push(`Disallow: ${value}`)
          return lines.join('\n')
        })
        let body = blocks.join('\n\n') + '\n'
        if (options.sitemap) body += `\nSitemap: ${options.sitemap}\n`
        writePublicAsset(context, 'robots.txt', body)
      })
    },
  })
}

// ─── alias ────────────────────────────────────────────────────────────────────

/**
 * Resolves exact import specifiers to project files before the native
 * resolver, e.g. `alias({ '~content': 'content/index.ts' })`. Targets are
 * resolved from the project root.
 */
export function alias(map: Record<string, string>): RuvyxaPlugin {
  const entries = Object.entries(map)
  for (const [specifier, target] of entries) {
    if (specifier === '' || typeof target !== 'string' || target === '') {
      throw new TypeError('alias: every entry needs a non-empty specifier and target path')
    }
  }

  return definePlugin({
    name: 'ruvyxa:alias',
    setup({ resolveId }) {
      resolveId((id, _importer, context) => {
        for (const [specifier, target] of entries) {
          if (id === specifier) return path.resolve(context.root, target)
        }
        return undefined
      })
    },
  })
}

// ─── bundleBudget ─────────────────────────────────────────────────────────────

export interface BundleBudgetOptions {
  /** Maximum size in KiB for any single client JavaScript file. */
  maxChunkKb?: number
  /** Maximum combined size in KiB of all client JavaScript files. */
  maxTotalKb?: number
}

/**
 * Fails the production build when emitted client JavaScript exceeds the
 * configured budget, so bundle regressions surface in CI instead of in
 * production. Sizes are measured on the final minified output.
 */
export function bundleBudget(options: BundleBudgetOptions): RuvyxaPlugin {
  const { maxChunkKb, maxTotalKb } = options ?? {}
  for (const [name, value] of Object.entries({ maxChunkKb, maxTotalKb })) {
    if (value !== undefined && (typeof value !== 'number' || !(value > 0))) {
      throw new TypeError(`bundleBudget: ${name} must be a positive number of KiB`)
    }
  }
  if (maxChunkKb === undefined && maxTotalKb === undefined) {
    throw new TypeError('bundleBudget: set maxChunkKb and/or maxTotalKb')
  }

  return definePlugin({
    name: 'ruvyxa:bundle-budget',
    setup({ onBuildComplete }) {
      onBuildComplete((context) => {
        const clientDir = path.join(context.outDir, 'client')
        const files = clientJavaScriptSizes(clientDir)
        const failures: string[] = []
        if (maxChunkKb !== undefined) {
          for (const file of files) {
            if (file.bytes > maxChunkKb * 1024) {
              failures.push(
                `${file.name} is ${formatKb(file.bytes)} KiB (chunk budget ${maxChunkKb} KiB)`,
              )
            }
          }
        }
        if (maxTotalKb !== undefined) {
          const total = files.reduce((sum, file) => sum + file.bytes, 0)
          if (total > maxTotalKb * 1024) {
            failures.push(
              `client JavaScript totals ${formatKb(total)} KiB (total budget ${maxTotalKb} KiB)`,
            )
          }
        }
        if (failures.length > 0) {
          throw new Error(`bundle budget exceeded:\n- ${failures.join('\n- ')}`)
        }
      })
    },
  })
}

function clientJavaScriptSizes(clientDir: string): Array<{ name: string; bytes: number }> {
  let entries: string[]
  try {
    entries = readdirSync(clientDir, { recursive: true }) as string[]
  } catch {
    return []
  }
  const files: Array<{ name: string; bytes: number }> = []
  for (const entry of entries) {
    const name = String(entry)
    if (!name.endsWith('.js') && !name.endsWith('.mjs')) continue
    const stats = statSync(path.join(clientDir, name))
    if (stats.isFile()) files.push({ name: name.replaceAll('\\', '/'), bytes: stats.size })
  }
  return files.sort((a, b) => a.name.localeCompare(b.name))
}

function formatKb(bytes: number): string {
  return (bytes / 1024).toFixed(1)
}

// ─── requireEnv ───────────────────────────────────────────────────────────────

/**
 * Fails the production build when required environment variables are missing
 * or empty, so misconfigured deployments are caught at build time.
 */
export function requireEnv(names: string[]): RuvyxaPlugin {
  if (!Array.isArray(names) || names.length === 0 || names.some((name) => !name)) {
    throw new TypeError('requireEnv: pass a non-empty array of variable names')
  }

  return definePlugin({
    name: 'ruvyxa:require-env',
    setup({ onBuildComplete }) {
      onBuildComplete(() => {
        const missing = names.filter((name) => {
          const value = process.env[name]
          return value === undefined || value === ''
        })
        if (missing.length > 0) {
          throw new Error(`missing required environment variables: ${missing.join(', ')}`)
        }
      })
    },
  })
}

// ─── shared helpers ───────────────────────────────────────────────────────────

function normalizeSiteUrl(value: unknown, plugin: string): string {
  if (typeof value !== 'string' || !/^https?:\/\//.test(value)) {
    throw new TypeError(`${plugin}: siteUrl must be an absolute http(s) URL`)
  }
  return value.replace(/\/+$/, '')
}

function manifestPagePaths(context: PluginBuildContext): string[] {
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
  return paths.sort()
}

/** Writes into the directory served as `/` by the production server and adapters. */
function writePublicAsset(context: PluginBuildContext, fileName: string, contents: string): void {
  const assetsDir = path.join(context.outDir, 'assets')
  mkdirSync(assetsDir, { recursive: true })
  writeFileSync(path.join(assetsDir, fileName), contents, 'utf8')
}

function escapeXml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll("'", '&apos;')
    .replaceAll('"', '&quot;')
}
