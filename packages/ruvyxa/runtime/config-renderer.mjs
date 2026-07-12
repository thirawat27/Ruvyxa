import { existsSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundleWithMetadata,
  runtimeAliases,
  toImportPath,
} from './compiler.mjs'

const [projectRootArg] = process.argv.slice(2)

if (!projectRootArg) {
  fail('RUV1601', 'Config renderer requires a project root argument.')
}

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

try {
  const configFile = findConfig(projectRoot)
  if (!configFile) {
    await ok({}, 'no-config')
  }

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    projectRoot,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile], 'mjs'),
  )

  const bundle = await compileBundleWithMetadata({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:config-entry.ts',
    outfile,
    platform: 'node',
    aliases: runtimeAliases(runtimeDir),
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  const config = mod.default ?? {}
  await ok(await sanitizeConfig(config), bundle.dependencyHash)
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

async function sanitizeConfig(config) {
  assertKnownKeys(config, 'config', [
    'appDir',
    'outDir',
    'runtime',
    'react',
    'typescript',
    'css',
    'server',
    'build',
    'render',
    'debug',
    'image',
    'security',
    'cache',
    'middleware',
    'adapter',
    'adapterOptions',
    'plugins',
  ])
  assertKnownKeys(config.css, 'config.css', ['entries'])
  assertKnownKeys(config.server, 'config.server', ['host', 'port'])
  assertKnownKeys(config.build, 'config.build', [
    'minify',
    'map',
    'treeShake',
    'split',
    'workers',
    'jsx',
    'target',
    'manifest',
    'warm',
  ])
  assertKnownKeys(config.debug, 'config.debug', ['overlay', 'traces'])
  assertKnownKeys(config.image, 'config.image', ['optimize', 'quality', 'lossless', 'workers'])
  assertKnownKeys(config.security, 'config.security', [
    'actionLimit',
    'apiLimit',
    'actionRateLimit',
    'sameOrigin',
    'fetchMeta',
    'headers',
  ])
  assertKnownKeys(config.security?.actionRateLimit, 'config.security.actionRateLimit', [
    'max',
    'window',
  ])
  assertKnownKeys(config.cache, 'config.cache', ['routes', 'css', 'dir'])
  assertKnownKeys(config.render, 'config.render', ['strategy', 'revalidate'])
  assertKnownKeys(config.middleware, 'config.middleware', ['builtin', 'layers', 'plugins'])
  assertKnownKeys(config.middleware?.builtin, 'config.middleware.builtin', [
    'cors',
    'timing',
    'log',
    'rate',
    'headers',
  ])
  assertKnownKeys(config.middleware?.builtin?.cors, 'config.middleware.builtin.cors', [
    'origins',
    'methods',
    'headers',
    'credentials',
    'maxAge',
  ])
  assertKnownKeys(config.middleware?.builtin?.rate, 'config.middleware.builtin.rate', [
    'max',
    'window',
    'key',
  ])
  if (Array.isArray(config.middleware?.plugins)) {
    for (const [index, plugin] of config.middleware.plugins.entries()) {
      assertKnownKeys(plugin, `config.middleware.plugins[${index}]`, [
        'name',
        'path',
        'phase',
        'routes',
        'config',
        'allow',
      ])
      assertKnownKeys(plugin?.allow, `config.middleware.plugins[${index}].allow`, [
        'env',
        'read',
        'net',
        'timeout',
        'memory',
      ])
    }
  }

  return {
    appDir: stringValue(config.appDir),
    outDir: stringValue(config.outDir),
    runtime: stringValue(config.runtime),
    react: booleanValue(config.react),
    css: objectValue(config.css, {
      entries: stringArrayValue(config.css?.entries),
    }),
    server: objectValue(config.server, {
      host: stringValue(config.server?.host),
      port: numberValue(config.server?.port),
    }),
    build: objectValue(config.build, {
      minify: booleanValue(config.build?.minify),
      map: booleanValue(config.build?.map),
      treeShake: booleanValue(config.build?.treeShake),
      split: stringValue(config.build?.split),
      workers: numberValue(config.build?.workers),
      jsx: stringValue(config.build?.jsx),
      target: stringValue(config.build?.target),
      manifest: booleanValue(config.build?.manifest),
      warm: booleanValue(config.build?.warm),
    }),
    render: objectValue(config.render, {
      strategy: stringValue(config.render?.strategy),
      revalidate: numberValue(config.render?.revalidate),
    }),
    debug: objectValue(config.debug, {
      overlay: booleanValue(config.debug?.overlay),
      traces: booleanValue(config.debug?.traces),
    }),
    image: objectValue(config.image, {
      optimize: booleanValue(config.image?.optimize),
      quality: numberValue(config.image?.quality),
      lossless: booleanValue(config.image?.lossless),
      workers: numberValue(config.image?.workers),
    }),
    security: objectValue(config.security, {
      actionLimit: numberValue(config.security?.actionLimit),
      apiLimit: numberValue(config.security?.apiLimit),
      actionRateLimit: objectValue(config.security?.actionRateLimit, {
        max: numberValue(config.security?.actionRateLimit?.max),
        window: numberValue(config.security?.actionRateLimit?.window),
      }),
      sameOrigin: booleanValue(config.security?.sameOrigin),
      fetchMeta: booleanValue(config.security?.fetchMeta),
      headers: booleanValue(config.security?.headers),
    }),
    cache: objectValue(config.cache, {
      routes: booleanValue(config.cache?.routes),
      css: booleanValue(config.cache?.css),
      dir: stringValue(config.cache?.dir),
    }),
    middleware: safeJsonValue(config.middleware),
    adapter: await adapterOutput(config.adapter, projectRoot, config.outDir),
    adapterOptions: safeJsonValue(config.adapterOptions),
    plugins: pluginDescriptors(config.plugins),
  }
}

async function adapterOutput(adapter, root, outDir) {
  if (adapter === undefined) return undefined
  if (!adapter || typeof adapter !== 'object' || typeof adapter.build !== 'function') {
    throw new Error('RUV1603 config.adapter must provide a build(context) function.')
  }

  const output = await adapter.build({ root, outDir: stringValue(outDir) ?? '.ruvyxa' })
  if (!output || typeof output !== 'object') {
    throw new Error('RUV1603 config.adapter.build(context) must return an adapter output object.')
  }
  if (typeof output.name !== 'string' || typeof output.target !== 'string') {
    throw new Error('RUV1603 adapter output must include string name and target fields.')
  }

  const serialized = safeJsonValue(output)
  if (serialized === undefined) {
    throw new Error('RUV1603 adapter output must be JSON-serializable.')
  }

  return serialized
}

function assertKnownKeys(value, field, allowedKeys) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return
  const allowed = new Set(allowedKeys)
  const unknown = Object.keys(value).filter((key) => !allowed.has(key))
  if (unknown.length > 0) {
    throw new Error(
      `RUV1602 unknown ${field} field${unknown.length === 1 ? '' : 's'}: ${unknown.join(', ')}`,
    )
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

function stringArrayValue(value) {
  if (!Array.isArray(value) || !value.every((item) => typeof item === 'string')) return undefined
  return value
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

async function ok(config, dependencyHash) {
  await writeJson({ ok: true, config, dependencyHash })
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
