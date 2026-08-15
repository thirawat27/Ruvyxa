import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerModule = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')
const coreServerModule = path.join(workspaceRoot, 'packages/@ruvyxa/core/dist/server.js')

const { createHandler, prerenderRelativePath } = await import(
  `file://${handlerModule.replaceAll('\\', '/')}`
)
const { revalidatePath } = await import(`file://${coreServerModule.replaceAll('\\', '/')}`)
const { actionReferenceId } = await import(
  `file://${path.join(workspaceRoot, 'packages/ruvyxa/runtime/action-runtime.mjs').replaceAll('\\', '/')}`
)
const revalidationConformance = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/revalidation-conformance.json'), 'utf8'),
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
    assert.equal(redirected.headers.get('location'), 'https://example.com/fr-FR/about')

    const localized = await handler(new Request('https://example.com/th/about'))
    const html = await localized.text()
    assert.match(html, /<html lang="th">/)
    assert.match(html, /hreflang="fr-FR" href="\/fr-FR\/about"/)
    assert.match(html, /hreflang="x-default" href="\/en\/about"/)
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
