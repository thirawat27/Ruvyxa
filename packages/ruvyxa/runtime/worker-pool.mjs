#!/usr/bin/env node
/**
 * Persistent Node worker for Ruvyxa.
 *
 * Stays alive and processes JSON-delimited requests from stdin.
 * Each request line is a JSON object with a `type` field indicating
 * which renderer to invoke. Responses are written as single-line JSON
 * to stdout, terminated by a newline.
 *
 * Protocol:
 *   Request:  { id, type: "ssr"|"api"|"action"|"client", ...args }
 *   Response: { id, ...result }
 *
 * Performance optimizations:
 *   - Module import cache: avoids re-parsing unchanged bundles on every request
 *   - Directory creation cache: eliminates redundant mkdir syscalls
 *   - LRU-bounded bundle cache with build locks (no duplicate builds)
 *   - Lazy React dependency resolution (cached after first check)
 *   - Graceful shutdown with SIGTERM/SIGINT handling
 *   - Memory pressure monitoring with automatic cache eviction
 */
import { createHash } from 'node:crypto'
import { once } from 'node:events'
import { existsSync } from 'node:fs'
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createInterface } from 'node:readline'

import {
  clearCompilerCache,
  compileBundleWithMetadata,
  invalidateCompilerCache,
  runtimeAliases,
  toImportPath,
} from './compiler.mjs'

// --- Configuration ---
const MAX_BUNDLE_CACHE_ENTRIES = positiveIntegerEnv('RUVYXA_CACHE_MAX_ENTRIES', 256)
const MAX_NODE_TIMEOUT_MS = 2_147_483_647
const WORKER_REQUEST_TIMEOUT_MS = positiveIntegerEnv(
  'RUVYXA_WORKER_TIMEOUT_MS',
  30_000,
  MAX_NODE_TIMEOUT_MS,
)
const MEMORY_PRESSURE_THRESHOLD_MB = positiveIntegerEnv('RUVYXA_MEMORY_LIMIT_MB', 512)
const API_STREAM_CHUNK_BYTES = 64 * 1024
const runtimeDir = path.dirname(fileURLToPath(import.meta.url))

// --- LRU Cache ---
class LRUCache {
  #max
  #map = new Map()

  constructor(max) {
    this.#max = max
  }

  get(key) {
    if (!this.#map.has(key)) return undefined
    const value = this.#map.get(key)
    this.#map.delete(key)
    this.#map.set(key, value)
    return value
  }

  set(key, value) {
    let evicted
    if (this.#map.has(key)) {
      this.#map.delete(key)
    } else if (this.#map.size >= this.#max) {
      const evictedKey = this.#map.keys().next().value
      evicted = { key: evictedKey, value: this.#map.get(evictedKey) }
      this.#map.delete(evictedKey)
    }
    this.#map.set(key, value)
    return evicted
  }

  has(key) {
    return this.#map.has(key)
  }

  delete(key) {
    const value = this.#map.get(key)
    this.#map.delete(key)
    return value
  }

  clear() {
    this.#map.clear()
  }

  get size() {
    return this.#map.size
  }

  keys() {
    return this.#map.keys()
  }
}

// --- State ---
const bundleCache = new LRUCache(MAX_BUNDLE_CACHE_ENTRIES)
// Cache key -> normalized absolute project files used to build that bundle.
const bundleInputs = new Map()
const bundleFingerprints = new Map()
const buildLocks = new Map()

// Performance: Module import cache — avoids re-parsing JS on every request.
// Key: absolute outfile path, Value: imported module object.
// Invalidated only when the bundle is re-built.
const moduleCache = new LRUCache(MAX_BUNDLE_CACHE_ENTRIES)

// Performance: Track directories already created to skip mkdir syscalls.
const createdDirs = new Set()

// Performance: Cache React dependency resolution per project root.
const reactResolvedRoots = new Set()

// Performance: Request coalescing — collapse duplicate concurrent renders.
// Key: coalesce_key (route+params hash), Value: Promise of result.
// If two SSR requests for the same page arrive concurrently, only one
// actually renders; the second awaits the same Promise.
const renderCoalesceMap = new Map()

// Performance: Warm module queue — tracks modules to pre-import on idle.
const warmupQueue = []
let warmupTimer = null

let activeRequests = 0
let isShuttingDown = false
let moduleImportVersion = 0

// --- Graceful Shutdown ---
function shutdown() {
  if (isShuttingDown) return
  isShuttingDown = true
  if (activeRequests === 0) process.exit(0)
  setTimeout(() => process.exit(0), 5000).unref()
}

process.on('SIGTERM', () => shutdown('SIGTERM'))
process.on('SIGINT', () => shutdown('SIGINT'))

// --- Memory Pressure Monitor ---
const memoryCheckInterval = setInterval(() => {
  const heapMB = process.memoryUsage().heapUsed / 1024 / 1024
  if (heapMB > MEMORY_PRESSURE_THRESHOLD_MB) {
    const evictCount = Math.ceil(bundleCache.size / 2)
    const keys = bundleCache.keys()
    for (let i = 0; i < evictCount; i++) {
      const { value, done } = keys.next()
      if (done) break
      deleteBundleCacheEntry(value)
    }
    moduleCache.clear()
    clearCompilerCache()
  }
}, 30_000)
memoryCheckInterval.unref()

// --- Request Processing ---
const rl = createInterface({ input: process.stdin })

rl.on('line', async (line) => {
  if (isShuttingDown) return

  let request
  try {
    request = JSON.parse(line)
  } catch {
    return
  }

  const { id } = request
  if (!id) return

  activeRequests++

  try {
    const result = await withTimeout(
      dispatchRequest(request),
      WORKER_REQUEST_TIMEOUT_MS,
      `Request ${request.type}:${id} timed out after ${WORKER_REQUEST_TIMEOUT_MS}ms`,
    )
    if (result?.streamResponse instanceof Response) {
      await emitApiStream(id, result)
    } else {
      await writeWorkerMessage({ id, ...result })
    }
  } catch (error) {
    try {
      await writeWorkerMessage({ id, ...workerError(error) })
    } catch {
      shutdown()
    }
  } finally {
    activeRequests--
    if (isShuttingDown && activeRequests === 0) process.exit(0)
  }
})

rl.on('close', () => shutdown('stdin-close'))
process.stdin.resume()

// --- Request Dispatcher ---
async function dispatchRequest(request) {
  switch (request.type) {
    case 'ssr':
      return handleSsrCoalesced(request)
    case 'ssg':
      return handleSsgCoalesced(request)
    case 'staticParams':
      return handleStaticParams(request)
    case 'api':
      return handleApi(request)
    case 'action':
      return handleAction(request)
    case 'client':
      return handleClient(request)
    case 'warmup':
      return handleWarmup(request)
    case 'ping':
      return {
        ok: true,
        pong: true,
        cacheSize: bundleCache.size,
        moduleCacheSize: moduleCache.size,
        activeRequests,
        coalesceMapSize: renderCoalesceMap.size,
        workerRequestTimeoutMs: WORKER_REQUEST_TIMEOUT_MS,
        memoryPressureThresholdMb: MEMORY_PRESSURE_THRESHOLD_MB,
      }
    case 'invalidate':
      return { ok: true, ...invalidateBundleCache(request.paths) }
    default:
      return { ok: false, code: 'RUV1700', message: `Unknown request type: ${request.type}` }
  }
}

// --- Timeout Utility ---
function withTimeout(promise, ms, message) {
  if (!ms || ms <= 0) return promise
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), ms)
    timer.unref()
    promise.then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      (error) => {
        clearTimeout(timer)
        reject(error)
      },
    )
  })
}

function workerError(error) {
  return {
    ok: false,
    code: 'RUV1700',
    message: error instanceof Error ? error.message : String(error),
    stack: error?.stack,
  }
}

async function writeWorkerMessage(message) {
  if (!process.stdout.write(`${JSON.stringify(message)}\n`)) {
    await once(process.stdout, 'drain')
  }
}

async function emitApiStream(id, result) {
  const { streamResponse, ...head } = result
  await writeWorkerMessage({ id, frame: 'api-start', ...head })

  const reader = streamResponse.body?.getReader()
  if (!reader) {
    await writeWorkerMessage({ id, frame: 'api-end', ok: true })
    return
  }

  try {
    while (true) {
      const { done, value } = await withTimeout(
        reader.read(),
        WORKER_REQUEST_TIMEOUT_MS,
        `API response stream ${id} was idle for ${WORKER_REQUEST_TIMEOUT_MS}ms`,
      )
      if (done) break

      const bytes = Buffer.from(value.buffer, value.byteOffset, value.byteLength)
      for (let offset = 0; offset < bytes.length; offset += API_STREAM_CHUNK_BYTES) {
        await writeWorkerMessage({
          id,
          frame: 'api-chunk',
          ok: true,
          bodyBase64: bytes.subarray(offset, offset + API_STREAM_CHUNK_BYTES).toString('base64'),
        })
      }
    }
    await writeWorkerMessage({ id, frame: 'api-end', ok: true })
  } catch (error) {
    try {
      await reader.cancel(error)
    } catch {
      // The source may already be closed; the protocol error below is authoritative.
    }
    await writeWorkerMessage({ id, frame: 'api-error', ...workerError(error) })
  } finally {
    reader.releaseLock()
  }
}

// --- Build Lock ---
async function withBuildLock(cacheKey, buildFn) {
  if (buildLocks.has(cacheKey)) {
    return buildLocks.get(cacheKey)
  }
  const buildPromise = buildFn()
  buildLocks.set(cacheKey, buildPromise)
  try {
    return await buildPromise
  } finally {
    buildLocks.delete(cacheKey)
  }
}

// --- Fast mkdir (cached) ---
async function ensureDir(dir) {
  if (createdDirs.has(dir)) return
  await mkdir(dir, { recursive: true })
  createdDirs.add(dir)
}

// --- Fast module import (cached) ---
// Avoids V8 re-parsing the same JS file on every request.
// Only cache-busts when the bundle is freshly built.
async function importModule(outfile, forceReload = false) {
  if (!forceReload) {
    const cached = moduleCache.get(outfile)
    if (cached) return cached
  }
  // Use timestamp only when we need to bust Node's ESM cache
  const mod = await import(pathToFileURL(outfile).href + `?t=${++moduleImportVersion}`)
  moduleCache.set(outfile, mod)
  return mod
}

// --- Fast React resolution (cached per project root) ---
function ensureReactDeps(resolvedRoot) {
  if (reactResolvedRoots.has(resolvedRoot)) return
  const requireFromProject = createRequire(path.join(resolvedRoot, 'package.json'))
  requireFromProject.resolve('react')
  requireFromProject.resolve('react-dom/server')
  reactResolvedRoots.add(resolvedRoot)
}

// --- SSR Handler with Request Coalescing ---
// If two concurrent SSR requests hit the same page+params, only one renders;
// the duplicate awaits the same promise. This eliminates redundant work
// during rapid navigation or concurrent crawler hits.
async function handleSsrCoalesced(request) {
  const { pageFile, requestPath, params } = request
  const coalesceKey = `ssr:${pageFile}:${requestPath}:${JSON.stringify(params || {})}`

  // Check if an identical render is already in-flight.
  if (renderCoalesceMap.has(coalesceKey)) {
    return renderCoalesceMap.get(coalesceKey)
  }

  // No duplicate — start the render and register the promise.
  const renderPromise = handleSsr(request).finally(() => {
    renderCoalesceMap.delete(coalesceKey)
  })
  renderCoalesceMap.set(coalesceKey, renderPromise)
  return renderPromise
}

// --- Warmup Handler ---
// Pre-imports module bundles into V8's module cache during idle time,
// so the first real request for a route doesn't pay the import cost.
async function handleWarmup(request) {
  const { projectRoot, routes } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  let warmed = 0

  if (routes && Array.isArray(routes)) {
    for (const route of routes) {
      try {
        if (route.pageFile) {
          const layouts = route.appDir
            ? collectLayouts(route.appDir, path.dirname(route.pageFile))
            : []
          const { outfile } = await bundleSsrModule(resolvedRoot, route.pageFile, layouts)
          await importModule(outfile, false)
          warmed++
        }
      } catch {
        // Warmup failures are non-fatal — the module will be compiled on first request.
      }
    }
  }

  // Also pre-resolve React deps for the project root.
  try {
    ensureReactDeps(resolvedRoot)
  } catch {
    // Non-fatal
  }

  return { ok: true, warmed, moduleCacheSize: moduleCache.size }
}

// --- SSR Handler ---
async function handleSsr(request) {
  const { projectRoot, appDir, pageFile, requestPath, params } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureReactDeps(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const { outfile, freshBuild } = await bundleSsrModule(resolvedRoot, pageFile, layouts)
  const mod = await importModule(outfile, freshBuild)
  const html = await mod.render({ path: requestPath, params: params || {} })

  return { ok: true, html }
}

// --- SSG Handler with Request Coalescing ---
async function handleSsgCoalesced(request) {
  const { pageFile, requestPath, params, mode, fresh } = request
  const coalesceKey = `ssg:${pageFile}:${requestPath}:${JSON.stringify(params || {})}:${mode || 'full'}:${fresh ? 'fresh' : 'cached'}`

  if (renderCoalesceMap.has(coalesceKey)) {
    return renderCoalesceMap.get(coalesceKey)
  }

  const renderPromise = handleSsg(request).finally(() => {
    renderCoalesceMap.delete(coalesceKey)
  })
  renderCoalesceMap.set(coalesceKey, renderPromise)
  return renderPromise
}

// --- SSG Handler ---
// Renders a page at build time (or for ISR background revalidation).
// mode: "full" = wait for all content, "ppr" = shell only (Suspense fallbacks).
async function handleSsg(request) {
  const { projectRoot, appDir, pageFile, requestPath, params, mode, fresh } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureReactDeps(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const { outfile, freshBuild, dependencyHash, inputs } = await bundleSsgModule(
    resolvedRoot,
    pageFile,
    layouts,
    mode || 'full',
  )
  const mod = await importModule(outfile, freshBuild || fresh)
  const html = await mod.render({ path: requestPath, params: params || {} })

  return { ok: true, html, dependencyHash, inputs }
}

// --- Static parameter discovery ---
// Keep this in the persistent worker so build-time dynamic SSG routes reuse the
// same dependency checks, compiler cache, and module cache as page rendering.
async function handleStaticParams(request) {
  const { projectRoot, pageFile, routePath = '', segments = [], routes = [] } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureReactDeps(resolvedRoot)

  const cacheDir = path.join(resolvedRoot, '.ruvyxa', 'cache', 'ssg')
  await ensureDir(cacheDir)
  const moduleCode = `export { getStaticParams, staticParams } from ${JSON.stringify(toImportPath(pageFile))}`
  const hash = createHash('sha256')
    .update(moduleCode)
    .update(pageFile)
    .update('params')
    .digest('hex')
    .slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)
  const paramsCacheFile = path.join(cacheDir, `${hash}.params.json`)
  const cacheKey = `ssg-params:${pageFile}:${hash}`
  const contextHash = createHash('sha256')
    .update(JSON.stringify({ routePath, segments, routes }))
    .digest('hex')

  const { freshBuild, dependencyHash } = await withBuildLock(cacheKey, async () => {
    const cached = bundleCache.get(cacheKey)
    if (cached) {
      return {
        outfile: cached,
        freshBuild: false,
        dependencyHash: bundleFingerprints.get(cacheKey),
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot: resolvedRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:ssg-params-entry.ts',
      outfile,
      platform: 'node',
      external: ['react', 'react/jsx-runtime', 'react-dom/server', 'node:stream'],
      aliases: runtimeAliases(runtimeDir),
    })
    cacheBundle(cacheKey, outfile, resolvedRoot, bundle.inputs, bundle.dependencyHash)
    return { outfile, freshBuild: true, dependencyHash: bundle.dependencyHash }
  })

  const cachedParams = await readStaticParamsCache(paramsCacheFile, dependencyHash, contextHash)
  if (cachedParams) return { ok: true, params: cachedParams, cached: true }

  const mod = await importModule(outfile, freshBuild)
  const context = {
    routes,
    route: { path: routePath, segments },
  }
  const result =
    typeof mod.getStaticParams === 'function'
      ? await mod.getStaticParams(context)
      : mod.staticParams
  const normalized = normalizeStaticParams(result, segments)

  if (normalized.cacheSeconds !== null) {
    await writeStaticParamsCache(paramsCacheFile, {
      version: 1,
      dependencyHash,
      contextHash,
      expiresAt: Date.now() + normalized.cacheSeconds * 1000,
      params: normalized.params,
    })
  }

  return { ok: true, params: normalized.params, cached: false }
}

function normalizeStaticParams(result, segments) {
  let values = result
  let cacheSeconds = null
  if (result && typeof result === 'object' && !Array.isArray(result) && 'params' in result) {
    values = result.params
    cacheSeconds = parseStaticParamsCacheDuration(result.cache)
  }
  if (values === undefined) return { params: [], cacheSeconds }
  if (!Array.isArray(values)) {
    throw new Error('RUV1510 static params must be an array or an object with a params array')
  }

  const params = values.map((value, index) => {
    if (typeof value === 'string' || typeof value === 'number') {
      if (segments.length !== 1) {
        throw new Error(
          `RUV1511 static params shorthand at index ${index} requires exactly one dynamic route segment`,
        )
      }
      const segment = segments[0]
      const normalized = String(value)
      return { [segment.name]: segment.catchAll ? [normalized] : normalized }
    }
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      throw new Error(`RUV1512 static params entry at index ${index} must be an object or scalar`)
    }
    return value
  })
  return { params, cacheSeconds }
}

function parseStaticParamsCacheDuration(value) {
  if (value === undefined || value === null || value === false) return null
  let seconds
  if (typeof value === 'number') {
    seconds = value
  } else if (typeof value === 'string') {
    const match = /^(\d+)(s|m|h|d)$/.exec(value.trim())
    if (!match) {
      throw new Error('RUV1513 static params cache must use seconds or a duration like 10m')
    }
    const multipliers = { s: 1, m: 60, h: 3600, d: 86400 }
    seconds = Number(match[1]) * multipliers[match[2]]
  } else {
    throw new Error('RUV1513 static params cache must be a positive number or duration string')
  }
  if (!Number.isSafeInteger(seconds) || seconds <= 0 || seconds > 31_536_000) {
    throw new Error('RUV1513 static params cache must be between 1 second and 365 days')
  }
  return seconds
}

async function readStaticParamsCache(file, dependencyHash, contextHash) {
  if (!dependencyHash) return null
  try {
    const cached = JSON.parse(await readFile(file, 'utf8'))
    if (
      cached.version === 1 &&
      cached.dependencyHash === dependencyHash &&
      cached.contextHash === contextHash &&
      Number.isSafeInteger(cached.expiresAt) &&
      cached.expiresAt > Date.now() &&
      Array.isArray(cached.params)
    ) {
      return cached.params
    }
  } catch {
    // Missing, expired, or malformed cache entries are rebuilt below.
  }
  return null
}

async function writeStaticParamsCache(file, value) {
  const temporary = `${file}.${process.pid}.${Date.now()}.tmp`
  await writeFile(temporary, `${JSON.stringify(value)}\n`)
  try {
    await rename(temporary, file)
  } catch (error) {
    if (error?.code !== 'EEXIST' && error?.code !== 'EPERM') throw error
    await rm(file, { force: true })
    await rename(temporary, file)
  }
}

// --- API Handler ---
async function handleApi(request) {
  const {
    projectRoot,
    routeFile,
    method,
    requestPath,
    params,
    headers: requestHeaders = {},
    headerPairs,
    body: requestBody,
    bodyBase64,
    streamResponse,
  } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  const { outfile, freshBuild } = await bundleApiModule(resolvedRoot, routeFile)
  const mod = await importModule(outfile, freshBuild)
  const handler = mod[method.toUpperCase()]

  if (typeof handler !== 'function') {
    return {
      ok: true,
      status: 405,
      headers: { 'content-type': 'text/plain; charset=utf-8' },
      body: `Method ${method.toUpperCase()} is not allowed`,
    }
  }

  const upperMethod = method.toUpperCase()
  const requestInit = {
    method: upperMethod,
    // headerPairs preserves duplicate values; retain the object fallback for
    // older Rust workers that only send the legacy headers field.
    headers: Array.isArray(headerPairs) ? headerPairs : requestHeaders,
  }
  if (upperMethod !== 'GET' && upperMethod !== 'HEAD') {
    if (typeof bodyBase64 === 'string') {
      requestInit.body = Buffer.from(bodyBase64, 'base64')
    } else if (requestBody != null) {
      requestInit.body = requestBody
    }
  }
  const req = new Request(`http://localhost${requestPath}`, requestInit)
  const result = await handler({ request: req, params: params || {} })
  const response = normalizeResponse(result)
  const headerPairsResult = responseHeaderPairs(response)
  const headers = Object.fromEntries(headerPairsResult)

  if (streamResponse) {
    return {
      ok: true,
      status: response.status,
      headers,
      headerPairs: headerPairsResult,
      streamResponse: response,
    }
  }

  const body = await response.text()

  return { ok: true, status: response.status, headers, headerPairs: headerPairsResult, body }
}

function responseHeaderPairs(response) {
  const headerPairs = []
  for (const [name, value] of response.headers.entries()) {
    if (name !== 'set-cookie') headerPairs.push([name, value])
  }
  for (const value of response.headers.getSetCookie()) {
    headerPairs.push(['set-cookie', value])
  }
  return headerPairs
}

// --- Action Handler ---
async function handleAction(request) {
  const { projectRoot, actionFile, actionName, payloadJson, requestPath } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  const { outfile, freshBuild } = await bundleActionModule(resolvedRoot, actionFile)
  const mod = await importModule(outfile, freshBuild)
  const action = mod[actionName]

  if (typeof action !== 'function' || action.ruvyxa?.kind !== 'action') {
    return {
      ok: true,
      status: 404,
      headers: { 'content-type': 'application/json; charset=utf-8' },
      body: JSON.stringify({
        error: `Action ${actionName} was not found in ${path.basename(actionFile)}`,
      }),
    }
  }

  const input = parsePayload(payloadJson)
  const invalidated = []
  const req = new Request(`http://localhost${requestPath}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(input),
  })
  const result = await action(input, {
    request: req,
    invalidate(key) {
      invalidated.push(key)
    },
  })
  const response = normalizeActionResult(result, invalidated)
  const body = await response.text()
  const headers = Object.fromEntries(response.headers.entries())

  return { ok: true, status: response.status, headers, body }
}

// --- Client Bundle Handler ---
async function handleClient(request) {
  const { projectRoot, appDir, pageFile, requestPath, params } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const { outfile } = await bundleClientModule(
    resolvedRoot,
    pageFile,
    layouts,
    requestPath,
    JSON.stringify(params || {}),
  )
  const script = await readFile(outfile, 'utf8')

  return { ok: true, script }
}

// --- Bundle Cache Invalidation ---
function invalidateBundleCache(paths) {
  invalidateCompilerCache(paths)
  if (!paths || paths.length === 0) {
    const invalidated = bundleCache.size
    bundleCache.clear()
    bundleInputs.clear()
    bundleFingerprints.clear()
    moduleCache.clear()
    buildLocks.clear()
    return { invalidated }
  }
  const normalizedPaths = paths.map(normalizeAbsolutePath)
  let invalidated = 0
  for (const key of bundleCache.keys()) {
    const inputs = bundleInputs.get(key) ?? new Set()
    const entryMatches = normalizedPaths.some((changedPath) =>
      key.replaceAll('\\', '/').includes(changedPath),
    )
    const dependencyMatches = normalizedPaths.some((changedPath) => inputs.has(changedPath))
    if (entryMatches || dependencyMatches) {
      deleteBundleCacheEntry(key)
      invalidated++
    }
  }
  return { invalidated }
}

function positiveIntegerEnv(name, fallback, maximum = Number.MAX_SAFE_INTEGER) {
  const rawValue = (process.env[name] ?? '').trim()
  if (!/^\+?\d+$/.test(rawValue)) return fallback
  const value = Number(rawValue)
  return Number.isSafeInteger(value) && value > 0 && value <= maximum ? value : fallback
}

function normalizeAbsolutePath(file) {
  return path.resolve(file).replaceAll('\\', '/')
}

function cacheBundle(cacheKey, outfile, projectRoot, inputs, dependencyHash) {
  const evicted = bundleCache.set(cacheKey, outfile)
  if (evicted) {
    bundleInputs.delete(evicted.key)
    bundleFingerprints.delete(evicted.key)
    if (evicted.value) moduleCache.delete(evicted.value)
  }
  bundleInputs.set(
    cacheKey,
    new Set((inputs ?? []).map((input) => normalizeAbsolutePath(path.join(projectRoot, input)))),
  )
  if (dependencyHash) bundleFingerprints.set(cacheKey, dependencyHash)
}

function deleteBundleCacheEntry(cacheKey) {
  const outfile = bundleCache.delete(cacheKey)
  bundleInputs.delete(cacheKey)
  bundleFingerprints.delete(cacheKey)
  buildLocks.delete(cacheKey)
  if (outfile) moduleCache.delete(outfile)
}

// --- Shared Utilities ---
function collectLayouts(appDir, routeDir) {
  const layouts = []
  let current = appDir

  pushIfExists(layouts, path.join(current, 'layout.tsx'))

  const relative = path.relative(appDir, routeDir)
  if (relative && !relative.startsWith('..')) {
    for (const segment of relative.split(path.sep)) {
      if (!segment) continue
      current = path.join(current, segment)
      pushIfExists(layouts, path.join(current, 'layout.tsx'))
    }
  }

  return layouts
}

function pushIfExists(collection, file) {
  if (existsSync(file)) {
    collection.push(file)
  }
}

// --- Bundle functions now return { outfile, freshBuild } ---
// freshBuild=true means V8 module cache needs busting

async function bundleSsrModule(projectRoot, pageFile, layouts) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssr')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const moduleCode = `
import React from "react"
import { renderToPipeableStream } from "react-dom/server"
import { Writable } from "node:stream"
${imports.join('\n')}

export async function render(ctx) {
  let tree = React.createElement(Page, { params: ctx.params ?? {}, requestPath: ctx.path })
  for (const Layout of [${wrappers.join(', ')}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }

  return new Promise((resolve, reject) => {
    const chunks = []
    const writable = new Writable({
      write(chunk, encoding, callback) {
        chunks.push(chunk)
        callback()
      },
    })

    const { pipe } = renderToPipeableStream(tree, {
      onAllReady() {
        pipe(writable)
        writable.on("finish", () => {
          const html = Buffer.concat(chunks).toString("utf8")
          resolve(html.trimStart().toLowerCase().startsWith("<!doctype") ? html : "<!doctype html>" + html)
        })
      },
      onShellError(error) {
        reject(error)
      },
      onError(error) {
        if (process.env.RUVYXA_DEBUG) console.error("[ssr stream error]", error)
      },
    })
  })
}
`

  const hash = createHash('sha256').update(moduleCode).update(pageFile).digest('hex').slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  const cacheKey = `ssr:${pageFile}:${hash}`
  const cached = bundleCache.get(cacheKey)
  if (cached) return { outfile: cached, freshBuild: false }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) return { outfile: rechecked, freshBuild: false }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:ssr-entry.tsx',
      outfile,
      platform: 'node',
      external: ['react', 'react/jsx-runtime', 'react-dom/server', 'node:stream'],
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs)
    return { outfile, freshBuild: true }
  })
}

async function bundleApiModule(projectRoot, routeFile) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'api')
  await ensureDir(cacheDir)

  const moduleCode = `export * from ${JSON.stringify(toImportPath(routeFile))}`
  const hash = createHash('sha256').update(moduleCode).update(routeFile).digest('hex').slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  const cacheKey = `api:${routeFile}:${hash}`
  const cached = bundleCache.get(cacheKey)
  if (cached) return { outfile: cached, freshBuild: false }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) return { outfile: rechecked, freshBuild: false }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:api-entry.ts',
      outfile,
      platform: 'node',
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs)
    return { outfile, freshBuild: true }
  })
}

async function bundleActionModule(projectRoot, actionFile) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'actions')
  await ensureDir(cacheDir)

  const moduleCode = `export * from ${JSON.stringify(toImportPath(actionFile))}`
  const hash = createHash('sha256').update(moduleCode).update(actionFile).digest('hex').slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  const cacheKey = `action:${actionFile}:${hash}`
  const cached = bundleCache.get(cacheKey)
  if (cached) return { outfile: cached, freshBuild: false }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) return { outfile: rechecked, freshBuild: false }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:action-entry.ts',
      outfile,
      platform: 'node',
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs)
    return { outfile, freshBuild: true }
  })
}

async function bundleClientModule(projectRoot, pageFile, layouts, requestPath, paramsJson) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'client')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const moduleCode = `
import React from "react"
import { hydrateRoot } from "react-dom/client"
${imports.join('\n')}

const params = globalThis.__RUVYXA_ROUTE_PARAMS__ ?? ${paramsJson}
const currentRequestPath = globalThis.__RUVYXA_REQUEST_PATH__ ?? ${JSON.stringify(requestPath)}
let tree = React.createElement(Page, { params, requestPath: currentRequestPath })
for (const Layout of [${wrappers.join(', ')}].reverse()) {
  tree = React.createElement(Layout, null, tree)
}

if (globalThis.__RUVYXA_ROOT__) {
  globalThis.__RUVYXA_ROOT__.render(tree)
} else {
  globalThis.__RUVYXA_ROOT__ = hydrateRoot(document, tree)
}
window.__RUVYXA_HYDRATED = true
`

  const hash = createHash('sha256').update(moduleCode).update(pageFile).digest('hex').slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.js`)

  const cacheKey = `client:${pageFile}:${hash}`
  const cached = bundleCache.get(cacheKey)
  if (cached) return { outfile: cached, freshBuild: false }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) return { outfile: rechecked, freshBuild: false }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:client-entry.tsx',
      outfile,
      platform: 'browser',
      minify: process.env.RUVYXA_CLIENT_MINIFY === '1',
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs)
    return { outfile, freshBuild: true }
  })
}

// --- SSG Bundle ---
// Bundles a page for static generation. mode controls onShellReady vs onAllReady.
async function bundleSsgModule(projectRoot, pageFile, layouts, mode) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssg')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const readyEvent = mode === 'ppr' ? 'onShellReady' : 'onAllReady'

  const moduleCode = `
import React from "react"
import { renderToPipeableStream } from "react-dom/server"
import { Writable } from "node:stream"
${imports.join('\n')}

export async function render(ctx) {
  let tree = React.createElement(Page, { params: ctx.params ?? {}, requestPath: ctx.path })
  for (const Layout of [${wrappers.join(', ')}].reverse()) {
    tree = React.createElement(Layout, null, tree)
  }

  return new Promise((resolve, reject) => {
    const chunks = []
    const writable = new Writable({
      write(chunk, encoding, callback) {
        chunks.push(chunk)
        callback()
      },
    })

    const { pipe } = renderToPipeableStream(tree, {
      ${readyEvent}() {
        pipe(writable)
        writable.on("finish", () => {
          const html = Buffer.concat(chunks).toString("utf8")
          resolve(html.trimStart().toLowerCase().startsWith("<!doctype") ? html : "<!doctype html>" + html)
        })
      },
      onShellError(error) {
        reject(error)
      },
      onError(error) {
        ${mode === 'ppr' ? '// PPR: non-fatal streaming errors for dynamic slots' : 'reject(error)'}
      },
    })
  })
}
`

  const hash = createHash('sha256')
    .update(moduleCode)
    .update(pageFile)
    .update(mode)
    .digest('hex')
    .slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  const cacheKey = `ssg:${pageFile}:${hash}`
  const cached = bundleCache.get(cacheKey)
  if (cached) {
    return {
      outfile: cached,
      freshBuild: false,
      dependencyHash: bundleFingerprints.get(cacheKey),
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        freshBuild: false,
        dependencyHash: bundleFingerprints.get(cacheKey),
        inputs: [...(bundleInputs.get(cacheKey) ?? [])],
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:ssg-entry.tsx',
      outfile,
      platform: 'node',
      external: ['react', 'react/jsx-runtime', 'react-dom/server', 'node:stream'],
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, bundle.dependencyHash)
    return {
      outfile,
      freshBuild: true,
      dependencyHash: bundle.dependencyHash,
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
  })
}

function normalizeResponse(result) {
  if (result instanceof Response) return result
  return Response.json(result)
}

function normalizeActionResult(result, invalidated) {
  if (result instanceof Response) return result
  return Response.json({ data: result, invalidated })
}

function parsePayload(payloadJson) {
  let parsed
  try {
    parsed = JSON.parse(payloadJson || '{}')
  } catch {
    parsed = Object.fromEntries(new URLSearchParams(payloadJson))
  }
  if (parsed && typeof parsed === 'object' && 'input' in parsed) {
    return parsed.input
  }
  return parsed
}
