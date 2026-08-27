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
    // **No `deno.json`.** A configuration file makes that directory Deno's own
    // scope, and the scope it creates declares no dependencies and no
    // `nodeModulesDir` — so the npm specifiers the server reaches for at
    // request time stop resolving. One was emitted here as a place to put the
    // entrypoint a Deno Deploy build needs, and it turned every
    // server-components request into `Import "react-server-dom-webpack/
    // client.edge" not a dependency` and a 500, while every other route kept
    // answering. Without the file Deno walks up to the project and resolves
    // them, which is what the deployment has always relied on.
    assert.equal(
      output.artifacts?.some((artifact) => artifact.path.endsWith('deno.json')),
      false,
    )
    // Deno Deploy has no framework preset for Ruvyxa, so the entrypoint is the
    // project's to name, and the README is where it is named.
    const readme = output.artifacts?.find((artifact) => artifact.path.endsWith('README.md'))
    assert.match(String(readme?.contents), /\.ruvyxa\/deploy\/deno\/server\/index\.mjs/)
    assert.match(String(readme?.contents), /DENO_DEPLOY/)
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
