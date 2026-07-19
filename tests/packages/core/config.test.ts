import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import {
  config,
  definePlugin,
  type RuvyxaConfig,
} from '../../../packages/@ruvyxa/core/src/config.ts'

describe('config API', () => {
  it('accepts builtin middleware and TypeScript-native plugins', () => {
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
    assert.equal(defined.plugins?.[0]?.name, 'auth')
    assert.equal(defined.adapterOptions?.region, 'iad1')
    assert.equal(defined.build?.treeShake, false)
    assert.equal(defined.build?.manifest, true)
  })

  it('rejects malformed plugin definitions at the application boundary', () => {
    assert.throws(() => definePlugin({ name: ' ', setup() {} }), /must have a non-empty name/)
    assert.throws(() => definePlugin({ name: 'broken' } as never), /must provide setup\(context\)/)
  })
})
