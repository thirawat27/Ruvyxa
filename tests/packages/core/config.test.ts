import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { config, type RuvyxaConfig } from '../../../packages/@ruvyxa/core/dist/config.js'
import {
  definePlugin,
  withResponseHeader,
  type PluginHttpRequestHandler,
  type PluginHttpRequestRegistration,
  type PluginHttpResponseRegistration,
  type PluginRegistrationApi,
} from '../../../packages/@ruvyxa/core/dist/plugin.js'

function registrationApi(
  onRequest: (value: PluginHttpRequestRegistration | PluginHttpRequestHandler) => void,
): PluginRegistrationApi {
  return {
    environment: 'production',
    http: { onRequest, onResponse() {}, route() {} },
    build: {
      onStart() {},
      onResolve() {},
      onLoad() {},
      onTransform() {},
      onComplete() {},
    },
    dev: { onFileChange() {} },
    diagnostics: { report() {} },
    native: { claim() {} },
  }
}

describe('config and plugin APIs', () => {
  it('accepts grouped plugin sockets in application config', async () => {
    const markdownPlugin = () => () => undefined
    const authPlugin = definePlugin({
      name: 'auth',
      register({ http, build }) {
        http.onRequest({
          match: ['/api/*'],
          handler({ request }) {
            return request.headers.has('authorization')
              ? undefined
              : new Response('Unauthorized', { status: 401 })
          },
        })
        build.onTransform(({ code, id, environment }) =>
          environment === 'client' && id.endsWith('.tsx')
            ? { code: `${code}\n// transformed` }
            : undefined,
        )
        build.onComplete(({ root, outDir, manifest }) => {
          assert.ok(root)
          assert.ok(outDir)
          assert.ok(manifest)
        })
      },
    })
    const settings: RuvyxaConfig = {
      middleware: {
        workers: 2,
        timeoutMs: 15_000,
        builtin: {
          timing: true,
          log: true,
          cors: {
            origins: ['http://localhost:5173'],
            methods: ['GET', 'POST'],
            headers: ['Content-Type'],
            credentials: true,
            maxAge: 86400,
          },
          rate: { max: 100, window: 60, key: 'ip' },
          headers: { 'X-Powered-By': 'Ruvyxa' },
        },
      },
      plugins: [authPlugin],
      markdown: {
        gfm: true,
        remarkPlugins: [[markdownPlugin, { enabled: true }]],
        rehypePlugins: [markdownPlugin],
        recmaPlugins: [markdownPlugin],
        remarkRehypeOptions: { footnoteLabel: 'Notes' },
      },
      adapterOptions: { region: 'iad1' },
      build: { treeShake: false, manifest: true },
    }

    const defined = config(settings)
    assert.equal(defined.middleware?.builtin?.timing, true)
    assert.equal(defined.plugins?.[0]?.name, 'auth')
    assert.equal(defined.markdown?.remarkPlugins?.length, 1)

    let registered: PluginHttpRequestRegistration | PluginHttpRequestHandler | undefined
    await authPlugin.register(registrationApi((value) => (registered = value)))
    assert.deepEqual((registered as PluginHttpRequestRegistration).match, ['/api/*'])
  })

  it('rejects malformed plugin definitions', () => {
    assert.throws(() => definePlugin({ name: ' ', register() {} }), /must have a non-empty name/)
    assert.throws(
      () => definePlugin({ name: 'broken' } as never),
      /must declare behavior or provide register\(api\)/,
    )
    assert.equal(definePlugin({ name: 'valid', register() {} }).name, 'valid')
  })

  it('rejects concise definitions that cannot register behavior', () => {
    for (const definition of [
      { name: 'empty-headers', headers: {} },
      { name: 'empty-routes', http: { routes: [] } },
      { name: 'empty-diagnostics', diagnostics: [] },
    ]) {
      assert.throws(
        () => definePlugin(definition as never),
        /must declare behavior or provide register\(api\)/,
      )
    }

    assert.throws(
      () => definePlugin({ name: 'unknown-build-hook', build: { onTypo() {} } } as never),
      /build\.onTypo is not supported/,
    )
  })

  it('normalizes concise declarations into every existing socket before register()', async () => {
    const registrations: string[] = []
    const plugin = definePlugin({
      name: 'concise',
      headers: { 'x-plugin': 'active' },
      http: {
        match: ['/api/*'],
        onRequest() {},
        onResponse() {},
        routes: [
          { method: 'GET', path: '/plugin/status', handler: () => Response.json({ ok: true }) },
        ],
      },
      build: {
        onStart() {},
        onResolve() {},
        onLoad() {},
        onTransform() {},
        onComplete() {},
      },
      dev: { onFileChange() {} },
      diagnostics: [{ level: 'info', code: 'DX001', message: 'concise declarations enabled' }],
      native: { realtime: true },
      register({ diagnostics }) {
        diagnostics.report({ level: 'info', code: 'DX002', message: 'advanced hook ran last' })
      },
    })

    await plugin.register({
      environment: 'production',
      http: {
        onRequest() {
          registrations.push('http.request')
        },
        onResponse() {
          registrations.push('http.response')
        },
        route() {
          registrations.push('http.route')
        },
      },
      build: {
        onStart() {
          registrations.push('build.start')
        },
        onResolve() {
          registrations.push('build.resolve')
        },
        onLoad() {
          registrations.push('build.load')
        },
        onTransform() {
          registrations.push('build.transform')
        },
        onComplete() {
          registrations.push('build.complete')
        },
      },
      dev: {
        onFileChange() {
          registrations.push('dev.fileChange')
        },
      },
      diagnostics: {
        report(value) {
          registrations.push(`diagnostic.${value.code}`)
        },
      },
      native: {
        claim(capability, options) {
          registrations.push(`native.${capability}.${options?.path ?? 'default'}`)
        },
      },
    })

    assert.deepEqual(registrations, [
      'http.request',
      'http.response',
      'http.route',
      'http.response',
      'build.start',
      'build.resolve',
      'build.load',
      'build.transform',
      'build.complete',
      'dev.fileChange',
      'diagnostic.DX001',
      'native.realtime@1.default',
      'diagnostic.DX002',
    ])
  })

  it('preserves an HTTP match used only to scope generated headers', async () => {
    let registered: PluginHttpResponseRegistration | undefined
    const plugin = definePlugin({
      name: 'scoped-headers',
      headers: { 'x-plugin': 'active' },
      http: { match: ['/api/*'] },
    })

    await plugin.register({
      ...registrationApi(() => {}),
      http: {
        onRequest() {},
        onResponse(value) {
          registered = typeof value === 'function' ? { handler: value } : value
        },
        route() {},
      },
    })

    assert.deepEqual(registered?.match, ['/api/*'])
  })

  it('copies a response when changing one header', async () => {
    const original = new Response('Hello', {
      status: 201,
      statusText: 'Created',
      headers: { 'x-existing': 'kept' },
    })
    const updated = withResponseHeader(original, 'x-plugin', 'active')

    assert.notEqual(updated, original)
    assert.equal(updated.status, 201)
    assert.equal(updated.statusText, 'Created')
    assert.equal(updated.headers.get('x-existing'), 'kept')
    assert.equal(updated.headers.get('x-plugin'), 'active')
    assert.equal(await updated.text(), 'Hello')
  })
  it('accepts head declarations and freezes them onto the plugin', () => {
    const plugin = definePlugin({
      name: 'analytics',
      head: [
        { tag: 'link', attrs: { rel: 'preconnect', href: 'https://cdn.example' } },
        { tag: 'script', attrs: { defer: true }, children: 'window.analytics = 1' },
      ],
    })

    assert.equal(plugin.head?.length, 2)
    assert.equal(plugin.head?.[0].tag, 'link')
    assert.throws(() => {
      // @ts-expect-error frozen at definition time
      plugin.head[0].tag = 'meta'
    })
  })

  it('rejects head declarations that could escape the head', () => {
    const cases: Array<[string, unknown]> = [
      ['tag must be one of', { tag: 'div' }],
      ['invalid attribute name', { tag: 'meta', attrs: { 'x y': '1' } }],
      ['must be a string, number, or boolean', { tag: 'meta', attrs: { content: { a: 1 } } }],
      ['children is only supported on', { tag: 'meta', children: 'text' }],
      ['must not contain a closing', { tag: 'script', children: 'a</script><img>' }],
    ]

    for (const [message, head] of cases) {
      assert.throws(
        () => definePlugin({ name: 'bad', head: head as never }),
        (error: Error) => error.message.includes('RUV2102') && error.message.includes(message),
        message,
      )
    }
  })

  it('reports plugin authoring mistakes with a diagnostic code', () => {
    assert.throws(
      () => definePlugin({ name: 'empty' }),
      /RUV2102 .*must declare behavior or provide register\(api\)/,
    )
  })
})
