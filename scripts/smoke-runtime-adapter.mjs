import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import path from 'node:path'

const [runtime, deploymentDirectory, portArg] = process.argv.slice(2)
if (!['node', 'bun', 'deno'].includes(runtime) || !deploymentDirectory || !portArg) {
  console.error(
    'usage: node scripts/smoke-runtime-adapter.mjs <node|bun|deno> <deployment-dir> <port>',
  )
  process.exit(2)
}

const port = Number(portArg)
const base = `http://127.0.0.1:${port}`
const entry = path.join('server', 'index.mjs')
const args = runtime === 'deno' ? ['run', '-A', '--no-prompt', entry] : [entry]
const child = spawn(runtimeExecutable(runtime), args, {
  cwd: path.resolve(deploymentDirectory),
  env: { ...process.env, HOST: '127.0.0.1', PORT: String(port) },
  stdio: ['ignore', 'pipe', 'pipe'],
})

let output = ''
child.stdout.on('data', (chunk) => (output += chunk))
child.stderr.on('data', (chunk) => (output += chunk))

/**
 * What the emitted server has to get right, checked against the real runtime.
 *
 * One endpoint was not enough. The three transports differ in how a request
 * reaches the handler and how a file becomes a response body, so the cases
 * below are chosen for the decisions that differ: the publish directory, the
 * generated route registry, the pre-rendered read, and the headers the shared
 * core attaches. A Bun range bug that served a whole file for a sliced
 * `BunFile` got through a health check without a mark on it.
 */
const checks = [
  {
    name: 'GET /api/health reaches the generated route registry',
    path: '/api/health',
    assert: (response, body) => {
      if (response.status !== 200) return `status ${response.status}`
      if (!body.includes('Ruvyxa')) return `body ${body.slice(0, 120)}`
      return null
    },
  },
  {
    name: 'GET / serves the pre-rendered page',
    path: '/',
    assert: (response, body) => {
      if (response.status !== 200) return `status ${response.status}`
      if (!body.includes('data-smoke="page"')) return `body ${body.slice(0, 120)}`
      return null
    },
  },
  {
    name: 'GET /cached serves an ISR route',
    path: '/cached',
    assert: (response, body) => {
      if (response.status !== 200) return `status ${response.status}`
      if (!body.includes('data-smoke="cached"')) return `body ${body.slice(0, 120)}`
      return null
    },
  },
  {
    name: 'GET /smoke.svg serves a public asset with its cache policy',
    path: '/smoke.svg',
    assert: (response) => {
      if (response.status !== 200) return `status ${response.status}`
      const type = response.headers.get('content-type')
      if (type !== 'image/svg+xml') return `content-type ${type}`
      const cache = response.headers.get('cache-control')
      if (cache !== 'public, max-age=3600, must-revalidate') return `cache-control ${cache}`
      return null
    },
  },
  {
    name: 'a served page carries the security defaults',
    path: '/',
    assert: (response) => {
      const nosniff = response.headers.get('x-content-type-options')
      if (nosniff !== 'nosniff') return `x-content-type-options ${nosniff}`
      const frame = response.headers.get('x-frame-options')
      if (frame !== 'DENY') return `x-frame-options ${frame}`
      return null
    },
  },
  {
    name: 'an unknown path is a 404 rather than a crash',
    path: '/definitely-not-a-route',
    assert: (response) => (response.status === 404 ? null : `status ${response.status}`),
  },
]

try {
  await waitUntilServing()
  for (const check of checks) {
    const response = await fetch(base + check.path)
    const body = await response.text()
    const failure = check.assert(response, body)
    if (failure) throw new Error(`${runtime}: ${check.name} — ${failure}\n${output}`)
    console.log(`[ok] ${runtime} · ${check.name}`)
  }
  console.log(`[ok] ${runtime} deployment artifact passed ${checks.length} checks`)
} finally {
  child.kill()
  await Promise.race([
    new Promise((resolve) => child.once('exit', resolve)),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ])
}

/** Poll until the server answers, so a slow cold start is not read as a failure. */
async function waitUntilServing() {
  const deadline = Date.now() + 15_000
  let lastError
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`server exited with ${child.exitCode}: ${output}`)
    try {
      const response = await fetch(`${base}/api/health`)
      await response.arrayBuffer()
      return
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 200))
    }
  }
  throw new Error(`${runtime} server never answered: ${lastError}\n${output}`)
}

/** Real executables a Windows command shim of `name` could be standing in front of. */
function windowsCandidates(name, directory) {
  if (name === 'node') return [path.join(directory, 'node.exe')]
  if (name === 'bun') {
    return [path.join(directory, 'bun.exe'), path.join(directory, 'node_modules/bun/bin/bun.exe')]
  }
  return [
    path.join(directory, 'deno.exe'),
    path.join(directory, 'node_modules/deno/deno.exe'),
    path.join(directory, 'node_modules/deno/node_modules/@deno/win32-x64/deno.exe'),
  ]
}

function runtimeExecutable(name) {
  if (process.platform !== 'win32') return name
  const pathValue = Object.entries(process.env).find(([key]) => key.toLowerCase() === 'path')?.[1]
  for (const directory of (pathValue ?? '').split(path.delimiter)) {
    const executable = windowsCandidates(name, directory).find(existsSync)
    if (executable) return executable
  }
  throw new Error(`could not resolve the ${name} executable behind its Windows command shim`)
}
