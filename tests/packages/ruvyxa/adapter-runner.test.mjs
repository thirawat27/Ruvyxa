import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile, mkdir } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const adapterRunner = path.join(workspaceRoot, 'packages/ruvyxa/runtime/adapter-runner.mjs')

describe('adapter runner', () => {
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
          { kind: 'file', path: 'wrangler.jsonc', scope: 'project', skipIfExists: true, contents: '{"name":"app"}' }
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
      await assert.rejects(readFile(path.join(root, '.vercel/output/static/stale.js')))
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('rejects project-scope artifacts outside the allowlist', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-adapter-runner-'))
    const outputDir = path.join(root, '.ruvyxa-staging')
    try {
      await mkdir(outputDir, { recursive: true })
      await writeFile(
        path.join(root, 'ruvyxa.config.mjs'),
        `export default { adapter: { build() { return { artifacts: [
          { kind: 'file', path: 'package.json', scope: 'project', contents: '{}' }
        ] } } } }`,
      )

      const result = await runRunnerResult(root, outputDir)

      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /project-scope adapter artifact path is not allowlisted/)
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

function runRunner(root, outputDir, adapterName) {
  return new Promise((resolve, reject) => {
    const args = [adapterRunner, root, outputDir]
    if (adapterName) args.push(adapterName)
    const child = spawn(process.execPath, args, { stdio: 'pipe' })
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
