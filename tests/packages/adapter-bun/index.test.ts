import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { bun } from '../../../packages/@ruvyxa/adapter-bun/dist/index.js'

describe('bun', () => {
  it('returns bun deployment output', async () => {
    const output = await bun().build({ root: '.', outDir: '.ruvyxa' })

    // A launcher alone made a Bun host depend on the ruvyxa CLI and its native
    // binary at runtime, unlike every other self-hosted target.
    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'function', path: 'deploy/bun/server' },
        { kind: 'static-site', path: 'deploy/bun/public' },
        { kind: 'file', path: 'deploy/bun/start.mjs' },
        { kind: 'file', path: 'deploy/bun/README.md' },
      ],
    )

    // The server is the shared standalone source, so Bun and Node make the
    // same ordering, fallback, and cache-header decisions. Only the transport
    // differs, and it has to be Bun's own: `createHandler` already is the
    // `Request` → `Response` function `Bun.serve` wants, and routing it through
    // `node:http` instead made every request pay to have a `Request` taken
    // apart and a `Response` rebuilt.
    const server = output.artifacts?.find((artifact) => artifact.kind === 'function')
    const source = server && 'handlerSource' in server ? String(server.handlerSource) : ''
    assert.match(source, /Bun\.serve\(/)
    assert.doesNotMatch(source, /from 'node:http'/)
    assert.match(source, /isAssetPath\(url\.pathname\)/)
    assert.match(source, /public, max-age=3600, must-revalidate/)
    // The handler's response is returned as it is, so a streamed render still
    // streams and nothing is buffered on the way out.
    assert.doesNotMatch(source, /response\.arrayBuffer\(\)/)
    assert.doesNotMatch(source, /npx/)
    // `idleTimeout` has defaulted to 0 — never retire an idle connection —
    // since Bun 1.1.27, which is already on the safe side of the 502 the Node
    // transport raises `keepAliveTimeout` to avoid, and is what lets a long
    // streamed response stay open. It is set only when an operator asks for a
    // bound, and clamped to the 255-second maximum Bun accepts.
    assert.match(source, /RUVYXA_KEEP_ALIVE_TIMEOUT/)
    assert.match(source, /Math\.min\(255,/)
    // Spread, so the option is simply absent when nothing was configured and
    // Bun's own default stands.
    assert.match(source, /\.\.\.idleTimeout,/)
    // The slice is handed over as a file, not as its `.stream()`: measured
    // against Bun 1.4.0, a sliced `BunFile`'s stream served by `Bun.serve`
    // sends the whole file, so a seek would have played the entire video.
    assert.match(source, /file\.slice\(plan\.partial\.start, plan\.partial\.end \+ 1\)/)
    assert.doesNotMatch(source, /plan\.partial\.end \+ 1\)\.stream\(\)/)

    // An API-only app has no prerendered pages; the publish directory must be
    // optional so the build does not fail with RUV2202.
    assert.equal(
      output.artifacts?.find((artifact) => artifact.kind === 'static-site')?.optional,
      true,
    )
    assert.deepEqual(bun().supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])

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
