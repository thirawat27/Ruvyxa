import assert from 'node:assert/strict'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerModule = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')

const { createHandler, prerenderRelativePath } = await import(
  `file://${handlerModule.replaceAll('\\', '/')}`
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

  it('decodes catch-all segments like dynamic segments', async () => {
    const rendered = []
    const handler = handlerFor([pageRoute('docs', '/docs/[...path]')], rendered)

    const response = await handler(new Request('http://localhost/docs/a%20b/c'))
    assert.equal(response.status, 200)
    assert.deepEqual(rendered.at(-1).params.path, ['a b', 'c'])
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
    // The un-normalized request path still reaches render, like the dev server.
    assert.equal(rendered.at(-1).pathname, '/docs/a/')

    const duplicate = await handler(new Request('http://localhost/docs//a'))
    assert.equal(duplicate.status, 200)
    assert.deepEqual(rendered.at(-1).params.path, ['a'])

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
