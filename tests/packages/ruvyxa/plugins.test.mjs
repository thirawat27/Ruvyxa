import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'

import {
  alias,
  bundleBudget,
  headers,
  redirects,
  requireEnv,
  robots,
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
