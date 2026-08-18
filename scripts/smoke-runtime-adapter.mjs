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

try {
  const deadline = Date.now() + 15_000
  let lastError
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`server exited with ${child.exitCode}: ${output}`)
    try {
      const response = await fetch(`http://127.0.0.1:${port}/api/health`)
      const body = await response.text()
      if (response.status !== 200 || !body.includes('Ruvyxa')) {
        throw new Error(`unexpected health response ${response.status}: ${body}`)
      }
      console.log(`[ok] ${runtime} deployment artifact served /api/health`)
      lastError = undefined
      break
    } catch (error) {
      lastError = error
      await new Promise((resolve) => setTimeout(resolve, 200))
    }
  }
  if (lastError) throw lastError
} finally {
  child.kill()
  await Promise.race([
    new Promise((resolve) => child.once('exit', resolve)),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ])
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
