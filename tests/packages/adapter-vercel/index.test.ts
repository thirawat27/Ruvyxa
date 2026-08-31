import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { Readable, Writable } from 'node:stream'
import { copyFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

import { repoRoot } from '../../repo-root.ts'
import {
  DEFAULT_SECURITY_HEADERS,
  staticAssetPattern,
} from '../../../packages/@ruvyxa/core/dist/utils.js'
import { nonPublishableStrategies } from '../../../packages/@ruvyxa/core/dist/deploy-manifest.js'
import type { BuildContext } from '../../../packages/@ruvyxa/core/dist/index.js'
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
    // First, and on everything: `handle: filesystem` below answers a
    // pre-rendered document and every public file from Vercel's own edge, so
    // `createHandler` — the only other place these are set — is never invoked
    // for them, and a deployed SSG page carried none of them.
    assert.deepEqual(config.routes[0], {
      src: '/(.*)',
      headers: DEFAULT_SECURITY_HEADERS,
      continue: true,
    })
    assert.deepEqual(config.routes[1], {
      src: '^/__ruvyxa/client/(.*)$',
      headers: { 'cache-control': 'public, max-age=31536000, immutable' },
      continue: true,
    })
    // Public assets carry a revalidating cache header instead of Vercel's
    // `max-age=0`, and `/__ruvyxa/` is excluded so the immutable header set by
    // routes[0] is not overwritten with the shorter lifetime.
    assert.deepEqual(config.routes[2], {
      src: staticAssetPattern(),
      headers: { 'cache-control': 'public, max-age=3600, must-revalidate' },
      continue: true,
    })
    assert.doesNotMatch('/__ruvyxa/client/app.js', new RegExp(staticAssetPattern()))
    assert.match('/logo.png', new RegExp(staticAssetPattern()))
    assert.deepEqual(config.routes[3], { handle: 'filesystem' })
    // A filesystem miss on an asset path is a 404, never a page render: this
    // is what kept `/logo.png` returning a 200 HTML document from `/[lang]`.
    assert.deepEqual(config.routes[4], { src: staticAssetPattern(), status: 404 })
    assert.deepEqual(config.routes[5], { src: '/(.*)', dest: '/__ruvyxa_handler' })

    // ISR and PPR pages must stay out of the publish directory, or
    // `handle: filesystem` answers them before the function can revalidate.
    //
    // Compared against the derived rule rather than a literal copy of it.
    for (const artifact of output.artifacts?.filter((item) => item.kind === 'static-site') ?? []) {
      assert.deepEqual(artifact.excludeStrategies, nonPublishableStrategies())
      assert.ok(artifact.excludeStrategies?.includes('isr'))
      assert.ok(artifact.excludeStrategies?.includes('ppr'))
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

    // The ISR cache directory must not be a fixed name under `os.tmpdir()`.
    //
    // It was `os.tmpdir()/ruvyxa-isr-cache` — the same directory for every
    // Ruvyxa deployment on the host and for every previous build of this one,
    // read *before* the bundled prerender output, so whatever was there won. A
    // redeploy served the previous build's documents, whose `<script src>`
    // names client chunks the new build no longer publishes. On Linux the
    // parent is mode 1777, so a planted file or symlink at a known route path
    // was served as that page and written through on the next refresh.
    assert.doesNotMatch(
      String(functionArtifact!.handlerSource),
      /tmpdir\(\),\s*'ruvyxa-isr-cache'\s*\)/,
      'the cache directory must carry a per-deployment identity, not a fixed name',
    )
    assert.match(
      String(functionArtifact!.handlerSource),
      /createHash\('sha256'\)[\s\S]{0,200}import\.meta\.dirname/,
      'the identity hashes the build id together with the bundle directory',
    )
    assert.match(
      String(functionArtifact!.handlerSource),
      /mkdirSync\(isrCacheDir, \{ recursive: true, mode: 0o700 \}\)/,
      'created owner-only, because the parent is world-writable on Linux',
    )
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

  it('snaps an on-demand image width to a size Vercel will accept', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-vercel-image-'))
    try {
      const output = await vercel({ projectOutput: false }).build({
        root,
        outDir: '.ruvyxa',
        buildInfo: { runtime: { image: { onDemand: true } } },
      })
      const artifact = output.artifacts?.find((item) => item.kind === 'function')
      assert.ok(artifact?.handlerSource)
      await writeFile(path.join(root, 'index.mjs'), artifact.handlerSource)
      await writeFile(path.join(root, 'manifest.mjs'), 'export default { routes: [] }\n')
      await writeFile(
        path.join(root, 'route-modules.mjs'),
        'export async function loadRouteModule() { return null }\n' +
          'export async function loadActionModule() { return null }\n' +
          'export const applyPluginHttp = undefined\n' +
          'export const documentCacheHandler = null\n',
      )
      for (const runtimeFile of handlerRuntimeFiles) {
        await copyFile(
          path.join(workspaceRoot, 'packages/ruvyxa/runtime', runtimeFile),
          path.join(root, runtimeFile),
        )
      }
      const { default: handler } = await import(
        pathToFileURL(path.join(root, 'index.mjs')).href + `?t=${Date.now()}`
      )
      // `<Image>` puts the author's own `width` into the srcset unsnapped, so a
      // width that is not one of the declared sizes reaches this function.
      // Vercel answers 400 for a `w` its `images.sizes` never listed, so the
      // redirect has to name the nearest size that was declared — otherwise an
      // image that renders under `ruvyxa start` is broken the moment it deploys.
      const request = Readable.from([])
      Object.assign(request, {
        url: '/__ruvyxa/image?src=%2Flogo.png&w=500&q=75',
        method: 'GET',
        headers: { host: 'localhost' },
      })
      const headers = new Map<string, unknown>()
      const response = Object.assign(
        new Writable({
          write(_chunk: Buffer, _encoding: string, callback: () => void) {
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
      await handler(request, response)

      assert.equal(response.statusCode, 307)
      const location = new URL(String(headers.get('location')))
      assert.equal(location.pathname, '/_vercel/image')
      assert.equal(location.searchParams.get('url'), '/logo.png')
      assert.equal(location.searchParams.get('w'), '640')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
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
        export const documentCacheHandler = null
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

describe('vercel per-route edge split', () => {
  /** A deploy manifest with one edge route beside ordinary Node ones. */
  const mixedManifest = {
    routes: [
      {
        id: 'app/page',
        path: '/',
        kind: 'page',
        serve: 'static',
        strategy: 'ssg',
        runtime: 'node',
        serverComponents: false,
      },
      {
        id: 'app/edge/page',
        path: '/edge',
        kind: 'page',
        serve: 'function',
        strategy: 'ssr',
        runtime: 'edge',
        serverComponents: false,
      },
      {
        id: 'app/shop/[id]/page',
        path: '/shop/[id]',
        kind: 'page',
        serve: 'function',
        strategy: 'ssr',
        runtime: 'edge',
        serverComponents: false,
      },
      {
        id: 'app/dash/page',
        path: '/dash',
        kind: 'page',
        serve: 'function',
        strategy: 'ssr',
        runtime: 'node',
        serverComponents: false,
      },
    ],
  } as unknown as BuildContext['deployManifest']

  const ctx = (deployManifest: BuildContext['deployManifest']): BuildContext => ({
    root: '.',
    outDir: '.ruvyxa',
    deployManifest,
  })

  it('emits one function until asked for two', async () => {
    const output = await vercel({ projectOutput: false }).build(ctx(mixedManifest))
    const functions = output.artifacts?.filter((artifact) => artifact.kind === 'function') ?? []
    assert.equal(functions.length, 1, 'the split is opt-in')
  })

  it('gives the edge routes their own function, with their own runtime and routes', async () => {
    const output = await vercel({ splitEdgeRoutes: true, projectOutput: false }).build(
      ctx(mixedManifest),
    )
    const edgeFunction = output.artifacts?.find(
      (artifact) => artifact.kind === 'function' && artifact.path.includes('__ruvyxa_edge'),
    )
    assert.ok(edgeFunction, 'an edge function is emitted')
    assert.equal(edgeFunction?.target, 'edge')
    assert.deepEqual(edgeFunction?.routes, ['app/edge/page', 'app/shop/[id]/page'])

    const vcConfig = output.artifacts?.find((artifact) =>
      artifact.path.includes('__ruvyxa_edge.func/.vc-config.json'),
    )
    assert.deepEqual(JSON.parse(String(vcConfig?.contents)), {
      runtime: 'edge',
      entrypoint: 'index.mjs',
    })
  })

  it('routes edge paths to it, ahead of the catch-all', async () => {
    const output = await vercel({ splitEdgeRoutes: true, projectOutput: false }).build(
      ctx(mixedManifest),
    )
    const config = JSON.parse(
      String(output.artifacts?.find((a) => a.path.endsWith('output/config.json'))?.contents),
    )
    const routes: { src?: string; dest?: string }[] = config.routes
    const edgeAt = routes.findIndex((route) => route.dest === '/__ruvyxa_edge')
    // Found by destination, not by pattern: the security-header rule is also
    // `/(.*)`, and matching on the pattern found that one instead.
    const catchAllAt = routes.findIndex((route) => route.dest === '/__ruvyxa_handler')
    assert.ok(edgeAt >= 0 && edgeAt < catchAllAt, 'an edge rule must precede the catch-all')
    // A dynamic segment becomes a segment matcher, not a literal.
    assert.ok(routes.some((route) => route.src === '^/shop/[^/]+$'))
    assert.ok(routes.some((route) => route.src === '^/edge$'))
  })

  it('refuses an edge route that needs what one function owns', async () => {
    const withRsc = {
      routes: [
        {
          id: 'app/rsc/page',
          path: '/rsc',
          kind: 'page',
          serve: 'function',
          strategy: 'ssr',
          runtime: 'edge',
          serverComponents: true,
        },
      ],
    } as unknown as BuildContext['deployManifest']
    await assert.rejects(
      async () => vercel({ splitEdgeRoutes: true }).build(ctx(withRsc)),
      /RUV2203.*renders server components/s,
    )

    const withIsr = {
      routes: [
        {
          id: 'app/feed/page',
          path: '/feed',
          kind: 'page',
          serve: 'function',
          strategy: 'isr',
          runtime: 'edge',
          serverComponents: false,
        },
      ],
    } as unknown as BuildContext['deployManifest']
    await assert.rejects(
      async () => vercel({ splitEdgeRoutes: true }).build(ctx(withIsr)),
      /RUV2203.*writable document store/s,
    )
  })

  it('ignores an edge declaration on a route served from a file', async () => {
    // Nothing to place: a CDN answers it before any function runs.
    const staticEdge = {
      routes: [
        {
          id: 'app/page',
          path: '/',
          kind: 'page',
          serve: 'static',
          strategy: 'ssg',
          runtime: 'edge',
          serverComponents: false,
        },
      ],
    } as unknown as BuildContext['deployManifest']
    const output = await vercel({ splitEdgeRoutes: true, projectOutput: false }).build(
      ctx(staticEdge),
    )
    assert.equal(
      output.artifacts?.some((artifact) => artifact.path.includes('__ruvyxa_edge')),
      false,
    )
  })
})

describe('vercel prerender functions', () => {
  const manifest = {
    buildId: '0123456789abcdef',
    routes: [
      {
        id: 'app/isr-page/page',
        path: '/isr-page',
        kind: 'page',
        serve: 'function',
        strategy: 'isr',
        runtime: 'node',
        revalidate: 120,
      },
      {
        id: 'app/ppr-page/page',
        path: '/ppr-page',
        kind: 'page',
        serve: 'function',
        strategy: 'ppr',
        runtime: 'node',
        revalidate: null,
      },
      {
        id: 'app/news/[slug]/page',
        path: '/news/[slug]',
        kind: 'page',
        serve: 'function',
        strategy: 'isr',
        runtime: 'node',
        revalidate: 30,
      },
      {
        id: 'app/about/page',
        path: '/about',
        kind: 'page',
        serve: 'static',
        strategy: 'ssg',
        runtime: 'node',
        revalidate: null,
      },
    ],
    prerendered: [
      { path: '/news/launch', document: 'news/launch/index.html', strategy: 'isr' },
      { path: '/blog/hello', document: 'blog/hello/index.html', strategy: 'ssg' },
    ],
  } as unknown as BuildContext['deployManifest']

  const buildWith = (deployManifest: BuildContext['deployManifest']) =>
    vercel({ projectOutput: false }).build({
      root: '.',
      outDir: '.ruvyxa',
      deployManifest,
    } as BuildContext)

  const build = () => buildWith(manifest)

  const configFor = (
    artifacts: NonNullable<Awaited<ReturnType<typeof build>>['artifacts']>,
    name: string,
  ) =>
    JSON.parse(
      String(
        artifacts.find((artifact) =>
          artifact.path.endsWith(`/functions/${name}.prerender-config.json`),
        )?.contents ?? '{}',
      ),
    )

  it('mounts each ISR path as its own prerender function, linked to the one bundle', async () => {
    const output = await build()
    const artifacts = output.artifacts ?? []
    const aliases = artifacts.filter((artifact) => artifact.kind === 'function-alias')

    // The pattern itself is not a path anybody requests; the expansion is.
    assert.deepEqual(
      aliases.map((artifact) =>
        artifact.path.replace('deploy/vercel/.vercel/output/functions/', ''),
      ),
      ['isr-page.func', 'news/launch.func'],
    )
    for (const alias of aliases) {
      assert.equal(alias.aliasOf, 'deploy/vercel/.vercel/output/functions/__ruvyxa_handler.func')
    }
    // One bundle, aliased — never a second compiled function.
    assert.equal(artifacts.filter((artifact) => artifact.kind === 'function').length, 1)
  })

  it('gives each one the window the route asked for, and a bypass token', async () => {
    const artifacts = (await build()).artifacts ?? []
    const page = configFor(artifacts, 'isr-page')
    assert.equal(page.expiration, 120)
    assert.deepEqual(page.allowQuery, [])
    assert.match(String(page.bypassToken), /^[0-9a-f]{32}$/)

    // An expansion has no window of its own and inherits the pattern's.
    assert.equal(configFor(artifacts, 'news/launch').expiration, 30)

    // PPR streams its holes at request time; a prerender cache in front of that
    // is a different mechanism and is not claimed here.
    assert.equal(
      artifacts.some((artifact) => artifact.path.includes('ppr-page')),
      false,
    )
    // An SSG route is answered from a file and never reaches a function.
    assert.equal(
      artifacts.some((artifact) => artifact.path.includes('about')),
      false,
    )
  })

  it("gives an expansion its own route's window, whatever the manifest order is", async () => {
    // The catch-all is declared first on purpose. "The first pattern that fits"
    // is not an answer — the router resolves static before dynamic before
    // catch-all, so `/blog/hello` belongs to `/blog/[slug]` however the two
    // routes happen to be ordered on disk.
    const artifacts =
      (
        await buildWith({
          buildId: '0123456789abcdef',
          routes: [
            {
              id: 'app/[...all]/page',
              path: '/[...all]',
              kind: 'page',
              serve: 'function',
              strategy: 'isr',
              runtime: 'node',
              revalidate: 3600,
            },
            {
              id: 'app/blog/[slug]/page',
              path: '/blog/[slug]',
              kind: 'page',
              serve: 'function',
              strategy: 'isr',
              runtime: 'node',
              revalidate: 60,
            },
          ],
          prerendered: [
            { path: '/blog/hello', document: 'blog/hello/index.html', strategy: 'isr' },
            { path: '/legal', document: 'legal/index.html', strategy: 'isr' },
          ],
        } as unknown as BuildContext['deployManifest'])
      ).artifacts ?? []

    assert.equal(configFor(artifacts, 'blog/hello').expiration, 60)
    // The catch-all still lends its own window to the paths only it answers.
    assert.equal(configFor(artifacts, 'legal').expiration, 3600)
  })

  it('takes an expansion parent only from an ISR page, and defaults when none claims it', async () => {
    // A dynamic API route and a PPR page can both match a page expansion's
    // path, and neither can have produced it. A manifest carrying that overlap
    // is exactly what the narrowing defends against.
    const artifacts =
      (
        await buildWith({
          buildId: '0123456789abcdef',
          routes: [
            {
              id: 'app/[...proxy]/route',
              path: '/[...proxy]',
              kind: 'api',
              serve: 'function',
              strategy: 'ssr',
              runtime: 'node',
              revalidate: 900,
            },
            {
              id: 'app/[...marketing]/page',
              path: '/[...marketing]',
              kind: 'page',
              serve: 'function',
              strategy: 'ppr',
              runtime: 'node',
              revalidate: 3600,
            },
            {
              id: 'app/notes/[slug]/page',
              path: '/notes/[slug]',
              kind: 'page',
              serve: 'function',
              strategy: 'isr',
              runtime: 'node',
              revalidate: 45,
            },
          ],
          prerendered: [
            { path: '/notes/first', document: 'notes/first/index.html', strategy: 'isr' },
            { path: '/pricing', document: 'pricing/index.html', strategy: 'isr' },
          ],
        } as unknown as BuildContext['deployManifest'])
      ).artifacts ?? []

    assert.equal(configFor(artifacts, 'notes/first').expiration, 45)
    // No ISR page claims `/pricing`, so it takes the default window rather than
    // borrowing a longer one from a route that did not render it. Shortening
    // the cache is the safe direction; refusing to emit it is not.
    assert.equal(configFor(artifacts, 'pricing').expiration, 60)
  })

  it('purges the CDN when revalidatePath() forces a write', async () => {
    const artifacts = (await build()).artifacts ?? []
    const source = String(
      artifacts.find((artifact) => artifact.kind === 'function')?.handlerSource ?? '',
    )
    const token = configFor(artifacts, 'isr-page').bypassToken
    // Writing to the function's own store leaves the Prerender cache in front
    // of it serving the old document; this is the documented way to invalidate.
    assert.match(source, /'x-prerender-revalidate': BYPASS_TOKEN/)
    assert.match(source, new RegExp(`const BYPASS_TOKEN = "${token}"`))
    assert.match(source, /forced === true\) return revalidateOnVercel/)
  })

  it('derives the bypass token so two builds of one commit agree', async () => {
    const first = configFor((await build()).artifacts ?? [], 'isr-page').bypassToken
    const second = configFor((await build()).artifacts ?? [], 'isr-page').bypassToken
    assert.equal(first, second)

    process.env.RUVYXA_PREVIEW_SECRET = 'a-project-secret'
    try {
      const configured = configFor((await build()).artifacts ?? [], 'isr-page').bypassToken
      assert.notEqual(configured, first, 'the project secret decides the token when it is set')
      assert.match(String(configured), /^[0-9a-f]{32}$/)
    } finally {
      delete process.env.RUVYXA_PREVIEW_SECRET
    }
  })

  it('reads the origin to purge from the request being served, not from a shared slot', async () => {
    const artifacts = (await build()).artifacts ?? []
    const source = String(
      artifacts.find((artifact) => artifact.kind === 'function')?.handlerSource ?? '',
    )
    // One instance of a Vercel function answers more than one request at a
    // time, so a module-level `requestOrigin = url.origin` is overwritten by
    // whatever arrived next: the purge then went to the other request's domain
    // and left the page it was asked to drop cached on the domain a visitor was
    // reading. Nothing about the response shows it, and it only happens under
    // concurrency.
    assert.match(source, /new AsyncLocalStorage\(\)/)
    assert.match(source, /requestOrigin\.run\(url\.origin,/)
    assert.match(source, /const origin = requestOrigin\.getStore\(\)/)
    assert.doesNotMatch(source, /^\s*requestOrigin = /m)
  })
})
