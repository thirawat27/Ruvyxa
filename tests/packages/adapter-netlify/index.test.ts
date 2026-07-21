import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { netlifyAdapter } from '../../../packages/@ruvyxa/adapter-netlify/src/index.ts'

describe('netlifyAdapter', () => {
  it('returns serverless deployment output with function artifacts', async () => {
    const output = await netlifyAdapter().build({ root: '.', outDir: '.ruvyxa' })

    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'static-site', path: 'deploy/netlify/publish', scope: undefined },
        { kind: 'function', path: 'deploy/netlify/functions/ruvyxa-handler', scope: undefined },
        { kind: 'file', path: 'deploy/netlify/netlify.toml', scope: undefined },
        { kind: 'file', path: 'netlify.toml', scope: 'project' },
      ],
    )

    // Verify netlify.toml includes functions directory
    const toml = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/netlify/netlify.toml',
    )
    assert.match(toml && 'contents' in toml ? String(toml.contents) : '', /functions = "functions"/)
    assert.match(
      toml && 'contents' in toml ? String(toml.contents) : '',
      /for = "\/client\/\*"[\s\S]*Cache-Control = "public, max-age=31536000, immutable"/,
    )

    // Verify project-scope netlify.toml
    const projectToml = output.artifacts?.find((artifact) => artifact.path === 'netlify.toml')
    assert.equal(projectToml?.skipIfExists, true)
    assert.match(
      projectToml && 'contents' in projectToml ? String(projectToml.contents) : '',
      /publish = "\.ruvyxa\/deploy\/netlify\/publish"/,
    )
    assert.match(
      projectToml && 'contents' in projectToml ? String(projectToml.contents) : '',
      /functions = "\.ruvyxa\/deploy\/netlify\/functions"/,
    )

    // Verify function artifact has handler source
    const functionArtifact = output.artifacts?.find(
      (artifact) =>
        artifact.kind === 'function' && artifact.path === 'deploy/netlify/functions/ruvyxa-handler',
    )
    assert.ok(functionArtifact)
    assert.ok('handlerSource' in functionArtifact!)
    assert.match(String(functionArtifact!.handlerSource), /createHandler/)
    assert.match(String(functionArtifact!.handlerSource), /export default/)

    // The ISR cache reads and writes files by request path, so it must go
    // through the shared containment helper rather than joining the raw
    // pathname onto the cache directory.
    assert.match(String(functionArtifact!.handlerSource), /prerenderRelativePath/)
    assert.doesNotMatch(
      String(functionArtifact!.handlerSource),
      /path\.join\(prerenderDir, pathname/,
    )
    // Netlify Functions v2 config export
    assert.match(String(functionArtifact!.handlerSource), /export const config/)
    assert.match(String(functionArtifact!.handlerSource), /preferStatic: true/)

    // Verify projectConfig: false
    assert.deepEqual(
      netlifyAdapter({ projectConfig: false })
        .build({ root: '.', outDir: '.ruvyxa' })
        .artifacts?.map(({ path }) => path),
      [
        'deploy/netlify/publish',
        'deploy/netlify/functions/ruvyxa-handler',
        'deploy/netlify/netlify.toml',
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

  it('declares supported strategies', () => {
    const adapter = netlifyAdapter()
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
  })
})
