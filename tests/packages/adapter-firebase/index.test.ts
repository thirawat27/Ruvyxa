import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { mkdtemp, rm } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'

import { DEFAULT_SECURITY_HEADERS } from '../../../packages/@ruvyxa/core/dist/utils.js'
import { firebase } from '../../../packages/@ruvyxa/adapter-firebase/dist/index.js'
import {
  deployFunction,
  echoManifest,
  echoRouteModules,
  nodeRequest,
  nodeResponse,
  ECHO_BINARY_BODY,
  ECHO_COOKIES,
} from '../../deployed-function.ts'

/**
 * A stand-in for `firebase-functions/v2/https`, resolved from the function
 * directory the way the real dependency is on Cloud Functions.
 *
 * `onRequest` hands back the request handler with the deployment options
 * attached, so the test can both invoke the function and read the options the
 * adapter declared for it.
 */
const firebaseFunctionsStub = {
  'node_modules/firebase-functions/package.json': JSON.stringify({
    name: 'firebase-functions',
    version: '0.0.0-test',
    type: 'module',
    exports: { './v2/https': './v2/https.js' },
  }),
  'node_modules/firebase-functions/v2/https.js':
    'export function onRequest(options, handler) {\n' +
    '  const invoke = (request, response) => handler(request, response)\n' +
    '  invoke.deploymentOptions = options\n' +
    '  return invoke\n' +
    '}\n',
}

describe('firebase', () => {
  it('emits Hosting, Functions v2, cache, and rewrite artifacts', async () => {
    const adapter = firebase({ functionName: 'webApp', region: 'asia-east1' })
    const output = await adapter.build({ root: '.', outDir: '.ruvyxa' })

    assert.equal(output.platform, 'firebase')
    assert.equal(output.target, 'serverless')
    assert.deepEqual(adapter.supports, ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'])
    assert.deepEqual(
      output.artifacts?.map(({ kind, path, scope }) => ({ kind, path, scope })),
      [
        { kind: 'static-site', path: 'deploy/firebase/public', scope: undefined },
        { kind: 'function', path: 'deploy/firebase/functions', scope: undefined },
        {
          kind: 'file',
          path: 'deploy/firebase/functions/package.json',
          scope: undefined,
        },
        { kind: 'file', path: 'deploy/firebase/firebase.json', scope: undefined },
        { kind: 'file', path: 'deploy/firebase/README.md', scope: undefined },
        { kind: 'file', path: 'firebase.json', scope: 'project' },
      ],
    )

    const firebaseJson = output.artifacts?.find((artifact) => artifact.path === 'firebase.json')
    assert.equal(firebaseJson?.skipIfExists, true)
    const config = JSON.parse(
      firebaseJson && 'contents' in firebaseJson ? String(firebaseJson.contents) : '{}',
    )
    assert.equal(config.hosting.public, '.ruvyxa/deploy/firebase/public')
    assert.deepEqual(config.hosting.rewrites, [
      {
        source: '**',
        function: { functionId: 'webApp', region: 'asia-east1', pinTag: true },
      },
    ])
    assert.equal(config.functions[0].runtime, 'nodejs24')
    // First, and on everything Hosting answers: a pre-rendered document and
    // every public file are served from `public` without the rewrite reaching
    // the function, which is the only other place these are set.
    assert.equal(config.hosting.headers[0].source, '**')
    assert.deepEqual(
      Object.fromEntries(
        config.hosting.headers[0].headers.map(
          (entry: { key: string; value: string }) => [entry.key, entry.value] as const,
        ),
      ),
      DEFAULT_SECURITY_HEADERS,
    )
    assert.equal(config.hosting.headers[1].headers[0].value, 'public, max-age=31536000, immutable')

    const handler = output.artifacts?.find((artifact) => artifact.kind === 'function')
    const source = String(handler && 'handlerSource' in handler ? handler.handlerSource : '')

    // The ISR cache directory must not be a fixed name under `os.tmpdir()`.
    //
    // It was `os.tmpdir()/ruvyxa-isr-cache` — the same directory for every
    // Ruvyxa deployment on the host and for every previous build of this one,
    // read *before* the bundled prerender output, so whatever was there won.
    // A redeploy served the previous build's documents, whose `<script src>`
    // names client chunks the new build no longer publishes. On Linux the
    // parent is mode 1777, so a planted file or symlink at a known route path
    // was served as that page and written through on the next refresh.
    assert.doesNotMatch(
      source,
      /tmpdir\(\),\s*'ruvyxa-isr-cache'\s*\)/,
      'the cache directory must carry a per-deployment identity, not a fixed name',
    )
    assert.match(
      source,
      /createHash\('sha256'\)[\s\S]{0,200}import\.meta\.dirname/,
      'the identity hashes the build id together with the bundle directory',
    )
    assert.match(
      source,
      /mkdirSync\(isrCacheDir, \{ recursive: true, mode: 0o700 \}\)/,
      'created owner-only, because the parent is world-writable on Linux',
    )
    assert.match(source, /firebase-functions\/v2\/https/)
    assert.match(source, /export const webApp = onRequest/)
    assert.match(source, /os\.tmpdir\(\)/)
    assert.match(source, /prerenderRelativePath/)
    assert.doesNotMatch(source, /ISR cache write failures/)
    assert.match(source, /getSetCookie/)
    // A second-generation function runs on Cloud Run, which forwards bytes as
    // they are written; collecting the whole response first only cost memory
    // and time to first byte.
    assert.match(source, /pipeline\(Readable\.fromWeb\(response\.body\), res\)/)
    assert.doesNotMatch(source, /res\.send\(Buffer\.from/)

    const packageArtifact = output.artifacts?.find((artifact) =>
      artifact.path.endsWith('functions/package.json'),
    )
    const packageJson = JSON.parse(
      packageArtifact && 'contents' in packageArtifact ? String(packageArtifact.contents) : '{}',
    )
    assert.equal(packageJson.engines.node, '24')
    assert.equal(packageJson.dependencies['firebase-functions'], '^7.3.0')
  })

  /**
   * `firebase deploy` resolves these paths against the directory holding
   * firebase.json, so the project-root copy has to name the configured
   * `outDir` and the deploy-directory copy has to name its own siblings.
   * Both were hard-coded to `.ruvyxa`, which pointed a project that configures
   * `outDir` at a directory that does not exist.
   */
  it('writes config paths for the directory each firebase.json sits in', async () => {
    const output = await firebase().build({ root: '/srv/app', outDir: '/srv/app/build' })

    const configFor = (path: string) => {
      const artifact = output.artifacts?.find((item) => item.path === path)
      return JSON.parse(artifact && 'contents' in artifact ? String(artifact.contents) : '{}')
    }

    const project = configFor('firebase.json')
    assert.equal(project.hosting.public, 'build/deploy/firebase/public')
    assert.equal(project.functions[0].source, 'build/deploy/firebase/functions')

    const deployLocal = configFor('deploy/firebase/firebase.json')
    assert.equal(deployLocal.hosting.public, 'public')
    assert.equal(deployLocal.functions[0].source, 'functions')
  })

  it('pins the functions runtime and the package engines to one major', async () => {
    const output = await firebase({ runtime: 'nodejs22' }).build({ root: '.', outDir: '.ruvyxa' })
    const contentsOf = (suffix: string) => {
      const artifact = output.artifacts?.find((item) => item.path.endsWith(suffix))
      return JSON.parse(artifact && 'contents' in artifact ? String(artifact.contents) : '{}')
    }
    assert.equal(contentsOf('firebase.json').functions[0].runtime, 'nodejs22')
    assert.equal(contentsOf('functions/package.json').engines.node, '22')
    assert.throws(() => firebase({ runtime: 'nodejs18' as unknown as 'nodejs22' }), /runtime/)
  })

  it('validates function and region identifiers', () => {
    assert.doesNotThrow(() => firebase({ region: 'europe-west10' }))
    assert.throws(() => firebase({ functionName: 'bad-name' }), /functionName/)
    assert.throws(() => firebase({ region: 'moon-1' }), /region/)
  })

  // Cloud Functions v2 hands the function an Express-style request whose body
  // may already be parsed, and takes a Node response back. Every step of that
  // translation is hand-written here, and none of it was ever executed.
  it('serves a request through the deployed function bundle', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-firebase-handler-'))
    try {
      const output = await firebase({ projectConfig: false }).build({ root, outDir: '.ruvyxa' })
      const artifact = output.artifacts?.find((item) => item.kind === 'function')
      assert.ok(artifact && 'handlerSource' in artifact && artifact.handlerSource)

      const bundle = await deployFunction(root, {
        handlerSource: String(artifact.handlerSource),
        manifest: echoManifest(),
        routeModules: echoRouteModules(),
        extraFiles: firebaseFunctionsStub,
      })

      // The export name is the configured function name, which is what
      // firebase.json's rewrite points at. A mismatch deploys a function
      // nothing routes to.
      const invoke = bundle.ruvyxaServer as ((
        request: unknown,
        response: unknown,
      ) => Promise<void>) & {
        deploymentOptions: { region?: string; timeoutSeconds?: number }
      }
      assert.equal(typeof invoke, 'function')
      assert.equal(invoke.deploymentOptions.region, 'us-central1')

      // The raw bytes a platform exposes beside its parsed body win, so a body
      // the platform parsed is never re-serialized when the original survives.
      const raw = nodeResponse()
      await invoke(
        nodeRequest({
          url: '/api/echo',
          method: 'POST',
          headers: { host: 'example.test', 'content-type': 'text/plain' },
          rawBody: Buffer.from('streamed-payload'),
        }),
        raw.response,
      )
      assert.equal(raw.response.statusCode, 200)
      assert.equal(raw.body().toString(), 'streamed-payload')
      // Repeated Set-Cookie must reach `setHeader` as an array. Folding them
      // into one comma-joined string is the classic way to lose the second
      // cookie, and it satisfies every text assertion.
      assert.deepEqual(raw.headers.get('set-cookie'), ECHO_COOKIES)

      // With no rawBody, a JSON body the platform parsed is serialized back.
      const parsed = nodeResponse()
      await invoke(
        nodeRequest({
          url: '/api/echo',
          method: 'POST',
          headers: { host: 'example.test', 'content-type': 'application/json' },
          body: { parsed: true },
        }),
        parsed.response,
      )
      assert.equal(parsed.body().toString(), '{"parsed":true}')

      // A form body is re-encoded as a form, not as JSON.
      const form = nodeResponse()
      await invoke(
        nodeRequest({
          url: '/api/echo',
          method: 'POST',
          headers: {
            host: 'example.test',
            'content-type': 'application/x-www-form-urlencoded',
          },
          body: { name: 'ada', role: 'admin' },
        }),
        form.response,
      )
      assert.equal(form.body().toString(), 'name=ada&role=admin')

      // Bytes that are not valid UTF-8 survive: a wrapper that round-trips the
      // body through a string replaces them and the check still reads "200".
      const binary = nodeResponse()
      await invoke(
        nodeRequest({
          url: '/api/echo',
          method: 'POST',
          headers: { host: 'example.test', 'x-binary': '1' },
        }),
        binary.response,
      )
      assert.deepEqual(binary.body(), ECHO_BINARY_BODY)

      const missing = nodeResponse()
      await invoke(
        nodeRequest({
          url: '/api/absent',
          method: 'POST',
          headers: { host: 'example.test' },
        }),
        missing.response,
      )
      assert.equal(missing.response.statusCode, 404)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})
