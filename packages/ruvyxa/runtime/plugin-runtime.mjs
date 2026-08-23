import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  cacheFileName,
  compileBundleIfChanged,
  compileContentSource,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'
// The registry and its HTTP dispatch are shared with the serverless handler.
// This module owns only the stdio protocol the Rust host speaks, and the build
// hooks, which have no meaning outside a build.
import {
  createPluginRegistry,
  describeRegistry,
  dispatchPluginRequest,
  dispatchPluginResponse,
  matchesPatterns,
  unsupportedReturn,
} from './plugin-http.mjs'
import { transformWithReactCompiler } from './react-compiler.mjs'

const [projectRootArg, mode] = process.argv.slice(2)

/**
 * Which environment this host serves, as stated by whoever spawned it.
 *
 * Named apart from the build `environment` (client/server/edge/worker/shared)
 * that transform hooks carry — the two are unrelated axes.
 *
 * Read by name rather than by position: the persistent path passes
 * `--persistent` where the one-shot build path passes a hook name. A host that
 * says nothing is treated as production, so development-only request handling
 * is never enabled for a server that may be serving real traffic.
 */
const hostEnvironment = process.argv.includes('--environment=development')
  ? 'development'
  : 'production'

if (!projectRootArg || !mode) {
  writeResponse(failure('RUV1701', 'Plugin runtime requires project root and mode arguments.'))
  process.exit(1)
}

// Stdout is reserved for the NDJSON protocol.
console.log = console.info = console.debug = (...args) => console.error(...args)

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))
let reactCompilerEnabled = false

try {
  const registry = await loadRegistry(projectRoot)
  if (mode === '--persistent') {
    await runPersistent(registry)
  } else {
    const payload = JSON.parse(readFileSync(0, 'utf8'))
    const response = await handleHook(registry, mode, payload)
    writeResponse(response)
    if (!response.ok) process.exitCode = 1
  }
} catch (error) {
  writeResponse(failureFromError(error), mode === '--persistent')
  process.exitCode = 1
}

async function loadRegistry(root) {
  const configFile = findConfig(root)
  if (!configFile) return createPluginRegistry({ root, plugins: [], environment: hostEnvironment })

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    root,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile, 'plugin-runtime'], 'mjs'),
  )
  await compileBundleIfChanged({
    projectRoot: root,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:plugin-config-entry.ts',
    outfile,
    platform: serverPlatform(),
    bundleAliasDependencies: true,
    aliases: runtimeAliases(runtimeDir),
    markdownConfig: false,
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  const config = mod.default ?? {}
  reactCompilerEnabled = config.reactCompiler === true
  const configuredPlugins = Array.isArray(config.plugins) ? config.plugins : []
  const contentPlugin = await configuredContentPlugin(root, configFile, config)
  return createPluginRegistry({
    root,
    plugins: contentPlugin ? [...configuredPlugins, contentPlugin] : configuredPlugins,
    markdown: config.markdown,
    environment: hostEnvironment,
  })
}

async function configuredContentPlugin(root, configFile, config) {
  const content = config?.content
  const enabled =
    content === true ||
    (content &&
      typeof content === 'object' &&
      !Array.isArray(content) &&
      (content.engine === true ||
        (content.engine && typeof content.engine === 'object' && !Array.isArray(content.engine))))
  if (!enabled) return undefined

  const moduleCode = 'export { contentEngineFromConfig as default } from "ruvyxa/plugins"'
  const outfile = path.join(
    root,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile, 'content-engine-runtime'], 'mjs'),
  )
  await compileBundleIfChanged({
    projectRoot: root,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:content-engine-config-entry.ts',
    outfile,
    platform: serverPlatform(),
    bundleAliasDependencies: true,
    aliases: runtimeAliases(runtimeDir),
  })
  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  return mod.default(config)
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

async function runPersistent(registry) {
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })
  for await (const line of lines) {
    if (!line.trim()) continue
    let response
    try {
      const payload = JSON.parse(line)
      response = await handleHook(registry, payload.hook, payload)
    } catch (error) {
      response = failureFromError(error)
    }
    writeResponse(response, true)
  }
}

async function handleHook(registry, hook, payload) {
  switch (hook) {
    case 'describe':
      return success(describeRegistry(registry))
    case 'http.request':
      return success(await runHttpRequest(registry, payload))
    case 'http.response':
      return success(await runHttpResponse(registry, payload))
    case 'build.start':
      await runBuildStart(registry, payload)
      return success(null)
    case 'build.resolve':
      return success(await runBuildResolve(registry, payload))
    case 'build.load':
      return success(await runBuildLoad(registry, payload))
    case 'build.transform':
      return success(await runBuildTransform(registry, payload))
    case 'content.compile':
      return success(await runContentCompile(registry, payload))
    case 'build.complete':
      await runBuildComplete(registry, payload)
      return success(null)
    case 'dev.fileChange':
      await runDevFileChange(registry, payload)
      return success(null)
    default:
      return failure('RUV1701', `Unknown plugin hook: ${hook}`)
  }
}

async function runBuildResolve(registry, payload) {
  const base = buildContext(registry, payload)
  const context = Object.freeze({
    ...base,
    id: String(payload.id ?? ''),
    importer: payload.importer ?? undefined,
  })
  for (const entry of registry.buildResolve) {
    const result = await entry.hook(context)
    if (typeof result === 'string') return result
    if (result !== null && result !== undefined)
      throw unsupportedReturn(entry.plugin, 'build.onResolve')
  }
  return null
}

async function runBuildLoad(registry, payload) {
  const context = Object.freeze({
    ...buildContext(registry, payload),
    id: String(payload.id ?? ''),
  })
  for (const entry of registry.buildLoad) {
    const result = await entry.hook(context)
    const normalized = normalizeCodeResult(entry.plugin, 'build.onLoad', result)
    if (normalized) return normalized
  }
  return null
}

async function runBuildTransform(registry, payload) {
  let code = String(payload.code ?? '')
  let map
  let changed = false
  if (reactCompilerEnabled) {
    const compiled = transformWithReactCompiler(code, String(payload.id ?? ''))
    if (compiled) {
      code = compiled.code
      map = compiled.map
      changed = true
    }
  }
  const base = buildContext(registry, payload)
  for (const entry of registry.buildTransform) {
    const context = Object.freeze({ ...base, code, id: String(payload.id ?? '') })
    const result = normalizeCodeResult(entry.plugin, 'build.onTransform', await entry.hook(context))
    if (!result) continue
    code = result.code
    if (result.map !== undefined) map = result.map
    changed = true
  }
  return changed ? { code, ...(map === undefined ? {} : { map }) } : null
}

async function runContentCompile(registry, payload) {
  const id = path.resolve(String(payload.id ?? ''))
  const extension = path.extname(id).toLowerCase()
  if (extension !== '.md' && extension !== '.mdx') return null
  const compiled = await compileContentSource(
    String(payload.code ?? ''),
    id,
    registry.root,
    registry.markdown ?? null,
  )
  return { code: compiled.source }
}

function normalizeCodeResult(plugin, socket, result) {
  if (result === null || result === undefined) return null
  if (typeof result === 'string') return { code: result }
  if (result && typeof result === 'object' && typeof result.code === 'string') {
    return {
      code: result.code,
      ...(result.map === undefined || result.map === null
        ? {}
        : { map: typeof result.map === 'string' ? result.map : JSON.stringify(result.map) }),
    }
  }
  throw unsupportedReturn(plugin, socket)
}

function buildContext(registry, payload) {
  const allowed = new Set(['client', 'server', 'edge', 'worker', 'shared'])
  const environment = allowed.has(payload.environment) ? payload.environment : 'client'
  return { root: registry.root, environment }
}

async function runBuildStart(registry, payload) {
  const context = Object.freeze({ root: registry.root, outDir: path.resolve(payload.outDir) })
  for (const entry of registry.buildStart) await entry.hook(context)
}

async function runBuildComplete(registry, payload) {
  const context = Object.freeze({
    root: registry.root,
    outDir: path.resolve(payload.outDir),
    manifest: Object.freeze(payload.manifest ?? {}),
  })
  for (const entry of registry.buildComplete) await entry.hook(context)
}

async function runHttpRequest(registry, payload) {
  const outcome = await dispatchPluginRequest(registry, requestFromPayload(payload.request))
  return outcome.kind === 'response'
    ? { kind: 'response', response: await responseToPayload(outcome.response) }
    : { kind: 'request', request: await requestToPayload(outcome.request) }
}

async function runHttpResponse(registry, payload) {
  const request = requestFromPayload(payload.request)
  const response = await dispatchPluginResponse(
    registry,
    request,
    responseFromPayload(payload.response),
  )
  return { response: await responseToPayload(response) }
}

async function runDevFileChange(registry, payload) {
  const paths = Array.isArray(payload.paths) ? payload.paths.map(String) : []
  for (const entry of registry.devFileChange) {
    const selected = entry.match
      ? paths.filter((value) => matchesPatterns(entry.match, value))
      : paths
    if (selected.length === 0) continue
    await entry.handler(Object.freeze({ root: registry.root, paths: Object.freeze(selected) }))
  }
}

function requestFromPayload(value = {}) {
  const pathname = typeof value.path === 'string' && value.path.startsWith('/') ? value.path : '/'
  const method = String(value.method ?? 'GET').toUpperCase()
  const body = method === 'GET' || method === 'HEAD' ? undefined : decodeBody(value.bodyBase64)
  return new Request(`http://ruvyxa.local${pathname}`, {
    method,
    headers: headersFromPairs(value.headers),
    body,
  })
}

function responseFromPayload(value = {}) {
  return new Response(decodeBody(value.bodyBase64), {
    status: Number(value.status ?? 200),
    headers: headersFromPairs(value.headers),
  })
}

async function requestToPayload(request) {
  const url = new URL(request.url)
  return {
    method: request.method,
    path: url.pathname + url.search,
    headers: headerPairs(request.headers),
    bodyBase64: await encodeBody(request),
  }
}

async function responseToPayload(response) {
  return {
    status: response.status,
    headers: headerPairs(response.headers),
    bodyBase64: await encodeBody(response),
  }
}

function headersFromPairs(value) {
  const headers = new Headers()
  if (Array.isArray(value)) {
    for (const pair of value) {
      if (Array.isArray(pair) && pair.length === 2) headers.append(String(pair[0]), String(pair[1]))
    }
  }
  return headers
}

function headerPairs(headers) {
  const pairs = Array.from(headers.entries()).filter(([name]) => name !== 'set-cookie')
  const cookies = typeof headers.getSetCookie === 'function' ? headers.getSetCookie() : []
  for (const cookie of cookies) pairs.push(['set-cookie', cookie])
  return pairs
}

function decodeBody(value) {
  return typeof value === 'string' ? Buffer.from(value, 'base64') : undefined
}

async function encodeBody(message) {
  const bytes = Buffer.from(await message.arrayBuffer())
  return bytes.length > 0 ? bytes.toString('base64') : undefined
}

function success(result) {
  return { ok: true, result }
}

function failure(code, message, stack) {
  return { ok: false, code, message, stack }
}

function failureFromError(error) {
  return failure(
    error?.pluginCode === 'RUV1701' ? 'RUV1701' : 'RUV1700',
    error instanceof Error ? error.message : String(error),
    error?.stack,
  )
}

function writeResponse(response, newline = false) {
  process.stdout.write(JSON.stringify(response) + (newline ? '\n' : ''))
}
