import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { DEFAULT_SECURITY_HEADERS } from '../../../packages/@ruvyxa/core/dist/utils.js'
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
        { kind: 'file', path: 'deploy/aws/customHttp.yml', scope: undefined },
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
        { kind: 'file', path: 'customHttp.yml', scope: 'project' },
      ],
    )

    // Amplify's route targets carry `cacheControl` and nothing else, so the
    // security defaults have no place in the deploy manifest — and a `Static`
    // target is answered by the CDN, which never invokes the compute resource
    // where the standalone server sets them. `customHttp.yml` is the mechanism
    // Amplify documents for it, and without the file a deployed pre-rendered
    // page carried none of the seven.
    const customHttp = output.artifacts?.find((artifact) => artifact.path === 'customHttp.yml')
    // A project may already have written its own Amplify rules into this file.
    assert.equal(customHttp && 'skipIfExists' in customHttp ? customHttp.skipIfExists : false, true)
    const yaml = String(customHttp && 'contents' in customHttp ? customHttp.contents : '')
    assert.match(yaml, /^customHeaders:\n {2}- pattern: '\*\*'\n {4}headers:\n/)
    for (const [key, value] of Object.entries(DEFAULT_SECURITY_HEADERS)) {
      assert.ok(yaml.includes(`      - key: '${key}'\n        value: '${value}'\n`), key)
    }

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
