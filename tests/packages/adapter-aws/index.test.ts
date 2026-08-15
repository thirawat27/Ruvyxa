import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { aws } from '../../../packages/@ruvyxa/adapter-aws/dist/index.js'

describe('aws', () => {
  it('emits an Amplify Hosting static and compute deployment bundle', async () => {
    const adapter = aws()
    const output = await adapter.build({ root: '.', outDir: '.ruvyxa' })

    assert.equal(output.platform, 'aws')
    assert.equal(output.target, 'serverless')
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        {
          kind: 'static-site',
          path: 'deploy/aws/.amplify-hosting/static',
          scope: undefined,
        },
        {
          kind: 'function',
          path: 'deploy/aws/.amplify-hosting/compute/default',
          scope: undefined,
        },
        {
          kind: 'file',
          path: 'deploy/aws/.amplify-hosting/compute/default/server.js',
          scope: undefined,
        },
        {
          kind: 'file',
          path: 'deploy/aws/.amplify-hosting/deploy-manifest.json',
          scope: undefined,
        },
        { kind: 'file', path: 'deploy/aws/README.md', scope: undefined },
        { kind: 'static-site', path: '.amplify-hosting/static', scope: 'project' },
        {
          kind: 'function',
          path: '.amplify-hosting/compute/default',
          scope: 'project',
        },
        {
          kind: 'file',
          path: '.amplify-hosting/compute/default/server.js',
          scope: 'project',
        },
        {
          kind: 'file',
          path: '.amplify-hosting/deploy-manifest.json',
          scope: 'project',
        },
      ],
    )

    const manifestArtifact = output.artifacts?.find(
      (artifact) => artifact.path === '.amplify-hosting/deploy-manifest.json',
    )
    const manifest = JSON.parse(
      manifestArtifact && 'contents' in manifestArtifact ? String(manifestArtifact.contents) : '{}',
    )
    assert.equal(manifest.version, 1)
    assert.equal(manifest.routes.at(-1).path, '/*')
    assert.deepEqual(manifest.computeResources, [
      { name: 'default', runtime: 'nodejs24.x', entrypoint: 'server.js' },
    ])
    assert.match(manifest.framework.version, /^\d+\.\d+\.\d+/)

    const compute = output.artifacts?.find(
      (artifact) =>
        artifact.kind === 'function' && artifact.path === '.amplify-hosting/compute/default',
    )
    const source = String(compute && 'handlerSource' in compute ? compute.handlerSource : '')
    assert.match(source, /server\.listen/)
    assert.match(source, /process\.env\.PORT \|\| 3000/)
    assert.match(source, /os\.tmpdir\(\)/)
    assert.match(source, /ruvyxa-isr-cache/)
  })

  it('can keep output inside the Ruvyxa build directory', async () => {
    const output = await aws({ projectOutput: false }).build({
      root: '.',
      outDir: '.ruvyxa',
    })
    assert.equal(
      output.artifacts?.some((artifact) => artifact.scope === 'project'),
      false,
    )
  })
})
