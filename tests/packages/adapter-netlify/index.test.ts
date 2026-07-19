import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { netlifyAdapter } from '../../../packages/@ruvyxa/adapter-netlify/src/index.ts'

describe('netlifyAdapter', () => {
  it('returns serverless deployment output', async () => {
    const output = await netlifyAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'static-site', path: 'deploy/netlify/publish' },
        { kind: 'file', path: 'deploy/netlify/netlify.toml' },
      ],
    )

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
        name: 'netlify',
        target: 'serverless',
        platform: 'netlify',
        entry: '.ruvyxa/server/app',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
        functionsDir: '.ruvyxa/netlify/functions',
      },
    )
  })
})
