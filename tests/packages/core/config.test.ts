import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import {
  config,
  definePlugin,
  plugin,
  type RuvyxaConfig,
} from '../../../packages/@ruvyxa/core/src/config.ts'

describe('config API', () => {
  it('accepts builtin middleware and plugins', () => {
    const authPlugin = definePlugin({
      name: 'auth',
      setup({ addMiddleware, transform, onBuildComplete }) {
        addMiddleware({
          routes: ['/api/*'],
          onRequest(request) {
            return request.headers.has('authorization')
              ? undefined
              : new Response('Unauthorized', { status: 401 })
          },
        })
        transform((code, id, context) =>
          context.environment === 'client' && id.endsWith('.tsx')
            ? { code: `${code}\n// transformed` }
            : undefined,
        )
        onBuildComplete(({ root, outDir, manifest }) => {
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
          rate: {
            max: 100,
            window: 60,
            key: 'ip',
          },
          headers: {
            'X-Powered-By': 'Ruvyxa',
          },
        },
      },
      plugins: [authPlugin],
      adapterOptions: {
        region: 'iad1',
      },
      build: {
        treeShake: false,
        manifest: true,
      },
    }

    const defined = config(settings)

    assert.equal(defined.middleware?.builtin?.timing, true)
    assert.equal(defined.middleware?.workers, 2)
    assert.equal(defined.middleware?.timeoutMs, 15_000)
    assert.equal(defined.plugins?.[0]?.name, 'auth')
    assert.equal(defined.adapterOptions?.region, 'iad1')
    assert.equal(defined.build?.treeShake, false)
    assert.equal(defined.build?.manifest, true)
  })

  it('rejects malformed plugin definitions at the application boundary', () => {
    assert.throws(() => definePlugin({ name: ' ', setup() {} }), /must have a non-empty name/)
    assert.throws(() => definePlugin({ name: 'broken' } as never), /must provide setup\(context\)/)
  })

  it('creates a middleware plugin without setup boilerplate', () => {
    const auth = plugin('auth', {
      routes: ['/api/*'],
      onRequest: (request) =>
        request.headers.has('authorization')
          ? undefined
          : new Response('Unauthorized', { status: 401 }),
    })
    let registered: unknown

    auth.setup({
      addMiddleware(value) {
        registered = value
      },
      resolveId() {},
      transform() {},
      onBuildComplete() {},
    })

    assert.equal(auth.name, 'auth')
    assert.deepEqual((registered as { routes?: string[] }).routes, ['/api/*'])
    assert.equal(typeof (registered as { onRequest?: unknown }).onRequest, 'function')

    const logger = plugin('logger', (request) => request)
    logger.setup({
      addMiddleware(value) {
        registered = value
      },
      resolveId() {},
      transform() {},
      onBuildComplete() {},
    })
    assert.equal(typeof registered, 'function')
  })
})
