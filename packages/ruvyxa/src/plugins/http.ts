import { createHash, randomBytes, randomUUID } from 'node:crypto'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { definePlugin } from '@ruvyxa/core/plugin'
import type { PluginBuildContext, RuvyxaPlugin } from '@ruvyxa/core/plugin'
import { originIsCrossSite } from '@ruvyxa/core/origin-policy'

import {
  appendHeaderValue,
  cloneResponse,
  compareStable,
  isDirectory,
  mergeVary,
  normalizeHeaderName,
  normalizePublicPath,
  normalizeRoutes,
  validateAbsoluteHttpUrl,
  validateRoutePattern,
  walkFiles,
  writeFileAtomic,
} from './shared.js'

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
    register({ http }) {
      http.onRequest({
        match: normalized.map((rule) => rule.source),
        handler({ request }) {
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
export function matchSource(source: string, pathname: string): string | null {
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
    register({ http }) {
      http.onResponse({
        ...(scoped ? { match: normalized.map((rule) => rule.source as string) } : {}),
        handler({ request, response }) {
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
    register({ http }) {
      http.onRequest({
        ...(routes ? { match: routes } : {}),
        handler({ request }) {
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
      })
      http.onResponse({
        ...(routes ? { match: routes } : {}),
        handler({ request, response }) {
          const headers = new Headers(response.headers)
          const requestId = request.headers.get(requestIdHeader) ?? randomUUID()
          const traceparent = traceContext
            ? (request.headers.get('traceparent') ?? createTraceparent())
            : (request.headers.get('traceparent') ?? '')
          // `Number(null)` is `0`, and `0` is finite — so an absent start
          // header used to pass the guard and report `Date.now()` itself as the
          // duration: every such response carried a `Server-Timing` of roughly
          // fifty-six years, and any dashboard averaging it was ruined by one
          // sample. The header goes missing whenever the response hook runs
          // without the request hook, which is what happens as soon as an
          // earlier plugin short-circuits the request with its own `Response`.
          const startedAtHeader = request.headers.get(OBSERVABILITY_START_HEADER)
          const startedAt = startedAtHeader === null ? Number.NaN : Number(startedAtHeader)
          const durationMs =
            Number.isFinite(startedAt) && startedAt > 0 ? Math.max(0, Date.now() - startedAt) : 0
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
  /**
   * Cover the inline scripts in prerendered documents with CSP hashes.
   *
   * Only React's streaming runtime needs this: a route that streams Suspense
   * content carries React's own inline swap script, which is neither Ruvyxa's
   * to move into a data block nor per-request, so a hash is what fits. The
   * build records the hashes and matching responses pick theirs up.
   *
   * Requires `contentSecurityPolicy` to define `script-src`. Pass `outDir` when
   * the project's build output is not `.ruvyxa`.
   */
  inlineScriptHashes?: boolean | { outDir?: string }
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

  const hashOptions = options.inlineScriptHashes
  if (hashOptions && configured.get('content-security-policy') === null) {
    throw new TypeError(
      'securityHeaders: inlineScriptHashes needs a contentSecurityPolicy to add sources to',
    )
  }
  const hashOutDir =
    typeof hashOptions === 'object' && typeof hashOptions.outDir === 'string'
      ? hashOptions.outDir
      : '.ruvyxa'

  return definePlugin({
    name: 'ruvyxa:security-headers',
    register({ http, build }) {
      // Built on the first response rather than here: the application root is
      // a property of the request context, not of registration.
      let lookup: ((pathname: string) => string[]) | null = null
      http.onResponse({
        ...(routes ? { match: routes } : {}),
        handler({ request, response, root }) {
          const output = new Headers(response.headers)
          configured.forEach((value, name) => output.set(name, value))
          if (hashOptions) {
            lookup ??= createInlineHashLookup(path.resolve(root, hashOutDir))
            const hashes = lookup(new URL(request.url).pathname)
            if (hashes.length > 0) {
              const policy = output.get('content-security-policy')
              if (policy) output.set('content-security-policy', withScriptSources(policy, hashes))
            }
          }
          return cloneResponse(response, output)
        },
      })
      if (hashOptions) build.onComplete(writeInlineHashManifest)
    },
  })
}

/**
 * Append sources to a policy's `script-src`, leaving every other directive be.
 *
 * The directive has to already exist: adding one would change a policy that
 * deliberately falls back to `default-src`, which is a different decision than
 * the one this option was asked to make.
 */
function withScriptSources(policy: string, sources: readonly string[]): string {
  let matched = false
  const directives = policy
    .split(';')
    .map((directive) => directive.trim())
    .filter((directive) => directive !== '')
    .map((directive) => {
      if (!/^script-src(\s|$)/i.test(directive)) return directive
      matched = true
      const missing = sources.filter((source) => !directive.includes(source))
      return missing.length > 0 ? `${directive} ${missing.join(' ')}` : directive
    })
  return matched ? directives.join('; ') : policy
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

// ─── headScriptHashes ────────────────────────────────────────────────────────

/**
 * CSP source hashes for the inline scripts and styles plugins contribute.
 *
 * A plugin's `head` entries are declared once at config load and are identical
 * on every request, so they can be covered by a hash instead of a nonce. That
 * is the whole reason a project can keep `script-src` free of
 * `'unsafe-inline'` while still using a plugin that injects a snippet.
 *
 * ```ts
 * const plugins = [analytics()]
 * export default config({
 *   plugins: [
 *     ...plugins,
 *     securityHeaders({
 *       contentSecurityPolicy: { 'script-src': ["'self'", ...headScriptHashes(plugins)] },
 *     }),
 *   ],
 * })
 * ```
 *
 * Pass the plugins whose head entries need covering — normally every plugin in
 * the config except `securityHeaders` itself. A plugin that loads its script
 * with `src` instead of inlining it needs no hash and contributes none; the
 * first-party `webVitals` is built that way deliberately.
 */
export function headScriptHashes(
  plugins: readonly RuvyxaPlugin[],
  options: { tag?: 'script' | 'style' } = {},
): string[] {
  if (!Array.isArray(plugins)) {
    throw new TypeError('headScriptHashes: pass the array of plugins to cover')
  }
  const tag = options.tag ?? 'script'
  const hashes = new Set<string>()
  for (const plugin of plugins) {
    for (const entry of plugin?.head ?? []) {
      if (entry.tag !== tag) continue
      const children = entry.children
      // An entry with no text has nothing to execute, so nothing to hash. A
      // hash for the empty string would be a source no browser ever matches.
      if (typeof children !== 'string' || children === '') continue
      // The browser hashes exactly the bytes between the tags, which is what
      // the server writes verbatim — no trimming, no re-encoding.
      hashes.add(`'sha256-${createHash('sha256').update(children, 'utf8').digest('base64')}'`)
    }
  }
  // Sorted so a config that reorders its plugins still produces the same policy
  // string, and a build stays byte-identical.
  return [...hashes].sort(compareStable)
}

// ─── prerendered inline script hashes ────────────────────────────────────────

/** Where the build records the hashes, relative to the build output directory. */
const INLINE_HASH_MANIFEST = 'csp-inline-hashes.json'

/**
 * Inline scripts that a hash has to cover, and the ones it must not.
 *
 * `src` scripts execute a file `script-src` already governs, and a data block
 * (`application/json`, `application/ld+json`) is not executed at all — hashing
 * either would publish a source that matches nothing.
 */
const INLINE_SCRIPT_PATTERN = /<script(?![^>]*\ssrc=)([^>]*)>([\s\S]*?)<\/script\s*>/gi

function isHashableScriptTag(attributes: string): boolean {
  const type = /\stype\s*=\s*("([^"]*)"|'([^']*)'|([^\s>]+))/i.exec(attributes)
  if (!type) return true
  const value = (type[2] ?? type[3] ?? type[4] ?? '').trim().toLowerCase()
  return value === '' || value === 'text/javascript' || value === 'module'
}

/** `'sha256-…'` sources for every inline script in one rendered document. */
function documentInlineHashes(html: string): string[] {
  const hashes = new Set<string>()
  INLINE_SCRIPT_PATTERN.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = INLINE_SCRIPT_PATTERN.exec(html)) !== null) {
    if (!isHashableScriptTag(match[1] ?? '')) continue
    const body = match[2] ?? ''
    if (body === '') continue
    hashes.add(`'sha256-${createHash('sha256').update(body, 'utf8').digest('base64')}'`)
  }
  return [...hashes].sort(compareStable)
}

/** The application path a prerendered file answers. */
function prerenderedPathname(prerenderDir: string, file: string): string {
  const relative = path.relative(prerenderDir, file).split(path.sep).join('/')
  const withoutIndex = relative.replace(/(^|\/)index\.html$/, '')
  const trimmed = withoutIndex.replace(/\.html$/, '')
  return trimmed === '' ? '/' : `/${trimmed}`
}

/**
 * Record the inline-script hashes of every prerendered document.
 *
 * A route that streams Suspense content carries React's own inline runtime —
 * the script that swaps a resolved boundary into place. It is React's, not
 * Ruvyxa's, so it cannot be moved into a data block the way the route
 * bootstrap was, and it is written into a build artifact that every request
 * reuses, so a per-request nonce would be baked in and therefore public.
 *
 * A hash is the mechanism that fits: the bytes are fixed once the artifact is
 * written. They are per-document, though — React's swap script names the
 * boundary ids it is completing — so they cannot be maintained by hand, which
 * is why the build records them.
 */
function writeInlineHashManifest(context: PluginBuildContext): void {
  const prerenderDir = path.join(context.outDir, 'prerender')
  if (!isDirectory(prerenderDir)) return
  const entries: Record<string, string[]> = {}
  for (const file of walkFiles(prerenderDir)) {
    if (!file.endsWith('.html')) continue
    const hashes = documentInlineHashes(readFileSync(file, 'utf8'))
    if (hashes.length === 0) continue
    entries[prerenderedPathname(prerenderDir, file)] = hashes
  }
  // Written even when empty, so a server reading it can tell "no document
  // needed a hash" apart from "the build never recorded any".
  const sorted = Object.fromEntries(
    Object.keys(entries)
      .sort(compareStable)
      .map((key) => [key, entries[key]]),
  )
  writeFileAtomic(
    path.join(context.outDir, INLINE_HASH_MANIFEST),
    `${JSON.stringify({ version: 1, documents: sorted }, null, 2)}\n`,
  )
}

/** Reads the recorded hashes once per process, tolerating their absence. */
function createInlineHashLookup(outDir: string): (pathname: string) => string[] {
  let documents: Record<string, string[]> | null | undefined
  return (pathname) => {
    if (documents === undefined) {
      try {
        const parsed = JSON.parse(
          readFileSync(path.join(outDir, INLINE_HASH_MANIFEST), 'utf8'),
        ) as {
          documents?: Record<string, string[]>
        }
        documents = parsed.documents ?? {}
      } catch {
        // No manifest: a development server has not built anything, and a
        // deployment that did not enable this at build time has nothing to
        // read. Either way the policy goes out without the extra sources
        // rather than the response failing.
        documents = null
      }
    }
    if (!documents) return []
    return documents[pathname] ?? documents[`${pathname}/`] ?? []
  }
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
    // Load-bearing despite discarding its result: `Headers.set`/`append` throw
    // on a value no header may carry — a newline above all — so writing each
    // configured value into a throwaway `Headers` is what rejects an injection
    // attempt at config time instead of at the first matching response.
    const probe = new Headers()
    if (rule.browser !== undefined) probe.set('cache-control', rule.browser)
    if (rule.cdn !== undefined) probe.set('cdn-cache-control', rule.cdn)
    for (const value of rule.vary ?? []) probe.append('vary', value)
    return { ...rule, vary: rule.vary ? [...rule.vary] : undefined }
  })
  const scoped = normalized.every((rule) => rule.source !== undefined)

  return definePlugin({
    name: 'ruvyxa:cache-rules',
    register({ http }) {
      http.onResponse({
        ...(scoped ? { match: normalized.map((rule) => rule.source as string) } : {}),
        handler({ request, response }) {
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

// ─── originGuard ─────────────────────────────────────────────────────────────

export interface OriginGuardOptions {
  /** Exact paths or trailing-`*` prefixes to guard. @default ["/api/*"] */
  routes?: string[]
  /** Methods allowed without same-origin evidence. @default ["GET","HEAD","OPTIONS"] */
  safeMethods?: string[]
  /** Absolute origins accepted in addition to the request's own. */
  allowOrigins?: string[]
  /** Status used for a rejected request. @default 403 */
  status?: number
}

const DEFAULT_SAFE_METHODS = ['GET', 'HEAD', 'OPTIONS']

/**
 * Rejects cross-site mutation requests to route handlers.
 *
 * Server actions already carry this protection in both hosts. A route under
 * `app/api/` does not: it is reachable by any origin, and a session cookie
 * defaults to `SameSite=Lax`, which a cross-site form POST still carries. This
 * plugin closes that gap for the routes it is pointed at.
 *
 * Opt-in rather than default, because an API meant to be called from another
 * origin is a legitimate design — that case is governed by CORS instead.
 *
 * The decision itself comes from `@ruvyxa/core/origin-policy`, shared with the
 * action endpoint and held against the native host by
 * `tests/fixtures/origin-policy-conformance.json`.
 *
 * It passes no trusted scheme: weighing `X-Forwarded-Proto` is only meaningful
 * against a trusted-proxy list, which a plugin has no access to. The host
 * comparison is the load-bearing check either way — a browser sets `Origin`
 * itself and a cross-site page cannot forge it.
 */
export function originGuard(options: OriginGuardOptions = {}): RuvyxaPlugin {
  const routes = normalizeRoutes(options.routes ?? ['/api/*'], 'originGuard') as string[]
  const safeMethods = new Set(
    (options.safeMethods ?? DEFAULT_SAFE_METHODS).map((method, index) => {
      if (typeof method !== 'string' || !/^[A-Za-z]+$/.test(method)) {
        throw new TypeError(`originGuard: safeMethods[${index}] must be an HTTP method token`)
      }
      return method.toUpperCase()
    }),
  )
  const allowOrigins = new Set(
    (options.allowOrigins ?? []).map((origin, index) => {
      validateAbsoluteHttpUrl(origin, `originGuard.allowOrigins[${index}]`)
      return new URL(origin).origin.toLowerCase()
    }),
  )
  const status = options.status ?? 403
  if (!Number.isInteger(status) || status < 400 || status > 499) {
    throw new TypeError('originGuard: status must be a 4xx integer')
  }

  return definePlugin({
    name: 'ruvyxa:origin-guard',
    register({ http }) {
      http.onRequest({
        match: routes,
        handler({ request }) {
          if (safeMethods.has(request.method.toUpperCase())) return undefined
          // The `Host` header is what the client actually sent. Both hosts
          // rebuild the request URL from that same header before a plugin sees
          // it, so the URL is the correct fallback rather than a weaker one.
          const host = request.headers.get('host') ?? new URL(request.url).host
          if (!originIsCrossSite(request.headers, host, { allowOrigins })) return undefined
          return new Response('Cross-origin request blocked\n', {
            status,
            headers: { 'content-type': 'text/plain; charset=utf-8' },
          })
        },
      })
    },
  })
}

// ─── healthCheck ─────────────────────────────────────────────────────────────

export interface HealthCheckOptions {
  /** Exact path the endpoint answers on. @default "/health" */
  path?: string
  /**
   * Build the response payload. Returning a string sends it as text; returning
   * an object sends it as JSON. Omit for a plain `ok` response.
   */
  check?: () => unknown | Promise<unknown>
  /** Status used when `check` throws. @default 503 */
  failureStatus?: number
}

/**
 * Serves a liveness endpoint from the request host, ahead of route rendering.
 *
 * A platform health probe should not depend on the render pipeline: a check
 * that renders a page reports the renderer, not the process. This answers from
 * the plugin host directly, so it stays truthful while rendering is degraded.
 */
export function healthCheck(options: HealthCheckOptions = {}): RuvyxaPlugin {
  const routePath = normalizePublicPath(options.path ?? '/health', 'healthCheck')
  if (options.check !== undefined && typeof options.check !== 'function') {
    throw new TypeError('healthCheck: check must be a function')
  }
  const failureStatus = options.failureStatus ?? 503
  if (!Number.isInteger(failureStatus) || failureStatus < 400 || failureStatus > 599) {
    throw new TypeError('healthCheck: failureStatus must be a 4xx or 5xx integer')
  }

  return definePlugin({
    name: 'ruvyxa:health-check',
    register({ http }) {
      http.route({
        path: routePath,
        method: ['GET', 'HEAD'],
        async handler() {
          try {
            const result = options.check ? await options.check() : 'ok'
            return healthResponse(200, result)
          } catch (error) {
            return healthResponse(failureStatus, {
              status: 'error',
              error: error instanceof Error ? error.message : String(error),
            })
          }
        },
      })
    },
  })
}

function healthResponse(status: number, payload: unknown): Response {
  const headers = { 'cache-control': 'no-store' }
  if (typeof payload === 'string') {
    return new Response(`${payload}\n`, {
      status,
      headers: { ...headers, 'content-type': 'text/plain; charset=utf-8' },
    })
  }
  return new Response(`${JSON.stringify(payload)}\n`, {
    status,
    headers: { ...headers, 'content-type': 'application/json; charset=utf-8' },
  })
}
