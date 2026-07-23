/**
 * Standalone serverless request handler for Ruvyxa.
 *
 * Provides a self-contained Request → Response function that does not depend
 * on the Rust host process or the NDJSON worker-pool protocol. Adapters
 * generate a thin platform wrapper that imports this handler.
 *
 * At build time, adapter-runner.mjs bundles route modules into the output
 * directory. This handler imports those pre-compiled modules and dispatches
 * requests using the build manifest.
 *
 * Supported rendering strategies:
 *   - SSR: full server render on every request
 *   - ISR: serve pre-rendered HTML, revalidate in background after TTL
 *   - PPR: serve pre-rendered shell, stream dynamic slots
 *   - CSR: serve static shell HTML
 *   - API: invoke method-specific handlers (GET/POST/PUT/DELETE/PATCH etc.)
 *
 * ISR/PPR behavior depends on platform capabilities passed via options.
 */

/**
 * @typedef {Object} RouteEntry
 * @property {string} id
 * @property {string} path
 * @property {'page'|'api'} kind
 * @property {string} file
 * @property {string[]} layoutChain
 * @property {{strategy: string, revalidate?: number, hasDynamicSlots?: boolean}} render
 */

/**
 * @typedef {Object} HandlerOptions
 * @property {RouteEntry[]} routes - Build manifest routes
 * @property {string} buildDir - Absolute path to the build output directory
 * @property {string} [basePath] - Optional base path prefix
 * @property {(routeId: string) => Promise<{render: (ctx: object) => Promise<string>}>} importPage
 *   Import a pre-compiled page module. Adapters supply this to abstract away
 *   platform-specific module resolution.
 * @property {(routeId: string) => Promise<Record<string, Function>>} importApi
 *   Import a pre-compiled API route module.
 * @property {(path: string, revalidate?: number) => string|{html: string, stale: boolean}|null} [readPrerendered]
 *   Synchronous read of a pre-rendered HTML file. ISR-capable adapters return
 *   freshness explicitly; a legacy string result is treated as stale.
 * @property {(path: string, html: string, revalidate: number) => void} [writePrerendered]
 *   Write pre-rendered HTML to ISR cache with a TTL.
 * @property {string[]} [supportedStrategies]
 *   Strategies the platform supports. Defaults to ['ssr','ssg','csr','isr','ppr','api'].
 *   Unsupported strategies produce a 501 response.
 */

/**
 * Create a serverless request handler.
 *
 * @param {HandlerOptions} options
 * @returns {(request: Request, runtimeContext?: {waitUntil?: (promise: Promise<unknown>) => void}) => Promise<Response>}
 */
export function createHandler(options) {
  const {
    routes,
    basePath = '',
    importPage,
    importApi,
    readPrerendered,
    writePrerendered,
    supportedStrategies = ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
  } = options
  const pendingRevalidations = new Map()

  // Pre-compile route patterns for matching. Sort by specificity so a
  // static segment always wins over a dynamic one at the same position —
  // manifest order is alphabetical, where "[" sorts before letters and
  // would otherwise shadow /blog/new behind /blog/[slug], diverging from
  // the dev server's static-first router.
  const compiledRoutes = routes
    .map((route) => ({
      ...route,
      pattern: compilePattern(route.path),
      specificity: routeSpecificity(route.path),
    }))
    .sort((left, right) => compareSpecificity(left.specificity, right.specificity))

  return async function handle(request, runtimeContext = {}) {
    const url = new URL(request.url)
    const pathname = stripBasePath(url.pathname, basePath)
    // A request outside the configured base path is not ours to serve.
    // Slicing unconditionally would turn `/other/thing` into `r/thing` and let
    // it match an unrelated route.
    if (pathname === null) {
      return new Response('Not Found', { status: 404 })
    }

    let match
    try {
      // Route matching percent-decodes parameters, which throws on malformed
      // input such as `/blog/%ZZ`. This ran outside the handler's try block, so
      // the URIError escaped as an unhandled rejection instead of a response.
      //
      // Matching uses the dev router's segment semantics (split_path in
      // crates/ruvyxa_dev_server/src/router.rs drops empty segments), so
      // `/docs/a/`, `/docs//a`, and `/docs/a` resolve to the same route with
      // the same params. Without this, the greedy catch-all regex captured the
      // trailing slash and produced params like ["a", ""] in deploys only.
      // The un-normalized pathname is still what render and the prerender
      // cache receive, matching what the dev server passes.
      match = matchRoute(compiledRoutes, normalizeMatchPath(pathname))
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      console.error(`[ruvyxa] Malformed request path ${pathname}:`, message)
      return new Response('Bad Request', {
        status: 400,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
      })
    }
    if (!match) {
      return new Response('Not Found', { status: 404 })
    }

    const { route, params } = match

    // Check platform support for the route's strategy
    const strategy = route.kind === 'api' ? 'api' : route.render.strategy
    if (!supportedStrategies.includes(strategy)) {
      return new Response(
        `RUV2210 Platform does not support rendering strategy "${strategy}" for route ${route.path}. ` +
          `Supported: ${supportedStrategies.join(', ')}.`,
        { status: 501, headers: { 'content-type': 'text/plain; charset=utf-8' } },
      )
    }

    try {
      if (route.kind === 'api') {
        return await handleApi(route, request, params)
      }
      return await handlePage(route, request, pathname, params, runtimeContext)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      console.error(`[ruvyxa] Error handling ${pathname}:`, message)
      // Log the detail server-side only: serverless is production, and the
      // dev server likewise never exposes internal error text to clients.
      return new Response('Internal Server Error', {
        status: 500,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
      })
    }
  }

  async function handleApi(route, request, params) {
    const mod = await importApi(route.id)
    const method = request.method.toUpperCase()
    const handler = mod[method]

    if (typeof handler !== 'function') {
      return new Response(`Method ${method} is not allowed`, {
        status: 405,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
      })
    }

    const result = await handler({ request, params })
    return normalizeResponse(result)
  }

  async function handlePage(route, request, pathname, params, runtimeContext) {
    const strategy = route.render.strategy

    // CSR: return pre-rendered shell (no server render needed)
    if (strategy === 'csr') {
      const cached = normalizeCacheEntry(readPrerendered?.(pathname))
      if (cached) {
        return new Response(cached.html, {
          status: 200,
          headers: { 'content-type': 'text/html; charset=utf-8' },
        })
      }
      // Fallback: render the shell
      return await renderPage(route, pathname, params)
    }

    // SSG: serve pre-rendered HTML directly
    if (strategy === 'ssg') {
      const cached = normalizeCacheEntry(readPrerendered?.(pathname))
      if (cached) {
        return new Response(cached.html, {
          status: 200,
          headers: { 'content-type': 'text/html; charset=utf-8' },
        })
      }
      // Fallback to SSR if pre-rendered not available
      return await renderPage(route, pathname, params)
    }

    // ISR: serve cached HTML, revalidate in background if stale
    if (strategy === 'isr') {
      const revalidate = route.render.revalidate ?? 60
      const cached = normalizeCacheEntry(readPrerendered?.(pathname, revalidate))
      if (cached) {
        if (cached.stale) {
          const revalidation = scheduleRevalidation(route, pathname, params)
          if (revalidation) {
            if (typeof runtimeContext.waitUntil === 'function') {
              runtimeContext.waitUntil(revalidation)
            } else {
              // A serverless runtime may freeze untracked work as soon as the
              // response is returned. Waiting is slower, but never loses the
              // refresh when the platform exposes no lifetime hook.
              await revalidation
            }
          }
        }
        return new Response(cached.html, {
          status: 200,
          headers: {
            'content-type': 'text/html; charset=utf-8',
            'x-ruvyxa-isr': 'HIT',
            'cache-control': `s-maxage=${route.render.revalidate ?? 60}, stale-while-revalidate`,
          },
        })
      }
      // Cache miss: render on demand
      const rendered = await renderPage(route, pathname, params)
      // Cache the result for future requests
      if (writePrerendered && rendered.status === 200) {
        const body = await rendered.clone().text()
        writePrerendered(pathname, body, route.render.revalidate ?? 60)
      }
      return rendered
    }

    // PPR: serve pre-rendered shell, then dynamic content
    if (strategy === 'ppr') {
      // For serverless without streaming support, fall back to full SSR
      // Platform wrappers can override this with streaming if available
      return await renderPage(route, pathname, params)
    }

    // SSR (default): full server render
    return await renderPage(route, pathname, params)
  }

  async function renderPage(route, pathname, params) {
    const mod = await importPage(route.id)
    const html = await mod.render({ path: pathname, params: params ?? {} })
    return new Response(html, {
      status: 200,
      headers: { 'content-type': 'text/html; charset=utf-8' },
    })
  }

  function scheduleRevalidation(route, pathname, params) {
    if (!writePrerendered) return null
    const pending = pendingRevalidations.get(pathname)
    if (pending) return pending
    const revalidation = Promise.resolve().then(async () => {
      try {
        const mod = await importPage(route.id)
        const html = await mod.render({ path: pathname, params: params ?? {} })
        writePrerendered(pathname, html, route.render.revalidate ?? 60)
      } catch (error) {
        console.error(`[ruvyxa] ISR revalidation failed for ${pathname}:`, error)
      } finally {
        pendingRevalidations.delete(pathname)
      }
    })
    pendingRevalidations.set(pathname, revalidation)
    return revalidation
  }
}

function normalizeCacheEntry(value) {
  if (typeof value === 'string') return { html: value, stale: true }
  if (!value || typeof value !== 'object' || typeof value.html !== 'string') return null
  return { html: value.html, stale: value.stale === true }
}

// ─── Prerender Cache Paths ──────────────────────────────────────────────────

/**
 * Map a request path to the relative location of its pre-rendered HTML.
 *
 * Mirrors the build writer, which stores `<prerenderDir>/<path>/index.html`
 * using the raw (still percent-encoded) route path, so this must not decode.
 *
 * Returns `null` when the path cannot be mapped to a contained location.
 * Adapters join the result onto their cache directory and touch the file
 * system, so this is the single place that decides what is in bounds — the
 * platform URL parser is not a substitute, because adapters may be handed a
 * path from a source that never went through it.
 *
 * @param {string} pathname Request path, beginning with `/`.
 * @returns {string|null} A `.../index.html` relative path, or null if unsafe.
 */
/**
 * Reject a path segment that could escape, or misname, the cache directory.
 *
 * Written as explicit character tests rather than a regular expression: this
 * guard decides what reaches the file system, and it must stay obvious that
 * separators, control characters, and Windows stream/drive separators are all
 * covered.
 */
function isUnsafeSegment(segment) {
  if (segment === '.' || segment === '..') return true
  for (const char of segment) {
    if (char === '/' || char === '\\' || char === ':') return true
    const code = char.codePointAt(0)
    if (code < 0x20 || code === 0x7f) return true
  }
  return false
}

export function prerenderRelativePath(pathname) {
  if (typeof pathname !== 'string' || !pathname.startsWith('/')) return null

  const segments = []
  for (const segment of pathname.split('/')) {
    if (segment === '') continue
    if (isUnsafeSegment(segment)) return null
    segments.push(segment)
  }

  return segments.length === 0 ? 'index.html' : `${segments.join('/')}/index.html`
}

// ─── Route Matching ─────────────────────────────────────────────────────────

/**
 * Remove `basePath` from a request path.
 *
 * Returns the remaining path, or `null` when the request falls outside the
 * base path and must not be served by this handler.
 */
function stripBasePath(pathname, basePath) {
  if (!basePath) return pathname

  const prefix = basePath.endsWith('/') ? basePath.slice(0, -1) : basePath
  if (!prefix) return pathname
  if (pathname === prefix) return '/'
  // Require a segment boundary so `/appointments` is not treated as `/app`
  // plus `ointments`.
  if (!pathname.startsWith(`${prefix}/`)) return null
  return pathname.slice(prefix.length) || '/'
}

/**
 * Collapse duplicate slashes and drop the trailing slash so pattern matching
 * sees the same segments as the dev server's router, which splits on `/` and
 * filters empty segments.
 */
function normalizeMatchPath(pathname) {
  if (pathname === '/') return pathname
  const segments = pathname.split('/').filter(Boolean)
  return segments.length === 0 ? '/' : `/${segments.join('/')}`
}

/**
 * Compile a route path pattern into a regex and parameter names.
 * Supports:
 *   - Static segments: /about
 *   - Dynamic segments: /blog/[slug]
 *   - Catch-all segments: /docs/[...path]
 *   - Optional catch-all: /docs/[[...path]]
 */
function compilePattern(routePath) {
  if (routePath === '/') {
    return { regex: /^\/$/, paramNames: [], catchAll: null }
  }

  const paramNames = []
  let catchAll = null
  const segments = routePath.split('/').filter(Boolean)
  let pattern = '^'

  for (const segment of segments) {
    // Optional catch-all: [[...name]]
    const optionalCatchAll = segment.match(/^\[\[\.\.\.(\w+)\]\]$/)
    if (optionalCatchAll) {
      paramNames.push(optionalCatchAll[1])
      catchAll = { name: optionalCatchAll[1], optional: true }
      pattern += '(?:/(.*))?'
      continue
    }

    // Catch-all: [...name]
    const catchAllMatch = segment.match(/^\[\.\.\.(\w+)\]$/)
    if (catchAllMatch) {
      paramNames.push(catchAllMatch[1])
      catchAll = { name: catchAllMatch[1], optional: false }
      pattern += '/(.+)'
      continue
    }

    // Dynamic segment: [name]
    const dynamicMatch = segment.match(/^\[(\w+)\]$/)
    if (dynamicMatch) {
      paramNames.push(dynamicMatch[1])
      pattern += '/([^/]+)'
      continue
    }

    // Static segment
    pattern += `/${escapeRegex(segment)}`
  }

  pattern += '/?$'
  return { regex: new RegExp(pattern), paramNames, catchAll }
}

/**
 * Per-segment specificity score: static (0) < dynamic (1) < catch-all (2)
 * < optional catch-all (3). Lower-scoring routes match first.
 */
function routeSpecificity(routePath) {
  if (routePath === '/') return [0]
  return routePath
    .split('/')
    .filter(Boolean)
    .map((segment) => {
      if (/^\[\[\.\.\.\w+\]\]$/.test(segment)) return 3
      if (/^\[\.\.\.\w+\]$/.test(segment)) return 2
      if (/^\[\w+\]$/.test(segment)) return 1
      return 0
    })
}

function compareSpecificity(left, right) {
  const length = Math.max(left.length, right.length)
  for (let index = 0; index < length; index++) {
    const leftScore = left[index] ?? -1
    const rightScore = right[index] ?? -1
    if (leftScore !== rightScore) return leftScore - rightScore
  }
  return 0
}

function matchRoute(compiledRoutes, pathname) {
  for (const route of compiledRoutes) {
    const match = route.pattern.regex.exec(pathname)
    if (!match) continue

    const params = {}
    for (let i = 0; i < route.pattern.paramNames.length; i++) {
      const name = route.pattern.paramNames[i]
      const value = match[i + 1]

      if (route.pattern.catchAll && name === route.pattern.catchAll.name) {
        // Decode each captured segment like the dev server does; leaving
        // them encoded makes /docs/a%20b produce different params in
        // serverless deploys than in development.
        //
        // An optional catch-all that captured nothing stays absent rather than
        // becoming `[]`. The documented contract is "undefined at the parent
        // route", and the dev server's router omits the key there, so emitting
        // an empty array would make `/shop` behave differently in a deploy.
        if (value) {
          params[name] = value.split('/').map((segment) => decodeURIComponent(segment))
        }
      } else {
        params[name] = value ? decodeURIComponent(value) : undefined
      }
    }

    return { route, params }
  }
  return null
}

function escapeRegex(str) {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

// ─── Response Normalization ─────────────────────────────────────────────────

function normalizeResponse(result) {
  if (result instanceof Response) return result
  return Response.json(result)
}
