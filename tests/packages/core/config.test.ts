import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { config, type RuvyxaConfig } from '../../../packages/@ruvyxa/core/src/config.ts'

describe('config API', () => {
  it('accepts documented middleware configuration', () => {
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
        plugins: [
          {
            name: 'auth-guard',
            path: 'plugins/auth-guard.wasm',
            phase: 'request',
            routes: ['/api/*'],
            config: { apiKeyHeader: 'X-Api-Key' },
            allow: {
              env: ['AUTH_SECRET'],
              read: ['./content'],
              net: ['api.example.com'],
              timeout: 5000,
              memory: 67108864,
            },
          },
        ],
      },
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
    assert.equal(defined.middleware?.plugins?.[0]?.phase, 'request')
    assert.equal(defined.adapterOptions?.region, 'iad1')
    assert.equal(defined.build?.treeShake, false)
    assert.equal(defined.build?.manifest, true)
  })
})
