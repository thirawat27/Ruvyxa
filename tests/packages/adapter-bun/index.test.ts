import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { bunAdapter } from '../../../packages/@ruvyxa/adapter-bun/src/index.ts'

describe('bunAdapter', () => {
  it('returns bun deployment output', async () => {
    const output = await bunAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'file', path: 'deploy/bun/start.mjs' },
        { kind: 'file', path: 'deploy/bun/README.md' },
      ],
    )

    assert.deepEqual(
      {
        name: output.name,
        target: output.target,
        platform: output.platform,
        runtime: output.runtime,
        entry: output.entry,
        assetsDir: output.assetsDir,
        clientDir: output.clientDir,
        chunkManifest: output.chunkManifest,
      },
      {
        name: 'bun',
        target: 'node',
        platform: 'bun',
        runtime: 'bun',
        entry: '.ruvyxa/server/app',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
      },
    )
  })
})
