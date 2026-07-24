import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { bunAdapter } from '../../../packages/@ruvyxa/adapter-bun/src/index.ts'

describe('bunAdapter', () => {
  it('returns bun deployment output', async () => {
    const output = await bunAdapter().build({ root: '.', outDir: '.ruvyxa' })

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
    // same ordering, fallback, and cache-header decisions.
    const server = output.artifacts?.find((artifact) => artifact.kind === 'function')
    const source = server && 'handlerSource' in server ? String(server.handlerSource) : ''
    assert.match(source, /node:http/)
    assert.match(source, /isAssetPath\(url\.pathname\)/)
    assert.match(source, /public, max-age=3600, must-revalidate/)
    assert.doesNotMatch(source, /npx/)

    // An API-only app has no prerendered pages; the publish directory must be
    // optional so the build does not fail with RUV2202.
    assert.equal(
      output.artifacts?.find((artifact) => artifact.kind === 'static-site')?.optional,
      true,
    )
    assert.deepEqual(bunAdapter().supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])

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
