import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { cloudflareAdapter } from '../../../packages/@ruvyxa/adapter-cloudflare/src/index.ts'

describe('cloudflareAdapter', () => {
  it('returns edge deployment output', async () => {
    const output = await cloudflareAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path }) => ({ kind, path })),
      [
        { kind: 'static-site', path: 'deploy/cloudflare/assets' },
        { kind: 'file', path: 'deploy/cloudflare/wrangler.jsonc' },
        { kind: 'file', path: 'deploy/cloudflare/assets/_headers' },
      ],
    )

    const headersArtifact = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/cloudflare/assets/_headers',
    )
    assert.match(
      headersArtifact && 'contents' in headersArtifact ? String(headersArtifact.contents) : '',
      /\/client\/\*\n {2}Cache-Control: public, max-age=31536000, immutable/,
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
      },
      {
        name: 'cloudflare',
        target: 'edge',
        platform: 'cloudflare',
        entry: '.ruvyxa/server/app',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
      },
    )
  })
})
