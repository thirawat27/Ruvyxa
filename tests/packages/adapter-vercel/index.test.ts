import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { Readable, Writable } from 'node:stream'
import { copyFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

import { repoRoot } from '../../repo-root.ts'
import { staticAssetPattern } from '../../../packages/@ruvyxa/core/dist/utils.js'
import { vercel } from '../../../packages/@ruvyxa/adapter-vercel/dist/index.js'

const workspaceRoot = repoRoot

// Read from the handler rather than restated here, so a new sibling reaches
// this test the moment it reaches a real function bundle.
const { HANDLER_RUNTIME_FILES: handlerRuntimeFiles } = (await import(
  pathToFileURL(path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')).href
)) as { HANDLER_RUNTIME_FILES: readonly string[] }

describe('vercel', () => {
  it('returns serverless deployment output with function artifacts', async () => {
    const output = await vercel().build({ root: '.', outDir: '.ruvyxa' })

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

    // Every static-site artifact must tolerate builds with no prerendered
    // pages (API-only or all-SSR apps) instead of failing with RUV2202.
    assert.ok(
      output.artifacts
        ?.filter((artifact) => artifact.kind === 'static-site')
        .every((artifact) => artifact.optional === true),
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
      src: '^/__ruvyxa/client/(.*)$',
      headers: { 'cache-control': 'public, max-age=31536000, immutable' },
      continue: true,
    })
    // Public assets carry a revalidating cache header instead of Vercel's
    // `max-age=0`, and `/__ruvyxa/` is excluded so the immutable header set by
    // routes[0] is not overwritten with the shorter lifetime.
    assert.deepEqual(config.routes[1], {
      src: staticAssetPattern(),
      headers: { 'cache-control': 'public, max-age=3600, must-revalidate' },
      continue: true,
    })
    assert.doesNotMatch('/__ruvyxa/client/app.js', new RegExp(staticAssetPattern()))
    assert.match('/logo.png', new RegExp(staticAssetPattern()))
    assert.deepEqual(config.routes[2], { handle: 'filesystem' })
    // A filesystem miss on an asset path is a 404, never a page render: this
    // is what kept `/logo.png` returning a 200 HTML document from `/[lang]`.
    assert.deepEqual(config.routes[3], { src: staticAssetPattern(), status: 404 })
    assert.deepEqual(config.routes[4], { src: '/(.*)', dest: '/__ruvyxa_handler' })

    // ISR and PPR pages must stay out of the publish directory, or
    // `handle: filesystem` answers them before the function can revalidate.
    for (const artifact of output.artifacts?.filter((item) => item.kind === 'static-site') ?? []) {
      assert.deepEqual(artifact.excludeStrategies, ['isr', 'ppr'])
    }

    // Verify function config
    const vcConfig = output.artifacts?.find(
      (artifact) =>
        artifact.path ===
        'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func/.vc-config.json',
    )
    const funcConfig = JSON.parse(
      vcConfig && 'contents' in vcConfig ? String(vcConfig.contents) : '{}',
    )
    assert.equal(funcConfig.runtime, 'nodejs24.x')
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
    assert.match(String(functionArtifact!.handlerSource), /loadRouteModule/)
    assert.doesNotMatch(String(functionArtifact!.handlerSource), /\.\/server\/app/)
    assert.match(String(functionArtifact!.handlerSource), /export default/)
    assert.match(String(functionArtifact!.handlerSource), /for await \(const chunk of req\)/)
    assert.match(String(functionArtifact!.handlerSource), /getSetCookie/)
    assert.doesNotMatch(String(functionArtifact!.handlerSource), /ISR cache write failures/)

    // The ISR cache reads and writes files by request path, so it must go
    // through the shared containment helper rather than joining the raw
    // pathname onto the cache directory.
    assert.match(String(functionArtifact!.handlerSource), /prerenderRelativePath/)
    assert.doesNotMatch(
      String(functionArtifact!.handlerSource),
      /path\.join\(prerenderDir, pathname/,
    )

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
      (
        await vercel({ projectOutput: false }).build({ root: '.', outDir: '.ruvyxa' })
      ).artifacts?.map(({ path }) => path),
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
    const adapter = vercel()
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
  })

  it('allows custom runtime and maxDuration', async () => {
    const output = await vercel({ runtime: 'nodejs22.x', maxDuration: 30 }).build({
      root: '.',
      outDir: '.ruvyxa',
    })
    const vcConfig = output.artifacts?.find((a) => a.path.endsWith('.vc-config.json'))
    const config = JSON.parse(vcConfig && 'contents' in vcConfig ? String(vcConfig.contents) : '{}')
    assert.equal(config.runtime, 'nodejs22.x')
    assert.equal(config.maxDuration, 30)
    // Unset by default so Vercel's own region selection applies.
    assert.equal('regions' in config, false)
  })

  it('pins function regions when asked, and rejects malformed region lists', async () => {
    const output = await vercel({ regions: ['sin1'] }).build({ root: '.', outDir: '.ruvyxa' })
    const vcConfig = output.artifacts?.find((a) => a.path.endsWith('.vc-config.json'))
    const config = JSON.parse(vcConfig && 'contents' in vcConfig ? String(vcConfig.contents) : '{}')
    assert.deepEqual(config.regions, ['sin1'])

    assert.throws(() => vercel({ regions: [] }), /RUV2001/)
    assert.throws(() => vercel({ regions: [''] }), /RUV2001/)
  })

  it('emits a Web-standard Edge Function with validated runtime policy', async () => {
    const adapter = vercel({ edge: true, regions: ['sin1'], projectOutput: false })
    const output = await adapter.build({
      root: '.',
      outDir: '.ruvyxa',
      buildInfo: {
        runtime: {
          middleware: { builtin: { timing: true, rate: { max: 10, window: 60 } } },
        },
      },
    })
    assert.equal(output.target, 'edge')
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'api'])

    const vcConfigArtifact = output.artifacts?.find((item) => item.path.endsWith('.vc-config.json'))
    const vcConfig = JSON.parse(String(vcConfigArtifact?.contents ?? '{}'))
    assert.deepEqual(vcConfig, {
      runtime: 'edge',
      entrypoint: 'index.mjs',
      regions: ['sin1'],
    })

    const functionArtifact = output.artifacts?.find((item) => item.kind === 'function')
    const source = String(functionArtifact?.handlerSource ?? '')
    assert.match(source, /middleware: runtimePolicy\.middleware/)
    assert.match(source, /i18n: manifest\.i18n/)
    assert.match(source, /supportedStrategies: \['ssr', 'ssg', 'csr', 'api'\]/)
    assert.doesNotMatch(source, /node:/)
    assert.doesNotMatch(source, /Buffer|process\.|readFileSync/)

    assert.throws(() => vercel({ edge: true, runtime: 'nodejs22.x' }), /RUV2001/)
    assert.throws(() => vercel({ edge: true, maxDuration: 30 }), /RUV2001/)
  })

  it('configures Vercel native same-origin image optimization on demand', async () => {
    const output = await vercel({ projectOutput: false }).build({
      root: '.',
      outDir: '.ruvyxa',
      buildInfo: {
        runtime: {
          image: { onDemand: true, maxWidth: 2048, sizes: [640, 828, 2048, 3840] },
        },
      },
    })
    const configArtifact = output.artifacts?.find((item) =>
      item.path.endsWith('/output/config.json'),
    )
    const config = JSON.parse(String(configArtifact?.contents ?? '{}'))
    assert.deepEqual(config.images, {
      sizes: [640, 828, 2048],
      domains: [],
      minimumCacheTTL: 86400,
      formats: ['image/avif', 'image/webp'],
      localPatterns: [{ pathname: '^/(?!__ruvyxa/).*$' }],
    })
    const functionArtifact = output.artifacts?.find((item) => item.kind === 'function')
    const source = String(functionArtifact?.handlerSource ?? '')
    assert.match(source, /new URL\('\/_vercel\/image'/)
    assert.match(source, /onDemand === true \? optimizeImage/)
  })

  it('forwards streamed requests, repeated Set-Cookie headers, and binary responses', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-vercel-handler-'))
    try {
      const output = await vercel({ projectOutput: false }).build({ root, outDir: '.ruvyxa' })
      const artifact = output.artifacts?.find((item) => item.kind === 'function')
      assert.ok(artifact?.handlerSource)
      await mkdir(path.join(root, 'prerender'), { recursive: true })
      await writeFile(path.join(root, 'index.mjs'), artifact.handlerSource)
      // The handler imports the manifest as a module, the way adapter-runner
      // emits it, so platform bundlers keep it in the deployed function.
      const manifest = {
        routes: [
          {
            id: 'app/api/echo/route',
            kind: 'api',
            path: '/api/echo',
            file: 'app/api/echo/route.ts',
            render: { strategy: 'ssr' },
          },
        ],
      }
      await writeFile(path.join(root, 'manifest.json'), JSON.stringify(manifest))
      await writeFile(
        path.join(root, 'manifest.mjs'),
        `export default ${JSON.stringify(manifest)}\n`,
      )
      await writeFile(
        path.join(root, 'route-modules.mjs'),
        `const api = { async POST({ request }) {
          if (request.headers.get('x-binary') === '1') {
            return new Response(Uint8Array.from([0, 128, 255, 65]), {
              headers: { 'content-type': 'application/octet-stream' },
            })
          }
          const headers = new Headers()
          headers.append('set-cookie', 'first=1; Path=/')
          headers.append('set-cookie', 'second=2; Path=/')
          return new Response(await request.text(), { headers })
        } }
        export async function loadRouteModule() { return api }
        // The generated registry exports these too; the Vercel handler imports
        // all three, so a stub that omits them fails at module load.
        export async function loadActionModule() { return null }
        export const applyPluginHttp = undefined
        `,
      )
      // The handler and its siblings travel together: `adapter-runner.mjs`
      // copies the whole set into a function directory, because the handler
      // imports them as siblings and a deployed function resolves no bare
      // specifiers. The list comes from the handler itself rather than being
      // repeated here — a local copy passed this test while shipping a bundle
      // that threw on its first request.
      for (const runtimeFile of handlerRuntimeFiles) {
        await copyFile(
          path.join(workspaceRoot, 'packages/ruvyxa/runtime', runtimeFile),
          path.join(root, runtimeFile),
        )
      }

      const { default: handler } = await import(
        pathToFileURL(path.join(root, 'index.mjs')).href + `?t=${Date.now()}`
      )
      const request = Readable.from([Buffer.from('streamed-payload')])
      Object.assign(request, {
        url: '/api/echo',
        method: 'POST',
        headers: { host: 'localhost', 'content-type': 'text/plain' },
      })
      // A real `ServerResponse` is what the platform launcher passes, and the
      // handler now pipes into it rather than handing `end()` one buffer. The
      // double is a Writable for the same reason: a plain object with `end`
      // would accept a handler that cannot stream at all.
      const createResponse = () => {
        const chunks: Buffer[] = []
        const headers = new Map<string, unknown>()
        const response = Object.assign(
          new Writable({
            write(chunk: Buffer, _encoding: string, callback: () => void) {
              chunks.push(Buffer.from(chunk))
              callback()
            },
          }),
          {
            statusCode: 0,
            setHeader(name: string, value: unknown) {
              headers.set(name, value)
            },
          },
        )
        return { response, headers, body: () => Buffer.concat(chunks) }
      }

      const first = createResponse()
      await handler(request, first.response)

      assert.equal(first.response.statusCode, 200)
      assert.equal(first.body().toString(), 'streamed-payload')
      assert.deepEqual(first.headers.get('set-cookie'), ['first=1; Path=/', 'second=2; Path=/'])

      const parsedRequest = Readable.from([])
      Object.assign(parsedRequest, {
        url: '/api/echo',
        method: 'POST',
        headers: { host: 'localhost', 'content-type': 'application/json' },
        body: { parsed: true },
      })
      const parsed = createResponse()
      await handler(parsedRequest, parsed.response)
      assert.equal(parsed.body().toString(), '{"parsed":true}')

      const binaryRequest = Readable.from([])
      Object.assign(binaryRequest, {
        url: '/api/echo',
        method: 'POST',
        headers: { host: 'localhost', 'x-binary': '1' },
      })
      const binary = createResponse()
      await handler(binaryRequest, binary.response)
      assert.deepEqual(binary.body(), Buffer.from([0, 128, 255, 65]))
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  /**
   * Buffering the whole response held it in the function's memory and delayed
   * the first byte until the last one existed, which is the wrong shape for a
   * streamed PPR shell or a large API body. The handler pipes instead, and the
   * platform only forwards bytes early when the function config says so.
   */
  it('streams the response body instead of buffering it', async () => {
    const output = await vercel().build({ root: '.', outDir: '.ruvyxa' })

    const handler = output.artifacts?.find(
      (artifact) => artifact.kind === 'function' && artifact.path.startsWith('deploy/'),
    )
    const source = String(handler && 'handlerSource' in handler ? handler.handlerSource : '')
    assert.match(source, /pipeline\(Readable\.fromWeb\(response\.body\), res\)/)
    assert.doesNotMatch(source, /Buffer\.from\(await response\.arrayBuffer\(\)\)/)

    const vcConfigArtifact = output.artifacts?.find((item) => item.path.endsWith('.vc-config.json'))
    const vcConfig = JSON.parse(
      vcConfigArtifact && 'contents' in vcConfigArtifact ? String(vcConfigArtifact.contents) : '{}',
    )
    assert.equal(vcConfig.supportsResponseStreaming, true)
    assert.equal(vcConfig.launcherType, 'Nodejs')
  })

  // An edge function returns the Response itself, so there is nothing to
  // configure a launcher or streaming for.
  it('keeps the edge function config to the documented edge fields', async () => {
    const output = await vercel({ edge: true }).build({ root: '.', outDir: '.ruvyxa' })
    const vcConfigArtifact = output.artifacts?.find((item) => item.path.endsWith('.vc-config.json'))
    const vcConfig = JSON.parse(
      vcConfigArtifact && 'contents' in vcConfigArtifact ? String(vcConfigArtifact.contents) : '{}',
    )
    assert.deepEqual(Object.keys(vcConfig).sort(), ['entrypoint', 'runtime'])
    assert.equal(vcConfig.runtime, 'edge')
  })
})
