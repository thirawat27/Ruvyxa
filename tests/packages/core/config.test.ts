import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { config, type RuvyxaConfig } from '../../../packages/@ruvyxa/core/dist/config.js'

describe('config()', () => {
  it('returns the application config it was given, typed and unchanged', () => {
    const markdownPlugin = () => () => undefined
    const settings: RuvyxaConfig = {
      appDir: 'app',
      outDir: '.ruvyxa',
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
      headers: [{ source: '/api/:path*', headers: [{ key: 'cache-control', value: 'no-store' }] }],
      redirects: async () => [{ source: '/old', destination: '/new', permanent: true }],
      proxy: {
        matcher: '/admin/:path*',
        handler: () => undefined,
      },
      realtime: true,
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
    assert.equal(defined, settings)
    assert.equal(defined.middleware?.builtin?.timing, true)
    assert.equal(defined.markdown?.remarkPlugins?.length, 1)
    assert.equal(typeof defined.proxy?.handler, 'function')
  })
})
