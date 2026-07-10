import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { vercelAdapter } from '../../../packages/@ruvyxa/adapter-vercel/src/index.ts'

describe('vercelAdapter', () => {
  it('returns serverless deployment output', async () => {
    const output = await vercelAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      {
        name: output.name,
        target: output.target,
        platform: output.platform,
        entry: output.entry,
        assetsDir: output.assetsDir,
        clientDir: output.clientDir,
        chunkManifest: output.chunkManifest,
        functionsDir: output.functionsDir,
      },
      {
        name: 'vercel',
        target: 'serverless',
        platform: 'vercel',
        entry: '.ruvyxa/server/app',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
        functionsDir: '.ruvyxa/functions',
      },
    )
  })
})
