import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { vercelAdapter } from '../../../packages/@ruvyxa/adapter-vercel/src/index.ts'

describe('vercelAdapter', () => {
  it('returns serverless deployment output with function artifacts', async () => {
    const output = await vercelAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'static-site', path: 'deploy/vercel/.vercel/output/static', scope: undefined },
        {
          kind: 'function',
          path: 'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func',
          scope: undefined,
        },
        {
          kind: 'file',
          path: 'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
          scope: undefined,
        },
        { kind: 'file', path: 'deploy/vercel/.vercel/output/config.json', scope: undefined },
        { kind: 'static-site', path: '.vercel/output/static', scope: 'project' },
        {
          kind: 'function',
          path: '.vercel/output/functions/__ruvyxa_handler.func',
          scope: 'project',
        },
        {
          kind: 'file',
          path: '.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
          scope: 'project',
        },
        { kind: 'file', path: '.vercel/output/config.json', scope: 'project' },
      ],
    )

    // Verify Build Output API config
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
    assert.deepEqual(config.routes[1], { handle: 'filesystem' })
    assert.deepEqual(config.routes[2], { src: '/(.*)', dest: '/__ruvyxa_handler' })

    // Verify function config
    const vcConfig = output.artifacts?.find(
      (artifact) =>
        artifact.path ===
        'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
    )
    const funcConfig = JSON.parse(
      vcConfig && 'contents' in vcConfig ? String(vcConfig.contents) : '{}',
    )
    assert.equal(funcConfig.runtime, 'nodejs20.x')
    assert.equal(funcConfig.handler, 'index.mjs')
    assert.equal(funcConfig.maxDuration, 10)

    // Verify function artifact has handler source
    const functionArtifact = output.artifacts?.find(
      (artifact) =>
        artifact.kind === 'function' &&
        artifact.path === 'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func',
    )
    assert.ok(functionArtifact)
    assert.ok('handlerSource' in functionArtifact!)
    assert.match(String(functionArtifact!.handlerSource), /createHandler/)
    assert.match(String(functionArtifact!.handlerSource), /export default/)

    // Project and build config should match
    const projectConfig = output.artifacts?.find(
      (artifact) => artifact.path === '.vercel/output/config.json',
    )
    assert.equal(
      projectConfig && 'contents' in projectConfig ? String(projectConfig.contents) : '',
      configArtifact && 'contents' in configArtifact ? String(configArtifact.contents) : 'x',
    )

    // Verify projectOutput: false disables project-scope artifacts
    assert.deepEqual(
      vercelAdapter({ projectOutput: false })
        .build({ root: '.', outDir: '.ruvyxa' })
        .artifacts?.map(({ path }) => path),
      [
        'deploy/vercel/.vercel/output/static',
        'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func',
        'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
        'deploy/vercel/.vercel/output/config.json',
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

  it('declares supported strategies', () => {
    const adapter = vercelAdapter()
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
  })

  it('allows custom runtime and maxDuration', () => {
    const output = vercelAdapter({ runtime: 'nodejs22.x', maxDuration: 30 }).build({
      root: '.',
      outDir: '.ruvyxa',
    })
    const vcConfig = output.artifacts?.find((a) => a.path.endsWith('.vc-config.json'))
    const config = JSON.parse(vcConfig && 'contents' in vcConfig ? String(vcConfig.contents) : '{}')
    assert.equal(config.runtime, 'nodejs22.x')
    assert.equal(config.maxDuration, 30)
  })
})
