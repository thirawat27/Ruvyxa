import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import test, { after } from 'node:test'

import { createFixtureWorkspace } from './fixture-workspace.mjs'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const workerScript = path.join(repoRoot, 'packages/ruvyxa/runtime/worker-pool.mjs')
const fixtureWorkspace = await createFixtureWorkspace(
  'ruvyxa-worker-tests-',
  path.join(repoRoot, 'examples/demo'),
)
after(() => rm(fixtureWorkspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

/// Spawns one worker with a request/response helper and registers cleanup on `t`.
function startWorker(t, cleanupDirs = []) {
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
    for (const dir of cleanupDirs) {
      await rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
    }
  })

  return { request }
}

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
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'cache-test-'))
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

test('forwards action request headers and preserves repeated response headers', async (t) => {
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'action-test-'))
  const actionFile = path.join(projectRoot, 'app/account/action.ts')
  await mkdir(path.dirname(actionFile), { recursive: true })
  await writeFile(
    actionFile,
    `import { action } from 'ruvyxa/server'
export const inspect = action.handler(async ({ request }) => {
  const headers = new Headers()
  headers.append('set-cookie', 'a=1; Path=/')
  headers.append('set-cookie', 'b=2; Path=/')
  return new Response(request.headers.get('authorization') || '', { headers })
})
`,
  )

  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const lines = createInterface({ input: worker.stdout })
  const responsePromise = new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('worker action timed out')), 10_000)
    lines.once('line', (line) => {
      clearTimeout(timer)
      resolve(JSON.parse(line))
    })
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

  worker.stdin.write(
    `${JSON.stringify({
      id: 'action',
      type: 'action',
      projectRoot,
      actionFile,
      actionName: 'inspect',
      payloadJson: '{}',
      contentType: 'application/json',
      requestPath: '/account',
      headerPairs: [
        ['authorization', 'Bearer worker-token'],
        ['cookie', 'a=1'],
        ['cookie', 'b=2'],
      ],
    })}\n`,
  )

  const response = await responsePromise
  assert.equal(response.ok, true, response.message)
  assert.equal(response.body, 'Bearer worker-token')
  assert.deepEqual(
    response.headerPairs.filter(([name]) => name === 'set-cookie'),
    [
      ['set-cookie', 'a=1; Path=/'],
      ['set-cookie', 'b=2; Path=/'],
    ],
  )
})

test('resolves static params and isolates build-time page module state', async (t) => {
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'render-test-'))
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

test('parses action payloads by content type and rejects malformed JSON', async (t) => {
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'action-payload-test-'))
  const actionFile = path.join(projectRoot, 'app/todos/action.ts')
  await mkdir(path.dirname(actionFile), { recursive: true })
  await writeFile(
    actionFile,
    `import { action } from 'ruvyxa/server'
export const createTodo = action
  .input({
    parse(value) {
      return { title: String(value.title).trim() }
    },
  })
  .realtime(['todos'])
  .handler(async ({ input, invalidate }) => {
    invalidate('todos')
    return { title: input.title, completed: false }
  })

export const rejectTodo = action.realtime('todos').handler(async () => {
  return new Response('rejected', { status: 422 })
})

export const routeTodo = action.realtime().handler(async () => ({ ok: true }))
`,
  )
  const { request } = startWorker(t, [projectRoot])

  const base = {
    type: 'action',
    projectRoot,
    actionFile,
    actionName: 'createTodo',
    requestPath: '/todos',
    headerPairs: [],
  }

  const json = await request({
    ...base,
    payloadJson: JSON.stringify({ title: 'Test' }),
    contentType: 'application/json',
  })
  assert.equal(json.ok, true, json.message)
  assert.equal(json.status, 200)
  assert.deepEqual(JSON.parse(json.body), {
    data: { title: 'Test', completed: false },
    invalidated: ['todos'],
  })
  const realtimeEvent = JSON.parse(
    Buffer.from(json.headers['x-ruvyxa-realtime-event'], 'base64url').toString('utf8'),
  )
  assert.deepEqual(realtimeEvent, {
    version: 1,
    type: 'action',
    channels: ['todos'],
    action: 'createTodo',
    path: '/todos',
    invalidated: ['todos'],
  })

  const rejected = await request({
    ...base,
    actionName: 'rejectTodo',
    payloadJson: '{}',
    contentType: 'application/json',
  })
  assert.equal(rejected.ok, true, rejected.message)
  assert.equal(rejected.status, 422)
  assert.equal(rejected.headers['x-ruvyxa-realtime-event'], undefined)

  const longRoute = `/${'segment/'.repeat(30)}`
  const routeScoped = await request({
    ...base,
    actionName: 'routeTodo',
    requestPath: longRoute,
    payloadJson: '{}',
    contentType: 'application/json',
  })
  const routeEvent = JSON.parse(
    Buffer.from(routeScoped.headers['x-ruvyxa-realtime-event'], 'base64url').toString('utf8'),
  )
  assert.equal(routeEvent.channels[0], 'route-hash:64d412af0acae2fa')

  const form = await request({
    ...base,
    payloadJson: 'title=Form+Todo',
    contentType: 'application/x-www-form-urlencoded',
  })
  assert.equal(form.ok, true, form.message)
  assert.equal(JSON.parse(form.body).data.title, 'Form Todo')

  const missing = await request({
    ...base,
    actionName: 'missingAction',
    payloadJson: '{}',
    contentType: 'application/json',
  })
  assert.equal(missing.ok, true, missing.message)
  assert.equal(missing.status, 404)

  const malformed = await request({
    ...base,
    payloadJson: 'title=Wrong+Parser',
    contentType: 'application/json',
  })
  assert.equal(malformed.ok, false, 'malformed JSON must not be reinterpreted as form input')
})

test('client bundles hydrate cleanly and enforce boundary diagnostics', async (t) => {
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'client-test-'))
  const appDir = path.join(projectRoot, 'app')
  const pageFile = path.join(appDir, 'page.tsx')
  await mkdir(appDir, { recursive: true })
  await writeFile(
    path.join(appDir, 'layout.tsx'),
    'export default function Layout({ children }) { return <html><body>{children}</body></html> }\n',
  )
  await writeFile(pageFile, 'export default function Page() { return <main>Hello</main> }\n')
  const { request } = startWorker(t, [projectRoot])

  const base = { type: 'client', projectRoot, appDir, pageFile, requestPath: '/', params: {} }

  const clean = await request(base)
  assert.equal(clean.ok, true, clean.message)
  assert.match(clean.script, /hydrateRoot/)
  assert.match(clean.script, /__RUVYXA_HYDRATED/)
  assert.doesNotMatch(clean.script, /from ["']react(?:-dom\/client)?["']/)

  await writeFile(
    pageFile,
    'import "server-only"\nexport default function Page() { return <main /> }\n',
  )
  await request({ type: 'invalidate', paths: [pageFile] })
  const serverOnly = await request(base)
  assert.equal(serverOnly.ok, false)
  assert.match(serverOnly.message, /RUV1007/)

  await writeFile(
    pageFile,
    'export default function Page() { return <main>{process.env.DATABASE_URL}</main> }\n',
  )
  await request({ type: 'invalidate', paths: [pageFile] })
  const privateEnv = await request(base)
  assert.equal(privateEnv.ok, false)
  assert.match(privateEnv.message, /RUV1008/)

  await writeFile(
    pageFile,
    'export default function Page() { return <main>{process.env["DATABASE_URL"]}</main> }\n',
  )
  await request({ type: 'invalidate', paths: [pageFile] })
  const bracketEnv = await request(base)
  assert.equal(bracketEnv.ok, false)
  assert.match(bracketEnv.message, /RUV1008/)
})

test('streams large binary API responses as bounded frames', async (t) => {
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'stream-test-'))
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
