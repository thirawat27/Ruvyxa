/**
 * `headers()`, `redirects()`, and `rewrites()` through the deployed handler.
 *
 * The rule semantics are held by `tests/fixtures/route-rules-conformance.json`
 * and replayed by the shared module in both languages; what this file covers is
 * the *stage*: where in the request the deployed host applies each list, that
 * a framework endpoint is exempt, that a rewrite is checked once and not
 * against its own result, and that an external rewrite is fetched. The native
 * host's stage is tested in `crates/ruvyxa_dev_server/src/render_pipeline.rs`.
 */
import assert from 'node:assert/strict'
import path from 'node:path'
import { afterEach, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const runtimeUrl = (file) =>
  `file://${path.join(workspaceRoot, 'packages/ruvyxa/runtime', file).replaceAll('\\', '/')}`
const { createHandler } = await import(runtimeUrl('serverless-handler.mjs'))

const page = (path) => ({
  path,
  kind: 'page',
  file: `app${path === '/' ? '' : path}/page.tsx`,
  render: { strategy: 'ssr' },
})
const ROUTES = [page('/'), page('/about'), page('/news'), page('/blog/[slug]')]

function handlerWith(rules) {
  return createHandler({
    routes: ROUTES,
    securityHeaders: false,
    // The handler imports a page by route id; what the render sees is the path
    // it is rendering — the rewritten one — and the params the route captured.
    importPage: async () => ({
      render: async ({ path, params }) =>
        `<html><body data-path="${path}" data-params='${JSON.stringify(params ?? {})}'></body></html>`,
    }),
    importApi: async () => ({}),
    importAction: async () => null,
    ...rules,
  })
}

const get = (handler, url, headers = {}) =>
  handler(new Request(`https://example.test${url}`, { headers }))

describe('headers() on the deployed host', () => {
  it('sets matching rules on the response, later rules winning', async () => {
    const handler = handlerWith({
      headers: [
        { source: '/:path*', headers: [{ key: 'x-hello', value: 'there' }] },
        { source: '/blog/:slug', headers: [{ key: 'x-hello', value: ':slug' }] },
      ],
    })
    const blog = await get(handler, '/blog/first')
    assert.equal(blog.headers.get('x-hello'), 'first')
    const about = await get(handler, '/about')
    assert.equal(about.headers.get('x-hello'), 'there')
  })

  it('decides on the request as it arrived, not on a rewritten path', async () => {
    const handler = handlerWith({
      headers: [{ source: '/about', headers: [{ key: 'x-from', value: 'about' }] }],
      rewrites: { beforeFiles: [{ source: '/about', destination: '/news' }] },
    })
    const response = await get(handler, '/about')
    assert.match(await response.text(), /data-path="\/news"/)
    assert.equal(response.headers.get('x-from'), 'about')
  })
})

describe('redirects() on the deployed host', () => {
  it('answers before any route, carrying the query', async () => {
    const handler = handlerWith({
      redirects: [{ source: '/old/:slug', destination: '/blog/:slug', permanent: true }],
    })
    const response = await get(handler, '/old/first?ref=1')
    assert.equal(response.status, 308)
    assert.equal(response.headers.get('location'), '/blog/first?ref=1')
  })

  it('never touches a framework endpoint', async () => {
    const handler = handlerWith({
      redirects: [{ source: '/:path*', destination: '/', permanent: false }],
    })
    const response = await get(handler, '/__ruvyxa/flight?path=/')
    assert.notEqual(response.status, 307)
  })
})

describe('rewrites() on the deployed host', () => {
  it('beforeFiles serves another route under the requested URL', async () => {
    const handler = handlerWith({
      rewrites: { beforeFiles: [{ source: '/docs/:slug', destination: '/blog/:slug' }] },
    })
    const response = await get(handler, '/docs/hello')
    assert.equal(response.status, 200)
    assert.match(await response.text(), /data-params='\{"slug":"[a-z]+"\}'/)
  })

  it('a bare list is afterFiles: a static page wins, a dynamic route yields', async () => {
    const handler = handlerWith({
      rewrites: [
        { source: '/about', destination: '/news' },
        { source: '/blog/:slug', destination: '/news' },
      ],
    })
    const about = await get(handler, '/about')
    assert.match(
      await about.text(),
      /data-path="\/about"/,
      'a static page is a file, checked first',
    )
    const blog = await get(handler, '/blog/x')
    assert.match(await blog.text(), /data-path="\/news"/, 'afterFiles runs before a dynamic route')
  })

  it('fallback runs only when nothing matched', async () => {
    const handler = handlerWith({
      rewrites: { fallback: [{ source: '/:path*', destination: '/news' }] },
    })
    const missing = await get(handler, '/nothing/here')
    assert.match(await missing.text(), /data-path="\/news"/)
    const blog = await get(handler, '/blog/x')
    assert.match(await blog.text(), /data-params='\{"slug":"[a-z]+"\}'/)
  })

  it('is checked once, never against its own result', async () => {
    const handler = handlerWith({
      rewrites: {
        beforeFiles: [
          { source: '/a', destination: '/b' },
          { source: '/b', destination: '/a' },
        ],
      },
    })
    const response = await get(handler, '/a')
    // `/b` is not a route, so the rewritten request is a plain 404 — not a
    // loop and not a second rewrite back to `/a`.
    assert.equal(response.status, 404)
  })

  const originalFetch = globalThis.fetch
  afterEach(() => {
    globalThis.fetch = originalFetch
  })

  it('fetches an external destination', async () => {
    const seen = []
    globalThis.fetch = async (request) => {
      seen.push(request.url)
      return new Response('upstream', { status: 200 })
    }
    const handler = handlerWith({
      rewrites: [{ source: '/blog/:slug', destination: 'https://old.example/blog/:slug' }],
    })
    const response = await get(handler, '/blog/hello')
    assert.equal(await response.text(), 'upstream')
    assert.deepEqual(seen, ['https://old.example/blog/hello'])
  })
})

describe('proxy on the deployed host', () => {
  it('runs for matching paths only, and its Response answers the request', async () => {
    const seen = []
    const handler = handlerWith({
      proxy: {
        matcher: '/admin/:path*',
        handler(request) {
          seen.push(new URL(request.url).pathname)
          return new Response('denied', { status: 401 })
        },
      },
    })
    const denied = await get(handler, '/admin/users')
    assert.equal(denied.status, 401)
    assert.equal(await denied.text(), 'denied')
    const about = await get(handler, '/about')
    assert.equal(about.status, 200)
    assert.deepEqual(seen, ['/admin/users'])
  })

  it('runs on every request when there is no matcher, framework endpoints excepted', async () => {
    const seen = []
    const handler = handlerWith({
      proxy: {
        handler(request) {
          seen.push(new URL(request.url).pathname)
        },
      },
    })
    await get(handler, '/about')
    await get(handler, '/__ruvyxa/flight?path=/')
    assert.deepEqual(seen, ['/about'])
  })

  it('a returned Request is the one that continues: a new path is a rewrite', async () => {
    const handler = handlerWith({
      proxy: {
        handler(request) {
          return new Request(new URL('/news', request.url), request)
        },
      },
    })
    const response = await get(handler, '/about')
    assert.match(await response.text(), /data-path="\/news"/)
  })

  it('runs after redirects and before beforeFiles rewrites', async () => {
    const seen = []
    const handler = handlerWith({
      redirects: [{ source: '/old', destination: '/about', permanent: false }],
      rewrites: { beforeFiles: [{ source: '/forwarded', destination: '/news' }] },
      proxy: {
        handler(request) {
          const url = new URL(request.url)
          seen.push(url.pathname)
          if (url.pathname === '/about') return new Request(new URL('/forwarded', url), request)
        },
      },
    })
    const redirected = await get(handler, '/old')
    assert.equal(redirected.status, 307, 'a redirect answers before the proxy runs')
    const rewritten = await get(handler, '/about')
    assert.match(
      await rewritten.text(),
      /data-path="\/news"/,
      'beforeFiles reads the path the proxy forwarded',
    )
    assert.deepEqual(seen, ['/about'])
  })
})
