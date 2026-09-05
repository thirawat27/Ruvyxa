import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerModule = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')
const coreServerModule = path.join(workspaceRoot, 'packages/@ruvyxa/core/dist/server.js')

const {
  MAX_TRACKED_RATE_LIMIT_KEYS,
  boundedKey,
  consumeFixedWindow,
  createHandler,
  prerenderRelativePath,
  rateLimitKey,
} = await import(`file://${handlerModule.replaceAll('\\', '/')}`)
const { cookies, revalidatePath } = await import(`file://${coreServerModule.replaceAll('\\', '/')}`)
const { actionReferenceId, runAction } = await import(
  `file://${path.join(workspaceRoot, 'packages/ruvyxa/runtime/action-runtime.mjs').replaceAll('\\', '/')}`
)
const revalidationConformance = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/revalidation-conformance.json'), 'utf8'),
)
const storedDocumentConformance = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/stored-document-conformance.json'), 'utf8'),
)

function pageRoute(id, routePath, strategy = 'ssr') {
  return { id, path: routePath, kind: 'page', file: `${id}.tsx`, render: { strategy } }
}

function handlerFor(routes, rendered) {
  return createHandler({
    routes,
    importPage: async (routeId) => ({
      render: async ({ path: pathname, params }) => {
        rendered.push({ routeId, pathname, params })
        return `<html>${routeId}</html>`
      },
    }),
    importApi: async () => ({}),
  })
}

describe('serverless request body limits', () => {
  it('binds versioned action references and rejects nonce replay', async () => {
    const fixture = JSON.parse(
      readFileSync(path.join(workspaceRoot, 'tests/fixtures/action-contract.json'), 'utf8'),
    )
    assert.equal(actionReferenceId(fixture.routeId, fixture.source), fixture.expected)
    const submit = async (input) => input
    submit.ruvyxa = { kind: 'action' }
    const route = {
      ...pageRoute(fixture.routeId, '/target'),
      actionReferenceId: fixture.expected,
    }
    const handler = createHandler({
      routes: [route],
      importPage: async () => ({}),
      importApi: async () => ({}),
      importAction: async () => ({ submit }),
    })
    const invoke = (id, nonce) =>
      handler(
        new Request(
          `http://localhost/__ruvyxa/action?path=/target&name=submit&id=${encodeURIComponent(id)}`,
          {
            method: 'POST',
            headers: {
              'content-type': 'application/json',
              host: 'localhost',
              origin: 'http://localhost',
              'sec-fetch-site': 'same-origin',
              'x-ruvyxa-action-nonce': nonce,
            },
            body: '{}',
          },
        ),
      )

    assert.equal((await invoke(fixture.expected, '0123456789abcdef')).status, 200)
    assert.equal((await invoke(fixture.expected, '0123456789abcdef')).status, 409)
    assert.equal((await invoke('a_0000000000000000', 'fedcba9876543210')).status, 409)
  })

  it('refuses a saturated replay guard rather than dropping a live nonce', async () => {
    const fixture = JSON.parse(
      readFileSync(path.join(workspaceRoot, 'tests/fixtures/action-contract.json'), 'utf8'),
    )
    const { maxEntries, perClientMaxEntries, saturation } = fixture.nonce
    assert.equal(saturation.behavior, 'reject')

    const submit = async (input) => input
    submit.ruvyxa = { kind: 'action' }
    const route = {
      ...pageRoute(fixture.routeId, '/target'),
      actionReferenceId: fixture.expected,
    }
    const handler = createHandler({
      routes: [route],
      importPage: async () => ({}),
      importApi: async () => ({}),
      importAction: async () => ({ submit }),
      // The action rate limiter would answer long before the replay guard
      // filled; this test is about the guard's own bound.
      security: { actionRateLimit: { max: maxEntries * 2, window: 60 } },
      // The requests below vary the client through `cf-connecting-ip`, which
      // only names a client where the platform's own ingress writes it — so
      // this handler declares that platform the way the Cloudflare adapter
      // does. Without the declaration every request is one anonymous client
      // and the per-client quota, which is what this measures, has nothing to
      // measure.
      clientIpHeaders: ['cf-connecting-ip'],
    })
    const invoke = (nonce, client = '203.0.113.1') =>
      handler(
        new Request(
          `http://localhost/__ruvyxa/action?path=/target&name=submit&id=${encodeURIComponent(fixture.expected)}`,
          {
            method: 'POST',
            headers: {
              'content-type': 'application/json',
              host: 'localhost',
              origin: 'http://localhost',
              'sec-fetch-site': 'same-origin',
              'x-ruvyxa-action-nonce': nonce,
              'cf-connecting-ip': client,
            },
            body: '{}',
          },
        ),
      )

    const first = 'n0000000000000000'
    // Spread across addresses: no single one may fill the pool any more, which
    // is what the `clientSaturation` case below holds.
    for (let index = 0; index < maxEntries; index++) {
      const client = `203.0.113.${Math.floor(index / perClientMaxEntries)}`
      assert.equal((await invoke(`n${String(index).padStart(16, '0')}`, client)).status, 200)
    }

    const saturated = await invoke('fedcba9876543210', '198.51.100.7')
    assert.equal(saturated.status, saturation.status)
    assert.equal(await saturated.text(), saturation.message)

    // The entry that eviction would have freed is still held, so its replay is
    // still refused. That is what failing closed buys.
    assert.equal((await invoke(first, '203.0.113.0')).status, 409)
  })

  it('refuses one client over its quota without refusing everyone else', async () => {
    const fixture = JSON.parse(
      readFileSync(path.join(workspaceRoot, 'tests/fixtures/action-contract.json'), 'utf8'),
    )
    const { maxEntries, perClientMaxEntries, clientSaturation } = fixture.nonce
    assert.equal(clientSaturation.behavior, 'reject')

    const submit = async (input) => input
    submit.ruvyxa = { kind: 'action' }
    const route = {
      ...pageRoute(fixture.routeId, '/target'),
      actionReferenceId: fixture.expected,
    }
    const handler = createHandler({
      routes: [route],
      importPage: async () => ({}),
      importApi: async () => ({}),
      importAction: async () => ({ submit }),
      security: { actionRateLimit: { max: maxEntries * 2, window: 60 } },
      // As above: the addresses below reach the guard only because this
      // handler declares the ingress header that carries them.
      clientIpHeaders: ['cf-connecting-ip'],
    })
    const invoke = (nonce, client) =>
      handler(
        new Request(
          `http://localhost/__ruvyxa/action?path=/target&name=submit&id=${encodeURIComponent(fixture.expected)}`,
          {
            method: 'POST',
            headers: {
              'content-type': 'application/json',
              host: 'localhost',
              origin: 'http://localhost',
              'sec-fetch-site': 'same-origin',
              'x-ruvyxa-action-nonce': nonce,
              'cf-connecting-ip': client,
            },
            body: '{}',
          },
        ),
      )

    const noisy = '203.0.113.7'
    for (let index = 0; index < perClientMaxEntries; index++) {
      assert.equal((await invoke(`n${String(index).padStart(16, '0')}`, noisy)).status, 200)
    }

    const refused = await invoke('fedcba9876543210', noisy)
    assert.equal(refused.status, clientSaturation.status)
    assert.equal(await refused.text(), clientSaturation.message)

    // The pool is a tenth full, so every other address is unaffected — the
    // whole point of the quota.
    assert.equal((await invoke('fedcba9876543210', '198.51.100.8')).status, 200)
  })

  it('stops reading a lengthless action body at the action limit', async () => {
    const chunk = new TextEncoder().encode('abc')
    const chunkCount = 100
    let producedBytes = 0
    const body = new ReadableStream({
      pull(controller) {
        if (producedBytes >= chunk.byteLength * chunkCount) {
          controller.close()
          return
        }
        producedBytes += chunk.byteLength
        controller.enqueue(chunk)
      },
    })
    const handler = createHandler({
      routes: [],
      importPage: async () => ({}),
      importApi: async () => ({}),
      importAction: async () => ({}),
      security: { actionLimit: 4, apiLimit: 1024 },
    })
    const request = new Request('http://localhost/__ruvyxa/action?path=/target&name=submit', {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        host: 'localhost',
        origin: 'http://localhost',
        'sec-fetch-site': 'same-origin',
      },
      body,
      duplex: 'half',
    })

    assert.equal(request.headers.has('content-length'), false)
    const response = await handler(request)

    assert.equal(response.status, 413)
    assert.equal(await response.text(), 'Action payload is too large')
    assert.ok(
      producedBytes < chunk.byteLength * chunkCount,
      `the action handler consumed all ${producedBytes} bytes before enforcing its limit`,
    )
  })

  it('uses the action limit instead of the generic API limit for action routes', async () => {
    const submit = async (input) => input
    submit.ruvyxa = { kind: 'action' }
    const handler = createHandler({
      routes: [pageRoute('target', '/target')],
      importPage: async () => ({}),
      importApi: async () => ({}),
      importAction: async () => ({ submit }),
      security: { actionLimit: 8, apiLimit: 4 },
    })
    const response = await handler(
      new Request('http://localhost/__ruvyxa/action?path=/target&name=submit', {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          host: 'localhost',
          origin: 'http://localhost',
          'sec-fetch-site': 'same-origin',
        },
        body: '"12345"',
      }),
    )

    assert.equal(response.status, 200)
    assert.deepEqual(await response.json(), { data: '12345', invalidated: [] })
  })

  it('reapplies the endpoint limit when a plugin forwards a new request', async () => {
    const chunk = new TextEncoder().encode('abc')
    let producedBytes = 0
    const forwardedBody = new ReadableStream({
      pull(controller) {
        if (producedBytes >= 300) {
          controller.close()
          return
        }
        producedBytes += chunk.byteLength
        controller.enqueue(chunk)
      },
    })
    const handler = createHandler({
      routes: [],
      importPage: async () => ({}),
      importApi: async () => ({}),
      importAction: async () => ({}),
      security: { actionLimit: 4, apiLimit: 1024 },
      pluginHttp: (_request, next) =>
        next(
          new Request('http://localhost/__ruvyxa/action?path=/target&name=submit', {
            method: 'POST',
            headers: {
              'content-type': 'application/json',
              host: 'localhost',
              origin: 'http://localhost',
              'sec-fetch-site': 'same-origin',
            },
            body: forwardedBody,
            duplex: 'half',
          }),
        ),
    })

    const response = await handler(new Request('http://localhost/proxy'))

    assert.equal(response.status, 413)
    assert.equal(await response.text(), 'Action payload is too large')
    assert.ok(producedBytes < 300, 'the forwarded body bypassed the action stream limit')
  })
})

describe('serverless handler route matching', () => {
  it('prefers static routes over dynamic and catch-all siblings', async () => {
    const rendered = []
    // Alphabetical manifest order puts "[" before letters; the handler must
    // still route /blog/new to the static page like the dev server does.
    const handler = handlerFor(
      [
        pageRoute('blog-slug', '/blog/[slug]'),
        pageRoute('blog-new', '/blog/new'),
        pageRoute('docs-catchall', '/docs/[...path]'),
        pageRoute('docs-about', '/docs/about'),
      ],
      rendered,
    )

    const staticResponse = await handler(new Request('http://localhost/blog/new'))
    assert.equal(staticResponse.status, 200)
    assert.equal(rendered.at(-1).routeId, 'blog-new')

    const dynamicResponse = await handler(new Request('http://localhost/blog/other'))
    assert.equal(dynamicResponse.status, 200)
    assert.equal(rendered.at(-1).routeId, 'blog-slug')
    assert.equal(rendered.at(-1).params.slug, 'other')

    const docsStatic = await handler(new Request('http://localhost/docs/about'))
    assert.equal(docsStatic.status, 200)
    assert.equal(rendered.at(-1).routeId, 'docs-about')
  })

  it('answers a missing static asset with 404 instead of a dynamic page render', async () => {
    const rendered = []
    const handler = handlerFor(
      [
        pageRoute('lang', '/[lang]'),
        pageRoute('docs-catchall', '/docs/[...path]'),
        pageRoute('sitemap', '/sitemap.xml'),
      ],
      rendered,
    )

    // The CDN already missed on these; rendering /[lang] for them returns a
    // 200 HTML body where the browser expects image bytes, and bills a
    // function invocation for every favicon request.
    for (const assetPath of ['/logo.png', '/favicon.ico', '/docs/app.css']) {
      const response = await handler(new Request(`http://localhost${assetPath}`))
      assert.equal(response.status, 404, assetPath)
    }
    assert.deepEqual(rendered, [])

    // An explicitly declared route keeps its extension-bearing path.
    const sitemap = await handler(new Request('http://localhost/sitemap.xml'))
    assert.equal(sitemap.status, 200)
    assert.equal(rendered.at(-1).routeId, 'sitemap')

    // Extensions outside the asset list stay ordinary parameter values.
    const document = await handler(new Request('http://localhost/readme.md'))
    assert.equal(document.status, 200)
    assert.equal(rendered.at(-1).params.lang, 'readme.md')
  })

  it('decodes catch-all segments like dynamic segments', async () => {
    const rendered = []
    const handler = handlerFor([pageRoute('docs', '/docs/[...path]')], rendered)

    const response = await handler(new Request('http://localhost/docs/a%20b/c'))
    assert.equal(response.status, 200)
    assert.deepEqual(rendered.at(-1).params.path, ['a b', 'c'])
  })

  it('canonicalizes the full path once like the development router', async () => {
    const rendered = []
    const handler = handlerFor(
      [pageRoute('unicode', '/ทดสอบ'), pageRoute('blog-slug', '/blog/[slug]')],
      rendered,
    )

    const unicode = await handler(
      new Request('http://localhost/%E0%B8%97%E0%B8%94%E0%B8%AA%E0%B8%AD%E0%B8%9A'),
    )
    assert.equal(unicode.status, 200)
    assert.equal(rendered.at(-1).routeId, 'unicode')
    assert.equal(rendered.at(-1).pathname, '/ทดสอบ')

    const literalPercent = await handler(new Request('http://localhost/blog/%2520'))
    assert.equal(literalPercent.status, 200)
    assert.equal(rendered.at(-1).params.slug, '%20')
    assert.equal(rendered.at(-1).pathname, '/blog/%20')
  })

  it('uses the canonical path for prerender cache lookup', async () => {
    const cachePaths = []
    const route = pageRoute('blog-slug', '/blog/[slug]', 'ssg')
    const handler = createHandler({
      routes: [route],
      importPage: async () => ({ render: async () => '<html>fallback</html>' }),
      importApi: async () => ({}),
      readPrerendered(pathname) {
        cachePaths.push(pathname)
        return '<html>cached</html>'
      },
    })

    const response = await handler(new Request('http://localhost/blog/a%20b'))

    assert.equal(response.status, 200)
    assert.equal(await response.text(), '<html>cached</html>')
    assert.deepEqual(cachePaths, ['/blog/a b'])
  })

  it('matches trailing and duplicate slashes like the dev router', async () => {
    const rendered = []
    const handler = handlerFor(
      [
        pageRoute('docs', '/docs/[...path]'),
        pageRoute('shop', '/shop/[[...slug]]'),
        pageRoute('about', '/about'),
      ],
      rendered,
    )

    // The dev router splits on `/` and drops empty segments, so a trailing
    // slash must not leak an empty catch-all segment into params.
    const trailing = await handler(new Request('http://localhost/docs/a/'))
    assert.equal(trailing.status, 200)
    assert.deepEqual(rendered.at(-1).params.path, ['a'])
    // The canonical path reaches render, matching the dev server boundary.
    assert.equal(rendered.at(-1).pathname, '/docs/a')

    const duplicate = await handler(new Request('http://localhost/docs//a'))
    assert.equal(duplicate.status, 200)
    assert.deepEqual(rendered.at(-1).params.path, ['a'])
    assert.equal(rendered.at(-1).pathname, '/docs/a')

    // An optional catch-all keeps its "absent at the parent route" contract
    // even when the parent is requested with a trailing slash.
    const optionalParent = await handler(new Request('http://localhost/shop/'))
    assert.equal(optionalParent.status, 200)
    assert.equal(rendered.at(-1).routeId, 'shop')
    assert.equal('slug' in rendered.at(-1).params, false)

    assert.equal((await handler(new Request('http://localhost/about/'))).status, 200)
    assert.equal(rendered.at(-1).routeId, 'about')
  })

  it('does not leak internal error detail in responses', async () => {
    const handler = createHandler({
      routes: [pageRoute('boom', '/boom')],
      importPage: async () => ({
        render: async () => {
          throw new Error('secret internal detail /srv/app/db.ts')
        },
      }),
      importApi: async () => ({}),
    })

    const response = await handler(new Request('http://localhost/boom'))
    assert.equal(response.status, 500)
    const body = await response.text()
    assert.equal(body.includes('secret internal detail'), false)
  })
})

describe('serverless handler request validation', () => {
  it('applies security defaults and preserves explicit application headers', async () => {
    const handler = createHandler({
      routes: [{ id: 'api', path: '/api', kind: 'api', file: 'api.ts', render: {} }],
      importPage: async () => ({}),
      importApi: async () => ({
        GET: () =>
          new Response('ok', {
            headers: { 'permissions-policy': 'camera=(self)' },
          }),
      }),
    })

    const response = await handler(new Request('http://localhost/api'))

    assert.equal(response.headers.get('x-content-type-options'), 'nosniff')
    assert.equal(response.headers.get('x-frame-options'), 'DENY')
    assert.equal(response.headers.get('cross-origin-opener-policy'), 'same-origin')
    assert.equal(response.headers.get('permissions-policy'), 'camera=(self)')
  })

  it('can disable framework security defaults without removing application headers', async () => {
    const handler = createHandler({
      routes: [pageRoute('home', '/')],
      securityHeaders: false,
      importPage: async () => ({ render: async () => '<html>home</html>' }),
      importApi: async () => ({}),
    })

    const response = await handler(new Request('http://localhost/'))

    assert.equal(response.headers.get('x-content-type-options'), null)
    assert.equal(response.headers.get('content-type'), 'text/html; charset=utf-8')
  })

  it('rejects paths outside the configured base path instead of slicing them', async () => {
    const rendered = []
    const handler = createHandler({
      routes: [pageRoute('home', '/'), pageRoute('about', '/about')],
      basePath: '/app',
      importPage: async (routeId) => ({
        render: async ({ path: pathname, params }) => {
          rendered.push({ routeId, pathname, params })
          return `<html>${routeId}</html>`
        },
      }),
      importApi: async () => ({}),
    })

    assert.equal((await handler(new Request('http://localhost/app/about'))).status, 200)
    assert.equal(rendered.at(-1).routeId, 'about')

    assert.equal((await handler(new Request('http://localhost/app'))).status, 200)
    assert.equal(rendered.at(-1).routeId, 'home')

    // Blind slicing turned "/other/about" into "r/about" and "/appointments"
    // into "ointments"; neither request belongs to this handler.
    assert.equal((await handler(new Request('http://localhost/other/about'))).status, 404)
    assert.equal((await handler(new Request('http://localhost/appointments'))).status, 404)
    assert.equal(rendered.length, 2, 'no extra route was rendered')
  })

  it('answers malformed percent-encoding with 400 instead of throwing', async () => {
    const rendered = []
    const handler = handlerFor([pageRoute('blog-slug', '/blog/[slug]')], rendered)

    const response = await handler(new Request('http://localhost/blog/%ZZ'))

    assert.equal(response.status, 400)
    assert.equal(rendered.length, 0)
  })

  it('answers encoded path boundaries and control characters with 400', async () => {
    const rendered = []
    const handler = handlerFor([pageRoute('blog-slug', '/blog/[slug]')], rendered)

    // WHATWG URL parsing removes encoded dot-segments before Request exposes
    // `url`; the remaining boundary-changing values reach this guard intact.
    for (const pathname of ['/blog/%2F', '/blog/%5C', '/blog/%00']) {
      const response = await handler(new Request(`http://localhost${pathname}`))
      assert.equal(response.status, 400, pathname)
    }
    assert.equal(rendered.length, 0)
  })
})

describe('Fetch-native built-in middleware parity', () => {
  const route = { id: 'api', path: '/api', kind: 'api', file: 'api.ts', render: {} }

  function middlewareHandler(middleware) {
    return createHandler({
      routes: [route],
      middleware,
      importPage: async () => ({}),
      importApi: async () => ({ GET: () => new Response('ok') }),
    })
  }

  it('applies CORS, timing, request IDs, and configured response headers', async () => {
    const handler = middlewareHandler({
      builtin: {
        cors: {
          origins: ['https://app.example'],
          methods: ['GET', 'OPTIONS'],
          headers: ['Content-Type'],
          credentials: true,
          maxAge: 3600,
        },
        timing: true,
        log: true,
        headers: { 'x-app': 'ruvyxa' },
      },
    })
    const response = await handler(
      new Request('https://worker.example/api', {
        headers: { origin: 'https://app.example', 'x-request-id': 'known-request' },
      }),
    )

    assert.equal(response.status, 200)
    assert.equal(response.headers.get('access-control-allow-origin'), 'https://app.example')
    assert.equal(response.headers.get('access-control-allow-credentials'), 'true')
    assert.match(response.headers.get('vary'), /Origin/i)
    assert.match(response.headers.get('x-response-time'), /^\d+ms$/)
    assert.equal(response.headers.get('x-request-id'), 'known-request')
    assert.equal(response.headers.get('x-app'), 'ruvyxa')
  })

  it('answers valid preflight before routing and varies rejected origins', async () => {
    const handler = middlewareHandler({
      builtin: {
        cors: { origins: ['https://app.example'], methods: ['GET'], maxAge: 60 },
      },
    })
    const preflight = await handler(
      new Request('https://worker.example/missing', {
        method: 'OPTIONS',
        headers: {
          origin: 'https://app.example',
          'access-control-request-method': 'GET',
        },
      }),
    )
    assert.equal(preflight.status, 204)
    assert.equal(preflight.headers.get('access-control-allow-origin'), 'https://app.example')

    const rejected = await handler(
      new Request('https://worker.example/api', { headers: { origin: 'https://evil.example' } }),
    )
    assert.equal(rejected.headers.get('access-control-allow-origin'), null)
    assert.match(rejected.headers.get('vary'), /Origin/i)
  })

  it('sends CORS negotiation headers on preflight responses only', async () => {
    // `Allow-Methods`, `Allow-Headers`, and `Max-Age` answer a preflight
    // question and a browser reads them nowhere else. Asserted as one pair so a
    // header that crosses the line has to move this test with it, and so this
    // host cannot drift from the Rust middleware that serves the same apps.
    const preflightOnly = [
      'access-control-allow-methods',
      'access-control-allow-headers',
      'access-control-max-age',
    ]
    const both = ['access-control-allow-origin', 'access-control-allow-credentials']
    const handler = middlewareHandler({
      builtin: {
        cors: {
          origins: ['https://app.example'],
          methods: ['GET', 'OPTIONS'],
          headers: ['Content-Type'],
          credentials: true,
          maxAge: 3600,
        },
      },
    })

    const preflight = await handler(
      new Request('https://worker.example/api', {
        method: 'OPTIONS',
        headers: {
          origin: 'https://app.example',
          'access-control-request-method': 'GET',
        },
      }),
    )
    for (const name of [...preflightOnly, ...both]) {
      assert.ok(preflight.headers.has(name), `preflight response is missing ${name}`)
    }

    const actual = await handler(
      new Request('https://worker.example/api', {
        headers: { origin: 'https://app.example' },
      }),
    )
    for (const name of preflightOnly) {
      assert.equal(actual.headers.get(name), null, `actual response should not carry ${name}`)
    }
    for (const name of both) {
      assert.ok(actual.headers.has(name), `actual response is missing ${name}`)
    }
    assert.match(actual.headers.get('vary'), /Origin/i)
  })

  it('enforces bounded window rate limits with platform and explicit header keys', async () => {
    const handler = middlewareHandler({
      builtin: { rate: { max: 1, window: 60, key: 'header:x-tenant' } },
    })
    const request = (tenant) =>
      new Request('https://worker.example/api', { headers: { 'x-tenant': tenant } })

    assert.equal((await handler(request('a'))).status, 200)
    const limited = await handler(request('a'))
    assert.equal(limited.status, 429)
    assert.equal(limited.headers.get('retry-after'), '60')
    assert.equal((await handler(request('b'))).status, 200)
  })
})

describe('serverless i18n and dynamic image parity', () => {
  const i18n = {
    locales: ['en', 'th', 'fr-FR'],
    defaultLocale: 'en',
    localeParam: 'lang',
    detectLocale: true,
    cookie: 'RUVYXA_LOCALE',
  }

  it('redirects to a supported locale and injects lang and hreflang metadata', async () => {
    const route = pageRoute('localized-about', '/[lang]/about')
    const handler = createHandler({
      routes: [route],
      i18n,
      importPage: async () => ({
        render: async () => '<!doctype html><html><head></head><body>About</body></html>',
      }),
      importApi: async () => ({}),
    })

    const redirected = await handler(
      new Request('https://example.com/about', {
        headers: { 'accept-language': 'fr-CA,th;q=.8' },
      }),
    )
    assert.equal(redirected.status, 307)
    assert.equal(redirected.headers.get('location'), '/fr-FR/about')

    const localized = await handler(new Request('https://example.com/th/about'))
    const html = await localized.text()
    assert.match(html, /<html lang="th">/)
    assert.match(html, /hreflang="fr-FR" href="\/fr-FR\/about"/)
    assert.match(html, /hreflang="x-default" href="\/en\/about"/)
  })

  it('redirects unprefixed URLs from the shared conformance table', async () => {
    // `locale_redirect_path` in crates/ruvyxa_dev_server/src/i18n.rs answers the
    // same table. Two of these cases used to be answered differently here, and
    // neither showed up because every other test in both languages sets
    // `detectLocale: true` and asks for an ordinary path.
    const fixture = JSON.parse(
      readFileSync(
        path.join(workspaceRoot, 'tests/fixtures/i18n-routing-conformance.json'),
        'utf8',
      ),
    )
    assert.ok(fixture.cases.length > 0, 'the table must carry cases')
    const routes = fixture.routes.map((routePath) => pageRoute(`app${routePath}/page`, routePath))

    for (const testCase of fixture.cases) {
      const handler = createHandler({
        routes,
        i18n: { ...fixture.config, detectLocale: testCase.detectLocale },
        importPage: async () => ({ render: async () => '<html></html>' }),
        importApi: async () => ({}),
      })
      // `query` is absent on a case that sends none, and an empty string is a
      // case in its own right: `/about?` must redirect without the `?`.
      const search = testCase.query === undefined ? '' : `?${testCase.query}`
      const response = await handler(
        new Request(`https://example.com${testCase.path}${search}`, {
          method: testCase.method,
          headers: testCase.headers,
        }),
      )
      // The header is read as-is rather than through `new URL()`: the value is
      // a root-relative path on both hosts now, and resolving it against a base
      // would hide an absolute one built from the request's `Host`.
      const location = response.status === 307 ? response.headers.get('location') : null
      assert.equal(location, testCase.redirect, `${testCase.path} ${testCase.$why}`)
    }
  })

  it('builds the locale redirect without consulting the client Host header', async () => {
    // RTMS-04. `Response.redirect()` demands an absolute URL, so this host
    // reached for `new URL(redirect, request.url)` and inherited whatever origin
    // the transport derived from the raw `Host` header -- which on the
    // standalone server is the client's. The native host
    // (`render_pipeline.rs`'s locale redirect) sends the path alone, and RFC
    // 9110 has allowed a relative `Location` since 2014. The realistic harm is
    // cache poisoning: the response carries no `Vary: Host`, so a shared cache
    // keyed on path alone can store the forged target and serve it to real
    // visitors.
    const handler = createHandler({
      routes: [pageRoute('localized-home', '/[lang]')],
      i18n,
      importPage: async () => ({ render: async () => '<html></html>' }),
      importApi: async () => ({}),
    })

    const response = await handler(
      new Request('https://example.com/', { headers: { host: 'evil.example' } }),
    )
    assert.equal(response.status, 307)
    assert.equal(response.headers.get('location'), '/en')
    assert.ok(
      !response.headers.get('location').includes('://'),
      'the redirect target must carry no origin at all',
    )
  })

  it('validates image requests before invoking a platform optimizer', async () => {
    const calls = []
    const handler = createHandler({
      routes: [],
      importPage: async () => ({}),
      importApi: async () => ({}),
      optimizeImage: async (_request, input) => {
        calls.push(input)
        return new Response('image', { headers: { 'content-type': 'image/webp' } })
      },
    })

    const optimized = await handler(
      new Request('https://example.com/__ruvyxa/image?src=%2Fhero.jpg&w=640&q=75'),
    )
    assert.equal(optimized.status, 200)
    assert.deepEqual(calls, [{ src: '/hero.jpg', width: 640, quality: 75 }])
    assert.equal(
      (
        await handler(
          new Request('https://example.com/__ruvyxa/image?src=https://evil.test/a.jpg&w=640'),
        )
      ).status,
      400,
    )
    assert.equal(calls.length, 1)
  })

  it('refuses an image src the native host refuses, segment for segment', async () => {
    // `rejects_external_and_traversing_sources_before_io` and the `?`/`#` guard
    // in `crates/ruvyxa_dev_server/src/dynamic_image.rs` decide the same thing
    // under `ruvyxa dev` and `ruvyxa start`. This host used to check only the
    // leading slash and the backslash, so `/../a.png` and `/a.png?x=1` reached
    // the adapter's optimizer — one URL answering on one deployment target and
    // 400ing on another, with nothing in the markup or the status to show it.
    const calls = []
    const handler = createHandler({
      routes: [],
      importPage: async () => ({}),
      importApi: async () => ({}),
      optimizeImage: async (_request, input) => {
        calls.push(input)
        return new Response('image', { headers: { 'content-type': 'image/webp' } })
      },
    })

    for (const src of [
      '/../a.png',
      '/a/../../b.png',
      '/a/./b.png',
      '//host/a.png',
      '/a:b.png',
      '/a.png?x=1',
      '/a.png#frag',
      '/',
    ]) {
      const response = await handler(
        new Request(`https://example.com/__ruvyxa/image?src=${encodeURIComponent(src)}&w=640`),
      )
      assert.equal(response.status, 400, src)
    }
    assert.equal(calls.length, 0)
  })
})

describe('prerender cache path mapping', () => {
  it('maps ordinary request paths to the build writer layout', () => {
    assert.equal(prerenderRelativePath('/'), 'index.html')
    assert.equal(prerenderRelativePath('/about'), 'about/index.html')
    assert.equal(prerenderRelativePath('/blog/hello-world'), 'blog/hello-world/index.html')
    assert.equal(prerenderRelativePath('/a/b/'), 'a/b/index.html')
    // Percent-encoding is preserved, because the build writer stores the raw
    // route path. Decoding here would look for a file that was never written.
    assert.equal(prerenderRelativePath('/docs/a%20b'), 'docs/a%20b/index.html')
  })

  it('refuses paths that could escape or misname the cache directory', () => {
    for (const pathname of [
      '/a/../b',
      '/../etc/passwd',
      '/a/./b',
      '/a\\b',
      '/a:b',
      'no-leading-slash',
      '',
    ]) {
      assert.equal(prerenderRelativePath(pathname), null, pathname)
    }
    assert.equal(prerenderRelativePath(undefined), null)
  })
})

describe('ISR cache freshness', () => {
  it('serves the page it rendered even when the cache write fails', async () => {
    // A read-only runtime filesystem is ordinary in production: a container run
    // with --read-only, a pod with readOnlyRootFilesystem, Cloud Run, a Lambda
    // bundle outside /tmp. Storing the render is a cache optimization, so a
    // write that throws has to degrade to rendering every time — never to a
    // 500 for a page that had already rendered correctly.
    let renders = 0
    const route = pageRoute('isr', '/isr', 'isr')
    route.render.revalidate = 60
    const handler = createHandler({
      routes: [route],
      importPage: async () => ({
        render: async () => {
          renders += 1
          return '<html>rendered</html>'
        },
      }),
      importApi: async () => ({}),
      readPrerendered: () => null,
      writePrerendered: () => {
        throw Object.assign(new Error('EROFS: read-only file system'), { code: 'EROFS' })
      },
    })

    const first = await handler(new Request('http://localhost/isr'))
    assert.equal(first.status, 200)
    assert.equal(await first.text(), '<html>rendered</html>')

    // Nothing was stored, so the next request renders again rather than
    // serving a cached document that does not exist.
    const second = await handler(new Request('http://localhost/isr'))
    assert.equal(second.status, 200)
    assert.equal(await second.text(), '<html>rendered</html>')
    assert.equal(renders, 2)
  })

  it('does not regenerate a fresh cache hit', async () => {
    let renders = 0
    const route = pageRoute('isr', '/isr', 'isr')
    route.render.revalidate = 60
    const handler = createHandler({
      routes: [route],
      importPage: async () => ({
        render: async () => {
          renders += 1
          return '<html>new</html>'
        },
      }),
      importApi: async () => ({}),
      readPrerendered: () => ({ html: '<html>cached</html>', stale: false }),
      writePrerendered: () => {},
    })

    const response = await handler(new Request('http://localhost/isr'))
    await new Promise((resolve) => setImmediate(resolve))

    assert.equal(await response.text(), '<html>cached</html>')
    assert.equal(renders, 0)
  })

  it('coalesces concurrent regeneration for a stale cache entry', async () => {
    let renders = 0
    let writes = 0
    let releaseRender
    const renderGate = new Promise((resolve) => {
      releaseRender = resolve
    })
    const route = pageRoute('isr', '/isr', 'isr')
    route.render.revalidate = 60
    const handler = createHandler({
      routes: [route],
      importPage: async () => ({
        render: async () => {
          renders += 1
          await renderGate
          return '<html>new</html>'
        },
      }),
      importApi: async () => ({}),
      readPrerendered: () => ({ html: '<html>stale</html>', stale: true }),
      writePrerendered: () => {
        writes += 1
      },
    })

    const runtimeContext = { waitUntil() {} }
    const [first, second] = await Promise.all([
      handler(new Request('http://localhost/isr'), runtimeContext),
      handler(new Request('http://localhost/isr'), runtimeContext),
    ])
    assert.equal(await first.text(), '<html>stale</html>')
    assert.equal(await second.text(), '<html>stale</html>')
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(renders, 1)

    releaseRender()
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(writes, 1)
  })

  it('waits for asynchronous background persistence when no lifetime hook exists', async () => {
    let releaseWrite
    let responseSettled = false
    const writeGate = new Promise((resolve) => {
      releaseWrite = resolve
    })
    const route = pageRoute('isr', '/isr', 'isr')
    route.render.revalidate = 60
    const handler = createHandler({
      routes: [route],
      importPage: async () => ({ render: async () => '<html>new</html>' }),
      importApi: async () => ({}),
      readPrerendered: () => ({ html: '<html>stale</html>', stale: true }),
      writePrerendered: async () => writeGate,
    })

    const responsePromise = handler(new Request('http://localhost/isr')).finally(() => {
      responseSettled = true
    })
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(responseSettled, false)

    releaseWrite()
    const response = await responsePromise
    assert.equal(await response.text(), '<html>stale</html>')
  })
})

/**
 * What a `readPrerendered` answer means and what each strategy does with it,
 * replayed from the fixture the worker and the Axum host replay too.
 *
 * The rule lived in three places with no fixture and drifted twice: the worker
 * called a bare string fresh while `normalizeCacheEntry` here called it stale,
 * and the Axum host treated a stale document as a miss while `serveIncremental`
 * here serves it and refreshes behind the response.
 */
describe('stored document conformance', () => {
  function handlerServing(strategy, answer, counters) {
    const route = pageRoute('stored', '/stored', strategy)
    route.render.revalidate = 60
    return createHandler({
      routes: [route],
      importPage: async () => ({
        render: async () => {
          counters.renders += 1
          return '<html>rendered</html>'
        },
      }),
      importApi: async () => ({}),
      readPrerendered: () => structuredClone(answer),
      writePrerendered: () => {},
    })
  }

  for (const entry of storedDocumentConformance.answers) {
    it(`answers: ${entry.name}`, async () => {
      const counters = { renders: 0 }
      const handler = handlerServing('isr', entry.answer, counters)
      const response = await handler(new Request('http://localhost/stored'))
      await new Promise((resolve) => setImmediate(resolve))
      if (entry.expect.kind === 'miss') {
        assert.equal(await response.text(), '<html>rendered</html>')
        assert.equal(counters.renders, 1, 'a miss renders')
      } else {
        assert.equal(await response.text(), entry.expect.html)
        assert.equal(
          counters.renders,
          entry.expect.stale ? 1 : 0,
          'only a stale document refreshes',
        )
      }
    })
  }

  for (const entry of storedDocumentConformance.serving) {
    it(`serving: ${entry.name}`, async () => {
      const counters = { renders: 0 }
      const answer =
        entry.document.kind === 'held'
          ? { html: entry.document.html, stale: entry.document.stale }
          : null
      const handler = handlerServing(entry.strategy, answer, counters)
      const response = await handler(new Request('http://localhost/stored'))
      await new Promise((resolve) => setImmediate(resolve))
      if (entry.expect.served) {
        assert.equal(await response.text(), entry.document.html)
        assert.equal(counters.renders, entry.expect.refresh ? 1 : 0)
      } else {
        assert.equal(await response.text(), '<html>rendered</html>')
        assert.equal(counters.renders, 1)
      }
    })
  }
})

describe('bounded path revalidation state', () => {
  it('keeps forced prerender claims through render and persistence failures', async () => {
    for (const strategy of ['ssg', 'isr', 'ppr', 'csr']) {
      let renders = 0
      let reads = 0
      let writes = 0
      let stored = '<html>stale</html>'
      const route = pageRoute(`page-${strategy}`, '/page', strategy)
      route.render.revalidate = 60
      const handler = createHandler({
        routes: [
          { id: 'invalidate', path: '/invalidate', kind: 'api', file: 'api.ts', render: {} },
          route,
        ],
        importApi: async () => ({
          GET: () => {
            revalidatePath('/page')
            return new Response(null, { status: 204 })
          },
        }),
        importPage: async () => ({
          render: async () => {
            renders++
            if (renders === 1) throw new Error(`failed ${strategy} render`)
            return '<html>fresh</html>'
          },
        }),
        readPrerendered: () => {
          reads++
          return strategy === 'isr' ? { html: stored, stale: false } : stored
        },
        writePrerendered: async (_pathname, html) => {
          writes++
          if (writes === 1) throw new Error(`failed ${strategy} persistence`)
          stored = html
        },
      })

      assert.equal(
        (await handler(new Request('http://localhost/invalidate'))).status,
        204,
        strategy,
      )
      assert.equal((await handler(new Request('http://localhost/page'))).status, 500, strategy)
      assert.equal(reads, 0, strategy)
      assert.equal((await handler(new Request('http://localhost/page'))).status, 500, strategy)
      assert.equal(reads, 0, strategy)
      assert.equal((await handler(new Request('http://localhost/page'))).status, 200, strategy)
      assert.equal(writes, 2, strategy)

      const afterAck = await handler(new Request('http://localhost/page'))
      assert.equal(afterAck.status, 200, strategy)
      if (strategy === 'ppr') {
        assert.equal(writes, 2, strategy)
        assert.equal(reads, 0, strategy)
      } else {
        assert.equal(await afterAck.text(), '<html>fresh</html>', strategy)
        assert.equal(reads, 1, strategy)
      }
    }
  })

  it('keeps a failed SSR claim and acknowledges only a successful render', async () => {
    let renders = 0
    const handler = createHandler({
      routes: [
        { id: 'invalidate', path: '/invalidate', kind: 'api', file: 'api.ts', render: {} },
        pageRoute('ssr', '/page', 'ssr'),
      ],
      importApi: async () => ({
        GET: () => {
          revalidatePath('/page')
          return new Response(null, { status: 204 })
        },
      }),
      importPage: async () => ({
        render: async () => {
          renders++
          if (renders === 1) throw new Error('failed SSR render')
          return '<html>fresh</html>'
        },
      }),
    })

    await handler(new Request('http://localhost/invalidate'))
    assert.equal((await handler(new Request('http://localhost/page'))).status, 500)
    assert.equal((await handler(new Request('http://localhost/page'))).status, 200)
  })

  it('fails closed for an oversized payload from an older worker contract', async () => {
    let prerenderReads = 0
    const handler = createHandler({
      routes: [
        { id: 'legacy', path: '/legacy', kind: 'api', file: 'api.ts', render: {} },
        pageRoute('target', '/target', 'ssg'),
      ],
      importApi: async () => ({
        GET: () => {
          const context = globalThis.__RUVYXA_REQUEST_CONTEXT__.peek()
          for (let index = 0; index <= revalidationConformance.maxPathsPerRequest; index++) {
            context.revalidate.add(`/legacy/${index}`)
          }
          return new Response(null, { status: 204 })
        },
      }),
      importPage: async () => ({ render: async () => '<html>fresh</html>' }),
      readPrerendered: () => {
        prerenderReads++
        return '<html>stale</html>'
      },
      writePrerendered() {},
    })

    assert.equal((await handler(new Request('http://localhost/legacy'))).status, 204)
    const target = await handler(new Request('http://localhost/target'))
    assert.equal(await target.text(), '<html>fresh</html>')
    assert.equal(prerenderReads, 0)
  })

  it('fails closed instead of dropping invalidations when pending paths exceed the bound', async () => {
    let renders = 0
    let prerenderReads = 0
    const routes = [
      { id: 'invalidate', path: '/invalidate/[batch]', kind: 'api', file: 'api.ts', render: {} },
      pageRoute('target', '/target', 'ssg'),
    ]
    const handler = createHandler({
      routes,
      importApi: async () => ({
        GET: ({ params }) => {
          for (let index = 0; index < revalidationConformance.maxPathsPerRequest; index++) {
            revalidatePath(`/pending/${params.batch}/${index}`)
          }
          return new Response(null, { status: 204 })
        },
      }),
      importPage: async () => ({
        render: async () => {
          renders++
          return '<html>fresh</html>'
        },
      }),
      readPrerendered: () => {
        prerenderReads++
        return '<html>stale</html>'
      },
      writePrerendered() {},
    })

    const batchesToCapacity = Math.floor(
      revalidationConformance.maxPendingExactPaths / revalidationConformance.maxPathsPerRequest,
    )
    for (let batch = 0; batch < batchesToCapacity; batch++) {
      const response = await handler(new Request(`http://localhost/invalidate/${batch}`))
      assert.equal(response.status, 204)
    }

    // Updating generations for paths already pending at exact capacity must
    // not be mistaken for adding a new key and trigger global bypass.
    assert.equal((await handler(new Request('http://localhost/invalidate/0'))).status, 204)
    const beforeOverflow = await handler(new Request('http://localhost/target'))
    assert.equal(await beforeOverflow.text(), '<html>stale</html>')
    assert.equal(prerenderReads, 1)
    assert.equal(renders, 0)

    assert.equal(
      (await handler(new Request(`http://localhost/invalidate/${batchesToCapacity}`))).status,
      204,
    )

    const first = await handler(new Request('http://localhost/target'))
    const second = await handler(new Request('http://localhost/target'))
    assert.equal(await first.text(), '<html>fresh</html>')
    assert.equal(await second.text(), '<html>fresh</html>')
    assert.equal(prerenderReads, 1)
    assert.equal(renders, 2)
  })
})

describe('optional catch-all parity with the dev server', () => {
  it('omits the parameter at the parent route instead of using an empty array', async () => {
    const rendered = []
    const handler = handlerFor([pageRoute('shop', '/shop/[[...slug]]')], rendered)

    await handler(new Request('http://localhost/shop'))
    // Documented contract: undefined at the parent, string[] below it. The dev
    // server's router omits the key, so a deploy must not report [].
    assert.equal(rendered.at(-1).params.slug, undefined)
    assert.equal(Object.hasOwn(rendered.at(-1).params, 'slug'), false)

    await handler(new Request('http://localhost/shop/clothes/tops'))
    assert.deepEqual(rendered.at(-1).params.slug, ['clothes', 'tops'])
  })
})

/**
 * The half of `/__ruvyxa/rsc` a status code on the payload endpoint cannot see.
 *
 * The native host answers both verbs on this path: `GET` renders a route's
 * payload, `POST` runs one of the server functions that route exposes. The
 * deployed handler answered `GET` and refused `POST` with a `405`, so clicking
 * anything wired to a server function on a deployed server-components page
 * threw `Connection closed.` in the browser and blanked the document, while the
 * same page worked under `ruvyxa dev` and `ruvyxa start`.
 */
describe('server functions on a deployed server-components route', () => {
  function rscRoute() {
    return {
      id: 'app/rsc/page',
      path: '/rsc',
      kind: 'page',
      file: 'app/rsc/page.tsx',
      render: { strategy: 'ssr', serverComponents: true },
    }
  }

  function handlerWith(pageModule) {
    return createHandler({
      routes: [rscRoute()],
      importPage: async () => pageModule,
      importApi: async () => ({}),
    })
  }

  /**
   * The request a browser makes, `Origin` included.
   *
   * This endpoint's origin check is fail-closed when neither `Origin` nor
   * `Sec-Fetch-Site` is present, which is what `/__ruvyxa/action` has always
   * done and what `/__ruvyxa/rsc` now does on both hosts. A probe that sends
   * neither is not the request any real caller makes: both browser halves of
   * this endpoint — `rsc-client-runtime.mjs` and `@ruvyxa/react`'s router — run
   * in a browser, and a browser always sends one of the two.
   */
  function actionRequest(body = 'ARGS', headers = {}) {
    return new Request('http://localhost/__ruvyxa/rsc?path=/rsc', {
      method: 'POST',
      headers: {
        host: 'localhost',
        origin: 'http://localhost',
        'x-ruvyxa-rsc': '1',
        'x-ruvyxa-action': 'ruv:s_abc#run',
        ...headers,
      },
      body,
    })
  }

  it('runs the named function and answers with its payload', async () => {
    const calls = []
    const handler = handlerWith({
      render: async () => '<html></html>',
      rscAction: async (call) => {
        calls.push(call)
        return '0:"ok"\n'
      },
    })

    const response = await handler(actionRequest())

    assert.equal(response.status, 200)
    assert.equal(response.headers.get('content-type'), 'text/x-component; charset=utf-8')
    // Never cached and never shared: the answer belongs to one visitor, and the
    // header gate is what keeps a cross-origin page from asking at all.
    assert.equal(response.headers.get('cache-control'), 'private, no-store')
    assert.equal(response.headers.get('vary'), 'x-ruvyxa-rsc')
    assert.equal(await response.text(), '0:"ok"\n')
    assert.deepEqual(calls, [{ reference: 'ruv:s_abc#run', body: 'ARGS' }])
  })

  it('refuses a call that names no reference', async () => {
    const handler = handlerWith({ render: async () => '', rscAction: async () => '' })
    const response = await handler(actionRequest('ARGS', { 'x-ruvyxa-action': '' }))
    assert.equal(response.status, 400)
  })

  it('refuses a call a cross-origin page could make', async () => {
    const handler = handlerWith({ render: async () => '', rscAction: async () => '' })
    const request = new Request('http://localhost/__ruvyxa/rsc?path=/rsc', {
      method: 'POST',
      headers: { 'x-ruvyxa-action': 'ruv:s_abc#run' },
      body: 'ARGS',
    })
    assert.equal((await handler(request)).status, 400)
  })

  it('refuses a call whose Origin names another site', async () => {
    // RUV-H5. The navigation header used to be the whole cross-origin defence
    // here, on the premise that no preflight answers it — but the built-in CORS
    // layer wraps this handler and answers preflights before dispatch, so a
    // project that enabled CORS for its own API silently made the header
    // settable and handed any page the visitor's server functions.
    const handler = handlerWith({ render: async () => '', rscAction: async () => '' })
    const response = await handler(actionRequest('ARGS', { origin: 'https://evil.test' }))
    assert.equal(response.status, 403)
  })

  it('refuses a call Sec-Fetch-Site reports as cross-site', async () => {
    const handler = handlerWith({ render: async () => '', rscAction: async () => '' })
    const response = await handler(actionRequest('ARGS', { 'sec-fetch-site': 'cross-site' }))
    assert.equal(response.status, 403)
  })

  it('refuses the same call once past its ceiling, and keeps the budget per function', async () => {
    // Keyed `rsc:{client}:{canonical path}:{reference}`, the shape
    // `rsc_action_rate_limit_key` uses: a page issuing several server-function
    // calls in one interaction spends a budget per function rather than one
    // between them.
    const handler = createHandler({
      routes: [rscRoute()],
      importPage: async () => ({ render: async () => '', rscAction: async () => '0:"ok"\n' }),
      importApi: async () => ({}),
      security: { actionRateLimit: { max: 2, window: 60 } },
    })

    const statuses = []
    for (let attempt = 0; attempt < 3; attempt++) {
      statuses.push((await handler(actionRequest())).status)
    }
    assert.deepEqual(statuses, [200, 200, 429])

    // A second reference on the same route has its own bucket.
    const other = await handler(actionRequest('ARGS', { 'x-ruvyxa-action': 'ruv:s_abc#save' }))
    assert.equal(other.status, 200)
  })

  it('reports a route with no server functions as 501 rather than 404', async () => {
    // Distinguishable on purpose: the route exists and renders through the
    // pipeline, it simply declares nothing callable, so the build had nothing
    // to compile an action bundle from.
    const handler = handlerWith({ render: async () => '', rscPayload: async () => '' })
    const response = await handler(actionRequest())
    assert.equal(response.status, 501)
    assert.match(await response.text(), /RUV1866/)
  })

  it('hands a form posted without JavaScript to the render', async () => {
    // React writes the reference into hidden fields rather than into an
    // `action` attribute, so a no-JS submission posts to the page's own URL.
    // Nothing here recognised that, and the page re-rendered with its initial
    // state — a 200 indistinguishable from a form that was never submitted.
    const seen = []
    const handler = handlerWith({
      render: async (ctx) => {
        seen.push(ctx.formData)
        return `<html>${ctx.formData ? 'submitted' : 'initial'}</html>`
      },
    })

    const body = new URLSearchParams({ $ACTION_REF_1: '', version: '1.0.30' })
    const response = await handler(
      new Request('http://localhost/rsc', {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body,
      }),
    )

    assert.equal(await response.text(), '<html>submitted</html>')
    // A submission is one visitor's answer whatever the route's strategy says.
    assert.equal(response.headers.get('cache-control'), 'no-store')
    assert.equal(seen.at(-1).get('version'), '1.0.30')

    await handler(new Request('http://localhost/rsc'))
    assert.equal(seen.at(-1), null)
  })

  it('leaves a form posted to an ordinary page alone', async () => {
    const seen = []
    const handler = createHandler({
      routes: [{ ...rscRoute(), render: { strategy: 'ssr' } }],
      importPage: async () => ({
        render: async (ctx) => {
          seen.push(ctx.formData)
          return '<html></html>'
        },
      }),
      importApi: async () => ({}),
    })

    await handler(
      new Request('http://localhost/rsc', {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ version: '1' }),
      }),
    )
    assert.equal(seen.at(-1), null)
  })
})

describe('internal framework headers stay inside the framework', () => {
  // `x-ruvyxa-realtime-event` is a transport between `worker-pool.mjs` and the
  // Rust host, which reads it off the worker's response and strips it before
  // anything reaches the network. `runAction` used to attach it for both
  // hosts, and this one has no reader downstream: a function's response is
  // what the browser receives. So every action on a realtime-declaring route
  // published its channel list, its name, and every key it passed to
  // `invalidate()` -- application-chosen strings such as `user:42:cart` -- to
  // whoever called it. Both native hosts stripped it and this one did not,
  // which is the two-request-hosts shape `AGENTS.md` warns about.
  const INTERNAL_RESPONSE_HEADERS = ['x-ruvyxa-realtime-event']

  function realtimeAction() {
    const submit = async (_input, { invalidate }) => {
      invalidate('user:42:cart')
      return { ok: true }
    }
    submit.ruvyxa = { kind: 'action', realtime: { channels: ['orders'] } }
    return submit
  }

  async function invokeRealtimeAction() {
    const fixture = JSON.parse(
      readFileSync(path.join(workspaceRoot, 'tests/fixtures/action-contract.json'), 'utf8'),
    )
    const handler = createHandler({
      routes: [{ ...pageRoute(fixture.routeId, '/target'), actionReferenceId: fixture.expected }],
      importPage: async () => ({}),
      importApi: async () => ({}),
      importAction: async () => ({ submit: realtimeAction() }),
    })
    return handler(
      new Request(
        `http://localhost/__ruvyxa/action?path=/target&name=submit&id=${encodeURIComponent(fixture.expected)}`,
        {
          method: 'POST',
          headers: {
            'content-type': 'application/json',
            host: 'localhost',
            origin: 'http://localhost',
            'sec-fetch-site': 'same-origin',
            'x-ruvyxa-action-nonce': '0f1e2d3c4b5a6978',
          },
          body: '{}',
        },
      ),
    )
  }

  it('answers a realtime-declaring action without leaking its event', async () => {
    const response = await invokeRealtimeAction()
    assert.equal(response.status, 200)
    for (const name of INTERNAL_RESPONSE_HEADERS) {
      assert.equal(response.headers.get(name), null, `${name} must not reach the client`)
    }
  })

  it('publishes no x-ruvyxa header a browser was not meant to read', async () => {
    const response = await invokeRealtimeAction()
    // Broader than the list above on purpose: the defect was a header nobody
    // thought about on this host, so the assertion is over the whole namespace
    // rather than over the one name that went wrong. `x-ruvyxa-isr` is the
    // single member a client is meant to see -- it reports cache status.
    const published = [...response.headers.keys()].filter(
      (name) => name.startsWith('x-ruvyxa-') && name !== 'x-ruvyxa-isr',
    )
    assert.deepEqual(published, [])
  })

  it('still hands the event to a host that has somewhere to put it', async () => {
    const { response, realtimeEvent } = await runAction({
      module: { submit: realtimeAction() },
      actionName: 'submit',
      payload: '{}',
      contentType: 'application/json',
      requestPath: '/target',
      headerPairs: [['host', 'localhost']],
    })
    assert.equal(response.headers.get('x-ruvyxa-realtime-event'), null)
    assert.deepEqual(realtimeEvent, {
      version: 1,
      type: 'action',
      channels: ['orders'],
      action: 'submit',
      path: '/target',
      invalidated: ['user:42:cart'],
    })
  })
})

/**
 * The document validator, on the host every deployed build runs.
 *
 * `documentCacheControl` tells a browser to revalidate a stored document before
 * every reuse, and this handler gave it nothing to revalidate against — so a
 * page a reader already held was sent again in full on every navigation. The
 * membership half of the rule is `tests/fixtures/deploy-output-conformance.json`,
 * replayed in `deploy-manifest.test.mjs`; what is measured here is the answer.
 */
describe('deployed document validators', () => {
  const storedDocument = '<html><body>stored</body></html>'

  function documentHandler(strategy, overrides = {}) {
    return createHandler({
      routes: [pageRoute('page', '/', strategy)],
      importPage: async () => ({ render: async () => '<html><body>rendered</body></html>' }),
      importApi: async () => ({}),
      readPrerendered: () => ({ html: storedDocument, stale: false }),
      ...overrides,
    })
  }

  const get = (handler, headers = {}) =>
    handler(new Request('http://localhost/', { headers: { host: 'localhost', ...headers } }))

  for (const strategy of ['ssg', 'csr', 'isr']) {
    it(`answers a revalidation of a ${strategy} document with 304`, async () => {
      const handler = documentHandler(strategy)
      const first = await get(handler)
      assert.equal(first.status, 200)
      const etag = first.headers.get('etag')
      assert.ok(etag, `a ${strategy} document must carry a validator`)
      assert.match(etag, /^W\//, 'weak: the same document is served identity or compressed')

      const revalidated = await get(handler, { 'if-none-match': etag })
      assert.equal(revalidated.status, 304)
      assert.equal(revalidated.headers.get('etag'), etag)
      // What a cache still needs: when to ask again. What it must not be given:
      // a length describing a body that is not there.
      assert.equal(
        revalidated.headers.get('cache-control'),
        first.headers.get('cache-control'),
        'the 304 keeps the lifetime its 200 named',
      )
      assert.equal(revalidated.headers.get('content-length'), null)
      assert.equal(await revalidated.text(), '')
    })
  }

  it('never validates a per-request render', async () => {
    // An `ssr` document may carry one visitor's data. A validator on it invites
    // a 304 for a page that was rendered for somebody else.
    const response = await get(documentHandler('ssr', { readPrerendered: undefined }))
    assert.equal(response.status, 200)
    assert.equal(response.headers.get('etag'), null)
  })

  /**
   * The validator has to describe the bytes that leave, not the bytes the
   * strategy layer read.
   *
   * A plugin `http.onResponse` hook may replace the document body — the
   * first-party `pwa` plugin injects into every HTML response — so a validator
   * written where the document is read would name bytes nobody received, and
   * answer 304 for a document that changed.
   */
  it('validates the body a plugin actually left behind', async () => {
    const injected = '<html><body>stored+plugin</body></html>'
    const handler = documentHandler('ssg', {
      pluginHttp: async (request, next) => {
        const response = await next(request)
        if (!response.headers.get('content-type')?.includes('text/html')) return response
        const headers = new Headers(response.headers)
        await response.text()
        return new Response(injected, { status: response.status, headers })
      },
    })

    const first = await get(handler)
    assert.equal(await first.text(), injected)
    const etag = first.headers.get('etag')
    assert.ok(etag)

    // The validator the strategy layer would have produced must not match.
    const stale = await documentValidatorOf(storedDocument)
    assert.notEqual(etag, stale, 'the validator must describe the injected body')

    assert.equal((await get(handler, { 'if-none-match': etag })).status, 304)
    assert.equal((await get(handler, { 'if-none-match': stale })).status, 200)
  })

  it('publishes no internal marker header on a validated document', async () => {
    const response = await get(documentHandler('ssg'))
    const published = [...response.headers.keys()].filter(
      (name) => name.startsWith('x-ruvyxa-') && name !== 'x-ruvyxa-isr',
    )
    assert.deepEqual(published, [])
  })

  /** The validator this handler computes for a body, recomputed independently. */
  async function documentValidatorOf(body) {
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(body))
    const hex = Array.from(new Uint8Array(digest, 0, 8), (byte) =>
      byte.toString(16).padStart(2, '0'),
    ).join('')
    return `W/"${hex}"`
  }
})

/**
 * A document rendered for one visitor is never offered to a shared cache.
 *
 * `requestScoped` was computed correctly and read only by the two *store*
 * decisions — the ISR write and the forced-render write. Nothing consulted it
 * when the `cache-control` was set, so an `isr` page that called `cookies()`
 * left with `public, max-age=0, s-maxage=…, stale-while-revalidate=…` and one
 * visitor's personalised HTML in the body. A CDN given that stores it and
 * answers every later request for the URL from it. `draftMode()` is the
 * sharpest case: it exists to show unpublished content to an authorised
 * previewer, and its own docstring promises such a request is never served from
 * a cache.
 *
 * Both halves are pinned in one test on purpose. The store guarantee already
 * held; it has to keep holding, and a fix that traded one for the other would
 * still pass a test that measured only the header.
 */
describe('request-scoped documents are never shared-cacheable', () => {
  /**
   * A handler whose render reads request state, with nothing stored for the URL.
   *
   * `readPrerendered` answers `null` so every strategy reaches the render: a
   * stored document is not request-scoped, and serving one would measure
   * nothing. The accessor is the framework's own `cookies()` rather than a
   * stand-in, because what makes a render request-scoped is precisely that it
   * called it.
   */
  function scopedHandler(strategy, read = () => null) {
    const writes = []
    const handler = createHandler({
      routes: [pageRoute('page', '/', strategy)],
      importPage: async () => ({
        render: async () => {
          cookies().get('session')
          return '<html><body>one visitor</body></html>'
        },
      }),
      importApi: async () => ({}),
      readPrerendered: read,
      writePrerendered: async (pathname, html) => {
        writes.push({ pathname, html })
      },
    })
    return { handler, writes }
  }

  const get = (handler) =>
    handler(
      new Request('http://localhost/', { headers: { host: 'localhost', cookie: 'session=a' } }),
    )

  for (const strategy of ['ssg', 'csr', 'isr']) {
    it(`answers a request-scoped ${strategy} render with no-store and stores nothing`, async () => {
      const { handler, writes } = scopedHandler(strategy)
      const response = await get(handler)

      assert.equal(response.status, 200)
      const cacheControl = response.headers.get('cache-control')
      assert.equal(cacheControl, 'no-store', `${strategy} rendered for one visitor`)
      // Stated separately from the equality above: `s-maxage` is the directive
      // that hands the document to a shared cache, and it is the one that must
      // not survive any later rewording of the header.
      assert.doesNotMatch(cacheControl, /s-maxage/, 'no shared-cache lifetime')
      assert.doesNotMatch(cacheControl, /\bpublic\b/, 'not offered to a shared cache')
      assert.deepEqual(writes, [], 'a per-visitor document is never stored')
    })
  }

  it('still caches a render that read nothing', async () => {
    // The guard is about this response, not about the route: a page that never
    // touched request state keeps the lifetime its strategy names.
    const handler = createHandler({
      routes: [pageRoute('page', '/', 'isr')],
      importPage: async () => ({ render: async () => '<html><body>everyone</body></html>' }),
      importApi: async () => ({}),
      readPrerendered: () => null,
      writePrerendered: async () => {},
    })
    const response = await get(handler)
    assert.match(response.headers.get('cache-control'), /s-maxage=60/)
  })
})

/**
 * A log line is a record, and a caller must not be able to write one.
 *
 * `?name=` on the action endpoint is percent-decoded by `searchParams.get`, so
 * it arrives with whatever bytes the caller chose — and it was interpolated
 * straight into `console.error`. Measured before the fix:
 * `?name=bad%0Ainjected...` produced `[ruvyxa] Server action bad\ninjected...`,
 * a second line in the deployed log written by whoever sent the request. Every
 * adapter runs this handler, so it was every deployment.
 */
describe('log records', () => {
  const FORGED = '\ninjected: everything is fine'

  /** Run `body` with the console captured, and hand back what it wrote. */
  async function captureLogs(body) {
    const captured = []
    const originals = {}
    for (const level of ['log', 'info', 'warn', 'error']) {
      originals[level] = console[level]
      console[level] = (...args) => captured.push(args.map((value) => String(value)).join(' '))
    }
    try {
      await body()
    } finally {
      for (const [level, original] of Object.entries(originals)) console[level] = original
    }
    return captured
  }

  it('never lets a caller-supplied value open a second line', async () => {
    const failing = async () => {
      throw new Error('boom')
    }
    failing.ruvyxa = { kind: 'action' }
    const handler = createHandler({
      routes: [pageRoute('home', '/target')],
      importPage: async () => ({ render: async () => '<html></html>' }),
      importApi: async () => ({}),
      importAction: async () => ({ [`bad${FORGED}`]: failing }),
    })

    const logs = await captureLogs(() =>
      handler(
        new Request(
          `http://localhost/__ruvyxa/action?path=/target&name=${encodeURIComponent(`bad${FORGED}`)}`,
          {
            method: 'POST',
            headers: {
              'content-type': 'application/json',
              host: 'localhost',
              origin: 'http://localhost',
              'sec-fetch-site': 'same-origin',
            },
            body: '{}',
          },
        ),
      ),
    )

    assert.ok(logs.length > 0, 'the failure must still be reported')
    for (const line of logs) {
      assert.ok(
        !line.includes('\n') && !line.includes('\r'),
        `forged line: ${JSON.stringify(line)}`,
      )
    }
    // Still says which action failed, which is the reason the value is there.
    assert.ok(
      logs.some((line) => line.includes('bad')),
      logs.join(' | '),
    )
  })

  it('writes the request log as one JSON record when the host asks for it', async () => {
    const handler = createHandler({
      routes: [pageRoute('home', '/')],
      importPage: async () => ({ render: async () => '<html>home</html>' }),
      importApi: async () => ({}),
      middleware: { builtin: { log: true } },
      logFormat: 'json',
    })

    const logs = await captureLogs(() =>
      handler(new Request('http://localhost/', { headers: { host: 'localhost' } })),
    )
    const request = logs.map((line) => JSON.parse(line)).find((record) => record.msg === 'request')
    assert.ok(request, logs.join(' | '))
    assert.equal(request.level, 'info')
    assert.equal(request.path, '/')
    assert.equal(request.method, 'GET')
    assert.equal(typeof request.duration_ms, 'number')
  })

  it('leaves the request log human-readable by default', async () => {
    const handler = createHandler({
      routes: [pageRoute('home', '/')],
      importPage: async () => ({ render: async () => '<html>home</html>' }),
      importApi: async () => ({}),
      middleware: { builtin: { log: true } },
    })
    const logs = await captureLogs(() =>
      handler(new Request('http://localhost/', { headers: { host: 'localhost' } })),
    )
    assert.ok(
      logs.some((line) => line.startsWith('[ruvyxa] request ') && line.includes('path=/')),
      logs.join(' | '),
    )
  })
})

describe('rate limiter conformance with the native middleware', () => {
  // The deployed half of `tests/fixtures/rate-limit-conformance.json`. The
  // native half is `the_shared_rate_limit_conformance_table_is_answered_the_same_way`
  // in `crates/ruvyxa_middleware/src/builtin.rs`.
  //
  // Both hosts enforce one `middleware.builtin.rate` block and nothing held
  // them to one answer, so the same defect was fixed in the native limiter
  // while every deployed build still carried it: at capacity the sweep freed
  // only buckets whose whole window had elapsed, and the limiter then refused
  // every key it had not already seen.
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
    return spec === 'capacity' ? MAX_TRACKED_RATE_LIMIT_KEYS : spec
  }

  function floodKey(index) {
    return `flood-${String(index).padStart(5, '0')}`
  }

  /**
   * The request one key case describes.
   *
   * How the client is attributed is host-local: the native host reads a
   * transport peer, and a deployed function has none, so it reads the
   * forwarded chain. With no trusted proxies configured the rightmost hop is
   * the client, which is the address the case names.
   */
  function requestFor(testCase) {
    const headers = new Headers()
    const configured = valueOf(testCase.keyHeader)
    if (configured !== null && testCase.keyBy.startsWith('header:')) {
      headers.set(testCase.keyBy.slice('header:'.length), configured)
    }
    if (testCase.client !== null) headers.set('x-forwarded-for', testCase.client)
    return new Request('https://worker.example/api', { headers })
  }

  for (const testCase of contract.keyCases) {
    it(`keys: ${testCase.name}`, () => {
      const key = rateLimitKey(requestFor(testCase), testCase.keyBy, [], [])
      assert.ok(
        key.length <= contract.maxKeyLength,
        `a tracked key of ${key.length} exceeds the ${contract.maxKeyLength} bound`,
      )
      assert.equal(key, boundedKey(valueOf(testCase.identity)))
      if (testCase.distinctFrom !== undefined) {
        // Two clients that share a bucket can each limit the other, so an
        // identity the limiter must tell apart may never collapse into one key.
        assert.notEqual(key, boundedKey(valueOf(testCase.distinctFrom)))
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
            // Staggered start times inside the window, oldest first, so
            // "the least recently started bucket" names exactly one of them.
            startedAt:
              testCase.prefill.state === 'expired'
                ? now - testCase.windowSeconds * 1000 - 1000
                : now - (count - index),
          })
        }
      }

      for (const [index, request] of testCase.requests.entries()) {
        const refusal = consumeFixedWindow(
          buckets,
          request.identity,
          testCase.max,
          testCase.windowSeconds,
        )
        assert.equal(
          refusal === null,
          request.allowed,
          `request ${index} for ${request.identity} was ${refusal === null ? 'admitted' : 'refused'}`,
        )
        if (refusal !== null) assert.equal(refusal.status, 429)
      }

      assert.equal(buckets.size, countOf(testCase.expectTracked))
      for (const request of testCase.requests) {
        assert.ok(buckets.has(request.identity), `${request.identity} was not tracked`)
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
 * The shared preflight table, replayed against `createHandler`.
 *
 * `crates/ruvyxa_middleware/src/builtin.rs` replays the same cases against the
 * layered native stack. The two disagreed: the native host puts the limiter
 * outside CORS so an `OPTIONS` is charged, and this handler answered preflights
 * before the limiter saw them — so the same `rateLimit.max` bought a different
 * number of real requests depending on where a project was deployed, and a
 * cross-origin page sends one preflight for every request it cannot simplify.
 */
describe('a preflight against the shared rate-limit table', () => {
  const contract = JSON.parse(
    readFileSync(path.join(workspaceRoot, 'tests/fixtures/rate-limit-conformance.json'), 'utf8'),
  )

  for (const testCase of contract.preflightCases.cases) {
    it(testCase.name, async () => {
      const handler = createHandler({
        routes: [pageRoute('app/page', '/')],
        importPage: async () => ({ render: async () => '<html>ok</html>' }),
        importApi: async () => ({}),
        middleware: {
          builtin: {
            cors: { origins: ['https://app.example'], credentials: true, methods: ['GET', 'POST'] },
            rate: { max: testCase.max, window: testCase.windowSeconds, key: 'ip' },
          },
        },
      })

      const request = () =>
        new Request('https://deployed.example/', {
          method: testCase.preflight ? 'OPTIONS' : 'GET',
          headers: testCase.preflight
            ? {
                origin: 'https://app.example',
                'access-control-request-method': 'POST',
                'x-forwarded-for': '203.0.113.7',
              }
            : { origin: 'https://app.example', 'x-forwarded-for': '203.0.113.7' },
        })

      let refusal = null
      for (let attempt = 1; attempt <= testCase.requests; attempt += 1) {
        const response = await handler(request())
        if (attempt < testCase.expectRefusedAt) {
          assert.notEqual(response.status, 429, `request ${attempt} was refused early`)
        } else if (attempt === testCase.expectRefusedAt) {
          assert.equal(
            response.status,
            429,
            `request ${attempt} was not refused, so a preflight cost nothing`,
          )
          refusal = response
        }
      }

      assert.ok(refusal, 'the table names a refusal')
      if (testCase.expectAllowOrigin) {
        assert.equal(
          refusal.headers.get('access-control-allow-origin'),
          'https://app.example',
          'a refusal the browser cannot read is an opaque failure',
        )
      }
      if (testCase.expectNegotiationHeaders === false) {
        assert.equal(
          refusal.headers.get('access-control-allow-methods'),
          null,
          'a refusal is not a preflight answer',
        )
      }
    })
  }
})

/**
 * The JavaScript half of the shared query-normalisation table.
 *
 * `crates/ruvyxa_dev_server/src/i18n.rs` replays the same cases against its own
 * `encoded_query`. This side asserts what that implementation was written to
 * match: `URL.search`, which is what the deployed host actually uses when it
 * reattaches a query to a locale redirect.
 *
 * The expectations were measured against `URL` rather than derived from the
 * spec, and that mattered — a rule written from the spec was wrong in both
 * directions, encoding characters `URL` leaves alone and preserving the newline
 * pair, which `URL` deletes.
 */
describe('the shared query normalisation', () => {
  const contract = JSON.parse(
    readFileSync(path.join(workspaceRoot, 'tests/fixtures/i18n-routing-conformance.json'), 'utf8'),
  )

  it('is what URL.search does, for every case the native host replays', () => {
    assert.ok(contract.queryCases.cases.length > 0, 'an empty table asserts nothing')
    for (const testCase of contract.queryCases.cases) {
      assert.equal(
        new URL(`https://example.test/p?${testCase.query}`).search.slice(1),
        testCase.expect,
        testCase.name,
      )
    }
  })

  it('drops the bytes that would split a Location header', () => {
    const injected = contract.queryCases.cases.find((testCase) =>
      testCase.name.includes('carriage return'),
    )
    assert.ok(injected, 'the table has to keep a response-splitting case')
    assert.doesNotMatch(injected.expect, /[\r\n]/)
  })
})
