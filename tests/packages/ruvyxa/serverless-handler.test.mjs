import assert from 'node:assert/strict'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerModule = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')

const { createHandler } = await import(`file://${handlerModule.replaceAll('\\', '/')}`)

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
