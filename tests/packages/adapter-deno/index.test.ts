import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { deno } from '../../../packages/@ruvyxa/adapter-deno/dist/index.js'

describe('deno', () => {
  it('returns a self-contained Deno deployment', async () => {
    const output = await deno().build({ root: '.', outDir: '.ruvyxa' })
    assert.equal(output.name, 'deno')
    assert.equal(output.target, 'node')
    assert.equal(output.platform, 'deno')
    assert.equal(output.runtime, 'deno')
    assert.deepEqual(deno().supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'function', path: 'deploy/deno/server' },
        { kind: 'static-site', path: 'deploy/deno/public' },
        { kind: 'file', path: 'deploy/deno/start.mjs' },
        { kind: 'file', path: 'deploy/deno/README.md' },
      ],
    )
    const server = output.artifacts?.find((artifact) => artifact.kind === 'function')
    const source = server && 'handlerSource' in server ? String(server.handlerSource) : ''
    assert.match(source, /node:http/)
    assert.match(source, /Readable\.fromWeb\(response\.body\)\.pipe\(res\)/)
    assert.doesNotMatch(source, /response\.arrayBuffer\(\)/)
    assert.equal(
      output.artifacts?.find((artifact) => artifact.kind === 'static-site')?.optional,
      true,
    )
  })
})
