import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import test, { after } from 'node:test'

import { createFixtureWorkspace } from './fixture-workspace.mjs'
import {
  CachePressureController,
  LruCache,
} from '../../../packages/ruvyxa/runtime/cache-budget.mjs'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const workerScript = path.join(repoRoot, 'packages/ruvyxa/runtime/worker-pool.mjs')
const fixtureWorkspace = await createFixtureWorkspace(
  'ruvyxa-worker-tests-',
  path.join(repoRoot, 'examples/demo'),
)

test('cache pressure uses the shared hysteresis contract and reports eviction reasons', async () => {
  const fixture = JSON.parse(
    await readFile(path.join(repoRoot, 'tests/fixtures/cache-budget-contract.json'), 'utf8'),
  )
  assert.equal(fixture.contract, 'ruvyxa.cache-budget')
  assert.equal(fixture.schemaVersion, 1)
  const pressure = new CachePressureController(fixture)

  for (const expected of fixture.observations) {
    assert.deepEqual(pressure.observe(expected.residentBytes), {
      level: expected.level,
      targetBytes: Math.floor(fixture.hardLimitBytes * fixture.targetRatio),
      toFreeBytes: expected.toFreeBytes,
      stopSpeculation: expected.stopSpeculation,
    })
  }
  pressure.recordEviction('bundle', 2)
  pressure.recordEviction('compilerSweep')

  assert.deepEqual(pressure.snapshot(1_000).evictions, { bundle: 2, compilerSweep: 1 })
})

test('cache pressure rejects invalid budgets and observations', () => {
  assert.throws(() => new CachePressureController({ hardLimitBytes: 0 }), TypeError)
  const pressure = new CachePressureController({ hardLimitBytes: 1_000 })
  assert.throws(() => pressure.observe(-1), TypeError)
  assert.throws(() => pressure.recordEviction('bundle', -1), TypeError)
})

test('cache pressure eviction skips entries pinned by active work', () => {
  const cache = new LruCache(3)
  cache.set('oldest', 1)
  cache.set('pinned', 2)
  cache.set('newest', 3)

  assert.deepEqual(cache.evictOldest(new Set(['oldest', 'pinned'])), {
    key: 'newest',
    value: 3,
  })
  assert.equal(cache.has('oldest'), true)
  assert.equal(cache.has('pinned'), true)
})
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
      RUVYXA_WORKER_MAX_QUEUE: '0',
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
  assert.equal(response.cacheBudget.hardLimitBytes, 512 * 1024 * 1024)
  assert.equal(response.cacheBudget.softLimitBytes, Math.floor(512 * 1024 * 1024 * 0.8))
  assert.equal(response.compilerCache.maxEntries, 512)
  assert.equal(response.maxQueuedRequests, response.maxConcurrentRequests * 4)
})

test('rejects numeric environment values with trailing units', async (t) => {
  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    env: {
      ...process.env,
      RUVYXA_WORKER_TIMEOUT_MS: '1234ms',
      RUVYXA_MEMORY_LIMIT_MB: '64mb',
      RUVYXA_WORKER_MAX_QUEUE: '2requests',
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
  assert.equal(response.maxQueuedRequests, response.maxConcurrentRequests * 4)
})

test('bounds concurrent requests and keeps bookkeeping requests unqueued', async (t) => {
  // One slot, so two overlapping renders must serialize instead of both
  // starting. `activeRequests` used to be counted but never enforced, letting a
  // burst start every render at once and exhaust the heap.
  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    env: { ...process.env, RUVYXA_WORKER_MAX_CONCURRENCY: '1' },
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

  const pending = new Map()
  lines.on('line', (line) => {
    const message = JSON.parse(line)
    pending.get(message.id)?.(message)
    pending.delete(message.id)
  })
  const send = (id, payload) => {
    const settled = new Promise((resolve) => pending.set(id, resolve))
    worker.stdin.write(`${JSON.stringify({ id, ...payload })}\n`)
    return settled
  }

  const configuration = await send('configuration', { type: 'ping' })
  assert.equal(configuration.maxConcurrentRequests, 1)

  // Two SSR requests against a nonexistent project: both fail, but only after
  // being admitted, so the second has to wait for the first slot to free.
  const first = send('render-1', {
    type: 'ssr',
    projectRoot: repoRoot,
    appDir: path.join(repoRoot, 'missing-app'),
    pageFile: path.join(repoRoot, 'missing-app/page.tsx'),
    requestPath: '/',
    routePath: '/',
    params: {},
  })
  const second = send('render-2', {
    type: 'ssr',
    projectRoot: repoRoot,
    appDir: path.join(repoRoot, 'missing-app'),
    pageFile: path.join(repoRoot, 'missing-app/page.tsx'),
    requestPath: '/other',
    routePath: '/other',
    params: {},
  })

  // A ping bypasses the queue, so it answers even while a slot is held.
  const duringLoad = await send('during-load', { type: 'ping' })
  assert.equal(duringLoad.pong, true)
  assert.ok(
    duringLoad.activeRequests <= 1,
    `never more than the configured slots may be active, got ${duringLoad.activeRequests}`,
  )

  // Both requests still complete — the gate queues work, it does not drop it.
  for (const settled of [await first, await second]) {
    assert.equal(settled.ok, false)
  }

  const afterDrain = await send('after-drain', { type: 'ping' })
  assert.equal(afterDrain.activeRequests, 0, 'every slot must be released')
  assert.equal(afterDrain.queuedRequests, 0, 'the queue must drain')
})

test('does not coalesce SSR requests with different observable headers', async (t) => {
  const appDir = await mkdtemp(path.join(fixtureWorkspace, 'header-coalescing-'))
  const pageFile = path.join(appDir, 'page.tsx')
  await writeFile(
    pageFile,
    `import { headers } from 'ruvyxa/server'
export default function Page() {
  const current = headers()
  return <main>{current.get('accept-language')}|{current.get('x-tenant')}|{current.get('x-repeat')}</main>
}
`,
  )

  const { request } = startWorker(t, [appDir])
  const base = {
    type: 'ssr',
    projectRoot: fixtureWorkspace,
    appDir,
    pageFile,
    requestPath: '/request-context?view=full',
    routePath: '/request-context',
    method: 'GET',
    params: {},
  }

  // Both requests enter the worker before the cold compile completes. Before
  // the fix they shared a render because only cookie/authorization contributed
  // to the key, so the second response contained the first request's values.
  const [english, thai] = await Promise.all([
    request({
      ...base,
      headerPairs: [
        ['Accept-Language', 'en-A'],
        ['X-Tenant', 'tenant-a'],
        ['X-Repeat', 'one-a'],
        ['x-repeat', 'two-a'],
      ],
    }),
    request({
      ...base,
      headerPairs: [
        ['accept-language', 'th-B'],
        ['x-tenant', 'tenant-b'],
        ['x-repeat', 'one-b'],
        ['X-Repeat', 'two-b'],
      ],
    }),
  ])

  assert.equal(english.ok, true, english.message)
  assert.equal(thai.ok, true, thai.message)
  for (const value of ['en-A', 'tenant-a', 'one-a, two-a'])
    assert.match(english.html, new RegExp(value))
  for (const value of ['th-B', 'tenant-b', 'one-b, two-b'])
    assert.match(thai.html, new RegExp(value))
  assert.doesNotMatch(thai.html, /tenant-a/)
})

test('keeps the query target in SSR context and coalescing identity', async (t) => {
  const appDir = await mkdtemp(path.join(fixtureWorkspace, 'query-coalescing-'))
  const pageFile = path.join(appDir, 'page.tsx')
  await writeFile(
    pageFile,
    `export default function Page() {
  const context = (globalThis as any).__RUVYXA_REQUEST_CONTEXT__.current()
  return <main>{context.url}</main>
}
`,
  )

  const { request } = startWorker(t, [appDir])
  const base = {
    type: 'ssr',
    projectRoot: fixtureWorkspace,
    appDir,
    pageFile,
    requestPath: '/search',
    routePath: '/search',
    method: 'GET',
    params: {},
    headerPairs: [],
  }
  const [first, second] = await Promise.all([
    request({ ...base, requestTarget: '/search?q=first' }),
    request({ ...base, requestTarget: '/search?q=second' }),
  ])

  assert.equal(first.ok, true, first.message)
  assert.equal(second.ok, true, second.message)
  assert.match(first.html, /\/search\?q=first/)
  assert.match(second.html, /\/search\?q=second/)
})

test('reports the source graph used by server and client route bundles', async (t) => {
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'hmr-inputs-'))
  const appDir = path.join(projectRoot, 'app')
  const pageFile = path.join(appDir, 'page.tsx')
  const sharedFile = path.join(projectRoot, 'lib/shared.ts')
  await mkdir(appDir, { recursive: true })
  await mkdir(path.dirname(sharedFile), { recursive: true })
  await writeFile(sharedFile, "export const label = 'tracked'\n")
  await writeFile(
    pageFile,
    "import { label } from '../lib/shared.js'\nexport default function Page() { return <main>{label}</main> }\n",
  )
  const { request } = startWorker(t, [projectRoot])
  const base = {
    projectRoot,
    appDir,
    pageFile,
    requestPath: '/',
    routePath: '/',
    params: {},
  }

  const server = await request({ ...base, type: 'ssr', method: 'GET', headerPairs: [] })
  const client = await request({ ...base, type: 'client' })

  for (const [kind, response] of [
    ['server', server],
    ['client', client],
  ]) {
    assert.equal(response.ok, true, response.message)
    assert.ok(Array.isArray(response.inputs), `${kind} bundle did not report its inputs`)
    assert.ok(
      response.inputs.some((input) => path.resolve(input) === path.resolve(pageFile)),
      `${kind} inputs omitted the route entry`,
    )
    assert.ok(
      response.inputs.some((input) => path.resolve(input) === path.resolve(sharedFile)),
      `${kind} inputs omitted a transitive dependency`,
    )
  }
})

test('rejects overload once the bounded admission queue is full', async (t) => {
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'admission-test-'))
  const appDir = path.join(projectRoot, 'app/api/slow')
  const routeFile = path.join(appDir, 'route.ts')
  await mkdir(appDir, { recursive: true })
  await writeFile(
    routeFile,
    `export async function GET() {
      await new Promise((resolve) => setTimeout(resolve, 250))
      return Response.json({ ok: true })
    }\n`,
  )

  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    env: {
      ...process.env,
      RUVYXA_WORKER_MAX_CONCURRENCY: '1',
      RUVYXA_WORKER_MAX_QUEUE: '1',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const lines = createInterface({ input: worker.stdout })
  const pending = new Map()
  lines.on('line', (line) => {
    const message = JSON.parse(line)
    pending.get(message.id)?.(message)
    pending.delete(message.id)
  })
  const send = (id) => {
    const settled = new Promise((resolve) => pending.set(id, resolve))
    worker.stdin.write(
      `${JSON.stringify({
        id,
        type: 'api',
        projectRoot,
        routeFile,
        method: 'GET',
        requestPath: '/api/slow',
        headers: {},
        params: {},
      })}\n`,
    )
    return settled
  }

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

  const active = send('active')
  const queued = send('queued')
  const rejected = await send('rejected')

  assert.equal(rejected.ok, false)
  assert.equal(rejected.code, 'RUV1705')
  assert.match(rejected.message, /queue is full/)
  assert.equal((await active).ok, true)
  assert.equal((await queued).ok, true)

  const afterDrain = await new Promise((resolve) => {
    pending.set('after-drain', resolve)
    worker.stdin.write(`${JSON.stringify({ id: 'after-drain', type: 'ping' })}\n`)
  })
  assert.equal(afterDrain.maxQueuedRequests, 1)
  assert.equal(afterDrain.rejectedRequests, 1)
  assert.equal(afterDrain.activeRequests, 0)
  assert.equal(afterDrain.queuedRequests, 0)
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
  assert.ok(
    first.inputs.some((input) => path.resolve(input) === path.resolve(utilityFile)),
    'API response inputs omitted an imported utility',
  )
  assert.match(first.inputsVersion, /^[a-f0-9]{16}$/)

  const cached = await request({ ...apiRequest, knownInputsVersion: first.inputsVersion })
  assert.equal(cached.inputsVersion, first.inputsVersion)
  assert.equal(Object.hasOwn(cached, 'inputs'), false)

  await writeFile(utilityFile, `export const value = 'second'\n`)
  const invalidation = await request({ type: 'invalidate', paths: [utilityFile] })
  assert.equal(invalidation.ok, true)
  assert.equal(invalidation.invalidated, 1)

  const second = await request({ ...apiRequest, knownInputsVersion: first.inputsVersion })
  assert.equal(second.ok, true)
  assert.deepEqual(JSON.parse(second.body), { value: 'second' })
  assert.equal(second.inputsVersion, first.inputsVersion)
  assert.equal(Object.hasOwn(second, 'inputs'), false)

  const extraFile = path.join(projectRoot, 'lib/extra.ts')
  await writeFile(extraFile, "export const suffix = '!'\n")
  await writeFile(
    routeFile,
    `import { value } from '../../../lib/value.js'
import { suffix } from '../../../lib/extra.js'
export function GET() { return Response.json({ value: value + suffix }) }
`,
  )
  const routeInvalidation = await request({ type: 'invalidate', paths: [routeFile] })
  assert.equal(routeInvalidation.invalidated, 1)
  const changedGraph = await request({
    ...apiRequest,
    knownInputsVersion: first.inputsVersion,
  })
  assert.notEqual(changedGraph.inputsVersion, first.inputsVersion)
  assert.ok(changedGraph.inputs.some((input) => path.resolve(input) === path.resolve(extraFile)))
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

test('reports action transitive inputs once per host-owned graph version', async (t) => {
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'action-inputs-'))
  const actionFile = path.join(projectRoot, 'app/account/action.ts')
  const utilityFile = path.join(projectRoot, 'lib/message.ts')
  await mkdir(path.dirname(actionFile), { recursive: true })
  await mkdir(path.dirname(utilityFile), { recursive: true })
  await writeFile(utilityFile, "export const message = 'tracked action input'\n")
  await writeFile(
    actionFile,
    `import { action } from 'ruvyxa/server'
import { message } from '../../lib/message.js'
export const inspect = action.handler(async () => ({ message }))
`,
  )

  const { request } = startWorker(t, [projectRoot])
  const actionRequest = {
    type: 'action',
    projectRoot,
    actionFile,
    actionName: 'inspect',
    payloadJson: '{}',
    contentType: 'application/json',
    requestPath: '/account',
    headerPairs: [],
  }
  const first = await request(actionRequest)
  assert.equal(first.ok, true, first.message)
  assert.match(first.inputsVersion, /^[a-f0-9]{16}$/)
  assert.ok(first.inputs.some((input) => path.resolve(input) === path.resolve(utilityFile)))

  const cached = await request({
    ...actionRequest,
    knownInputsVersion: first.inputsVersion,
  })
  assert.equal(cached.inputsVersion, first.inputsVersion)
  assert.equal(Object.hasOwn(cached, 'inputs'), false)
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

test('an unchanged rebuild reuses its module URL instead of retaining a new graph', async (t) => {
  // Node's ESM loader never releases a module URL. A rebuild that emits
  // byte-identical output must therefore reuse its previous URL: busting the
  // cache with a monotonic token retained one whole module graph per rebuild,
  // and the file watcher rebuilds on every save.
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'module-url-test-'))
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

  const { request } = startWorker(t, [projectRoot])

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
  const afterFirst = await request({ type: 'ping' })
  assert.equal(afterFirst.retainedModuleUrls, 1)

  // Rebuild without changing anything the bundle depends on. The bundle cache
  // is dropped, the compiler re-runs, and the emitted code is identical.
  const untouched = await request({ type: 'invalidate', paths: [utilityFile] })
  assert.equal(untouched.ok, true)
  const repeat = await request(apiRequest)
  assert.equal(repeat.ok, true)
  assert.deepEqual(JSON.parse(repeat.body), { value: 'first' })
  const afterRepeat = await request({ type: 'ping' })
  assert.equal(
    afterRepeat.retainedModuleUrls,
    1,
    'an identical rebuild must not register a second module URL',
  )

  // A real source change must still reload: correctness comes before the leak.
  await writeFile(utilityFile, `export const value = 'second'\n`)
  await request({ type: 'invalidate', paths: [utilityFile] })
  const changed = await request(apiRequest)
  assert.equal(changed.ok, true)
  assert.deepEqual(JSON.parse(changed.body), { value: 'second' })
  const afterChange = await request({ type: 'ping' })
  assert.equal(afterChange.retainedModuleUrls, 2, 'changed output must load under a new module URL')
})

test('a cancelled streaming request stops producing and releases its slot', async (t) => {
  // The host removes its own bookkeeping when a client disconnects mid-stream,
  // which told the worker nothing at all: the worker's stream loop is bounded
  // only by *idle* time between chunks, and an SSE route is never idle. The
  // process kept producing forever while the pool counted it as idle and routed
  // more work onto it.
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'cancel-stream-'))
  const appDir = path.join(projectRoot, 'app/api/forever')
  const routeFile = path.join(appDir, 'route.ts')
  await mkdir(appDir, { recursive: true })
  // Deliberately ignores `request.signal`: the route is the shape that caused
  // the finding, so the stop has to come from the worker cancelling the reader.
  await writeFile(
    routeFile,
    `export function GET() {
  let timer
  const stream = new ReadableStream({
    start(controller) {
      const encoder = new TextEncoder()
      timer = setInterval(() => controller.enqueue(encoder.encode('tick')), 10)
    },
    cancel() {
      clearInterval(timer)
    },
  })
  return new Response(stream, { headers: { 'content-type': 'text/event-stream' } })
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
    await rm(projectRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  })

  const frames = []
  const waiters = []
  lines.on('line', (line) => {
    frames.push(JSON.parse(line))
    for (const waiter of waiters.splice(0)) waiter()
  })
  const waitFor = async (predicate, what) => {
    const deadline = Date.now() + 10_000
    while (!predicate()) {
      if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
      await new Promise((resolve) => {
        waiters.push(resolve)
        setTimeout(resolve, 50)
      })
    }
  }
  const framesFor = (id) => frames.filter((frame) => frame.id === id)

  worker.stdin.write(
    `${JSON.stringify({
      id: 'forever',
      type: 'api',
      projectRoot,
      routeFile,
      method: 'GET',
      requestPath: '/api/forever',
      headers: {},
      params: {},
      streamResponse: true,
    })}\n`,
  )

  await waitFor(
    () => framesFor('forever').filter((frame) => frame.frame === 'api-chunk').length >= 2,
    'the stream to start producing',
  )

  worker.stdin.write(`${JSON.stringify({ type: 'cancel', id: 'forever' })}\n`)

  await waitFor(
    () => framesFor('forever').some((frame) => frame.frame === 'api-error'),
    'the cancelled stream to end',
  )
  const terminal = framesFor('forever').at(-1)
  assert.equal(terminal.frame, 'api-error')
  assert.equal(terminal.code, 'RUV1704')

  // Nothing may follow the terminal frame: an aborted producer that keeps
  // enqueuing is the worker-exhaustion half of the finding.
  const settled = framesFor('forever').length
  await new Promise((resolve) => setTimeout(resolve, 250))
  assert.equal(framesFor('forever').length, settled, 'a cancelled stream must stop producing')

  worker.stdin.write(`${JSON.stringify({ id: 'after-cancel', type: 'ping' })}\n`)
  await waitFor(() => framesFor('after-cancel').length === 1, 'the ping to answer')
  assert.equal(framesFor('after-cancel')[0].activeRequests, 0, 'the slot must be released')

  // A cancel for an id the worker already answered is a no-op, not an error
  // frame the host would read as a response to the request it names.
  worker.stdin.write(`${JSON.stringify({ type: 'cancel', id: 'forever' })}\n`)
  worker.stdin.write(`${JSON.stringify({ id: 'after-repeat', type: 'ping' })}\n`)
  await waitFor(() => framesFor('after-repeat').length === 1, 'the second ping to answer')
  assert.equal(framesFor('forever').length, settled, 'a repeated cancel must answer nothing')
})

test('an API route handler receives a signal the host can abort', async (t) => {
  // The `Request` a route handler received carried no signal at all, so a route
  // written to observe a disconnect — a long poll, an outbound `fetch` it wants
  // to abort with the caller — could not. This is the observable behaviour
  // change: a handler that inspects `request.signal` now sees a real one.
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'cancel-signal-'))
  const appDir = path.join(projectRoot, 'app/api/wait')
  const routeFile = path.join(appDir, 'route.ts')
  await mkdir(appDir, { recursive: true })
  await writeFile(
    routeFile,
    `export function GET({ request }) {
  if (request.signal.aborted) return Response.json({ aborted: true })
  return new Promise((resolve) => {
    request.signal.addEventListener('abort', () => resolve(Response.json({ aborted: true })))
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
    await rm(projectRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  })

  const waitFrame = (id, what) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), 10_000)
      lines.on('line', (line) => {
        const frame = JSON.parse(line)
        if (frame.id !== id) return
        clearTimeout(timer)
        resolve(frame)
      })
    })
  const answered = waitFrame('wait', 'the aborted handler')
  const admitted = waitFrame('admitted', 'the bookkeeping ping')

  worker.stdin.write(
    `${JSON.stringify({
      id: 'wait',
      type: 'api',
      projectRoot,
      routeFile,
      method: 'GET',
      requestPath: '/api/wait',
      headers: {},
      params: {},
    })}\n`,
  )
  // Lines are read in order, so by the time this ping has been answered the
  // request above has been admitted and is running. Cancelling before that
  // would be withdrawn from the queue instead — correct, but a different path
  // from the one this test is about.
  worker.stdin.write(`${JSON.stringify({ id: 'admitted', type: 'ping' })}\n`)
  await admitted
  worker.stdin.write(`${JSON.stringify({ type: 'cancel', id: 'wait' })}\n`)

  const frame = await answered
  assert.equal(frame.ok, true, frame.message)
  assert.deepEqual(JSON.parse(frame.body), { aborted: true })
})

test('a change inside a directory input invalidates the bundle that globbed it', async (t) => {
  // `import.meta.glob` records its watch *root* — a directory — as a bundle
  // input, so the only thing that can invalidate on a new or removed match is
  // the directory-prefix branch of `invalidateBundleCache`. That branch was
  // unreachable: `statSync` was never imported, so classifying an input as a
  // directory threw `ReferenceError` inside a `try` whose `catch` returned
  // `false`, every input was recorded as a file, and adding a post to a globbed
  // directory left `ruvyxa dev` serving the pre-edit render.
  const projectRoot = await mkdtemp(path.join(fixtureWorkspace, 'glob-dir-test-'))
  const appDir = path.join(projectRoot, 'app/api/posts')
  const routeFile = path.join(appDir, 'route.ts')
  const contentDir = path.join(projectRoot, 'content')
  await mkdir(appDir, { recursive: true })
  await mkdir(contentDir, { recursive: true })
  await writeFile(path.join(contentDir, 'first.ts'), `export const title = 'first'\n`)
  await writeFile(
    routeFile,
    `const posts = import.meta.glob('../../../content/*.ts', { eager: true })
export function GET() { return Response.json({ titles: Object.values(posts).map((m) => m.title).sort() }) }
`,
  )

  const { request } = startWorker(t, [projectRoot])

  const apiRequest = {
    type: 'api',
    projectRoot,
    routeFile,
    method: 'GET',
    requestPath: '/api/posts',
    headers: {},
    params: {},
  }

  const first = await request(apiRequest)
  assert.equal(first.ok, true, first.message)
  assert.deepEqual(JSON.parse(first.body), { titles: ['first'] })
  assert.ok(
    first.inputs.some((input) => path.resolve(input) === path.resolve(contentDir)),
    'the glob watch root must be reported as a bundle input',
  )

  // A brand-new match. Nothing names the directory itself, so only the
  // directory-prefix branch can connect this path to the cached bundle.
  const addedFile = path.join(contentDir, 'second.ts')
  await writeFile(addedFile, `export const title = 'second'\n`)
  const invalidation = await request({ type: 'invalidate', paths: [addedFile] })
  assert.equal(invalidation.ok, true)
  assert.equal(
    invalidation.invalidated,
    1,
    'a change under a recorded directory input must invalidate the bundle',
  )

  const second = await request(apiRequest)
  assert.equal(second.ok, true, second.message)
  assert.deepEqual(JSON.parse(second.body), { titles: ['first', 'second'] })
})

// The worker spends the shutdown window; the Rust host waits it out plus a
// margin before killing the process. They were once written independently — 5 s
// here against a 2 s host wait — so a shutdown reaching a busy worker always
// took the kill branch and the window could never be spent. The host normalizes
// the value into `RUVYXA_WORKER_SHUTDOWN_MS`, so the worker has to honour it.
test('the shutdown grace comes from the environment the host normalizes', async (t) => {
  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    env: { ...process.env, RUVYXA_WORKER_SHUTDOWN_MS: '12000' },
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

  const configured = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('worker ping timed out')), 10_000)
    lines.once('line', (line) => {
      clearTimeout(timer)
      resolve(JSON.parse(line))
    })
    worker.stdin.write(`${JSON.stringify({ id: 'shutdown-grace', type: 'ping' })}
`)
  })

  assert.equal(configured.ok, true)
  assert.equal(configured.workerShutdownGraceMs, 12_000)
})

// An idle worker must not sit out the grace: the host waits for the process to
// exit, so a worker holding nothing has to go as soon as its stdin closes. This
// is why raising the grace costs an ordinary Ctrl-C nothing.
test('an idle worker exits as soon as its stdin closes', async (t) => {
  const worker = spawn(process.execPath, [workerScript], {
    cwd: repoRoot,
    // Far longer than this test will wait, so a worker that waits out the
    // window instead of exiting fails rather than passing slowly.
    env: { ...process.env, RUVYXA_WORKER_SHUTDOWN_MS: '600000' },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const lines = createInterface({ input: worker.stdout })
  t.after(() => {
    lines.close()
    if (worker.exitCode === null) worker.kill()
  })

  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('worker ping timed out')), 10_000)
    lines.once('line', () => {
      clearTimeout(timer)
      resolve()
    })
    worker.stdin.write(`${JSON.stringify({ id: 'idle-exit', type: 'ping' })}
`)
  })

  worker.stdin.end()
  const exited = await Promise.race([
    new Promise((resolve) => worker.once('exit', () => resolve('exited'))),
    new Promise((resolve) => setTimeout(() => resolve('still running'), 10_000)),
  ])
  assert.equal(exited, 'exited')
})
