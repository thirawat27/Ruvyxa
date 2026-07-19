import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundle,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'

const [projectRootArg, mode] = process.argv.slice(2)

if (!projectRootArg || !mode) {
  writeResponse(
    failure('RUV1701', 'Plugin runner requires project root and hook or mode arguments.'),
  )
  process.exit(1)
}

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

try {
  const config = await loadConfig(projectRoot)
  const plugins = normalizePlugins(config.plugins)

  if (mode === '--persistent') {
    await runPersistent(plugins)
  } else {
    const payload = JSON.parse(await readStdin())
    const response = await handleHook(plugins, mode, payload)
    writeResponse(response)
    if (!response.ok) process.exitCode = 1
  }
} catch (error) {
  writeResponse(
    failure('RUV1700', error instanceof Error ? error.message : String(error), error?.stack),
    mode === '--persistent',
  )
  process.exitCode = 1
}

async function runPersistent(plugins) {
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })
  for await (const line of lines) {
    if (!line.trim()) continue

    let response
    try {
      const payload = JSON.parse(line)
      response = await handleHook(plugins, payload.hook, payload)
    } catch (error) {
      response = failure(
        'RUV1700',
        error instanceof Error ? error.message : String(error),
        error?.stack,
      )
    }
    writeResponse(response, true)
  }
}

async function handleHook(plugins, hook, payload) {
  if (hook === 'resolveId') {
    return success(await runResolveId(plugins, payload))
  }
  if (hook === 'transform') {
    return success(await runTransform(plugins, payload))
  }
  return failure('RUV1701', `Unknown plugin hook: ${hook}`)
}

async function loadConfig(root) {
  const configFile = findConfig(root)
  if (!configFile) return {}

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    root,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile, 'plugin-runner'], 'mjs'),
  )

  await compileBundle({
    projectRoot: root,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:plugin-config-entry.ts',
    outfile,
    platform: serverPlatform(),
    aliases: runtimeAliases(runtimeDir),
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  return mod.default ?? {}
}

function findConfig(root) {
  for (const fileName of [
    'ruvyxa.config.ts',
    'ruvyxa.config.mts',
    'ruvyxa.config.js',
    'ruvyxa.config.mjs',
  ]) {
    const file = path.join(root, fileName)
    if (existsSync(file)) return file
  }
  return null
}

function normalizePlugins(value) {
  if (!Array.isArray(value)) return []
  const plugins = value.filter(
    (plugin) => plugin && typeof plugin === 'object' && typeof plugin.name === 'string',
  )
  return [
    ...plugins.filter((plugin) => plugin.enforce === 'pre'),
    ...plugins.filter((plugin) => plugin.enforce !== 'pre' && plugin.enforce !== 'post'),
    ...plugins.filter((plugin) => plugin.enforce === 'post'),
  ]
}

async function runResolveId(plugins, payload) {
  for (const plugin of plugins) {
    if (typeof plugin.resolveId !== 'function') continue

    const result = await plugin.resolveId(payload.id, payload.importer ?? undefined)
    if (typeof result === 'string') {
      return result
    }
  }
  return null
}

async function runTransform(plugins, payload) {
  let code = String(payload.code ?? '')
  let map
  let changed = false
  const ctx = {
    environment: payload.environment ?? 'client',
  }

  for (const plugin of plugins) {
    if (typeof plugin.transform !== 'function') continue

    const result = await plugin.transform(code, String(payload.id ?? ''), ctx)
    if (!result) continue

    if (typeof result === 'string') {
      code = result
      changed = true
      continue
    }

    if (typeof result === 'object' && typeof result.code === 'string') {
      code = result.code
      if (result.map !== undefined && result.map !== null) {
        map = typeof result.map === 'string' ? result.map : JSON.stringify(result.map)
      }
      changed = true
    }
  }

  return changed ? { code, ...(map === undefined ? {} : { map }) } : null
}

async function readStdin() {
  return readFileSync(0, 'utf8')
}

function success(result) {
  return { ok: true, result }
}

function failure(code, message, stack) {
  return { ok: false, code, message, stack }
}

function writeResponse(response, newline = false) {
  process.stdout.write(JSON.stringify(response) + (newline ? '\n' : ''))
}
