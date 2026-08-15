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
import { availableParallelism } from 'node:os'
import { createHash, randomUUID } from 'node:crypto'
import { once } from 'node:events'
import { existsSync } from 'node:fs'
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { createInterface } from 'node:readline'

import {
  collectRevalidations,
  requestContext,
  runWithRequestContext,
  usedRequestContext,
} from './request-context.mjs'
import {
  clearCompilerCache,
  collectSpecials,
  compileBundleWithMetadata,
  INSTRUMENTATION_FILES,
  invalidateCompilerCache,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'
import { clientEntrySource, metaSourceImports, nodeSsrEntrySource } from './entry-templates.mjs'
import { WorkerAdmissionController } from './worker-admission.mjs'

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

/**
 * Requests this worker will execute at once.
 *
 * `activeRequests` was counted but never used to gate admission, so a burst on
 * one worker started every render concurrently: each one holds a React tree, a
 * compiled bundle, and its response buffer, and enough of them together exhaust
 * the heap or thrash the CPU into timeouts that look like hangs. Queueing beyond
 * this point costs a little latency and keeps the ones already running fast.
 *
 * Renders are CPU-bound, so the useful width is the core count rather than a
 * large fixed number. The Rust pool bounds its stdin channel but has no view of
 * how much work is in flight inside a worker, which is why the limit belongs
 * here.
 */
const MAX_CONCURRENT_REQUESTS = positiveIntegerEnv(
  'RUVYXA_WORKER_MAX_CONCURRENCY',
  Math.max(2, Math.min(8, availableParallelism())),
)
// Keep overload memory finite. Each queued request retains its parsed payload,
// including request bodies and render metadata, until a slot becomes free.
// Four waiting requests per active slot absorbs short bursts without allowing
// sustained overload to grow the heap without bound.
const MAX_QUEUED_REQUESTS = positiveIntegerEnv(
  'RUVYXA_WORKER_MAX_QUEUE',
  MAX_CONCURRENT_REQUESTS * 4,
)
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
// Cache key -> content hash of the emitted bundle. This is the ESM import
// version token; see `importModule`.
const bundleVersions = new Map()
const buildLocks = new Map()

// Performance: Module import cache — avoids re-parsing JS on every request.
// Key: `<outfile>?<version>`, Value: imported module object.
// A rebuild that changes the emitted code changes the version and misses here.
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

// Every module URL this process has imported. Node's ESM loader never releases
// a loaded URL, so this set's size is the number of module graphs the process
// is retaining and can never free. Reported by `ping` so the cost is
// measurable rather than inferred from heap growth.
const registeredModuleUrls = new Set()

let isShuttingDown = false
let moduleImportVersion = 0
const admission = new WorkerAdmissionController({
  maxConcurrentRequests: MAX_CONCURRENT_REQUESTS,
  maxQueuedRequests: MAX_QUEUED_REQUESTS,
})

/**
 * Write a stderr line tagged with the severity this worker intends.
 *
 * stdout belongs to the NDJSON response protocol, so stderr carries everything
 * else — routine lifecycle notices alongside genuine failures. The host cannot
 * tell those apart from the text and used to log the whole channel as warnings.
 * Only the side that knows *why* it is writing can classify the line, so the
 * tag carries that decision across the pipe. Untagged output (a thrown stack,
 * an unhandled rejection) is still treated as a warning on the far side.
 *
 * Parsed by `parse_worker_stderr_tag` in `crates/ruvyxa_dev_server/src/worker_pool.rs`.
 */
function note(level, message) {
  process.stderr.write(`[ruvyxa:${level}] ${message}\n`)
}

// --- Graceful Shutdown ---
function shutdown(reason = 'unknown') {
  if (isShuttingDown) return
  isShuttingDown = true
  // The reason was previously discarded — `shutdown` took no parameter while
  // every caller passed one — which made a signal indistinguishable from a
  // closed stdin in the logs. It is `debug` because reaching here is the
  // normal end of a worker's life: the host closes stdin once a build or dev
  // session is done.
  note('debug', `worker shutting down (${reason})`)
  // Settle parked handlers so shutdown has no dangling admission promises.
  // Rust also observes the process exit and closes their pending responses.
  admission.close()
  if (admission.activeRequests === 0) process.exit(0)
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

  // Cheap requests must not queue behind renders: an invalidation or ping is
  // bookkeeping, and delaying it would leave workers serving stale bundles
  // precisely when the pool is busy.
  const needsSlot = request.type !== 'invalidate' && request.type !== 'ping'
  if (needsSlot) {
    const admitted = await admission.acquire()
    if (!admitted) {
      // `close()` settles parked handlers as false during shutdown. That is a
      // lifecycle event, not overload, and stdout may already be unavailable.
      if (isShuttingDown) return
      await writeWorkerMessage({
        id,
        ok: false,
        code: 'RUV1705',
        message: `JavaScript worker queue is full (${MAX_QUEUED_REQUESTS} waiting)`,
      })
      return
    }
    // The worker may have started shutting down while this request waited.
    if (isShuttingDown) {
      admission.release()
      return
    }
  }

  try {
    const result = await withTimeout(
      dispatchRequest(request),
      WORKER_REQUEST_TIMEOUT_MS,
      `Request ${request.type}:${id} timed out after ${WORKER_REQUEST_TIMEOUT_MS}ms`,
    )
    if (result?.streamResponse instanceof Response) {
      await emitApiStream(id, result)
    } else {
      await writeWorkerMessage({ id, ...result, retainedModuleUrls: registeredModuleUrls.size })
    }
  } catch (error) {
    try {
      await writeWorkerMessage({
        id,
        ...workerError(error),
        retainedModuleUrls: registeredModuleUrls.size,
      })
    } catch {
      shutdown('stdout-write-failed')
    }
  } finally {
    if (needsSlot) admission.release()
    if (isShuttingDown && admission.activeRequests === 0) process.exit(0)
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
        // Module graphs retained by Node's ESM registry for this process.
        retainedModuleUrls: registeredModuleUrls.size,
        // A persistently non-zero queue or rising rejection count means this
        // worker is the bottleneck.
        ...admission.snapshot(),
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
  await writeWorkerMessage({
    id,
    frame: 'api-start',
    ...head,
    retainedModuleUrls: registeredModuleUrls.size,
  })

  const reader = streamResponse.body?.getReader()
  if (!reader) {
    await writeWorkerMessage({
      id,
      frame: 'api-end',
      ok: true,
      retainedModuleUrls: registeredModuleUrls.size,
    })
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
    await writeWorkerMessage({
      id,
      frame: 'api-end',
      ok: true,
      retainedModuleUrls: registeredModuleUrls.size,
    })
  } catch (error) {
    try {
      await reader.cancel(error)
    } catch {
      // The source may already be closed; the protocol error below is authoritative.
    }
    await writeWorkerMessage({
      id,
      frame: 'api-error',
      ...workerError(error),
      retainedModuleUrls: registeredModuleUrls.size,
    })
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
//
// `version` is the content hash of the emitted bundle, not a counter. Node's
// ESM loader keeps every distinct module URL in its registry for the life of
// the process and offers no way to evict one, so a monotonic token leaks one
// whole module graph per rebuild — including the common dev case where a save
// invalidates the bundle cache but the recompiled output is byte-identical.
// Keying on content means only a genuinely changed bundle adds a registry
// entry, which is the floor Node's loader allows.
async function importModule(outfile, version) {
  const cacheKey = `${outfile}?${version}`
  const cached = moduleCache.get(cacheKey)
  if (cached) return cached
  const url = `${pathToFileURL(outfile).href}?v=${version}`
  // Every distinct URL costs one permanently retained module graph. Counting
  // them makes that cost observable through `ping` instead of only visible as
  // unexplained heap growth.
  registeredModuleUrls.add(url)
  const mod = await import(url)
  moduleCache.set(cacheKey, mod)
  return mod
}

/// Version token for a build-isolated import.
///
/// Production prerendering asks for a fresh module per path so page-module
/// state cannot leak between paths. That isolation requires a distinct URL, so
/// it keeps the monotonic counter and pays the registry cost deliberately.
function isolatedVersion(version) {
  return `${version}.${++moduleImportVersion}`
}

// --- Fast React resolution (cached per project root) ---
/**
 * Project roots whose `instrumentation.ts` has already run in this worker.
 *
 * Keyed by root rather than a single boolean because one worker process can
 * serve more than one project during tests, and because a `register()` that ran
 * for another root has not initialised this one.
 */
const instrumentedRoots = new Map()

/**
 * Run the project's `instrumentation.ts` once per worker process.
 *
 * The hook exists to install process-wide observability — an OpenTelemetry SDK,
 * an error reporter, a metrics exporter — which means it has to run *inside* the
 * process that renders, not in the CLI that spawned it. That is this worker, and
 * a worker learns its project root from its first request, so registration is
 * lazy rather than done at startup.
 *
 * A failure is reported and remembered as done. Retrying on every request would
 * turn one broken import into a per-request penalty, and refusing to serve would
 * make a telemetry misconfiguration take the site down — the wrong trade for a
 * hook whose whole purpose is to observe a site that is working.
 */
async function ensureInstrumentation(resolvedRoot) {
  const existing = instrumentedRoots.get(resolvedRoot)
  if (existing) return existing

  const pending = (async () => {
    const entry = INSTRUMENTATION_FILES.map((name) => path.join(resolvedRoot, name)).find(
      (candidate) => existsSync(candidate),
    )
    if (!entry) return

    try {
      const { outfile, version } = await bundleInstrumentationModule(resolvedRoot, entry)
      const mod = await importModule(outfile, version)
      if (typeof mod.register === 'function') {
        await mod.register()
      } else {
        note(
          'warn',
          `${path.basename(entry)} has no exported register() function; nothing was run.`,
        )
      }
    } catch (error) {
      note('error', `instrumentation failed: ${error instanceof Error ? error.stack : error}`)
    }
  })()

  instrumentedRoots.set(resolvedRoot, pending)
  return pending
}

async function bundleInstrumentationModule(projectRoot, entryFile) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'instrumentation')
  await ensureDir(cacheDir)

  const moduleCode = `export * from ${JSON.stringify(toImportPath(entryFile))}`
  const hash = createHash('sha256').update(moduleCode).update(entryFile).digest('hex').slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  const bundle = await compileBundleWithMetadata({
    projectRoot,
    entrySource: moduleCode,
    sourcefile: 'ruvyxa:instrumentation-entry.ts',
    outfile,
    platform: serverPlatform(),
    aliases: runtimeAliases(runtimeDir),
  })

  return { outfile, version: bundle.contentHash }
}

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
  // A page can observe every request header through `headers()`, not only
  // cookies and authorization. Coalescing requests with different observable
  // context would therefore return the first request's HTML to the second.
  // Hash the complete context to keep the map key bounded while preserving the
  // ordered duplicate values carried by the worker protocol.
  const coalesceKey = `ssr:${requestContextKey(request)}`

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
          const specials = route.appDir
            ? collectSpecials(route.appDir, path.dirname(route.pageFile))
            : null
          // Warmup must produce the same bundle a real request will ask for,
          // or the cache key differs and the warm module is never reused.
          const { outfile, version } = await bundleSsrModule(
            resolvedRoot,
            route.pageFile,
            layouts,
            route.routePath || '/',
            specials,
          )
          await importModule(outfile, version)
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

/**
 * Stable digest of everything an SSR render can observe from its request.
 * Header names are case-insensitive, but pair order and repeated values remain
 * intact because `Headers` can expose their combined ordering.
 */
function requestContextKey(request) {
  const headerPairs = Array.isArray(request.headerPairs)
    ? request.headerPairs.map(([name, value]) => [String(name).toLowerCase(), String(value)])
    : []
  return createHash('sha256')
    .update(
      JSON.stringify({
        projectRoot: String(request.projectRoot || ''),
        appDir: String(request.appDir || ''),
        pageFile: String(request.pageFile || ''),
        requestPath: String(request.requestPath || ''),
        requestTarget: String(request.requestTarget || request.requestPath || ''),
        routePath: String(request.routePath || request.requestPath || ''),
        params: request.params || {},
        method: String(request.method || 'GET').toUpperCase(),
        headerPairs,
      }),
    )
    .digest('hex')
}

// --- SSR Handler ---
async function handleSsr(request) {
  const { projectRoot, appDir, pageFile, requestPath, requestTarget, params, routePath } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureReactDeps(resolvedRoot)
  await ensureInstrumentation(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  // The route pattern, not the concrete URL: it keys the client-side route
  // registry, and a per-URL key would make every dynamic request a cache miss.
  const { outfile, version, inputs } = await bundleSsrModule(
    resolvedRoot,
    pageFile,
    layouts,
    routePath || requestPath,
    specials,
  )
  const mod = await importModule(outfile, version)
  const context = requestContext({
    headerPairs: request.headerPairs,
    method: request.method,
    url: requestTarget || requestPath,
  })
  const html = await runWithRequestContext(context, () =>
    mod.render({ path: requestPath, params: params || {} }),
  )

  // `requestScoped` tells the server this HTML belongs to one request and must
  // not enter a cache shared with other users. It is reported rather than
  // inferred: only the render knows whether it read a cookie.
  return { ok: true, html, requestScoped: usedRequestContext(context), inputs }
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
  const { projectRoot, appDir, pageFile, requestPath, params, mode, fresh, routePath } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  await ensureInstrumentation(resolvedRoot)
  ensureReactDeps(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const { outfile, version, dependencyHash, inputs } = await bundleSsgModule(
    resolvedRoot,
    pageFile,
    layouts,
    mode || 'full',
    routePath || requestPath,
    specials,
  )
  const mod = await importModule(outfile, fresh ? isolatedVersion(version) : version)
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

  const { version, dependencyHash } = await withBuildLock(cacheKey, async () => {
    const cached = bundleCache.get(cacheKey)
    if (cached) {
      return {
        outfile: cached,
        version: bundleVersions.get(cacheKey),
        dependencyHash: bundleFingerprints.get(cacheKey),
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot: resolvedRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:ssg-params-entry.ts',
      outfile,
      platform: serverPlatform(),
      external: ['react', 'react/jsx-runtime', 'react-dom/server', 'node:stream'],
      aliases: runtimeAliases(runtimeDir),
    })
    cacheBundle(
      cacheKey,
      outfile,
      resolvedRoot,
      bundle.inputs,
      bundle.dependencyHash,
      bundle.contentHash,
    )
    return { outfile, version: bundle.contentHash, dependencyHash: bundle.dependencyHash }
  })

  const cachedParams = await readStaticParamsCache(paramsCacheFile, dependencyHash, contextHash)
  if (cachedParams) return { ok: true, params: cachedParams, cached: true }

  const mod = await importModule(outfile, version)
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
  // `randomUUID` rather than a timestamp: two resolutions of the same route
  // landing in one millisecond produced one temporary path for both, so each
  // could rename a file the other was still writing. The `finally` is what keeps
  // a failed publish from leaving the temporary behind on every attempt.
  const temporary = `${file}.${process.pid}.${randomUUID()}.tmp`
  try {
    await writeFile(temporary, `${JSON.stringify(value)}\n`)
    try {
      await rename(temporary, file)
    } catch (error) {
      if (error?.code !== 'EEXIST' && error?.code !== 'EPERM') throw error
      await rm(file, { force: true })
      await rename(temporary, file)
    }
  } finally {
    await rm(temporary, { force: true })
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
  await ensureInstrumentation(resolvedRoot)
  const { outfile, version, inputs } = await bundleApiModule(resolvedRoot, routeFile)
  const mod = await importModule(outfile, version)
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
  // An API route already receives the `Request`, so the ambient accessors are
  // redundant there — but a helper shared with a page must not stop working
  // because it was called from a route handler instead.
  const context = requestContext({
    headerPairs,
    headers: requestHeaders,
    method: upperMethod,
    url: requestPath,
  })
  const result = await runWithRequestContext(context, () =>
    handler({ request: req, params: params || {} }),
  )
  const revalidate = collectRevalidations(context)
  const response = normalizeResponse(result)
  const headerPairsResult = responseHeaderPairs(response)
  const headers = Object.fromEntries(headerPairsResult)

  if (streamResponse) {
    return {
      ok: true,
      status: response.status,
      headers,
      headerPairs: headerPairsResult,
      revalidate,
      inputs,
      streamResponse: response,
    }
  }

  const body = await response.text()

  return {
    ok: true,
    status: response.status,
    headers,
    headerPairs: headerPairsResult,
    revalidate,
    inputs,
    body,
  }
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
  const {
    projectRoot,
    actionFile,
    actionName,
    payloadJson,
    contentType,
    requestPath,
    headers: requestHeaders = {},
    headerPairs,
  } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  await ensureInstrumentation(resolvedRoot)
  const { outfile, version } = await bundleActionModule(resolvedRoot, actionFile)
  const mod = await importModule(outfile, version)
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

  const input = parsePayload(payloadJson, contentType)
  const invalidated = []
  const req = new Request(`http://localhost${requestPath}`, {
    method: 'POST',
    // headerPairs preserves duplicate cookies and is the canonical protocol
    // representation. Keep the legacy map fallback for older Rust callers.
    headers: Array.isArray(headerPairs)
      ? headerPairs
      : {
          ...requestHeaders,
          'content-type': requestHeaders['content-type'] || contentType || 'application/json',
        },
    body: contentType === 'application/x-www-form-urlencoded' ? payloadJson : JSON.stringify(input),
  })
  const context = requestContext({
    headerPairs,
    headers: requestHeaders,
    method: 'POST',
    url: requestPath,
  })
  const result = await runWithRequestContext(context, () =>
    action(input, {
      request: req,
      invalidate(key) {
        invalidated.push(key)
      },
    }),
  )
  let response = normalizeActionResult(result, invalidated)
  const headersWithInternalEventRemoved = new Headers(response.headers)
  headersWithInternalEventRemoved.delete('x-ruvyxa-realtime-event')
  response = new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers: headersWithInternalEventRemoved,
  })
  const realtimeEvent =
    response.status >= 200 && response.status < 400
      ? actionRealtimeEvent(action, actionName, requestPath, invalidated)
      : null
  if (realtimeEvent) {
    response.headers.set(
      'x-ruvyxa-realtime-event',
      Buffer.from(JSON.stringify(realtimeEvent)).toString('base64url'),
    )
  }
  const body = await response.text()
  const headerPairsResult = responseHeaderPairs(response)
  const headers = Object.fromEntries(headerPairsResult)

  return {
    ok: true,
    status: response.status,
    headers,
    headerPairs: headerPairsResult,
    revalidate: collectRevalidations(context),
    body,
  }
}

function actionRealtimeEvent(action, actionName, requestPath, invalidated) {
  const configured = action.ruvyxa?.realtime
  if (!configured) return null
  if (!Array.isArray(configured.channels) || configured.channels.length > 16) {
    throw new TypeError(`Action ${actionName} has invalid realtime channel metadata`)
  }
  const channels = configured.channels.map((channel) => {
    if (typeof channel !== 'string' || !/^[A-Za-z0-9:._/-]{1,128}$/.test(channel)) {
      throw new TypeError(`Action ${actionName} has invalid realtime channel metadata`)
    }
    return channel
  })
  const pathname = new URL(requestPath, 'http://ruvyxa.local').pathname
  return {
    version: 1,
    type: 'action',
    channels: channels.length > 0 ? channels : [realtimeRouteChannel(pathname)],
    action: actionName,
    path: pathname.slice(0, 2048),
    invalidated: invalidated
      .filter((key) => typeof key === 'string' && key.length <= 256)
      .slice(0, 64),
  }
}

function realtimeRouteChannel(pathname) {
  const readable = `route:${pathname}`
  if (readable.length <= 128) return readable
  let hash = 0xcbf29ce484222325n
  for (const character of pathname) {
    hash ^= BigInt(character.codePointAt(0))
    hash = BigInt.asUintN(64, hash * 0x100000001b3n)
  }
  return `route-hash:${hash.toString(16).padStart(16, '0')}`
}

// --- Client Bundle Handler ---
async function handleClient(request) {
  const { projectRoot, appDir, pageFile, requestPath, params, routePath } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const { outfile, inputs } = await bundleClientModule(
    resolvedRoot,
    pageFile,
    layouts,
    requestPath,
    JSON.stringify(params || {}),
    routePath || requestPath,
    specials,
  )
  const script = await readFile(outfile, 'utf8')

  return { ok: true, script, inputs }
}

// --- Bundle Cache Invalidation ---
function invalidateBundleCache(paths) {
  invalidateCompilerCache(paths)
  if (!paths || paths.length === 0) {
    const invalidated = bundleCache.size
    bundleCache.clear()
    bundleInputs.clear()
    bundleFingerprints.clear()
    bundleVersions.clear()
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

function cacheBundle(cacheKey, outfile, projectRoot, inputs, dependencyHash, contentHash) {
  const evicted = bundleCache.set(cacheKey, outfile)
  if (evicted) {
    bundleInputs.delete(evicted.key)
    bundleFingerprints.delete(evicted.key)
    dropModuleCacheEntries(evicted.key, evicted.value)
    bundleVersions.delete(evicted.key)
  }
  if (contentHash) bundleVersions.set(cacheKey, contentHash)
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
  dropModuleCacheEntries(cacheKey, outfile)
  bundleVersions.delete(cacheKey)
}

// The module cache is keyed by `<outfile>?<version>`, so a bundle eviction has
// to drop the entry for the version that bundle was last built at.
function dropModuleCacheEntries(cacheKey, outfile) {
  if (!outfile) return
  const version = bundleVersions.get(cacheKey)
  if (version) moduleCache.delete(`${outfile}?${version}`)
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

// Turn a `collectSpecials` result into the import statements and component
// identifiers a generated entry needs. Absent kinds contribute nothing, so a
// route with no special files produces exactly the bundle it did before.
const SPECIAL_BINDINGS = [
  ['error', 'RouteError', 'errorName'],
  ['loading', 'RouteLoading', 'loadingName'],
  ['notFound', 'RouteNotFound', 'notFoundName'],
]

function specialEntryParts(specials) {
  const imports = []
  const names = { errorName: null, loadingName: null, notFoundName: null }
  for (const [kind, ident, nameKey] of SPECIAL_BINDINGS) {
    const file = specials?.[kind]
    if (file) {
      imports.push(`import ${ident} from ${JSON.stringify(toImportPath(file))}`)
      names[nameKey] = ident
    }
  }
  return { imports, names }
}

// --- Bundle functions return { outfile, version } ---
// `version` is the content hash of the emitted bundle and is the ESM import
// token. Identical output keeps the same token, so no new module URL is
// registered; changed output produces a new token and a genuine reload.

async function bundleSsrModule(projectRoot, pageFile, layouts, routePath = '/', specials = null) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssr')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const { imports: specialImports, names } = specialEntryParts(specials)
  imports.push(...specialImports)

  const { imports: metaImports, metaNames } = metaSourceImports(
    [...layouts, pageFile].map(toImportPath),
  )
  imports.push(...metaImports)

  const moduleCode = nodeSsrEntrySource({
    imports,
    pageName: 'Page',
    layoutNames: wrappers,
    routePath,
    readyEvent: 'onAllReady',
    tolerateStreamErrors: true,
    metaNames,
    ...names,
  })

  const hash = createHash('sha256').update(moduleCode).update(pageFile).digest('hex').slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.mjs`)

  const cacheKey = `ssr:${pageFile}:${hash}`
  const cached = bundleCache.get(cacheKey)
  if (cached) {
    return {
      outfile: cached,
      version: bundleVersions.get(cacheKey),
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        inputs: [...(bundleInputs.get(cacheKey) ?? [])],
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:ssr-entry.tsx',
      outfile,
      platform: serverPlatform(),
      external: ['react', 'react/jsx-runtime', 'react-dom/server', 'node:stream'],
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, null, bundle.contentHash)
    return {
      outfile,
      version: bundle.contentHash,
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
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
  if (cached) {
    return {
      outfile: cached,
      version: bundleVersions.get(cacheKey),
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        inputs: [...(bundleInputs.get(cacheKey) ?? [])],
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:api-entry.ts',
      outfile,
      platform: serverPlatform(),
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, null, bundle.contentHash)
    return {
      outfile,
      version: bundle.contentHash,
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
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
  if (cached) return { outfile: cached, version: bundleVersions.get(cacheKey) }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) return { outfile: rechecked, version: bundleVersions.get(cacheKey) }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:action-entry.ts',
      outfile,
      platform: serverPlatform(),
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, null, bundle.contentHash)
    return { outfile, version: bundle.contentHash }
  })
}

async function bundleClientModule(
  projectRoot,
  pageFile,
  layouts,
  requestPath,
  paramsJson,
  routePath = requestPath,
  specials = null,
) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'client')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const { imports: specialImports, names } = specialEntryParts(specials)
  imports.push(...specialImports)

  const { imports: metaImports, metaNames } = metaSourceImports(
    [...layouts, pageFile].map(toImportPath),
  )
  imports.push(...metaImports)

  const moduleCode = clientEntrySource({
    imports,
    pageName: 'Page',
    layoutNames: wrappers,
    routePath,
    requestPathLiteral: JSON.stringify(requestPath),
    paramsLiteral: paramsJson,
    metaNames,
    ...names,
  })

  const hash = createHash('sha256').update(moduleCode).update(pageFile).digest('hex').slice(0, 16)
  const outfile = path.join(cacheDir, `${hash}.js`)

  const cacheKey = `client:${pageFile}:${hash}`
  const cached = bundleCache.get(cacheKey)
  if (cached) {
    return {
      outfile: cached,
      version: bundleVersions.get(cacheKey),
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        inputs: [...(bundleInputs.get(cacheKey) ?? [])],
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:client-entry.tsx',
      outfile,
      platform: 'browser',
      minify: process.env.RUVYXA_CLIENT_MINIFY === '1',
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, null, bundle.contentHash)
    return {
      outfile,
      version: bundle.contentHash,
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
  })
}

// --- SSG Bundle ---
// Bundles a page for static generation. mode controls onShellReady vs onAllReady.
async function bundleSsgModule(
  projectRoot,
  pageFile,
  layouts,
  mode,
  routePath = '/',
  specials = null,
) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssg')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const wrappers = []

  layouts.forEach((layoutFile, index) => {
    imports.push(`import Layout${index} from ${JSON.stringify(toImportPath(layoutFile))}`)
    wrappers.push(`Layout${index}`)
  })

  const { imports: specialImports, names } = specialEntryParts(specials)
  imports.push(...specialImports)

  const { imports: metaImports, metaNames } = metaSourceImports(
    [...layouts, pageFile].map(toImportPath),
  )
  imports.push(...metaImports)

  const moduleCode = nodeSsrEntrySource({
    imports,
    pageName: 'Page',
    layoutNames: wrappers,
    routePath,
    // A partial prerender commits the static shell as soon as it is ready and
    // lets the dynamic slots stream in behind their Suspense boundaries.
    readyEvent: mode === 'ppr' ? 'onShellReady' : 'onAllReady',
    tolerateStreamErrors: mode === 'ppr',
    metaNames,
    ...names,
  })

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
      version: bundleVersions.get(cacheKey),
      dependencyHash: bundleFingerprints.get(cacheKey),
      inputs: [...(bundleInputs.get(cacheKey) ?? [])],
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        dependencyHash: bundleFingerprints.get(cacheKey),
        inputs: [...(bundleInputs.get(cacheKey) ?? [])],
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:ssg-entry.tsx',
      outfile,
      platform: serverPlatform(),
      external: ['react', 'react/jsx-runtime', 'react-dom/server', 'node:stream'],
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(
      cacheKey,
      outfile,
      projectRoot,
      bundle.inputs,
      bundle.dependencyHash,
      bundle.contentHash,
    )
    return {
      outfile,
      version: bundle.contentHash,
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

function parsePayload(payloadJson, contentType) {
  // `contentType` is additive for compatibility with older Rust workers. New
  // workers always send it, preventing content-type confusion between JSON
  // and URL-encoded action inputs.
  let parsed
  if (contentType === 'application/json') {
    parsed = JSON.parse(payloadJson || '{}')
  } else if (contentType === 'application/x-www-form-urlencoded') {
    parsed = Object.fromEntries(new URLSearchParams(payloadJson || ''))
  } else {
    try {
      parsed = JSON.parse(payloadJson || '{}')
    } catch {
      parsed = Object.fromEntries(new URLSearchParams(payloadJson))
    }
  }
  if (parsed && typeof parsed === 'object' && 'input' in parsed) {
    return parsed.input
  }
  return parsed
}
