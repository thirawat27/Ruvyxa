import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { nodeAdapter } from '../../../packages/@ruvyxa/adapter-node/src/index.ts'

describe('nodeAdapter', () => {
  it('returns node deployment output with a standalone server', async () => {
    const adapter = nodeAdapter()
    const output = await adapter.build({ root: '.', outDir: '.ruvyxa' })
    const { artifacts, ...deployment } = output

    assert.equal(adapter.name, 'node')
    assert.equal(adapter.target, 'node')
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
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
        { kind: 'function', path: 'deploy/node/server' },
        { kind: 'static-site', path: 'deploy/node/public' },
        { kind: 'file', path: 'deploy/node/start.mjs' },
        { kind: 'file', path: 'deploy/node/README.md' },
      ],
    )

    // The standalone server must be a plain node:http wrapper around the
    // generic serverless handler — runnable without the ruvyxa CLI.
    const serverArtifact = artifacts?.find((artifact) => artifact.path === 'deploy/node/server')
    assert.ok(serverArtifact)
    assert.ok('handlerSource' in serverArtifact!)
    const source = String(serverArtifact!.handlerSource)
    assert.match(source, /node:http/)
    assert.match(source, /createHandler/)
    assert.match(source, /loadRouteModule/)
    assert.match(source, /prerenderRelativePath/)
    assert.match(source, /process\.env\.PORT/)
    assert.match(source, /process\.env\.HOST/)
    // ISR support: reads and writes the prerender cache
    assert.match(source, /writePrerendered/)
    // Static assets served with immutable cache headers
    assert.match(source, /__ruvyxa\/client\//)
    assert.doesNotMatch(source, /npx/)

    // An API-only app has no prerendered pages; the publish directory must be
    // optional so the build does not fail with RUV2202.
    const publicArtifact = artifacts?.find((artifact) => artifact.path === 'deploy/node/public')
    assert.equal(publicArtifact?.optional, true)
  })
})
