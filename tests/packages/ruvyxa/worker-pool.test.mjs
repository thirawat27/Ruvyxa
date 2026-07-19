import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import test from 'node:test'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const workerScript = path.join(repoRoot, 'packages/ruvyxa/runtime/worker-pool.mjs')

test('uses safe worker defaults when numeric environment values are invalid', async (t) => {
  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    env: {
      ...process.env,
      RUVYXA_WORKER_TIMEOUT_MS: '2147483648',
      RUVYXA_MEMORY_LIMIT_MB: 'not-a-number',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const lines = createInterface({ input: worker.stdout })

  t.after(async () => {
    lines.close()
    worker.stdin.end()
    await Promise.race([
      new Promise((resolve) => worker.once('exit', resolve)),
      new Promise((resolve) => setTimeout(resolve, 2_000)),
    ])
    if (worker.exitCode === null) worker.kill()
  })

  const response = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('worker ping timed out')), 10_000)
    lines.once('line', (line) => {
      clearTimeout(timer)
      resolve(JSON.parse(line))
    })
    worker.stdin.write(`${JSON.stringify({ id: 'configuration', type: 'ping' })}\n`)
  })

  assert.equal(response.ok, true)
  assert.equal(response.workerRequestTimeoutMs, 30_000)
  assert.equal(response.memoryPressureThresholdMb, 512)
})

test('rejects numeric environment values with trailing units', async (t) => {
  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    env: {
      ...process.env,
      RUVYXA_WORKER_TIMEOUT_MS: '1234ms',
      RUVYXA_MEMORY_LIMIT_MB: '64mb',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const lines = createInterface({ input: worker.stdout })

  t.after(async () => {
    lines.close()
    worker.stdin.end()
    await Promise.race([
      new Promise((resolve) => worker.once('exit', resolve)),
      new Promise((resolve) => setTimeout(resolve, 2_000)),
    ])
    if (worker.exitCode === null) worker.kill()
  })

  const response = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('worker ping timed out')), 10_000)
    lines.once('line', (line) => {
      clearTimeout(timer)
      resolve(JSON.parse(line))
    })
    worker.stdin.write(`${JSON.stringify({ id: 'configuration', type: 'ping' })}\n`)
  })

  assert.equal(response.ok, true)
  assert.equal(response.workerRequestTimeoutMs, 30_000)
  assert.equal(response.memoryPressureThresholdMb, 512)
})

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

test('resolves static params and isolates build-time page module state', async (t) => {
  const projectRoot = await mkdtemp(path.join(repoRoot, 'examples/demo/.worker-pool-test-'))
  const appDir = path.join(projectRoot, 'app/products/[id]')
  const pageFile = path.join(appDir, 'page.tsx')
  const paramsFile = path.join(appDir, 'params.ts')
  await mkdir(appDir, { recursive: true })
  await writeFile(path.join(projectRoot, 'package.json'), '{"type":"module"}\n')
  await writeFile(paramsFile, "export const suffix = 'first'\n")
  await writeFile(
    pageFile,
    `import { suffix } from './params'
let renders = 0
let discoveries = 0
export function getStaticParams({ routes, route }) {
  if (routes.length !== 2 || route.path !== '/products/[id]' || route.segments[0].name !== 'id') {
    throw new Error('static params context mismatch')
  }
  discoveries += 1
  return { params: ['one-' + suffix + '-' + discoveries, 'two-' + suffix + '-' + discoveries], cache: '1s' }
}
export default function Page({ params }) {
  renders += 1
  return <main>{params.id + ':' + renders}</main>
}
`,
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

  const staticParamsRequest = {
    type: 'staticParams',
    projectRoot,
    pageFile,
    routePath: '/products/[id]',
    segments: [{ name: 'id', catchAll: false, optional: false }],
    routes: [
      { id: 'home', path: '/' },
      { id: 'products', path: '/products/[id]' },
    ],
  }
  const staticParams = await request(staticParamsRequest)
  assert.equal(staticParams.ok, true, staticParams.message)
  assert.deepEqual(staticParams.params, [{ id: 'one-first-1' }, { id: 'two-first-1' }])
  assert.equal(staticParams.cached, false)

  const automaticRender = await request({
    type: 'ssg',
    projectRoot,
    appDir: path.dirname(path.dirname(appDir)),
    pageFile,
    requestPath: '/products/one-first-1',
    params: { id: 'one-first-1' },
    mode: 'full',
    fresh: true,
  })
  assert.equal(automaticRender.ok, true, automaticRender.message)
  assert.match(automaticRender.html, /one-first-1:1/)
  assert.match(automaticRender.dependencyHash, /^[a-f0-9]{64}$/)
  assert.ok(automaticRender.inputs.some((input) => path.resolve(input) === path.resolve(pageFile)))

  const cachedParams = await request(staticParamsRequest)
  assert.equal(cachedParams.ok, true, cachedParams.message)
  assert.deepEqual(cachedParams.params, staticParams.params)
  assert.equal(cachedParams.cached, true)

  await new Promise((resolve) => setTimeout(resolve, 1_100))
  const expiredParams = await request(staticParamsRequest)
  assert.equal(expiredParams.ok, true, expiredParams.message)
  assert.deepEqual(expiredParams.params, [{ id: 'one-first-2' }, { id: 'two-first-2' }])
  assert.equal(expiredParams.cached, false)

  await writeFile(paramsFile, "export const suffix = 'second'\n")
  const invalidation = await request({ type: 'invalidate', paths: [paramsFile] })
  assert.equal(invalidation.ok, true)
  assert.equal(invalidation.invalidated, 2)
  const refreshedParams = await request(staticParamsRequest)
  assert.equal(refreshedParams.ok, true, refreshedParams.message)
  assert.deepEqual(refreshedParams.params, [{ id: 'one-second-1' }, { id: 'two-second-1' }])
  assert.equal(refreshedParams.cached, false)

  await writeFile(
    pageFile,
    `import React from 'react'
export const staticParams = [3, 4]
export default function Page({ params }) {
  return React.createElement('main', null, params.id + ':1')
}
`,
  )
  const pageInvalidation = await request({ type: 'invalidate', paths: [pageFile] })
  assert.equal(pageInvalidation.ok, true)
  assert.equal(pageInvalidation.invalidated, 1)
  const declaredParams = await request(staticParamsRequest)
  assert.equal(declaredParams.ok, true, declaredParams.message)
  assert.deepEqual(declaredParams.params, [{ id: '3' }, { id: '4' }])

  for (const { id } of declaredParams.params) {
    const render = await request({
      type: 'ssg',
      projectRoot,
      appDir: path.dirname(path.dirname(appDir)),
      pageFile,
      requestPath: `/products/${id}`,
      params: { id },
      mode: 'full',
      fresh: true,
    })
    assert.equal(render.ok, true)
    assert.match(render.html, new RegExp(`${id}:1`))
  }
})

test('streams large binary API responses as bounded frames', async (t) => {
  const projectRoot = await mkdtemp(path.join(repoRoot, '.worker-pool-stream-test-'))
  const appDir = path.join(projectRoot, 'app/api/binary')
  const routeFile = path.join(appDir, 'route.ts')
  await mkdir(appDir, { recursive: true })
  await writeFile(
    routeFile,
    `export function GET() {
  const bytes = new Uint8Array(150_000)
  for (let index = 0; index < bytes.length; index++) bytes[index] = index % 251
  return new Response(bytes, {
    status: 206,
    headers: { 'content-type': 'application/octet-stream', 'x-streamed': 'yes' },
  })
}
`,
  )

  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const lines = createInterface({ input: worker.stdout })

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

  const frames = await new Promise((resolve, reject) => {
    const received = []
    const timer = setTimeout(() => reject(new Error('streamed worker request timed out')), 10_000)
    lines.on('line', (line) => {
      const response = JSON.parse(line)
      if (response.id !== 'stream') return
      received.push(response)
      if (response.frame === 'api-end' || response.frame === 'api-error' || !response.frame) {
        clearTimeout(timer)
        resolve(received)
      }
    })
    worker.stdin.write(
      `${JSON.stringify({
        id: 'stream',
        type: 'api',
        projectRoot,
        routeFile,
        method: 'GET',
        requestPath: '/api/binary',
        headers: {},
        params: {},
        streamResponse: true,
      })}\n`,
    )
  })

  assert.equal(frames[0].frame, 'api-start', frames[0].message)
  assert.equal(frames[0].status, 206)
  assert.equal(frames[0].headers['content-type'], 'application/octet-stream')
  assert.equal(frames[0].headers['x-streamed'], 'yes')
  assert.equal(frames.at(-1).frame, 'api-end')

  const chunks = frames.filter((frame) => frame.frame === 'api-chunk')
  assert.ok(chunks.length >= 3)
  const decoded = chunks.map((frame) => Buffer.from(frame.bodyBase64, 'base64'))
  assert.ok(decoded.every((chunk) => chunk.length <= 64 * 1024))

  const body = Buffer.concat(decoded)
  assert.equal(body.length, 150_000)
  for (const index of [0, 1, 250, 251, 65_535, 149_999]) {
    assert.equal(body[index], index % 251)
  }
})
