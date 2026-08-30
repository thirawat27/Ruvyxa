import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  alias,
  bundleBudget,
  cacheRules,
  contentEngine,
  DEFAULT_INDEX_LOCALE,
  feed,
  fonts,
  headers,
  headScriptHashes,
  healthCheck,
  llmsTxt,
  observability,
  openApi,
  originGuard,
  pwa,
  redirects,
  requireEnv,
  robots,
  searchIndex,
  securityHeaders,
  sitemap,
  webVitals,
  wellKnown,
} from '../../../packages/ruvyxa/dist/plugins.js'
import {
  MAX_TRACKED_PLUGIN_RATE_LIMIT_KEYS,
  boundedRateLimitKey,
  consumeFixedWindow,
} from '../../../packages/ruvyxa/dist/plugins/shared.js'
import {
  RESERVED_FRAMEWORK_PATHS,
  createPluginRegistry,
  decodedRequestPathname,
  dispatchPluginRequest,
  dispatchPluginResponse,
  matchesPatterns,
} from '../../../packages/ruvyxa/runtime/plugin-http.mjs'
import { canonicalRoutePath } from '../../../packages/ruvyxa/runtime/route-match.mjs'
import { definePlugin } from '../../../packages/@ruvyxa/core/dist/plugin.js'

/**
 * Runs a plugin registration with adapters for the focused hook tests below.
 *
 * `environment` defaults to production because that is what a host reports when
 * it says nothing, and because most plugins register identically either way.
 */
function register(plugin, environment = 'production') {
  const registered = {
    middleware: [],
    resolveId: [],
    buildComplete: [],
    routes: [],
    diagnostics: [],
  }
  plugin.register({
    environment,
    http: {
      onRequest(value) {
        const registration = typeof value === 'function' ? { handler: value } : value
        registered.middleware.push({
          routes: registration.match,
          onRequest(request, context = middlewareContext) {
            return registration.handler({ ...context, request, next() {} })
          },
        })
      },
      onResponse(value) {
        const registration = typeof value === 'function' ? { handler: value } : value
        const existing = [...registered.middleware]
          .reverse()
          .find(
            (entry) =>
              entry.onResponse === undefined &&
              JSON.stringify(entry.routes) === JSON.stringify(registration.match),
          )
        const target = existing ?? { routes: registration.match }
        target.onResponse = (request, response, context = middlewareContext) =>
          registration.handler({ ...context, request, response, next() {} })
        if (!existing) registered.middleware.push(target)
      },
      route(registration) {
        registered.routes.push(registration)
      },
    },
    build: {
      onStart() {},
      onResolve(hook) {
        registered.resolveId.push((id, importer, context) => hook({ ...context, id, importer }))
      },
      onLoad() {},
      onTransform() {},
      onComplete(hook) {
        registered.buildComplete.push(hook)
      },
    },
    dev: { onFileChange() {} },
    diagnostics: {
      report(diagnostic) {
        registered.diagnostics.push(diagnostic)
      },
    },
    native: { claim() {} },
  })
  return registered
}

/**
 * Record every locale a build asks `Intl` for while `run` executes.
 *
 * Node reads its ICU default from the operating system and ignores `LC_ALL`
 * and `LANG` on Windows, so a test cannot prove host-independence by running
 * the same build under two locales. It can prove the stronger property that
 * makes the host irrelevant: that no call reaches `Intl` without a locale at
 * all. `undefined` is the value that means "ask this machine".
 */
async function recordRequestedLocales(run) {
  const requested = []
  const RealSegmenter = Intl.Segmenter
  // `no-restricted-properties` and `no-extend-native` are both disabled across
  // this helper, and both bans are the reason it exists: it observes the exact
  // calls they forbid so a test can assert none of them reached ICU without a
  // locale. The patch is scoped to one `run()` and restored in `finally`.
  // oxlint-disable eslint/no-restricted-properties
  // oxlint-disable eslint/no-extend-native
  const realFold = String.prototype.toLocaleLowerCase
  Intl.Segmenter = class RecordingSegmenter extends RealSegmenter {
    constructor(locale, options) {
      requested.push(['Intl.Segmenter', locale])
      super(locale, options)
    }
  }
  String.prototype.toLocaleLowerCase = function recordingFold(locale) {
    requested.push(['toLocaleLowerCase', locale])
    return realFold.call(this, locale)
  }
  try {
    await run()
  } finally {
    Intl.Segmenter = RealSegmenter
    String.prototype.toLocaleLowerCase = realFold
  }
  // oxlint-enable eslint/no-extend-native
  // oxlint-enable eslint/no-restricted-properties
  return requested
}

const middlewareContext = { plugin: 'test', root: 'D:/app' }

function request(target, init) {
  return new Request(`http://ruvyxa.local${target}`, init)
}

const tempDirs = []
function tempBuildContext(manifest) {
  const outDir = mkdtempSync(path.join(tmpdir(), 'ruvyxa-plugins-'))
  tempDirs.push(outDir)
  return { root: 'D:/app', outDir, manifest }
}

/** The cache a generated service worker claims, read out of its own source. */
function cacheNameOf(serviceWorkerSource) {
  const matched = serviceWorkerSource.match(/const CACHE = "([^"]+)"/)
  assert.ok(matched, `no CACHE constant in:\n${serviceWorkerSource}`)
  return matched[1]
}

after(() => {
  for (const dir of tempDirs) rmSync(dir, { recursive: true, force: true })
})

describe('redirects()', () => {
  const plugin = redirects([
    { source: '/old', destination: '/new' },
    { source: '/docs/*', destination: '/manual/*', permanent: true },
    { source: '/away', destination: 'https://example.com/landing' },
  ])
  const { middleware } = register(plugin)
  const { onRequest, routes } = middleware[0]

  it('reports its sources as middleware routes for native prefiltering', () => {
    assert.deepEqual(routes, ['/old', '/docs/*', '/away'])
  })

  it('redirects exact matches with 307 and preserves the query string', async () => {
    const response = await onRequest(request('/old?a=1'), middlewareContext)
    assert.equal(response.status, 307)
    assert.equal(response.headers.get('location'), '/new?a=1')
  })

  it('appends the wildcard remainder and honors permanent', async () => {
    const response = await onRequest(request('/docs/guide/intro'), middlewareContext)
    assert.equal(response.status, 308)
    assert.equal(response.headers.get('location'), '/manual/guide/intro')
  })

  it('never appends the query to absolute external destinations', async () => {
    const response = await onRequest(request('/away?tracking=1'), middlewareContext)
    assert.equal(response.headers.get('location'), 'https://example.com/landing')
  })

  it('lets unmatched requests continue', async () => {
    assert.equal(await onRequest(request('/other'), middlewareContext), undefined)
  })

  it('accepts the documented global wildcard source', async () => {
    const { middleware } = register(redirects([{ source: '*', destination: '/maintenance' }]))
    assert.deepEqual(middleware[0].routes, ['*'])

    const response = await middleware[0].onRequest(
      request('/anywhere?from=test'),
      middlewareContext,
    )
    assert.equal(response.status, 307)
    assert.equal(response.headers.get('location'), '/maintenance?from=test')
  })

  it('rejects sources that do not start with a slash', () => {
    assert.throws(() => redirects([{ source: 'old', destination: '/new' }]), TypeError)
  })

  // The wildcard remainder is request-controlled. Concatenating it onto a
  // destination let `/go//evil.example` and `/go/\evil.example` produce a
  // `//evil.example` Location, which browsers resolve as another origin — the
  // same escape `safeReturnTo` already blocks in @ruvyxa/auth.
  it('never redirects off the requesting origin through the wildcard remainder', async () => {
    const { middleware } = register(redirects([{ source: '/go/*', destination: '/*' }]))
    for (const escape of ['/go//evil.example', '/go/\\evil.example', '/go//evil.example/path']) {
      const response = await middleware[0].onRequest(request(escape), middlewareContext)
      assert.equal(response, undefined, escape)
    }

    const same = await middleware[0].onRequest(request('/go/inside'), middlewareContext)
    assert.equal(same.headers.get('location'), '/inside')
  })

  it('keeps an absolute destination pinned to its own configured origin', async () => {
    const { middleware } = register(
      redirects([{ source: '/cdn/*', destination: 'https://assets.example/*' }]),
    )
    const response = await middleware[0].onRequest(
      request('/cdn//evil.example/logo.png'),
      middlewareContext,
    )
    assert.equal(
      new URL(response.headers.get('location')).origin,
      'https://assets.example',
      'a request path must not repoint an absolute destination',
    )
  })

  it('rejects destinations a browser reads as another origin', () => {
    for (const destination of ['*', '//evil.example', '/\\evil.example', 'ftp://example.com/x']) {
      assert.throws(
        () => redirects([{ source: '/x', destination }]),
        TypeError,
        `destination ${destination}`,
      )
    }
  })
})

describe('headers()', () => {
  it('sets headers on matching responses and scopes its routes', async () => {
    const plugin = headers([
      { source: '/api/*', headers: { 'cache-control': 'no-store' } },
      { source: '/api/versioned', headers: { 'x-api-version': '2' } },
    ])
    const { middleware } = register(plugin)
    assert.deepEqual(middleware[0].routes, ['/api/*', '/api/versioned'])

    const response = await middleware[0].onResponse(
      request('/api/versioned'),
      new Response('body', { status: 201, headers: { 'x-existing': 'kept' } }),
      middlewareContext,
    )
    assert.equal(response.status, 201)
    assert.equal(response.headers.get('x-existing'), 'kept')
    assert.equal(response.headers.get('cache-control'), 'no-store')
    assert.equal(response.headers.get('x-api-version'), '2')
    assert.equal(await response.text(), 'body')
  })

  it('returns undefined for unmatched paths so responses pass through untouched', async () => {
    const { middleware } = register(headers([{ source: '/admin', headers: { a: 'b' } }]))
    const result = await middleware[0].onResponse(
      request('/public'),
      new Response('x'),
      middlewareContext,
    )
    assert.equal(result, undefined)
  })

  it('omits middleware routes when any rule is unscoped', () => {
    const { middleware } = register(headers([{ headers: { 'x-global': '1' } }]))
    assert.equal(middleware[0].routes, undefined)
  })
})

describe('observability()', () => {
  it('propagates correlation metadata and records timing across request/response hooks', async () => {
    const entries = []
    const { middleware } = register(
      observability({
        routes: ['/api/*'],
        logger(entry) {
          entries.push(entry)
        },
      }),
    )
    const plugin = middleware[0]
    assert.deepEqual(plugin.routes, ['/api/*'])

    const observedRequest = await plugin.onRequest(request('/api/users?secret=hidden'))
    assert.match(observedRequest.headers.get('x-request-id'), /^[0-9a-f-]{36}$/)
    assert.match(observedRequest.headers.get('traceparent'), /^00-[0-9a-f]{32}-[0-9a-f]{16}-01$/)

    const response = await plugin.onResponse(
      observedRequest,
      new Response('ok', { status: 202, headers: { 'server-timing': 'render;dur=4' } }),
    )
    assert.equal(response.headers.get('x-request-id'), observedRequest.headers.get('x-request-id'))
    assert.match(response.headers.get('server-timing'), /render;dur=4, ruvyxa;dur=\d+/)
    assert.equal(entries.length, 1)
    assert.equal(entries[0].pathname, '/api/users')
    assert.equal(entries[0].status, 202)
    assert.equal('search' in entries[0], false)
  })

  it('reports no duration when the response hook runs without the request hook', async () => {
    // An earlier plugin can answer the request with its own `Response`, which
    // leaves the start header unset while this plugin's response hook still
    // runs. `Number(null)` is `0` and `0` is finite, so the old guard let that
    // through and reported `Date.now()` — about fifty-six years — as the
    // request duration, in the log entry and in `Server-Timing` alike.
    const entries = []
    const { middleware } = register(
      observability({
        logger(entry) {
          entries.push(entry)
        },
      }),
    )
    const response = await middleware[0].onResponse(request('/no-request-hook'), new Response('ok'))

    assert.equal(entries[0].durationMs, 0)
    assert.equal(response.headers.get('server-timing'), 'ruvyxa;dur=0')
  })

  it('replaces untrusted request IDs and invalid trace context', async () => {
    const { middleware } = register(observability({ log: false }))
    const output = await middleware[0].onRequest(
      request('/', { headers: { 'x-request-id': 'contains whitespace', traceparent: 'bad' } }),
    )
    assert.notEqual(output.headers.get('x-request-id'), 'contains whitespace')
    assert.match(output.headers.get('traceparent'), /^00-/)
  })

  it('keeps the response healthy when a custom log sink fails', async () => {
    const originalError = console.error
    const sinkFailures = []
    console.error = (...args) => sinkFailures.push(args)
    try {
      const { middleware } = register(
        observability({
          logger() {
            throw new Error('sink unavailable')
          },
        }),
      )
      const observedRequest = await middleware[0].onRequest(request('/healthy'))
      const response = await middleware[0].onResponse(observedRequest, new Response('ok'))

      assert.equal(await response.text(), 'ok')
      assert.equal(sinkFailures.length, 1)
      assert.deepEqual(sinkFailures[0], ['[ruvyxa:observability] log sink failed'])
    } finally {
      console.error = originalError
    }
  })
})

describe('securityHeaders()', () => {
  it('serializes CSP directives and applies explicit policy headers', async () => {
    const { middleware } = register(
      securityHeaders({
        routes: ['/admin/*'],
        contentSecurityPolicy: { 'default-src': ["'self'"], 'object-src': ["'none'"] },
        permissionsPolicy: 'camera=(self)',
      }),
    )
    assert.deepEqual(middleware[0].routes, ['/admin/*'])
    const response = await middleware[0].onResponse(request('/admin/users'), new Response('ok'))
    assert.equal(
      response.headers.get('content-security-policy'),
      "default-src 'self'; object-src 'none'",
    )
    assert.equal(response.headers.get('permissions-policy'), 'camera=(self)')
    assert.equal(
      response.headers.get('strict-transport-security'),
      'max-age=31536000; includeSubDomains',
    )
  })

  it('rejects malformed CSP directives and header values during config load', () => {
    assert.throws(
      () => securityHeaders({ contentSecurityPolicy: { 'script-src;': ["'self'"] } }),
      TypeError,
    )
    assert.throws(
      () => securityHeaders({ headers: { 'x-test': 'ok\r\ninjected: yes' } }),
      TypeError,
    )
  })
})

describe('cacheRules()', () => {
  it('applies the last matching cache policy and merges Vary values', async () => {
    const { middleware } = register(
      cacheRules([
        { source: '/api/*', browser: 'no-store', vary: ['accept-encoding'] },
        {
          source: '/api/public/*',
          browser: 'public, max-age=60',
          cdn: 'max-age=300',
          vary: ['origin'],
        },
      ]),
    )
    const response = await middleware[0].onResponse(
      request('/api/public/items'),
      new Response('ok', { headers: { vary: 'Accept-Encoding' } }),
    )
    assert.equal(response.headers.get('cache-control'), 'public, max-age=60')
    assert.equal(response.headers.get('cdn-cache-control'), 'max-age=300')
    assert.equal(response.headers.get('vary'), 'Accept-Encoding, origin')
  })

  it('requires at least one effective rule value', () => {
    assert.throws(() => cacheRules([]), TypeError)
    assert.throws(() => cacheRules([{ source: '/empty' }]), TypeError)
  })
})

describe('sitemap()', () => {
  const manifest = {
    routes: [
      { path: '/', kind: 'page' },
      { path: '/about', kind: 'page' },
      { path: '/blog/[slug]', kind: 'page' },
      { path: '/api/users', kind: 'api' },
      { path: '/drafts/secret', kind: 'page' },
    ],
  }

  it('writes static page routes into the served asset directory', async () => {
    const plugin = sitemap({ siteUrl: 'https://example.com/', exclude: ['/drafts/*'] })
    const { buildComplete } = register(plugin)
    const context = tempBuildContext(manifest)
    await buildComplete[0](context)

    const xml = readFileSync(path.join(context.outDir, 'assets', 'sitemap.xml'), 'utf8')
    assert.match(xml, /<loc>https:\/\/example\.com\/<\/loc>/)
    assert.match(xml, /<loc>https:\/\/example\.com\/about<\/loc>/)
    assert.doesNotMatch(xml, /blog/)
    assert.doesNotMatch(xml, /api/)
    assert.doesNotMatch(xml, /drafts/)
  })

  it('optionally writes a robots.txt pointing at the sitemap', async () => {
    const plugin = sitemap({ siteUrl: 'https://example.com', robots: true })
    const { buildComplete } = register(plugin)
    const context = tempBuildContext(manifest)
    await buildComplete[0](context)

    const robotsBody = readFileSync(path.join(context.outDir, 'assets', 'robots.txt'), 'utf8')
    assert.match(robotsBody, /Sitemap: https:\/\/example\.com\/sitemap\.xml/)
  })

  it('encodes URLs, includes additional paths, and shards at the protocol URL limit', async () => {
    const routes = Array.from({ length: 50_001 }, (_, index) => ({
      path: `/catalog/${index}`,
      kind: 'page',
    }))
    routes.push({ path: '/ชาไทย & coffee', kind: 'page' })
    const plugin = sitemap({
      siteUrl: 'https://example.com',
      additionalPaths: ['/limited edition'],
    })
    const { buildComplete } = register(plugin)
    const context = tempBuildContext({ routes })
    await buildComplete[0](context)

    const index = readFileSync(path.join(context.outDir, 'assets', 'sitemap.xml'), 'utf8')
    const first = readFileSync(path.join(context.outDir, 'assets', 'sitemap-0.xml'), 'utf8')
    const second = readFileSync(path.join(context.outDir, 'assets', 'sitemap-1.xml'), 'utf8')
    assert.match(index, /<sitemapindex/)
    assert.equal(first.match(/<url>/g)?.length, 50_000)
    assert.equal(second.match(/<url>/g)?.length, 3)
    assert.match(second, /%E0%B8%8A%E0%B8%B2%E0%B9%84%E0%B8%97%E0%B8%A2%20%26%20coffee/)
    assert.match(second, /limited%20edition/)
  })

  it('renders Next-style metadata entries with extension namespaces', async () => {
    const plugin = sitemap({
      siteUrl: 'https://example.com',
      defaults: {
        lastModified: new Date('2026-07-29T04:30:00.000Z'),
        changeFrequency: 'weekly',
        priority: 0.5,
      },
      entries: [
        {
          url: '/about',
          changeFrequency: 'monthly',
          priority: 0.8,
          alternates: { languages: { th: 'https://example.com/th/about' } },
          images: ['https://cdn.example.com/about.jpg'],
          videos: [
            {
              title: 'About & production',
              thumbnail_loc: 'https://cdn.example.com/thumb.jpg',
              description: 'A <rich> sitemap',
              duration: 120,
              family_friendly: 'yes',
              tag: ['framework', 'sitemap'],
            },
          ],
        },
      ],
    })
    const { buildComplete } = register(plugin)
    const context = tempBuildContext(manifest)
    await buildComplete[0](context)

    const xml = readFileSync(path.join(context.outDir, 'assets', 'sitemap.xml'), 'utf8')
    assert.match(xml, /xmlns:xhtml="http:\/\/www\.w3\.org\/1999\/xhtml"/)
    assert.match(xml, /xmlns:image="http:\/\/www\.google\.com\/schemas\/sitemap-image\/1\.1"/)
    assert.match(xml, /xmlns:video="http:\/\/www\.google\.com\/schemas\/sitemap-video\/1\.1"/)
    assert.match(xml, /<lastmod>2026-07-29T04:30:00\.000Z<\/lastmod>/)
    assert.match(xml, /<changefreq>monthly<\/changefreq>/)
    assert.match(xml, /<priority>0\.8<\/priority>/)
    assert.match(xml, /<video:title>About &amp; production<\/video:title>/)
    assert.match(xml, /<video:description>A &lt;rich&gt; sitemap<\/video:description>/)
    assert.match(xml, /  <url>\n    <loc>https:\/\/example\.com\/about<\/loc>/)
  })

  it('rejects invalid rich sitemap metadata before writing an asset', async () => {
    const invalidOptions = [
      { entries: [{ url: '/about', lastModified: 'yesterday' }] },
      { entries: [{ url: 'https://other.example/about' }] },
      { entries: [{ url: '/about', priority: 1.1 }] },
      { entries: [{ url: '/about', images: ['https://cdn.example.com/image.jpg#fragment'] }] },
      {
        entries: [
          {
            url: '/about',
            videos: [
              {
                title: 'Invalid video',
                thumbnail_loc: 'https://cdn.example.com/thumb.jpg',
                description: 'Invalid duration',
                duration: 0,
              },
            ],
          },
        ],
      },
    ]

    for (const options of invalidOptions) {
      const { buildComplete } = register(sitemap({ siteUrl: 'https://example.com', ...options }))
      await assert.rejects(async () => buildComplete[0](tempBuildContext(manifest)), TypeError)
    }
  })

  it('falls back to the committed route manifest when the build summary has no route list', async () => {
    const { buildComplete } = register(sitemap({ siteUrl: 'https://example.com' }))
    const context = tempBuildContext({ routes: 17 })
    writeFileSync(path.join(context.outDir, 'manifest.json'), JSON.stringify(manifest))
    await buildComplete[0](context)

    const xml = readFileSync(path.join(context.outDir, 'assets', 'sitemap.xml'), 'utf8')
    assert.match(xml, /<loc>https:\/\/example\.com\/about<\/loc>/)
  })

  it('rejects relative site URLs', () => {
    assert.throws(() => sitemap({ siteUrl: 'example.com' }), TypeError)
  })
})

describe('robots()', () => {
  it('writes user-agent blocks and a sitemap reference', async () => {
    const plugin = robots({
      rules: [{ userAgent: 'GoogleBot', allow: ['/'], disallow: ['/admin'] }],
      sitemap: 'https://example.com/sitemap.xml',
    })
    const { buildComplete } = register(plugin)
    const context = tempBuildContext({ routes: [] })
    await buildComplete[0](context)

    const body = readFileSync(path.join(context.outDir, 'assets', 'robots.txt'), 'utf8')
    assert.match(body, /User-agent: GoogleBot/)
    assert.match(body, /Allow: \//)
    assert.match(body, /Disallow: \/admin/)
    assert.match(body, /Sitemap: https:\/\/example\.com\/sitemap\.xml/)
  })

  it('supports Next-style scalar or array fields and rejects record injection', async () => {
    const { buildComplete } = register(
      robots({
        rules: {
          userAgent: ['Googlebot', 'Bingbot'],
          allow: '/',
          disallow: ['/private/', '/drafts/'],
          crawlDelay: 5,
        },
        sitemap: ['https://example.com/sitemap.xml', 'https://example.com/news-sitemap.xml'],
        host: 'https://example.com',
      }),
    )
    const context = tempBuildContext({ routes: [] })
    await buildComplete[0](context)
    const body = readFileSync(path.join(context.outDir, 'assets', 'robots.txt'), 'utf8')
    assert.match(body, /User-agent: Googlebot/)
    assert.match(body, /User-agent: Bingbot/)
    assert.match(body, /Crawl-delay: 5/)
    assert.equal(body.match(/Sitemap:/g)?.length, 2)
    assert.match(body, /Host: https:\/\/example\.com/)

    const invalid = register(robots({ rules: { userAgent: 'Bot\nDisallow', disallow: '/' } }))
    assert.throws(() => invalid.buildComplete[0](tempBuildContext({ routes: [] })), TypeError)
  })

  it('allows everything by default', async () => {
    const { buildComplete } = register(robots())
    const context = tempBuildContext({ routes: [] })
    await buildComplete[0](context)
    const body = readFileSync(path.join(context.outDir, 'assets', 'robots.txt'), 'utf8')
    assert.match(body, /User-agent: \*\nAllow: \//)
  })

  it('controls OpenAI search discovery independently from training', async () => {
    const { buildComplete } = register(robots({ openAi: { search: true, training: false } }))
    const context = tempBuildContext({ routes: [] })
    await buildComplete[0](context)
    const body = readFileSync(path.join(context.outDir, 'assets', 'robots.txt'), 'utf8')
    assert.match(body, /User-agent: OAI-SearchBot\nAllow: \//)
    assert.match(body, /User-agent: GPTBot\nDisallow: \//)
  })

  it('rejects ambiguous duplicate OpenAI crawler policies', () => {
    assert.throws(
      () =>
        robots({
          rules: [{ userAgent: 'oai-searchbot', disallow: ['/private'] }],
          openAi: { search: true },
        }),
      /configured by both rules and openAi\.search/,
    )
  })
})

describe('pwa()', () => {
  it('serves development artifacts and injects HTML once', async () => {
    const { middleware } = register(
      pwa({ name: 'Example', routes: ['/app/*'], offlineFallback: '/offline' }),
    )
    const plugin = middleware[0]
    const manifest = await plugin.onRequest(request('/manifest.webmanifest'))
    assert.equal(manifest.headers.get('content-type'), 'application/manifest+json; charset=utf-8')
    assert.equal((await manifest.json()).name, 'Example')

    const sw = await plugin.onRequest(request('/sw.js'))
    assert.match(await sw.text(), /const OFFLINE_FALLBACK = "\/offline"/)
    const htmlResponse = await plugin.onResponse(
      request('/app/home'),
      new Response('<html><head></head><body>App</body></html>', {
        headers: { 'content-type': 'text/html; charset=utf-8', 'content-length': '44' },
      }),
    )
    const html = await htmlResponse.text()
    assert.match(html, /rel="manifest"/)
    assert.match(html, /pwa-register\.js/)
    assert.equal(htmlResponse.headers.has('content-length'), false)

    const second = await plugin.onResponse(
      request('/app/home'),
      new Response(html, { headers: { 'content-type': 'text/html' } }),
    )
    assert.equal(second, undefined)
  })

  it('injects a path containing $-substitution characters literally', async () => {
    // `String.replace` reads `$&`, `` $` ``, `$'`, and `$1` out of a
    // *replacement string*. The injected tags carry a configured path through
    // `escapeHtmlAttribute`, which escapes `&` and so cannot neutralize a `$`,
    // so `$&` used to substitute the matched `</head>` into the tag's own href
    // and emit a second one.
    const { middleware } = register(
      pwa({ name: 'Example', manifestPath: '/manifest$&.webmanifest', routes: ['*'] }),
    )
    const htmlResponse = await middleware[0].onResponse(
      request('/'),
      new Response('<html><head></head><body>App</body></html>', {
        headers: { 'content-type': 'text/html; charset=utf-8' },
      }),
    )
    const html = await htmlResponse.text()

    assert.match(html, /href="\/manifest\$&amp;\.webmanifest"/)
    assert.equal(html.match(/<\/head>/g).length, 1, `exactly one </head>:\n${html}`)
    assert.equal(html.match(/<\/body>/g).length, 1, `exactly one </body>:\n${html}`)
  })

  it('writes PWA files and patches matching prerendered pages', async () => {
    const { buildComplete } = register(pwa({ name: 'Example', routes: ['/docs', '/docs/*'] }))
    const context = tempBuildContext({ routes: [] })
    mkdirSync(path.join(context.outDir, 'prerender', 'docs'), { recursive: true })
    mkdirSync(path.join(context.outDir, 'prerender', 'private'), { recursive: true })
    writeFileSync(
      path.join(context.outDir, 'prerender', 'docs', 'index.html'),
      '<html><head></head><body>Docs</body></html>',
    )
    writeFileSync(
      path.join(context.outDir, 'prerender', 'private', 'index.html'),
      '<html><head></head><body>Private</body></html>',
    )
    await buildComplete[0](context)

    assert.equal(
      JSON.parse(readFileSync(path.join(context.outDir, 'assets', 'manifest.webmanifest'))).name,
      'Example',
    )
    assert.match(
      readFileSync(path.join(context.outDir, 'assets', 'sw.js'), 'utf8'),
      /const CACHE = "ruvyxa-pwa-[0-9a-f]{12}-[0-9a-f]{12}"/,
    )
    assert.match(
      readFileSync(path.join(context.outDir, 'prerender', 'docs', 'index.html'), 'utf8'),
      /data-ruvyxa-pwa/,
    )
    assert.doesNotMatch(
      readFileSync(path.join(context.outDir, 'prerender', 'private', 'index.html'), 'utf8'),
      /data-ruvyxa-pwa/,
    )
  })

  it('rejects public path traversal', () => {
    assert.throws(() => pwa({ name: 'Bad', serviceWorkerPath: '/../sw.js' }), TypeError)
    assert.throws(
      () => pwa({ name: 'Bad', manifestPath: '//cdn.example/manifest.json' }),
      TypeError,
    )
    assert.throws(() => pwa({ name: 'Bad', serviceWorkerPath: '/%2e%2e/sw.js' }), TypeError)
    assert.throws(() => pwa({ name: 'Bad', serviceWorkerPath: '/%zz/sw.js' }), TypeError)
  })

  it('rejects colliding or non-file artifact paths', () => {
    assert.throws(
      () => pwa({ name: 'Bad', manifestPath: '/same', serviceWorkerPath: '/same' }),
      /must be distinct/,
    )
    assert.throws(() => pwa({ name: 'Bad', registerPath: '/' }), /must identify a file/)
  })

  it('rejects a scope the worker cannot claim without Service-Worker-Allowed', () => {
    // The header that widens a worker's scope is emitted from this plugin's own
    // request handler and from nowhere else — no adapter, no platform config,
    // and no static handler reproduces it. A build writes `sw.js` as a plain
    // public asset, so a CDN-served deployment serves it bare and the browser
    // refuses the registration with `SecurityError`. Rejecting the combination
    // here moves that from a production-only failure to a config-time one.
    assert.throws(() => pwa({ name: 'X', serviceWorkerPath: '/assets/sw.js', scope: '/' }), /scope/)
    assert.throws(
      () => pwa({ name: 'X', serviceWorkerPath: '/assets/sw.js', scope: '/other/' }),
      /scope/,
    )
    // `scope` defaults to `/`, so moving the worker without narrowing the scope
    // is the same rejection rather than a quiet one.
    assert.throws(() => pwa({ name: 'X', serviceWorkerPath: '/assets/sw.js' }), /scope/)
    // A worker at the root claims every scope by default, which is why the
    // default configuration has never hit this.
    assert.doesNotThrow(() => pwa({ name: 'X' }))
    assert.doesNotThrow(() =>
      pwa({ name: 'X', serviceWorkerPath: '/assets/sw.js', scope: '/assets/' }),
    )
    assert.doesNotThrow(() =>
      pwa({ name: 'X', serviceWorkerPath: '/assets/sw.js', scope: '/assets/app/' }),
    )
    // A sibling directory that shares a name prefix is not inside the worker's
    // own directory; a plain `startsWith` would accept it.
    assert.throws(
      () => pwa({ name: 'X', serviceWorkerPath: '/assets/sw.js', scope: '/assets-2/' }),
      /scope/,
    )
  })

  it('isolates caches by scope and waits for runtime cache writes', async () => {
    const app = register(pwa({ name: 'App', scope: '/app/' })).middleware[0]
    const admin = register(pwa({ name: 'Admin', scope: '/admin/' })).middleware[0]
    const appSource = await (await app.onRequest(request('/sw.js'))).text()
    const adminSource = await (await admin.onRequest(request('/sw.js'))).text()
    const appCache = appSource.match(/const CACHE = "([^"]+)"/)[1]
    const adminCache = adminSource.match(/const CACHE = "([^"]+)"/)[1]

    assert.notEqual(appCache, adminCache)
    assert.match(appSource, /name\.startsWith\(CACHE_PREFIX\)/)
    assert.doesNotMatch(appSource, /name\.startsWith\('ruvyxa-pwa-'\)/)
    assert.match(appSource, /event\.waitUntil\(cacheWrite\)/)
    assert.match(appSource, /\.catch\(\(\) => undefined\)/)
  })

  /**
   * Trap #4: cache identity is derived, never stamped. The worker is
   * cache-first with no revalidation, and `activate` drops only caches whose
   * name *differs*, so a fixed `-v1` suffix meant the install-time copy of an
   * unfingerprinted `/logo.png` or `/vendor.js` was served forever.
   *
   * Derived from what the build *emitted*, not from the build manifest. The
   * manifest carries `createdAtUnix`, so hashing it moved the name on every
   * build whether or not anything changed — which is the same defect wearing
   * the opposite mask: it broke `pnpm verify:reproducible`, and it made every
   * visitor re-download a site that was byte-identical to the one they had.
   *
   * Both directions are asserted here, because either alone is satisfied by a
   * bug: a name that never moves passes the second, and a name built from a
   * clock passes the first.
   */
  it('derives the cache name from what the build emitted', async () => {
    const registered = register(pwa({ name: 'Example' }))

    const buildWith = async (assets) => {
      const context = tempBuildContext({ routes: 2, createdAtUnix: Date.now() })
      mkdirSync(path.join(context.outDir, 'client'), { recursive: true })
      for (const [name, contents] of Object.entries(assets)) {
        writeFileSync(path.join(context.outDir, 'client', name), contents)
      }
      await registered.buildComplete[0](context)
      return {
        context,
        cache: cacheNameOf(readFileSync(path.join(context.outDir, 'assets', 'sw.js'), 'utf8')),
      }
    }

    const first = await buildWith({ 'app.js': 'export default 1' })
    const same = await buildWith({ 'app.js': 'export default 1' })
    const changed = await buildWith({ 'app.js': 'export default 2' })

    assert.equal(
      first.cache,
      same.cache,
      'two builds that emitted the same bytes must claim the same cache, or every ' +
        'visitor re-downloads an unchanged site and verify:reproducible fails',
    )
    assert.notEqual(
      first.cache,
      changed.cache,
      'a build that emitted different bytes must claim a different cache, or a ' +
        'cache-first worker serves the install-time copy of a changed asset forever',
    )

    // Both must keep the scope-derived prefix, or `activate` cannot recognise
    // the previous build's cache as one of ours and never deletes it.
    const source = readFileSync(path.join(changed.context.outDir, 'assets', 'sw.js'), 'utf8')
    const prefix = source.match(/const CACHE_PREFIX = "([^"]+)"/)[1]
    assert.ok(first.cache.startsWith(prefix), `${first.cache} does not start with ${prefix}`)
    assert.ok(changed.cache.startsWith(prefix), `${changed.cache} does not start with ${prefix}`)

    // The dev handler serves `/sw.js` from the same value the build wrote, or
    // the served worker and the deployed worker claim different caches.
    const served = await registered.middleware[0].onRequest(request('/sw.js'))
    assert.equal(cacheNameOf(await served.text()), changed.cache)
  })

  it('keeps version as an override of the derived cache name', async () => {
    const registered = register(pwa({ name: 'Example', version: 'pinned' }))
    const first = tempBuildContext({ routes: 2, createdAtUnix: 1 })
    const second = tempBuildContext({ routes: 2, createdAtUnix: 2 })
    await registered.buildComplete[0](first)
    await registered.buildComplete[0](second)

    const firstCache = cacheNameOf(readFileSync(path.join(first.outDir, 'assets', 'sw.js'), 'utf8'))
    const secondCache = cacheNameOf(
      readFileSync(path.join(second.outDir, 'assets', 'sw.js'), 'utf8'),
    )
    assert.equal(firstCache, secondCache)
    assert.match(secondCache, /^ruvyxa-pwa-[0-9a-f]{12}-pinned$/)
  })
})

describe('feed()', () => {
  it('writes RSS from an async content loader with escaped metadata', async () => {
    const { buildComplete } = register(
      feed({
        siteUrl: 'https://example.com',
        title: 'News & Notes',
        description: 'Latest posts',
        async items() {
          return [
            {
              title: 'Ruvyxa <1.0>',
              url: '/blog/launch',
              publishedAt: '2026-07-22T00:00:00Z',
              content: '<p>Fast ]]> launch</p>',
            },
          ]
        },
      }),
    )
    const context = tempBuildContext({ routes: [] })
    await buildComplete[0](context)
    const xml = readFileSync(path.join(context.outDir, 'assets', 'rss.xml'), 'utf8')
    assert.match(xml, /<title>News &amp; Notes<\/title>/)
    assert.match(xml, /<link>https:\/\/example\.com\/blog\/launch<\/link>/)
    assert.match(xml, /Wed, 22 Jul 2026 00:00:00 GMT/)
    assert.match(xml, /xmlns:content=/)
  })
})

describe('searchIndex()', () => {
  it('writes a stable locale-aware inverted index', async () => {
    const { buildComplete } = register(
      searchIndex({
        locale: 'th',
        stopWords: ['และ'],
        documents: [
          { id: 'b', title: 'ระบบปลั๊กอิน', url: '/plugins', text: 'รวดเร็ว และ เสถียร' },
          { id: 'a', title: 'เริ่มต้น', url: '/', text: 'Ruvyxa รวดเร็ว' },
        ],
      }),
    )
    const context = tempBuildContext({ routes: [] })
    await buildComplete[0](context)
    const index = JSON.parse(
      readFileSync(path.join(context.outDir, 'assets', 'search-index.json'), 'utf8'),
    )
    assert.deepEqual(
      index.documents.map((document) => document.id),
      ['a', 'b'],
    )
    assert.deepEqual(index.terms.รวดเร็ว, ['a', 'b'])
    assert.equal(index.terms.และ, undefined)
  })

  it('uses runtime-independent code-unit ordering for serialized output', async () => {
    const { buildComplete } = register(
      searchIndex({
        locale: 'en',
        documents: [
          { id: 'ä', title: 'Äther', url: '/a', text: 'zulu' },
          { id: 'z', title: 'Zulu', url: '/z', text: 'alpha' },
          { id: 'A', title: 'Alpha', url: '/capital-a', text: 'äther' },
        ],
      }),
    )
    const context = tempBuildContext({ routes: [] })
    await buildComplete[0](context)
    const index = JSON.parse(
      readFileSync(path.join(context.outDir, 'assets', 'search-index.json'), 'utf8'),
    )

    assert.deepEqual(
      index.documents.map((document) => document.id),
      ['A', 'z', 'ä'],
    )
    assert.deepEqual(Object.keys(index.terms), ['alpha', 'zulu', 'äther'])
    assert.deepEqual(index.terms.äther, ['A', 'ä'])
  })

  it('rejects duplicate document IDs at build time', async () => {
    const { buildComplete } = register(
      searchIndex({
        documents: [
          { id: 'same', title: 'One', url: '/one', text: 'one' },
          { id: 'same', title: 'Two', url: '/two', text: 'two' },
        ],
      }),
    )
    await assert.rejects(() => buildComplete[0](tempBuildContext({ routes: [] })), /duplicate id/)
  })

  // The index is a build artifact, so the same source has to emit the same
  // bytes everywhere. Both ingredients are locale-sensitive -- `Intl.Segmenter`
  // decides where words begin and case folding decides which key a document is
  // filed under -- and both used to receive `options.locale` straight through.
  // With `locale` unset that reached ICU as `undefined`, which does not mean
  // "locale-independent": it means "this machine's locale". A build on the
  // machine this framework is developed on segments as `th-TH`, the same build
  // on GitHub's runners as `en-US`, and on a Turkish contributor's laptop
  // `Istanbul` folds to `ıstanbul` and lands under a different term entirely.
  // `scripts/verify-reproducible.mjs` builds twice on one host, so it could
  // never see this.
  describe('locale is a function of the project, never of the build host', () => {
    const documents = [
      { id: 'a', title: 'Istanbul Airport', url: '/a', text: 'สวัสดีชาวโลก I love IT' },
      { id: 'b', title: 'ประเทศไทย', url: '/b', text: 'Istanbul again' },
    ]

    async function indexFor(options) {
      const { buildComplete } = register(searchIndex({ documents, ...options }))
      const context = tempBuildContext({ routes: [] })
      await buildComplete[0](context)
      return readFileSync(path.join(context.outDir, 'assets', 'search-index.json'), 'utf8')
    }

    it('falls back to a fixed locale rather than the host default', async () => {
      assert.equal(await indexFor({}), await indexFor({ locale: DEFAULT_INDEX_LOCALE }))
    })

    it('never asks Intl for a locale it was not given', async () => {
      const requested = await recordRequestedLocales(() => indexFor({}))
      assert.ok(requested.length > 0, 'the build should have consulted Intl at all')
      assert.deepEqual(
        requested.filter(([, locale]) => locale === undefined),
        [],
        'a call with no locale resolves to the build host and must not exist',
      )
    })

    it('says so rather than falling back silently', async () => {
      const { diagnostics } = register(searchIndex({ documents }))
      assert.deepEqual(
        diagnostics.map(({ level, code }) => [level, code]),
        [['warning', 'RUV2207']],
      )
      assert.match(diagnostics[0].message, /searchIndex: locale is not set/)
    })

    it('stays quiet once a project names one', async () => {
      const { diagnostics } = register(searchIndex({ documents, locale: 'th' }))
      assert.deepEqual(diagnostics, [])
    })

    it('rejects a malformed locale instead of reaching Intl with it', () => {
      assert.throws(() => searchIndex({ documents, locale: 'invalid_locale' }), /BCP 47/)
    })
  })
})

describe('contentEngine()', () => {
  function contentProject() {
    const root = mkdtempSync(path.join(tmpdir(), 'ruvyxa-content-engine-'))
    tempDirs.push(root)
    const writePage = (relative, source) => {
      const file = path.join(root, 'app', relative)
      mkdirSync(path.dirname(file), { recursive: true })
      writeFileSync(file, source)
    }
    writePage(
      '(marketing)/blog/launch/page.mdx',
      `---
title: Launch Day
description: The fast Ruvyxa launch.
publishedAt: 2026-07-22
updatedAt: 2026-07-23T10:30:00Z
author: Ada
tags: [release, framework]
answers:
  - question: Does Ruvyxa support citeable answers?
    answer: Yes. Answer data is explicit and links back to the canonical page.
    sources:
      - name: Ruvyxa rendering guide
        url: /docs/rendering
campaign:
  featured: true
---
# {frontmatter.title}

Ruvyxa ships **fast content** for everyone.
`,
    )
    writePage('about/page.md', '# About Ruvyxa\n\nA framework built for clear delivery.')
    writePage('blog/draft/page.md', '---\ndraft: true\n---\n# Secret roadmap')
    writePage('_private/page.md', '# Private notes')
    writePage('[slug]/page.md', '# Dynamic content')
    return root
  }

  const options = {
    siteUrl: 'https://example.com',
    title: 'Example content',
    description: 'News from Example.',
    locale: 'en',
  }

  it('derives live content, search, RSS, and sitemap artifacts from one source', async () => {
    const root = contentProject()
    const registered = register(contentEngine(options), 'development')
    assert.deepEqual(registered.middleware[0].routes, [
      '/content.json',
      '/search-index.json',
      '/rss.xml',
      '/sitemap.xml',
      '/llms.txt',
    ])

    const context = { plugin: 'ruvyxa:content-engine', root }
    const manifestResponse = await registered.middleware[0].onRequest(
      request('/content.json'),
      context,
    )
    const manifestBody = await manifestResponse.text()
    const manifest = JSON.parse(manifestBody)
    assert.deepEqual(
      manifest.entries.map((entry) => entry.route),
      ['/blog/launch', '/about'],
    )
    assert.equal(manifest.entries[0].url, 'https://example.com/blog/launch')
    assert.equal(manifest.entries[0].publishedAt, '2026-07-22T00:00:00.000Z')
    assert.equal(manifest.entries[0].frontmatter.campaign.featured, true)
    assert.deepEqual(manifest.entries[0].tags, ['framework', 'release'])
    assert.deepEqual(manifest.entries[0].answers, [
      {
        question: 'Does Ruvyxa support citeable answers?',
        answer: 'Yes. Answer data is explicit and links back to the canonical page.',
        sources: [{ name: 'Ruvyxa rendering guide', url: 'https://example.com/docs/rendering' }],
      },
    ])
    assert.equal(manifest.entries[1].title, 'About Ruvyxa')
    assert.equal(
      manifest.entries[1].description,
      'About Ruvyxa A framework built for clear delivery.',
    )
    assert.equal(
      manifest.entries.some((entry) => entry.route.includes('draft')),
      false,
    )

    const searchResponse = await registered.middleware[0].onRequest(
      request('/search-index.json'),
      context,
    )
    const searchBody = await searchResponse.text()
    const search = JSON.parse(searchBody)
    assert.deepEqual(search.terms.framework, ['/about', '/blog/launch'])
    assert.deepEqual(search.terms.content, ['/blog/launch'])

    const feedResponse = await registered.middleware[0].onRequest(request('/rss.xml'), context)
    const feedBody = await feedResponse.text()
    assert.match(feedBody, /<title>Launch Day<\/title>/)
    assert.match(feedBody, /<author>Ada<\/author>/)
    assert.doesNotMatch(feedBody, /Secret roadmap/)

    const sitemapResponse = await registered.middleware[0].onRequest(
      request('/sitemap.xml'),
      context,
    )
    const sitemapBody = await sitemapResponse.text()
    assert.match(sitemapBody, /https:\/\/example\.com\/blog\/launch/)
    assert.match(sitemapBody, /<lastmod>2026-07-23T10:30:00\.000Z<\/lastmod>/)
    assert.doesNotMatch(sitemapBody, /\[slug\]|_private|draft/)

    const llmsResponse = await registered.middleware[0].onRequest(request('/llms.txt'), context)
    assert.equal(llmsResponse.headers.get('content-type'), 'text/plain; charset=utf-8')
    const llmsBody = await llmsResponse.text()
    assert.match(llmsBody, /^# Example content\n\n> News from Example\./)
    assert.match(
      llmsBody,
      /\[Launch Day\]\(<https:\/\/example\.com\/blog\/launch>\): The fast Ruvyxa launch\./,
    )
    assert.match(llmsBody, /Does Ruvyxa support citeable answers\? — Yes\./)

    const buildContext = tempBuildContext({ routes: [] })
    buildContext.root = root
    await registered.buildComplete[0](buildContext)
    for (const [name, expected] of [
      ['content.json', manifestBody],
      ['search-index.json', searchBody],
      ['rss.xml', feedBody],
      ['sitemap.xml', sitemapBody],
      ['llms.txt', llmsBody],
    ]) {
      assert.equal(readFileSync(path.join(buildContext.outDir, 'assets', name), 'utf8'), expected)
    }
  })

  it('handles HEAD safely and lets unsupported methods or missing source trees continue', async () => {
    const root = contentProject()
    const { middleware } = register(contentEngine(options), 'development')
    const context = { plugin: 'ruvyxa:content-engine', root }
    const head = await middleware[0].onRequest(
      request('/content.json', { method: 'HEAD' }),
      context,
    )
    assert.equal(await head.text(), '')
    assert.equal(
      await middleware[0].onRequest(request('/content.json', { method: 'POST' }), context),
      undefined,
    )
    assert.equal(
      await middleware[0].onRequest(request('/content.json'), {
        ...context,
        root: path.join(root, 'missing'),
      }),
      undefined,
    )
  })

  it('invalidates live artifacts when a content page changes', async () => {
    const root = contentProject()
    const { middleware } = register(contentEngine(options), 'development')
    const context = { plugin: 'ruvyxa:content-engine', root }
    const before = await middleware[0].onRequest(request('/content.json'), context)
    assert.match(await before.text(), /A framework built for clear delivery/)

    writeFileSync(
      path.join(root, 'app', 'about', 'page.md'),
      '# About Ruvyxa\n\nUpdated content appears without restarting the development server.',
    )
    const after = await middleware[0].onRequest(request('/content.json'), context)
    assert.match(await after.text(), /Updated content appears without restarting/)
  })

  it('rejects unsafe configuration and invalid content metadata', async () => {
    assert.throws(() => contentEngine({ ...options, appDir: '../content' }), /project root/)
    assert.throws(
      () => contentEngine({ ...options, feedPath: '/same', sitemapPath: '/same' }),
      /must be distinct/,
    )
    assert.throws(() => contentEngine({ ...options, locale: 'invalid_locale' }), /BCP 47/)

    const root = mkdtempSync(path.join(tmpdir(), 'ruvyxa-content-engine-invalid-'))
    tempDirs.push(root)
    const file = path.join(root, 'app', 'bad', 'page.md')
    mkdirSync(path.dirname(file), { recursive: true })
    writeFileSync(file, '---\ntags: release\n---\n# Bad metadata')
    const { buildComplete } = register(contentEngine(options))
    const context = tempBuildContext({ routes: [] })
    context.root = root
    assert.throws(() => buildComplete[0](context), /frontmatter\.tags/)

    writeFileSync(file, '---\npublishedAt: 2026-02-31\n---\n# Invalid date')
    assert.throws(() => buildComplete[0](context), /ISO date string/)

    writeFileSync(file, '---\nnull\n---\n# Invalid mapping')
    assert.throws(() => buildComplete[0](context), /YAML mapping/)

    writeFileSync(file, '---\nanswers:\n  - question: Missing answer\n---\n# Invalid answer')
    assert.throws(() => buildComplete[0](context), /answers\[0\]\.answer/)

    writeFileSync(
      file,
      '---\nanswers:\n  - question: Bad source\n    answer: Explicit\n    sources:\n      - name: Local\n        url: javascript:alert(1)\n---\n# Invalid source',
    )
    assert.throws(() => buildComplete[0](context), /must use http\(s\)/)
  })

  it('escapes markdown syntax in llms.txt titles and descriptions', async () => {
    const root = mkdtempSync(path.join(tmpdir(), 'ruvyxa-content-engine-'))
    tempDirs.push(root)
    const file = path.join(root, 'app', 'notes', 'page.md')
    mkdirSync(path.dirname(file), { recursive: true })
    writeFileSync(
      file,
      "---\ntitle: 'Notes [draft]'\ndescription: 'See [the guide](/docs) for C:\\paths.'\n---\n# Notes\n",
    )
    const registered = register(contentEngine(options), 'development')
    const context = { plugin: 'ruvyxa:content-engine', root }
    const response = await registered.middleware[0].onRequest(request('/llms.txt'), context)
    const body = await response.text()
    assert.ok(
      body.includes(
        '- [Notes \\[draft\\]](<https://example.com/notes>): See \\[the guide\\](/docs) for C:\\\\paths.',
      ),
      `llms.txt entry was not escaped: ${body}`,
    )
  })

  it('can disable the llms.txt artifact', async () => {
    const root = contentProject()
    const registered = register(contentEngine({ ...options, llmsPath: false }), 'development')
    assert.doesNotMatch(registered.middleware[0].routes.join(','), /llms\.txt/)
    const context = tempBuildContext({ routes: [] })
    context.root = root
    await registered.buildComplete[0](context)
    assert.equal(existsSync(path.join(context.outDir, 'assets', 'llms.txt')), false)
  })

  it('leaves the live artifacts to the build in production', () => {
    // Mirror of the `feed()` and `searchIndex()` environment cases. The live
    // handler recursively walks the content tree and stats every page *to
    // compute the key its own cache is checked against*, so the syscalls are
    // per request whether or not the cache hits — on exactly the paths crawlers
    // poll, behind `cache-control: no-cache`.
    assert.deepEqual(register(contentEngine(options), 'production').middleware, [])
    assert.equal(register(contentEngine(options), 'development').middleware.length, 1)
    // The build half is unconditional: `assets/` is written either way.
    assert.equal(register(contentEngine(options), 'production').buildComplete.length, 1)
  })
})

describe('openApi()', () => {
  const options = {
    info: { title: 'Example API', version: '1.0.0' },
    operations: [
      { method: 'GET', path: '/api/users', operationId: 'listUsers', summary: 'List users' },
      {
        method: 'post',
        path: '/api/users',
        operationId: 'createUser',
        responses: { 201: { description: 'Created' } },
      },
    ],
  }

  it('serves the document in development and writes it after build', async () => {
    const registered = register(openApi(options))
    const response = await registered.middleware[0].onRequest(request('/openapi.json'))
    const document = await response.json()
    assert.equal(document.openapi, '3.1.0')
    assert.equal(document.paths['/api/users'].get.operationId, 'listUsers')
    assert.equal(document.paths['/api/users'].post.responses['201'].description, 'Created')

    const context = tempBuildContext({ routes: [] })
    mkdirSync(path.join(context.outDir, 'assets'), { recursive: true })
    writeFileSync(path.join(context.outDir, 'assets', 'openapi.json'), 'stale')
    await registered.buildComplete[0](context)
    assert.equal(
      JSON.parse(readFileSync(path.join(context.outDir, 'assets', 'openapi.json'))).info.title,
      'Example API',
    )
    assert.deepEqual(
      readdirSync(path.join(context.outDir, 'assets')).filter((name) => name.includes('.tmp-')),
      [],
    )
  })

  it('rejects duplicate method/path pairs and operation IDs', () => {
    assert.throws(
      () =>
        openApi({
          info: { title: 'API', version: '1' },
          operations: [
            { method: 'get', path: '/x' },
            { method: 'GET', path: '/x' },
          ],
        }),
      /duplicate GET \/x/,
    )
    assert.throws(
      () =>
        openApi({
          info: { title: 'API', version: '1' },
          operations: [
            { method: 'get', path: '/x', operationId: 'same' },
            { method: 'post', path: '/x', operationId: 'same' },
          ],
        }),
      /duplicate operationId/,
    )
  })
})

describe('bundleBudget()', () => {
  function contextWithClientFiles(files) {
    const context = tempBuildContext({ routes: [] })
    for (const [name, bytes] of Object.entries(files)) {
      const file = path.join(context.outDir, 'client', name)
      mkdirSync(path.dirname(file), { recursive: true })
      writeFileSync(file, 'x'.repeat(bytes))
    }
    return context
  }

  it('passes when every file fits the budget', async () => {
    const { buildComplete } = register(bundleBudget({ maxChunkKb: 1, maxTotalKb: 2 }))
    const context = contextWithClientFiles({ 'app.js': 500, 'chunks/route.js': 600 })
    await buildComplete[0](context)
  })

  it('fails the build when a chunk or the total exceeds the budget', async () => {
    const { buildComplete } = register(bundleBudget({ maxChunkKb: 1, maxTotalKb: 1 }))
    const context = contextWithClientFiles({ 'app.js': 2048, 'style.css': 5000 })
    await assert.rejects(
      async () => buildComplete[0](context),
      (error) => {
        assert.match(error.message, /bundle budget exceeded/)
        assert.match(error.message, /app\.js is 2\.0 KiB \(chunk budget 1 KiB\)/)
        assert.match(error.message, /totals 2\.0 KiB \(total budget 1 KiB\)/)
        assert.doesNotMatch(error.message, /style\.css/)
        return true
      },
    )
  })

  it('treats a missing client directory as empty', async () => {
    const { buildComplete } = register(bundleBudget({ maxTotalKb: 1 }))
    await buildComplete[0](tempBuildContext({ routes: [] }))
  })

  it('rejects configurations without any budget', () => {
    assert.throws(() => bundleBudget({}), TypeError)
    assert.throws(() => bundleBudget({ maxChunkKb: -1 }), TypeError)
  })
})

describe('requireEnv()', () => {
  it('passes when every variable is set and lists all missing names', async () => {
    process.env.RUVYXA_TEST_PRESENT = 'yes'
    process.env.RUVYXA_TEST_EMPTY = ''
    try {
      const { buildComplete } = register(requireEnv(['RUVYXA_TEST_PRESENT']))
      await buildComplete[0](tempBuildContext({ routes: [] }))

      const failing = register(
        requireEnv(['RUVYXA_TEST_PRESENT', 'RUVYXA_TEST_EMPTY', 'RUVYXA_TEST_ABSENT']),
      )
      await assert.rejects(
        async () => failing.buildComplete[0](tempBuildContext({ routes: [] })),
        /missing required environment variables: RUVYXA_TEST_EMPTY, RUVYXA_TEST_ABSENT/,
      )
    } finally {
      delete process.env.RUVYXA_TEST_PRESENT
      delete process.env.RUVYXA_TEST_EMPTY
    }
  })

  it('rejects empty name lists', () => {
    assert.throws(() => requireEnv([]), TypeError)
  })
})

describe('alias()', () => {
  it('resolves exact specifiers from the project root and skips the rest', () => {
    const { resolveId } = register(alias({ '~content': 'content/index.ts' }))
    const context = { root: 'D:/app', environment: 'server' }
    assert.equal(
      resolveId[0]('~content', undefined, context),
      path.resolve('D:/app', 'content/index.ts'),
    )
    assert.equal(resolveId[0]('other', undefined, context), undefined)
  })

  it('rejects empty targets', () => {
    assert.throws(() => alias({ '~x': '' }), TypeError)
  })
})

describe('fonts()', () => {
  /** Replace global fetch with a fixture map for the duration of one call. */
  async function withFetch(responses, run) {
    const original = globalThis.fetch
    const requested = []
    globalThis.fetch = async (url) => {
      requested.push(String(url))
      const body = responses[String(url)]
      if (body === undefined) return new Response('missing', { status: 404 })
      return typeof body === 'string'
        ? new Response(body, { status: 200 })
        : new Response(body, { status: 200 })
    }
    try {
      return await run(requested)
    } finally {
      globalThis.fetch = original
    }
  }

  const sheetUrl = 'https://fonts.googleapis.com/css2?family=Inter:wght@400&display=swap'
  const fontUrl = 'https://fonts.gstatic.com/s/inter/v1/abc.woff2'
  const css = `@font-face{font-family:'Inter';src:url(${fontUrl}) format('woff2');}`

  it('declares a stylesheet link and preload in head', () => {
    const plugin = fonts({ google: [sheetUrl] })
    assert.deepEqual(
      plugin.head.map((entry) => [entry.tag, entry.attrs.rel, entry.attrs.href]),
      [
        ['link', 'preload', '/fonts/fonts.css'],
        ['link', 'stylesheet', '/fonts/fonts.css'],
      ],
    )

    const withoutPreload = fonts({ google: [sheetUrl], preload: false })
    assert.equal(withoutPreload.head.length, 1)
  })

  it('downloads the font files and rewrites the stylesheet to local paths', async () => {
    const { buildComplete } = register(fonts({ google: [sheetUrl] }))
    const context = tempBuildContext({ routes: [] })

    await withFetch({ [sheetUrl]: css, [fontUrl]: Buffer.from('woff2-bytes') }, async () => {
      await buildComplete[0](context)
    })

    const generated = readFileSync(path.join(context.outDir, 'assets/fonts/fonts.css'), 'utf8')
    assert.match(generated, /url\(\/fonts\/abc-[a-f0-9]{8}\.woff2\)/)
    // No gstatic origin survives: that request is what blocks first paint.
    assert.doesNotMatch(generated, /fonts\.gstatic\.com/)

    const fontFile = readdirSync(path.join(context.outDir, 'assets/fonts')).find((name) =>
      name.endsWith('.woff2'),
    )
    assert.ok(fontFile, 'font file was written')
    assert.equal(
      readFileSync(path.join(context.outDir, 'assets/fonts', fontFile), 'utf8'),
      'woff2-bytes',
    )
  })

  it('reports a diagnostic instead of failing the build when the network is unavailable', async () => {
    const reported = []
    const plugin = fonts({ google: [sheetUrl] })
    const buildComplete = []
    plugin.register({
      http: { onRequest() {}, onResponse() {}, route() {} },
      build: {
        onStart() {},
        onResolve() {},
        onLoad() {},
        onTransform() {},
        onComplete(hook) {
          buildComplete.push(hook)
        },
      },
      dev: { onFileChange() {} },
      diagnostics: { report: (diagnostic) => reported.push(diagnostic) },
      native: { claim() {} },
    })

    const context = tempBuildContext({ routes: [] })
    await withFetch({}, async () => {
      await buildComplete[0](context)
    })

    assert.equal(reported.length, 1)
    assert.equal(reported[0].level, 'warning')
    assert.match(reported[0].message, /could not self-host Google Fonts/)

    // The `<link rel="stylesheet">` is fixed when the plugin is constructed, so
    // it ships whether or not the download worked. Leaving the file absent
    // pointed a render-blocking request at a 404 on every page — worse than the
    // third-party round trip the plugin removes. An empty stylesheet keeps the
    // reference resolvable and the page on its fallback fonts.
    const stylesheet = path.join(context.outDir, 'assets/fonts/fonts.css')
    assert.equal(existsSync(stylesheet), true)
    assert.match(readFileSync(stylesheet, 'utf8'), /ruvyxa:fonts/)
    assert.doesNotMatch(readFileSync(stylesheet, 'utf8'), /@font-face/)
  })

  /** Swap in an arbitrary fetch implementation for the duration of one call. */
  async function withFetchImpl(impl, run) {
    const original = globalThis.fetch
    globalThis.fetch = impl
    try {
      return await run()
    } finally {
      globalThis.fetch = original
    }
  }

  it('degrades to the fallback stylesheet when a fetch never settles', async () => {
    // A fetch that fails is already covered; one that neither succeeds nor
    // fails used to hang `ruvyxa build` with no diagnostic until CI's own
    // timeout killed it, which is the outcome the fail-soft design exists to
    // avoid. The abort has to land in the same catch as any other failure.
    const { buildComplete, diagnostics } = register(fonts({ google: [sheetUrl], timeoutMs: 25 }))
    const context = tempBuildContext({ routes: [] })

    const seenSignals = []
    await withFetchImpl(
      (url, init) =>
        new Promise((_resolve, reject) => {
          seenSignals.push(init?.signal)
          init?.signal?.addEventListener('abort', () => {
            reject(init.signal.reason ?? new Error('aborted'))
          })
        }),
      async () => {
        await buildComplete[0](context)
      },
    )

    assert.equal(seenSignals.length, 1)
    assert.ok(seenSignals[0] instanceof AbortSignal, 'the fetch is given an abort signal')

    assert.equal(diagnostics.length, 1)
    assert.equal(diagnostics[0].level, 'warning')
    assert.equal(diagnostics[0].code, 'RUV2103')
    // The timeout is named so a slow-but-working connection is diagnosable
    // rather than a silent degrade to fallback fonts.
    assert.match(diagnostics[0].message, /did not respond within 25 ?ms/)

    const stylesheet = path.join(context.outDir, 'assets/fonts/fonts.css')
    assert.equal(existsSync(stylesheet), true)
    assert.match(readFileSync(stylesheet, 'utf8'), /ruvyxa:fonts/)
  })

  it('refuses a response that declares more bytes than the ceiling', async () => {
    const { buildComplete, diagnostics } = register(fonts({ google: [sheetUrl] }))
    const context = tempBuildContext({ routes: [] })

    await withFetchImpl(
      async () =>
        new Response('body', {
          status: 200,
          headers: { 'content-length': String(512 * 1024 * 1024) },
        }),
      async () => {
        await buildComplete[0](context)
      },
    )

    assert.equal(diagnostics.length, 1)
    assert.equal(diagnostics[0].code, 'RUV2103')
    assert.match(diagnostics[0].message, /bytes/)
    assert.match(
      readFileSync(path.join(context.outDir, 'assets/fonts/fonts.css'), 'utf8'),
      /ruvyxa:fonts/,
    )
  })

  it('stops reading a body that runs past the ceiling without a content-length', async () => {
    const { buildComplete, diagnostics } = register(fonts({ google: [sheetUrl], maxBytes: 4096 }))
    const context = tempBuildContext({ routes: [] })

    let chunksRead = 0
    await withFetchImpl(
      async () =>
        new Response(
          new ReadableStream({
            pull(controller) {
              chunksRead += 1
              controller.enqueue(new Uint8Array(1024))
            },
          }),
          { status: 200 },
        ),
      async () => {
        await buildComplete[0](context)
      },
    )

    assert.equal(diagnostics.length, 1)
    assert.equal(diagnostics[0].code, 'RUV2103')
    // The read stops at the ceiling instead of buffering a hostile body whole.
    assert.ok(chunksRead < 64, `read stopped early (${chunksRead} chunks)`)
    assert.match(
      readFileSync(path.join(context.outDir, 'assets/fonts/fonts.css'), 'utf8'),
      /ruvyxa:fonts/,
    )
  })

  it('bounds the font-file download as well as the stylesheet', async () => {
    const { buildComplete, diagnostics } = register(fonts({ google: [sheetUrl], timeoutMs: 25 }))
    const context = tempBuildContext({ routes: [] })

    await withFetchImpl(
      (url, init) =>
        String(url) === sheetUrl
          ? Promise.resolve(new Response(css, { status: 200 }))
          : new Promise((_resolve, reject) => {
              init?.signal?.addEventListener('abort', () => {
                reject(init.signal.reason ?? new Error('aborted'))
              })
            }),
      async () => {
        await buildComplete[0](context)
      },
    )

    assert.equal(diagnostics.length, 1)
    assert.ok(diagnostics[0].message.includes(fontUrl), 'names the font file that stalled')
    assert.match(diagnostics[0].message, /did not respond within 25 ?ms/)
  })

  it('rejects URLs that are not Google Fonts stylesheets', () => {
    assert.throws(() => fonts({ google: [] }), TypeError)
    assert.throws(() => fonts({ google: ['https://cdn.example/font.css'] }), TypeError)
    assert.throws(() => fonts({ google: [sheetUrl], publicPath: '/' }), TypeError)
    assert.throws(() => fonts({ google: [sheetUrl], timeoutMs: 0 }), /timeoutMs/)
    assert.throws(() => fonts({ google: [sheetUrl], maxBytes: -1 }), /maxBytes/)
  })
})

describe('originGuard()', () => {
  const { middleware } = register(originGuard())
  const [{ onRequest, routes }] = middleware

  /** A request carrying whatever same-origin evidence the case is about. */
  function apiPost(headers) {
    return request('/api/todos', { method: 'POST', headers })
  }

  it('scopes itself to /api/* by default', () => {
    assert.deepEqual(routes, ['/api/*'])
  })

  it('lets safe methods through without same-origin evidence', async () => {
    assert.equal(await onRequest(request('/api/todos')), undefined)
  })

  it('accepts a POST whose Origin matches the Host', async () => {
    const accepted = await onRequest(
      apiPost({ host: 'ruvyxa.local', origin: 'http://ruvyxa.local' }),
    )
    assert.equal(accepted, undefined)
  })

  it('blocks a POST from another origin', async () => {
    const blocked = await onRequest(apiPost({ host: 'ruvyxa.local', origin: 'https://evil.test' }))
    assert.equal(blocked.status, 403)
    assert.match(await blocked.text(), /Cross-origin request blocked/)
  })

  it('fails closed when neither Origin nor Fetch Metadata is present', async () => {
    const blocked = await onRequest(apiPost({ host: 'ruvyxa.local' }))
    assert.equal(blocked.status, 403)
  })

  it('accepts Sec-Fetch-Site as the substitute for a stripped Origin', async () => {
    const accepted = await onRequest(
      apiPost({ host: 'ruvyxa.local', 'sec-fetch-site': 'same-origin' }),
    )
    assert.equal(accepted, undefined)
  })

  it('falls back to the request URL when no Host header survived', async () => {
    const accepted = await onRequest(apiPost({ origin: 'http://ruvyxa.local' }))
    assert.equal(accepted, undefined)
  })

  it('accepts an explicitly allowed third-party origin', async () => {
    const { middleware: allowed } = register(
      originGuard({ allowOrigins: ['https://partner.example'] }),
    )
    const accepted = await allowed[0].onRequest(
      apiPost({ host: 'ruvyxa.local', origin: 'https://partner.example' }),
    )
    assert.equal(accepted, undefined)
  })

  it('rejects a non-4xx status and a malformed allowed origin', () => {
    assert.throws(() => originGuard({ status: 500 }), /status must be a 4xx integer/)
    assert.throws(() => originGuard({ allowOrigins: ['not-a-url'] }), /allowOrigins\[0\]/)
  })
})

describe('healthCheck()', () => {
  it('registers an exact GET/HEAD route on /health by default', () => {
    const { routes } = register(healthCheck())
    assert.equal(routes.length, 1)
    assert.equal(routes[0].path, '/health')
    assert.deepEqual(routes[0].method, ['GET', 'HEAD'])
  })

  it('answers ok as plain text when no check is supplied', async () => {
    const { routes } = register(healthCheck())
    const response = await routes[0].handler({ request: request('/health') })
    assert.equal(response.status, 200)
    assert.equal(response.headers.get('cache-control'), 'no-store')
    assert.equal(await response.text(), 'ok\n')
  })

  it('serializes an object result as JSON', async () => {
    const { routes } = register(healthCheck({ check: () => ({ status: 'up', queue: 3 }) }))
    const response = await routes[0].handler({ request: request('/health') })
    assert.equal(response.headers.get('content-type'), 'application/json; charset=utf-8')
    assert.deepEqual(await response.json(), { status: 'up', queue: 3 })
  })

  it('reports a thrown check as the configured failure status without echoing it', async () => {
    // `/health` is reachable without credentials by definition — a platform
    // probe calls it — so whatever it says is public. A driver's message names
    // internal hosts, private IPs, ports, and database names; an anonymous
    // caller polling a degraded dependency must not be handed that map.
    const logged = []
    const { routes } = register(
      healthCheck({
        path: '/readyz',
        failureStatus: 500,
        logger: (entry) => logged.push(entry),
        check() {
          throw new Error('connect ECONNREFUSED 10.0.0.5:5432')
        },
      }),
    )
    assert.equal(routes[0].path, '/readyz')
    const response = await routes[0].handler({ request: request('/readyz') })
    assert.equal(response.status, 500)
    assert.deepEqual(await response.json(), { status: 'error' })

    // The operator still has to be able to debug this, so the message goes to
    // the log sink rather than being dropped.
    assert.equal(logged.length, 1)
    assert.equal(logged[0].path, '/readyz')
    assert.equal(logged[0].message, 'connect ECONNREFUSED 10.0.0.5:5432')
    assert.ok(logged[0].error instanceof Error)
  })

  it('echoes the message only under the explicit exposeErrors opt-in', async () => {
    const { routes } = register(
      healthCheck({
        exposeErrors: true,
        logger() {},
        check() {
          throw new Error('connect ECONNREFUSED 10.0.0.5:5432')
        },
      }),
    )
    const response = await routes[0].handler({ request: request('/health') })
    assert.equal(response.status, 503)
    assert.deepEqual(await response.json(), {
      status: 'error',
      error: 'connect ECONNREFUSED 10.0.0.5:5432',
    })
  })

  it('writes the failure to console.error when no logger is supplied', async () => {
    const { routes } = register(healthCheck({ check: () => Promise.reject(new Error('down')) }))
    const original = console.error
    const written = []
    console.error = (...args) => written.push(args.join(' '))
    try {
      const response = await routes[0].handler({ request: request('/health') })
      assert.equal(response.status, 503)
      assert.deepEqual(await response.json(), { status: 'error' })
    } finally {
      console.error = original
    }
    assert.equal(written.length, 1)
    assert.match(written[0], /\[ruvyxa:health-check\] \/health check failed: down/)
  })

  it('never lets a throwing log sink turn the probe into an unhandled error', async () => {
    const { routes } = register(
      healthCheck({
        logger() {
          throw new Error('sink exploded')
        },
        check() {
          throw new Error('down')
        },
      }),
    )
    const original = console.error
    console.error = () => {}
    try {
      const response = await routes[0].handler({ request: request('/health') })
      assert.equal(response.status, 503)
      assert.deepEqual(await response.json(), { status: 'error' })
    } finally {
      console.error = original
    }
  })

  it('rejects a non-function logger', () => {
    assert.throws(() => healthCheck({ logger: 'stderr' }), /logger must be a function/)
  })
})

describe('webVitals()', () => {
  const plugin = webVitals()
  const { middleware, routes, buildComplete } = register(plugin)

  it('loads its client script with src rather than inlining it', () => {
    assert.deepEqual(plugin.head, [
      { tag: 'script', attrs: { src: '/web-vitals.js', defer: true } },
    ])
  })

  it('serves the client script so development matches the build', async () => {
    const response = await middleware[0].onRequest(request('/web-vitals.js'))
    assert.equal(response.headers.get('content-type'), 'text/javascript; charset=utf-8')
    const body = await response.text()
    assert.match(body, /PerformanceObserver/)
    assert.match(body, /"\/__metrics\/web-vitals"/)
  })

  it('writes the same script into the build output', async () => {
    const context = tempBuildContext({})
    for (const hook of buildComplete) await hook(context)
    const written = readFileSync(path.join(context.outDir, 'assets', 'web-vitals.js'), 'utf8')
    assert.match(written, /Generated by ruvyxa\/plugins webVitals/)
  })

  it('accepts a well-formed metric and answers the beacon with 204', async () => {
    const seen = []
    const { routes: collector } = register(webVitals({ logger: (entry) => seen.push(entry) }))
    const response = await collector[0].handler({
      request: request('/__metrics/web-vitals', {
        method: 'POST',
        body: JSON.stringify({ name: 'LCP', value: 1234.5, pathname: '/pricing' }),
      }),
    })
    assert.equal(response.status, 204)
    assert.deepEqual(seen, [{ name: 'LCP', value: 1234.5, pathname: '/pricing' }])
  })

  it('drops a payload that is not the shape its own script sends', async () => {
    const seen = []
    const { routes: collector } = register(webVitals({ logger: (entry) => seen.push(entry) }))
    for (const body of [
      JSON.stringify({ name: 'INJECTED', value: 1, pathname: '/' }),
      JSON.stringify({ name: 'LCP', value: -1, pathname: '/' }),
      JSON.stringify({ name: 'LCP', value: 1, pathname: 'https://evil.test' }),
      'not json',
    ]) {
      const response = await collector[0].handler({
        request: request('/__metrics/web-vitals', { method: 'POST', body }),
      })
      assert.equal(response.status, 204)
    }
    assert.deepEqual(seen, [])
  })

  it('registers the collector as an exact POST route', () => {
    assert.equal(routes[0].path, '/__metrics/web-vitals')
    assert.equal(routes[0].method, 'POST')
  })

  it('rejects a script path that collides with the endpoint', () => {
    assert.throws(
      () => webVitals({ endpoint: '/vitals.js', scriptPath: '/vitals.js' }),
      /endpoint and scriptPath must differ/,
    )
    assert.throws(() => webVitals({ sampleRate: 2 }), /sampleRate must be a number from 0 to 1/)
  })

  /** Post one well-formed beacon from `ip`, and answer with its status. */
  async function beacon(collector, ip) {
    const response = await collector.handler({
      request: request('/__metrics/web-vitals', {
        method: 'POST',
        headers: ip ? { 'x-real-ip': ip } : {},
        body: JSON.stringify({ name: 'LCP', value: 1, pathname: '/' }),
      }),
    })
    return response.status
  }

  it('refuses beacons past the per-client budget', async () => {
    const seen = []
    const { routes: collector } = register(
      webVitals({
        logger: (entry) => seen.push(entry),
        clientIp: (value) => value.headers.get('x-real-ip'),
        rateLimit: { max: 2, windowSeconds: 60 },
      }),
    )
    const statuses = []
    for (let index = 0; index < 4; index += 1) {
      statuses.push(await beacon(collector[0], '203.0.113.9'))
    }
    assert.deepEqual(statuses, [204, 204, 429, 429])
    assert.equal(seen.length, 2)
    // A different client keeps its own allowance: the limiter must not be one
    // shared counter that the loudest caller drains for everybody.
    assert.equal(await beacon(collector[0], '198.51.100.4'), 204)
    assert.equal(seen.length, 3)
  })

  it('bounds the endpoint as a whole when no client resolver is configured', async () => {
    const seen = []
    const { routes: collector } = register(
      webVitals({ logger: (entry) => seen.push(entry), rateLimit: { max: 1, windowSeconds: 60 } }),
    )
    const statuses = []
    for (let index = 0; index < 60; index += 1) statuses.push(await beacon(collector[0], undefined))
    // Fifty times the per-client ceiling, derived rather than separately
    // configurable, exactly as `@ruvyxa/auth` derives its wider ceilings.
    assert.equal(statuses.filter((status) => status === 204).length, 50)
    assert.equal(statuses.filter((status) => status === 429).length, 10)
    assert.equal(seen.length, 50)
  })

  it('accepts every beacon when the limiter is turned off', async () => {
    const seen = []
    const { routes: collector } = register(
      webVitals({ logger: (entry) => seen.push(entry), rateLimit: false }),
    )
    for (let index = 0; index < 200; index += 1) {
      assert.equal(await beacon(collector[0], undefined), 204)
    }
    assert.equal(seen.length, 200)
  })

  it('rejects a rate limit that would switch the endpoint off', () => {
    assert.throws(() => webVitals({ rateLimit: { max: 0 } }), /rateLimit\.max/)
    assert.throws(() => webVitals({ rateLimit: { windowSeconds: 0 } }), /rateLimit\.windowSeconds/)
    assert.throws(() => webVitals({ clientIp: 'x-real-ip' }), /clientIp must be a function/)
  })
})

describe('wellKnown()', () => {
  const plugin = wellKnown({
    securityTxt: {
      contact: ['mailto:security@example.com', 'https://example.com/report'],
      expires: '2027-01-01T00:00:00.000Z',
      policy: 'https://example.com/security-policy',
      preferredLanguages: ['en', 'th'],
    },
    entries: [{ name: 'apple-app-site-association', body: { applinks: { details: [] } } }],
  })
  const { middleware, buildComplete } = register(plugin)

  it('renders security.txt with the two fields RFC 9116 requires', async () => {
    const response = await middleware[0].onRequest(request('/.well-known/security.txt'))
    assert.equal(response.headers.get('content-type'), 'text/plain; charset=utf-8')
    assert.equal(
      await response.text(),
      'Contact: mailto:security@example.com\n' +
        'Contact: https://example.com/report\n' +
        'Expires: 2027-01-01T00:00:00.000Z\n' +
        'Policy: https://example.com/security-policy\n' +
        'Preferred-Languages: en, th\n',
    )
  })

  it('serializes an object entry as JSON', async () => {
    const response = await middleware[0].onRequest(
      request('/.well-known/apple-app-site-association'),
    )
    assert.equal(response.headers.get('content-type'), 'application/json; charset=utf-8')
    assert.deepEqual(await response.json(), { applinks: { details: [] } })
  })

  it('writes every file into the build output under .well-known', async () => {
    const context = tempBuildContext({})
    for (const hook of buildComplete) await hook(context)
    const dir = path.join(context.outDir, 'assets', '.well-known')
    assert.deepEqual(readdirSync(dir).sort(), ['apple-app-site-association', 'security.txt'])
  })

  it('rejects a contact that is not a reachable URI scheme', () => {
    assert.throws(
      () => wellKnown({ securityTxt: { contact: 'security@example.com', expires: '2027-01-01' } }),
      /must be a mailto:, https:\/\/, or tel: URI/,
    )
  })

  it('rejects an unusable expiry and an empty declaration', () => {
    assert.throws(
      () => wellKnown({ securityTxt: { contact: 'mailto:a@b.co', expires: 'whenever' } }),
      /expires must be a valid date/,
    )
    assert.throws(() => wellKnown(), /pass securityTxt and\/or at least one entry/)
  })

  it('rejects a contact carrying its own newline', () => {
    // `Contact: ${contact}` is interpolated into a line-oriented record, so a
    // newline inside the value writes a directive of the attacker's choosing.
    // Every other URL field in the same function already rejects `[\r\n\0]`
    // through `validateAbsoluteHttpUrl`; `contact` was checked for a scheme
    // prefix only.
    assert.throws(
      () =>
        wellKnown({
          securityTxt: {
            contact: 'mailto:a@b.co\nPolicy: https://evil.example',
            expires: '2027-01-01T00:00:00.000Z',
          },
        }),
      /contact/,
    )
    assert.throws(
      () =>
        wellKnown({
          securityTxt: {
            contact: [
              'mailto:a@b.co',
              'https://example.com/report\r\nCanonical: https://evil.test',
            ],
            expires: '2027-01-01T00:00:00.000Z',
          },
        }),
      /contact/,
    )
  })

  it('rejects a content type a Headers constructor would refuse, at construction', () => {
    // Stored unvalidated, `contentType` first reached a `Headers` constructor at
    // request time, so a config typo was a 500 on a `/.well-known/` path in
    // production instead of a build-time failure.
    assert.throws(
      () =>
        wellKnown({
          entries: [{ name: 'note.txt', body: 'x', contentType: 'text/plain\r\nX-Evil: 1' }],
        }),
      /contentType/,
    )
    assert.doesNotThrow(() =>
      wellKnown({ entries: [{ name: 'note.txt', body: 'x', contentType: 'text/plain' }] }),
    )
  })
})

describe('llmsTxt()', () => {
  it('renders curated sections ahead of routes discovered from the manifest', async () => {
    const { buildComplete } = register(
      llmsTxt({
        siteUrl: 'https://example.com',
        title: 'Example',
        summary: 'A demo application.',
        sections: [
          { links: [{ title: 'API', url: '/docs/api', notes: 'REST reference' }], title: 'Docs' },
        ],
        exclude: ['/internal/*'],
      }),
    )
    const context = tempBuildContext({
      routes: [
        { kind: 'page', path: '/' },
        { kind: 'page', path: '/pricing' },
        { kind: 'page', path: '/internal/admin' },
        { kind: 'page', path: '/blog/[slug]' },
        { kind: 'api', path: '/api/health' },
      ],
    })
    for (const hook of buildComplete) await hook(context)
    assert.equal(
      readFileSync(path.join(context.outDir, 'assets', 'llms.txt'), 'utf8'),
      '# Example\n\n' +
        '> A demo application.\n\n' +
        '## Docs\n- [API](https://example.com/docs/api): REST reference\n\n' +
        '## Pages\n' +
        '- [Home](https://example.com/)\n' +
        '- [/pricing](https://example.com/pricing)\n',
    )
  })

  it('omits discovered routes when routes is false', async () => {
    const { buildComplete } = register(
      llmsTxt({
        siteUrl: 'https://example.com',
        title: 'Example',
        routes: false,
        sections: [{ title: 'Start', links: [{ title: 'Home', url: '/' }] }],
      }),
    )
    const context = tempBuildContext({ routes: [{ kind: 'page', path: '/pricing' }] })
    for (const hook of buildComplete) await hook(context)
    const body = readFileSync(path.join(context.outDir, 'assets', 'llms.txt'), 'utf8')
    assert.doesNotMatch(body, /pricing/)
  })

  it('requires a title and a site origin', () => {
    assert.throws(() => llmsTxt({ siteUrl: 'https://example.com', title: '' }), /title/)
    assert.throws(() => llmsTxt({ siteUrl: 'nope', title: 'Example' }), /siteUrl/)
  })
})

describe('build-time artifacts in development', () => {
  it('serves robots.txt from the same bytes the build writes', async () => {
    const { middleware, buildComplete } = register(
      robots({ sitemap: 'https://x.test/sitemap.xml' }),
    )
    const response = await middleware[0].onRequest(request('/robots.txt'))
    const served = await response.text()
    assert.equal(response.headers.get('content-type'), 'text/plain; charset=utf-8')

    const context = tempBuildContext({})
    for (const hook of buildComplete) await hook(context)
    assert.equal(readFileSync(path.join(context.outDir, 'assets', 'robots.txt'), 'utf8'), served)
  })

  it('serves a feed and a search index built from static input', async () => {
    const feedPlugin = register(
      feed({
        siteUrl: 'https://example.com',
        title: 'Example',
        description: 'Posts',
        items: [{ title: 'Hello', url: '/blog/hello' }],
      }),
    )
    const feedResponse = await feedPlugin.middleware[0].onRequest(request('/rss.xml'))
    assert.equal(feedResponse.headers.get('content-type'), 'application/rss+xml; charset=utf-8')
    assert.match(await feedResponse.text(), /<title>Hello<\/title>/)

    const searchPlugin = register(
      searchIndex({
        documents: [{ id: 'a', title: 'Alpha', url: '/a', text: 'first document' }],
      }),
    )
    const searchResponse = await searchPlugin.middleware[0].onRequest(request('/search-index.json'))
    assert.deepEqual((await searchResponse.json()).terms.document, ['a'])
  })

  it('leaves loader-backed artifacts to the build in production', () => {
    // A loader may read files or query a database. Running it per request in
    // production would put that on the response path, in front of the asset the
    // build already wrote.
    const loaderFeed = () =>
      feed({
        siteUrl: 'https://example.com',
        title: 'Example',
        description: 'Posts',
        items: () => [{ title: 'Hello', url: '/blog/hello' }],
      })
    assert.deepEqual(register(loaderFeed(), 'production').middleware, [])
    assert.equal(register(loaderFeed(), 'development').middleware.length, 1)
  })

  it('runs a loader per request in development, where nothing is built yet', async () => {
    let loads = 0
    const { middleware } = register(
      feed({
        siteUrl: 'https://example.com',
        title: 'Example',
        description: 'Posts',
        items: () => {
          loads += 1
          return [{ title: `Post ${loads}`, url: '/blog/hello' }]
        },
      }),
      'development',
    )
    // Not memoized: the developer edits the source the loader reads, and a
    // cached answer would show them the feed they had when the server started.
    assert.match(await (await middleware[0].onRequest(request('/rss.xml'))).text(), /Post 1/)
    assert.match(await (await middleware[0].onRequest(request('/rss.xml'))).text(), /Post 2/)
    assert.equal(loads, 2)
  })

  it('serves a loader-backed search index in development', async () => {
    const { middleware } = register(
      searchIndex({
        documents: () => [{ id: 'a', title: 'Alpha', url: '/a', text: 'loaded document' }],
      }),
      'development',
    )
    const response = await middleware[0].onRequest(request('/search-index.json'))
    assert.deepEqual((await response.json()).terms.loaded, ['a'])
  })

  it('memoizes a static list rather than rebuilding it per request', async () => {
    const { middleware } = register(
      feed({
        siteUrl: 'https://example.com',
        title: 'Example',
        description: 'Posts',
        items: [{ title: 'Hello', url: '/blog/hello' }],
      }),
      'development',
    )
    const first = await (await middleware[0].onRequest(request('/rss.xml'))).text()
    const second = await (await middleware[0].onRequest(request('/rss.xml'))).text()
    assert.equal(first, second)
  })
})

describe('headScriptHashes()', () => {
  const inline = definePlugin({
    name: 'test:inline',
    head: [
      { tag: 'script', children: 'console.info("a")' },
      { tag: 'style', children: '.a{color:red}' },
      // Nothing to execute, so nothing to hash.
      { tag: 'link', attrs: { rel: 'preconnect', href: 'https://example.com' } },
    ],
  })

  it('hashes exactly the bytes between the tags', () => {
    const expected = `'sha256-${createHash('sha256').update('console.info("a")', 'utf8').digest('base64')}'`
    assert.deepEqual(headScriptHashes([inline]), [expected])
  })

  it('hashes styles when asked, so style-src can drop unsafe-inline too', () => {
    const expected = `'sha256-${createHash('sha256').update('.a{color:red}', 'utf8').digest('base64')}'`
    assert.deepEqual(headScriptHashes([inline], { tag: 'style' }), [expected])
  })

  it('contributes nothing for a plugin that loads its script with src', () => {
    // `webVitals` publishes its script as a build asset for exactly this
    // reason: an inline snippet would force every consumer to hash it.
    assert.deepEqual(headScriptHashes([webVitals()]), [])
    assert.deepEqual(headScriptHashes([redirects([{ source: '/a', destination: '/b' }])]), [])
  })

  it('is order-independent and deduplicated, so the policy string is stable', () => {
    const other = definePlugin({ name: 'test:other', head: { tag: 'script', children: 'b()' } })
    const duplicate = definePlugin({
      name: 'test:duplicate',
      head: { tag: 'script', children: 'console.info("a")' },
    })
    assert.deepEqual(
      headScriptHashes([inline, other, duplicate]),
      headScriptHashes([duplicate, other, inline]),
    )
    assert.equal(headScriptHashes([inline, duplicate]).length, 1)
  })

  it('composes into a policy securityHeaders accepts', async () => {
    const plugins = [inline]
    const { middleware } = register(
      securityHeaders({
        contentSecurityPolicy: { 'script-src': ["'self'", ...headScriptHashes(plugins)] },
      }),
    )
    const response = await middleware[0].onResponse(request('/'), new Response('ok'))
    const policy = response.headers.get('content-security-policy')
    assert.match(policy, /script-src 'self' 'sha256-[A-Za-z0-9+/=]+'/)
    assert.doesNotMatch(policy, /unsafe-inline/)
  })

  it('rejects anything that is not a plugin list', () => {
    assert.throws(() => headScriptHashes(undefined), /pass the array of plugins/)
  })
})

describe('securityHeaders({ inlineScriptHashes })', () => {
  /** A prerender tree shaped the way a build writes one. */
  function prerenderFixture(documents) {
    const context = tempBuildContext({})
    for (const [route, html] of Object.entries(documents)) {
      const dir = path.join(context.outDir, 'prerender', ...route.split('/').filter(Boolean))
      mkdirSync(dir, { recursive: true })
      writeFileSync(path.join(dir, 'index.html'), html, 'utf8')
    }
    return context
  }

  const policy = { 'default-src': ["'self'"], 'script-src': ["'self'"] }

  async function runBuildAndRespond(documents, pathname, options = {}) {
    const context = prerenderFixture(documents)
    // An absolute `outDir` is what a project with a non-default build
    // directory passes, and `path.resolve` ignores the root beside it.
    const plugin = securityHeaders({
      contentSecurityPolicy: policy,
      inlineScriptHashes: { outDir: context.outDir },
      ...options,
    })
    const { middleware, buildComplete } = register(plugin)
    for (const hook of buildComplete) await hook(context)
    const response = await middleware[0].onResponse(request(pathname), new Response('ok'))
    return { response, context }
  }

  it("records a hash for React's streaming swap script and nothing else", async () => {
    const context = prerenderFixture({
      '/ppr': [
        '<html><body>',
        '<script id="_R_">requestAnimationFrame(function(){$RT=performance.now()});</script>',
        '<script>$RB=[];$RC("B:0","S:0")</script>',
        // Not hashed: `script-src` already governs a file, a data block is not
        // executed, and an empty element has nothing to run.
        '<script type="module" src="/app.js"></script>',
        '<script type="application/json" id="__ruvyxa-bootstrap">{"params":{}}</script>',
        '<script type="application/ld+json">{"@type":"Article"}</script>',
        '</body></html>',
      ].join(''),
      '/plain': '<html><body><script type="module" src="/app.js"></script></body></html>',
    })
    const { buildComplete } = register(
      securityHeaders({ contentSecurityPolicy: policy, inlineScriptHashes: true }),
    )
    for (const hook of buildComplete) await hook(context)

    const manifest = JSON.parse(
      readFileSync(path.join(context.outDir, 'csp-inline-hashes.json'), 'utf8'),
    )
    assert.deepEqual(Object.keys(manifest.documents), ['/ppr'])
    assert.equal(manifest.documents['/ppr'].length, 2)

    const expected = `'sha256-${createHash('sha256')
      .update('$RB=[];$RC("B:0","S:0")', 'utf8')
      .digest('base64')}'`
    assert.ok(manifest.documents['/ppr'].includes(expected), 'the swap script must be covered')
  })

  it("adds a document's hashes to script-src on its own response only", async () => {
    const documents = {
      '/ppr': '<html><body><script>$RC("B:0","S:0")</script></body></html>',
      '/plain': '<html><body></body></html>',
    }
    const covered = await runBuildAndRespond(documents, '/ppr')
    const policyHeader = covered.response.headers.get('content-security-policy')
    assert.match(policyHeader, /script-src 'self' 'sha256-[A-Za-z0-9+/=]+'/)
    assert.match(policyHeader, /default-src 'self'/)

    const uncovered = await runBuildAndRespond(documents, '/plain')
    assert.equal(
      uncovered.response.headers.get('content-security-policy'),
      "default-src 'self'; script-src 'self'",
    )
  })

  it('leaves the policy alone when the build recorded nothing', async () => {
    const plugin = securityHeaders({
      contentSecurityPolicy: policy,
      inlineScriptHashes: { outDir: path.join(tmpdir(), 'ruvyxa-absent-manifest') },
    })
    const { middleware } = register(plugin)
    // No build ran, so there is no manifest to read. A missing file must not
    // fail the response; it just goes out without the extra sources.
    const response = await middleware[0].onResponse(request('/ppr'), new Response('ok'))
    assert.equal(
      response.headers.get('content-security-policy'),
      "default-src 'self'; script-src 'self'",
    )
  })

  it('refuses to be enabled without a policy to add sources to', () => {
    assert.throws(
      () => securityHeaders({ inlineScriptHashes: true }),
      /needs a contentSecurityPolicy/,
    )
  })

  it('does not add a script-src directive that the policy deliberately omits', async () => {
    // A policy with only `default-src` is falling back on purpose. Inventing a
    // `script-src` would narrow it to exactly the hashes, blocking the
    // application's own bundles.
    const { response } = await runBuildAndRespond(
      { '/ppr': '<html><body><script>$RC("B:0","S:0")</script></body></html>' },
      '/ppr',
      { contentSecurityPolicy: { 'default-src': ["'self'"] } },
    )
    assert.equal(response.headers.get('content-security-policy'), "default-src 'self'")
  })
})

describe('plugin path scope', () => {
  // Replays `tests/fixtures/plugin-path-scope-conformance.json`. The native
  // host replays the same file from `plugin_bridge.rs`, where the seam is which
  // path string it hands the plugin runtime rather than the match itself.
  const fixture = JSON.parse(
    readFileSync(
      path.join(import.meta.dirname, '..', '..', 'fixtures', 'plugin-path-scope-conformance.json'),
      'utf8',
    ),
  )

  /** The request a client can send for one fixture target. */
  function requestFor(target, method = 'POST') {
    return new Request(`http://ruvyxa.test${target}`, { method })
  }

  /**
   * One registry holding all three scoping shapes the table covers. The three
   * scopes are mutually exclusive across the table, so the body of whichever
   * short-circuit fires names the hook that matched.
   */
  function scopedRegistry() {
    return createPluginRegistry({
      root: '/project',
      plugins: [
        {
          name: 'fixture:route',
          register({ http }) {
            http.route({
              path: fixture.routePath,
              handler: () => new Response('route'),
            })
          },
        },
        {
          name: 'fixture:api-prefix',
          register({ http }) {
            http.onRequest({
              match: fixture.patterns.apiPrefix,
              handler: () => new Response('apiPrefix'),
            })
          },
        },
        {
          name: 'fixture:admin-exact',
          register({ http }) {
            http.onRequest({
              match: fixture.patterns.adminExact,
              handler: () => new Response('adminExact'),
            })
          },
        },
      ],
    })
  }

  /** The hook the table says must run for this target, or null for none. */
  function expectedHook(testCase) {
    if (testCase.matchesRoute) return 'route'
    for (const key of Object.keys(fixture.patterns)) {
      if (testCase.inScope[key]) return key
    }
    return null
  }

  it('agrees with the router on what a request path is', () => {
    for (const testCase of fixture.cases) {
      assert.equal(
        canonicalRoutePath(testCase.target),
        testCase.canonical,
        `canonical form of ${testCase.target}`,
      )
      const expected = testCase.canonical ?? testCase.fallbackPathname
      assert.equal(
        decodedRequestPathname(requestFor(testCase.target)),
        expected,
        `plugin path for ${testCase.target}`,
      )
    }
  })

  it('scopes every hook shape by the canonical path', () => {
    for (const testCase of fixture.cases) {
      const pathname = decodedRequestPathname(requestFor(testCase.target))
      for (const [key, patterns] of Object.entries(fixture.patterns)) {
        assert.equal(
          matchesPatterns(patterns, pathname),
          testCase.inScope[key],
          `${key} scope for ${testCase.target}`,
        )
      }
      assert.equal(
        fixture.routePath === pathname,
        testCase.matchesRoute,
        `route match for ${testCase.target}`,
      )
    }
  })

  it('dispatches request hooks and plugin routes on the canonical path', async () => {
    const registry = await scopedRegistry()
    for (const testCase of fixture.cases) {
      const outcome = await dispatchPluginRequest(registry, requestFor(testCase.target))
      const actual = outcome.kind === 'response' ? await outcome.response.text() : null
      assert.equal(actual, expectedHook(testCase), `dispatch for ${testCase.target}`)
    }
  })

  it('scopes response hooks on the canonical path', async () => {
    const registry = await createPluginRegistry({
      root: '/project',
      plugins: [
        {
          name: 'fixture:api-response',
          register({ http }) {
            http.onResponse({
              match: fixture.patterns.apiPrefix,
              handler: ({ response }) => {
                const marked = new Response(response.body, response)
                marked.headers.set('x-fixture-scope', 'apiPrefix')
                return marked
              },
            })
          },
        },
      ],
    })
    for (const testCase of fixture.cases) {
      const response = await dispatchPluginResponse(
        registry,
        requestFor(testCase.target),
        new Response('ok'),
      )
      assert.equal(
        response.headers.get('x-fixture-scope'),
        testCase.inScope.apiPrefix ? 'apiPrefix' : null,
        `response scope for ${testCase.target}`,
      )
    }
  })

  it('refuses a plugin route that claims a reserved framework path', async () => {
    // RTMS-05. `RESERVED_FRAMEWORK_PATHS` was read only by the two socket
    // normalisers, so despite its docstring a plugin *route* at
    // `/__ruvyxa/action` registered cleanly -- and then answered every server
    // action in a deployed build while being dead under `dev`/`start`, where
    // the framework endpoint is an axum route ahead of the plugin-bearing
    // fallback. Refusing at registration is the only place both hosts can agree.
    for (const reserved of RESERVED_FRAMEWORK_PATHS) {
      await assert.rejects(
        () =>
          createPluginRegistry({
            root: '/project',
            plugins: [
              {
                name: 'fixture:reserved-route',
                register({ http }) {
                  http.route({
                    path: reserved,
                    method: 'POST',
                    handler: () => new Response('shadowed'),
                  })
                },
              },
            ],
          }),
        (error) =>
          error instanceof TypeError &&
          error.message ===
            `plugin "fixture:reserved-route" http.route() path "${reserved}" collides with a reserved framework route`,
        `http.route({ path: '${reserved}' }) must be refused`,
      )
    }
  })

  it('guards the RUV-C2 request against originGuard defaults', async () => {
    const registry = await createPluginRegistry({
      root: '/project',
      plugins: [originGuard()],
    })
    // The demonstrated attack: a plain cross-site form POST whose action
    // carries a leading double slash. It routes to `/api/users` either way, so
    // the guard has to see it.
    const outcome = await dispatchPluginRequest(
      registry,
      new Request('http://victim.test//api/users', {
        method: 'POST',
        headers: { host: 'victim.test', origin: 'https://evil.test' },
      }),
    )
    assert.equal(outcome.kind, 'response')
    assert.equal(outcome.response.status, 403)
  })
})

describe('the plugin rate limiter answers the shared admission table', () => {
  // The third replay of `tests/fixtures/rate-limit-conformance.json`. The other
  // two are `the_shared_rate_limit_conformance_table_is_answered_the_same_way`
  // in `crates/ruvyxa_middleware/src/builtin.rs` and
  // `rate limiter conformance with the native middleware` in
  // `tests/packages/ruvyxa/serverless-handler.test.mjs`.
  //
  // `webVitals` needed a limiter and the workspace already had four, none of
  // which it could reach: the native ones are Rust, and the deployed one lives
  // in the module every adapter copies verbatim into a function bundle. So this
  // is a fifth copy of the algorithm, and a fifth copy that agrees with the
  // others only by inspection is a copy that will stop agreeing. The map policy
  // is the shared part and this table is what holds it.
  //
  // How a key is *bounded* is deliberately host-local, so only the bound itself
  // is asserted here rather than the spelling.
  const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
  const contract = JSON.parse(
    readFileSync(path.join(workspaceRoot, 'tests/fixtures/rate-limit-conformance.json'), 'utf8'),
  )

  /** Expand a fixture value, which is a string or a { repeat, times, suffix }. */
  function valueOf(spec) {
    if (spec === null || spec === undefined) return spec
    if (typeof spec === 'string') return spec
    return spec.repeat.repeat(spec.times) + (spec.suffix ?? '')
  }

  function countOf(spec) {
    return spec === 'capacity' ? MAX_TRACKED_PLUGIN_RATE_LIMIT_KEYS : spec
  }

  function floodKey(index) {
    return `flood-${String(index).padStart(5, '0')}`
  }

  for (const testCase of contract.keyCases) {
    it(`bounds: ${testCase.name}`, () => {
      const key = boundedRateLimitKey(valueOf(testCase.identity))
      assert.ok(
        key.length <= contract.maxKeyLength,
        `a tracked key of ${key.length} exceeds the ${contract.maxKeyLength} bound`,
      )
      assert.equal(key, boundedRateLimitKey(valueOf(testCase.identity)))
      if (testCase.distinctFrom !== undefined) {
        // Two clients that share a bucket can each limit the other.
        assert.notEqual(key, boundedRateLimitKey(valueOf(testCase.distinctFrom)))
      }
    })
  }

  for (const testCase of contract.admissionCases) {
    it(`admits: ${testCase.name}`, () => {
      const now = Date.now()
      const buckets = new Map()
      if (testCase.prefill) {
        const count = countOf(testCase.prefill.count)
        for (let index = 0; index < count; index += 1) {
          buckets.set(floodKey(index), {
            remaining: 0,
            startedAt:
              testCase.prefill.state === 'expired'
                ? now - testCase.windowSeconds * 1000 - 1000
                : now - (count - index),
          })
        }
      }

      for (const [index, entry] of testCase.requests.entries()) {
        const retryAfter = consumeFixedWindow(
          buckets,
          entry.identity,
          testCase.max,
          testCase.windowSeconds,
        )
        assert.equal(
          retryAfter === null,
          entry.allowed,
          `request ${index} for ${entry.identity} was ${retryAfter === null ? 'admitted' : 'refused'}`,
        )
        if (retryAfter !== null) assert.ok(retryAfter >= 1, 'a refusal must name a wait')
      }

      assert.equal(buckets.size, countOf(testCase.expectTracked))
      for (const entry of testCase.requests) {
        assert.ok(buckets.has(entry.identity), `${entry.identity} was not tracked`)
      }
      if (testCase.expectEvicted === 'oldest') {
        assert.ok(!buckets.has(floodKey(0)), 'the evicted bucket must be the oldest one')
      }
      if (testCase.expectRetained === 'newest') {
        assert.ok(
          buckets.has(floodKey(countOf(testCase.prefill.count) - 1)),
          'the most recently active client must not be the one evicted',
        )
      }
    })
  }
})

/**
 * The harness and the runtime apply one registration rule, not two that agree.
 *
 * They were two, each commented as mirroring the other, and they had already
 * drifted: `plugin-harness.ts` accepted an `http.route()` on a reserved
 * framework path that `plugin-http.mjs` refuses. A plugin could therefore pass
 * the harness that validates it and be rejected by the server that runs it —
 * and a reserved path is not a cosmetic disagreement, because the native host
 * panics inside axum when a second handler registers one.
 *
 * Both are now `packages/@ruvyxa/core/src/plugin-registration.ts`, copied into
 * the runtime by `pnpm --filter ruvyxa sync:runtime` because a serverless
 * function bundle resolves no bare specifiers.
 */
describe('the shared plugin registration rules', () => {
  it('refuses a reserved framework path in both validators', async () => {
    const shared = await import('../../../packages/ruvyxa/runtime/plugin-registration.mjs')
    assert.ok(
      shared.RESERVED_FRAMEWORK_PATHS.length > 0,
      'the reserved list has to have entries for this to assert anything',
    )

    for (const reserved of shared.RESERVED_FRAMEWORK_PATHS) {
      // The real registry every host runs, not the stub `register` helper
      // above -- that one records registrations and validates nothing.
      await assert.rejects(
        createPluginRegistry({
          root: '.',
          environment: 'production',
          plugins: [
            {
              name: 'claimer',
              register: (api) =>
                api.http.route({ path: reserved, handler: () => new Response('') }),
            },
          ],
        }),
        /reserved framework route/,
        `the runtime accepted ${reserved}`,
      )
    }
  })

  it('answers the transport-path rule the same way from either side', async () => {
    const shared = await import('../../../packages/ruvyxa/runtime/plugin-registration.mjs')

    // The allowlist, not the old axum-0.7 denylist. `/{room}` registered a
    // single-segment wildcard that shadowed every one-segment project page.
    for (const literal of ['/socket', '/a/b', '/x-1.2_3~4']) {
      assert.equal(shared.isLiteralTransportPath(literal), true, literal)
    }
    for (const wildcard of ['/{room}', '/{', '/*rest', '/a?b', '/a#b', 'socket', '/']) {
      assert.equal(shared.isLiteralTransportPath(wildcard), false, wildcard)
    }
  })

  it('keeps the heartbeat bounds on both transports', async () => {
    const shared = await import('../../../packages/ruvyxa/runtime/plugin-registration.mjs')

    for (const normalize of [shared.normalizeRealtime, shared.normalizePresence]) {
      assert.throws(() => normalize('p', { heartbeatMs: 4_999 }), /between 5000 and 120000/)
      assert.throws(() => normalize('p', { heartbeatMs: 120_001 }), /between 5000 and 120000/)
      assert.equal(normalize('p', { heartbeatMs: 25_000 }).heartbeatMs, 25_000)
      // A reserved path is refused here too, which is the check that used to
      // exist in one validator and not the other.
      assert.throws(() => normalize('p', { path: '/__ruvyxa/hmr' }), /reserved framework route/)
    }
  })
})
