import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { static as staticOutput } from '../../../packages/@ruvyxa/adapter-static/dist/index.js'

describe('static', () => {
  it('returns static deployment output', async () => {
    const output = await staticOutput().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'static-site', path: 'static' },
        { kind: 'file', path: 'static/_headers' },
      ],
    )

    // Without this file even the content-hashed bundles are served with a
    // revalidate-every-request default on hosts that read `_headers`.
    const headers = output.artifacts?.find((artifact) => artifact.kind === 'file')
    const contents = headers && 'contents' in headers ? String(headers.contents) : ''
    assert.match(contents, /\/__ruvyxa\/client\/\*\n {2}Cache-Control: public, max-age=31536000/)
    assert.match(contents, /\/\*\.png\n {2}Cache-Control: public, max-age=3600/)
    assert.match(contents, /\/\*\n {2}X-Content-Type-Options: nosniff/)
    assert.match(contents, / {2}X-Frame-Options: DENY/)

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
    const output = await staticOutput({ outputDir: 'deploy/public' }).build({
      root: '.',
      outDir: '.ruvyxa',
    })

    assert.equal(output.entry, '.ruvyxa/deploy/public')
    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'static-site', path: 'deploy/public' },
        { kind: 'file', path: 'deploy/public/_headers' },
      ],
    )
  })

  it('rejects output paths that escape the build directory', () => {
    assert.throws(() => staticOutput({ outputDir: '../public' }), /inside the build output/)
    assert.throws(() => staticOutput({ outputDir: 'C:\\public' }), /inside the build output/)
    assert.throws(() => staticOutput({ outputDir: 'assets' }), /overlaps protected build output/)
  })
})
