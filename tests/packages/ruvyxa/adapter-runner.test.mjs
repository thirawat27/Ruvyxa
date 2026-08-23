import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const adapterRunner = path.join(workspaceRoot, 'packages/ruvyxa/runtime/adapter-runner.mjs')

describe('adapter runner', () => {
  it('replays the official adapter capability matrix through inspection', async () => {
    const contract = JSON.parse(
      await readFile(path.join(workspaceRoot, 'tests/fixtures/adapter-contract.json'), 'utf8'),
    )
    assert.equal(contract.contract, 'ruvyxa.adapter')
    assert.equal(contract.schemaVersion, 1)

    for (const expected of contract.adapters) {
      const root = await mkdtemp(path.join(os.tmpdir(), `ruvyxa-${expected.name}-inspect-`))
      const outputDir = path.join(root, '.ruvyxa-staging')
      try {
        await mkdir(outputDir, { recursive: true })
        const result = await runRunner(root, outputDir, expected.name, {
          RUVYXA_ADAPTER_RUNNER_MODE: 'inspect',
        })
        assert.deepEqual(result.result, expected, expected.name)
      } finally {
        await rm(root, { recursive: true, force: true })
      }
    }
  })

  it('rejects every unsupported official adapter capability with the shared diagnostic', async () => {
    const contract = JSON.parse(
      await readFile(path.join(workspaceRoot, 'tests/fixtures/adapter-contract.json'), 'utf8'),
    )
    const capabilities = ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api']

    for (const adapter of contract.adapters) {
      for (const capability of capabilities.filter((item) => !adapter.supports.includes(item))) {
        const root = await mkdtemp(path.join(os.tmpdir(), `ruvyxa-${adapter.name}-${capability}-`))
        const outputDir = path.join(root, '.ruvyxa-staging')
        try {
          await mkdir(outputDir, { recursive: true })
          const route =
            capability === 'api'
              ? { id: 'app/api/fixture/route', kind: 'api', path: '/api/fixture' }
              : {
                  id: 'app/fixture/page',
                  kind: 'page',
                  path: '/fixture',
                  render: { strategy: capability },
                }
          await writeFile(
            path.join(outputDir, 'manifest.json'),
            JSON.stringify({ routes: [route] }),
          )

          const result = await runRunnerResult(root, outputDir, adapter.name)
          assert.equal(result.exitCode, 1, `${adapter.name}:${capability}`)
          assert.match(
            result.parsed.message,
            new RegExp(contract.unsupportedDiagnostics.route),
            `${adapter.name}:${capability}`,
          )
        } finally {
          await rm(root, { recursive: true, force: true })
        }
      }
    }
  })

  it('materializes root-contained artifacts for every official adapter', async () => {
    const contract = JSON.parse(
      await readFile(path.join(workspaceRoot, 'tests/fixtures/adapter-contract.json'), 'utf8'),
    )

    for (const adapter of contract.adapters) {
      const root = await mkdtemp(path.join(os.tmpdir(), `ruvyxa-${adapter.name}-artifacts-`))
      const outputDir = path.join(root, '.ruvyxa-staging')
      try {
        await mkdir(path.join(outputDir, 'assets'), { recursive: true })
        await mkdir(path.join(outputDir, 'client'), { recursive: true })
        await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
        await writeFile(path.join(outputDir, 'assets', 'logo.svg'), '<svg/>')
        await writeFile(path.join(outputDir, 'client', 'app.js'), 'export {}')
        await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
        await writeFile(path.join(outputDir, 'manifest.json'), JSON.stringify({ routes: [] }))

        const result = await runRunner(root, outputDir, adapter.name)
        assert.ok(result.result.length > 0, `${adapter.name} emitted no artifact descriptors`)
        for (const artifact of result.result) {
          const base = artifact.scope === 'project' ? root : outputDir
          const resolved = path.resolve(base, artifact.path)
          assert.ok(
            resolved === base || resolved.startsWith(base + path.sep),
            `${adapter.name}:${artifact.path} escaped its scope`,
          )
          await stat(resolved)
          if (artifact.kind === 'function') {
            await readFile(path.join(resolved, 'index.mjs'), 'utf8')
            await readFile(path.join(resolved, 'serverless-handler.mjs'), 'utf8')
            await readFile(path.join(resolved, 'manifest.mjs'), 'utf8')
          }
        }
      } finally {
        await rm(root, { recursive: true, force: true })
      }
    }
  })

  it('inspects adapter capabilities without materializing declared artifacts', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: {
          name: 'fixture', target: 'serverless', supports: ['ssg', 'api'],
          build() { return {
            name: 'fixture', target: 'serverless', runtime: 'node', platform: 'aws',
            artifacts: [{ kind: 'file', path: 'deploy/health.txt', contents: 'ready' }]
          } }
        } }`,
      )

      const result = await runRunner(root, outputDir, undefined, {
        RUVYXA_ADAPTER_RUNNER_MODE: 'inspect',
      })

      assert.deepEqual(result.result, {
        name: 'fixture',
        target: 'serverless',
        runtime: 'node',
        platform: 'aws',
        supports: ['ssg', 'api'],
      })
      await assert.rejects(readFile(path.join(outputDir, 'deploy/health.txt')), /ENOENT/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('materializes static deployment artifacts from a static-only build', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'assets'), { recursive: true })
      await mkdir(path.join(outputDir, 'client'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender', 'about'), { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'static-site', path: 'deploy/site' },
          { kind: 'file', path: 'deploy/site/_headers', contents: 'X-Frame-Options: DENY\\n' }
        ] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [
            { kind: 'page', path: '/', render: { strategy: 'ssg' } },
            { kind: 'page', path: '/about', render: { strategy: 'csr' } },
          ],
        }),
      )
      await writeFile(path.join(outputDir, 'assets', 'app.css'), 'body {}')
      await writeFile(path.join(outputDir, 'client', 'app.js'), 'export {}')
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      await writeFile(
        path.join(outputDir, 'prerender', 'about', 'index.html'),
        '<main>about</main>',
      )

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [
        { kind: 'static-site', path: 'deploy/site' },
        { kind: 'file', path: 'deploy/site/_headers' },
      ])
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/index.html'), 'utf8'),
        '<main>home</main>',
      )
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/about/index.html'), 'utf8'),
        '<main>about</main>',
      )
      assert.equal(await readFile(path.join(outputDir, 'deploy/site/app.css'), 'utf8'), 'body {}')
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/__ruvyxa/client/app.js'), 'utf8'),
        'export {}',
      )
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/_headers'), 'utf8'),
        'X-Frame-Options: DENY\n',
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // RUV2213 used to refuse *every* non-pre-rendered server-components route,
  // for every adapter, because the generated route module rendered through the
  // ordinary SSR entry — the page would have deployed with no payload in the
  // document and nothing for its browser bundle to hydrate. The runner compiles
  // the `react-server` graph and the SSR registry into the function now, so the
  // remaining constraint is the one an adapter genuinely has: a target with no
  // server cannot run a Flight pass at request time.
  it('refuses a dynamic server-components route on a target with no server', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: {
          name: 'static', target: 'static', supports: ['ssg', 'csr'],
          build() { return { artifacts: [] } }
        } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [
            { kind: 'page', path: '/live', render: { strategy: 'ssr', serverComponents: true } },
            { kind: 'page', path: '/docs', render: { strategy: 'ssg', serverComponents: true } },
          ],
        }),
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /RUV2202 adapter static/)
      assert.match(result.parsed.message, /\/live \(ssr\)/)
      // The pre-rendered one is not named: its payload is inside the file the
      // adapter publishes, so a static host serves it correctly.
      assert.doesNotMatch(result.parsed.message, /\/docs/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // A deployment is a production build whichever way the host starts it, and
  // its browser half already says so unconditionally — the Rust bundler folds
  // `NODE_ENV` to production and cannot be told otherwise. The server half read
  // the ambient value, and nothing in an emitted deployment sets one, so the
  // documented `node server/index.mjs` ran React's *development* build against
  // a production browser bundle. Every HTTP-level check passed: 200, correct
  // markup, a payload in the document. The browser threw `Failed to read a RSC
  // payload created by a development version of React` and rendered nothing,
  // and the development payload published the build machine's absolute source
  // paths to every visitor.
  it('pins NODE_ENV to production in the function a deployment emits', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    const functionDir = path.join(outputDir, 'deploy', 'function')
    try {
      await installFakeReact(root)
      await mkdir(path.join(root, 'app', 'mode'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
      await writeFile(
        path.join(root, 'app', 'layout.tsx'),
        `export default function Layout({ children }) { return <body>{children}</body> }`,
      )
      await writeFile(
        path.join(root, 'app', 'mode', 'page.tsx'),
        `export default function Page() { return <main>{process.env.NODE_ENV ?? 'unset'}</main> }`,
      )

      const manifest = {
        routes: [
          {
            id: 'app/mode/page',
            kind: 'page',
            path: '/mode',
            file: 'app/mode/page.tsx',
            layoutChain: ['app/layout'],
            render: { strategy: 'ssr' },
          },
        ],
      }
      await writeFile(path.join(outputDir, 'manifest.json'), JSON.stringify(manifest))

      const handlerSource = `import { createHandler } from './serverless-handler.mjs'
import { loadRouteModule } from './route-modules.mjs'
const routes = ${JSON.stringify(manifest.routes)}
export default createHandler({ routes, importPage: loadRouteModule, importApi: loadRouteModule })
`
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return {
          artifacts: [{ kind: 'function', path: 'deploy/function', handlerSource: ${JSON.stringify(handlerSource)} }]
        } } } }`,
      )

      await runRunner(root, outputDir)

      const registry = await readFile(path.join(functionDir, 'route-modules.mjs'), 'utf8')
      const pin = registry.indexOf('globalThis.process.env.NODE_ENV = "production"')
      assert.notEqual(pin, -1, 'the emitted registry does not pin NODE_ENV')
      // Ahead of the first module factory, because React reads the value while
      // its own factory runs: a pin after it is a pin that never happened. A
      // statement in the *entry* cannot do this either — ESM evaluates a
      // module's imports before any statement of the importer.
      assert.ok(
        pin < registry.indexOf('const __m'),
        'the pin runs after a module factory has already read NODE_ENV',
      )

      // The claim is behavioural, so it is checked by running the artifact the
      // documented way — a process with no `NODE_ENV` exported — rather than by
      // reading the source it was compiled from.
      assert.equal(
        await renderThroughFunction(functionDir, '/mode'),
        '<!doctype html><body><main>production</main></body>',
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('deploys a dynamic server-components route on an adapter that runs one', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: {
          name: 'node', target: 'node', supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
          build() { return { artifacts: [] } }
        } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [
            { kind: 'page', path: '/live', render: { strategy: 'ssr', serverComponents: true } },
          ],
        }),
      )

      // No artifacts are declared, so nothing is compiled here; what is under
      // test is that the capability check lets the route through at all.
      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [])
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('deploys a pre-rendered server-components route without complaint', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { name: 'static', supports: ['ssg', 'csr'], build() { return { artifacts: [] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [
            { kind: 'page', path: '/docs', render: { strategy: 'ssg', serverComponents: true } },
          ],
        }),
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 0, result.parsed?.message ?? '')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('rejects routes the adapter declares it does not support', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { name: 'static', supports: ['ssg', 'csr'], build() { return { artifacts: [{ kind: 'static-site', path: 'deploy/site' }] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [{ kind: 'api', path: '/api/health', render: { strategy: 'ssr' } }],
        }),
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /RUV2202 adapter static supports ssg, csr/)
      assert.match(result.parsed.message, /\/api\/health \(api\)/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // Regression: the static-only rule used to live in `materializeStaticSite`
  // and applied to every `static-site` artifact, so the vercel/netlify/
  // cloudflare adapters -- which emit that artifact for the static layer beside
  // a serverless function -- could never build an app with an API or SSR route.
  it('allows a hybrid adapter to emit a static-site artifact alongside SSR and API routes', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { name: 'vercel', supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'], build() { return { artifacts: [{ kind: 'static-site', path: 'deploy/vercel/static' }] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [
            { kind: 'page', path: '/', render: { strategy: 'ssg' } },
            { kind: 'page', path: '/blog/[slug]', render: { strategy: 'ssr' } },
            { kind: 'page', path: '/isr-page', render: { strategy: 'isr' } },
            { kind: 'api', path: '/api/health' },
          ],
        }),
      )

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [{ kind: 'static-site', path: 'deploy/vercel/static' }])
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/vercel/static/index.html'), 'utf8'),
        '<main>home</main>',
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // A host that serves the publish directory before invoking the function
  // (Vercel `handle: filesystem`, Netlify `preferStatic`) pins a published ISR
  // page to its build-time snapshot forever, so the adapter can hold those
  // pages back. Build telemetry must not become a public URL either.
  it('keeps excluded strategies and build telemetry out of the publish directory', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'assets'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender', 'isr-page'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender', 'blog', 'hello'), { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'static-site', path: 'deploy/site', excludeStrategies: ['isr', 'ppr'] }
        ] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({
          routes: [
            { kind: 'page', path: '/', render: { strategy: 'ssg' } },
            { kind: 'page', path: '/isr-page', render: { strategy: 'isr' } },
            { kind: 'page', path: '/blog/[slug]', render: { strategy: 'isr' } },
          ],
        }),
      )
      await writeFile(path.join(outputDir, 'assets', 'logo.png'), 'png-bytes')
      await writeFile(path.join(outputDir, 'assets', '.ruvyxa-images.json'), '{"entries":[]}')
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      await writeFile(
        path.join(outputDir, 'prerender', 'isr-page', 'index.html'),
        '<main>isr</main>',
      )
      await writeFile(
        path.join(outputDir, 'prerender', 'blog', 'hello', 'index.html'),
        '<main>hello</main>',
      )
      await writeFile(
        path.join(outputDir, 'prerender', 'manifest.json'),
        JSON.stringify({
          routes: [
            { path: '/', strategy: 'ssg', htmlFile: 'index.html' },
            { path: '/isr-page', strategy: 'isr', htmlFile: 'index.html' },
            { path: '/blog/hello', strategy: 'isr', htmlFile: 'index.html' },
          ],
        }),
      )

      await runRunner(root, outputDir)

      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/index.html'), 'utf8'),
        '<main>home</main>',
      )
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/site/logo.png'), 'utf8'),
        'png-bytes',
      )
      for (const withheld of [
        'deploy/site/isr-page/index.html',
        'deploy/site/blog/hello/index.html',
        'deploy/site/.ruvyxa-images.json',
        'deploy/site/manifest.json',
      ]) {
        await assert.rejects(readFile(path.join(outputDir, withheld), 'utf8'), withheld)
      }
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('materializes executable page and API modules instead of raw TypeScript sources', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    const functionDir = path.join(outputDir, 'deploy', 'function')
    try {
      await installFakeReact(root)
      await mkdir(path.join(root, 'app', 'hello', '[name]'), { recursive: true })
      await mkdir(path.join(root, 'app', 'api', 'echo'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })

      await writeFile(
        path.join(root, 'app', 'layout.tsx'),
        `export default function Layout({ children }) { return <body>{children}</body> }`,
      )
      await writeFile(
        path.join(root, 'app', 'hello', '[name]', 'page.tsx'),
        `export default function Page({ params }) { return <main>Hello {params.name}</main> }`,
      )
      await writeFile(
        path.join(root, 'app', 'api', 'echo', 'route.ts'),
        `export async function POST({ request, params }) {
          return Response.json({ body: await request.text(), params })
        }`,
      )

      // Backslashes deliberately model a manifest produced on Windows. Route
      // resolution must stay portable instead of treating them as filename
      // characters on POSIX hosts.
      const manifest = {
        routes: [
          {
            id: 'app/hello/[name]/page',
            kind: 'page',
            path: '/hello/[name]',
            file: 'app\\hello\\[name]\\page.tsx',
            layoutChain: ['app/layout'],
            render: { strategy: 'ssr' },
          },
          {
            id: 'app/api/echo/route',
            kind: 'api',
            path: '/api/echo',
            file: 'app\\api\\echo\\route.ts',
            layoutChain: ['app/layout'],
            render: { strategy: 'ssr' },
          },
        ],
      }
      await writeFile(path.join(outputDir, 'manifest.json'), JSON.stringify(manifest))

      const handlerSource = `import { createHandler } from './serverless-handler.mjs'
import { loadRouteModule } from './route-modules.mjs'
const routes = ${JSON.stringify(manifest.routes)}
const handler = createHandler({ routes, importPage: loadRouteModule, importApi: loadRouteModule })
export default handler
`
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return {
          target: 'edge',
          artifacts: [{ kind: 'function', path: 'deploy/function', handlerSource: ${JSON.stringify(handlerSource)} }]
        } } } }`,
      )

      await runRunner(root, outputDir)

      const { default: handler } = await import(
        `${new URL(`file://${functionDir.replaceAll('\\', '/')}/index.mjs`).href}?t=${Date.now()}`
      )
      const pageResponse = await handler(new Request('http://localhost/hello/Ada'))
      assert.equal(pageResponse.status, 200)
      assert.equal(await pageResponse.text(), '<!doctype html><body><main>Hello Ada</main></body>')

      const apiResponse = await handler(
        new Request('http://localhost/api/echo', { method: 'POST', body: 'payload' }),
      )
      assert.equal(apiResponse.status, 200)
      assert.deepEqual(await apiResponse.json(), { body: 'payload', params: {} })

      const registry = await readFile(path.join(functionDir, 'route-modules.mjs'), 'utf8')
      assert.match(registry, /loadRouteModule/)
      assert.match(registry, /renderPage0/)
      assert.doesNotMatch(registry, /import\(`\.\/server\/app\//)

      // This one is an `edge` artifact, which has no `process` at runtime at
      // all — the stand-in each module gets is the only `NODE_ENV` its React
      // will ever read, and it was compiled from whatever the *build* process
      // happened to export, which is nothing. So a Worker ran React's
      // development build with no way for the deployment to say otherwise.
      assert.match(registry, /NODE_ENV: "production"/)
      assert.doesNotMatch(registry, /NODE_ENV: "development"/)

      // The manifest also ships as a module. A platform that re-bundles the
      // function (Netlify's esbuild step) keeps only what it can resolve
      // statically, and a sibling manifest.json read at runtime crashed the
      // deployed function with ENOENT /var/task/manifest.json.
      const { default: bundledManifest } = await import(
        `${new URL(`file://${functionDir.replaceAll('\\', '/')}/manifest.mjs`).href}?t=${Date.now()}`
      )
      assert.deepEqual(bundledManifest.routes, manifest.routes)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  /// The reported serverless incident, at the layer where it happened.
  ///
  /// Adapters bundle route dependencies into the deployed function
  /// (`bundlePackages: true`). An SDK that reads its own version through
  /// `require('../package.json')` — the shape `gaxios` uses, and through it
  /// `google-auth-library` and `@google/genai` — sent that JSON file to the
  /// JavaScript transform and failed every adapter build that touched it.
  ///
  /// There is one `bundlePackages: true` call site, so this covers every
  /// platform target rather than the one that reported the failure.
  it('bundles an API route SDK that reads its own package.json into the function', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    const functionDir = path.join(outputDir, 'deploy', 'function')
    try {
      await installFakeReact(root)
      await mkdir(path.join(root, 'app', 'api', 'version'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })

      const sdkDir = path.join(root, 'node_modules', 'fake-sdk', 'build', 'cjs', 'src')
      await mkdir(sdkDir, { recursive: true })
      await writeFile(
        path.join(root, 'node_modules', 'fake-sdk', 'package.json'),
        JSON.stringify({ name: 'fake-sdk', version: '4.2.1', main: 'build/cjs/src/index.cjs' }),
      )
      await writeFile(
        path.join(sdkDir, 'index.cjs'),
        "const pkg = require('../../../package.json')\n" +
          'module.exports = { userAgent: `fake-sdk/${pkg.version}` }\n',
      )

      await writeFile(
        path.join(root, 'app', 'layout.tsx'),
        `export default function Layout({ children }) { return <body>{children}</body> }`,
      )
      await writeFile(
        path.join(root, 'app', 'api', 'version', 'route.ts'),
        `import sdk from 'fake-sdk'
        export async function GET() { return Response.json({ agent: sdk.userAgent }) }`,
      )

      const manifest = {
        routes: [
          {
            id: 'app/api/version/route',
            kind: 'api',
            path: '/api/version',
            file: 'app/api/version/route.ts',
            layoutChain: ['app/layout'],
            render: { strategy: 'ssr' },
          },
        ],
      }
      await writeFile(path.join(outputDir, 'manifest.json'), JSON.stringify(manifest))

      const handlerSource = `import { createHandler } from './serverless-handler.mjs'
import { loadRouteModule } from './route-modules.mjs'
const routes = ${JSON.stringify(manifest.routes)}
const handler = createHandler({ routes, importPage: loadRouteModule, importApi: loadRouteModule })
export default handler
`
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return {
          target: 'node',
          artifacts: [{ kind: 'function', path: 'deploy/function', handlerSource: ${JSON.stringify(handlerSource)} }]
        } } } }`,
      )

      await runRunner(root, outputDir)

      const { default: handler } = await import(
        `${new URL(`file://${functionDir.replaceAll('\\', '/')}/index.mjs`).href}?t=${Date.now()}`
      )
      const response = await handler(new Request('http://localhost/api/version'))
      assert.equal(response.status, 200)
      assert.deepEqual(await response.json(), { agent: 'fake-sdk/4.2.1' })

      // The JSON travels as data inside the function, not as a sibling file the
      // platform would have to ship separately.
      const registry = await readFile(path.join(functionDir, 'route-modules.mjs'), 'utf8')
      assert.match(registry, /JSON\.parse\(/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('materializes allowlisted project-scope artifacts at the project root', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'assets'), { recursive: true })
      await mkdir(path.join(outputDir, 'client'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'static-site', path: '.vercel/output/static', scope: 'project' },
          { kind: 'file', path: '.vercel/output/config.json', scope: 'project', contents: '{"version":3}' },
          { kind: 'file', path: 'netlify.toml', scope: 'project', skipIfExists: true, contents: 'generated' },
          { kind: 'file', path: 'wrangler.jsonc', scope: 'project', skipIfExists: true, contents: '{"name":"app"}' },
          { kind: 'file', path: 'railway.json', scope: 'project', skipIfExists: true, contents: '{"build":{}}' },
          { kind: 'file', path: 'render.yaml', scope: 'project', skipIfExists: true, contents: 'services: []' },
          { kind: 'file', path: 'firebase.json', scope: 'project', skipIfExists: true, contents: '{"hosting":{}}' },
          { kind: 'file', path: '.amplify-hosting/deploy-manifest.json', scope: 'project', contents: '{"version":1}' }
        ] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({ routes: [{ kind: 'page', path: '/', render: { strategy: 'ssg' } }] }),
      )
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      // Stale output from an earlier build must be replaced, and a
      // user-authored netlify.toml must never be overwritten.
      await mkdir(path.join(root, '.vercel/output/static'), { recursive: true })
      await writeFile(path.join(root, '.vercel/output/static/stale.js'), 'stale')
      await writeFile(path.join(root, 'netlify.toml'), 'user-authored')

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [
        { kind: 'static-site', path: '.vercel/output/static', scope: 'project' },
        { kind: 'file', path: '.vercel/output/config.json', scope: 'project' },
        { kind: 'file', path: 'netlify.toml', scope: 'project', skipped: true },
        { kind: 'file', path: 'wrangler.jsonc', scope: 'project' },
        { kind: 'file', path: 'railway.json', scope: 'project' },
        { kind: 'file', path: 'render.yaml', scope: 'project' },
        { kind: 'file', path: 'firebase.json', scope: 'project' },
        {
          kind: 'file',
          path: '.amplify-hosting/deploy-manifest.json',
          scope: 'project',
        },
      ])
      assert.equal(
        await readFile(path.join(root, '.vercel/output/static/index.html'), 'utf8'),
        '<main>home</main>',
      )
      assert.equal(
        await readFile(path.join(root, '.vercel/output/config.json'), 'utf8'),
        '{"version":3}',
      )
      assert.equal(await readFile(path.join(root, 'netlify.toml'), 'utf8'), 'user-authored')
      assert.equal(await readFile(path.join(root, 'wrangler.jsonc'), 'utf8'), '{"name":"app"}')
      assert.equal(await readFile(path.join(root, 'railway.json'), 'utf8'), '{"build":{}}')
      assert.equal(await readFile(path.join(root, 'render.yaml'), 'utf8'), 'services: []')
      assert.equal(await readFile(path.join(root, 'firebase.json'), 'utf8'), '{"hosting":{}}')
      assert.equal(
        await readFile(path.join(root, '.amplify-hosting/deploy-manifest.json'), 'utf8'),
        '{"version":1}',
      )
      await assert.rejects(readFile(path.join(root, '.vercel/output/static/stale.js')))
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // The manifests, the source directories, and the project's own config are
  // what a deploy adapter must not overwrite. Everything else at the project
  // root is a platform's discovery location and belongs to the adapter.
  for (const [name, artifactPath] of [
    ['a package manifest', 'package.json'],
    ['the application source directory', 'app/page.tsx'],
    ["the project's own config", 'ruvyxa.config.ts'],
    ['a lockfile', 'pnpm-lock.yaml'],
    ['the build output directory', '.ruvyxa/manifest.json'],
    ['a path escaping the project root', '../outside.json'],
  ]) {
    it(`refuses a project-scope artifact over ${name}`, async () => {
      const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
      const outputDir = path.join(root, '.ruvyxa-staging')
      try {
        await mkdir(outputDir, { recursive: true })
        await writeFile(
          path.join(root, 'ruvyxa.config.mjs'),
          `export default { adapter: { build() { return { artifacts: [
            { kind: 'file', path: ${JSON.stringify(artifactPath)}, scope: 'project', contents: '{}' }
          ] } } } }`,
        )

        const result = await runRunnerResult(root, outputDir)

        assert.equal(result.exitCode, 1)
        assert.match(
          result.parsed.message,
          /would overwrite project source|escapes the project root/,
          result.parsed.message,
        )
        // The refusal has to happen before the write, not be reported after it.
        assert.equal(
          existsSync(path.resolve(root, artifactPath)),
          false,
          `${artifactPath} was written despite being refused`,
        )
      } finally {
        await rm(root, { recursive: true, force: true })
      }
    })
  }

  // The allowlist this replaced named eleven paths, one per official adapter,
  // so a community adapter could not write the file its own platform discovers.
  it('lets an adapter write the project file its platform discovers', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: {
          name: 'flyio', target: 'node', platform: 'flyio',
          build() { return { name: 'flyio', target: 'node', platform: 'flyio', artifacts: [
            { kind: 'file', path: 'fly.toml', scope: 'project', contents: 'app = "demo"\\n' },
            { kind: 'file', path: 'Dockerfile', scope: 'project', contents: 'FROM node:24\\n' },
            // Firebase App Hosting's real file name, and the reason the
            // protected entries match whole path segments: a prefix test reads
            // this as living inside \`app\` and refuses it.
            { kind: 'file', path: 'apphosting.yaml', scope: 'project', contents: 'runConfig: {}\\n' }
          ] } }
        } }`,
      )

      const result = await runRunner(root, outputDir)

      assert.deepEqual(
        result.result,
        [
          { kind: 'file', path: 'fly.toml', scope: 'project' },
          { kind: 'file', path: 'Dockerfile', scope: 'project' },
          { kind: 'file', path: 'apphosting.yaml', scope: 'project' },
        ],
        'a platform this repository has never heard of still gets its config file',
      )
      assert.equal(await readFile(path.join(root, 'fly.toml'), 'utf8'), 'app = "demo"\n')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('rejects artifacts that overlap protected build output', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'file', path: 'manifest.json', contents: '{}' }
        ] } } } }`,
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /overlaps protected build output/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('materializes Netlify Frameworks API artifacts at the project root', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'file', path: '.netlify/v1/config.json', scope: 'project', contents: '{"headers":[]}' }
        ] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({ routes: [{ kind: 'page', path: '/', render: { strategy: 'ssg' } }] }),
      )

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [
        { kind: 'file', path: '.netlify/v1/config.json', scope: 'project' },
      ])
      assert.equal(
        await readFile(path.join(root, '.netlify/v1/config.json'), 'utf8'),
        '{"headers":[]}',
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // An API-only app has no prerendered pages. A static-site artifact marked
  // optional must assemble assets and client bundles instead of failing with
  // RUV2202.
  it('tolerates a missing prerender directory for optional static-site artifacts', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'assets'), { recursive: true })
      await mkdir(path.join(outputDir, 'client'), { recursive: true })
      await writeFile(path.join(outputDir, 'assets', 'logo.svg'), '<svg/>')
      await writeFile(path.join(outputDir, 'client', 'app.js'), 'export {}')
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'static-site', path: 'deploy/node/public', optional: true }
        ] } } } }`,
      )
      await writeFile(
        path.join(outputDir, 'manifest.json'),
        JSON.stringify({ routes: [{ kind: 'api', path: '/api/health' }] }),
      )

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [{ kind: 'static-site', path: 'deploy/node/public' }])
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/node/public/logo.svg'), 'utf8'),
        '<svg/>',
      )
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/node/public/__ruvyxa/client/app.js'), 'utf8'),
        'export {}',
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // The same function bundle emitted at several destinations (deploy directory
  // plus a platform discovery directory) must compile once and copy after.
  it('reuses an identical function bundle across destinations', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await installFakeReact(root)
      await mkdir(path.join(root, 'app', 'api', 'echo'), { recursive: true })
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'app', 'api', 'echo', 'route.ts'),
        `export async function GET() { return Response.json({ ok: true }) }`,
      )
      const manifest = {
        routes: [
          {
            id: 'app/api/echo/route',
            kind: 'api',
            path: '/api/echo',
            file: 'app/api/echo/route.ts',
            layoutChain: [],
            render: { strategy: 'ssr' },
          },
        ],
      }
      await writeFile(path.join(outputDir, 'manifest.json'), JSON.stringify(manifest))
      const handlerSource = `import { loadRouteModule } from './route-modules.mjs'
export default loadRouteModule
`
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return {
          artifacts: [
            { kind: 'function', path: 'deploy/a', handlerSource: ${JSON.stringify(handlerSource)} },
            { kind: 'function', path: 'deploy/b', handlerSource: ${JSON.stringify(handlerSource)} }
          ]
        } } } }`,
      )

      const result = await runRunner(root, outputDir)

      assert.deepEqual(result.result, [
        { kind: 'function', path: 'deploy/a' },
        { kind: 'function', path: 'deploy/b' },
      ])
      const first = await readFile(path.join(outputDir, 'deploy/a/route-modules.mjs'), 'utf8')
      const second = await readFile(path.join(outputDir, 'deploy/b/route-modules.mjs'), 'utf8')
      assert.equal(first, second)
      assert.equal(
        await readFile(path.join(outputDir, 'deploy/b/index.mjs'), 'utf8'),
        handlerSource,
      )
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('reports the resolution candidates for an unknown named adapter', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })

      const result = await runRunnerResult(root, outputDir, 'does-not-exist')

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /RUV2203 adapter does-not-exist could not be resolved/)
      assert.match(result.parsed.message, /@ruvyxa\/adapter-does-not-exist/)
      assert.match(result.parsed.message, /ruvyxa-adapter-does-not-exist/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // `adapterOptions` was declared in RuvyxaConfig, validated by the config
  // renderer, written into build.json, and documented -- while this runner
  // called the factory with no arguments, so every adapter selected by name
  // (which is every zero-config deploy) was stuck on its defaults.
  it('hands config.adapterOptions to an adapter selected by name', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-options-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'assets'), { recursive: true })
      await mkdir(path.join(outputDir, 'client'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      await installFakeReact(root)
      await mkdir(path.join(root, 'app'), { recursive: true })
      await writeFile(path.join(outputDir, 'manifest.json'), JSON.stringify({ routes: [] }))
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapterOptions: { serviceName: 'checkout-api' } }`,
      )

      await runRunner(root, outputDir, 'render')

      const blueprint = await readFile(path.join(outputDir, 'deploy/render/render.yaml'), 'utf8')
      assert.match(blueprint, /name: "checkout-api"/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // The options reach the factory itself, not a copy the runner interprets:
  // the adapter's own validator is what rejects this one.
  it('reports an adapter option the factory refuses', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-options-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapterOptions: { serviceName: 'Not A Valid Name!' } }`,
      )

      const result = await runRunnerResult(root, outputDir, 'render')

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /RUV2001/)
      assert.match(result.parsed.message, /serviceName/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('refuses adapterOptions beside an already-constructed config.adapter', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-options-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default {
          adapter: {
            name: 'fixture', target: 'static', supports: ['ssg'],
            build() { return { name: 'fixture', target: 'static', entry: 'x', assetsDir: 'y' } }
          },
          adapterOptions: { region: 'iad1' },
        }`,
      )

      const result = await runRunnerResult(root, outputDir, undefined)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /RUV2200 config\.adapterOptions/)
      assert.match(result.parsed.message, /Pass the options to the factory instead/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('rejects adapterOptions that is not an options object', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-options-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapterOptions: ['iad1'] }`,
      )

      const result = await runRunnerResult(root, outputDir, 'render')

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /RUV2200 config\.adapterOptions must be an object/)
      assert.match(result.parsed.message, /got array/)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  // Official adapters resolve through the ruvyxa package's own dependencies,
  // so `--adapter node` works in a project that never installed the adapter.
  it('resolves official adapters without a project install', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(path.join(outputDir, 'assets'), { recursive: true })
      await mkdir(path.join(outputDir, 'client'), { recursive: true })
      await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
      await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
      await installFakeReact(root)
      await mkdir(path.join(root, 'app'), { recursive: true })
      await writeFile(path.join(outputDir, 'manifest.json'), JSON.stringify({ routes: [] }))

      const result = await runRunner(root, outputDir, 'node')

      const kinds = result.result.map(({ kind, path: artifactPath }) => ({
        kind,
        path: artifactPath,
      }))
      assert.deepEqual(kinds, [
        { kind: 'function', path: 'deploy/node/server' },
        { kind: 'static-site', path: 'deploy/node/public' },
        { kind: 'file', path: 'deploy/node/start.mjs' },
        { kind: 'file', path: 'deploy/node/README.md' },
      ])
      await readFile(path.join(outputDir, 'deploy/node/server/index.mjs'), 'utf8')
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('resolves and materializes every new zero-config provider adapter', async () => {
    const providers = [
      {
        name: 'railway',
        expected: 'deploy/railway/server/index.mjs',
      },
      {
        name: 'render',
        expected: 'deploy/render/server/index.mjs',
      },
      {
        name: 'firebase',
        expected: 'deploy/firebase/functions/package.json',
      },
      {
        name: 'aws',
        expected: '.amplify-hosting/deploy-manifest.json',
      },
    ]

    for (const provider of providers) {
      const root = await mkdtemp(path.join(os.tmpdir(), `ruvyxa-${provider.name}-runner-`))
      const outputDir = path.join(root, '.ruvyxa-staging')
      try {
        await mkdir(path.join(outputDir, 'assets'), { recursive: true })
        await mkdir(path.join(outputDir, 'client'), { recursive: true })
        await mkdir(path.join(outputDir, 'prerender'), { recursive: true })
        await writeFile(path.join(outputDir, 'prerender', 'index.html'), '<main>home</main>')
        await writeFile(path.join(outputDir, 'manifest.json'), JSON.stringify({ routes: [] }))

        const result = await runRunner(root, outputDir, provider.name)

        assert.ok(result.result.length > 0, provider.name)
        const expectedPath = provider.expected.startsWith('.')
          ? path.join(root, provider.expected)
          : path.join(outputDir, provider.expected)
        await readFile(expectedPath, 'utf8')
      } finally {
        await rm(root, { recursive: true, force: true })
      }
    }
  })
})

async function installFakeReact(root) {
  const reactDir = path.join(root, 'node_modules', 'react')
  const reactDomDir = path.join(root, 'node_modules', 'react-dom')
  await mkdir(reactDir, { recursive: true })
  await mkdir(reactDomDir, { recursive: true })
  await writeFile(
    path.join(reactDir, 'package.json'),
    JSON.stringify({
      name: 'react',
      type: 'module',
      exports: { '.': './index.js', './jsx-runtime': './jsx-runtime.js' },
    }),
  )
  await writeFile(
    path.join(reactDir, 'index.js'),
    `export function createElement(type, props, ...children) {
      return { type, props: { ...(props ?? {}), children: children.length > 1 ? children : children[0] } }
    }
    export function createContext(defaultValue) {
      const context = { _currentValue: defaultValue }
      context.Provider = function Provider(props) { return props.children }
      context.Consumer = function Consumer(props) { return props.children(context._currentValue) }
      return context
    }
    export class Component {
      constructor(props) { this.props = props; this.state = null }
      setState(next) { this.state = { ...(this.state ?? {}), ...next } }
    }
    export function Suspense(props) { return props.children }
    export default { createElement, createContext, Component, Suspense }
    `,
  )
  await writeFile(
    path.join(reactDir, 'jsx-runtime.js'),
    `export function jsx(type, props) { return { type, props: props ?? {} } }
     export const jsxs = jsx
     export const Fragment = Symbol.for('fake.fragment')
    `,
  )
  await writeFile(
    path.join(reactDomDir, 'package.json'),
    JSON.stringify({
      name: 'react-dom',
      type: 'module',
      exports: { './server': './server.js', './server.browser': './server.js' },
    }),
  )
  await writeFile(
    path.join(reactDomDir, 'server.js'),
    `function render(value) {
      if (value == null || value === false) return ''
      if (Array.isArray(value)) return value.map(render).join('')
      if (typeof value !== 'object') return String(value)
      if (typeof value.type === 'function') return render(value.type(value.props ?? {}))
      const children = render(value.props?.children)
      return '<' + value.type + '>' + children + '</' + value.type + '>'
    }
    export function renderToString(tree) { return render(tree) }
    `,
  )
}

function runRunner(root, outputDir, adapterName, env = {}) {
  return new Promise((resolve, reject) => {
    const args = [adapterRunner, root, outputDir]
    if (adapterName) args.push(adapterName)
    const child = spawn(process.execPath, args, {
      stdio: 'pipe',
      env: { ...process.env, ...env },
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', reject)
    child.on('close', (code) => {
      try {
        const parsed = JSON.parse(stdout)
        if (code === 0 && parsed.ok) resolve(parsed)
        else reject(new Error(`adapter runner failed (${code}): ${stdout || stderr}`))
      } catch (error) {
        reject(
          new Error(`invalid runner JSON: ${error.message}; stdout=${stdout}; stderr=${stderr}`),
        )
      }
    })
  })
}

/**
 * Render one path through an emitted function, in a process with no `NODE_ENV`.
 *
 * A child rather than an `import()` here, for two reasons that both matter: the
 * pin the caller is checking mutates `process.env` of whatever process loads the
 * bundle, and this test's whole subject is what the bundle does when the host
 * exported nothing — which cannot be arranged for the test runner itself.
 */
function renderThroughFunction(functionDir, requestPath) {
  const entry = pathToFileURL(path.join(functionDir, 'index.mjs')).href
  const source = `const { default: handler } = await import(${JSON.stringify(entry)})
const response = await handler(new Request('http://localhost${requestPath}'))
process.stdout.write(await response.text())`
  const env = { ...process.env }
  delete env.NODE_ENV
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['--input-type=module', '-e', source], {
      stdio: 'pipe',
      env,
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', reject)
    child.on('close', (code) => {
      if (code === 0) resolve(stdout)
      else reject(new Error(`render failed (${code}): ${stderr}`))
    })
  })
}

function runRunnerResult(root, outputDir, adapterName) {
  return new Promise((resolve, reject) => {
    const args = [adapterRunner, root, outputDir]
    if (adapterName) args.push(adapterName)
    const child = spawn(process.execPath, args, { stdio: 'pipe' })
    let stdout = ''
    child.stdout.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.on('error', reject)
    child.on('close', (exitCode) => {
      try {
        resolve({ exitCode, parsed: JSON.parse(stdout) })
      } catch (error) {
        reject(new Error(`invalid runner JSON: ${error.message}; stdout=${stdout}`))
      }
    })
  })
}
