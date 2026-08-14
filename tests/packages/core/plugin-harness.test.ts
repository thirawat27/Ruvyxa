import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { definePlugin } from '../../../packages/@ruvyxa/core/dist/plugin.js'
import { createPluginHarness } from '../../../packages/@ruvyxa/core/dist/plugin-harness.js'

describe('plugin test harness', () => {
  it('runs response hooks scoped by their match patterns', async () => {
    const plugin = definePlugin({
      name: 'api-headers',
      http: { match: ['/api/*'] },
      headers: { 'x-api': '1' },
    })

    const harness = await createPluginHarness(plugin)

    const scoped = await harness.respond(new Response('ok'), '/api/items')
    assert.equal(scoped.headers.get('x-api'), '1')

    // A path outside the declared scope must not be touched.
    const unscoped = await harness.respond(new Response('ok'), '/about')
    assert.equal(unscoped.headers.get('x-api'), null)
  })

  it('reports the response a request hook short-circuits with', async () => {
    const plugin = definePlugin({
      name: 'gate',
      http: {
        match: ['/admin/*'],
        onRequest: () => new Response(null, { status: 302, headers: { location: '/login' } }),
      },
    })

    const harness = await createPluginHarness(plugin)

    const blocked = await harness.request('/admin/users')
    assert.equal(blocked.response?.status, 302)
    assert.equal(blocked.response?.headers.get('location'), '/login')

    // Everything else continues to the router with no response.
    const allowed = await harness.request('/')
    assert.equal(allowed.response, undefined)
    assert.equal(allowed.request.url, 'http://localhost/')
  })

  it('invokes a registered route only for its path and method', async () => {
    const plugin = definePlugin({
      name: 'health',
      http: {
        routes: [
          {
            path: '/_health',
            method: 'GET',
            handler: () => Response.json({ ok: true }),
          },
        ],
      },
    })

    const harness = await createPluginHarness(plugin)

    assert.equal(harness.routes.length, 1)
    const hit = await harness.route('/_health')
    assert.ok(hit)
    assert.deepEqual(await hit.json(), { ok: true })
    assert.equal(await harness.route('/_health', { method: 'POST' }), undefined)
    assert.equal(await harness.route('/missing'), undefined)
  })

  it('drives build hooks in build order and threads transforms', async () => {
    const started: string[] = []
    const plugin = definePlugin({
      name: 'build-steps',
      build: {
        onStart: ({ outDir }) => void started.push(outDir),
        onResolve: ({ id }) => (id === 'virtual:config' ? '/resolved/config.ts' : undefined),
        onLoad: ({ id }) => (id === '/resolved/config.ts' ? 'export default 1' : undefined),
        onTransform: ({ code }) => `${code}\n// stamped`,
      },
    })

    const harness = await createPluginHarness(plugin, { root: '/app' })

    await harness.build.start()
    assert.deepEqual(started, ['/app/.ruvyxa'])
    assert.equal(await harness.build.resolve('virtual:config'), '/resolved/config.ts')
    assert.equal(await harness.build.resolve('./other'), null)
    assert.deepEqual(await harness.build.load('/resolved/config.ts'), { code: 'export default 1' })
    assert.deepEqual(await harness.build.transform('const a = 1', '/a.ts'), {
      code: 'const a = 1\n// stamped',
    })
  })

  it('delivers only the changed paths a dev hook asked for', async () => {
    const seen: string[] = []
    const plugin = definePlugin({
      name: 'watcher',
      dev: {
        onFileChange: {
          match: ['content/*'],
          handler: ({ paths }) => void seen.push(...paths),
        },
      },
    })

    const harness = await createPluginHarness(plugin)

    await harness.fileChange(['content/post.md', 'app/page.tsx'])
    assert.deepEqual(seen, ['content/post.md'])

    // No matching path means the hook does not run at all.
    await harness.fileChange('app/layout.tsx')
    assert.deepEqual(seen, ['content/post.md'])
  })

  it('collects declarations made during registration', async () => {
    const plugin = definePlugin({
      name: 'analytics',
      head: { tag: 'script', attrs: { src: 'https://cdn.example/a.js', defer: true } },
      diagnostics: { level: 'warning', code: 'AN001', message: 'sampling is on' },
    })

    const harness = await createPluginHarness(plugin)

    assert.deepEqual(harness.head, [
      { tag: 'script', attrs: { src: 'https://cdn.example/a.js', defer: true } },
    ])
    assert.equal(harness.diagnostics[0]?.plugin, 'analytics')
    assert.equal(harness.diagnostics[0]?.level, 'warning')
    assert.equal(harness.diagnostics[0]?.message, 'sampling is on')
  })

  it('applies several plugins in configuration order', async () => {
    const first = definePlugin({
      name: 'first',
      http: { onResponse: ({ response }) => appendHeader(response, 'x-order', 'first') },
    })
    const second = definePlugin({
      name: 'second',
      http: { onResponse: ({ response }) => appendHeader(response, 'x-order', 'second') },
    })

    const harness = await createPluginHarness([first, second])
    const response = await harness.respond(new Response('ok'), '/')

    assert.equal(response.headers.get('x-order'), 'first, second')
  })
})

function appendHeader(response: Response, name: string, value: string): Response {
  const headers = new Headers(response.headers)
  headers.append(name, value)
  return new Response(response.body, { status: response.status, headers })
}
