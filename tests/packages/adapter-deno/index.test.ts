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
    // Deno's own server, not `node:http`: `createHandler` already is the
    // `Request` → `Response` function `Deno.serve` takes, and going through the
    // compatibility layer made every request pay for a translation in each
    // direction. The decisions either side of it are the shared ones.
    const server = output.artifacts?.find((artifact) => artifact.kind === 'function')
    const source = server && 'handlerSource' in server ? String(server.handlerSource) : ''
    assert.match(source, /Deno\.serve\(/)
    assert.doesNotMatch(source, /from 'node:http'/)
    assert.match(source, /isAssetPath\(url\.pathname\)/)
    assert.match(source, /public, max-age=3600, must-revalidate/)
    // The handler's response is returned as it is, so a streamed render still
    // streams and nothing is buffered on the way out.
    assert.doesNotMatch(source, /response\.arrayBuffer\(\)/)
    // `server.shutdown()` waits for in-flight responses, which is the drain the
    // Node transport has to build by hand.
    assert.match(source, /server\.shutdown\(\)/)
    assert.equal(
      output.artifacts?.find((artifact) => artifact.kind === 'static-site')?.optional,
      true,
    )
  })
})
