// Real benchmark harness: Ruvyxa vs Next.js vs Astro (minimal starters).
// Metrics: cold production build time, dev server time-to-first-200,
// prod server time-to-first-200, throughput (autocannon), client JS size.
//
// Setup (one time), from an empty working directory:
//   npm create ruvyxa@latest ruvyxa-app            (or copy templates/minimal + npm install)
//   npx create-next-app@latest next-app --ts --app --skip-install --disable-git \
//     --no-eslint --no-tailwind --no-src-dir --no-import-alias && (cd next-app && npm install)
//   npm create astro@latest astro-app -- --template minimal --no-git --yes
//   node bench-frameworks.mjs                       (BENCH_ROOT=<dir> to point elsewhere)
//
// Results in the README were produced by this script; medians of RUNS (default 3) cold runs.
import { spawn, execSync } from 'node:child_process'
import { rmSync, readdirSync, statSync, existsSync } from 'node:fs'
import path from 'node:path'

const ROOT = process.env.BENCH_ROOT ? path.resolve(process.env.BENCH_ROOT) : import.meta.dirname
const RUNS = Number(process.env.RUNS ?? 3)

const apps = {
  ruvyxa: {
    dir: path.join(ROOT, 'ruvyxa-app'),
    clean: ['.ruvyxa', 'dist'],
    build: ['npm', ['run', 'build']],
    dev: { cmd: ['npm', ['run', 'dev', '--', '--port', '4600']], url: 'http://localhost:4600/' },
    prod: { cmd: ['npm', ['run', 'start', '--', '--port', '4601']], url: 'http://localhost:4601/' },
    clientDirs: ['.ruvyxa/client', 'dist/client'],
  },
  next: {
    dir: path.join(ROOT, 'next-app'),
    clean: ['.next'],
    build: ['npm', ['run', 'build']],
    dev: { cmd: ['npm', ['run', 'dev', '--', '--port', '4610']], url: 'http://localhost:4610/' },
    prod: { cmd: ['npm', ['run', 'start', '--', '--port', '4611']], url: 'http://localhost:4611/' },
    clientDirs: ['.next/static/chunks'],
  },
  astro: {
    dir: path.join(ROOT, 'astro-app'),
    clean: ['dist', '.astro', 'node_modules/.astro', 'node_modules/.vite'],
    build: ['npm', ['run', 'build']],
    dev: {
      cmd: ['npm', ['run', 'dev', '--', '--port', '4620']],
      url: 'http://localhost:4620/',
    },
    prod: {
      cmd: ['npm', ['run', 'preview', '--', '--port', '4621']],
      url: 'http://localhost:4621/',
    },
    clientDirs: ['dist/_astro'],
  },
}

const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]

function clean(app) {
  for (const dir of app.clean) rmSync(path.join(app.dir, dir), { recursive: true, force: true })
}

function run([cmd, args], dir) {
  const started = performance.now()
  execSync([cmd, ...args].join(' '), { cwd: dir, stdio: 'pipe', shell: true })
  return performance.now() - started
}

function startServer([cmd, args], dir) {
  return spawn([cmd, ...args].join(' '), {
    cwd: dir,
    shell: true,
    stdio: 'ignore',
    detached: false,
  })
}

async function timeToFirst200(child, url, timeoutMs = 120_000) {
  const started = performance.now()
  while (performance.now() - started < timeoutMs) {
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(2000) })
      if (response.ok) {
        await response.arrayBuffer()
        return performance.now() - started
      }
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  throw new Error(`server never answered 200 at ${url}`)
}

function killTree(child, url) {
  try {
    execSync(`taskkill /F /T /PID ${child.pid}`, { stdio: 'ignore' })
  } catch {}
  // Some dev servers (Astro/Vite) detach from the npm process tree; kill by port too,
  // otherwise the next "cold start" run measures a stale server answering instantly.
  if (url) {
    const port = new URL(url).port
    try {
      execSync(
        `powershell -Command "Get-NetTCPConnection -LocalPort ${port} -State Listen -ErrorAction SilentlyContinue | ForEach-Object { taskkill /F /T /PID $_.OwningProcess }"`,
        { stdio: 'ignore' },
      )
    } catch {}
  }
}

function dirBytes(dir) {
  if (!existsSync(dir)) return 0
  let total = 0
  for (const entry of readdirSync(dir, { recursive: true })) {
    const file = path.join(dir, String(entry))
    const stats = statSync(file)
    if (stats.isFile() && /\.(js|mjs)$/.test(file)) total += stats.size
  }
  return total
}

async function throughput(url, seconds = 10, connections = 25) {
  const out = execSync(`npx --yes autocannon@8 -d ${seconds} -c ${connections} --json ${url}`, {
    stdio: ['ignore', 'pipe', 'pipe'],
    shell: true,
    maxBuffer: 64 * 1024 * 1024,
  }).toString()
  const parsed = JSON.parse(out)
  return {
    rps: parsed.requests.average,
    p50: parsed.latency.p50,
    p99: parsed.latency.p99,
  }
}

const results = {}
for (const [name, app] of Object.entries(apps)) {
  console.log(`\n=== ${name} ===`)
  const builds = []
  for (let i = 0; i < RUNS; i++) {
    clean(app)
    const ms = run(app.build, app.dir)
    builds.push(ms)
    console.log(`build[${i}] ${Math.round(ms)}ms`)
  }

  const devTimes = []
  for (let i = 0; i < RUNS; i++) {
    clean(app) // cold dev: no caches
    const child = startServer(app.dev.cmd, app.dir)
    const ms = await timeToFirst200(child, app.dev.url)
    devTimes.push(ms)
    killTree(child, app.dev.url)
    console.log(`dev-ready[${i}] ${Math.round(ms)}ms`)
    await new Promise((resolve) => setTimeout(resolve, 750))
  }

  // Rebuild once so prod uses a valid build.
  clean(app)
  run(app.build, app.dir)

  const prodTimes = []
  for (let i = 0; i < RUNS; i++) {
    const child = startServer(app.prod.cmd, app.dir)
    const ms = await timeToFirst200(child, app.prod.url)
    prodTimes.push(ms)
    if (i < RUNS - 1) {
      killTree(child, app.prod.url)
      await new Promise((resolve) => setTimeout(resolve, 750))
    } else {
      // keep the last instance alive for the load test
      console.log(`prod-ready[${i}] ${Math.round(ms)}ms (load test starting)`)
      const load = await throughput(app.prod.url)
      results[name] = { builds, devTimes, prodTimes, load }
      killTree(child, app.prod.url)
    }
    console.log(`prod-ready[${i}] ${Math.round(ms)}ms`)
  }

  const clientBytes = app.clientDirs.reduce(
    (sum, dir) => sum + dirBytes(path.join(app.dir, dir)),
    0,
  )
  results[name].clientBytes = clientBytes
  console.log(`client JS ${Math.round(clientBytes / 1024)}KB`)
}

console.log('\n=== SUMMARY (median of ' + RUNS + ') ===')
for (const [name, r] of Object.entries(results)) {
  console.log(
    JSON.stringify({
      name,
      buildMs: Math.round(median(r.builds)),
      devReadyMs: Math.round(median(r.devTimes)),
      prodReadyMs: Math.round(median(r.prodTimes)),
      rps: Math.round(r.load.rps),
      p50: r.load.p50,
      p99: r.load.p99,
      clientKB: Math.round(r.clientBytes / 1024),
    }),
  )
}
