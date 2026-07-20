import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { cloudflareAdapter } from '../../../packages/@ruvyxa/adapter-cloudflare/src/index.ts'

describe('cloudflareAdapter', () => {
  it('returns edge deployment output with worker function', async () => {
    const output = await cloudflareAdapter({ compatibilityDate: '2024-12-01' }).build({
      root: '.',
      outDir: '.ruvyxa',
    })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'static-site', path: 'deploy/cloudflare/assets', scope: undefined },
        { kind: 'function', path: 'deploy/cloudflare/worker', scope: undefined },
        { kind: 'file', path: 'deploy/cloudflare/wrangler.jsonc', scope: undefined },
        { kind: 'file', path: 'deploy/cloudflare/assets/_headers', scope: undefined },
        { kind: 'file', path: 'wrangler.jsonc', scope: 'project' },
      ],
    )

    // Verify wrangler.jsonc has main and compatibility_date
    const wranglerArtifact = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/cloudflare/wrangler.jsonc',
    )
    const wranglerConfig = JSON.parse(
      wranglerArtifact && 'contents' in wranglerArtifact ? String(wranglerArtifact.contents) : '{}',
    )
    assert.equal(wranglerConfig.name, 'ruvyxa-app')
    assert.equal(wranglerConfig.main, './worker/index.mjs')
    assert.equal(wranglerConfig.compatibility_date, '2024-12-01')
    assert.deepEqual(wranglerConfig.assets, { directory: './assets' })

    // Verify function artifact has handler source
    const functionArtifact = output.artifacts?.find(
      (artifact) => artifact.kind === 'function' && artifact.path === 'deploy/cloudflare/worker',
    )
    assert.ok(functionArtifact)
    assert.ok('handlerSource' in functionArtifact!)
    assert.match(String(functionArtifact!.handlerSource), /createHandler/)
    assert.match(String(functionArtifact!.handlerSource), /export default/)

    // Verify _headers for client cache
    const headersArtifact = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/cloudflare/assets/_headers',
    )
    assert.match(
      headersArtifact && 'contents' in headersArtifact ? String(headersArtifact.contents) : '',
      /\/client\/\*\n {2}Cache-Control: public, max-age=31536000, immutable/,
    )

    // Verify project-scope wrangler.jsonc
    const projectConfig = output.artifacts?.find((artifact) => artifact.path === 'wrangler.jsonc')
    assert.equal(projectConfig?.skipIfExists, true)
    const projectWrangler = JSON.parse(
      projectConfig && 'contents' in projectConfig ? String(projectConfig.contents) : '{}',
    )
    assert.equal(projectWrangler.main, '.ruvyxa/deploy/cloudflare/worker/index.mjs')
    assert.ok(projectWrangler.compatibility_date)

    // Verify projectConfig: false disables project-scope artifacts
    assert.deepEqual(
      (
        await cloudflareAdapter({ projectConfig: false, compatibilityDate: '2024-12-01' }).build({
          root: '.',
          outDir: '.ruvyxa',
        })
      ).artifacts?.map(({ path }) => path),
      [
        'deploy/cloudflare/assets',
        'deploy/cloudflare/worker',
        'deploy/cloudflare/wrangler.jsonc',
        'deploy/cloudflare/assets/_headers',
      ],
    )

    // Verify adapter metadata
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

  it('declares supported strategies', () => {
    const adapter = cloudflareAdapter()
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'api'])
  })
})
