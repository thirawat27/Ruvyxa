import path from 'node:path'
import { createHash } from 'node:crypto'
import { existsSync, readFileSync } from 'node:fs'
import { definePlugin } from '@ruvyxa/core/plugin'
import type { PluginBuildContext, RuvyxaPlugin } from '@ruvyxa/core/plugin'

import { matchSource } from './http.js'
import {
  compareStable,
  escapeHtmlAttribute,
  normalizePublicFilePath,
  normalizePublicPath,
  normalizeRoutes,
  uniqueStrings,
  walkFiles,
  writeFileAtomic,
  writePublicAsset,
} from './shared.js'

// ─── pwa ─────────────────────────────────────────────────────────────────────

export interface PwaIcon {
  src: string
  sizes: string
  type?: string
  /**
   * The three values the manifest spec defines, plus an escape hatch.
   *
   * `(string & {})` keeps the literals in autocomplete. A plain `| string`
   * absorbs them: the union collapses to `string` and the editor stops
   * offering `maskable` at all.
   */
  purpose?: 'any' | 'maskable' | 'monochrome' | (string & {})
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
  /**
   * Pins the plugin-owned cache name instead of deriving it from the build.
   *
   * The derived default names a new cache per build, so a change to an
   * unfingerprinted asset reaches a returning visitor. Setting this takes that
   * over: the cache is invalidated only when this value changes, and
   * forgetting to change it serves the previous bytes indefinitely. Use it
   * only when a deliberately stable cache is worth that.
   */
  version?: string
}

/**
 * Generates a web manifest and service worker, serves them in dev, and wires HTML automatically.
 *
 * The worker's cache name derives from the scope, the precache list, the
 * offline fallback, and the build manifest, so each build claims a fresh cache
 * and `activate` deletes the previous one. The trade is one cold fetch per
 * runtime-cached asset after a deploy — and a visitor who goes offline between
 * the deploy and that fetch has an empty runtime cache — against the
 * alternative, which is a cache-first worker serving an unfingerprinted asset
 * from the install-time copy forever.
 */
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
  assertScopeIsWithinTheWorkersDirectory(serviceWorkerPath, scope)
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
  // The prefix stays derived from the scope alone, and therefore stable across
  // builds, because `activate` deletes exactly the caches that start with it. A
  // prefix that moved with the build would stop recognising the previous
  // build's cache as one of ours and would never drop it. Only the suffix
  // carries build identity.
  const cachePrefix = `ruvyxa-pwa-${createHash('sha256').update(scope).digest('hex').slice(0, 12)}-`
  /**
   * The cache name for one build.
   *
   * Derived, never stamped: a fixed suffix is only correct while somebody
   * remembers to bump it, and this worker is cache-first with no revalidation,
   * so forgetting is silent and permanent. `buildIdentity` is the serialized
   * build manifest at `build.onComplete` and the empty string before a build
   * has run — in development, where nothing is deployed and the worker is
   * served `no-cache`.
   */
  const cacheName = (buildIdentity: string): string => {
    if (options.version !== undefined) return `${cachePrefix}${options.version}`
    const digest = createHash('sha256')
      .update(stableJson([scope, precache, offlineFallback ?? null, buildIdentity]))
      .digest('hex')
      .slice(0, 12)
    return `${cachePrefix}${digest}`
  }
  // Reassigned at `build.onComplete`. The `http.onRequest` path below serves
  // `/sw.js` from this binding, so the worker a host serves and the worker the
  // build writes claim the same cache rather than fighting over two.
  let serviceWorkerBody = createServiceWorker(cacheName(''), cachePrefix, precache, offlineFallback)
  const middlewareRoutes = uniqueStrings([
    ...htmlRoutes,
    manifestPath,
    serviceWorkerPath,
    registerPath,
  ])

  return definePlugin({
    name: 'ruvyxa:pwa',
    register({ http, build }) {
      http.onRequest({
        match: middlewareRoutes,
        handler({ request }) {
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
      })
      http.onResponse({
        match: middlewareRoutes,
        async handler({ request, response }) {
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
      build.onComplete((context) => {
        serviceWorkerBody = createServiceWorker(
          cacheName(stableJson(context.manifest)),
          cachePrefix,
          precache,
          offlineFallback,
        )
        writePublicAsset(context, manifestPath, manifestBody)
        writePublicAsset(context, serviceWorkerPath, serviceWorkerBody)
        writePublicAsset(context, registerPath, registerBody)
        patchPrerenderedHtml(context, htmlRoutes, manifestPath, registerPath)
      })
    },
  })
}

/**
 * A worker may claim its own directory and no more.
 *
 * `Service-Worker-Allowed` is the one header that widens that, and this plugin's
 * request handler is the only thing in the repository that writes it: no
 * adapter, no platform config, and no static handler reproduces it, while
 * `build.onComplete` writes the worker as a plain public asset. So a broad
 * scope registers wherever the plugin answers the request and fails with
 * `SecurityError: The path of the provided scope … is not under the max scope
 * allowed` wherever a CDN or a static host serves the file instead — production
 * only, on the deployments that are hardest to reach.
 *
 * The comparison is a prefix test against a directory that always ends in `/`,
 * which is what keeps `/assets-2/` from reading as inside `/assets/`. A scope
 * written without its trailing slash is rejected for the same reason the
 * browser rejects it: service worker scope matching is a prefix test over the
 * serialized URL, so `/assets` covers `/assets-2` and is genuinely wider than
 * the directory the script sits in.
 */
function assertScopeIsWithinTheWorkersDirectory(serviceWorkerPath: string, scope: string): void {
  const directory = serviceWorkerPath.slice(0, serviceWorkerPath.lastIndexOf('/') + 1)
  if (scope.startsWith(directory)) return
  throw new TypeError(
    `pwa: scope "${scope}" is outside "${directory}", the widest scope a worker at ` +
      `"${serviceWorkerPath}" can claim once a CDN or static host serves it. ` +
      `Move serviceWorkerPath to the directory the scope needs, or narrow scope to "${directory}".`,
  )
}

/**
 * Serializes a value for hashing with object keys in a fixed order.
 *
 * `JSON.stringify` emits keys in insertion order, so two structurally equal
 * manifests that were assembled differently would hash differently and name two
 * caches for one build. Keys sort with `compareStable` — `localeCompare` orders
 * by the host's ICU locale and would make the cache name machine-dependent.
 */
function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`
  if (value !== null && typeof value === 'object') {
    const record = value as Record<string, unknown>
    const entries = Object.keys(record)
      .sort(compareStable)
      .map((key) => `${JSON.stringify(key)}:${stableJson(record[key])}`)
    return `{${entries.join(',')}}`
  }
  // `undefined`, a function, or a symbol stringifies to `undefined`, which is
  // not JSON; name it so the digest stays a total function of the input.
  return JSON.stringify(value) ?? 'null'
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
  // Replacer functions, not replacement strings. `String.replace` reads `$&`,
  // `` $` ``, `$'`, and `$1` out of a *replacement string*, and these carry a
  // configured path through `escapeHtmlAttribute` — which turns `&` into
  // `&amp;` and so cannot neutralize a `$`. A `manifestPath` containing `$&`
  // therefore substituted the matched `</head>` into its own `href` and emitted
  // a second one. A function's return value is always literal.
  let output = html.includes('</head>')
    ? html.replace('</head>', () => `${manifestTag}</head>`)
    : `${manifestTag}${html}`
  output = output.includes('</body>')
    ? output.replace('</body>', () => `${registerTag}</body>`)
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
    if (injected !== html) writeFileAtomic(file, injected)
  }
}
