import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { definePlugin } from '../../../packages/@ruvyxa/core/dist/plugin.js'
import type { RuvyxaPlugin } from '../../../packages/@ruvyxa/core/dist/plugin.js'
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
        onStart: ({ outDir }) => {
          started.push(outDir)
        },
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
          handler: ({ paths }) => {
            seen.push(...paths)
          },
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

/**
 * The harness must refuse everything `createPluginRegistry` refuses.
 *
 * A harness that records what the framework rejects reports success for a
 * plugin `ruvyxa dev` will not start with — the exact failure it exists to
 * prevent. Each case here names one rule the production registry enforces at
 * construction; the messages are matched loosely so the two wordings may differ
 * while the refusal may not.
 */
describe('plugin harness registration contract', () => {
  it('rejects a duplicate plugin name', async () => {
    const plugin = definePlugin({
      name: 'twice',
      http: { onRequest: () => undefined },
    })

    await assert.rejects(() => createPluginHarness([plugin, plugin]), /duplicate plugin name/)
  })

  it('rejects a plugin that is not a registrable object', async () => {
    await assert.rejects(
      () => createPluginHarness([null as unknown as RuvyxaPlugin]),
      /must be a plugin object/,
    )
    await assert.rejects(
      () => createPluginHarness({ name: 'no-register' } as unknown as RuvyxaPlugin),
      /must provide register\(api\)/,
    )
  })

  it('rejects an error-level diagnostic', async () => {
    const plugin = definePlugin({
      name: 'fails-boot',
      diagnostics: { level: 'error', code: 'AN001', message: 'unsupported configuration' },
    })

    await assert.rejects(() => createPluginHarness(plugin), /AN001 unsupported configuration/)
  })

  it('rejects a malformed diagnostic', async () => {
    const plugin = definePlugin({
      name: 'bad-diagnostic',
      diagnostics: { level: 'fatal', code: 'AN001', message: 'x' } as unknown as {
        level: 'info'
        code: string
        message: string
      },
    })

    await assert.rejects(() => createPluginHarness(plugin), /diagnostic level must be/)
  })

  it('rejects a route path that is not an exact absolute path', async () => {
    for (const path of ['health', '/health/*', '/health?x=1', '/health#a']) {
      const plugin = definePlugin({
        name: `route-${path}`,
        http: { routes: [{ path, handler: () => new Response('ok') }] },
      })
      await assert.rejects(() => createPluginHarness(plugin), /must be an exact absolute path/)
    }
  })

  it('rejects an invalid HTTP method token', async () => {
    const plugin = definePlugin({
      name: 'bad-method',
      http: { routes: [{ path: '/health', method: 'GE T', handler: () => new Response('ok') }] },
    })

    await assert.rejects(() => createPluginHarness(plugin), /valid HTTP method tokens/)
  })

  it('rejects two plugins claiming one route', async () => {
    const first = definePlugin({
      name: 'health-a',
      http: { routes: [{ path: '/health', method: 'GET', handler: () => new Response('a') }] },
    })
    const second = definePlugin({
      name: 'health-b',
      http: { routes: [{ path: '/health', method: 'GET', handler: () => new Response('b') }] },
    })

    await assert.rejects(() => createPluginHarness([first, second]), /conflicts with plugin/)
  })

  it('rejects a match list that is empty, unslashed, or wildcarded in the middle', async () => {
    const cases: Array<readonly string[]> = [
      [],
      ['api/*'],
      ['/api/*/items'],
      ['/api/**'],
      [''],
      [42 as unknown as string],
    ]
    for (const match of cases) {
      const plugin = definePlugin({
        name: `match-${JSON.stringify(match)}`,
        http: { match, onRequest: () => undefined },
      })
      await assert.rejects(() => createPluginHarness(plugin), /http\.onRequest\(\)\.match/)
    }

    const notAnArray = definePlugin({
      name: 'match-string',
      http: { match: '/api/*' as unknown as readonly string[], onRequest: () => undefined },
    })
    await assert.rejects(() => createPluginHarness(notAnArray), /http\.onRequest\(\)\.match/)
  })

  it('rejects an unknown or doubly-claimed native capability', async () => {
    const unknown: RuvyxaPlugin = {
      name: 'unknown-capability',
      register(api) {
        ;(api.native.claim as unknown as (capability: string) => void)('sockets@1')
      },
    }
    await assert.rejects(() => createPluginHarness(unknown), /unsupported native capability/)

    const first = definePlugin({ name: 'realtime-a', native: { realtime: true } })
    const second = definePlugin({ name: 'realtime-b', native: { realtime: true } })
    await assert.rejects(() => createPluginHarness([first, second]), /already owned by plugin/)
  })

  it('rejects a hook that returns neither a Response, a Request, nor undefined', async () => {
    const plugin = definePlugin({
      name: 'bad-return',
      http: { onRequest: () => 'nope' as unknown as undefined },
    })
    const harness = await createPluginHarness(plugin)

    await assert.rejects(() => harness.request('/'), /returned an unsupported value/)
  })

  it('rejects next() called with the wrong type', async () => {
    const request = definePlugin({
      name: 'bad-next-request',
      http: {
        onRequest: ({ next }) => {
          ;(next as unknown as (value: unknown) => void)('/elsewhere')
        },
      },
    })
    const requestHarness = await createPluginHarness(request)
    await assert.rejects(() => requestHarness.request('/'), /next\(\) expects a Request/)

    const response = definePlugin({
      name: 'bad-next-response',
      http: {
        onResponse: ({ next }) => {
          ;(next as unknown as (value: unknown) => void)('ok')
        },
      },
    })
    const responseHarness = await createPluginHarness(response)
    await assert.rejects(
      () => responseHarness.respond(new Response('ok'), '/'),
      /next\(\) expects a Response/,
    )
  })

  it('matches hooks against the decoded pathname', async () => {
    const plugin = definePlugin({
      name: 'decoded',
      http: { match: ['/files/my doc'], onRequest: () => new Response('hit') },
    })
    const harness = await createPluginHarness(plugin)

    const encoded = await harness.request('/files/my%20doc')
    assert.equal(await encoded.response?.text(), 'hit')
  })

  it('dispatches routes and request hooks from one registration-ordered list', async () => {
    const order: string[] = []
    const hookFirst = definePlugin({
      name: 'hook-first',
      http: {
        onRequest: () => {
          order.push('hook')
          return undefined
        },
      },
    })
    const routeSecond = definePlugin({
      name: 'route-second',
      http: {
        routes: [
          {
            path: '/x',
            handler: () => {
              order.push('route')
              return new Response('x')
            },
          },
        ],
      },
    })

    const harness = await createPluginHarness([hookFirst, routeSecond])
    const result = await harness.request('/x')
    assert.equal(await result.response?.text(), 'x')
    assert.deepEqual(order, ['hook', 'route'])

    // Reversed, the route answers before the hook is ever reached.
    order.length = 0
    const reversed = await createPluginHarness([routeSecond, hookFirst])
    const short = await reversed.request('/x')
    assert.equal(await short.response?.text(), 'x')
    assert.deepEqual(order, ['route'])
  })

  it('continues with the request next() was given', async () => {
    const plugin = definePlugin({
      name: 'rewriter',
      http: {
        onRequest: ({ request, next }) => {
          next(new Request('http://localhost/rewritten', { headers: request.headers }))
          return undefined
        },
      },
    })
    const harness = await createPluginHarness(plugin)

    const result = await harness.request('/original')
    assert.equal(result.request.url, 'http://localhost/rewritten')
  })
})

function appendHeader(response: Response, name: string, value: string): Response {
  const headers = new Headers(response.headers)
  headers.append(name, value)
  return new Response(response.body, { status: response.status, headers })
}
