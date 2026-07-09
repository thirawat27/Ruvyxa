import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { nodeAdapter } from '../../../packages/@ruvyxa/adapter-node/src/index.ts'

describe('nodeAdapter', () => {
  it('returns node deployment output', async () => {
    const adapter = nodeAdapter()
    const output = await adapter.build({ root: '.', outDir: '.ruvyxa' })

    assert.equal(adapter.name, 'node')
    assert.equal(adapter.target, 'node')
    assert.deepEqual(output, {
      name: 'node',
      target: 'node',
      platform: 'node',
      entry: '.ruvyxa/server/app',
      assetsDir: '.ruvyxa/assets',
    })
  })
})
