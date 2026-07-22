import assert from 'node:assert/strict'
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

import {
  alias,
  bundleBudget,
  cacheRules,
  contentEngine,
  feed,
  headers,
  observability,
  openApi,
  pwa,
  redirects,
  requireEnv,
  robots,
  searchIndex,
  securityHeaders,
  sitemap,
} from '../../../packages/ruvyxa/dist/plugins.js'

/** Runs a plugin's setup with a capturing registration context. */
function register(plugin) {
  const registered = { middleware: [], resolveId: [], buildComplete: [] }
  plugin.setup({
    addMiddleware(value) {
      registered.middleware.push(typeof value === 'function' ? { onRequest: value } : value)
    },
    resolveId(hook) {
      registered.resolveId.push(hook)
    },
    transform() {},
    onBuildComplete(hook) {
      registered.buildComplete.push(hook)
    },
  })
  return registered
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
      /ruvyxa-pwa-[0-9a-f]{12}-v1/,
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
    const registered = register(contentEngine(options))
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
    const { middleware } = register(contentEngine(options))
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
    const { middleware } = register(contentEngine(options))
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

  it('can disable the experimental llms.txt artifact', async () => {
    const root = contentProject()
    const registered = register(contentEngine({ ...options, llmsPath: false }))
    assert.doesNotMatch(registered.middleware[0].routes.join(','), /llms\.txt/)
    const context = tempBuildContext({ routes: [] })
    context.root = root
    await registered.buildComplete[0](context)
    assert.equal(existsSync(path.join(context.outDir, 'assets', 'llms.txt')), false)
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
