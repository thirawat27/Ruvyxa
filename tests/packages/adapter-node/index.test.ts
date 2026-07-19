import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { nodeAdapter } from '../../../packages/@ruvyxa/adapter-node/src/index.ts'

describe('nodeAdapter', () => {
  it('returns node deployment output', async () => {
    const adapter = nodeAdapter()
    const output = await adapter.build({ root: '.', outDir: '.ruvyxa' })
    const { artifacts, ...deployment } = output

    assert.equal(adapter.name, 'node')
    assert.equal(adapter.target, 'node')
    assert.deepEqual(deployment, {
      name: 'node',
      target: 'node',
      platform: 'node',
      entry: '.ruvyxa/server/app',
      assetsDir: '.ruvyxa/assets',
      clientDir: '.ruvyxa/client',
      chunkManifest: '.ruvyxa/client/chunk-manifest.json',
    })
    assert.deepEqual(
      artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'file', path: 'deploy/node/start.mjs' },
        { kind: 'file', path: 'deploy/node/README.md' },
      ],
    )
  })
})
