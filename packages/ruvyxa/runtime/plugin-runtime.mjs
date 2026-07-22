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
    failure('RUV1701', 'Plugin runtime requires project root and hook or mode arguments.'),
  )
  process.exit(1)
}

// Stdout is the NDJSON protocol. Application-style plugin logging belongs on stderr.
console.log = console.info = console.debug = (...args) => console.error(...args)

const projectRoot = path.resolve(projectRootArg)
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

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
  writeResponse(
    failure('RUV1700', error instanceof Error ? error.message : String(error), error?.stack),
    mode === '--persistent',
  )
  process.exitCode = 1
}

async function loadRegistry(root) {
  const configFile = findConfig(root)
  if (!configFile) return createRegistry(root, [])

  const moduleCode = `export { default } from ${JSON.stringify(toImportPath(configFile))}`
  const outfile = path.join(
    root,
    '.ruvyxa',
    'cache',
    'config',
    cacheFileName([moduleCode, configFile, 'plugin-runtime'], 'mjs'),
  )
  await compileBundle({
    projectRoot: root,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:plugin-config-entry.ts',
    outfile,
    platform: serverPlatform(),
    bundleAliasDependencies: true,
    aliases: runtimeAliases(runtimeDir),
  })

  const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
  const config = mod.default ?? {}
  return createRegistry(root, config.plugins)
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

async function createRegistry(root, pluginsValue) {
  const plugins = Array.isArray(pluginsValue) ? pluginsValue : []
  const names = new Set()
  const registry = {
    root,
    plugins: [],
    middleware: [],
    resolveId: [],
    transform: [],
    buildComplete: [],
    realtime: null,
  }

  for (const [index, plugin] of plugins.entries()) {
    if (!plugin || typeof plugin !== 'object') {
      throw new TypeError(`config.plugins[${index}] must be an object`)
    }
    const name = typeof plugin.name === 'string' ? plugin.name.trim() : ''
    if (!name) throw new TypeError(`config.plugins[${index}] must have a non-empty name`)
    if (names.has(name)) throw new TypeError(`duplicate plugin name: ${name}`)
    if (typeof plugin.setup !== 'function') {
      throw new TypeError(`plugin "${name}" must provide setup(context)`)
    }
    names.add(name)
    registry.plugins.push(name)

    const setupContext = Object.freeze({
      addMiddleware(value) {
        registry.middleware.push(normalizeMiddleware(name, value))
      },
      resolveId(hook) {
        assertHook(name, 'resolveId', hook)
        registry.resolveId.push({ plugin: name, hook })
      },
      transform(hook) {
        assertHook(name, 'transform', hook)
        registry.transform.push({ plugin: name, hook })
      },
      onBuildComplete(hook) {
        assertHook(name, 'onBuildComplete', hook)
        registry.buildComplete.push({ plugin: name, hook })
      },
      enableRealtime(options = {}) {
        if (registry.realtime) {
          throw new TypeError(
            `plugin "${name}" cannot enable realtime because plugin "${registry.realtime.plugin}" already owns the transport`,
          )
        }
        registry.realtime = normalizeRealtime(name, options)
      },
    })
    await plugin.setup(setupContext)
  }

  return registry
}

function normalizeRealtime(plugin, value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError(`plugin "${plugin}" enableRealtime() expects an options object`)
  }
  const pathValue = value.path ?? '/__ruvyxa/realtime'
  const heartbeatMs = value.heartbeatMs ?? 25_000
  const capacity = value.capacity ?? 256
  if (
    typeof pathValue !== 'string' ||
    !pathValue.startsWith('/') ||
    pathValue.includes('?') ||
    pathValue.includes('#') ||
    pathValue.includes('*')
  ) {
    throw new TypeError(
      `plugin "${plugin}" realtime path must be an absolute path without query, fragment, or wildcard`,
    )
  }
  if (!Number.isInteger(heartbeatMs) || heartbeatMs < 5_000 || heartbeatMs > 120_000) {
    throw new TypeError(`plugin "${plugin}" realtime heartbeatMs must be between 5000 and 120000`)
  }
  if (!Number.isInteger(capacity) || capacity < 16 || capacity > 4096) {
    throw new TypeError(`plugin "${plugin}" realtime capacity must be between 16 and 4096`)
  }
  const reserved = ['/__ruvyxa/hmr', '/__ruvyxa/client', '/__ruvyxa/action', '/__ruvyxa/trace']
  if (reserved.includes(pathValue)) {
    throw new TypeError(
      `plugin "${plugin}" realtime path "${pathValue}" collides with a reserved framework route`,
    )
  }
  return Object.freeze({ plugin, path: pathValue, heartbeatMs, capacity })
}

function normalizeMiddleware(plugin, value) {
  const middleware = typeof value === 'function' ? { onRequest: value } : value
  if (!middleware || typeof middleware !== 'object') {
    throw new TypeError(
      `plugin "${plugin}" addMiddleware() expects a function or middleware object`,
    )
  }
  const onRequest = middleware.onRequest
  const onResponse = middleware.onResponse
  if (typeof onRequest !== 'function' && typeof onResponse !== 'function') {
    throw new TypeError(`plugin "${plugin}" middleware must provide onRequest and/or onResponse`)
  }
  const routes = middleware.routes
  if (
    routes !== undefined &&
    (!Array.isArray(routes) || routes.some((route) => typeof route !== 'string'))
  ) {
    throw new TypeError(`plugin "${plugin}" middleware routes must be an array of strings`)
  }
  if (routes) {
    for (const [index, route] of routes.entries()) {
      const wildcard = route.indexOf('*')
      const validStart = route === '*' || route.startsWith('/')
      const validWildcard =
        wildcard === -1 || (wildcard === route.length - 1 && wildcard === route.lastIndexOf('*'))
      if (!validStart || !validWildcard) {
        throw new TypeError(
          `plugin "${plugin}" middleware routes[${index}] must start with "/" or equal "*", with a wildcard only at the end`,
        )
      }
    }
  }
  return { plugin, routes, onRequest, onResponse }
}

function assertHook(plugin, name, hook) {
  if (typeof hook !== 'function') {
    throw new TypeError(`plugin "${plugin}" ${name}() expects a function`)
  }
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
      response = failure(
        'RUV1700',
        error instanceof Error ? error.message : String(error),
        error?.stack,
      )
    }
    writeResponse(response, true)
  }
}

async function handleHook(registry, hook, payload) {
  switch (hook) {
    case 'describe':
      return success({
        plugins: registry.plugins,
        middleware: {
          request: registry.middleware.filter((entry) => entry.onRequest).length,
          response: registry.middleware.filter((entry) => entry.onResponse).length,
          requestRoutes: middlewareRouteUnion(registry.middleware, 'onRequest'),
          responseRoutes: middlewareRouteUnion(registry.middleware, 'onResponse'),
        },
        resolveId: registry.resolveId.length,
        transform: registry.transform.length,
        buildComplete: registry.buildComplete.length,
        realtime: registry.realtime,
      })
    case 'resolveId':
      return success(await runResolveId(registry, payload))
    case 'transform':
      return success(await runTransform(registry, payload))
    case 'middlewareRequest':
      return success(await runRequestMiddleware(registry, payload))
    case 'middlewareResponse':
      return success(await runResponseMiddleware(registry, payload))
    case 'buildComplete':
      await runBuildComplete(registry, payload)
      return success(null)
    default:
      return failure('RUV1701', `Unknown plugin hook: ${hook}`)
  }
}

async function runResolveId(registry, payload) {
  const context = transformContext(registry, payload)
  for (const entry of registry.resolveId) {
    const result = await entry.hook(payload.id, payload.importer ?? undefined, context)
    if (typeof result === 'string') return result
  }
  return null
}

async function runTransform(registry, payload) {
  let code = String(payload.code ?? '')
  let map
  let changed = false
  const context = transformContext(registry, payload)

  for (const entry of registry.transform) {
    const result = await entry.hook(code, String(payload.id ?? ''), context)
    if (typeof result === 'string') {
      code = result
      changed = true
    } else if (result && typeof result === 'object' && typeof result.code === 'string') {
      code = result.code
      if (result.map !== undefined && result.map !== null) {
        map = typeof result.map === 'string' ? result.map : JSON.stringify(result.map)
      }
      changed = true
    }
  }
  return changed ? { code, ...(map === undefined ? {} : { map }) } : null
}

function transformContext(registry, payload) {
  return Object.freeze({
    root: registry.root,
    environment: payload.environment === 'server' ? 'server' : 'client',
  })
}

async function runRequestMiddleware(registry, payload) {
  let request = requestFromPayload(payload.request)
  for (const middleware of registry.middleware) {
    if (!middleware.onRequest || !matchesRoutes(middleware.routes, new URL(request.url).pathname)) {
      continue
    }
    const context = middlewareContext(registry, middleware)
    const result = await middleware.onRequest(request.clone(), context)
    if (result instanceof Response) {
      return { kind: 'response', response: await responseToPayload(result) }
    }
    if (result instanceof Request) request = result
    else if (result !== undefined) {
      throw new TypeError(
        `plugin "${middleware.plugin}" request middleware returned an unsupported value`,
      )
    }
  }
  return { kind: 'request', request: await requestToPayload(request) }
}

async function runResponseMiddleware(registry, payload) {
  const request = requestFromPayload(payload.request)
  let response = responseFromPayload(payload.response)
  for (const middleware of registry.middleware) {
    if (
      !middleware.onResponse ||
      !matchesRoutes(middleware.routes, new URL(request.url).pathname)
    ) {
      continue
    }
    const result = await middleware.onResponse(
      request.clone(),
      response.clone(),
      middlewareContext(registry, middleware),
    )
    if (result instanceof Response) response = result
    else if (result !== undefined) {
      throw new TypeError(
        `plugin "${middleware.plugin}" response middleware returned an unsupported value`,
      )
    }
  }
  return { response: await responseToPayload(response) }
}

async function runBuildComplete(registry, payload) {
  const context = Object.freeze({
    root: registry.root,
    outDir: path.resolve(payload.outDir),
    manifest: Object.freeze(payload.manifest ?? {}),
  })
  for (const entry of registry.buildComplete) await entry.hook(context)
}

function middlewareContext(registry, middleware) {
  return Object.freeze({ plugin: middleware.plugin, root: registry.root })
}

/**
 * Union of route patterns for one middleware direction, used by the native
 * server to skip the plugin round-trip for paths no middleware can match.
 * `null` means at least one middleware matches every route.
 */
function middlewareRouteUnion(middleware, hookName) {
  const patterns = new Set()
  for (const entry of middleware) {
    if (typeof entry[hookName] !== 'function') continue
    if (!entry.routes || entry.routes.length === 0 || entry.routes.includes('*')) return null
    for (const route of entry.routes) patterns.add(route)
  }
  return [...patterns]
}

function matchesRoutes(routes, pathname) {
  if (!routes || routes.length === 0) return true
  return routes.some((route) => {
    if (route === '*') return true
    if (route.endsWith('*')) return pathname.startsWith(route.slice(0, -1))
    return pathname === route
  })
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

function writeResponse(response, newline = false) {
  process.stdout.write(JSON.stringify(response) + (newline ? '\n' : ''))
}
