/**
 * What a server-components route's entry answer is derived from.
 *
 * `rscClientEntry` runs two compiles and reports one answer. The first walks
 * the route with React's `react-server` condition and stops at every
 * `'use client'` module: it reads that module — it has to, to see the directive
 * — turns it into a reference, and does not follow its imports. The second
 * compiles those references, and is therefore the only graph that can see a
 * `'use server'` module a client component imports, or produce
 * `serverReferences` at all.
 *
 * The reported `inputs` have to cover both, because a build caches this answer
 * against them. Reported from the server graph alone they stop one file short
 * of everything a client component imports — including the actions module whose
 * source versions the reference ids in that answer. A build would keep handing
 * the browser proxies for a function id the server no longer registers, and
 * every call through them fails at run time.
 *
 * The project is written under `.test-build/` rather than the OS temp directory
 * for the same reason the sibling server-components test is: the compiler
 * resolves `react` and `react-server-dom-webpack` by walking up to the nearest
 * `node_modules`, and a project outside the workspace has none to find.
 */
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { createInterface } from 'node:readline'
import test, { after } from 'node:test'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const workerScript = path.join(repoRoot, 'packages/ruvyxa/runtime/worker-pool.mjs')

const scratchRoot = path.join(repoRoot, '.test-build')
await mkdir(scratchRoot, { recursive: true })
const projectRoot = await mkdtemp(path.join(scratchRoot, 'rsc-entry-'))
after(() => rm(projectRoot, { recursive: true, force: true }))

const appDir = path.join(projectRoot, 'app')
await mkdir(appDir, { recursive: true })
await writeFile(path.join(projectRoot, 'package.json'), '{"type":"module"}\n', 'utf8')

await writeFile(
  path.join(appDir, 'actions.ts'),
  `'use server'
export async function save(word: string): Promise<string> {
  return 'saved ' + word
}
`,
  'utf8',
)

// A client component, and the only route into the action. The server graph
// reads this file and stops, so `actions.ts` behind it is reachable by the
// registry compile alone — which is what the assertions below are about.
await writeFile(
  path.join(appDir, 'form.tsx'),
  `'use client'
import { save } from './actions'
export default function Form() {
  return <form action={() => save('x')}><button>save</button></form>
}
`,
  'utf8',
)

await writeFile(
  path.join(appDir, 'page.tsx'),
  `import Form from './form'
export const serverComponents = true
export default function Page() {
  return <main><Form /></main>
}
`,
  'utf8',
)

test('reports the inputs of both compiles behind a route entry', async (t) => {
  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const lines = createInterface({ input: worker.stdout })
  const pending = new Map()
  lines.on('line', (line) => {
    const response = JSON.parse(line)
    pending.get(response.id)?.(response)
    pending.delete(response.id)
  })
  let stderr = ''
  worker.stderr.setEncoding('utf8')
  worker.stderr.on('data', (chunk) => {
    stderr += chunk
  })
  let nextId = 1
  const request = (payload) =>
    new Promise((resolve, reject) => {
      const id = String(nextId++)
      const timer = setTimeout(
        () => reject(new Error(`worker request ${id} timed out\n${stderr}`)),
        120_000,
      )
      pending.set(id, (response) => {
        clearTimeout(timer)
        resolve(response)
      })
      worker.stdin.write(`${JSON.stringify({ id, ...payload })}\n`)
    })

  t.after(async () => {
    lines.close()
    worker.stdin.end()
    await Promise.race([
      new Promise((resolve) => worker.once('exit', resolve)),
      new Promise((resolve) => setTimeout(resolve, 2_000)),
    ])
    if (worker.exitCode === null) worker.kill()
  })

  const entry = await request({
    type: 'rscClientEntry',
    projectRoot,
    appDir,
    pageFile: path.join(appDir, 'page.tsx'),
    routePath: '/',
  })
  assert.equal(entry.ok, true, `${entry.code ?? ''} ${entry.message ?? ''}\n${stderr}`)
  assert.ok(entry.entrySource.length > 0, 'the route needs a browser entry')

  // Only the registry compile can see this, which is what makes its inputs
  // load-bearing rather than incidental.
  assert.deepEqual(
    entry.serverReferences.map((reference) => path.basename(reference.file)),
    ['actions.ts'],
  )

  const inputs = new Set(entry.inputs.map((input) => path.resolve(input)))
  assert.ok(
    inputs.has(path.resolve(appDir, 'page.tsx')),
    'the route the server graph compiled must be an input',
  )
  assert.ok(
    inputs.has(path.resolve(appDir, 'form.tsx')),
    'the client component is read by both compiles and must be an input',
  )
  // The one the server graph cannot reach: it read `form.tsx` to find the
  // directive and stopped there, so the module behind it is known only to the
  // registry. The reference ids reported above are versioned by this file's
  // source, so an answer cached without it goes stale silently.
  assert.ok(
    inputs.has(path.resolve(appDir, 'actions.ts')),
    'a module reachable only through a client component must be an input',
  )
  assert.match(
    entry.inputsVersion,
    /^[a-f0-9]{16}$/,
    'the version has to describe the list that was reported, not one of the two compiles',
  )
})
