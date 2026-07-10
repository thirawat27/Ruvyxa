import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { staticAdapter } from '../../../packages/@ruvyxa/adapter-static/src/index.ts'

describe('staticAdapter', () => {
  it('returns static deployment output', async () => {
    const output = await staticAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      {
        name: output.name,
        target: output.target,
        platform: output.platform,
        entry: output.entry,
        assetsDir: output.assetsDir,
        clientDir: output.clientDir,
        chunkManifest: output.chunkManifest,
      },
      {
        name: 'static',
        target: 'static',
        platform: 'static',
        entry: '.ruvyxa/static',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
      },
    )
  })
})
