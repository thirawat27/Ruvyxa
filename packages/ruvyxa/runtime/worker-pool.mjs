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
  clientVendorEntrySource,
  clientVendorSpecifier,
  clientVendorUrls,
  collectLayouts,
  collectSlots,
  collectSpecials,
  collectTemplates,
  compileBundleWithMetadata,
  compilerCacheStats,
  hasModuleDirective,
  INSTRUMENTATION_FILES,
  invalidateCompilerCache,
  runtimeAliases,
  serverPlatform,
  toImportPath,
} from './compiler.mjs'
import { cache as serverCache } from '@ruvyxa/core/server'
// The realtime event an action publishes is validated in one place, not two.
// This host and `serverless-handler.mjs` both build it, and both are JavaScript
// in this package, so the rule that decides which channel names an action may
// address — and how far its metadata is truncated — is shared rather than
// mirrored. The copy that used to live here spelled the same limits a different
// way (`{1,128}` for the length check, a literal `128` for the channel cap),
// which is the state a rule is in just before it drifts.
import { actionRealtimeEvent } from './action-runtime.mjs'
import {
  clientEntrySource,
  metaSourceImports,
  nodeSsrEntrySource,
  rscActionEntrySource,
  rscClientEntrySource,
  rscServerEntrySource,
  wrapperEntryParts,
} from './entry-templates.mjs'
import {
  RSC_BROWSER_PACKAGE,
  RSC_SSR_PACKAGE,
  clientRegistrySource,
  mergeServerReferences,
  serverManifest,
  serverProxyModuleSource,
} from './client-references.mjs'
import {
  flightManifest,
  readStreamText,
  renderServerComponents,
  renderServerComponentsStream,
} from './server-components.mjs'
import { collectIntercepts } from './route-intercepts.mjs'
import { WorkerAdmissionController } from './worker-admission.mjs'
import { CachePressureController, LruCache } from './cache-budget.mjs'
import { encodeFlightPayload } from './flight.mjs'
import { compareEntryKeys } from './order.mjs'

/**
 * Names a page may use to declare its static parameter set, most specific
 * first.
 *
 * `generateStaticParams` is Next.js's name for the same export with the same
 * contract, and accepting it removes a silent failure: a page brought over
 * declared its parameters, nothing recognised the name, and the route was
 * served dynamically with no diagnostic anywhere.
 *
 * Held to the same list as `STATIC_PARAMS_EXPORTS` in
 * `crates/ruvyxa_graph/src/lib.rs`, which decides whether a route *has* static
 * params; this decides what to call when it does. Recognising a name in one and
 * not the other is a route that discovers as SSG and then pre-renders nothing.
 */
const STATIC_PARAMS_EXPORTS = ['getStaticParams', 'staticParams', 'generateStaticParams']

// --- Configuration ---
const MAX_BUNDLE_CACHE_ENTRIES = positiveIntegerEnv('RUVYXA_CACHE_MAX_ENTRIES', 256)
const MAX_NODE_TIMEOUT_MS = 2_147_483_647
const WORKER_REQUEST_TIMEOUT_MS = positiveIntegerEnv(
  'RUVYXA_WORKER_TIMEOUT_MS',
  30_000,
  MAX_NODE_TIMEOUT_MS,
)
const MEMORY_PRESSURE_THRESHOLD_MB = positiveIntegerEnv('RUVYXA_MEMORY_LIMIT_MB', 512)
const memoryPressure = new CachePressureController({
  hardLimitBytes: MEMORY_PRESSURE_THRESHOLD_MB * 1024 * 1024,
})
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

// --- State ---
const bundleCache = new LruCache(MAX_BUNDLE_CACHE_ENTRIES)
// Cache key -> normalized absolute project files used to build that bundle.
const bundleInputs = new Map()
// Cache key -> input paths that were directories when the bundle was built.
// Keeping this fact separately makes deletion/rename invalidation reliable:
// by the time the watcher reports a deletion, stat() can no longer classify it.
const bundleInputDirectories = new Map()
// Cache key -> hash of the normalized dependency path set. This is distinct
// from emitted-code `bundleVersions`: HMR graph ownership changes only when
// membership changes, even if an edit changes output bytes (or tree-shakes to
// the same output).
const bundleInputVersions = new Map()
const bundleFingerprints = new Map()
// Cache key -> content hash of the emitted bundle. This is the ESM import
// version token; see `importModule`.
const bundleVersions = new Map()
const buildLocks = new Map()

// Performance: Module import cache — avoids re-parsing JS on every request.
// Key: `<outfile>?<version>`, Value: imported module object.
// A rebuild that changes the emitted code changes the version and misses here.
const moduleCache = new LruCache(MAX_BUNDLE_CACHE_ENTRIES)

// Performance: Track directories already created to skip mkdir syscalls.
const createdDirs = new Set()

// Performance: Cache React dependency resolution per project root.
const reactResolvedRoots = new Set()

// Same, for the optional server-components runtime. Separate from the set
// above because an app that never opts in should never be asked for it.
const serverComponentDepsChecked = new Set()

// Cache key of a `react-server` bundle -> the `'use client'` modules that graph
// turned into references. Kept beside `bundleCache` rather than inside it
// because the browser build needs the list without recompiling the server
// graph, and a second scan of the route's imports would be a second answer to
// which modules are client modules.
const rscBundleReferences = new Map()

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
function enforceWorkerCacheBudget() {
  const heapUsed = process.memoryUsage().heapUsed
  const pressure = memoryPressure.observe(heapUsed)

  // Sweeping runs only under pressure, but the reading is returned either way:
  // the speculation check at the call site needs `stopSpeculation` on every
  // tick, not just the ticks that evicted something.
  if (pressure.level !== 'none') {
    const protectedKeys = new Set(buildLocks.keys())
    const fraction = Math.min(1, pressure.toFreeBytes / Math.max(heapUsed, 1))
    const requested = Math.max(1, Math.ceil(bundleCache.size * fraction))
    let bundleEvictions = 0
    for (let index = 0; index < requested; index++) {
      const evicted = bundleCache.evictOldest(protectedKeys)
      if (!evicted) break
      deleteBundleCacheEntry(evicted.key, evicted.value)
      bundleEvictions++
    }
    memoryPressure.recordEviction('bundle', bundleEvictions)
    memoryPressure.recordEviction('module', moduleCache.clear())
    clearCompilerCache()
    memoryPressure.recordEviction('compilerSweep')
  }

  return pressure
}

const memoryCheckInterval = setInterval(enforceWorkerCacheBudget, 30_000)
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
    case 'flight':
      return handleFlight(request)
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
    case 'clientVendor':
      return handleClientVendor(request)
    case 'rscClientEntry':
      return handleServerComponentsEntry(request)
    case 'rscPayload':
      // A soft navigation into a server-components route. The browser already
      // has a document, so only the payload is rendered.
      return handleServerComponents(request, { html: false })
    case 'rscAction':
      return handleRscAction(request)
    case 'rscDocument':
      return handleServerComponentsDocument(request)
    case 'warmup':
      if (enforceWorkerCacheBudget().stopSpeculation) {
        return { ok: true, warmed: 0, skipped: 'memory-pressure' }
      }
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
        cacheBudget: memoryPressure.snapshot(process.memoryUsage().heapUsed),
        compilerCache: compilerCacheStats(),
      }
    case 'invalidate':
      return { ok: true, traceId: request.traceId, ...invalidateBundleCache(request.paths) }
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

/**
 * Frame a streamed response as `api-start`, `api-chunk`s, and `api-end`.
 *
 * `streamTrailer` is for a value that does not exist yet when the first frame
 * has to go out. A server-components document is the case: its Flight payload
 * is only complete once the render is, and the host needs it to write the data
 * block the browser hydrates from — which belongs at the *end* of the document
 * anyway. The thunk is awaited after the last chunk and merged into `api-end`.
 */
async function emitApiStream(id, result) {
  const { streamResponse, streamTrailer, ...head } = result
  await writeWorkerMessage({
    id,
    frame: 'api-start',
    ...head,
    retainedModuleUrls: registeredModuleUrls.size,
  })

  const endFrame = async () => ({
    id,
    frame: 'api-end',
    ok: true,
    ...(streamTrailer ? await streamTrailer() : null),
    retainedModuleUrls: registeredModuleUrls.size,
  })

  const reader = streamResponse.body?.getReader()
  if (!reader) {
    await writeWorkerMessage(await endFrame())
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
    await writeWorkerMessage(await endFrame())
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

/**
 * Import a module and assert it exposes a callable `render` export.
 *
 * On Windows CI and high-parallelism builds the output file can briefly contain
 * a partial write when an isolated import races against `writeIfChanged`. When
 * that happens the ESM loader evaluates an incomplete (or empty) module that
 * lacks the expected `render` export.
 *
 * This helper detects the condition, evicts the broken cache entry, waits
 * briefly for the file system to settle, then re-imports once. If the retry
 * still fails it throws a diagnostic error listing the exports that were
 * actually present so the root cause is visible in CI logs.
 */
async function importRenderModule(outfile, version, pageFile) {
  const mod = await importModule(outfile, version)
  if (typeof mod.render === 'function') return mod

  // Evict the broken entry from the module cache so a retry gets a fresh read.
  const cacheKey = `${outfile}?${version}`
  moduleCache.delete(cacheKey)

  // Brief pause gives the filesystem time to flush a concurrent write.
  await new Promise((resolve) => setTimeout(resolve, 50))

  const retryVersion = isolatedVersion(version)
  const retried = await importModule(outfile, retryVersion)
  if (typeof retried.render === 'function') return retried

  const available = Object.keys(retried).filter((k) => typeof retried[k] === 'function')
  throw new Error(
    `mod.render is not a function for ${pageFile}. ` +
      `The bundled module at ${outfile} exports: [${available.join(', ') || 'none'}]. ` +
      `This usually indicates a partial file write or bundler cache corruption.`,
  )
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
  return coalesce(`ssr:${requestContextKey(request)}`, () => handleSsr(request))
}

/**
 * Run `render` once per key, handing every concurrent caller the same promise.
 *
 * The SSR and SSG entry points each had this written out. The mechanism is not
 * where they differ — the *key* is, and that is the only part worth reading
 * twice: what makes two requests the same page is a per-strategy decision, and
 * getting it too coarse returns one caller's HTML to another. Keeping the
 * bookkeeping here leaves each caller owning just that.
 *
 * The entry is removed in `finally`, so a rejected render is not cached as the
 * answer for the next request.
 */
function coalesce(coalesceKey, render) {
  const inFlight = renderCoalesceMap.get(coalesceKey)
  if (inFlight) return inFlight
  const pending = render().finally(() => renderCoalesceMap.delete(coalesceKey))
  renderCoalesceMap.set(coalesceKey, pending)
  return pending
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
          const templates = route.appDir
            ? collectTemplates(route.appDir, path.dirname(route.pageFile))
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
            templates,
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
  // A server-components route renders through two graphs and returns a payload
  // beside its HTML, so it takes its own path from here rather than a branch
  // inside a render that assumes one graph.
  if (request.serverComponents) return handleServerComponents(request)

  const { projectRoot, appDir, pageFile, requestPath, requestTarget, params, routePath } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureReactDeps(resolvedRoot)
  await ensureInstrumentation(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  // The route pattern, not the concrete URL: it keys the client-side route
  // registry, and a per-URL key would make every dynamic request a cache miss.
  const { outfile, version, inputsVersion, inputs } = await bundleSsrModule(
    resolvedRoot,
    pageFile,
    layouts,
    routePath || requestPath,
    specials,
    { templates, slots },
  )
  const mod = await importRenderModule(outfile, version, pageFile)
  const context = requestContext({
    headerPairs: request.headerPairs,
    method: request.method,
    url: requestTarget || requestPath,
    params: params || {},
  })
  const html = await runWithRequestContext(context, () =>
    mod.render({ path: requestPath, params: params || {} }),
  )

  // `requestScoped` tells the server this HTML belongs to one request and must
  // not enter a cache shared with other users. It is reported rather than
  // inferred: only the render knows whether it read a cookie.
  return {
    ok: true,
    html,
    requestScoped: usedRequestContext(context),
    inputsVersion,
    inputs,
  }
}

/** Render one public, version-bound Flight payload through the SSR module graph. */
async function handleFlight(request) {
  const { projectRoot, appDir, pageFile, requestPath, params, routePath, artifactVersion } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  await ensureInstrumentation(resolvedRoot)
  ensureReactDeps(resolvedRoot)
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const { outfile, version, inputsVersion, inputs } = await bundleSsrModule(
    resolvedRoot,
    pageFile,
    layouts,
    routePath || requestPath,
    specials,
    { templates, slots },
  )
  const mod = await importModule(outfile, version)
  const context = requestContext({
    method: 'GET',
    url: requestPath,
    headerPairs: [],
    params: params || {},
  })
  const source = await readFile(pageFile, 'utf8')
  const usesCache = hasModuleDirective(source, 'use cache')
  const produce = () => mod.flight({ path: requestPath, params: params || {} })
  const tree = await runWithRequestContext(context, () =>
    usesCache
      ? serverCache(flightCacheKey(routePath || requestPath, requestPath, params)).get(produce)
      : produce(),
  )
  if (usedRequestContext(context)) {
    throw new Error('RUV1840 Flight payload read private request state')
  }
  return {
    ok: true,
    flight: encodeFlightPayload({
      manifestVersion: artifactVersion,
      route: requestPath,
      tree,
    }),
    inputsVersion,
    inputs,
  }
}

function flightCacheKey(routePath, requestPath, params) {
  const sortedParams = Object.fromEntries(Object.entries(params || {}).sort(compareEntryKeys))
  return `flight:${JSON.stringify([routePath, requestPath, sortedParams])}`
}

// --- SSG Handler with Request Coalescing ---
async function handleSsgCoalesced(request) {
  const { pageFile, requestPath, params, mode, fresh } = request
  // Params are sorted for the same reason `flightCacheKey` sorts them: the two
  // objects describe the same page whichever order their keys were built in,
  // and an unsorted key silently answers that they are different renders.
  const sortedParams = Object.fromEntries(Object.entries(params || {}).sort(compareEntryKeys))
  const coalesceKey = `ssg:${pageFile}:${requestPath}:${JSON.stringify(sortedParams)}:${mode || 'full'}:${fresh ? 'fresh' : 'cached'}`
  return coalesce(coalesceKey, () => handleSsg(request))
}

// --- SSG Handler ---
// Renders a page at build time (or for ISR background revalidation).
// mode: "full" = wait for all content, "ppr" = shell only (Suspense fallbacks).
async function handleSsg(request) {
  // Pre-rendering a server-components route is the same render, written to a
  // file instead of a response. The payload travels with the HTML so the file
  // on disk is complete: nothing runs a renderer when it is later served.
  if (request.serverComponents) return handleServerComponents(request, { fresh: request.fresh })

  const { projectRoot, appDir, pageFile, requestPath, params, mode, fresh, routePath } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  await ensureInstrumentation(resolvedRoot)
  ensureReactDeps(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const { outfile, version, dependencyHash, inputsVersion, inputs } = await bundleSsgModule(
    resolvedRoot,
    pageFile,
    layouts,
    mode || 'full',
    routePath || requestPath,
    specials,
    { templates, slots },
  )
  const mod = await importRenderModule(
    outfile,
    fresh ? isolatedVersion(version) : version,
    pageFile,
  )
  const html = await mod.render({ path: requestPath, params: params || {} })

  return { ok: true, html, dependencyHash, inputsVersion, inputs }
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
  const moduleCode = `export { ${STATIC_PARAMS_EXPORTS.join(', ')} } from ${JSON.stringify(toImportPath(pageFile))}`
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
  let result
  for (const name of STATIC_PARAMS_EXPORTS) {
    const declared = mod[name]
    if (declared === undefined) continue
    result = typeof declared === 'function' ? await declared(context) : declared
    break
  }
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
    const match = /^(\d+)([smhd])$/.exec(value.trim())
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
  const { outfile, version, inputsVersion, inputs } = await bundleApiModule(resolvedRoot, routeFile)
  const mod = await importModule(outfile, version)
  const handler = mod[method.toUpperCase()]
  const dependencyMetadata = dependencyResponseMetadata(
    inputsVersion,
    inputs,
    request.knownInputsVersion,
  )

  if (typeof handler !== 'function') {
    return {
      ok: true,
      status: 405,
      headers: { 'content-type': 'text/plain; charset=utf-8' },
      body: `Method ${method.toUpperCase()} is not allowed`,
      ...dependencyMetadata,
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
    params: params || {},
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
      ...dependencyMetadata,
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
    ...dependencyMetadata,
    body,
  }
}

function dependencyResponseMetadata(inputsVersion, inputs, knownInputsVersion) {
  if (knownInputsVersion === inputsVersion) return { inputsVersion }
  return { inputsVersion, inputs }
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
  const { outfile, version, inputsVersion, inputs } = await bundleActionModule(
    resolvedRoot,
    actionFile,
  )
  const mod = await importModule(outfile, version)
  const action = mod[actionName]
  const dependencyMetadata = dependencyResponseMetadata(
    inputsVersion,
    inputs,
    request.knownInputsVersion,
  )

  if (typeof action !== 'function' || action.ruvyxa?.kind !== 'action') {
    return {
      ok: true,
      status: 404,
      headers: { 'content-type': 'application/json; charset=utf-8' },
      body: JSON.stringify({
        error: `Action ${actionName} was not found in ${path.basename(actionFile)}`,
      }),
      ...dependencyMetadata,
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
    ...dependencyMetadata,
    body,
  }
}

// --- Client Bundle Handler ---
async function handleClient(request) {
  if (request.serverComponents) return handleServerComponentsClient(request)

  const { projectRoot, appDir, pageFile, requestPath, params, routePath } = request

  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const intercepts = collectIntercepts(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const { outfile, inputsVersion, inputs } = await bundleClientModule(
    resolvedRoot,
    pageFile,
    layouts,
    requestPath,
    JSON.stringify(params || {}),
    routePath || requestPath,
    specials,
    { templates, slots, intercepts },
  )
  const script = await readFile(outfile, 'utf8')

  return { ok: true, script, inputsVersion, inputs }
}

/**
 * Compile one shared browser module — React and its family — for development.
 *
 * Built with its *siblings* rewritten to their own URLs, so the browser holds
 * one instance of each no matter how many route bundles import them. A build
 * achieves the same thing with a shared chunk; development compiles per route
 * on demand and has no cross-route analysis to build one from.
 */
async function handleClientVendor(request) {
  const { projectRoot, name } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  const specifier = clientVendorSpecifier(name)
  if (!specifier) {
    const error = new Error(`RUV1305 unknown shared browser module ${JSON.stringify(name)}`)
    error.code = 'RUV1305'
    throw error
  }
  ensureReactDeps(resolvedRoot)

  const cacheDir = path.join(resolvedRoot, '.ruvyxa', 'cache', 'client')
  await ensureDir(cacheDir)
  // `export *` cannot enumerate a CommonJS module's names, so the shared module
  // publishes its exports object on a registry instead.
  const moduleCode = clientVendorEntrySource(specifier)
  const hash = createHash('sha256').update(moduleCode).digest('hex').slice(0, 16)
  const outfile = path.join(cacheDir, `vendor.${name}.${hash}.js`)
  const cacheKey = `vendor:${name}:${hash}`

  const cached = bundleCache.get(cacheKey)
  if (cached) return { ok: true, script: await readFile(cached, 'utf8') }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) return { ok: true, script: await readFile(rechecked, 'utf8') }

    const bundle = await compileBundleWithMetadata({
      projectRoot: resolvedRoot,
      entrySource: moduleCode,
      sourcefile: `ruvyxa:vendor-${name}.ts`,
      outfile,
      platform: 'browser',
      minify: process.env.RUVYXA_CLIENT_MINIFY === '1',
      externalUrls: clientVendorUrls(specifier),
      aliases: runtimeAliases(runtimeDir),
    })
    cacheBundle(cacheKey, outfile, resolvedRoot, bundle.inputs, null, bundle.contentHash)
    return { ok: true, script: await readFile(outfile, 'utf8') }
  })
}

// --- Bundle Cache Invalidation ---
function invalidateBundleCache(paths) {
  invalidateCompilerCache(paths)
  if (!paths || paths.length === 0) {
    const invalidated = bundleCache.size
    bundleCache.clear()
    bundleInputs.clear()
    bundleInputDirectories.clear()
    bundleInputVersions.clear()
    bundleFingerprints.clear()
    bundleVersions.clear()
    rscBundleReferences.clear()
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
    const directories = bundleInputDirectories.get(key) ?? new Set()
    const dependencyMatches = normalizedPaths.some(
      (changedPath) =>
        inputs.has(changedPath) ||
        [...directories].some((input) => changedPath.startsWith(`${input}/`)),
    )
    if (entryMatches || dependencyMatches) {
      deleteBundleCacheEntry(key)
      rscBundleReferences.delete(key)
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
    bundleInputDirectories.delete(evicted.key)
    bundleInputVersions.delete(evicted.key)
    bundleFingerprints.delete(evicted.key)
    dropModuleCacheEntries(evicted.key, evicted.value)
    bundleVersions.delete(evicted.key)
  }
  if (contentHash) bundleVersions.set(cacheKey, contentHash)
  const normalizedInputs = new Set(
    (inputs ?? []).map((input) => normalizeAbsolutePath(path.join(projectRoot, input))),
  )
  bundleInputs.set(cacheKey, normalizedInputs)
  bundleInputDirectories.set(
    cacheKey,
    new Set(
      [...normalizedInputs].filter((input) => {
        try {
          return statSync(input).isDirectory()
        } catch {
          return false
        }
      }),
    ),
  )
  bundleInputVersions.set(cacheKey, inputsVersionOf(normalizedInputs))
  if (dependencyHash) bundleFingerprints.set(cacheKey, dependencyHash)
}

/**
 * The short token that stands for "this exact set of input files".
 *
 * A function rather than an expression inside `cacheBundle` because a second
 * caller now needs it: a response describing more than one compile reports the
 * union of their inputs, and a version computed a different way there would
 * claim two different things about the same list. Order is normalized, so the
 * token answers to the set and not to the order it was collected in.
 */
function inputsVersionOf(inputs) {
  return createHash('sha256')
    .update([...inputs].sort().join('\0'))
    .digest('hex')
    .slice(0, 16)
}

function deleteBundleCacheEntry(cacheKey, knownOutfile) {
  const outfile = knownOutfile ?? bundleCache.delete(cacheKey)
  bundleInputs.delete(cacheKey)
  bundleInputDirectories.delete(cacheKey)
  bundleInputVersions.delete(cacheKey)
  bundleFingerprints.delete(cacheKey)
  buildLocks.delete(cacheKey)
  dropModuleCacheEntries(cacheKey, outfile)
  bundleVersions.delete(cacheKey)
}

function bundleInputMetadata(cacheKey) {
  return {
    inputsVersion: bundleInputVersions.get(cacheKey),
    inputs: [...(bundleInputs.get(cacheKey) ?? [])],
  }
}

// The module cache is keyed by `<outfile>?<version>`, so a bundle eviction has
// to drop the entry for the version that bundle was last built at.
function dropModuleCacheEntries(cacheKey, outfile) {
  if (!outfile) return
  const version = bundleVersions.get(cacheKey)
  if (version) moduleCache.delete(`${outfile}?${version}`)
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

async function bundleSsrModule(
  projectRoot,
  pageFile,
  layouts,
  routePath = '/',
  specials = null,
  nested = {},
) {
  const { templates = [], slots = [] } = nested
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssr')
  await ensureDir(cacheDir)

  const imports = [`import Page, * as PageModule from ${JSON.stringify(toImportPath(pageFile))}`]
  const {
    imports: wrapperImports,
    layoutNames: wrappers,
    levels,
  } = wrapperEntryParts(layouts, templates, slots)
  imports.push(...wrapperImports)

  const { imports: specialImports, names } = specialEntryParts(specials)
  imports.push(...specialImports)

  const { imports: metaImports, metaNames } = metaSourceImports(
    [...layouts, pageFile].map(toImportPath),
  )
  imports.push(...metaImports)

  const moduleCode = nodeSsrEntrySource({
    imports,
    pageName: 'Page',
    pageModuleName: 'PageModule',
    layoutNames: wrappers,
    levels,
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
      ...bundleInputMetadata(cacheKey),
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        ...bundleInputMetadata(cacheKey),
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
      ...bundleInputMetadata(cacheKey),
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
      ...bundleInputMetadata(cacheKey),
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        ...bundleInputMetadata(cacheKey),
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
      ...bundleInputMetadata(cacheKey),
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
  if (cached) {
    return {
      outfile: cached,
      version: bundleVersions.get(cacheKey),
      ...bundleInputMetadata(cacheKey),
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        ...bundleInputMetadata(cacheKey),
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:action-entry.ts',
      outfile,
      platform: serverPlatform(),
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, null, bundle.contentHash)
    return {
      outfile,
      version: bundle.contentHash,
      ...bundleInputMetadata(cacheKey),
    }
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
  nested = {},
) {
  const { templates = [], slots = [], intercepts = [] } = nested
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'client')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const {
    imports: wrapperImports,
    layoutNames: wrappers,
    levels,
  } = wrapperEntryParts(layouts, templates, slots, intercepts)
  imports.push(...wrapperImports)

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
    levels,
    intercepts,
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
      ...bundleInputMetadata(cacheKey),
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        ...bundleInputMetadata(cacheKey),
      }
    }

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:client-entry.tsx',
      outfile,
      platform: 'browser',
      minify: process.env.RUVYXA_CLIENT_MINIFY === '1',
      // React comes from one shared URL rather than being inlined here. Two
      // route bundles each carrying their own copy is what made a soft
      // navigation render a component from one React into a root owned by
      // another, and every hook in it threw.
      externalUrls: clientVendorUrls(),
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, null, bundle.contentHash)
    return {
      outfile,
      version: bundle.contentHash,
      ...bundleInputMetadata(cacheKey),
    }
  })
}

/// --- Server Components Bundles ---
//
// A server-components route is compiled three times, because it runs in three
// places that must not share a React instance:
//
//   1. `rsc:server`   — the `react-server` graph. Produces the Flight payload.
//   2. `rsc:registry` — the same route's `'use client'` modules, compiled for
//                       *this* process, so the SSR pass can render their markup.
//   3. `rsc:client`   — the same modules again, compiled for the browser,
//                       alongside the decoder that replays the payload.
//
// (2) and (3) are separate builds of the same files on purpose: one links this
// process's React, the other links the browser's.

/**
 * Compile a route's `react-server` graph and report the client modules it
 * turned into references.
 *
 * Only `loading.tsx` of the three specials is imported. `error.tsx` and
 * `not-found.tsx` are rendered by a class boundary, and a class lifecycle does
 * not exist in the react-server build — the same constraint React itself
 * imposes when it requires an RSC `error.tsx` to be a client component.
 */
async function bundleRscServerModule(
  projectRoot,
  appDir,
  pageFile,
  layouts,
  routePath = '/',
  specials = null,
  nested = {},
) {
  const { templates = [], slots = [] } = nested
  // See `clientReferenceBase` below: the app directory's parent is the one
  // position the project's own tree and the build's staged copy share.
  const referenceBase = rscReferenceBase(appDir)
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'rsc')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const {
    imports: wrapperImports,
    layoutNames: wrappers,
    levels,
  } = wrapperEntryParts(layouts, templates, slots)
  imports.push(...wrapperImports)

  const loadingFile = specials?.loading ?? null
  if (loadingFile) {
    imports.push(`import RouteLoading from ${JSON.stringify(toImportPath(loadingFile))}`)
  }

  const { imports: metaImports, metaNames } = metaSourceImports(
    [...layouts, pageFile].map(toImportPath),
  )
  imports.push(...metaImports)

  const moduleCode = rscServerEntrySource({
    imports,
    pageName: 'Page',
    layoutNames: wrappers,
    levels,
    routePath,
    loadingName: loadingFile ? 'RouteLoading' : null,
    metaNames,
  })

  // The base is part of the key: it decides which ids the compile reports, and
  // two bases would otherwise share one cache entry and one set of references.
  const hash = createHash('sha256')
    .update(moduleCode)
    .update(pageFile)
    .update(referenceBase)
    .digest('hex')
    .slice(0, 16)
  const outfile = path.join(cacheDir, `server.${hash}.mjs`)
  const cacheKey = `rsc:server:${pageFile}:${hash}`

  const hit = rscBundleCacheHit(cacheKey)
  if (hit) return hit

  return withBuildLock(cacheKey, async () => {
    const rechecked = rscBundleCacheHit(cacheKey)
    if (rechecked) return rechecked

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:rsc-server.tsx',
      outfile,
      platform: serverPlatform(),
      bundleTarget: 'react-server',
      // Client-reference ids are measured from the directory holding the app
      // directory, not from the project root. `ruvyxa build` compiles this
      // route from the project's own sources while `ruvyxa start` compiles it
      // from the copy staged under `<out>/server/`; measured from the root
      // those two produce different ids, and the payload a running server
      // rendered then named a reference the browser bundle never registered.
      clientReferenceBase: referenceBase,
      // The one graph in this framework that carries its dependencies. Leaving
      // them external would resolve `react` through Node, which has no way to
      // know this module wants the `react-server` build — the process is
      // already running the other one.
      bundlePackages: true,
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
    rscBundleReferences.set(cacheKey, {
      client: bundle.clientReferences,
      server: bundle.serverReferences,
    })
    return {
      outfile,
      version: bundle.contentHash,
      dependencyHash: bundle.dependencyHash,
      clientReferences: bundle.clientReferences,
      serverReferences: bundle.serverReferences,
      ...bundleInputMetadata(cacheKey),
    }
  })
}

/**
 * Compile the client modules a payload references, for *this* process.
 *
 * Importing the result registers each module under its id, which is what lets
 * the SSR pass render a client component's markup instead of leaving a hole in
 * the document.
 */
/**
 * The directory every server-components reference id is measured from.
 *
 * Not the project root: the same route is compiled from two trees — the
 * project's own sources during `ruvyxa build`, and the copy staged under
 * `<out>/server/` which `ruvyxa start` serves from — and measured from the root
 * those give different ids for one module. The directory holding the app
 * directory is the position both trees share.
 *
 * Every graph that computes an id has to ask this, not just the server one.
 * When only the server graph did, a `'use server'` module got one id there and
 * another in the browser graph, and `ruvyxa start` answered a call with
 * `RUV1865` for a function it was holding all along — while `ruvyxa dev`, where
 * the two bases happen to coincide, worked.
 */
function rscReferenceBase(appDir) {
  return path.dirname(path.resolve(appDir))
}

async function bundleRscSsrRegistry(projectRoot, appDir, references) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'rsc')
  await ensureDir(cacheDir)

  const referenceBase = rscReferenceBase(appDir)
  const { imports, statements } = clientRegistrySource(references)
  const moduleCode = `${imports.join('\n')}\n${statements.join('\n')}\n`
  // The base is part of the key: it decides which ids the compile reports,
  // and two bases would otherwise share one entry and one set of references.
  const hash = createHash('sha256')
    .update(moduleCode)
    .update(referenceBase)
    .digest('hex')
    .slice(0, 16)
  const outfile = path.join(cacheDir, `registry.${hash}.mjs`)
  const cacheKey = `rsc:registry:${hash}`

  const hit = rscBundleCacheHit(cacheKey)
  if (hit) return hit

  return withBuildLock(cacheKey, async () => {
    const rechecked = rscBundleCacheHit(cacheKey)
    if (rechecked) return rechecked

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:rsc-registry.tsx',
      outfile,
      platform: serverPlatform(),
      // A server bundle normally leaves packages to Node's resolver, but this
      // one lifts client modules *out* of their packages and inlines them into
      // a file under `.ruvyxa/cache/rsc/`. A bare specifier left behind then
      // resolves from the cache directory instead of from the package that
      // wrote it — `@ruvyxa/react` importing `@ruvyxa/core` came back
      // "Cannot find package" the moment a layout used `<Link>`, because pnpm
      // keeps that dependency under the react package rather than the app.
      // Carrying the dependencies along also keeps this registry and the
      // browser bundle built from the same references symmetric: both render
      // the same components, so both must contain the same modules.
      bundlePackages: true,
      // React and the DOM renderer stay external so these components share the
      // instance `react-dom/server` renders them with. Bundling either would
      // put two copies in one render and every hook would throw.
      external: ['react', 'react/jsx-runtime', 'react-dom', 'react-dom/client', 'react-dom/server'],
      // This bundle is the *client* side of the boundary that happens to run on
      // a server, so a `'use server'` module it reaches becomes a reference
      // here exactly as it does in the browser. `<form action={save}>` renders
      // React's hidden reference fields either way, which is what makes the
      // server-rendered markup and the hydrated markup agree.
      serverReferenceClient: RSC_SSR_PACKAGE,
      clientReferenceBase: referenceBase,
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, null, bundle.contentHash)
    rscBundleReferences.set(cacheKey, { client: references, server: bundle.serverReferences })
    return {
      outfile,
      version: bundle.contentHash,
      serverReferences: bundle.serverReferences,
      ...bundleInputMetadata(cacheKey),
    }
  })
}

/** Compile the browser bundle for a server-components route. */
async function bundleRscClientModule(
  projectRoot,
  appDir,
  references,
  routePath,
  requestPath,
  paramsJson,
) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'client')
  await ensureDir(cacheDir)

  const referenceBase = rscReferenceBase(appDir)
  const moduleCode = rscClientEntrySource({
    references,
    routePath,
    requestPathLiteral: JSON.stringify(requestPath),
    paramsLiteral: paramsJson,
  })
  const hash = createHash('sha256')
    .update(moduleCode)
    .update(referenceBase)
    .digest('hex')
    .slice(0, 16)
  const outfile = path.join(cacheDir, `rsc.${hash}.js`)
  const cacheKey = `rsc:client:${hash}`

  const hit = rscBundleCacheHit(cacheKey)
  if (hit) return hit

  return withBuildLock(cacheKey, async () => {
    const rechecked = rscBundleCacheHit(cacheKey)
    if (rechecked) return rechecked

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:rsc-client.tsx',
      outfile,
      platform: 'browser',
      minify: process.env.RUVYXA_CLIENT_MINIFY === '1',
      externalUrls: clientVendorUrls(),
      serverReferenceClient: RSC_BROWSER_PACKAGE,
      clientReferenceBase: referenceBase,
      aliases: runtimeAliases(runtimeDir),
    })

    cacheBundle(cacheKey, outfile, projectRoot, bundle.inputs, null, bundle.contentHash)
    rscBundleReferences.set(cacheKey, { client: references, server: bundle.serverReferences })
    return {
      outfile,
      version: bundle.contentHash,
      serverReferences: bundle.serverReferences,
      ...bundleInputMetadata(cacheKey),
    }
  })
}

/**
 * Compile the bundle that can run this route's server functions.
 *
 * Keyed by the reference list rather than by the route: two routes that reach
 * the same actions file share one bundle, and one route whose actions change
 * gets a new one. The list arrives already sorted from the compiler, so the key
 * is stable across processes.
 */
async function bundleRscActionModule(projectRoot, appDir, references) {
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'rsc')
  await ensureDir(cacheDir)

  const referenceBase = rscReferenceBase(appDir)
  const moduleCode = rscActionEntrySource({ references })
  const hash = createHash('sha256')
    .update(moduleCode)
    .update(referenceBase)
    .digest('hex')
    .slice(0, 16)
  const outfile = path.join(cacheDir, `action.${hash}.mjs`)
  const cacheKey = `rsc:action:${hash}`

  const hit = rscBundleCacheHit(cacheKey)
  if (hit) return hit

  return withBuildLock(cacheKey, async () => {
    const rechecked = rscBundleCacheHit(cacheKey)
    if (rechecked) return rechecked

    const bundle = await compileBundleWithMetadata({
      projectRoot,
      entrySource: moduleCode,
      sourcefile: 'ruvyxa:rsc-action.tsx',
      outfile,
      platform: serverPlatform(),
      // The server functions run in the same realm the page renders in, so this
      // bundle carries the `react-server` build for the same reason the render
      // entry does: an action that returns an element tree must produce one the
      // page's own React can serialise.
      bundleTarget: 'react-server',
      clientReferenceBase: referenceBase,
      bundlePackages: true,
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
    rscBundleReferences.set(cacheKey, {
      client: bundle.clientReferences,
      server: bundle.serverReferences,
    })
    return {
      outfile,
      version: bundle.contentHash,
      dependencyHash: bundle.dependencyHash,
      clientReferences: bundle.clientReferences,
      serverReferences: bundle.serverReferences,
      ...bundleInputMetadata(cacheKey),
    }
  })
}

/**
 * Turn the bytes a server-function call arrived with back into React's shape.
 *
 * `encodeReply` produces a string for plain arguments and `FormData` when one
 * of them is a file or a stream, and `decodeReply` needs to be handed the same
 * kind back. `Response` is the parser for the second case — reimplementing
 * multipart parsing to avoid constructing one would be a second answer to a
 * question the runtime already answers.
 */
async function decodeActionBody(contentType, bodyBase64) {
  const bytes = Buffer.from(bodyBase64 ?? '', 'base64')
  if ((contentType ?? '').toLowerCase().startsWith('multipart/form-data')) {
    return await new Response(bytes, { headers: { 'content-type': contentType } }).formData()
  }
  return bytes.toString('utf8')
}

/**
 * Turn a submitted form's bytes into `FormData`.
 *
 * Separate from {@link decodeActionBody} because the two decode different
 * things. That one decodes what React's own encoder produced, which is a string
 * unless an argument forced multipart; this one decodes what a *browser*
 * produced from a `<form>`, which is multipart or url-encoded and never a
 * string. `Response` parses both from their content type, so neither case needs
 * a parser written here.
 */
async function decodeFormBody(contentType, bodyBase64) {
  const bytes = Buffer.from(bodyBase64 ?? '', 'base64')
  return await new Response(bytes, { headers: { 'content-type': contentType } }).formData()
}

/**
 * Run the server function a plain form post named, before the page is rendered.
 *
 * This is what `<form action={save}>` does in a browser with no JavaScript. The
 * hydrated page posts to `/__ruvyxa/rsc` and patches itself from the reply; a
 * page whose bundle has not loaded — or never will — submits the form the way
 * HTML always has, to the URL it is on. React put the reference id and any
 * bound arguments in hidden fields when it rendered the form, so the submission
 * carries everything needed to find the function.
 *
 * The action runs *before* the render and inside the same request context, so
 * what it wrote is what the page reads, and a `revalidatePath()` it called is
 * collected with the render's own.
 *
 * `null` unless a form post actually named an action. Any other POST to a page
 * — a search form, a probe, a stray request — renders the route unchanged,
 * which is what it did before this existed.
 */
async function runPostedFormAction({ projectRoot, appDir, request, server, registry }) {
  if (!request.formBody) return null
  const formData = await decodeFormBody(request.formContentType, request.formBody)
  const references = mergeServerReferences(server.serverReferences, registry.serverReferences)
  if (references.length === 0) return null
  const action = await bundleRscActionModule(projectRoot, appDir, references)
  const actionModule = await importModule(action.outfile, action.version)
  return await actionModule.runFormAction({ formData, serverManifest: serverManifest() })
}

/**
 * Run one server function and encode what it returned.
 *
 * The reply is a Flight payload, not JSON, so the function may return an
 * element tree containing client components — which is why it is serialised
 * against the same manifest a page render uses.
 *
 * The route travels with the call because it decides which graphs are searched
 * for the reference. A server function is reachable from the route whose page
 * or client components import it, and resolving it that way costs one compile
 * the host has usually already done, where a build-wide table of every action
 * in the application would be a second index to keep current.
 */
async function handleRscAction(request) {
  const {
    projectRoot,
    appDir,
    pageFile,
    requestPath,
    requestTarget,
    params,
    routePath,
    reference,
  } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureReactDeps(resolvedRoot)
  ensureServerComponentDeps(resolvedRoot)
  await ensureInstrumentation(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))

  const server = await bundleRscServerModule(
    resolvedRoot,
    appDir,
    pageFile,
    layouts,
    routePath || requestPath,
    specials,
    { templates, slots },
  )
  // Compiled for its reference list, not for its output: this is the graph that
  // sees the actions a `'use client'` component imports.
  const registry = await bundleRscSsrRegistry(resolvedRoot, appDir, server.clientReferences)
  const references = mergeServerReferences(server.serverReferences, registry.serverReferences)
  if (references.length === 0) {
    const error = new Error(
      `RUV1866 ${routePath || requestPath} declares no server functions, so ${reference} cannot be called on it`,
    )
    error.code = 'RUV1866'
    throw error
  }

  const action = await bundleRscActionModule(resolvedRoot, appDir, references)
  const actionModule = await importModule(action.outfile, action.version)

  const context = requestContext({
    headerPairs: request.headerPairs,
    method: request.method || 'POST',
    url: requestTarget || requestPath,
    params: params || {},
  })

  let failure = null
  const stream = await runWithRequestContext(context, async () =>
    actionModule.callServerFunction({
      reference,
      body: await decodeActionBody(request.contentType, request.body),
      manifest: flightManifest(server.clientReferences),
      serverManifest: serverManifest(),
      options: {
        onError(error) {
          failure ??= error
        },
      },
    }),
  )
  const payload = await readStreamText(stream)
  if (failure) throw failure

  return {
    ok: true,
    rscPayload: payload,
    requestScoped: usedRequestContext(context),
    revalidate: collectRevalidations(context),
  }
}

/** A cached RSC bundle, carrying back the references the server graph reported. */
function rscBundleCacheHit(cacheKey) {
  const cached = bundleCache.get(cacheKey)
  if (!cached) return null
  const references = rscBundleReferences.get(cacheKey)
  return {
    outfile: cached,
    version: bundleVersions.get(cacheKey),
    dependencyHash: bundleFingerprints.get(cacheKey),
    clientReferences: references?.client ?? [],
    serverReferences: references?.server ?? [],
    ...bundleInputMetadata(cacheKey),
  }
}

/**
 * Render one server-components route.
 *
 * The bundles are built in order because each depends on the last: the server
 * graph discovers the references, and the registry is built from them.
 *
 * `fresh` reloads the rendered module without discarding the compiled bundle,
 * which is what a build's per-path isolation needs and what `handleSsg` already
 * asks for on the ordinary path.
 */
async function handleServerComponents(request, { fresh = false, html = true } = {}) {
  const { projectRoot, appDir, pageFile, requestPath, requestTarget, params, routePath } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureReactDeps(resolvedRoot)
  ensureServerComponentDeps(resolvedRoot)
  await ensureInstrumentation(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))

  const server = await bundleRscServerModule(
    resolvedRoot,
    appDir,
    pageFile,
    layouts,
    routePath || requestPath,
    specials,
    { templates, slots },
  )
  // The registry is only needed by the SSR pass: it is what turns a reference
  // id into a module whose markup can be rendered. A payload-only render never
  // resolves one, so a soft navigation does not pay to compile it.
  let registry = null
  if (html) {
    registry = await bundleRscSsrRegistry(resolvedRoot, appDir, server.clientReferences)
    await importModule(registry.outfile, registry.version)
  }
  const serverModule = await importModule(
    server.outfile,
    fresh ? isolatedVersion(server.version) : server.version,
  )

  const context = requestContext({
    headerPairs: request.headerPairs,
    method: request.method,
    url: requestTarget || requestPath,
    params: params || {},
  })
  const rendered = await runWithRequestContext(context, async () => {
    // Inside the context, before the render: an action that set a cookie or
    // wrote a row must have done so by the time the page reads either.
    const posted = registry
      ? await runPostedFormAction({ projectRoot: resolvedRoot, appDir, request, server, registry })
      : null
    return await renderServerComponents({
      serverModule,
      references: server.clientReferences,
      ctx: { path: requestPath, params: params || {} },
      routePath: routePath || requestPath,
      html,
      formState: posted?.formState ?? null,
    })
  })

  // Both compiles when both ran. A pre-rendered page is cached against these,
  // and the registry is what supplies the client components React renders into
  // the HTML — including the hidden fields of a `<form action={fn}>`, whose
  // reference id is versioned by the action module's source. Reported from the
  // server graph alone, editing that module left every pre-rendered page
  // serving markup that names a function id the server no longer registers.
  const inputs = registry
    ? [...new Set([...(server.inputs ?? []), ...(registry.inputs ?? [])])]
    : (server.inputs ?? [])

  return {
    ok: true,
    html: rendered.html ?? undefined,
    rscPayload: rendered.payload,
    requestScoped: usedRequestContext(context),
    revalidate: collectRevalidations(context),
    dependencyHash: server.dependencyHash,
    inputsVersion: inputsVersionOf(inputs),
    inputs,
  }
}

/**
 * Render a server-components document as a stream.
 *
 * The same render as `handleServerComponents`, returned before it has finished.
 * Nothing here waits for the whole tree, so a `Suspense` boundary around a slow
 * server component sends its fallback with the shell and its content whenever
 * the server has it — instead of holding the first byte of the document until
 * the slowest part of the page is done.
 *
 * Only a route whose document is produced per request can do this. One that is
 * pre-rendered, cached, or revalidated has to become a string somewhere, and
 * that is `handleServerComponents` a few lines up.
 *
 * The payload rides out in the `api-end` trailer rather than in the first frame:
 * it is complete only when the render is, and the host writes it into the
 * document at the point the stream ends.
 */
async function handleServerComponentsDocument(request) {
  const { projectRoot, appDir, pageFile, requestPath, requestTarget, params, routePath } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureReactDeps(resolvedRoot)
  ensureServerComponentDeps(resolvedRoot)
  await ensureInstrumentation(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))

  const server = await bundleRscServerModule(
    resolvedRoot,
    appDir,
    pageFile,
    layouts,
    routePath || requestPath,
    specials,
    { templates, slots },
  )
  const registry = await bundleRscSsrRegistry(resolvedRoot, appDir, server.clientReferences)
  await importModule(registry.outfile, registry.version)
  const serverModule = await importModule(server.outfile, server.version)

  const context = requestContext({
    headerPairs: request.headerPairs,
    method: request.method,
    url: requestTarget || requestPath,
    params: params || {},
  })
  const rendered = await runWithRequestContext(context, async () => {
    const posted = await runPostedFormAction({
      projectRoot: resolvedRoot,
      appDir,
      request,
      server,
      registry,
    })
    return await renderServerComponentsStream({
      serverModule,
      references: server.clientReferences,
      ctx: { path: requestPath, params: params || {} },
      routePath: routePath || requestPath,
      formState: posted?.formState ?? null,
    })
  })

  return {
    ok: true,
    streamResponse: new Response(rendered.stream),
    async streamTrailer() {
      const payload = await rendered.payload
      for (const failure of rendered.failures) {
        // Reported, never thrown: the response left before this was known, and
        // the only thing left to do with it is put it in the log.
        console.error('[ruvyxa] server component failed after the shell was sent', failure)
      }
      return { rscPayload: payload }
    },
    // Read from the `api-start` frame, so the host can keep its dependency
    // bookkeeping current without waiting for the body. A form action has
    // already run by this point, which is why its `revalidatePath()` calls can
    // travel here rather than in the trailer.
    requestScoped: usedRequestContext(context),
    revalidate: collectRevalidations(context),
    dependencyHash: server.dependencyHash,
    inputsVersion: server.inputsVersion,
    inputs: server.inputs,
  }
}

/**
 * Report a server-components route's browser entry *source*.
 *
 * `ruvyxa build` compiles it with the Rust bundler, which is where `NODE_ENV`
 * folding, tree-shaking, minification, and the chunk budget live — a bundle
 * emitted here instead would ship React's development build to production.
 * The source still has to come from this process: only the `react-server` graph
 * knows which of the route's modules are client references, and generating the
 * entry in Rust would be a second answer to that.
 */
async function handleServerComponentsEntry(request) {
  const { projectRoot, appDir, pageFile, routePath } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureServerComponentDeps(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))
  const server = await bundleRscServerModule(
    resolvedRoot,
    appDir,
    pageFile,
    layouts,
    routePath,
    specials,
    { templates, slots },
  )
  // Compiled for its reference list, not for its output. This is the graph that
  // walks the client components, so it is the only one that sees the
  // `'use server'` modules they import — the server graph turned those
  // components into references and never followed their imports.
  const registry = await bundleRscSsrRegistry(resolvedRoot, appDir, server.clientReferences)

  // Both compiles, because this answer is derived from both. The server graph
  // reads a `'use client'` module — it has to, to see the directive — and then
  // stops, so nothing *behind* one is in its input list. A caller caching this
  // response against those inputs alone would never notice an edit to the
  // `'use server'` module a client component imports, and the ids in
  // `serverReferences` are versioned by that module's source: the browser
  // bundle would then be built from proxies naming a function the server no
  // longer registers, and every call through them fails at run time.
  const inputs = [...new Set([...(server.inputs ?? []), ...(registry.inputs ?? [])])]

  return {
    ok: true,
    entrySource: rscClientEntrySource({
      references: server.clientReferences,
      routePath,
      // The document's bootstrap block supplies both at run time; these are the
      // fallbacks for a document served without one.
      requestPathLiteral: JSON.stringify(routePath),
      paramsLiteral: '{}',
    }),
    // Every `'use server'` module the browser graph reaches, with the source
    // that must stand in for it there. The Rust bundler compiles that graph and
    // would otherwise walk the real file — which is server code, in the action
    // lane, and rejected as `RUV1820` the moment a client component imports it.
    // Handing over the text rather than the rule keeps one implementation of
    // what a server reference looks like.
    serverReferences: browserServerReferences(registry.serverReferences),
    inputsVersion: inputsVersionOf(inputs),
    inputs,
  }
}

/**
 * The stand-in source for each `'use server'` module, for a *browser* graph.
 *
 * The reference list is discovered by compiling the SSR registry, which is the
 * same set of client modules the browser bundle contains — but that compile
 * makes its proxies against `client.edge`, and the browser needs
 * `client.browser`. The ids are what the two graphs must agree on, and they do;
 * only the package differs.
 */
function browserServerReferences(references) {
  return (references ?? []).map((reference) => ({
    id: reference.id,
    file: reference.file,
    source: serverProxyModuleSource(reference.id, RSC_BROWSER_PACKAGE),
  }))
}

/** Build the browser bundle for a server-components route. */
async function handleServerComponentsClient(request) {
  const { projectRoot, appDir, pageFile, requestPath, params, routePath } = request
  const resolvedRoot = path.resolve(projectRoot || process.cwd())
  ensureServerComponentDeps(resolvedRoot)

  const layouts = collectLayouts(appDir, path.dirname(pageFile))
  const templates = collectTemplates(appDir, path.dirname(pageFile))
  const slots = collectSlots(appDir, path.dirname(pageFile))
  const specials = collectSpecials(appDir, path.dirname(pageFile))

  // The references come from the server graph, so the browser bundle is built
  // from what the payload will actually name rather than from a second scan of
  // the route's imports — two scans would be two answers.
  const server = await bundleRscServerModule(
    resolvedRoot,
    appDir,
    pageFile,
    layouts,
    routePath || requestPath,
    specials,
    { templates, slots },
  )
  const client = await bundleRscClientModule(
    resolvedRoot,
    appDir,
    server.clientReferences,
    routePath || requestPath,
    requestPath,
    JSON.stringify(params || {}),
  )
  const script = await readFile(client.outfile, 'utf8')
  return {
    ok: true,
    script,
    inputsVersion: client.inputsVersion,
    inputs: client.inputs,
  }
}

/**
 * Fail with an actionable message when the RSC runtime is not installed.
 *
 * `react-server-dom-webpack` is an optional peer: an app that never writes
 * `export const serverComponents = true` should not carry it. Without this the
 * first symptom is a resolver error naming a package the author never wrote.
 */
function ensureServerComponentDeps(projectRoot) {
  if (serverComponentDepsChecked.has(projectRoot)) return
  const requireFromProject = createRequire(path.join(projectRoot, 'package.json'))
  try {
    requireFromProject.resolve('react-server-dom-webpack/package.json')
    serverComponentDepsChecked.add(projectRoot)
  } catch {
    // The two share React internals rather than a public API, so the version has
    // to match exactly. Reading it off the copy this project resolved keeps the
    // command correct without a React version written down a second time here.
    let pinned = ''
    try {
      pinned = `@${requireFromProject('react/package.json').version}`
    } catch {
      pinned = ''
    }
    const error = new Error(
      'RUV1863 `export const serverComponents = true` needs react-server-dom-webpack, which is ' +
        'not installed. Install it at the same version as react ' +
        `(\`npm install react-server-dom-webpack${pinned}\`).`,
    )
    error.code = 'RUV1863'
    throw error
  }
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
  nested = {},
) {
  const { templates = [], slots = [] } = nested
  const cacheDir = path.join(projectRoot, '.ruvyxa', 'cache', 'ssg')
  await ensureDir(cacheDir)

  const imports = [`import Page from ${JSON.stringify(toImportPath(pageFile))}`]
  const {
    imports: wrapperImports,
    layoutNames: wrappers,
    levels,
  } = wrapperEntryParts(layouts, templates, slots)
  imports.push(...wrapperImports)

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
    levels,
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
      ...bundleInputMetadata(cacheKey),
    }
  }

  return withBuildLock(cacheKey, async () => {
    const rechecked = bundleCache.get(cacheKey)
    if (rechecked) {
      return {
        outfile: rechecked,
        version: bundleVersions.get(cacheKey),
        dependencyHash: bundleFingerprints.get(cacheKey),
        ...bundleInputMetadata(cacheKey),
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
      ...bundleInputMetadata(cacheKey),
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
