import { describe, it } from 'node:test'
import assert from 'node:assert/strict'

import { cloudflare } from '../../../packages/@ruvyxa/adapter-cloudflare/dist/index.js'

describe('cloudflare', () => {
  it('returns edge deployment output with worker function', async () => {
    const output = await cloudflare({ compatibilityDate: '2024-12-01' }).build({
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
      ],
    )

    // Every static-site artifact must tolerate builds with no prerendered
    // pages (API-only or all-SSR apps) instead of failing with RUV2202.
    assert.ok(
      output.artifacts
        ?.filter((artifact) => artifact.kind === 'static-site')
        .every((artifact) => artifact.optional === true),
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
    assert.match(String(functionArtifact!.handlerSource), /loadRouteModule/)
    assert.doesNotMatch(String(functionArtifact!.handlerSource), /\.\/server\/app/)
    assert.match(String(functionArtifact!.handlerSource), /export default/)

    // Verify _headers for client cache (hashed bundles live under
    // /__ruvyxa/client/)
    const headersArtifact = output.artifacts?.find(
      (artifact) => artifact.path === 'deploy/cloudflare/assets/_headers',
    )
    assert.match(
      headersArtifact && 'contents' in headersArtifact ? String(headersArtifact.contents) : '',
      /\/__ruvyxa\/client\/\*\n {2}Cache-Control: public, max-age=31536000, immutable/,
    )

    // Opt-in project-scope wrangler.jsonc embeds project-relative paths only —
    // the file is committed, so an absolute build-machine path would break
    // every other machine.
    const optIn = await cloudflare({
      projectConfig: true,
      compatibilityDate: '2024-12-01',
    }).build({ root: 'D:\\work\\site', outDir: 'D:\\work\\site\\.ruvyxa' })
    const projectConfig = optIn.artifacts?.find((artifact) => artifact.path === 'wrangler.jsonc')
    assert.ok(projectConfig)
    assert.equal(projectConfig?.skipIfExists, true)
    const projectWrangler = JSON.parse(
      projectConfig && 'contents' in projectConfig ? String(projectConfig.contents) : '{}',
    )
    assert.equal(projectWrangler.main, '.ruvyxa/deploy/cloudflare/worker/index.mjs')
    assert.deepEqual(projectWrangler.assets, { directory: '.ruvyxa/deploy/cloudflare/assets' })
    assert.ok(projectWrangler.compatibility_date)

    // Default: no project-scope artifacts at all
    assert.equal(
      output.artifacts?.some((artifact) => artifact.scope === 'project'),
      false,
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
    const adapter = cloudflare()
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'api'])
  })

  // The default used to be `new Date()`, so two builds of the same commit
  // produced different Workers, and a build machine ahead of the deploy
  // machine's workerd emitted a compatibility date wrangler rejects.
  it('pins a fixed default compatibility date', async () => {
    const compatibilityDate = async () => {
      const output = await cloudflare().build({ root: '.', outDir: '.ruvyxa' })
      const wrangler = output.artifacts?.find((artifact) =>
        artifact.path.endsWith('wrangler.jsonc'),
      )
      return JSON.parse(wrangler && 'contents' in wrangler ? String(wrangler.contents) : '{}')
        .compatibility_date
    }

    assert.equal(await compatibilityDate(), await compatibilityDate())
    assert.match(await compatibilityDate(), /^\d{4}-\d{2}-\d{2}$/)
    // A date ahead of the deploy machine's workerd is rejected by wrangler.
    assert.ok((await compatibilityDate()) <= new Date().toISOString().slice(0, 10))
  })

  it('forwards the runtime context and caches public assets', async () => {
    const output = await cloudflare().build({ root: '.', outDir: '.ruvyxa' })

    // waitUntil lives on the Workers execution context; dropping it stranded
    // any background work the shared handler schedules.
    const worker = output.artifacts?.find((artifact) => artifact.kind === 'function')
    assert.match(
      String(worker && 'handlerSource' in worker ? worker.handlerSource : ''),
      /handler\(request, ctx\)/,
    )

    // Workers default static assets to `max-age=0, must-revalidate`.
    const headers = output.artifacts?.find((artifact) => artifact.path.endsWith('_headers'))
    const contents = headers && 'contents' in headers ? String(headers.contents) : ''
    assert.match(contents, /\/\*\.webp\n {2}Cache-Control: public, max-age=3600, must-revalidate/)
    assert.doesNotMatch(contents, /^\/\*\.js$/m)
  })

  it('embeds validated edge middleware, i18n, and on-demand image wiring', async () => {
    const output = await cloudflare().build({
      root: '.',
      outDir: '.ruvyxa',
      buildInfo: {
        runtime: {
          middleware: { builtin: { timing: true, headers: { 'x-edge': 'yes' } } },
          image: { onDemand: true, maxWidth: 2048 },
        },
      },
    })
    const worker = output.artifacts?.find((artifact) => artifact.kind === 'function')
    const source = String(worker?.handlerSource ?? '')

    assert.match(source, /middleware: runtimePolicy\.middleware/)
    assert.match(source, /i18n: manifest\.i18n/)
    assert.match(source, /onDemand === true \? optimizeImage/)
    assert.match(source, /"maxWidth":2048/)
    assert.match(source, /cf: \{ image:/)
    assert.doesNotMatch(source, /node:/)
  })

  /**
   * `runtime/request-context.mjs` needs `node:async_hooks` to isolate
   * concurrent renders; without it the single-slot fallback refuses the second
   * overlapping request that reads cookies or headers. Cloudflare puts the
   * Node.js APIs behind `nodejs_compat` for every compatibility date before
   * 2026-08-04, which is the range this adapter's default sits in.
   */
  it('enables Node.js compatibility in every wrangler config it writes', async () => {
    const output = await cloudflare({ projectConfig: true }).build({
      root: '/srv/app',
      outDir: '/srv/app/.ruvyxa',
    })
    const wranglerArtifacts =
      output.artifacts?.filter((artifact) => artifact.path.endsWith('wrangler.jsonc')) ?? []
    assert.equal(wranglerArtifacts.length, 2)
    for (const artifact of wranglerArtifacts) {
      const config = JSON.parse(
        artifact && 'contents' in artifact ? String(artifact.contents) : '{}',
      )
      assert.deepEqual(config.compatibility_flags, ['nodejs_compat'], artifact.path)
    }
  })
})
