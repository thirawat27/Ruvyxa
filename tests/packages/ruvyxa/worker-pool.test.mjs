import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import test from 'node:test'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const workerScript = path.join(repoRoot, 'packages/ruvyxa/runtime/worker-pool.mjs')

test('invalidates a cached route bundle when an imported utility changes', async (t) => {
  const projectRoot = await mkdtemp(path.join(repoRoot, '.worker-pool-test-'))
  const appDir = path.join(projectRoot, 'app/api/value')
  const routeFile = path.join(appDir, 'route.ts')
  const utilityFile = path.join(projectRoot, 'lib/value.ts')
  await mkdir(appDir, { recursive: true })
  await mkdir(path.dirname(utilityFile), { recursive: true })
  await writeFile(utilityFile, `export const value = 'first'\n`)
  await writeFile(
    routeFile,
    `import { value } from '../../../lib/value.js'\nexport function GET() { return Response.json({ value }) }\n`,
  )

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
  let nextId = 1
  const request = (payload) =>
    new Promise((resolve, reject) => {
      const id = String(nextId++)
      const timer = setTimeout(() => reject(new Error(`worker request ${id} timed out`)), 10_000)
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
    await rm(projectRoot, { recursive: true, force: true })
  })

  const apiRequest = {
    type: 'api',
    projectRoot,
    routeFile,
    method: 'GET',
    requestPath: '/api/value',
    headers: {},
    params: {},
  }
  const first = await request(apiRequest)
  assert.equal(first.ok, true)
  assert.deepEqual(JSON.parse(first.body), { value: 'first' })

  await writeFile(utilityFile, `export const value = 'second'\n`)
  const invalidation = await request({ type: 'invalidate', paths: [utilityFile] })
  assert.equal(invalidation.ok, true)
  assert.equal(invalidation.invalidated, 1)

  const second = await request(apiRequest)
  assert.equal(second.ok, true)
  assert.deepEqual(JSON.parse(second.body), { value: 'second' })
})
