/**
 * One route, rendered the way a server-components route is actually rendered.
 *
 * Everything here runs the real pipeline: Ruvyxa's own compiler builds the
 * `react-server` graph, React's own `react-server-dom-webpack` produces the
 * Flight payload, and the SSR pass turns it into HTML. Nothing is stubbed,
 * because the failures this feature can have are exactly the ones a stub hides
 * — a `'use client'` module reaching the server graph, an id the two realms
 * spell differently, or two copies of React in one render.
 *
 * The project is written under `.test-build/` rather than into the OS temp
 * directory on purpose: the compiler resolves `react` and
 * `react-server-dom-webpack` by walking up to the nearest `node_modules`, and a
 * project outside the workspace has none to find.
 */
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const runtimeDir = path.join(workspaceRoot, 'packages/ruvyxa/runtime')
const load = (file) => import(pathToFileURL(path.join(runtimeDir, file)).href)

const { compileBundleWithMetadata, runtimeAliases, serverPlatform, toImportPath } =
  await load('compiler.mjs')
const { rscClientEntrySource, rscServerEntrySource } = await load('entry-templates.mjs')
const { clientModuleId, clientRegistrySource } = await load('client-references.mjs')
const { renderServerComponents } = await load('server-components.mjs')

const scratchRoot = path.join(workspaceRoot, '.test-build')
await mkdir(scratchRoot, { recursive: true })
const projectRoot = await mkdtemp(path.join(scratchRoot, 'server-components-'))
after(() => rm(projectRoot, { recursive: true, force: true }))

const appDir = path.join(projectRoot, 'app')
await mkdir(appDir, { recursive: true })

await writeFile(
  path.join(appDir, 'counter.tsx'),
  `'use client'
import { useState } from 'react'
export default function Counter({ start }: { start: number }) {
  const [value, setValue] = useState(start)
  return <button onClick={() => setValue(value + 1)}>count {value}</button>
}
export function Badge() {
  return <em>badge</em>
}
`,
  'utf8',
)

await writeFile(
  path.join(appDir, 'layout.tsx'),
  `export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
`,
  'utf8',
)

await writeFile(
  path.join(appDir, 'page.tsx'),
  `import Counter, { Badge } from './counter'
const secret = 'only-on-the-server'
export default async function Page() {
  return (
    <main>
      <h1>{secret}</h1>
      <Counter start={3} />
      <Badge />
    </main>
  )
}
`,
  'utf8',
)

const pageFile = path.join(appDir, 'page.tsx')
const layoutFile = path.join(appDir, 'layout.tsx')
const counterFile = path.join(appDir, 'counter.tsx')

const serverBundle = await compileBundleWithMetadata({
  projectRoot,
  entrySource: rscServerEntrySource({
    imports: [
      `import Page from ${JSON.stringify(toImportPath(pageFile))}`,
      `import Layout0 from ${JSON.stringify(toImportPath(layoutFile))}`,
    ],
    pageName: 'Page',
    layoutNames: ['Layout0'],
    routePath: '/',
  }),
  sourcefile: 'ruvyxa:rsc-server.tsx',
  outfile: path.join(projectRoot, '.ruvyxa/cache/rsc/server.mjs'),
  platform: serverPlatform(),
  bundleTarget: 'react-server',
  bundlePackages: true,
  aliases: runtimeAliases(runtimeDir),
  sourceMap: false,
})

const registry = clientRegistrySource(serverBundle.clientReferences)
const ssrRegistry = await compileBundleWithMetadata({
  projectRoot,
  entrySource: `${registry.imports.join('\n')}\n${registry.statements.join('\n')}\n`,
  sourcefile: 'ruvyxa:rsc-ssr-registry.tsx',
  outfile: path.join(projectRoot, '.ruvyxa/cache/rsc/ssr-registry.mjs'),
  platform: serverPlatform(),
  external: ['react', 'react/jsx-runtime'],
  aliases: runtimeAliases(runtimeDir),
  sourceMap: false,
})

await import(pathToFileURL(ssrRegistry.outfile).href)
const serverModule = await import(pathToFileURL(serverBundle.outfile).href)
const rendered = await renderServerComponents({
  serverModule,
  references: serverBundle.clientReferences,
  ctx: { path: '/', params: {} },
  routePath: '/',
})

describe('the server graph', () => {
  it('reports the `use client` module as a reference, keyed by project-relative path', () => {
    assert.deepEqual(
      serverBundle.clientReferences.map((reference) => reference.relativePath),
      ['app/counter.tsx'],
    )
    assert.equal(serverBundle.clientReferences[0].id, clientModuleId('app/counter.tsx'))
  })

  it('does not contain the client module it references', async () => {
    const { readFile } = await import('node:fs/promises')
    const code = await readFile(serverBundle.outfile, 'utf8')
    // The whole point of the split: a hook the react-server build does not
    // export must never be linked into this graph, and the marker string of the
    // component's body must not either.
    assert.doesNotMatch(code, /setValue/)
    assert.match(code, /only-on-the-server/)
  })

  it('links the server build of React, not the one with hooks', async () => {
    const { readFile } = await import('node:fs/promises')
    const code = await readFile(serverBundle.outfile, 'utf8')
    assert.doesNotMatch(code, /react-jsx-runtime\.development/)
    assert.match(code, /createClientModuleProxy/)
  })
})

describe('the Flight payload', () => {
  it('serialises the client component as a reference and the server output as data', () => {
    const id = clientModuleId('app/counter.tsx')
    assert.match(rendered.payload, new RegExp(`I\\["${id}"`))
    assert.match(rendered.payload, /only-on-the-server/)
    // `$L<n>` is how the tree points at a reference row; without it the client
    // component was inlined rather than referenced.
    assert.match(rendered.payload, /\$L\d/)
  })

  it('names one reference per module, not per imported export', () => {
    const id = clientModuleId('app/counter.tsx')
    const rows = rendered.payload.split('\n').filter((line) => line.includes(`I["${id}"`))
    assert.equal(rows.length, 2, rendered.payload)
  })
})

describe('the SSR pass', () => {
  it('renders a full document including the client components markup', () => {
    // React emits the doctype itself, in uppercase; the SSR pass only adds one
    // when the tree did not, which is why this check is case-insensitive.
    assert.match(rendered.html, /^<!doctype html>/i)
    assert.equal(rendered.html.toLowerCase().split('<!doctype').length, 2)
    assert.match(rendered.html, /<html lang="en">/)
    assert.match(rendered.html, /only-on-the-server/)
    // The comment is React's text-boundary marker between a literal and an
    // interpolated value. Asserting it is deliberate: it is part of what makes
    // hydration line up, so its absence would mean the markup came from
    // somewhere other than a real React render.
    assert.match(rendered.html, /<button>count <!-- -->3<\/button>/)
    assert.match(rendered.html, /<em>badge<\/em>/)
  })

  it('renders through one React, so a hook in a client component does not throw', () => {
    // `useState` runs during this render. Two React instances in one process
    // report "invalid hook call" here, which is the failure this pipeline is
    // shaped to avoid.
    assert.doesNotMatch(rendered.html, /invalid hook call/i)
  })
})

describe('the browser entry', () => {
  const source = rscClientEntrySource({
    references: [{ id: clientModuleId('app/counter.tsx'), file: counterFile }],
    routePath: '/',
    requestPathLiteral: '"/"',
    paramsLiteral: '{}',
  })

  it('installs the reference runtime before anything that reads its globals', () => {
    const runtimeAt = source.indexOf('rsc-client-runtime.mjs')
    const decoderAt = source.indexOf('react-server-dom-webpack/client.browser')
    assert.ok(runtimeAt >= 0 && decoderAt >= 0, source)
    assert.ok(runtimeAt < decoderAt, 'the decoder reads __webpack_require__.u while it loads')
  })

  it('registers each reference and never imports the page', () => {
    assert.match(source, /__ruvyxaRegisterClient\("ruv:m_[a-f0-9]{16}", __ruvyxaClient0\)/)
    assert.ok(!source.includes('page.tsx'), 'a server component must not reach the browser')
    assert.match(source, /hydrateRoot\(document/)
  })

  it('leaves a document served without a payload alone', () => {
    assert.match(source, /if \(__ruvyxaPayload !== null\)/)
  })
})
