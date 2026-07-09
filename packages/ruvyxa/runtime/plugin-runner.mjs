import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { cacheFileName, compileBundle, runtimeAliases, toImportPath } from './compiler.mjs'

const [projectRootArg, hook] = process.argv.slice(2)

if (!projectRootArg || !hook) {
  fail('RUV1701', 'Plugin runner requires project root and hook arguments.')
}

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

try {
  const payload = JSON.parse(await readStdin())
  const config = await loadConfig(projectRoot)
  const plugins = normalizePlugins(config.plugins)

  if (hook === 'resolveId') {
    ok(await runResolveId(plugins, payload))
  } else if (hook === 'transform') {
    ok(await runTransform(plugins, payload))
  } else {
    fail('RUV1701', `Unknown plugin hook: ${hook}`)
  }
} catch (error) {
  fail('RUV1700', error instanceof Error ? error.message : String(error), error?.stack)
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
    platform: 'node',
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
      changed = true
    }
  }

  return changed ? { code } : null
}

async function readStdin() {
  return readFileSync(0, 'utf8')
}

function ok(result) {
  process.stdout.write(JSON.stringify({ ok: true, result }))
  process.exit(0)
}

function fail(code, message, stack) {
  process.stdout.write(JSON.stringify({ ok: false, code, message, stack }))
  process.exit(1)
}
