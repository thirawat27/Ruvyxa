import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { netlifyAdapter } from '../../../packages/@ruvyxa/adapter-netlify/src/index.ts'

describe('netlifyAdapter', () => {
  it('returns serverless deployment output', async () => {
    const output = await netlifyAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'static-site', path: 'deploy/netlify/publish', scope: undefined },
        { kind: 'file', path: 'deploy/netlify/netlify.toml', scope: undefined },
        { kind: 'file', path: 'netlify.toml', scope: 'project' },
      ],
    )

    const toml = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/netlify/netlify.toml',
    )
    assert.match(
      toml && 'contents' in toml ? String(toml.contents) : '',
      /for = "\/client\/\*"[\s\S]*Cache-Control = "public, max-age=31536000, immutable"/,
    )

    const projectToml = output.artifacts?.find((artifact) => artifact.path === 'netlify.toml')
    assert.equal(projectToml?.skipIfExists, true)
    assert.match(
      projectToml && 'contents' in projectToml ? String(projectToml.contents) : '',
      /publish = "\.ruvyxa\/deploy\/netlify\/publish"/,
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
        functionsDir: output.functionsDir,
      },
      {
        name: 'netlify',
        target: 'serverless',
        platform: 'netlify',
        entry: '.ruvyxa/server/app',
        assetsDir: '.ruvyxa/assets',
        clientDir: '.ruvyxa/client',
        chunkManifest: '.ruvyxa/client/chunk-manifest.json',
        functionsDir: '.ruvyxa/netlify/functions',
      },
    )
  })
})
