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

import { createHash, randomBytes, randomUUID } from 'node:crypto'
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import path from 'node:path'
import { isMap, isScalar, isSeq, parseDocument } from 'yaml'

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
    if (
      !rule ||
      typeof rule.source !== 'string' ||
      (rule.source !== '*' && !rule.source.startsWith('/'))
    ) {
      throw new TypeError(
        `redirects: rules[${index}].source must be "*" or a path starting with "/"`,
      )
    }
    if (typeof rule.destination !== 'string' || rule.destination.length === 0) {
      throw new TypeError(`redirects: rules[${index}].destination must be a non-empty string`)
    }
    assertRedirectDestination(rule.destination, `redirects: rules[${index}].destination`)
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
            const location = redirectLocation(rule.destination, remainder, url.search)
            // A rule whose interpolated destination leaves the intended origin
            // is skipped rather than sent: the remainder is request-controlled.
            if (location === null) continue
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

/**
 * Base used to decide whether a redirect destination stays on the requesting
 * origin. The same technique guards `returnTo` in `@ruvyxa/auth`: resolve
 * against a fixed base and require the origin to survive, so every
 * normalization trick a browser performs (`//host`, `/\host`, folded tabs)
 * collapses into a detectable origin change instead of a silent escape.
 */
const REDIRECT_ORIGIN_PROBE = 'http://ruvyxa.invalid'

const ABSOLUTE_URL_PATTERN = /^[a-z][a-z0-9+.-]*:\/\//i

/**
 * Reject a destination that is neither an absolute http(s) URL nor a
 * same-origin absolute path, at config time.
 *
 * `//evil.example` and a bare `*` are both "non-empty strings" that a browser
 * reads as another origin, so accepting them turned a redirect rule into an
 * open redirect as soon as the request path was interpolated into it.
 */
function assertRedirectDestination(destination: string, field: string): void {
  const base = destination.endsWith('*') ? destination.slice(0, -1) : destination
  if (ABSOLUTE_URL_PATTERN.test(base)) {
    let parsed: URL
    try {
      parsed = new URL(base)
    } catch {
      throw new TypeError(`${field} must be an absolute http(s) URL or a path starting with "/"`)
    }
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      throw new TypeError(`${field} must use http(s) when it is an absolute URL`)
    }
    return
  }
  if (!base.startsWith('/') || base.startsWith('//') || base.includes('\\')) {
    throw new TypeError(`${field} must be an absolute http(s) URL or a path starting with "/"`)
  }
}

/**
 * Build the `Location` for a matched rule, or `null` when interpolating the
 * request-controlled remainder would move the redirect off the intended
 * origin.
 *
 * The origin a rule is allowed to reach is fixed by its configured
 * destination: an absolute destination pins its own origin, a path destination
 * pins the requesting origin. Only the path, query, and fragment may come from
 * the request.
 */
function redirectLocation(
  destination: string,
  remainder: string | null,
  search: string,
): string | null {
  const wildcard = destination.endsWith('*')
  const base = wildcard ? destination.slice(0, -1) : destination
  const absolute = ABSOLUTE_URL_PATTERN.test(base)

  let expectedOrigin: string
  let resolved: URL
  try {
    expectedOrigin = absolute ? new URL(base).origin : REDIRECT_ORIGIN_PROBE
    resolved = new URL(wildcard ? base + (remainder ?? '') : base, REDIRECT_ORIGIN_PROBE)
  } catch {
    return null
  }
  if (resolved.origin !== expectedOrigin) return null

  if (absolute) return resolved.href
  // A path destination carries the original query forward unless the rule
  // already specified one, matching the documented behavior.
  return `${resolved.pathname}${resolved.search || search}${resolved.hash}`
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

// ─── observability ───────────────────────────────────────────────────────────

export interface ObservabilityEntry {
  requestId: string
  traceparent: string
  method: string
  pathname: string
  status: number
  durationMs: number
}

export interface ObservabilityOptions {
  /** Exact paths or trailing-`*` prefixes. Omit to observe every route. */
  routes?: string[]
  /** Response/request correlation header. @default "x-request-id" */
  requestIdHeader?: string
  /** Emit a W3C trace context header when the request does not contain one. @default true */
  traceContext?: boolean
  /** Add a `Server-Timing` metric. @default true */
  serverTiming?: boolean
  /** Emit one JSON record per response. @default true */
  log?: boolean
  /** Custom structured log sink. Defaults to `console.info(JSON.stringify(entry))`. */
  logger?: (entry: ObservabilityEntry) => void
}

const OBSERVABILITY_START_HEADER = 'x-ruvyxa-observability-start'
const TRACEPARENT_PATTERN = /^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$/i
const REQUEST_ID_PATTERN = /^[A-Za-z0-9._:-]{1,128}$/

/** Adds request IDs, W3C trace context, timing, and privacy-safe structured logs. */
export function observability(options: ObservabilityOptions = {}): RuvyxaPlugin {
  const routes = normalizeRoutes(options.routes, 'observability')
  const requestIdHeader = normalizeHeaderName(
    options.requestIdHeader ?? 'x-request-id',
    'observability.requestIdHeader',
  )
  if (requestIdHeader === OBSERVABILITY_START_HEADER || requestIdHeader === 'traceparent') {
    throw new TypeError('observability: requestIdHeader conflicts with an internal trace header')
  }
  const traceContext = options.traceContext !== false
  const serverTiming = options.serverTiming !== false
  const shouldLog = options.log !== false
  if (options.logger !== undefined && typeof options.logger !== 'function') {
    throw new TypeError('observability: logger must be a function')
  }

  return definePlugin({
    name: 'ruvyxa:observability',
    setup({ addMiddleware }) {
      addMiddleware({
        ...(routes ? { routes } : {}),
        onRequest(request) {
          const headers = new Headers(request.headers)
          const incomingRequestId = headers.get(requestIdHeader)
          if (!incomingRequestId || !REQUEST_ID_PATTERN.test(incomingRequestId)) {
            headers.set(requestIdHeader, randomUUID())
          }
          if (traceContext) {
            const incomingTraceparent = headers.get('traceparent')
            if (!incomingTraceparent || !TRACEPARENT_PATTERN.test(incomingTraceparent)) {
              headers.set('traceparent', createTraceparent())
            }
          }
          // The request is serialized back to Rust after this hook, so this
          // timestamp transports timing state safely across a multi-worker pool.
          headers.set(OBSERVABILITY_START_HEADER, String(Date.now()))
          return new Request(request, { headers })
        },
        onResponse(request, response) {
          const headers = new Headers(response.headers)
          const requestId = request.headers.get(requestIdHeader) ?? randomUUID()
          const traceparent = traceContext
            ? (request.headers.get('traceparent') ?? createTraceparent())
            : (request.headers.get('traceparent') ?? '')
          const startedAt = Number(request.headers.get(OBSERVABILITY_START_HEADER))
          const durationMs = Number.isFinite(startedAt) ? Math.max(0, Date.now() - startedAt) : 0
          headers.set(requestIdHeader, requestId)
          if (traceContext) headers.set('traceparent', traceparent)
          if (serverTiming) appendHeaderValue(headers, 'server-timing', `ruvyxa;dur=${durationMs}`)

          if (shouldLog) {
            const entry: ObservabilityEntry = {
              requestId,
              traceparent,
              method: request.method,
              pathname: new URL(request.url).pathname,
              status: response.status,
              durationMs,
            }
            emitObservabilityEntry(options.logger, entry)
          }

          return cloneResponse(response, headers)
        },
      })
    },
  })
}

function createTraceparent(): string {
  return `00-${randomBytes(16).toString('hex')}-${randomBytes(8).toString('hex')}-01`
}

function emitObservabilityEntry(
  logger: ObservabilityOptions['logger'],
  entry: ObservabilityEntry,
): void {
  try {
    if (logger) logger(entry)
    else console.info(JSON.stringify(entry))
  } catch {
    // Telemetry must never turn an otherwise valid response into an HTTP error.
    try {
      console.error('[ruvyxa:observability] log sink failed')
    } catch {
      // Console implementations can also be replaced by application code.
    }
  }
}

// ─── securityHeaders ─────────────────────────────────────────────────────────

export type ContentSecurityPolicy = Record<string, string | string[]>

export interface SecurityHeadersOptions {
  /** Exact paths or trailing-`*` prefixes. Omit to protect every route. */
  routes?: string[]
  /** CSP string or directive map. Omitted by default because application policies differ. */
  contentSecurityPolicy?: string | ContentSecurityPolicy
  /** HSTS policy. @default "max-age=31536000; includeSubDomains" */
  strictTransportSecurity?: string
  permissionsPolicy?: string
  referrerPolicy?: string
  crossOriginOpenerPolicy?: string
  crossOriginEmbedderPolicy?: string
  crossOriginResourcePolicy?: string
  frameOptions?: string
  /** Additional response headers applied after the named options. */
  headers?: Record<string, string>
}

/** Applies route-scoped security policy while preserving framework defaults for omitted headers. */
export function securityHeaders(options: SecurityHeadersOptions = {}): RuvyxaPlugin {
  const routes = normalizeRoutes(options.routes, 'securityHeaders')
  const configured = new Headers()
  const set = (name: string, value: string | undefined) => {
    if (value !== undefined) configured.set(name, value)
  }
  if (options.contentSecurityPolicy !== undefined) {
    set('content-security-policy', serializeContentSecurityPolicy(options.contentSecurityPolicy))
  }
  set(
    'strict-transport-security',
    options.strictTransportSecurity ?? 'max-age=31536000; includeSubDomains',
  )
  set('permissions-policy', options.permissionsPolicy)
  set('referrer-policy', options.referrerPolicy)
  set('cross-origin-opener-policy', options.crossOriginOpenerPolicy)
  set('cross-origin-embedder-policy', options.crossOriginEmbedderPolicy)
  set('cross-origin-resource-policy', options.crossOriginResourcePolicy)
  set('x-frame-options', options.frameOptions)
  for (const [name, value] of Object.entries(options.headers ?? {})) configured.set(name, value)

  return definePlugin({
    name: 'ruvyxa:security-headers',
    setup({ addMiddleware }) {
      addMiddleware({
        ...(routes ? { routes } : {}),
        onResponse(_request, response) {
          const output = new Headers(response.headers)
          configured.forEach((value, name) => output.set(name, value))
          return cloneResponse(response, output)
        },
      })
    },
  })
}

function serializeContentSecurityPolicy(value: string | ContentSecurityPolicy): string {
  if (typeof value === 'string') {
    if (value.trim() === '') throw new TypeError('securityHeaders: CSP must not be empty')
    return value
  }
  if (!value || typeof value !== 'object') {
    throw new TypeError('securityHeaders: CSP must be a string or directive map')
  }
  const directives: string[] = []
  for (const [name, sources] of Object.entries(value)) {
    if (!/^[a-z][a-z0-9-]*$/.test(name)) {
      throw new TypeError(`securityHeaders: invalid CSP directive ${JSON.stringify(name)}`)
    }
    const values = Array.isArray(sources) ? sources : [sources]
    if (values.some((source) => typeof source !== 'string' || /[;\r\n]/.test(source))) {
      throw new TypeError(`securityHeaders: invalid source in CSP directive ${name}`)
    }
    directives.push([name, ...values].join(' '))
  }
  if (directives.length === 0) throw new TypeError('securityHeaders: CSP map must not be empty')
  return directives.join('; ')
}

// ─── cacheRules ──────────────────────────────────────────────────────────────

export interface CacheRule {
  /** Exact path or trailing-`*` prefix. Omit to match every route. */
  source?: string
  /** Browser cache policy written to `Cache-Control`. */
  browser?: string
  /** Shared-CDN policy written to `CDN-Cache-Control`. */
  cdn?: string
  /** Values merged into the response's existing `Vary` header. */
  vary?: string[]
}

/** Applies browser/CDN cache policy per route without replacing unrelated response metadata. */
export function cacheRules(rules: CacheRule[]): RuvyxaPlugin {
  if (!Array.isArray(rules) || rules.length === 0) {
    throw new TypeError('cacheRules: pass a non-empty array of rules')
  }
  const normalized = rules.map((rule, index) => {
    if (!rule || typeof rule !== 'object') {
      throw new TypeError(`cacheRules: rules[${index}] must be an object`)
    }
    if (rule.source !== undefined) validateRoutePattern(rule.source, `cacheRules.rules[${index}]`)
    if (!rule.browser && !rule.cdn && !rule.vary?.length) {
      throw new TypeError(`cacheRules: rules[${index}] must set browser, cdn, and/or vary`)
    }
    const probe = new Headers()
    if (rule.browser !== undefined) probe.set('cache-control', rule.browser)
    if (rule.cdn !== undefined) probe.set('cdn-cache-control', rule.cdn)
    for (const value of rule.vary ?? []) probe.append('vary', value)
    return { ...rule, vary: rule.vary ? [...rule.vary] : undefined }
  })
  const scoped = normalized.every((rule) => rule.source !== undefined)

  return definePlugin({
    name: 'ruvyxa:cache-rules',
    setup({ addMiddleware }) {
      addMiddleware({
        ...(scoped ? { routes: normalized.map((rule) => rule.source as string) } : {}),
        onResponse(request, response) {
          const pathname = new URL(request.url).pathname
          let output: Headers | undefined
          for (const rule of normalized) {
            if (rule.source !== undefined && matchSource(rule.source, pathname) === null) continue
            output ??= new Headers(response.headers)
            if (rule.browser !== undefined) output.set('cache-control', rule.browser)
            if (rule.cdn !== undefined) output.set('cdn-cache-control', rule.cdn)
            mergeVary(output, rule.vary ?? [])
          }
          return output ? cloneResponse(response, output) : undefined
        },
      })
    },
  })
}

// ─── pwa ─────────────────────────────────────────────────────────────────────

export interface PwaIcon {
  src: string
  sizes: string
  type?: string
  purpose?: 'any' | 'maskable' | 'monochrome' | string
}

export interface PwaOptions {
  name: string
  shortName?: string
  description?: string
  startUrl?: string
  scope?: string
  display?: 'browser' | 'fullscreen' | 'minimal-ui' | 'standalone'
  themeColor?: string
  backgroundColor?: string
  icons?: PwaIcon[]
  /** Pages whose HTML receives manifest and registration tags. @default ["*"] */
  routes?: string[]
  /** @default "/manifest.webmanifest" */
  manifestPath?: string
  /** @default "/sw.js" */
  serviceWorkerPath?: string
  /** @default "/pwa-register.js" */
  registerPath?: string
  /** Same-origin files cached during service-worker installation. */
  precache?: string[]
  /** Same-origin document returned when a navigation fails offline. */
  offlineFallback?: string
  /** Change this value to invalidate the plugin-owned cache. @default "v1" */
  version?: string
}

/** Generates a web manifest and service worker, serves them in dev, and wires HTML automatically. */
export function pwa(options: PwaOptions): RuvyxaPlugin {
  if (!options || typeof options.name !== 'string' || options.name.trim() === '') {
    throw new TypeError('pwa: name must be a non-empty string')
  }
  const manifestPath = normalizePublicFilePath(
    options.manifestPath ?? '/manifest.webmanifest',
    'pwa',
  )
  const serviceWorkerPath = normalizePublicFilePath(options.serviceWorkerPath ?? '/sw.js', 'pwa')
  const registerPath = normalizePublicFilePath(options.registerPath ?? '/pwa-register.js', 'pwa')
  if (new Set([manifestPath, serviceWorkerPath, registerPath]).size !== 3) {
    throw new TypeError('pwa: manifestPath, serviceWorkerPath, and registerPath must be distinct')
  }
  const scope = normalizePublicPath(options.scope ?? '/', 'pwa')
  const startUrl = normalizePublicPath(options.startUrl ?? '/', 'pwa')
  const htmlRoutes = normalizeRoutes(options.routes ?? ['*'], 'pwa') as string[]
  const offlineFallback = options.offlineFallback
    ? normalizePublicPath(options.offlineFallback, 'pwa')
    : undefined
  const precache = uniqueStrings([
    manifestPath,
    registerPath,
    ...(options.precache ?? []).map((value) => normalizePublicPath(value, 'pwa')),
    ...(offlineFallback ? [offlineFallback] : []),
  ])
  if (options.version !== undefined && !/^[A-Za-z0-9._-]{1,64}$/.test(options.version)) {
    throw new TypeError('pwa: version must contain only letters, numbers, dot, underscore, or dash')
  }
  const icons = (options.icons ?? []).map((icon, index) => {
    if (
      !icon ||
      typeof icon.src !== 'string' ||
      icon.src === '' ||
      typeof icon.sizes !== 'string' ||
      icon.sizes === ''
    ) {
      throw new TypeError(`pwa: icons[${index}] requires src and sizes strings`)
    }
    return { ...icon, src: normalizePublicPath(icon.src, 'pwa') }
  })
  const manifest = {
    name: options.name,
    short_name: options.shortName ?? options.name,
    ...(options.description ? { description: options.description } : {}),
    start_url: startUrl,
    scope,
    display: options.display ?? 'standalone',
    theme_color: options.themeColor ?? '#111827',
    background_color: options.backgroundColor ?? '#ffffff',
    ...(icons.length > 0 ? { icons } : {}),
  }
  const manifestBody = `${JSON.stringify(manifest, null, 2)}\n`
  const registerBody = createPwaRegistration(serviceWorkerPath, scope)
  const cachePrefix = `ruvyxa-pwa-${createHash('sha256').update(scope).digest('hex').slice(0, 12)}-`
  const serviceWorkerBody = createServiceWorker(
    `${cachePrefix}${options.version ?? 'v1'}`,
    cachePrefix,
    precache,
    offlineFallback,
  )
  const middlewareRoutes = uniqueStrings([
    ...htmlRoutes,
    manifestPath,
    serviceWorkerPath,
    registerPath,
  ])

  return definePlugin({
    name: 'ruvyxa:pwa',
    setup({ addMiddleware, onBuildComplete }) {
      addMiddleware({
        routes: middlewareRoutes,
        onRequest(request) {
          const pathname = new URL(request.url).pathname
          if (pathname === manifestPath) {
            return new Response(manifestBody, {
              headers: { 'content-type': 'application/manifest+json; charset=utf-8' },
            })
          }
          if (pathname === serviceWorkerPath) {
            return new Response(serviceWorkerBody, {
              headers: {
                'cache-control': 'no-cache',
                'content-type': 'text/javascript; charset=utf-8',
                'service-worker-allowed': scope,
              },
            })
          }
          if (pathname === registerPath) {
            return new Response(registerBody, {
              headers: {
                'cache-control': 'no-cache',
                'content-type': 'text/javascript; charset=utf-8',
              },
            })
          }
          return undefined
        },
        async onResponse(request, response) {
          const pathname = new URL(request.url).pathname
          if (!htmlRoutes.some((route) => matchSource(route, pathname) !== null)) return undefined
          if (!response.headers.get('content-type')?.toLowerCase().includes('text/html')) {
            return undefined
          }
          const html = await response.text()
          const injected = injectPwaMarkup(html, manifestPath, registerPath)
          if (injected === html) return undefined
          const headers = new Headers(response.headers)
          headers.delete('content-length')
          return new Response(injected, {
            status: response.status,
            statusText: response.statusText,
            headers,
          })
        },
      })
      onBuildComplete((context) => {
        writePublicAsset(context, manifestPath, manifestBody)
        writePublicAsset(context, serviceWorkerPath, serviceWorkerBody)
        writePublicAsset(context, registerPath, registerBody)
        patchPrerenderedHtml(context, htmlRoutes, manifestPath, registerPath)
      })
    },
  })
}

function createPwaRegistration(serviceWorkerPath: string, scope: string): string {
  return `if ('serviceWorker' in navigator) {\n  addEventListener('load', () => {\n    navigator.serviceWorker.register(${JSON.stringify(serviceWorkerPath)}, { scope: ${JSON.stringify(scope)} })\n      .catch((error) => console.error('Ruvyxa service worker registration failed', error));\n  });\n}\n`
}

function createServiceWorker(
  cacheName: string,
  cachePrefix: string,
  precache: string[],
  offlineFallback: string | undefined,
): string {
  return `const CACHE = ${JSON.stringify(cacheName)};
const CACHE_PREFIX = ${JSON.stringify(cachePrefix)};
const PRECACHE = ${JSON.stringify(precache)};
const OFFLINE_FALLBACK = ${JSON.stringify(offlineFallback ?? null)};

self.addEventListener('install', (event) => {
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(PRECACHE)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (event) => {
  event.waitUntil(caches.keys().then((names) => Promise.all(
    names.filter((name) => name.startsWith(CACHE_PREFIX) && name !== CACHE).map((name) => caches.delete(name))
  )).then(() => self.clients.claim()));
});

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET' || new URL(request.url).origin !== self.location.origin) return;
  if (request.mode === 'navigate') {
    event.respondWith(fetch(request).catch(async () => {
      const fallback = OFFLINE_FALLBACK ? await caches.match(OFFLINE_FALLBACK) : undefined;
      return fallback || Response.error();
    }));
    return;
  }
  if (!['font', 'image', 'script', 'style'].includes(request.destination)) return;
  event.respondWith(caches.match(request).then((cached) => cached || fetch(request).then((response) => {
    if (response.ok) {
      const cacheWrite = caches.open(CACHE)
        .then((cache) => cache.put(request, response.clone()))
        .catch(() => undefined);
      event.waitUntil(cacheWrite);
    }
    return response;
  })));
});
`
}

function injectPwaMarkup(html: string, manifestPath: string, registerPath: string): string {
  if (html.includes('data-ruvyxa-pwa')) return html
  const manifestTag = `<link rel="manifest" href="${escapeHtmlAttribute(manifestPath)}" data-ruvyxa-pwa>`
  const registerTag = `<script type="module" src="${escapeHtmlAttribute(registerPath)}" data-ruvyxa-pwa></script>`
  let output = html.includes('</head>')
    ? html.replace('</head>', `${manifestTag}</head>`)
    : `${manifestTag}${html}`
  output = output.includes('</body>')
    ? output.replace('</body>', `${registerTag}</body>`)
    : `${output}${registerTag}`
  return output
}

function patchPrerenderedHtml(
  context: PluginBuildContext,
  routes: string[],
  manifestPath: string,
  registerPath: string,
): void {
  const prerenderDir = path.join(context.outDir, 'prerender')
  if (!existsSync(prerenderDir)) return
  for (const file of walkFiles(prerenderDir).filter((entry) => entry.endsWith('.html'))) {
    const relative = path.relative(prerenderDir, file).replaceAll('\\', '/')
    const routePath = relative === 'index.html' ? '/' : `/${relative.replace(/\/index\.html$/, '')}`
    if (!routes.some((route) => matchSource(route, routePath) !== null)) continue
    const html = readFileSync(file, 'utf8')
    const injected = injectPwaMarkup(html, manifestPath, registerPath)
    if (injected !== html) writeTextFileAtomic(file, injected)
  }
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
  /** Separate OpenAI search discovery from model-training access. */
  openAi?: {
    /** Controls OAI-SearchBot. */
    search?: boolean
    /** Controls GPTBot. */
    training?: boolean
  }
  /** Absolute sitemap URL appended as a `Sitemap:` line. */
  sitemap?: string
}

/** Generates `robots.txt` into the build's public asset directory. */
export function robots(options: RobotsOptions = {}): RuvyxaPlugin {
  const rules = options.rules?.length ? [...options.rules] : [{ userAgent: '*', allow: ['/'] }]
  for (const [field, userAgent] of [
    ['search', 'OAI-SearchBot'],
    ['training', 'GPTBot'],
  ] as const) {
    const access = options.openAi?.[field]
    if (access !== undefined && typeof access !== 'boolean') {
      throw new TypeError(`robots: openAi.${field} must be a boolean`)
    }
    if (access === undefined) continue
    if (rules.some((rule) => (rule.userAgent ?? '*').toLowerCase() === userAgent.toLowerCase())) {
      throw new TypeError(`robots: ${userAgent} is configured by both rules and openAi.${field}`)
    }
    rules.push({ userAgent, ...(access ? { allow: ['/'] } : { disallow: ['/'] }) })
  }

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

// ─── feed ────────────────────────────────────────────────────────────────────

export interface FeedItem {
  title: string
  /** Absolute URL or a path resolved against `siteUrl`. */
  url: string
  description?: string
  content?: string
  id?: string
  publishedAt?: string | Date
  author?: string
  categories?: string[]
}

export interface FeedOptions {
  siteUrl: string
  title: string
  description: string
  /** Static items or a build-time loader. */
  items: FeedItem[] | (() => FeedItem[] | Promise<FeedItem[]>)
  /** @default "/rss.xml" */
  path?: string
  language?: string
  copyright?: string
}

/** Generates a deterministic RSS 2.0 feed from explicit content metadata. */
export function feed(options: FeedOptions): RuvyxaPlugin {
  const siteUrl = normalizeSiteUrl(options?.siteUrl, 'feed')
  if (typeof options.title !== 'string' || options.title.trim() === '') {
    throw new TypeError('feed: title must be a non-empty string')
  }
  if (typeof options.description !== 'string' || options.description.trim() === '') {
    throw new TypeError('feed: description must be a non-empty string')
  }
  if (!Array.isArray(options.items) && typeof options.items !== 'function') {
    throw new TypeError('feed: items must be an array or build-time loader')
  }
  const outputPath = normalizePublicFilePath(options.path ?? '/rss.xml', 'feed')

  return definePlugin({
    name: 'ruvyxa:feed',
    setup({ onBuildComplete }) {
      onBuildComplete(async (context) => {
        const items =
          typeof options.items === 'function' ? await options.items() : [...options.items]
        if (!Array.isArray(items)) throw new TypeError('feed: item loader must return an array')
        const body = createRssFeed(options, siteUrl, items)
        writePublicAsset(context, outputPath, body)
      })
    },
  })
}

function createRssFeed(options: FeedOptions, siteUrl: string, items: FeedItem[]): string {
  const entries = items.map((item, index) => {
    if (!item || typeof item.title !== 'string' || item.title.trim() === '') {
      throw new TypeError(`feed: items[${index}].title must be a non-empty string`)
    }
    if (typeof item.url !== 'string' || item.url.trim() === '') {
      throw new TypeError(`feed: items[${index}].url must be a non-empty string`)
    }
    const url = normalizeItemUrl(item.url, siteUrl, `feed.items[${index}].url`)
    const id = item.id ?? url
    const lines = [
      '    <item>',
      `      <title>${escapeXml(item.title)}</title>`,
      `      <link>${escapeXml(url)}</link>`,
      `      <guid isPermaLink="${item.id ? 'false' : 'true'}">${escapeXml(id)}</guid>`,
    ]
    if (item.description)
      lines.push(`      <description>${escapeXml(item.description)}</description>`)
    if (item.content) {
      lines.push(
        `      <content:encoded><![CDATA[${item.content.replaceAll(']]>', ']]]]><![CDATA[>')}]]></content:encoded>`,
      )
    }
    if (item.publishedAt) {
      lines.push(
        `      <pubDate>${normalizeDate(item.publishedAt, `feed.items[${index}]`)}</pubDate>`,
      )
    }
    if (item.author) lines.push(`      <author>${escapeXml(item.author)}</author>`)
    for (const category of item.categories ?? []) {
      lines.push(`      <category>${escapeXml(category)}</category>`)
    }
    lines.push('    </item>')
    return lines.join('\n')
  })
  return `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title>${escapeXml(options.title)}</title>
    <link>${escapeXml(siteUrl)}</link>
    <description>${escapeXml(options.description)}</description>
${options.language ? `    <language>${escapeXml(options.language)}</language>\n` : ''}${options.copyright ? `    <copyright>${escapeXml(options.copyright)}</copyright>\n` : ''}${entries.join('\n')}
  </channel>
</rss>
`
}

// ─── searchIndex ─────────────────────────────────────────────────────────────

export interface SearchDocument {
  id: string
  title: string
  url: string
  text: string
  tags?: string[]
}

export interface SearchIndexOptions {
  /** Static documents or a build-time loader. */
  documents: SearchDocument[] | (() => SearchDocument[] | Promise<SearchDocument[]>)
  /** @default "/search-index.json" */
  path?: string
  /** BCP 47 locale used for word segmentation, including languages such as Thai. */
  locale?: string
  stopWords?: string[]
  /** Ignore shorter terms. @default 2 */
  minTermLength?: number
}

/** Generates a compact static inverted index with locale-aware tokenization. */
export function searchIndex(options: SearchIndexOptions): RuvyxaPlugin {
  if (!options || (!Array.isArray(options.documents) && typeof options.documents !== 'function')) {
    throw new TypeError('searchIndex: documents must be an array or build-time loader')
  }
  const outputPath = normalizePublicFilePath(options.path ?? '/search-index.json', 'searchIndex')
  const minTermLength = options.minTermLength ?? 2
  if (!Number.isInteger(minTermLength) || minTermLength < 1 || minTermLength > 64) {
    throw new TypeError('searchIndex: minTermLength must be an integer from 1 to 64')
  }
  const stopWords = new Set(
    (options.stopWords ?? []).map((word) => word.toLocaleLowerCase(options.locale)),
  )

  return definePlugin({
    name: 'ruvyxa:search-index',
    setup({ onBuildComplete }) {
      onBuildComplete(async (context) => {
        const input =
          typeof options.documents === 'function'
            ? await options.documents()
            : [...options.documents]
        if (!Array.isArray(input)) {
          throw new TypeError('searchIndex: document loader must return an array')
        }
        writePublicAsset(
          context,
          outputPath,
          createSearchIndexBody(input, options.locale, stopWords, minTermLength),
        )
      })
    },
  })
}

function createSearchIndexBody(
  input: SearchDocument[],
  locale: string | undefined,
  stopWords: ReadonlySet<string>,
  minTermLength: number,
): string {
  const documents = normalizeSearchDocuments(input)
  const postings = new Map<string, Set<string>>()
  for (const document of documents) {
    const content = [document.title, document.text, ...(document.tags ?? [])].join(' ')
    for (const term of segmentWords(content, locale)) {
      const normalized = term.toLocaleLowerCase(locale)
      if (normalized.length < minTermLength || stopWords.has(normalized)) continue
      const ids = postings.get(normalized) ?? new Set<string>()
      ids.add(document.id)
      postings.set(normalized, ids)
    }
  }
  const terms = Object.fromEntries(
    [...postings.entries()]
      .sort(([left], [right]) => compareStable(left, right))
      .map(([term, ids]) => [term, [...ids].sort(compareStable)]),
  )
  return `${JSON.stringify({ version: 1, documents, terms })}\n`
}

function normalizeSearchDocuments(documents: SearchDocument[]): SearchDocument[] {
  const ids = new Set<string>()
  return documents
    .map((document, index) => {
      for (const field of ['id', 'title', 'url', 'text'] as const) {
        if (typeof document?.[field] !== 'string' || document[field].trim() === '') {
          throw new TypeError(
            `searchIndex: documents[${index}].${field} must be a non-empty string`,
          )
        }
      }
      if (ids.has(document.id)) throw new TypeError(`searchIndex: duplicate id ${document.id}`)
      if (
        document.tags !== undefined &&
        (!Array.isArray(document.tags) || document.tags.some((tag) => typeof tag !== 'string'))
      ) {
        throw new TypeError(`searchIndex: documents[${index}].tags must be an array of strings`)
      }
      ids.add(document.id)
      return { ...document, tags: document.tags ? [...document.tags] : undefined }
    })
    .sort((left, right) => compareStable(left.id, right.id))
}

function segmentWords(value: string, locale: string | undefined): string[] {
  const Segmenter = Intl.Segmenter
  if (Segmenter) {
    return [...new Segmenter(locale, { granularity: 'word' }).segment(value)]
      .filter((part) => part.isWordLike)
      .map((part) => part.segment)
  }
  return value.match(/[\p{L}\p{N}]+/gu) ?? []
}

// ─── contentEngine ────────────────────────────────────────────────────────────

export interface ContentEngineAnswerSource {
  name: string
  url: string
}

export interface ContentEngineAnswer {
  question: string
  answer: string
  sources?: ContentEngineAnswerSource[]
}

export interface ContentEngineEntry {
  id: string
  route: string
  url: string
  title: string
  description: string
  tags: string[]
  readingTimeMinutes: number
  /** Explicit, author-written answers suitable for visible answer blocks. */
  answers: ContentEngineAnswer[]
  publishedAt?: string
  updatedAt?: string
  author?: string
  /** Original JSON-compatible frontmatter for application-specific fields. */
  frontmatter: Readonly<Record<string, unknown>>
}

export interface ContentEngineOptions {
  siteUrl: string
  title: string
  description: string
  /** Directory containing file-system routes, relative to the project root. @default "app" */
  appDir?: string
  /** Exact route paths or trailing-`*` patterns omitted from every artifact. */
  exclude?: string[]
  /** BCP 47 locale used for search tokenization and reading-time estimates. */
  locale?: string
  stopWords?: string[]
  /** Ignore shorter search terms. @default 2 */
  minTermLength?: number
  /** @default "/content.json" */
  manifestPath?: string
  /** @default "/search-index.json" */
  searchPath?: string
  /** @default "/rss.xml" */
  feedPath?: string
  /** @default "/sitemap.xml" */
  sitemapPath?: string
  /** Experimental agent discovery index. Set false to disable. @default "/llms.txt" */
  llmsPath?: string | false
  language?: string
}

interface ContentEngineDocument extends ContentEngineEntry {
  text: string
}

interface ContentArtifact {
  body: string
  contentType: string
}

interface NormalizedContentEngineOptions {
  siteUrl: string
  title: string
  description: string
  appDir: string
  exclude: string[]
  locale: string | undefined
  stopWords: ReadonlySet<string>
  minTermLength: number
  manifestPath: string
  searchPath: string
  feedPath: string
  sitemapPath: string
  llmsPath: string | undefined
  language: string | undefined
}

/**
 * Turns native Markdown/MDX routes into one content graph and derives a live
 * content API, search index, RSS feed, and sitemap without duplicate metadata.
 */
export function contentEngine(options: ContentEngineOptions): RuvyxaPlugin {
  const normalized = normalizeContentEngineOptions(options)
  const outputPaths = [
    normalized.manifestPath,
    normalized.searchPath,
    normalized.feedPath,
    normalized.sitemapPath,
    ...(normalized.llmsPath ? [normalized.llmsPath] : []),
  ]
  let developmentCache:
    { root: string; fingerprint: string; artifacts: Map<string, ContentArtifact> } | undefined

  const developmentArtifacts = (root: string): Map<string, ContentArtifact> => {
    const appRoot = path.resolve(root, normalized.appDir)
    const files = contentPageFiles(appRoot)
    const fingerprint = contentFilesFingerprint(files)
    if (developmentCache?.root === root && developmentCache.fingerprint === fingerprint) {
      return developmentCache.artifacts
    }
    const artifacts = createContentEngineArtifacts(root, normalized, files)
    developmentCache = { root, fingerprint, artifacts }
    return artifacts
  }

  return definePlugin({
    name: 'ruvyxa:content-engine',
    setup({ addMiddleware, onBuildComplete }) {
      addMiddleware({
        routes: outputPaths,
        onRequest(request, context) {
          if (request.method !== 'GET' && request.method !== 'HEAD') return undefined
          const appRoot = path.resolve(context.root, normalized.appDir)
          if (!isDirectory(appRoot)) return undefined
          const artifact = developmentArtifacts(context.root).get(new URL(request.url).pathname)
          if (!artifact) return undefined
          return new Response(request.method === 'HEAD' ? null : artifact.body, {
            headers: {
              'cache-control': 'no-cache',
              'content-type': artifact.contentType,
            },
          })
        },
      })
      onBuildComplete((context) => {
        for (const [outputPath, artifact] of createContentEngineArtifacts(
          context.root,
          normalized,
        )) {
          writePublicAsset(context, outputPath, artifact.body)
        }
      })
    },
  })
}

function normalizeContentEngineOptions(
  options: ContentEngineOptions,
): NormalizedContentEngineOptions {
  const siteUrl = normalizeSiteUrl(options?.siteUrl, 'contentEngine')
  for (const field of ['title', 'description'] as const) {
    if (typeof options[field] !== 'string' || options[field].trim() === '') {
      throw new TypeError(`contentEngine: ${field} must be a non-empty string`)
    }
  }
  const appDir = options.appDir ?? 'app'
  if (
    typeof appDir !== 'string' ||
    appDir.trim() === '' ||
    path.isAbsolute(appDir) ||
    appDir.split(/[\\/]+/).some((segment) => segment === '..')
  ) {
    throw new TypeError('contentEngine: appDir must stay inside the project root')
  }
  const exclude = normalizeRoutes(options.exclude, 'contentEngine') ?? []
  if (options.locale !== undefined) {
    try {
      new Intl.Segmenter(options.locale)
    } catch {
      throw new TypeError('contentEngine: locale must be a valid BCP 47 locale')
    }
  }
  if (
    options.stopWords !== undefined &&
    (!Array.isArray(options.stopWords) ||
      options.stopWords.some((word) => typeof word !== 'string'))
  ) {
    throw new TypeError('contentEngine: stopWords must be an array of strings')
  }
  const minTermLength = options.minTermLength ?? 2
  if (!Number.isInteger(minTermLength) || minTermLength < 1 || minTermLength > 64) {
    throw new TypeError('contentEngine: minTermLength must be an integer from 1 to 64')
  }
  const manifestPath = normalizePublicFilePath(
    options.manifestPath ?? '/content.json',
    'contentEngine',
  )
  const searchPath = normalizePublicFilePath(
    options.searchPath ?? '/search-index.json',
    'contentEngine',
  )
  const feedPath = normalizePublicFilePath(options.feedPath ?? '/rss.xml', 'contentEngine')
  const sitemapPath = normalizePublicFilePath(
    options.sitemapPath ?? '/sitemap.xml',
    'contentEngine',
  )
  const llmsPath =
    options.llmsPath === false
      ? undefined
      : normalizePublicFilePath(options.llmsPath ?? '/llms.txt', 'contentEngine')
  const artifactPaths = [manifestPath, searchPath, feedPath, sitemapPath, llmsPath].filter(
    (value): value is string => value !== undefined,
  )
  if (new Set(artifactPaths).size !== artifactPaths.length) {
    throw new TypeError('contentEngine: generated artifact paths must be distinct')
  }

  return {
    siteUrl,
    title: options.title,
    description: options.description,
    appDir,
    exclude,
    locale: options.locale,
    stopWords: new Set(
      (options.stopWords ?? []).map((word) => word.toLocaleLowerCase(options.locale)),
    ),
    minTermLength,
    manifestPath,
    searchPath,
    feedPath,
    sitemapPath,
    llmsPath,
    language: options.language,
  }
}

function createContentEngineArtifacts(
  root: string,
  options: NormalizedContentEngineOptions,
  files?: string[],
): Map<string, ContentArtifact> {
  const appRoot = path.resolve(root, options.appDir)
  if (!isDirectory(appRoot)) {
    throw new TypeError(`contentEngine: app directory was not found at ${appRoot}`)
  }
  const documents = discoverContentEngineDocuments(appRoot, options, files)
  const entries = documents.map(({ text: _text, ...entry }) => entry)
  const manifestBody = `${JSON.stringify({ version: 1, entries }, null, 2)}\n`
  const searchBody = createSearchIndexBody(
    documents.map((document) => ({
      id: document.id,
      title: document.title,
      url: document.route,
      text: document.text,
      tags: document.tags,
    })),
    options.locale,
    options.stopWords,
    options.minTermLength,
  )
  const feedItems = documents.map((document) => ({
    title: document.title,
    url: document.route,
    description: document.description,
    publishedAt: document.publishedAt,
    author: document.author,
    categories: document.tags,
  }))
  const feedBody = createRssFeed(
    {
      siteUrl: options.siteUrl,
      title: options.title,
      description: options.description,
      language: options.language,
      items: feedItems,
    },
    options.siteUrl,
    feedItems,
  )
  const sitemapEntries = documents
    .map((document) => {
      const lastModified = document.updatedAt ?? document.publishedAt
      return `  <url><loc>${escapeXml(document.url)}</loc>${lastModified ? `<lastmod>${lastModified}</lastmod>` : ''}</url>`
    })
    .join('\n')
  const sitemapBody = `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${sitemapEntries}\n</urlset>\n`
  const artifacts = new Map<string, ContentArtifact>([
    [options.manifestPath, { body: manifestBody, contentType: 'application/json; charset=utf-8' }],
    [options.searchPath, { body: searchBody, contentType: 'application/json; charset=utf-8' }],
    [options.feedPath, { body: feedBody, contentType: 'application/rss+xml; charset=utf-8' }],
    [options.sitemapPath, { body: sitemapBody, contentType: 'application/xml; charset=utf-8' }],
  ])
  if (options.llmsPath) {
    artifacts.set(options.llmsPath, {
      body: createLlmsText(options, documents),
      contentType: 'text/plain; charset=utf-8',
    })
  }
  return artifacts
}

function createLlmsText(
  options: NormalizedContentEngineOptions,
  documents: ContentEngineDocument[],
): string {
  const lines = [
    `# ${escapeMarkdownText(options.title)}`,
    '',
    `> ${options.description.replace(/\s+/g, ' ').trim()}`,
    '',
    '## Content',
    '',
  ]
  for (const document of documents) {
    lines.push(
      `- [${escapeMarkdownText(document.title)}](<${document.url}>): ${document.description}`,
    )
    for (const answer of document.answers) {
      lines.push(
        `  - ${escapeMarkdownText(answer.question)} — ${escapeMarkdownText(answer.answer)}`,
      )
    }
  }
  return `${lines.join('\n')}\n`
}

function escapeMarkdownText(value: string): string {
  return value
    .replaceAll('\\', '\\\\')
    .replace(/([\[\]])/g, '\\$1')
    .replace(/\s+/g, ' ')
    .trim()
}

function discoverContentEngineDocuments(
  appRoot: string,
  options: NormalizedContentEngineOptions,
  files = contentPageFiles(appRoot),
): ContentEngineDocument[] {
  const routes = new Set<string>()
  const documents: ContentEngineDocument[] = []
  for (const file of files) {
    const route = contentRouteFromFile(appRoot, file)
    if (
      route.includes('[') ||
      options.exclude.some((pattern) => matchSource(pattern, route) !== null)
    ) {
      continue
    }
    if (routes.has(route)) {
      throw new TypeError(`contentEngine: multiple Markdown/MDX pages resolve to ${route}`)
    }
    routes.add(route)
    const source = readFileSync(file, 'utf8')
    const { frontmatter, body } = parseContentEngineSource(source, file)
    if (frontmatter.draft === true) continue
    if (frontmatter.draft !== undefined && typeof frontmatter.draft !== 'boolean') {
      throw new TypeError(`contentEngine: ${file} frontmatter.draft must be a boolean`)
    }
    const text = markdownToPlainText(body)
    const title = contentString(frontmatter.title, 'title', file) ?? firstMarkdownHeading(body)
    const resolvedTitle = title || contentTitleFromRoute(route, options.title)
    const descriptionValue =
      contentString(frontmatter.description, 'description', file) ??
      contentString(frontmatter.summary, 'summary', file) ??
      text
    const tags = contentTags(frontmatter.tags, file)
    const answers = contentAnswers(frontmatter.answers, file, options.siteUrl)
    const publishedAt = contentDate(
      frontmatter.publishedAt ?? frontmatter.date,
      'publishedAt',
      file,
    )
    const updatedAt = contentDate(frontmatter.updatedAt, 'updatedAt', file)
    const author = contentString(frontmatter.author, 'author', file)
    const searchableText = text || resolvedTitle
    const resolvedDescription = descriptionValue || resolvedTitle
    const wordCount = segmentWords(searchableText, options.locale).length
    documents.push({
      id: route,
      route,
      url: options.siteUrl + route,
      title: resolvedTitle,
      description: truncateContentText(resolvedDescription, 160),
      tags,
      readingTimeMinutes: Math.max(1, Math.ceil(wordCount / 200)),
      answers,
      ...(publishedAt ? { publishedAt } : {}),
      ...(updatedAt ? { updatedAt } : {}),
      ...(author ? { author } : {}),
      frontmatter,
      text: searchableText,
    })
  }
  return documents.sort((left, right) => {
    const byDate = compareStable(right.publishedAt ?? '', left.publishedAt ?? '')
    return byDate || compareStable(left.route, right.route)
  })
}

function contentPageFiles(root: string): string[] {
  const files: string[] = []
  const visit = (directory: string): void => {
    const entries = readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
      compareStable(left.name, right.name),
    )
    for (const entry of entries) {
      if (entry.isDirectory()) {
        if (entry.name.startsWith('_') || entry.name.startsWith('@')) continue
        visit(path.join(directory, entry.name))
      } else if (entry.isFile() && (entry.name === 'page.md' || entry.name === 'page.mdx')) {
        files.push(path.join(directory, entry.name))
      }
    }
  }
  visit(root)
  return files
}

function contentFilesFingerprint(files: string[]): string {
  const fingerprint = createHash('sha256')
  for (const file of files) {
    const metadata = statSync(file)
    fingerprint.update(file)
    fingerprint.update('\0')
    fingerprint.update(String(metadata.size))
    fingerprint.update('\0')
    fingerprint.update(String(metadata.mtimeMs))
    fingerprint.update('\0')
  }
  return fingerprint.digest('hex')
}

function contentRouteFromFile(appRoot: string, file: string): string {
  const relativeDirectory = path.relative(appRoot, path.dirname(file))
  const segments = relativeDirectory
    .split(path.sep)
    .filter(Boolean)
    .filter((segment) => !(segment.startsWith('(') && segment.endsWith(')')))
  return segments.length === 0 ? '/' : `/${segments.join('/')}`
}

function parseContentEngineSource(
  source: string,
  file: string,
): { frontmatter: Record<string, unknown>; body: string } {
  const normalized = source
    .replace(/^\uFEFF/, '')
    .replaceAll('\r\n', '\n')
    .replaceAll('\r', '\n')
  if (!normalized.startsWith('---\n')) return { frontmatter: {}, body: normalized }
  const lines = normalized.split('\n')
  const closing = lines.findIndex((line, index) => index > 0 && /^(---|\.\.\.)\s*$/.test(line))
  if (closing === -1) {
    throw new TypeError(`contentEngine: ${file} frontmatter has no closing delimiter`)
  }
  const frontmatterSource = `${lines.slice(1, closing).join('\n')}\n`
  const body = lines.slice(closing + 1).join('\n')
  if (frontmatterSource.trim() === '') return { frontmatter: {}, body }
  const document = parseDocument(frontmatterSource, { schema: 'core' })
  if (document.errors.length > 0) {
    throw new TypeError(`contentEngine: ${file} invalid YAML: ${document.errors[0].message}`)
  }
  let value: unknown
  try {
    assertContentEngineYamlKeys(document.contents)
    value = document.toJS({ maxAliasCount: 100 }) as unknown
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new TypeError(`contentEngine: ${file} frontmatter must be JSON-compatible: ${detail}`)
  }
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`contentEngine: ${file} frontmatter must be a YAML mapping`)
  }
  let frontmatter: Record<string, unknown>
  try {
    assertContentEngineJsonValue(value, new WeakSet())
    frontmatter = JSON.parse(JSON.stringify(value)) as Record<string, unknown>
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error)
    throw new TypeError(`contentEngine: ${file} frontmatter must be JSON-compatible: ${detail}`)
  }
  return { frontmatter, body }
}

function assertContentEngineYamlKeys(node: unknown): void {
  if (isMap(node)) {
    for (const pair of node.items) {
      if (!isScalar(pair.key) || typeof pair.key.value !== 'string') {
        throw new TypeError('YAML mapping keys must be strings')
      }
      assertContentEngineYamlKeys(pair.value)
    }
  } else if (isSeq(node)) {
    for (const child of node.items) assertContentEngineYamlKeys(child)
  }
}

function assertContentEngineJsonValue(value: unknown, ancestors: WeakSet<object>): void {
  if (typeof value === 'number' && !Number.isFinite(value)) {
    throw new TypeError('non-finite numbers are not supported')
  }
  if (value === null || typeof value !== 'object') return
  if (ancestors.has(value)) throw new TypeError('cyclic YAML aliases are not supported')
  ancestors.add(value)
  for (const child of Array.isArray(value) ? value : Object.values(value)) {
    assertContentEngineJsonValue(child, ancestors)
  }
  ancestors.delete(value)
}

function markdownToPlainText(body: string): string {
  const visible: string[] = []
  let fence: string | undefined
  let esmBlock = false
  for (const line of body.split('\n')) {
    const fenceMatch = line.match(/^\s*(```+|~~~+)/)
    if (fenceMatch) {
      if (!fence) fence = fenceMatch[1][0]
      else if (fence === fenceMatch[1][0]) fence = undefined
      continue
    }
    if (fence) continue
    if (/^\s*(?:import|export)\b/.test(line)) {
      esmBlock = true
      continue
    }
    if (esmBlock) {
      if (line.trim() === '') esmBlock = false
      continue
    }
    visible.push(markdownInlineText(line))
  }
  return visible.join(' ').replace(/\s+/g, ' ').trim()
}

function markdownInlineText(value: string): string {
  let output = value
    .replace(/<!--[^]*?-->/g, ' ')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/<[^>]+>/g, ' ')
  for (let iteration = 0; iteration < 4 && /\{[^{}]*\}/.test(output); iteration += 1) {
    output = output.replace(/\{[^{}]*\}/g, ' ')
  }
  return output
    .replace(/^\s{0,3}(?:#{1,6}|>|[-+*]\s|\d+[.)]\s)\s*/g, '')
    .replace(/[`*_~]/g, '')
    .replace(/\s+/g, ' ')
    .trim()
}

function firstMarkdownHeading(body: string): string | undefined {
  for (const line of body.split('\n')) {
    const match = line.match(/^\s{0,3}#\s+(.+?)\s*#*\s*$/)
    if (match) {
      const heading = markdownInlineText(match[1])
      if (heading) return heading
    }
  }
  return undefined
}

function contentString(value: unknown, field: string, file: string): string | undefined {
  if (value === undefined || value === null) return undefined
  if (typeof value !== 'string' || value.trim() === '') {
    throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be a non-empty string`)
  }
  return value.trim()
}

function contentTags(value: unknown, file: string): string[] {
  if (value === undefined || value === null) return []
  if (!Array.isArray(value) || value.some((tag) => typeof tag !== 'string' || tag.trim() === '')) {
    throw new TypeError(`contentEngine: ${file} frontmatter.tags must be an array of strings`)
  }
  return uniqueStrings(value.map((tag) => tag.trim())).sort(compareStable)
}

function contentAnswers(value: unknown, file: string, siteUrl: string): ContentEngineAnswer[] {
  if (value === undefined || value === null) return []
  if (!Array.isArray(value)) {
    throw new TypeError(`contentEngine: ${file} frontmatter.answers must be an array`)
  }
  return value.map((entry, answerIndex) => {
    const field = `answers[${answerIndex}]`
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) {
      throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be an object`)
    }
    const record = entry as Record<string, unknown>
    const question = contentString(record.question, `${field}.question`, file)
    const answer = contentString(record.answer, `${field}.answer`, file)
    if (!question) {
      throw new TypeError(
        `contentEngine: ${file} frontmatter.${field}.question must be a non-empty string`,
      )
    }
    if (!answer) {
      throw new TypeError(
        `contentEngine: ${file} frontmatter.${field}.answer must be a non-empty string`,
      )
    }
    if (record.sources === undefined || record.sources === null) return { question, answer }
    if (!Array.isArray(record.sources)) {
      throw new TypeError(`contentEngine: ${file} frontmatter.${field}.sources must be an array`)
    }
    const sources = record.sources.map((source, sourceIndex) => {
      const sourceField = `${field}.sources[${sourceIndex}]`
      if (source === null || typeof source !== 'object' || Array.isArray(source)) {
        throw new TypeError(`contentEngine: ${file} frontmatter.${sourceField} must be an object`)
      }
      const sourceRecord = source as Record<string, unknown>
      const name = contentString(sourceRecord.name, `${sourceField}.name`, file)
      const url = contentString(sourceRecord.url, `${sourceField}.url`, file)
      if (!name) {
        throw new TypeError(
          `contentEngine: ${file} frontmatter.${sourceField}.name must be a non-empty string`,
        )
      }
      if (!url) {
        throw new TypeError(
          `contentEngine: ${file} frontmatter.${sourceField}.url must be a non-empty string`,
        )
      }
      return {
        name,
        url: normalizeItemUrl(
          url,
          siteUrl,
          `contentEngine: ${file} frontmatter.${sourceField}.url`,
        ),
      }
    })
    return { question, answer, sources }
  })
}

function contentDate(value: unknown, field: string, file: string): string | undefined {
  if (value === undefined || value === null) return undefined
  if (
    typeof value !== 'string' ||
    !/^\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}(?::\d{2}(?:\.\d{1,9})?)?(?:Z|[+-]\d{2}:\d{2}))?$/.test(
      value,
    )
  ) {
    throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be an ISO date string`)
  }
  const [year, month, day] = value.slice(0, 10).split('-').map(Number)
  const daysInMonth = [
    31,
    year % 400 === 0 || (year % 4 === 0 && year % 100 !== 0) ? 29 : 28,
    31,
    30,
    31,
    30,
    31,
    31,
    30,
    31,
    30,
    31,
  ]
  if (month < 1 || month > 12 || day < 1 || day > daysInMonth[month - 1]) {
    throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be an ISO date string`)
  }
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) {
    throw new TypeError(`contentEngine: ${file} frontmatter.${field} must be an ISO date string`)
  }
  return date.toISOString()
}

function contentTitleFromRoute(route: string, siteTitle: string): string {
  if (route === '/') return siteTitle
  const segment = route.split('/').at(-1) ?? siteTitle
  const title = segment.replace(/[-_]+/g, ' ').trim()
  return title ? title[0].toLocaleUpperCase() + title.slice(1) : siteTitle
}

function truncateContentText(value: string, maximum: number): string {
  const normalized = value.replace(/\s+/g, ' ').trim()
  const characters = [...normalized]
  if (characters.length <= maximum) return normalized
  return `${characters
    .slice(0, maximum - 1)
    .join('')
    .trimEnd()}…`
}

// ─── openApi ─────────────────────────────────────────────────────────────────

export type OpenApiMethod =
  'delete' | 'get' | 'head' | 'options' | 'patch' | 'post' | 'put' | 'trace'

export interface OpenApiOperation {
  method: OpenApiMethod | Uppercase<OpenApiMethod>
  path: string
  summary?: string
  description?: string
  operationId?: string
  tags?: string[]
  parameters?: unknown[]
  requestBody?: Record<string, unknown>
  responses?: Record<string, unknown>
  security?: Array<Record<string, string[]>>
}

export interface OpenApiOptions {
  info: { title: string; version: string; description?: string }
  operations: OpenApiOperation[]
  servers?: Array<{ url: string; description?: string }>
  tags?: Array<{ name: string; description?: string }>
  components?: Record<string, unknown>
  /** @default "/openapi.json" */
  path?: string
}

/** Builds and serves an OpenAPI 3.1 document from explicit API operation metadata. */
export function openApi(options: OpenApiOptions): RuvyxaPlugin {
  if (
    !options?.info ||
    typeof options.info.title !== 'string' ||
    options.info.title.trim() === '' ||
    typeof options.info.version !== 'string' ||
    options.info.version.trim() === ''
  ) {
    throw new TypeError('openApi: info.title and info.version must be non-empty strings')
  }
  if (!Array.isArray(options.operations)) {
    throw new TypeError('openApi: operations must be an array')
  }
  const outputPath = normalizePublicFilePath(options.path ?? '/openapi.json', 'openApi')
  const paths: Record<string, Record<string, unknown>> = {}
  const operationIds = new Set<string>()
  for (const [index, operation] of options.operations.entries()) {
    if (!operation || typeof operation.path !== 'string' || !operation.path.startsWith('/')) {
      throw new TypeError(`openApi: operations[${index}].path must start with "/"`)
    }
    const method = String(operation.method).toLowerCase()
    if (!['delete', 'get', 'head', 'options', 'patch', 'post', 'put', 'trace'].includes(method)) {
      throw new TypeError(`openApi: operations[${index}].method is unsupported`)
    }
    if (paths[operation.path]?.[method]) {
      throw new TypeError(`openApi: duplicate ${method.toUpperCase()} ${operation.path}`)
    }
    if (operation.operationId) {
      if (operationIds.has(operation.operationId)) {
        throw new TypeError(`openApi: duplicate operationId ${operation.operationId}`)
      }
      operationIds.add(operation.operationId)
    }
    paths[operation.path] ??= {}
    paths[operation.path][method] = {
      ...(operation.summary ? { summary: operation.summary } : {}),
      ...(operation.description ? { description: operation.description } : {}),
      ...(operation.operationId ? { operationId: operation.operationId } : {}),
      ...(operation.tags ? { tags: operation.tags } : {}),
      ...(operation.parameters ? { parameters: operation.parameters } : {}),
      ...(operation.requestBody ? { requestBody: operation.requestBody } : {}),
      ...(operation.security ? { security: operation.security } : {}),
      responses: operation.responses ?? { '200': { description: 'Successful response' } },
    }
  }
  const document = {
    openapi: '3.1.0',
    info: options.info,
    ...(options.servers ? { servers: options.servers } : {}),
    ...(options.tags ? { tags: options.tags } : {}),
    paths,
    ...(options.components ? { components: options.components } : {}),
  }
  const body = `${JSON.stringify(document, null, 2)}\n`

  return definePlugin({
    name: 'ruvyxa:openapi',
    setup({ addMiddleware, onBuildComplete }) {
      addMiddleware({
        routes: [outputPath],
        onRequest(request) {
          if (new URL(request.url).pathname !== outputPath) return undefined
          return new Response(body, {
            headers: { 'content-type': 'application/json; charset=utf-8' },
          })
        },
      })
      onBuildComplete((context) => writePublicAsset(context, outputPath, body))
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
  return parsed.href.replace(/\/+$/, '')
}

function normalizeRoutes(routes: string[] | undefined, plugin: string): string[] | undefined {
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

function normalizeHeaderName(value: string, field: string): string {
  try {
    const probe = new Headers()
    probe.set(value, 'value')
    return value.toLowerCase()
  } catch {
    throw new TypeError(`${field} must be a valid HTTP header name`)
  }
}

function cloneResponse(response: Response, headers: Headers): Response {
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  })
}

function appendHeaderValue(headers: Headers, name: string, value: string): void {
  const existing = headers.get(name)
  headers.set(name, existing ? `${existing}, ${value}` : value)
}

function mergeVary(headers: Headers, values: string[]): void {
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

function normalizePublicPath(value: unknown, plugin: string): string {
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
    /[\u0000-\u001f\u007f]/.test(decoded) ||
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

function normalizePublicFilePath(value: unknown, plugin: string): string {
  const normalized = normalizePublicPath(value, plugin)
  if (normalized === '/' || normalized.endsWith('/')) {
    throw new TypeError(`${plugin}: public asset path must identify a file`)
  }
  return normalized
}

function compareStable(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0
}

function normalizeItemUrl(value: string, siteUrl: string, field: string): string {
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

function normalizeDate(value: string | Date, field: string): string {
  const date = value instanceof Date ? value : new Date(value)
  if (Number.isNaN(date.getTime())) throw new TypeError(`${field}.publishedAt must be a valid date`)
  return date.toUTCString()
}

function uniqueStrings(values: string[]): string[] {
  return [...new Set(values)]
}

function walkFiles(root: string): string[] {
  const files: string[] = []
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const resolved = path.join(root, entry.name)
    if (entry.isDirectory()) files.push(...walkFiles(resolved))
    else if (entry.isFile()) files.push(resolved)
  }
  return files
}

function isDirectory(value: string): boolean {
  try {
    return statSync(value).isDirectory()
  } catch {
    return false
  }
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
  const normalized = normalizePublicFilePath(
    fileName.startsWith('/') ? fileName : `/${fileName}`,
    'built-in plugin',
  ).slice(1)
  const assetsDir = path.join(context.outDir, 'assets')
  const destination = path.join(assetsDir, ...normalized.split('/'))
  mkdirSync(path.dirname(destination), { recursive: true })
  writeTextFileAtomic(destination, contents)
}

function writeTextFileAtomic(destination: string, contents: string): void {
  const temporary = `${destination}.tmp-${process.pid}-${randomUUID()}`
  try {
    writeFileSync(temporary, contents, 'utf8')
    renameSync(temporary, destination)
  } finally {
    rmSync(temporary, { force: true })
  }
}

function escapeXml(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll("'", '&apos;')
    .replaceAll('"', '&quot;')
}

function escapeHtmlAttribute(value: string): string {
  return value.replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;')
}
