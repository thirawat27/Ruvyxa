import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { createFixtureWorkspace } from './fixture-workspace.mjs'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const exampleRoot = path.join(workspaceRoot, 'examples/demo')
const apiRenderer = path.join(workspaceRoot, 'packages/ruvyxa/runtime/api-renderer.mjs')
const workerPool = path.join(workspaceRoot, 'packages/ruvyxa/runtime/worker-pool.mjs')
const fixtureWorkspace = await createFixtureWorkspace('ruvyxa-api-tests-', exampleRoot)
after(() => rm(fixtureWorkspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

describe('API renderer request forwarding', () => {
  it('forwards POST body and headers through the standalone renderer', async () => {
    await withFixture(async ({ root, routeFile }) => {
      const result = await runJson(apiRenderer, [
        root,
        routeFile,
        'POST',
        '/api/echo?source=cli',
        '{}',
        JSON.stringify({ title: 'from-cli' }),
        JSON.stringify({ 'content-type': 'application/json', 'x-test': 'cli' }),
      ])

      assert.equal(result.status, 200)
      assert.deepEqual(JSON.parse(result.body), {
        body: { title: 'from-cli' },
        header: 'cli',
        query: 'cli',
      })
    })
  })

  it('forwards POST body and headers through the persistent worker pool', async () => {
    await withFixture(async ({ root, routeFile }) => {
      const result = await runWorkerJson({
        id: 'api-body-test',
        type: 'api',
        projectRoot: root,
        routeFile,
        method: 'POST',
        requestPath: '/api/echo?source=worker',
        params: {},
        headers: { 'content-type': 'application/json', 'x-test': 'worker' },
        body: JSON.stringify({ title: 'from-worker' }),
      })

      assert.equal(result.status, 200)
      assert.deepEqual(JSON.parse(result.body), {
        body: { title: 'from-worker' },
        header: 'worker',
        query: 'worker',
      })
    })
  })

  it('preserves binary API bodies, duplicate request headers, and repeated Set-Cookie values', async () => {
    await withFixture(async ({ root, routeFile }) => {
      const body = Buffer.from([0, 255, 128, 13, 10])
      const result = await runWorkerJson({
        id: 'api-binary-and-headers-test',
        type: 'api',
        projectRoot: root,
        routeFile,
        method: 'POST',
        requestPath: '/api/echo',
        params: {},
        headers: { 'content-type': 'application/octet-stream' },
        headerPairs: [
          ['content-type', 'application/octet-stream'],
          ['x-repeat', 'first'],
          ['x-repeat', 'second'],
        ],
        bodyBase64: body.toString('base64'),
      })

      assert.equal(result.status, 200)
      assert.deepEqual(JSON.parse(result.body), {
        bytes: [...body],
        repeated: 'first, second',
      })
      assert.deepEqual(
        result.headerPairs.filter(([name]) => name === 'set-cookie'),
        [
          ['set-cookie', 'first=1; Path=/'],
          ['set-cookie', 'second=2; Path=/'],
        ],
      )
    })
  })

  it('prebundles a route dependency graph in the persistent worker', async () => {
    await withFixture(async ({ root, appDir, pageFile }) => {
      const result = await runWorkerJson({
        id: 'dependency-prebundle-test',
        type: 'warmup',
        projectRoot: root,
        routes: [{ pageFile, appDir }],
      })

      assert.equal(result.ok, true)
      assert.equal(result.warmed, 1)
      assert.ok(result.moduleCacheSize >= 1)
    })
  })
})

function runJson(script, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {
      stdio: ['ignore', 'pipe', 'pipe'],
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
        if (code === 0 && parsed.ok) {
          resolve(parsed)
        } else {
          reject(new Error(`script failed (${code}): ${stdout || stderr}`))
        }
      } catch (error) {
        reject(
          new Error(
            `invalid JSON from script: ${error.message}; stdout=${stdout}; stderr=${stderr}`,
          ),
        )
      }
    })
  })
}

function runWorkerJson(request) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [workerPool], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    let settled = false
    const timeout = setTimeout(() => {
      if (settled) return
      settled = true
      child.kill()
      reject(new Error(`worker timed out; stdout=${stdout}; stderr=${stderr}`))
    }, 10_000)
    timeout.unref()

    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
      const lineEnd = stdout.indexOf('\n')
      if (lineEnd === -1 || settled) return
      settled = true
      clearTimeout(timeout)
      child.kill()
      try {
        resolve(JSON.parse(stdout.slice(0, lineEnd)))
      } catch (error) {
        reject(
          new Error(`invalid worker JSON: ${error.message}; stdout=${stdout}; stderr=${stderr}`),
        )
      }
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', (error) => {
      if (settled) return
      settled = true
      clearTimeout(timeout)
      reject(error)
    })
    child.on('close', (code) => {
      if (settled) return
      settled = true
      clearTimeout(timeout)
      reject(
        new Error(`worker exited before response (${code}); stdout=${stdout}; stderr=${stderr}`),
      )
    })
    child.stdin.write(`${JSON.stringify(request)}\n`)
  })
}

async function withFixture(run) {
  const root = await mkdtemp(path.join(fixtureWorkspace, 'fixture-'))
  const routeDir = path.join(root, 'app/api/echo')
  const appDir = path.join(root, 'app')
  const routeFile = path.join(routeDir, 'route.ts')
  const pageFile = path.join(appDir, 'page.tsx')
  await mkdir(routeDir, { recursive: true })
  await writeFile(
    routeFile,
    `
      export async function POST({ request }) {
        if (request.headers.get('content-type') === 'application/octet-stream') {
          return new Response(JSON.stringify({
            bytes: [...new Uint8Array(await request.arrayBuffer())],
            repeated: request.headers.get('x-repeat'),
          }), {
            headers: [
              ['content-type', 'application/json; charset=utf-8'],
              ['set-cookie', 'first=1; Path=/'],
              ['set-cookie', 'second=2; Path=/'],
            ],
          })
        }
        return Response.json({
          body: await request.json(),
          header: request.headers.get('x-test'),
          query: new URL(request.url).searchParams.get('source'),
        })
      }
    `,
  )
  await writeFile(pageFile, 'export default function Page() { return <main>Warm</main> }\n')

  try {
    await run({ root, appDir, pageFile, routeFile })
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}
