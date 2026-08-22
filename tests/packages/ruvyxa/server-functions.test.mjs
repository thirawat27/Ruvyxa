/**
 * A `<form action={fn}>` answered without a line of JavaScript running.
 *
 * The hydrated path — React intercepts the submit and posts to
 * `/__ruvyxa/rsc` — is exercised by the endpoint conformance fixture. This file
 * covers the other one: a browser that has not loaded the bundle, or never
 * will, submitting the form the way HTML always has. Three things have to line
 * up for that to work, and each is asserted below:
 *
 *   1. The server-rendered markup carries React's hidden reference fields, so
 *      the submission says which function to run. A form whose action React
 *      could not encode renders `action="javascript:throw …"` instead, and that
 *      string is the exact shape of this feature failing silently.
 *   2. The posted fields resolve to the real function and call it.
 *   3. What it returned is replayed into the re-render, so the answer is in the
 *      document that comes back rather than in a payload nobody will read.
 *
 * Everything runs the real pipeline for the same reason the sibling
 * server-components test does: a stub cannot tell whether the two realms agree
 * about an id, and that agreement is the entire feature.
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
const { rscActionEntrySource, rscServerEntrySource } = await load('entry-templates.mjs')
const { RSC_SSR_PACKAGE, clientRegistrySource, serverManifest } =
  await load('client-references.mjs')
const { renderServerComponents } = await load('server-components.mjs')

const scratchRoot = path.join(workspaceRoot, '.test-build')
await mkdir(scratchRoot, { recursive: true })
const projectRoot = await mkdtemp(path.join(scratchRoot, 'server-functions-'))
after(() => rm(projectRoot, { recursive: true, force: true }))

const appDir = path.join(projectRoot, 'app')
await mkdir(appDir, { recursive: true })

// The reference base every graph must agree on. Two bases give one module two
// ids, and a post then names a function the render never registered.
const referenceBase = path.dirname(appDir)

await writeFile(
  path.join(appDir, 'actions.ts'),
  `'use server'
export async function echo(previous: string | null, form: FormData): Promise<string> {
  return 'echoed ' + String(form.get('word') ?? '') + ' after ' + String(previous)
}
`,
  'utf8',
)

await writeFile(
  path.join(appDir, 'form.tsx'),
  `'use client'
import { useActionState } from 'react'
import { echo } from './actions'
export default function Form() {
  const [answer, submit] = useActionState(echo, null)
  return (
    <form action={submit}>
      <input name="word" defaultValue="hello" />
      <output>{answer ?? 'nothing yet'}</output>
    </form>
  )
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
  `import Form from './form'
export default async function Page() {
  return (
    <main>
      <Form />
    </main>
  )
}
`,
  'utf8',
)

const pageFile = path.join(appDir, 'page.tsx')
const layoutFile = path.join(appDir, 'layout.tsx')

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
  clientReferenceBase: referenceBase,
  aliases: runtimeAliases(runtimeDir),
  sourceMap: false,
})

// The graph that walks the client component, and therefore the only one that
// sees the actions module it imports: the server graph turned the component
// into a reference and never followed its imports.
const registry = clientRegistrySource(serverBundle.clientReferences)
const ssrRegistry = await compileBundleWithMetadata({
  projectRoot,
  entrySource: `${registry.imports.join('\n')}\n${registry.statements.join('\n')}\n`,
  sourcefile: 'ruvyxa:rsc-ssr-registry.tsx',
  outfile: path.join(projectRoot, '.ruvyxa/cache/rsc/ssr-registry.mjs'),
  platform: serverPlatform(),
  bundlePackages: true,
  external: ['react', 'react/jsx-runtime', 'react-dom', 'react-dom/client', 'react-dom/server'],
  serverReferenceClient: RSC_SSR_PACKAGE,
  clientReferenceBase: referenceBase,
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

const actionBundle = await compileBundleWithMetadata({
  projectRoot,
  entrySource: rscActionEntrySource({ references: ssrRegistry.serverReferences }),
  sourcefile: 'ruvyxa:rsc-action.tsx',
  outfile: path.join(projectRoot, '.ruvyxa/cache/rsc/action.mjs'),
  platform: serverPlatform(),
  bundleTarget: 'react-server',
  bundlePackages: true,
  clientReferenceBase: referenceBase,
  aliases: runtimeAliases(runtimeDir),
  sourceMap: false,
})
const actionModule = await import(pathToFileURL(actionBundle.outfile).href)

/** Rebuild the submission a browser would send from the markup it was sent. */
function submissionFrom(html, fields = {}) {
  const form = html.match(/<form\b[^>]*>([\s\S]*?)<\/form>/)
  assert.ok(form, `no <form> in the rendered document:\n${html}`)
  const body = new FormData()
  for (const [, attributes] of form[1].matchAll(/<input\b([^>]*)>/g)) {
    const name = attributes.match(/\bname="([^"]*)"/)?.[1]
    if (!name) continue
    const value = (attributes.match(/\bvalue="([^"]*)"/)?.[1] ?? '')
      .replaceAll('&quot;', '"')
      .replaceAll('&amp;', '&')
    body.append(name, fields[name] ?? value)
  }
  for (const [name, value] of Object.entries(fields)) {
    if (!body.has(name)) body.append(name, value)
  }
  return body
}

describe('the rendered form', () => {
  it('carries the reference fields a submission needs', () => {
    // Present only because the SSR pass resolved the action to something React
    // knows how to encode. A plain function renders no such fields.
    assert.match(rendered.html, /name="\$ACTION_(ID|REF)_/)
    assert.match(rendered.html, /method="POST"/)
    assert.match(rendered.html, /encType="multipart\/form-data"/)
  })

  it('does not render the marker React uses for an action it cannot encode', () => {
    // React's stand-in when a form action has no encoding: submitting it throws
    // in the browser instead of reaching the server. Its presence is exactly
    // what "progressive enhancement is silently off" looks like.
    assert.doesNotMatch(rendered.html, /javascript:throw/)
  })

  it('names the actions module the client component imported', () => {
    assert.deepEqual(
      ssrRegistry.serverReferences.map((reference) => path.basename(reference.file)),
      ['actions.ts'],
    )
    assert.match(rendered.html, /ruv:s_[a-f0-9]{16}#echo/)
  })
})

describe('running the submitted action', () => {
  it('calls the real function with the posted fields', async () => {
    const posted = await actionModule.runFormAction({
      formData: submissionFrom(rendered.html, { word: 'there' }),
      serverManifest: serverManifest(),
    })
    assert.equal(posted.result, 'echoed there after null')
  })

  it('leaves a post that names no action alone', async () => {
    // Any other form on the page — a search box posting to the same URL — must
    // not be mistaken for a server function call.
    const body = new FormData()
    body.append('word', 'unrelated')
    const posted = await actionModule.runFormAction({
      formData: body,
      serverManifest: serverManifest(),
    })
    assert.equal(posted, null)
  })
})

describe('replaying the result into the next render', () => {
  it('puts what the action returned into the document', async () => {
    const posted = await actionModule.runFormAction({
      formData: submissionFrom(rendered.html, { word: 'again' }),
      serverManifest: serverManifest(),
    })
    // `useActionState` produced the action, so React wrote the extra key that
    // makes the return value replayable. Without it there is nothing to replay
    // and the hook would render its initial state again.
    assert.notEqual(posted.formState, null)

    const answered = await renderServerComponents({
      serverModule,
      references: serverBundle.clientReferences,
      ctx: { path: '/', params: {} },
      routePath: '/',
      formState: posted.formState,
    })
    assert.match(answered.html, /echoed again after null/)
    assert.doesNotMatch(answered.html, /nothing yet/)
  })

  it('renders the initial state when no form was posted', () => {
    assert.match(rendered.html, /nothing yet/)
  })
})
