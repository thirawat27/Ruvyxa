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
        { kind: 'function', path: '.netlify/v1/functions/ruvyxa-handler', scope: 'project' },
        { kind: 'file', path: '.netlify/v1/config.json', scope: 'project' },
      ],
    )

    // Every static-site artifact must tolerate builds with no prerendered
    // pages (API-only or all-SSR apps) instead of failing with RUV2202.
    assert.ok(
      output.artifacts
        ?.filter((artifact) => artifact.kind === 'static-site')
        .every((artifact) => artifact.optional === true),
    )

    // Verify netlify.toml includes functions directory
    const toml = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/netlify/netlify.toml',
    )
    assert.match(toml && 'contents' in toml ? String(toml.contents) : '', /functions = "functions"/)
    assert.match(
      toml && 'contents' in toml ? String(toml.contents) : '',
      /for = "\/__ruvyxa\/client\/\*"[\s\S]*Cache-Control = "public, max-age=31536000, immutable"/,
    )

    // Frameworks API config carries the immutable cache header for hashed
    // client bundles; Netlify discovers it at .netlify/v1/config.json.
    const frameworksConfigArtifact = output.artifacts?.find(
      (artifact) => artifact.path === '.netlify/v1/config.json',
    )
    assert.ok(frameworksConfigArtifact)
    const frameworksConfig = JSON.parse(
      frameworksConfigArtifact && 'contents' in frameworksConfigArtifact
        ? String(frameworksConfigArtifact.contents)
        : '{}',
    )
    assert.deepEqual(frameworksConfig.headers, [
      {
        for: '/__ruvyxa/client/*',
        values: { 'Cache-Control': 'public, max-age=31536000, immutable' },
      },
    ])

    // Verify function artifacts share the handler source
    for (const functionPath of [
      'deploy/netlify/functions/ruvyxa-handler',
      '.netlify/v1/functions/ruvyxa-handler',
    ]) {
      const functionArtifact = output.artifacts?.find(
        (artifact) => artifact.kind === 'function' && artifact.path === functionPath,
      )
      assert.ok(functionArtifact, functionPath)
      assert.ok('handlerSource' in functionArtifact!)
      assert.match(String(functionArtifact!.handlerSource), /createHandler/)
      assert.match(String(functionArtifact!.handlerSource), /loadRouteModule/)
      assert.doesNotMatch(String(functionArtifact!.handlerSource), /\.\/server\/app/)
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
    }

    // Opt-in project netlify.toml embeds project-relative paths only — the
    // file is committed, so an absolute build-machine path would break every
    // other machine (and Windows backslashes are TOML escapes).
    const optIn = await netlifyAdapter({ projectConfig: true }).build({
      root: 'D:\\work\\site',
      outDir: 'D:\\work\\site\\.ruvyxa',
    })
    const projectToml = optIn.artifacts?.find((artifact) => artifact.path === 'netlify.toml')
    assert.ok(projectToml)
    assert.equal(projectToml?.skipIfExists, true)
    assert.equal(projectToml?.scope, 'project')
    const projectTomlContents =
      projectToml && 'contents' in projectToml ? String(projectToml.contents) : ''
    assert.match(projectTomlContents, /publish = "\.ruvyxa\/deploy\/netlify\/publish"/)
    assert.match(projectTomlContents, /functions = "\.ruvyxa\/deploy\/netlify\/functions"/)
    assert.doesNotMatch(projectTomlContents, /D:\\/)

    // projectConfig defaults to off: no root netlify.toml artifact
    assert.equal(
      output.artifacts?.some((artifact) => artifact.path === 'netlify.toml'),
      false,
    )

    // frameworksApi: false drops the .netlify/v1 artifacts
    assert.deepEqual(
      (
        await netlifyAdapter({ frameworksApi: false }).build({ root: '.', outDir: '.ruvyxa' })
      ).artifacts?.map(({ path }) => path),
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
