import { existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { cacheFileName, compileBundle, runtimeAliases, toImportPath } from './compiler.mjs'

const [projectRootArg] = process.argv.slice(2)

if (!projectRootArg) {
  fail('RUV1601', 'Config renderer requires a project root argument.')
}

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

try {
  const configFile = findConfig(projectRoot)
  if (!configFile) {
    await ok({})
  }

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    projectRoot,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile], 'mjs'),
  )

  await compileBundle({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:config-entry.ts',
    outfile,
    platform: 'node',
    aliases: runtimeAliases(runtimeDir),
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  const config = mod.default ?? {}
  await ok(sanitizeConfig(config))
} catch (error) {
  await fail('RUV1600', error instanceof Error ? error.message : String(error), error?.stack)
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

function sanitizeConfig(config) {
  return {
    appDir: stringValue(config.appDir),
    outDir: stringValue(config.outDir),
    runtime: stringValue(config.runtime),
    react: booleanValue(config.react),
    server: objectValue(config.server, {
      host: stringValue(config.server?.host),
      port: numberValue(config.server?.port),
    }),
    build: objectValue(config.build, {
      minify: booleanValue(config.build?.minify),
      sourcemap: booleanValue(config.build?.sourcemap),
      treeShaking: booleanValue(config.build?.treeShaking),
      splitStrategy: stringValue(config.build?.splitStrategy),
      parallelism: numberValue(config.build?.parallelism),
      jsxRuntime: stringValue(config.build?.jsxRuntime),
      esTarget: stringValue(config.build?.esTarget),
      emitChunkManifest: booleanValue(config.build?.emitChunkManifest),
    }),
    debug: objectValue(config.debug, {
      overlay: booleanValue(config.debug?.overlay),
      traces: booleanValue(config.debug?.traces),
    }),
    security: objectValue(config.security, {
      actionBodyLimitBytes: numberValue(config.security?.actionBodyLimitBytes),
      sameOriginActions: booleanValue(config.security?.sameOriginActions),
      fetchMetadataActions: booleanValue(config.security?.fetchMetadataActions),
      securityHeaders: booleanValue(config.security?.securityHeaders),
    }),
    cache: objectValue(config.cache, {
      routeManifest: booleanValue(config.cache?.routeManifest),
      css: booleanValue(config.cache?.css),
    }),
    adapter: objectValue(config.adapter, {
      name: stringValue(config.adapter?.name),
      target: stringValue(config.adapter?.target),
    }),
    adapterOptions: safeJsonValue(config.adapterOptions),
    plugins: pluginDescriptors(config.plugins),
  }
}

function objectValue(source, value) {
  if (!source || typeof source !== 'object') return undefined
  const filtered = Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  )
  return Object.keys(filtered).length > 0 ? filtered : undefined
}

function stringValue(value) {
  return typeof value === 'string' ? value : undefined
}

function numberValue(value) {
  return Number.isFinite(value) ? value : undefined
}

function booleanValue(value) {
  return typeof value === 'boolean' ? value : undefined
}

function safeJsonValue(value) {
  if (value === undefined) return undefined
  try {
    JSON.stringify(value)
    return value
  } catch {
    return undefined
  }
}

function pluginDescriptors(value) {
  if (!Array.isArray(value)) return undefined
  const plugins = value
    .filter((plugin) => plugin && typeof plugin === 'object' && typeof plugin.name === 'string')
    .map((plugin) => ({
      name: plugin.name,
      enforce: stringValue(plugin.enforce),
      resolveId: typeof plugin.resolveId === 'function',
      transform: typeof plugin.transform === 'function',
    }))

  return plugins.length > 0 ? plugins : undefined
}

async function ok(config) {
  await writeJson({ ok: true, config })
  process.exit(0)
}

async function fail(code, message, stack) {
  await writeJson({ ok: false, code, message, stack })
  process.exit(1)
}

function writeJson(payload) {
  return new Promise((resolve, reject) => {
    process.stdout.write(JSON.stringify(payload), (error) => {
      if (error) {
        reject(error)
      } else {
        resolve()
      }
    })
  })
}
