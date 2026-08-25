#!/usr/bin/env node
/**
 * What a deployed Ruvyxa server does under more than one request at a time.
 *
 * `scripts/smoke-runtime-adapter.mjs` proves an artifact answers correctly; it
 * asks one question at a time and never asks twice. Everything that only shows
 * up under load is therefore unmeasured: how long the first request waits for a
 * cold process, whether latency holds when requests overlap, whether anything
 * leaks across a few thousand renders, and whether a per-request render costs
 * what a cached one does.
 *
 * **Measured, not gated.** Every number here is a property of the machine it
 * ran on — core count, free memory, and what else the CI runner is doing — so a
 * threshold asserted against it would fail for reasons that have nothing to do
 * with the code. The one thing it *does* fail on is a non-200, because a
 * request that errors under concurrency and succeeds alone is a defect whatever
 * the hardware.
 *
 * usage: node scripts/load-probe.mjs <deployment-dir> <port> [--requests N] [--concurrency N]
 */
import { spawn } from 'node:child_process'
import { execFileSync } from 'node:child_process'
import { Agent, request as httpRequest } from 'node:http'
import path from 'node:path'

const argv = process.argv.slice(2)
const [deploymentDirectory, portArg] = argv
const flag = (name, fallback) => {
  const index = argv.indexOf(`--${name}`)
  return index === -1 ? fallback : Number(argv[index + 1])
}
if (!deploymentDirectory || !portArg) {
  console.error(
    'usage: node scripts/load-probe.mjs <deployment-dir> <port> [--requests N] [--concurrency N]',
  )
  process.exit(2)
}

const port = Number(portArg)
const REQUESTS = flag('requests', 2_000)
const CONCURRENCY = flag('concurrency', 50)
const COLD_STARTS = flag('cold-starts', 5)
const ROUNDS = flag('rounds', 1)

/**
 * The routes worth separating, because each is a different amount of work.
 *
 * A pre-rendered document is a file read; an ISR hit is a cache read; a
 * per-request render runs the application. Reporting one number across all
 * three hides which of them is the cost.
 */
const ROUTES = [
  { path: '/', label: 'pre-rendered' },
  { path: '/isr-page', label: 'isr (cached)' },
  { path: '/request', label: 'per-request render' },
  { path: '/api/health', label: 'api route' },
]

let child = null

// One pool, reused: opening a fresh socket per request measures the kernel's
// accept path rather than the server's render path, and a serverless platform
// keeps connections alive in front of the function anyway.
const keepAlive = new Agent({ keepAlive: true, maxSockets: CONCURRENCY })

/** One request, timed, with the body drained so the socket is reusable. */
function timed(pathname) {
  return new Promise((resolve) => {
    const started = process.hrtime.bigint()
    const call = httpRequest(
      { host: '127.0.0.1', port, path: pathname, agent: keepAlive },
      (response) => {
        response.resume()
        response.on('end', () =>
          resolve({
            ms: Number(process.hrtime.bigint() - started) / 1e6,
            status: response.statusCode,
          }),
        )
      },
    )
    call.on('error', (error) => resolve({ ms: 0, status: 0, error: error.message }))
    call.end()
  })
}

function percentile(sorted, fraction) {
  if (sorted.length === 0) return 0
  const index = Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))
  return sorted[index]
}

/** Resident memory of the server process, in MB. */
function residentMb(pid) {
  try {
    if (process.platform === 'win32') {
      const out = execFileSync('powershell', [
        '-NoProfile',
        '-Command',
        `(Get-Process -Id ${pid}).WorkingSet64`,
      ])
      return Number(String(out).trim()) / 1024 / 1024
    }
    const out = execFileSync('ps', ['-o', 'rss=', '-p', String(pid)])
    return Number(String(out).trim()) / 1024
  } catch {
    return NaN
  }
}

function startServer() {
  child = spawn(process.execPath, [path.join('server', 'index.mjs')], {
    cwd: path.resolve(deploymentDirectory),
    env: { ...process.env, HOST: '127.0.0.1', PORT: String(port) },
    stdio: ['ignore', 'ignore', 'ignore'],
  })
}

async function stopServer() {
  if (!child) return
  child.kill()
  await Promise.race([
    new Promise((resolve) => child.once('exit', resolve)),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ])
  child = null
}

/** Spawn, ask once, stop. The number a serverless platform pays per cold instance. */
async function measureColdStart() {
  const samples = []
  for (let run = 0; run < COLD_STARTS; run += 1) {
    const started = process.hrtime.bigint()
    startServer()
    for (;;) {
      const answer = await timed('/api/health')
      if (answer.status === 200) break
      if (child.exitCode !== null) throw new Error('the server exited during cold start')
      await new Promise((resolve) => setTimeout(resolve, 5))
    }
    samples.push(Number(process.hrtime.bigint() - started) / 1e6)
    await stopServer()
  }
  samples.sort((left, right) => left - right)
  return samples
}

/** `count` requests, at most `CONCURRENCY` in flight, reported by percentile. */
async function measureRoute(pathname, count) {
  const results = []
  let issued = 0
  const worker = async () => {
    for (;;) {
      if (issued >= count) return
      issued += 1
      results.push(await timed(pathname))
    }
  }
  const started = process.hrtime.bigint()
  await Promise.all(Array.from({ length: CONCURRENCY }, worker))
  const elapsed = Number(process.hrtime.bigint() - started) / 1e6
  const failures = results.filter((result) => result.status !== 200)
  const times = results.map((result) => result.ms).sort((left, right) => left - right)
  return {
    rps: (results.length / elapsed) * 1000,
    p50: percentile(times, 0.5),
    p95: percentile(times, 0.95),
    p99: percentile(times, 0.99),
    max: times.at(-1) ?? 0,
    failures,
  }
}

const fixed = (value) => (Number.isFinite(value) ? value.toFixed(1) : '—')

try {
  console.log(`load probe · ${deploymentDirectory}`)
  console.log(`  ${REQUESTS} requests per route, ${CONCURRENCY} in flight, node ${process.version}`)
  console.log()

  const cold = await measureColdStart()
  console.log(
    `cold start  ${COLD_STARTS} spawns · min ${fixed(cold[0])}ms · median ${fixed(
      percentile(cold, 0.5),
    )}ms · max ${fixed(cold.at(-1))}ms`,
  )

  startServer()
  for (;;) {
    const answer = await timed('/api/health')
    if (answer.status === 200) break
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
  // Warm first: the numbers below are meant to describe a running server, not
  // the one-off cost of compiling and importing the route registry, which the
  // cold-start figure above already reports on its own.
  await measureRoute('/', 200)
  const before = residentMb(child.pid)

  console.log()
  console.log('route                  rps        p50      p95      p99      max      errors')
  let anyFailure = null
  // Sampled per round rather than once at the end, because a single delta
  // cannot tell a leak from a heap V8 has simply not bothered to collect. Two
  // points on a line look identical for both; the shape of the curve does not.
  const memory = [before]
  for (let round = 0; round < ROUNDS; round += 1) {
    for (const route of ROUTES) {
      const result = await measureRoute(route.path, REQUESTS)
      if (result.failures.length > 0) anyFailure ??= { route, result }
      if (round > 0) continue
      console.log(
        `${route.label.padEnd(20)}  ${fixed(result.rps).padStart(8)}  ${fixed(result.p50).padStart(
          7,
        )}  ${fixed(result.p95).padStart(7)}  ${fixed(result.p99).padStart(7)}  ${fixed(
          result.max,
        ).padStart(7)}  ${String(result.failures.length).padStart(6)}`,
      )
    }
    memory.push(residentMb(child.pid))
  }

  const perRound = REQUESTS * ROUTES.length
  console.log()
  console.log(
    `resident memory  ${memory
      .map((value, index) => `${index * perRound}req ${fixed(value)}MB`)
      .join('  ·  ')}`,
  )
  const growth = memory.slice(1).map((value, index) => value - memory[index])
  if (growth.length > 1) {
    const first = growth[0]
    const last = growth.at(-1)
    console.log(
      `  growth per round  ${growth.map((value) => fixed(value) + 'MB').join(' → ')}  ` +
        `(${last < first * 0.5 ? 'flattening — collected, not retained' : 'steady — retained'})`,
    )
  }

  if (anyFailure) {
    const { route, result } = anyFailure
    throw new Error(
      `${route.path}: ${result.failures.length} of ${REQUESTS} requests did not answer 200 ` +
        `(first: status ${result.failures[0].status}${
          result.failures[0].error ? `, ${result.failures[0].error}` : ''
        })`,
    )
  }
} finally {
  await stopServer()
}
