import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { vercelAdapter } from '../../../packages/@ruvyxa/adapter-vercel/src/index.ts'

describe('vercelAdapter', () => {
  it('returns serverless deployment output', async () => {
    const output = await vercelAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'static-site', path: 'deploy/vercel/.vercel/output/static', scope: undefined },
        { kind: 'file', path: 'deploy/vercel/.vercel/output/config.json', scope: undefined },
        { kind: 'static-site', path: '.vercel/output/static', scope: 'project' },
        { kind: 'file', path: '.vercel/output/config.json', scope: 'project' },
      ],
    )

    const projectConfig = output.artifacts?.find(
      (artifact) => artifact.path === '.vercel/output/config.json',
    )
    assert.equal(
      projectConfig && 'contents' in projectConfig ? String(projectConfig.contents) : '',
      output.artifacts?.find(
        (artifact) => artifact.path === 'deploy/vercel/.vercel/output/config.json',
      )?.contents,
    )

    assert.deepEqual(
      vercelAdapter({ projectOutput: false })
        .build({ root: '.', outDir: '.ruvyxa' })
        .artifacts?.map(({ path }) => path),
      ['deploy/vercel/.vercel/output/static', 'deploy/vercel/.vercel/output/config.json'],
    )

    const configArtifact = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/vercel/.vercel/output/config.json',
    )
    const config = JSON.parse(
      configArtifact && 'contents' in configArtifact ? String(configArtifact.contents) : '{}',
    )
    assert.equal(config.version, 3)
    assert.deepEqual(config.routes[0], {
      src: '^/client/(.*)$',
      headers: { 'cache-control': 'public, max-age=31536000, immutable' },
      continue: true,
    })
    assert.deepEqual(config.routes.at(-1), { handle: 'filesystem' })

    assert.deepEqual(
      {
        name: output.name,
        target: output.target,
        platform: output.platform,
        entry: output.entry,
        assetsDir: output.assetsDir,
        clientDir: output.clientDir,
        chunkManifest: output.chunkManifest,
        functionsDir: output.functionsDir,
      },
      {
        name: 'vercel',
        target: 'serverless',
        platform: 'vercel',
        entry: '.ruvyxa/server/app',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
        functionsDir: '.ruvyxa/functions',
      },
    )
  })
})
