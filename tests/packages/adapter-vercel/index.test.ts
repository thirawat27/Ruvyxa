import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { Readable } from 'node:stream'
import { copyFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { staticAssetPattern } from '../../../packages/@ruvyxa/core/src/utils.ts'
import { vercelAdapter } from '../../../packages/@ruvyxa/adapter-vercel/src/index.ts'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))

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
    assert.match(String(functionArtifact!.handlerSource), /loadRouteModule/)
    assert.doesNotMatch(String(functionArtifact!.handlerSource), /\.\/server\/app/)
    assert.match(String(functionArtifact!.handlerSource), /export default/)
    assert.match(String(functionArtifact!.handlerSource), /for await \(const chunk of req\)/)
    assert.match(String(functionArtifact!.handlerSource), /getSetCookie/)

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
    // Unset by default so Vercel's own region selection applies.
    assert.equal('regions' in config, false)
  })

  it('pins function regions when asked, and rejects malformed region lists', () => {
    const output = vercelAdapter({ regions: ['sin1'] }).build({ root: '.', outDir: '.ruvyxa' })
    const vcConfig = output.artifacts?.find((a) => a.path.endsWith('.vc-config.json'))
    const config = JSON.parse(vcConfig && 'contents' in vcConfig ? String(vcConfig.contents) : '{}')
    assert.deepEqual(config.regions, ['sin1'])

    assert.throws(() => vercelAdapter({ regions: [] }), /RUV2001/)
    assert.throws(() => vercelAdapter({ regions: [''] }), /RUV2001/)
  })

  it('forwards a streamed Node request body and repeated Set-Cookie headers', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-vercel-handler-'))
    try {
      const output = vercelAdapter({ projectOutput: false }).build({ root, outDir: '.ruvyxa' })
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
          const headers = new Headers()
          headers.append('set-cookie', 'first=1; Path=/')
          headers.append('set-cookie', 'second=2; Path=/')
          return new Response(await request.text(), { headers })
        } }
        export async function loadRouteModule() { return api }
        `,
      )
      await copyFile(
        path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs'),
        path.join(root, 'serverless-handler.mjs'),
      )

      const { default: handler } = await import(
        pathToFileURL(path.join(root, 'index.mjs')).href + `?t=${Date.now()}`
      )
      const request = Readable.from([Buffer.from('streamed-payload')])
      Object.assign(request, {
        url: '/api/echo',
        method: 'POST',
        headers: { host: 'localhost', 'content-type': 'text/plain' },
      })
      const headers = new Map()
      let body = ''
      const response = {
        statusCode: 0,
        setHeader(name, value) {
          headers.set(name, value)
        },
        end(value) {
          body = String(value)
        },
      }

      await handler(request, response)

      assert.equal(response.statusCode, 200)
      assert.equal(body, 'streamed-payload')
      assert.deepEqual(headers.get('set-cookie'), ['first=1; Path=/', 'second=2; Path=/'])

      const parsedRequest = Readable.from([])
      Object.assign(parsedRequest, {
        url: '/api/echo',
        method: 'POST',
        headers: { host: 'localhost', 'content-type': 'application/json' },
        body: { parsed: true },
      })
      body = ''
      await handler(parsedRequest, response)
      assert.equal(body, '{"parsed":true}')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})
