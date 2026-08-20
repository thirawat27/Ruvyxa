import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import type { AdapterArtifact } from '../../../packages/@ruvyxa/core/dist/types.js'
import { netlify } from '../../../packages/@ruvyxa/adapter-netlify/dist/index.js'

describe('netlify', () => {
  it('returns serverless deployment output with function artifacts', async () => {
    const output = await netlify().build({ root: '.', outDir: '.ruvyxa' })

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
    assert.deepEqual(frameworksConfig.headers[0], {
      for: '/__ruvyxa/client/*',
      values: { 'Cache-Control': 'public, max-age=31536000, immutable' },
    })
    // Public assets are not content-hashed, so they revalidate hourly rather
    // than inheriting Netlify's per-request default. The rules deliberately
    // skip js/css: on hosts whose `*` crosses path separators they would also
    // match the hashed bundles and downgrade their immutable header.
    const assetRules = frameworksConfig.headers.slice(1)
    assert.ok(assetRules.length > 0)
    assert.ok(
      assetRules.every(
        (rule: { for: string; values: Record<string, string> }) =>
          rule.values['Cache-Control'] === 'public, max-age=3600, must-revalidate',
      ),
    )
    assert.ok(assetRules.some((rule: { for: string }) => rule.for === '/*.png'))
    assert.equal(
      assetRules.some((rule: { for: string }) => rule.for === '/*.js'),
      false,
    )

    // Verify function artifacts share the handler source
    for (const functionPath of [
      'deploy/netlify/functions/ruvyxa-handler',
      '.netlify/v1/functions/ruvyxa-handler',
    ]) {
      const functionArtifact: AdapterArtifact | undefined = (output.artifacts ?? []).find(
        (artifact) => artifact.kind === 'function' && artifact.path === functionPath,
      )
      assert.ok(functionArtifact, functionPath)
      // Annotated because the narrowed property and the binding share a name,
      // which TypeScript reads as a circular initializer and widens to `any`.
      const handlerSource: string =
        'handlerSource' in functionArtifact ? String(functionArtifact.handlerSource) : ''
      assert.notEqual(handlerSource, '', functionPath)
      assert.match(handlerSource, /createHandler/)
      assert.match(handlerSource, /loadRouteModule/)
      assert.doesNotMatch(handlerSource, /\.\/server\/app/)
      assert.match(handlerSource, /export default/)

      // The ISR cache reads and writes files by request path, so it must go
      // through the shared containment helper rather than joining the raw
      // pathname onto the cache directory.
      assert.match(handlerSource, /prerenderRelativePath/)
      assert.doesNotMatch(handlerSource, /ISR cache write failures/)
      assert.doesNotMatch(handlerSource, /path\.join\(prerenderDir, pathname/)
      // Netlify Functions v2 config export
      assert.match(handlerSource, /export const config/)
      assert.match(handlerSource, /"preferStatic": true/)
      assert.match(handlerSource, /"path": "\/\*"/)

      // Framework attribution: Netlify reads `generator` to tell a
      // framework-emitted function from a hand-written one, and shows `name`
      // in the site UI.
      assert.match(handlerSource, /"generator": "ruvyxa\/\d+\.\d+\.\d+/)
      assert.match(handlerSource, /"name": "Ruvyxa SSR"/)

      // The deploy-time prerender output is not in the module graph, so it
      // only survives esbuild bundling if the config declares it. Each
      // artifact names its own location, because the glob resolves against
      // the site's base directory rather than the function directory.
      assert.ok(
        handlerSource.includes(`"${functionPath.replace('deploy/netlify/', '')}/prerender/**"`),
        `includedFiles must cover the prerender directory of ${functionPath}`,
      )

      // Netlify bundles the function with esbuild, so the manifest has to be
      // part of the module graph. Reading a sibling manifest.json at runtime
      // crashed the deployed function with ENOENT /var/task/manifest.json.
      assert.match(handlerSource, /import manifest from '\.\/manifest\.mjs'/)
      assert.doesNotMatch(handlerSource, /readFileSync\(manifestPath/)
    }

    // preferStatic serves a published page without invoking the function, so
    // ISR and PPR pages must stay out of the publish directory to revalidate.
    for (const artifact of output.artifacts?.filter((item) => item.kind === 'static-site') ?? []) {
      assert.deepEqual(artifact.excludeStrategies, ['isr', 'ppr'])
    }

    // Opt-in project netlify.toml embeds project-relative paths only — the
    // file is committed, so an absolute build-machine path would break every
    // other machine (and Windows backslashes are TOML escapes).
    const optIn = await netlify({ projectConfig: true }).build({
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
        await netlify({ frameworksApi: false }).build({ root: '.', outDir: '.ruvyxa' })
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
    const adapter = netlify()
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
  })
})
