/**
 * `instrumentation.ts`: the project's process-wide startup hook.
 *
 * The contract worth testing is "once per process, before the first request is
 * served, and never fatal". Each is asserted against a real spawned worker
 * rather than by calling the helper directly, because the thing that could
 * break it — a second call slipping in from a different request handler — only
 * exists in the worker's request loop.
 */

import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const workerScript = path.join(repoRoot, 'packages/ruvyxa/runtime/worker-pool.mjs')

/**
 * A throwaway project with one API route and the given instrumentation source.
 *
 * `instrumentation` may be `null`, which is the "project does not use the hook"
 * case — the one that must stay free of cost and free of noise.
 */
async function createProject(t, instrumentation) {
  const root = await mkdtemp(path.join(tmpdir(), 'ruvyxa-instrumentation-'))
  t.after(() => rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

  await writeFile(
    path.join(root, 'package.json'),
    JSON.stringify({ name: 'instrumentation-fixture', private: true, type: 'module' }),
  )
  await writeFile(
    path.join(root, 'route.ts'),
    `export function GET() {
       return new Response('ok')
     }`,
  )
  if (instrumentation !== null) {
    await writeFile(path.join(root, 'instrumentation.ts'), instrumentation)
  }
  return root
}

function startWorker(t) {
  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const lines = createInterface({ input: worker.stdout })
  const stderr = []
  worker.stderr.on('data', (chunk) => stderr.push(String(chunk)))

  const pending = new Map()
  lines.on('line', (line) => {
    const response = JSON.parse(line)
    pending.get(response.id)?.(response)
    pending.delete(response.id)
  })

  let nextId = 1
  const request = (payload) =>
    new Promise((resolve, reject) => {
      const id = String(nextId++)
      const timer = setTimeout(() => reject(new Error(`worker request ${id} timed out`)), 20_000)
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

  return { request, stderr: () => stderr.join('') }
}

const apiRequest = (root) => ({
  type: 'api',
  projectRoot: root,
  routeFile: path.join(root, 'route.ts'),
  method: 'GET',
  requestPath: '/api/thing',
  headers: {},
  headerPairs: [],
  params: {},
})

test('runs register() once per worker, however many requests arrive', async (t) => {
  // The hook installs process-wide state — a tracing SDK, an error reporter.
  // Running it twice double-registers those, which is how duplicate spans and
  // doubled error reports happen.
  // The marker lives inside the throwaway project, not the working directory:
  // test files run concurrently and the worker is spawned from the repo root,
  // so a shared filename would be a race between suites.
  const root = await createProject(t, null)
  const logFile = path.join(root, 'register.log')
  await writeFile(
    path.join(root, 'instrumentation.ts'),
    `import { appendFileSync } from 'node:fs'
     export async function register() {
       appendFileSync(${JSON.stringify(logFile)}, 'x')
     }`,
  )

  const worker = startWorker(t)
  const first = await worker.request(apiRequest(root))
  const second = await worker.request(apiRequest(root))

  assert.equal(first.ok, true, worker.stderr())
  assert.equal(second.ok, true)
  assert.equal(await readFile(logFile, 'utf8'), 'x', 'register() must run exactly once')
})

test('serves the request before the hook has anything to say', async (t) => {
  const root = await createProject(t, null)
  const worker = startWorker(t)

  const response = await worker.request(apiRequest(root))
  assert.equal(response.ok, true)
  assert.equal(response.status, 200)
  assert.doesNotMatch(
    worker.stderr(),
    /instrumentation/i,
    'a project without the file must produce no instrumentation output at all',
  )
})

test('a throwing register() is reported but does not fail the request', async (t) => {
  // Telemetry exists to observe a working site. A misconfigured exporter must
  // not be the reason the site stops serving.
  const root = await createProject(
    t,
    `export function register() {
       throw new Error('exporter endpoint is unreachable')
     }`,
  )
  const worker = startWorker(t)

  const response = await worker.request(apiRequest(root))
  assert.equal(response.ok, true)
  assert.equal(response.status, 200)
  assert.match(worker.stderr(), /instrumentation failed/)
  assert.match(worker.stderr(), /exporter endpoint is unreachable/)
})

test('a file without register() is called out rather than ignored', async (t) => {
  // Exporting the wrong name produces a file that is compiled, imported, and
  // does nothing. Silence there reads as "instrumentation is working".
  const root = await createProject(t, `export const setup = () => {}`)
  const worker = startWorker(t)

  const response = await worker.request(apiRequest(root))
  assert.equal(response.ok, true)
  assert.match(worker.stderr(), /no exported register\(\) function/)
})

test('the adapter runner and the worker recognise the same filenames', async () => {
  // They run the hook in different processes and locate it independently. A
  // name honoured by one and not the other is a project that works in `ruvyxa
  // dev` and silently loses its telemetry after deployment.
  const { INSTRUMENTATION_FILES } = await import(
    pathToFileUrl(path.join(repoRoot, 'packages/ruvyxa/runtime/compiler.mjs'))
  )
  const conformance = JSON.parse(
    await readFile(
      path.join(repoRoot, 'tests/fixtures/instrumentation-files-conformance.json'),
      'utf8',
    ),
  )
  assert.deepEqual([...INSTRUMENTATION_FILES], conformance.files)

  for (const file of ['worker-pool.mjs', 'adapter-runner.mjs']) {
    const source = await readFile(path.join(repoRoot, 'packages/ruvyxa/runtime', file), 'utf8')
    assert.match(
      source,
      /INSTRUMENTATION_FILES/,
      `${file} must resolve the hook from the shared list, not its own copy`,
    )
  }
  assert.ok(existsSync(path.join(repoRoot, 'packages/ruvyxa/runtime/compiler.mjs')))
})

function pathToFileUrl(absolute) {
  return new URL(`file://${absolute.replaceAll('\\', '/').replace(/^(?=[A-Za-z]:)/, '/')}`)
}
