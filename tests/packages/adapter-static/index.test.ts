import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { staticAdapter } from '../../../packages/@ruvyxa/adapter-static/src/index.ts'

describe('staticAdapter', () => {
  it('returns static deployment output', async () => {
    const output = await staticAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(output.artifacts, [{ kind: 'static-site', path: 'static' }])

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

  it('materializes custom output inside the build directory', async () => {
    const output = await staticAdapter({ outputDir: 'deploy/public' }).build({
      root: '.',
      outDir: '.ruvyxa',
    })

    assert.equal(output.entry, '.ruvyxa/deploy/public')
    assert.deepEqual(output.artifacts, [{ kind: 'static-site', path: 'deploy/public' }])
  })

  it('rejects output paths that escape the build directory', () => {
    assert.throws(() => staticAdapter({ outputDir: '../public' }), /inside the build output/)
    assert.throws(() => staticAdapter({ outputDir: 'C:\\public' }), /inside the build output/)
    assert.throws(() => staticAdapter({ outputDir: 'assets' }), /overlaps protected build output/)
  })
})
