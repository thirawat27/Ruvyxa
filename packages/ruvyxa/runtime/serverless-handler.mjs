/**
 * Standalone serverless request handler for Ruvyxa.
 *
 * Provides a self-contained Request → Response function that does not depend
 * on the Rust host process or the NDJSON worker-pool protocol. Adapters
 * generate a thin platform wrapper that imports this handler.
 *
 * At build time, adapter-runner.mjs bundles route modules into the output
 * directory. This handler imports those pre-compiled modules and dispatches
 * requests using the build manifest.
 *
 * Supported rendering strategies:
 *   - SSR: full server render on every request
 *   - ISR: serve pre-rendered HTML, revalidate in background after TTL
 *   - PPR: serve pre-rendered shell, stream dynamic slots
 *   - CSR: serve static shell HTML
 *   - API: invoke method-specific handlers (GET/POST/PUT/DELETE/PATCH etc.)
 *
 * ISR/PPR behavior depends on platform capabilities passed via options.
 *
 * The only imports this file is allowed to carry are the sibling runtime
 * modules listed in `HANDLER_RUNTIME_FILES`, which
 * `adapter-runner.mjs` copies into the function bundle next to this file.
 * Everything else must stay inlined: a deployed function directory resolves no
 * bare specifiers.
 */

import { runAction, validateActionPayload, validateActionRequest } from './action-runtime.mjs'
import { methodNotAllowed, normalizeResponse, selectRouteHandler } from './api-methods.mjs'
import {
  collectRevalidations,
  requestContext,
  runWithRequestContext,
  usedRequestContext,
} from './request-context.mjs'
import { canonicalRoutePath, createCanonicalRouteMatcher } from './route-match.mjs'
import { encodeFlightPayload, publicFlightError } from './flight.mjs'
// The same two cross-site checks `action-runtime.mjs` applies to
// `/__ruvyxa/action`, read straight from the shared policy so `/__ruvyxa/rsc`
// gets the rule rather than a port of it. Already in `HANDLER_RUNTIME_FILES` and
// already copied beside this file in every function bundle.
import { fetchSiteIsCrossSite, originIsCrossSite, parseForwardedScheme } from './origin-policy.mjs'

const MAX_REVALIDATIONS_PER_REQUEST = 64
const MAX_REVALIDATION_PATH_LENGTH = 2_048
const MAX_PENDING_PATH_REVALIDATIONS = 1_024

/**
 * Replay-guard bounds, held with the native host by
 * `tests/fixtures/action-contract.json`.
 */
const ACTION_NONCE_TTL_MS = 10 * 60 * 1000
const MAX_ACTION_NONCES = 10_000

/**
 * Live nonces one client address may hold, a tenth of the whole pool.
 *
 * `actionRateLimitResponse` runs in front of this guard but is keyed per client
 * *and per path and action*, so one caller spreading requests over two actions
 * earns two fresh buckets while the nonce pool stays one — enough for a single
 * address to saturate it and have every other caller's actions refused for a
 * TTL. Mirrors `ACTION_NONCE_MAX_PER_CLIENT` in `action_security.rs`.
 */
const MAX_ACTION_NONCES_PER_CLIENT = MAX_ACTION_NONCES / 10

/** Endpoint the framework's own server actions are posted to. */
const ACTION_PATH = '/__ruvyxa/action'
const FLIGHT_PATH = '/__ruvyxa/flight'
const RSC_PATH = '/__ruvyxa/rsc'
const IMAGE_PATH = '/__ruvyxa/image'

/**
 * The paths this host answers itself, decided before the plugin stage runs.
 *
 * The four `dispatch` rows of
 * `tests/fixtures/framework-endpoint-conformance.json`, and the ordering half of
 * a divergence: on the native host these are axum routes and the plugin-bearing
 * handler is the fallback, so a reserved path never reaches
 * `apply_request_plugins`. This host wrapped the plugin stage around everything,
 * so an `http.onRequest({ match: ['*'] })` auth hook guarded
 * `POST /__ruvyxa/action` when deployed and did not guard it under
 * `dev`/`start` — the direction that matters, because the guard is then never
 * exercised where it is being written.
 *
 * A subset of `RESERVED_FRAMEWORK_PATHS` in `plugin-http.mjs`, which is the
 * registration-time half of the same rule: the paths there include the ones only
 * the native host serves, and a plugin may not claim any of them on either host.
 */
const FRAMEWORK_ENDPOINT_PATHS = Object.freeze([ACTION_PATH, FLIGHT_PATH, RSC_PATH, IMAGE_PATH])

/**
 * The header that keeps {@link RSC_PATH} out of reach of a cross-origin page,
 * and the one naming the server function a `POST` there runs.
 *
 * This endpoint has no origin policy the way `/__ruvyxa/action` does: a
 * third-party page being unable to set a non-safelisted header without a
 * preflight nothing answers *is* the whole defence. It was four inline literals
 * in this file, matching two constants in `framework_endpoints.rs`, an export in
 * `rsc-client-runtime.mjs`, and one more literal in `@ruvyxa/react`'s router.
 * Named locally rather than imported, because this module is copied into
 * function bundles whose file set is registered in three places; the names are
 * held across hosts by `requiredHeaders` in
 * `tests/fixtures/framework-endpoint-conformance.json` instead.
 */
const RSC_REQUEST_HEADER = 'x-ruvyxa-rsc'
const SERVER_ACTION_HEADER = 'x-ruvyxa-action'

/** Defaults matching `ruvyxa build`'s validated `security` block. */
const DEFAULT_API_BODY_LIMIT = 10 * 1024 * 1024
const DEFAULT_ACTION_BODY_LIMIT = 1024 * 1024
/** Largest server-function call body, matching `framework_endpoints.rs`. */
const RSC_ACTION_BODY_LIMIT = 4 * 1024 * 1024
const DEFAULT_ACTION_RATE_MAX = 600
const DEFAULT_ACTION_RATE_WINDOW_SECONDS = 60

/**
 * The bounds a caller of `/__ruvyxa/image` is held to, and the quality an
 * absent `q` resolves to.
 *
 * The native host encodes the same four numbers in
 * `crates/ruvyxa_dev_server/src/dynamic_image.rs` and neither side can import
 * the other's, so `tests/fixtures/dynamic-image-conformance.json` holds them
 * together.
 *
 * `DEFAULT_IMAGE_QUALITY` is a fallback, not the answer: a project's own
 * `image.quality` arrives as the `imageQuality` option and wins. It is reached
 * by a manifest built before the runtime policy carried the value, so it has to
 * name the number that policy would have carried.
 */
const DEFAULT_IMAGE_QUALITY = 82
const MIN_IMAGE_QUALITY = 1
const MAX_IMAGE_QUALITY = 100
const MIN_IMAGE_WIDTH = 16
const MAX_IMAGE_WIDTH = 8192

/**
 * @typedef {Object} RouteEntry
 * @property {string} id
 * @property {string} path
 * @property {'page'|'api'} kind
 * @property {string} file
 * @property {string[]} layoutChain
 * @property {{strategy: string, revalidate?: number, hasDynamicSlots?: boolean}} render
 */

/**
 * @typedef {Object} HandlerOptions
 * @property {RouteEntry[]} routes - Build manifest routes
 * @property {string} buildDir - Absolute path to the build output directory
 * @property {string} [basePath] - Optional base path prefix
 * @property {(routeId: string) => Promise<{render: (ctx: object) => Promise<string>, flight?: (ctx: object) => Promise<unknown>}>} importPage
 *   Import a pre-compiled page module. Adapters supply this to abstract away
 *   platform-specific module resolution.
 * @property {(routeId: string) => Promise<Record<string, Function>>} importApi
 *   Import a pre-compiled API route module.
 * @property {(routeId: string) => Promise<Record<string, Function>>} [importAction]
 *   Import the pre-compiled `action.ts` that sits beside a page route. Omitted
 *   when the project declares no server actions; `POST /__ruvyxa/action` then
 *   answers 501 rather than 404, so a misconfigured deploy is distinguishable
 *   from a project that simply has no actions.
 * @property {(request: Request, next: (request: Request) => Promise<Response>) => Promise<Response>} [pluginHttp]
 *   Project plugin HTTP hooks, compiled into the function bundle by
 *   `adapter-runner.mjs`. Runs between the built-in middleware and routing,
 *   the same position `apply_request_plugins` holds in the native server.
 * @property {{apiLimit?: number, actionLimit?: number, headers?: boolean, sameOrigin?: boolean, fetchMeta?: boolean, trustedProxyIps?: string[], actionRateLimit?: {max?: number, window?: number}}} [security]
 *   The validated `security` block from `build.json`. Before this existed the
 *   deployed runtimes ignored it entirely: a function had no request body cap
 *   at all, `security.headers: false` had no effect, and `trustedProxyIps` was
 *   unused, while `ruvyxa start` enforced all three.
 * @property {string} [notFoundDocument] - Pre-rendered `app/not-found.tsx`, returned with 404 for an unmatched URL.
 * @property {(path: string, revalidate?: number) => string|{html: string, stale: boolean}|null|Promise<string|{html: string, stale: boolean}|null>} [readPrerendered]
 *   Read of a pre-rendered document. ISR-capable adapters return freshness
 *   explicitly; a legacy string result is treated as stale.
 *
 *   May be async. It was synchronous, and that alone is why the Cloudflare
 *   adapter could not do ISR: a Workers KV read returns a promise, so the
 *   only store an edge deployment has could not be consulted here. Awaiting a
 *   value that is not a promise costs a microtask and keeps every filesystem
 *   adapter exactly as it was.
 * @property {(path: string, html: string, revalidate?: number, forced?: boolean) => void|Promise<void>} [writePrerendered]
 *   Write pre-rendered HTML to ISR cache with a TTL.
 * @property {string[]} [supportedStrategies]
 *   Strategies the platform supports. Defaults to ['ssr','ssg','csr','isr','ppr','api'].
 *   Unsupported strategies produce a 501 response.
 * @property {boolean} [securityHeaders=true]
 *   Apply Ruvyxa's non-breaking security headers unless the response already
 *   defines a value for that header.
 * @property {{builtin?: object}} [middleware]
 *   Validated built-in middleware policy emitted by the Ruvyxa build. The
 *   Fetch-native implementation mirrors the Axum/Tower CORS, rate-limit,
 *   timing, logging, and custom-header behavior without Node.js polyfills.
 * @property {{locales: string[], defaultLocale: string, localeParam: string, detectLocale: boolean, cookie: string}} [i18n]
 * @property {(request: Request, input: {src: string, width: number, quality: number}) => Promise<Response>} [optimizeImage]
 * @property {number} [imageQuality]
 *   The project's `image.quality`, published by `ruvyxa build` as
 *   `runtime.image.quality`. Decides what an image request without a `q`
 *   parameter is encoded at, exactly as it does under `ruvyxa dev` and
 *   `ruvyxa start`. Falls back to `DEFAULT_IMAGE_QUALITY` when a manifest
 *   predates the field; an out-of-range value is clamped rather than refused,
 *   because a configuration mistake must not turn every image into a 400.
 */

/**
 * Runtime files a function bundle must carry beside this handler.
 *
 * This module imports each of them as a sibling and a deployed function
 * directory resolves no bare specifiers, so the set is a deployment contract
 * rather than a convenience. It is exported because it had three copies —
 * `materializeFunction` in `adapter-runner.mjs`, and the adapter tests that
 * assemble a function directory by hand — and adding `action-runtime.mjs` to
 * one of them produced a bundle that imported a file nobody had copied.
 * Everything that builds a function directory reads this list.
 */
export const HANDLER_RUNTIME_FILES = Object.freeze([
  'serverless-handler.mjs',
  'route-match.mjs',
  'request-context.mjs',
  'action-runtime.mjs',
  // `action-runtime.mjs` imports this for its two cross-site checks.
  'origin-policy.mjs',
  // Which export answers a method, and what a 405 has to say. Shared with the
  // two native hosts so the three cannot disagree about it.
  'api-methods.mjs',
  'flight.mjs',
  // Not imported by this file: the standalone server the node, bun, deno, aws,
  // railway, and render adapters emit bounds its own render concurrency with it,
  // and a deployed function directory resolves no bare specifiers, so it has to
  // be copied in beside that program. The serverless adapters carry the file and
  // never import it — a kilobyte against a second copy of a controller the
  // worker pool already proved.
  'worker-admission.mjs',
])

/**
 * Cache-control for a document served as a file, and the default for a page
 * that did not choose its own.
 *
 * Safe to store, never safe to pin: a redeploy replaces the document under the
 * same URL, and a reader holding a heuristically cached copy would keep seeing
 * the old site with nothing to tell it otherwise.
 */
export const DOCUMENT_CACHE_CONTROL = 'public, max-age=0, must-revalidate'

/**
 * The strategies whose document is stored bytes, and may therefore be validated.
 *
 * `DOCUMENT_CACHE_CONTROL` tells a browser to revalidate before every reuse, and
 * without a validator that revalidation can only be answered with the whole
 * document again — so a page a reader already holds was re-sent in full on every
 * navigation. ISR is the same question with `s-maxage` in front of it.
 *
 * `ssr` and `ppr` are absent because their document is produced for this request:
 * it may carry one visitor's data, it may still be streaming, and it is
 * `no-store` either way, so there is nothing for a validator to be about. The
 * same table is `document_validator_strategies` in
 * `crates/ruvyxa_graph/src/lib.rs`; both are replayed against
 * `tests/fixtures/deploy-output-conformance.json`.
 */
export const DOCUMENT_VALIDATOR_STRATEGIES = Object.freeze(['ssg', 'csr', 'isr'])

/**
 * Marker a stored document leaves the strategy layer with.
 *
 * The validator cannot be computed where the document is read, because a plugin
 * `http.onResponse` hook may still replace the body — the first-party `pwa`
 * plugin does exactly that, injecting into every HTML response. An ETag written
 * before that runs would describe bytes nobody received, and a validator that is
 * wrong is worse than none: it answers `304` for a document that changed.
 *
 * So the strategy layer only says "this one is validatable" and the outermost
 * wrapper, where the body is final, decides. Deleted there, so it never reaches
 * a client.
 */
const DOCUMENT_VALIDATOR_HEADER = 'x-ruvyxa-validate'

/** The revalidation window an ISR route that named none is given. */
const DEFAULT_REVALIDATE_SECONDS = 60

/**
 * How long a stale ISR document may still be served while it refreshes.
 *
 * The stale window is `ISR_EXPIRE_SECONDS - revalidate`, the formula Next.js
 * ships in production (its `expireTime`, one year by default). The directive
 * has to carry a number: RFC 5861 defines
 * `stale-while-revalidate=<delta-seconds>`, and Netlify's CDN documents only
 * the numeric form — a bare directive is dropped there, which silently turns
 * every refresh into a blocking render.
 */
const ISR_EXPIRE_SECONDS = 31_536_000

/**
 * What a server sends with a document it just served, by rendering strategy.
 *
 * ISR advertises the project's own clock so a CDN in front of the function can
 * hold the page for exactly as long as the project asked, and refresh it
 * without a gap. A per-request render advertises nothing cacheable: it may
 * carry one visitor's data.
 *
 * `max-age=0` is the same guard `DOCUMENT_CACHE_CONTROL` carries and for the
 * same reason: `s-maxage` speaks to the shared cache only, so an ISR response
 * that named no `max-age` left the *browser* with no freshness instruction, and
 * heuristic caching applies — a reader could hold the page across a redeploy
 * with nothing to tell it otherwise.
 *
 * The same table as `document_cache_control` in
 * `crates/ruvyxa_cli/src/deploy_manifest.rs` and `documentCacheControl` in
 * `@ruvyxa/core`; all three are replayed against
 * `tests/fixtures/deploy-output-conformance.json`.
 */
export function documentCacheControl(strategy, revalidate) {
  if (strategy === 'isr') {
    const window = revalidate ?? DEFAULT_REVALIDATE_SECONDS
    const stale = Math.max(0, ISR_EXPIRE_SECONDS - window)
    return `public, max-age=0, s-maxage=${window}, stale-while-revalidate=${stale}`
  }
  if (strategy === 'ssg' || strategy === 'csr') return DOCUMENT_CACHE_CONTROL
  return 'no-store'
}

/** Longest a single interpolated value may be in a log line. */
const LOG_VALUE_LIMIT = 256

/**
 * One caller-supplied value, rendered safe to put in a log line.
 *
 * A log line is a record something else parses — a collector, a `grep`, a
 * person deciding what happened. A value carrying a line terminator does not
 * appear in that record, it *becomes* a second record, written by whoever sent
 * the request. Proven rather than assumed: `?name=` on the action endpoint is
 * percent-decoded by `searchParams.get`, so
 * `?name=bad%0Ainjected%20line:%20everything%20is%20fine` reached
 * `console.error` with a real newline in it and the deployed log gained an entry
 * nobody wrote.
 *
 * Only the terminators and the other control characters are removed; the rest
 * is kept, because a path with an accented character in it is a path an operator
 * still has to be able to read. Bounded too — one field must not be able to bury
 * the rest of the line.
 */
export function logValue(value) {
  const text = typeof value === 'string' ? value : String(value)
  let rendered = ''
  for (const character of text) {
    if (rendered.length >= LOG_VALUE_LIMIT) return `${rendered}…`
    const code = character.codePointAt(0)
    const isControl = code < 0x20 || code === 0x7f || (code >= 0x80 && code <= 0x9f)
    // U+2028 and U+2029 terminate a line for enough readers to count as one.
    const isLineSeparator = code === 0x2028 || code === 0x2029
    rendered += isControl || isLineSeparator ? '.' : character
  }
  return rendered
}

/**
 * The console method a level writes through, read at call time.
 *
 * Captured once into a table instead, this stopped honouring a `console` a host
 * replaced after import — which is what a logging library and several platform
 * log shims do, and what this repository's own tests do to read the output.
 */
function logWriter(level) {
  if (level === 'error') return console.error
  if (level === 'warn') return console.warn
  return console.info
}

/**
 * Emit one operational record.
 *
 * `json` writes a single object per line, which is what a collector wants and
 * which also makes the escaping above structural rather than remembered:
 * `JSON.stringify` cannot emit a raw newline. `text` keeps the shape a person
 * reading a terminal expects. The host decides, because only the host knows
 * where its output goes — a serverless adapter's log is the platform's, and it
 * is left alone.
 */
export function logRecord(format, level, message, fields = {}) {
  const write = logWriter(level)
  if (format === 'json') {
    write(JSON.stringify({ level, msg: message, ...fields }))
    return
  }
  const rendered = Object.entries(fields)
    .map(([name, value]) => `${name}=${logValue(value)}`)
    .join(' ')
  write(rendered === '' ? `[ruvyxa] ${message}` : `[ruvyxa] ${message} ${rendered}`)
}

/** Security defaults shared with the native and standalone runtimes. */
export const DEFAULT_SECURITY_HEADERS = Object.freeze({
  'x-content-type-options': 'nosniff',
  'referrer-policy': 'strict-origin-when-cross-origin',
  'permissions-policy': 'camera=(), microphone=(), geolocation=()',
  'cross-origin-opener-policy': 'same-origin',
  'cross-origin-resource-policy': 'same-origin',
  'x-frame-options': 'DENY',
  'x-permitted-cross-domain-policies': 'none',
})

/**
 * Create a serverless request handler.
 *
 * @param {HandlerOptions} options
 * @returns {(request: Request, runtimeContext?: {waitUntil?: (promise: Promise<unknown>) => void}) => Promise<Response>}
 */
export function createHandler(options) {
  const {
    routes,
    basePath = '',
    importPage,
    importApi,
    importAction,
    pluginHttp,
    security,
    readPrerendered,
    writePrerendered,
    supportedStrategies = ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    middleware,
    i18n,
    optimizeImage,
    imageQuality,
    notFoundDocument,
    clientIpHeaders,
    logFormat,
  } = options
  // An explicit `securityHeaders` option still wins, so a caller constructing
  // the handler directly keeps full control; otherwise the project's own
  // `security.headers` decides, and only then the safe default.
  const securityHeaders = options.securityHeaders ?? security?.headers ?? true
  const apiBodyLimit = positiveInteger(security?.apiLimit) ?? DEFAULT_API_BODY_LIMIT
  const actionPolicy = {
    actionLimit: positiveInteger(security?.actionLimit) ?? DEFAULT_ACTION_BODY_LIMIT,
    sameOrigin: security?.sameOrigin,
    fetchMeta: security?.fetchMeta,
  }
  const actionRateLimit = {
    max: positiveInteger(security?.actionRateLimit?.max) ?? DEFAULT_ACTION_RATE_MAX,
    window:
      positiveInteger(security?.actionRateLimit?.window) ?? DEFAULT_ACTION_RATE_WINDOW_SECONDS,
  }
  const defaultImageQuality = Number.isInteger(imageQuality)
    ? Math.min(MAX_IMAGE_QUALITY, Math.max(MIN_IMAGE_QUALITY, imageQuality))
    : DEFAULT_IMAGE_QUALITY
  const trustedProxies = parseTrustedProxies(security?.trustedProxyIps)
  // Empty unless the adapter declared which headers its platform's ingress
  // writes, because a header nobody guaranteed is a header the caller typed.
  // See `clientAddress` for what treating that as an identity costs.
  const ingressHeaders = parseIngressHeaders(clientIpHeaders)
  // Only a host that knows where its output goes asks for this; a serverless
  // adapter's log belongs to its platform and is left in the shape it expects.
  const logAs = logFormat === 'json' ? 'json' : 'text'
  const actionBuckets = new Map()
  const actionNonces = new Map()
  /** Live nonce count per client address, kept level with `actionNonces`. */
  const actionNonceClients = new Map()
  /**
   * Persist a rendered document.
   *
   * Whether a failed write may be swallowed depends on what the write promised.
   *
   * An ordinary ISR/SSG store is a cache optimization: serving the page is the
   * request, storing it only makes the next one cheaper. Hosts whose runtime
   * filesystem is read-only are ordinary in production — a container started
   * with `--read-only`, a pod with `readOnlyRootFilesystem: true`, Cloud Run, a
   * Lambda bundle outside `/tmp` — and letting that throw turned a page that
   * had already rendered correctly into a 500 for every visitor. A full disk or
   * a revoked permission did the same. Those degrade to rendering every time.
   *
   * A write that settles a `revalidatePath()` claim is not an optimization. The
   * caller was told that the next request sees the new document, and this
   * instance cannot reach the ones already warm elsewhere, so the durable write
   * is the only thing that makes the promise true. Swallowing its failure would
   * report success while every later request kept serving the old page, so it
   * still surfaces and leaves the generation pending for retry.
   *
   * @returns whether the document was stored, so callers that only make sense
   * after a durable write can tell.
   */
  async function persistPrerendered(pathname, html, revalidate, forced = false) {
    if (!writePrerendered) return false
    // `forced` reaches the adapter, because a store is not always the only
    // thing that has to be told. A platform that caches the *response* in front
    // of this function — Vercel's Prerender Functions — keeps serving the old
    // document until its own window expires, so an adapter has to be able to
    // tell an ordinary background refresh from a `revalidatePath()` that must
    // reach the CDN as well.
    if (forced) {
      await writePrerendered(pathname, html, revalidate, true)
      return true
    }
    try {
      await writePrerendered(pathname, html, revalidate, false)
      return true
    } catch (error) {
      console.error(`[ruvyxa] could not store the rendered page for ${logValue(pathname)}:`, error)
      return false
    }
  }

  const pendingRevalidations = new Map()
  /**
   * Generation claims for URLs `revalidatePath()` named, waiting for a
   * successful render and durable adapter write.
   *
   * Held in the instance rather than pushed to the platform's cache store,
   * because the only universal capability an adapter provides is read and
   * write — there is no delete. Requests for one of these paths bypass
   * `readPrerendered` until `writePrerendered` succeeds; render/status/write
   * failures leave the exact generation pending for retry. A successful write
   * replaces the stored document for later requests and other instances. What
   * this cannot do is reach an instance that is already warm
   * elsewhere; that one keeps serving until its own TTL expires, which is the
   * same bound ISR already has.
   */
  const forcedRevalidations = new Map()
  let nextRevalidationGeneration = 0
  let bypassPrerendered = false
  const fetchMiddleware = createFetchMiddleware(middleware, trustedProxies, ingressHeaders, logAs)

  function failClosedRevalidations(message) {
    if (bypassPrerendered) return
    forcedRevalidations.clear()
    bypassPrerendered = true
    // The adapter contract has no universal delete operation. Bypassing every
    // prerendered artifact is the bounded option that cannot silently serve
    // invalidated HTML. This intentionally trades cache/CPU efficiency for
    // correctness until the serverless instance is recycled.
    console.warn(message)
  }

  function markForcedRevalidation(path) {
    if (bypassPrerendered) return
    if (
      !forcedRevalidations.has(path) &&
      forcedRevalidations.size >= MAX_PENDING_PATH_REVALIDATIONS
    ) {
      failClosedRevalidations(
        `[ruvyxa] More than ${MAX_PENDING_PATH_REVALIDATIONS} paths are waiting for revalidation; ` +
          'bypassing prerendered artifacts for this instance.',
      )
      return
    }
    nextRevalidationGeneration++
    if (!Number.isSafeInteger(nextRevalidationGeneration)) {
      failClosedRevalidations(
        '[ruvyxa] Revalidation generation exhausted; bypassing prerendered artifacts for this instance.',
      )
      return
    }
    forcedRevalidations.set(path, nextRevalidationGeneration)
  }

  function claimForcedRevalidation(path) {
    if (bypassPrerendered) return { all: true }
    const generation = forcedRevalidations.get(path)
    return generation === undefined ? null : { generation }
  }

  function acknowledgeForcedRevalidation(path, claim) {
    if (!claim || claim.all) return
    if (forcedRevalidations.get(path) === claim.generation) {
      forcedRevalidations.delete(path)
    }
  }

  // Compile once through the shared matcher so browser navigation and
  // serverless dispatch use the same precedence and indexed static-route path.
  // This host canonicalizes at its request boundary, so it deliberately uses
  // the canonical-input entry point and never decodes a segment twice.
  const matchRoute = createCanonicalRouteMatcher(routes)

  return async function handle(request, runtimeContext = {}) {
    const response = await fetchMiddleware(request, () =>
      limitThenDispatch(request, runtimeContext),
    )
    // Last, because this is the first point at which the body is the body: the
    // plugin response stage and the built-in middleware have both run.
    const validated = await withDocumentValidator(request, response)
    return securityHeaders ? withDefaultSecurityHeaders(validated) : validated
  }

  /**
   * Apply the request body limit, then run plugin hooks, then route.
   *
   * The order is the native server's: built-in middleware wraps the router,
   * `handle_request` caps the body with `to_bytes(api_body_limit_bytes)`, and
   * only then does `apply_request_plugins` run. Capping after the plugin stage
   * instead would hand an `http.onRequest` hook — the socket `@ruvyxa/auth` and
   * every project middleware is built on — a body no limit applied to, which is
   * the one caller most likely to read it into memory.
   */
  async function limitThenDispatch(request, runtimeContext) {
    const ingress = limitRequestBody(request)
    if (ingress.response) return ingress.response
    try {
      // The framework's own endpoints are decided here rather than inside the
      // plugin stage, which is what the native router's route-before-fallback
      // ordering does. A plugin can no longer shadow one, hook one, or 404 one
      // at its discretion — and `withDefaultSecurityHeaders` still runs on the
      // way out, which is the coverage a response hook was being used for.
      if (typeof pluginHttp !== 'function' || ownedByFramework(ingress.request)) {
        return await dispatch(ingress.request, runtimeContext)
      }
      return await pluginHttp(ingress.request, async (forwarded) => {
        const candidate = forwarded ?? ingress.request
        if (candidate === ingress.request) return dispatch(candidate, runtimeContext)

        // A plugin may forward a newly constructed Request. Reapply the same
        // endpoint-aware boundary because that body never passed through the
        // ingress stream above; native plugins can only forward the already
        // bounded body serialized by the host.
        const guarded = limitRequestBody(candidate)
        return guarded.response ?? dispatch(guarded.request, runtimeContext)
      })
    } catch (error) {
      // A plugin that read past the cap surfaces the stream error here rather
      // than inside `dispatch`, so the same 413 has to be produced on both
      // paths. Anything else is a plugin fault and is reported as such.
      if (isBodyLimitError(error)) return textResponse(413, ingress.message)
      const message = error instanceof Error ? error.message : String(error)
      console.error('[ruvyxa] Plugin HTTP middleware failed:', logValue(message))
      return textResponse(500, 'Internal Server Error')
    }
  }

  /** Apply the endpoint-specific body policy before any body consumer. */
  function limitRequestBody(request) {
    const policy = requestBodyPolicy(request)
    const response = declaredBodyTooLarge(request, policy.limit, policy.message)
    return response
      ? { request, response, message: policy.message }
      : { request: limitBodyStream(request, policy.limit), response: null, message: policy.message }
  }

  /**
   * The canonical, base-path-stripped pathname, or `null` when there is none.
   *
   * `dispatch` owns malformed-path reporting and answers the canonical 400, so
   * the two callers ahead of it — the body policy and the framework-endpoint
   * ordering — answer `null` and let the request reach it. A path this cannot
   * resolve is one no framework endpoint claims either way.
   */
  function endpointPathname(request) {
    try {
      return stripBasePath(canonicalRequestPath(new URL(request.url).pathname), basePath)
    } catch {
      return null
    }
  }

  /** Whether this host answers the request itself, ahead of any plugin. */
  function ownedByFramework(request) {
    const pathname = endpointPathname(request)
    return pathname !== null && FRAMEWORK_ENDPOINT_PATHS.includes(pathname)
  }

  /**
   * Select the same body owner as the native router. Actions have their own
   * Axum body layer and do not pass through the generic API fallback limit.
   */
  function requestBodyPolicy(request) {
    const pathname = endpointPathname(request)
    if (pathname === ACTION_PATH) {
      return { limit: actionPolicy.actionLimit, message: 'Action payload is too large' }
    }
    // A server-function call is arguments, not an upload: React encodes them
    // as text unless one is a file, and a file large enough to matter belongs
    // in a route handler that can stream it. Same bound as the native host's
    // `MAX_SERVER_ACTION_BODY`, applied in the same place the other endpoint
    // bounds are, so a call accepted locally is accepted here.
    if (pathname === RSC_PATH) {
      return { limit: RSC_ACTION_BODY_LIMIT, message: 'Server-function call too large' }
    }
    return { limit: apiBodyLimit, message: 'Request body is too large' }
  }

  async function dispatch(request, runtimeContext = {}) {
    const url = new URL(request.url)
    const rawPathname = url.pathname
    let canonicalPathname
    try {
      canonicalPathname = canonicalRequestPath(rawPathname)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      console.error(`[ruvyxa] Malformed request path ${logValue(rawPathname)}:`, logValue(message))
      return new Response('Bad Request', {
        status: 400,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
      })
    }
    const pathname = stripBasePath(canonicalPathname, basePath)
    // A request outside the configured base path is not ours to serve.
    // Slicing unconditionally would turn `/other/thing` into `r/thing` and let
    // it match an unrelated route.
    if (pathname === null) {
      return new Response('Not Found', { status: 404 })
    }

    // The request boundary above already decoded and normalized the path using
    // the same segment rules as the Rust development server.
    if (pathname === IMAGE_PATH) {
      return handleDynamicImage(
        request,
        runtimeContext.optimizeImage ?? optimizeImage,
        defaultImageQuality,
      )
    }

    if (pathname === ACTION_PATH) {
      return handleServerAction(request, url)
    }

    if (pathname === FLIGHT_PATH) {
      return handleFlight(request, url)
    }

    // A soft navigation into a server-components route. The generated registry
    // carries a payload-only renderer for each one now, so this answers rather
    // than reporting 501 and making the browser fall back to a document load.
    if (pathname === RSC_PATH) {
      return handleRscPayload(request, url)
    }

    const match = matchRoute(pathname)
    if (!match) {
      const redirect = localeRedirect(request, pathname, url.search, basePath, matchRoute, i18n)
      // The path alone, never an origin. `Response.redirect()` demands an
      // absolute URL, so this reached for `new URL(redirect, request.url)` and
      // took its origin from `request.url` — which the standalone server builds
      // from the raw `Host` header, so a client chose the redirect target. No
      // browser forges `Host` on its own, but the response carries no
      // `Vary: Host`, so any shared cache keyed on path can store the forged
      // `Location` and hand it to real visitors.
      //
      // RFC 9110 has allowed a relative `Location` since 2014 and every browser
      // resolves one against the request URL, so this is the same answer for
      // every legitimate request — and it is byte for byte what the native
      // host's locale redirect in `render_pipeline.rs` sends.
      if (redirect) return new Response(null, { status: 307, headers: { location: redirect } })
      // The application's own not-found page, pre-rendered by the build.
      //
      // `app/not-found.tsx` is the file every reader coming from another
      // framework writes for this, and a deployed build ignored it: only a
      // `notFound()` call inside a *matched* route reached it, so the one
      // request every deployment receives and no route owns was answered with
      // this bare string — while the same code under `ruvyxa dev` rendered the
      // project's page with its layout and styles.
      if (notFoundDocument) {
        return new Response(notFoundDocument, {
          status: 404,
          headers: {
            'content-type': 'text/html; charset=utf-8',
            'cache-control': DOCUMENT_CACHE_CONTROL,
          },
        })
      }
      return new Response('Not Found', { status: 404 })
    }

    const { route, params } = match

    // A missing static file must not be answered by a page render. The Rust
    // server resolves public files before routing, so `/logo.png` never
    // reaches the router there; in a deploy the CDN checks the filesystem
    // first and then hands the miss to this function, where a bare dynamic
    // segment such as `/[lang]` happily captures `logo.png` and returns a 200
    // HTML document. Browsers then show a broken image, and every favicon or
    // asset miss costs a function invocation. Explicitly declared routes
    // (`/sitemap.xml`, `/api/data.json`) still match — only dynamic segments
    // are refused.
    if (isStaticAssetPath(pathname) && hasDynamicSegment(route.path)) {
      return new Response('Not Found', {
        status: 404,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
      })
    }

    // Check platform support for the route's strategy
    const strategy = route.kind === 'api' ? 'api' : route.render.strategy
    if (!supportedStrategies.includes(strategy)) {
      return new Response(
        `RUV2210 Platform does not support rendering strategy "${strategy}" for route ${route.path}. ` +
          `Supported: ${supportedStrategies.join(', ')}.`,
        { status: 501, headers: { 'content-type': 'text/plain; charset=utf-8' } },
      )
    }

    try {
      if (route.kind === 'api') {
        return await handleApi(route, request, params)
      }
      return await handlePage(route, request, pathname, params, runtimeContext)
    } catch (error) {
      // A handler that read past the body cap surfaces here as a stream error.
      // Reporting it as 413 rather than 500 keeps the answer identical to the
      // declared-length rejection above, so a client cannot tell which of the
      // two bounds caught it.
      if (isBodyLimitError(error)) {
        return textResponse(413, 'Request body is too large')
      }
      const message = error instanceof Error ? error.message : String(error)
      console.error(`[ruvyxa] Error handling ${logValue(pathname)}:`, logValue(message))
      // Log the detail server-side only: serverless is production, and the
      // dev server likewise never exposes internal error text to clients.
      return new Response('Internal Server Error', {
        status: 500,
        headers: { 'content-type': 'text/plain; charset=utf-8' },
      })
    }
  }

  async function handleApi(route, request, params) {
    const mod = await importApi(route.id)
    const method = request.method.toUpperCase()
    const selected = selectRouteHandler(mod, method)

    if (!selected) {
      const refusal = methodNotAllowed(mod, method)
      return new Response(refusal.body, {
        status: refusal.status,
        headers: { allow: refusal.allow, 'content-type': 'text/plain; charset=utf-8' },
      })
    }

    const context = requestContext({
      headerPairs: [...request.headers],
      method,
      url: new URL(request.url).pathname,
      params: params ?? {},
    })
    const result = await runWithRequestContext(context, () => selected.handler({ request, params }))
    recordRevalidations(collectRevalidations(context))
    const response = normalizeResponse(result, `${method} ${new URL(request.url).pathname}`)
    // A `HEAD` answered by the route's `GET` keeps every header and drops the
    // content. There is no transport under a serverless function to do it: the
    // `Response` goes to the platform as it is.
    if (!selected.omitBody) return response
    return new Response(null, { status: response.status, headers: response.headers })
  }

  /**
   * Serve a public, version-bound server-component data payload.
   *
   * A page opts in by exporting `flight(context)`. Its result must satisfy the
   * same bounded JSON contract as every other Flight payload. The endpoint is
   * deliberately unavailable to authenticated or cookie-bearing requests:
   * prefetching must never become a channel for private request state.
   */
  async function handleFlight(request, url) {
    if (request.method !== 'GET') {
      return new Response('Method Not Allowed', {
        status: 405,
        headers: { allow: 'GET', 'content-type': 'text/plain; charset=utf-8' },
      })
    }
    if (request.headers.has('authorization') || request.headers.has('cookie')) {
      return textResponse(403, 'Flight requests must not include private request state')
    }
    if (request.headers.get('x-ruvyxa-flight') !== '1') {
      return textResponse(400, 'Flight requests require the Ruvyxa navigation header')
    }

    const requestedPath = url.searchParams.get('path') ?? ''
    const requestedArtifact = url.searchParams.get('artifact') ?? ''
    const pathname = canonicalRoutePath(requestedPath)
    if (pathname === null || !/^[a-f0-9]{16}$/.test(requestedArtifact)) {
      return textResponse(400, 'Flight request has an invalid route or artifact')
    }
    const match = matchRoute(pathname)
    if (!match || match.route.kind !== 'page')
      return textResponse(404, 'Flight route was not found')
    if (match.route.artifactVersion !== requestedArtifact) {
      return textResponse(409, 'Flight artifact is stale or invalid')
    }

    try {
      const module = await importPage(match.route.id)
      if (typeof module.flight !== 'function') {
        return textResponse(501, 'This route does not expose a Flight payload')
      }
      const context = requestContext({
        headerPairs: [...request.headers],
        method: request.method,
        url: pathname,
        params: match.params ?? {},
      })
      const tree = await runWithRequestContext(context, () =>
        module.flight({ path: pathname, params: match.params ?? {} }),
      )
      if (usedRequestContext(context)) {
        return textResponse(403, 'Flight payload read private request state')
      }
      const payload = encodeFlightPayload({
        manifestVersion: requestedArtifact,
        route: pathname,
        tree,
      })
      return new Response(payload, {
        headers: {
          'content-type': 'application/vnd.ruvyxa.flight+json; charset=utf-8',
          'cache-control': 'private, no-store',
          vary: 'x-ruvyxa-flight',
        },
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      console.error(`[ruvyxa] Flight render ${logValue(pathname)} failed:`, logValue(message))
      return new Response(publicFlightError(error, pathname), {
        status: 500,
        headers: {
          'content-type': 'application/vnd.ruvyxa.flight+json; charset=utf-8',
          'cache-control': 'private, no-store',
          vary: 'x-ruvyxa-flight',
        },
      })
    }
  }

  /**
   * Refuse a `/__ruvyxa/rsc` request that is not provably same-origin.
   *
   * The pair `validate_action_request` and `server_components_request_rejection`
   * both apply, reading the same two policy fields, because this is the same
   * class of request: a `GET` renders the visitor's page with their cookies and
   * a `POST` runs a project server function.
   *
   * This endpoint's whole request-validation story used to be one custom header,
   * on the premise that a cross-origin page cannot set one without a preflight
   * nothing answers. `createFetchMiddleware`'s CORS layer wraps the entire
   * handler and answers preflights before `dispatch` is reached, so a project
   * that enabled CORS for its own API — `{ origins: ['*'], credentials: true }`
   * — silently turned the header requirement into a preflight that *is*
   * answered, and a third-party page could call any server function with the
   * visitor's cookies.
   *
   * Fail-closed with neither `Origin` nor `Sec-Fetch-Site` present is inherited
   * from `originIsCrossSite` and is deliberate: the browser halves of this
   * endpoint (`rsc-client-runtime.mjs` and `@ruvyxa/react`'s router) always run
   * in a browser, and a browser sends one of the two. A `curl` that sends
   * neither now gets 403 here, as it always has at `/__ruvyxa/action`.
   *
   * A deployed function has no transport peer to weigh `X-Forwarded-Proto`
   * against, so — exactly as `actionOriginIsCrossSite` does — the header is read
   * as stated: the platform's ingress is the trusted proxy by construction, and
   * the host comparison is the load-bearing check either way.
   */
  function serverComponentsRequestRejection(headers) {
    const trustedScheme = parseForwardedScheme(headers.get('x-forwarded-proto'))
    if (
      actionPolicy.sameOrigin !== false &&
      originIsCrossSite(headers, headers.get('host') ?? '', { trustedScheme })
    ) {
      return textResponse(403, 'Cross-origin server-components request blocked')
    }
    if (actionPolicy.fetchMeta !== false && fetchSiteIsCrossSite(headers)) {
      return textResponse(403, 'Cross-site server-components request blocked')
    }
    return null
  }

  /**
   * Fixed-window limiter for one server-function call, shaped like
   * `actionRateLimitResponse` and keyed like `rsc_action_rate_limit_key`.
   *
   * Client, route, and the function being called, so a page that issues several
   * server-function calls in one interaction spends a budget per function rather
   * than one between them — the granularity the action endpoint gets from naming
   * the action.
   *
   * The `rsc:` prefix keeps these buckets out of the action endpoint's: one map
   * serves both, and a reference and an action name are separate namespaces that
   * can legitimately spell the same thing on one route. The path is the
   * canonical one, so alternate spellings of one route cannot each open a fresh
   * bucket. Both are the native key's two deliberate differences, kept here so
   * the two hosts refuse the same call at the same count.
   */
  function rscActionRateLimitResponse(request, requestPath, reference) {
    const key = boundedKey(
      `rsc:${clientAddress(request.headers, trustedProxies, ingressHeaders)}:${requestPath}:${reference}`,
    )
    return consumeFixedWindow(actionBuckets, key, actionRateLimit.max, actionRateLimit.window, {
      message: 'Server-function rate limit exceeded',
    })
  }

  /**
   * The Flight payload for a soft navigation into a server-components route.
   *
   * Mirrors `rsc_payload_endpoint` in the native server: the same header gate,
   * the same origin and fetch-metadata pair, the same content type, and the same
   * `Vary` — the browser router calls one endpoint and must not be able to tell
   * which host answered.
   *
   * This reported 501 in every deployed build until the generated route
   * registry learned to render through the server-components pipeline, so a
   * navigation into such a route fell back to a full document load.
   */
  async function handleRscPayload(request, url) {
    if (request.method === 'POST') return handleRscAction(request, url)
    if (request.method !== 'GET') {
      return new Response('Method Not Allowed', {
        status: 405,
        headers: { allow: 'GET, POST', 'content-type': 'text/plain; charset=utf-8' },
      })
    }
    // The header a cross-origin page cannot set without a preflight this host
    // does not answer — unless the project configured one, which is why the
    // origin pair below runs as well and this is no longer the whole gate.
    if (request.headers.get(RSC_REQUEST_HEADER) !== '1') {
      return textResponse(400, 'Server-components payload requests require the Ruvyxa header')
    }
    const refused = serverComponentsRequestRejection(request.headers)
    if (refused) return refused

    const pathname = canonicalRoutePath(url.searchParams.get('path') ?? '')
    if (pathname === null) return textResponse(400, 'Payload request has an invalid route')
    const match = matchRoute(pathname)
    if (!match || match.route.kind !== 'page') {
      return textResponse(404, 'Payload route was not found')
    }

    try {
      const module = await importPage(match.route.id)
      if (typeof module.rscPayload !== 'function') {
        return textResponse(
          501,
          'This route does not render through the server-components pipeline',
        )
      }
      const context = requestContext({
        headerPairs: [...request.headers],
        method: request.method,
        url: pathname,
        params: match.params ?? {},
      })
      const payload = await runWithRequestContext(context, () =>
        module.rscPayload({ path: pathname, params: match.params ?? {} }),
      )
      return new Response(payload, {
        headers: {
          'content-type': 'text/x-component; charset=utf-8',
          'cache-control': 'private, no-store',
          vary: RSC_REQUEST_HEADER,
        },
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      console.error(
        `[ruvyxa] Server-components payload ${logValue(pathname)} failed:`,
        logValue(message),
      )
      // The text is deliberately not the error's: this endpoint answers a
      // browser, and the native server does not expose internal render text
      // either.
      return textResponse(500, 'Server-components payload render failed')
    }
  }

  /**
   * Run one of a server-components route's server functions.
   *
   * `POST` to the same path that serves the route's payload, because it is the
   * same question asked twice — `GET` renders the route, `POST` runs one of the
   * functions it exposes — and because a second path would mean a second
   * reserved route and a second place the same-origin header is checked. This
   * mirrors `rsc_action_endpoint` in the native server, down to the header names
   * and the body bound.
   *
   * Nothing answered this in a deployed build until now: the endpoint accepted
   * `GET` and refused everything else with a `405`. Clicking anything wired to a
   * server function on a deployed server-components page therefore threw
   * `Connection closed.` in the browser and blanked the document, while the same
   * page worked under `ruvyxa dev` and `ruvyxa start`. It is the third time a
   * capability has existed on one of the two request hosts and not the other,
   * which is what `tests/fixtures/framework-endpoint-conformance.json` exists to catch.
   */
  async function handleRscAction(request, url) {
    if (request.headers.get(RSC_REQUEST_HEADER) !== '1') {
      return textResponse(400, 'Server-components payload requests require the Ruvyxa header')
    }
    const refused = serverComponentsRequestRejection(request.headers)
    if (refused) return refused
    const reference = request.headers.get(SERVER_ACTION_HEADER) ?? ''
    if (reference === '') {
      return textResponse(400, 'Server-function calls must name a reference')
    }

    const pathname = canonicalRoutePath(url.searchParams.get('path') ?? '')
    if (pathname === null) return textResponse(400, 'Payload request has an invalid route')

    // After validation and before anything is imported or run, the position
    // `handleServerAction` uses: a malformed request is cheap to refuse and must
    // not consume a client's budget. The key is the canonical request path, the
    // same string the native host takes from its resolved route, so both hosts
    // bucket one route's calls together however the caller spelled it.
    //
    // Ahead of the route lookup rather than behind it, which is the one place
    // this differs from the native ordering: an unmatched path is answered from
    // this map instead of costing a matcher walk per request, and the two hosts
    // still refuse a real call at the same count.
    const limited = rscActionRateLimitResponse(request, pathname, reference)
    if (limited) return limited

    const match = matchRoute(pathname)
    if (!match || match.route.kind !== 'page') {
      return textResponse(404, 'Payload route was not found')
    }

    // `encodeReply` produces a string for plain arguments and `FormData` when
    // one of them is a file or a stream, and `decodeReply` has to be handed the
    // same kind back. The size bound is applied by `requestBodyPolicy` rather
    // than here, so it covers both shapes and refuses before the body is read.
    const contentType = request.headers.get('content-type') ?? 'text/plain;charset=UTF-8'
    let body
    try {
      body = contentType.toLowerCase().startsWith('multipart/form-data')
        ? await request.formData()
        : await request.text()
    } catch {
      return textResponse(400, 'Server-function call body could not be read')
    }

    try {
      const module = await importPage(match.route.id)
      if (typeof module.rscAction !== 'function') {
        // Distinguishable from 404 on purpose, and from the payload endpoint's
        // 501: this route renders through the pipeline but declares no
        // `'use server'` function, so there was nothing to build a bundle from.
        return textResponse(501, 'RUV1866 this route declares no server functions')
      }
      const context = requestContext({
        headerPairs: [...request.headers],
        method: 'POST',
        url: pathname,
        params: match.params ?? {},
      })
      const payload = await runWithRequestContext(context, () =>
        module.rscAction({ reference, body }),
      )
      recordRevalidations(collectRevalidations(context))
      return new Response(payload, {
        headers: {
          'content-type': 'text/x-component; charset=utf-8',
          'cache-control': 'private, no-store',
          vary: RSC_REQUEST_HEADER,
        },
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      console.error(
        `[ruvyxa] Server function ${logValue(reference)} on ${logValue(pathname)} failed:`,
        logValue(message),
      )
      return textResponse(500, 'Server function failed')
    }
  }

  /** Apply the `revalidatePath()` calls a handler made, with the shared bounds. */
  function recordRevalidations(revalidations) {
    if (revalidations.length > MAX_REVALIDATIONS_PER_REQUEST) {
      failClosedRevalidations(
        `[ruvyxa] Received more than ${MAX_REVALIDATIONS_PER_REQUEST} revalidations from one request; ` +
          'bypassing prerendered artifacts for this instance.',
      )
      return
    }
    for (const path of revalidations) {
      if (
        typeof path !== 'string' ||
        !path.startsWith('/') ||
        path.length > MAX_REVALIDATION_PATH_LENGTH
      ) {
        console.warn('[ruvyxa] Ignoring revalidatePath() for an unusable path.')
        continue
      }
      markForcedRevalidation(path)
    }
  }

  /**
   * Serve `POST /__ruvyxa/action`.
   *
   * The native server exposes this endpoint from its router and validates it in
   * `action_security.rs`. Nothing served it here, so every `<form
   * action="/__ruvyxa/action?...">` — the shape the `crud` template, the demo,
   * and `ruvyxa add` all generate — fell through to route matching and returned
   * 404 in production while working under `ruvyxa dev`.
   *
   * The checks run in the same order as the native ones so a request accepted
   * locally is accepted here and vice versa; they live in `action-runtime.mjs`,
   * which documents what it tracks on the Rust side.
   */
  async function handleServerAction(request, url) {
    if (request.method !== 'POST') {
      return new Response('Method Not Allowed', {
        status: 405,
        headers: { allow: 'POST', 'content-type': 'text/plain; charset=utf-8' },
      })
    }
    if (typeof importAction !== 'function') {
      // Distinguishable from 404 on purpose: 404 would read as "this project
      // has no such action", when the real cause is a function bundle built
      // without the action registry.
      return textResponse(501, 'RUV2211 This deployment was built without server action support.')
    }

    const targetPath = url.searchParams.get('path') ?? ''
    const actionName = url.searchParams.get('name') ?? ''
    const actionReference = url.searchParams.get('id')
    if (actionName === '' || !targetPath.startsWith('/')) {
      return textResponse(400, 'Action request must name a target path and an action')
    }
    const canonicalTarget = canonicalRoutePath(targetPath)
    if (canonicalTarget === null) {
      return textResponse(400, 'Action target path contains an unsafe encoded segment')
    }

    // Refuse on the declared length before buffering, so an oversized payload
    // never becomes an allocation.
    const oversized = declaredBodyTooLarge(
      request,
      actionPolicy.actionLimit,
      'Action payload is too large',
    )
    if (oversized) return oversized

    try {
      const buffer = await request.arrayBuffer()
      const rejected = validateActionRequest(request.headers, buffer.byteLength, actionPolicy)
      if (rejected) return rejected

      let payloadText
      try {
        payloadText = new TextDecoder('utf-8', { fatal: true }).decode(buffer)
      } catch {
        return textResponse(400, 'Action payload must be valid UTF-8')
      }
      const validated = validateActionPayload(request.headers, payloadText)
      if (validated.response) return validated.response

      // Rate limiting comes after validation and before the action runs, the
      // same position the native endpoint uses: a malformed request is cheap to
      // reject and must not consume a client's budget.
      const limited = actionRateLimitResponse(request, canonicalTarget, actionName)
      if (limited) return limited

      const match = matchRoute(canonicalTarget)
      if (!match) return textResponse(404, 'Route not found for action')
      if (match.route.kind !== 'page') {
        return textResponse(405, 'Actions can only target page routes')
      }
      if (actionReference !== null) {
        if (actionReference !== match.route.actionReferenceId) {
          return textResponse(409, 'Action reference is stale or invalid')
        }
        const nonceRejection = consumeActionNonce(request.headers, actionReference)
        if (nonceRejection) return nonceRejection
      }

      const module = await importAction(match.route.id)
      if (!module) {
        return textResponse(404, 'Route action file was not found')
      }

      // `realtimeEvent` is deliberately dropped. `realtime@1` is a native-host
      // capability that no build artifact carries -- `ruvyxa build` warns
      // RUV2205 about exactly that -- so this host has nowhere to publish the
      // event to and no reader downstream to strip it. It used to arrive as an
      // `x-ruvyxa-realtime-event` header on the response, and since the
      // function's response is what the browser receives, every action on a
      // realtime-declaring route published its channel list and every key it
      // passed to `invalidate()` to the client.
      const { response, revalidate } = await runAction({
        module,
        actionName,
        payload: validated.payload,
        contentType: validated.contentType,
        requestPath: canonicalTarget,
        headerPairs: [...request.headers],
      })
      recordRevalidations(revalidate)
      return response
    } catch (error) {
      if (isBodyLimitError(error)) {
        return textResponse(413, 'Action payload is too large')
      }
      const message = error instanceof Error ? error.message : String(error)
      console.error(
        `[ruvyxa] Server action ${logValue(actionName)} on ${logValue(canonicalTarget)} failed:`,
        logValue(message),
      )
      // Serverless is production: the detail stays in the platform log, exactly
      // as the native server does outside dev.
      return textResponse(500, 'Internal Server Error')
    }
  }

  function consumeActionNonce(headers, actionReference) {
    const nonce = headers.get('x-ruvyxa-action-nonce') ?? ''
    if (!/^[A-Za-z0-9._~-]{16,128}$/.test(nonce)) {
      return textResponse(400, 'Versioned action requests require a valid replay nonce')
    }
    const client = clientAddress(headers, trustedProxies, ingressHeaders)
    const now = Date.now()

    // Every nonce is stored with the same TTL, so a Map's insertion order is
    // also its expiry order and the first live entry ends the sweep. Scanning
    // the whole map instead walked up to `MAX_ACTION_NONCES` entries on every
    // versioned action request, for a prefix that is all this can ever remove.
    // A backwards wall-clock step can leave an entry behind the one in front of
    // it; that only delays its removal until a later sweep reaches it, and a
    // nonce held longer than its TTL is refused, never wrongly accepted.
    for (const [key, entry] of actionNonces) {
      if (entry.expires > now) break
      actionNonces.delete(key)
      releaseActionNonceClient(entry.client)
    }

    const key = `${actionReference}:${nonce}`
    if (actionNonces.has(key)) return textResponse(409, 'Action request replayed')

    // This address is over its share of the pool. Checked before the global
    // bound so the caller that filled the map is the one refused, rather than
    // whichever request happens to arrive next. 429 rather than 503: only this
    // address is over its quota, and the instance still serves everyone else.
    if ((actionNonceClients.get(client) ?? 0) >= MAX_ACTION_NONCES_PER_CLIENT) {
      return textResponse(429, 'Action replay protection is saturated for this client')
    }

    // Full, with nothing expired left to drop. Evicting the oldest live nonce
    // to make room would accept that nonce's replay — the one thing this map
    // exists to refuse — and an attacker can reach this state on purpose by
    // sending fresh nonces. Saturation fails closed, the same choice
    // `failClosedRevalidations` makes when its own bound is reached.
    if (actionNonces.size >= MAX_ACTION_NONCES) {
      return textResponse(503, 'Action replay protection is saturated')
    }

    actionNonces.set(key, { expires: now + ACTION_NONCE_TTL_MS, client })
    actionNonceClients.set(client, (actionNonceClients.get(client) ?? 0) + 1)
    return null
  }

  /**
   * Drop one live entry from a client's count, forgetting the address when its
   * last nonce expires so `actionNonceClients` stays bounded by the pool.
   *
   * @param {string} client
   */
  function releaseActionNonceClient(client) {
    const held = actionNonceClients.get(client)
    if (held === undefined) return
    if (held <= 1) actionNonceClients.delete(client)
    else actionNonceClients.set(client, held - 1)
  }

  /**
   * Fixed-window action rate limiter, keyed per client, path, and action.
   *
   * Mirrors `action_rate_limit_key`. The native limiter hashes into a fixed
   * slot array because it must survive an address-rotating attacker on a
   * long-lived process; a function instance is short-lived and already bounded
   * by `MAX_TRACKED_RATE_LIMIT_KEYS`, so the simpler map used by the built-in
   * middleware is reused here rather than adding a second scheme.
   */
  function actionRateLimitResponse(request, targetPath, actionName) {
    // Bounded for the same reason the built-in limiter's key is: the path is
    // caller-written and this map retains ten thousand of whatever it is
    // handed. The client is at the front of the identity, so a caller cannot
    // reach another client's bucket by lengthening the part it controls.
    const key = boundedKey(
      `${clientAddress(request.headers, trustedProxies, ingressHeaders)}:${targetPath}:${actionName}`,
    )
    return consumeFixedWindow(actionBuckets, key, actionRateLimit.max, actionRateLimit.window, {
      message: 'Action rate limit exceeded',
    })
  }

  /**
   * Serve a stored HTML document as-is.
   *
   * Marked validatable: these are the bytes the build or a revalidation stored,
   * so two readers of this URL receive the same document and a reader who
   * already holds it can be told so. See `DOCUMENT_VALIDATOR_HEADER` for why the
   * ETag is not written here.
   */
  function prerenderedResponse(html, extraHeaders = {}) {
    return new Response(html, {
      status: 200,
      headers: {
        'content-type': 'text/html; charset=utf-8',
        [DOCUMENT_VALIDATOR_HEADER]: '1',
        ...extraHeaders,
      },
    })
  }

  /**
   * Store a fresh render and settle the `revalidatePath()` claim that asked for it.
   *
   * A page that read cookies, headers, or draft mode rendered for one visitor.
   * Writing it to the shared cache would serve that visitor's page to everyone
   * who asks for this URL next, so `requestScoped` renders are never stored.
   */
  async function persistForcedRender(pathname, forcedClaim, rendered, revalidate) {
    if (!writePrerendered || rendered.status !== 200 || rendered.requestScoped) return
    await persistPrerendered(pathname, await rendered.clone().text(), revalidate, true)
    acknowledgeForcedRevalidation(pathname, forcedClaim)
  }

  /**
   * CSR and SSG: serve what the build produced, and render only when there is
   * nothing stored or `revalidatePath()` invalidated it.
   *
   * The two strategies differ in what the stored document contains — a shell
   * versus a full page — which the build decided. At request time they are the
   * same lookup, so they share one path rather than two identical ones.
   */
  async function servePrerendered(route, request, pathname, params, forced, forcedClaim) {
    const cached = forced ? null : normalizeCacheEntry(await readPrerendered?.(pathname))
    if (cached) return prerenderedResponse(cached.html)

    const rendered = await renderPage(route, pathname, params, request)
    if (forced) await persistForcedRender(pathname, forcedClaim, rendered)
    return rendered
  }

  /** ISR: serve the cached copy, and refresh it behind the response when stale. */
  async function serveIncremental(
    route,
    request,
    pathname,
    params,
    runtimeContext,
    forced,
    forcedClaim,
  ) {
    const revalidate = route.render.revalidate ?? 60
    const cached = forced
      ? null
      : normalizeCacheEntry(await readPrerendered?.(pathname, revalidate))
    if (cached) {
      if (cached.stale) await refreshStaleEntry(route, pathname, params, runtimeContext)
      return prerenderedResponse(cached.html, {
        'x-ruvyxa-isr': 'HIT',
        'cache-control': documentCacheControl('isr', revalidate),
      })
    }

    const rendered = await renderPage(route, pathname, params, request)
    if (writePrerendered && rendered.status === 200 && !rendered.requestScoped) {
      const body = await rendered.clone().text()
      const stored = await persistPrerendered(pathname, body, revalidate, forced)
      if (forced && stored) acknowledgeForcedRevalidation(pathname, forcedClaim)
    }
    return rendered
  }

  /**
   * Kick off a background refresh for a stale ISR entry.
   *
   * A serverless runtime may freeze untracked work as soon as the response is
   * returned, so the refresh is handed to `waitUntil` when the platform offers
   * one. Awaiting it is slower, but it is the only way not to lose the refresh
   * on a platform that exposes no lifetime hook.
   */
  async function refreshStaleEntry(route, pathname, params, runtimeContext) {
    const revalidation = scheduleRevalidation(route, pathname, params)
    if (!revalidation) return
    if (typeof runtimeContext.waitUntil === 'function') {
      runtimeContext.waitUntil(revalidation)
      return
    }
    await revalidation
  }

  /**
   * Serve a page, and say how long the answer may be reused.
   *
   * Every document leaving this handler carries a `cache-control`. It did not
   * before, and "no header" is not "do not cache" to a shared cache: a CDN
   * given nothing may store a response under heuristic freshness, which on an
   * `ssr` page means one visitor's document served to the next. The policy
   * itself is one table — `documentCacheControl` — so the value a deployed
   * function sends and the value the native host sends for the same route
   * cannot drift. A header the render already set always wins: a page that
   * chose its own caching meant it.
   *
   * `requestScoped` outranks the strategy. `documentCacheControl` describes the
   * *route*; `requestScoped` describes *this response*, and a render that read
   * cookies, headers, or draft mode produced one visitor's document. Telling a
   * shared cache it may reuse that for the route's window is how one visitor's
   * page reaches everybody else — the same reason the stores above refuse it,
   * applied to the other cache the response can reach. Mirrors the `formData`
   * branch in `servePageDocument`, which answers `no-store` for exactly this
   * reason, and `insert_document_cache_control` on the native host.
   */
  async function handlePage(route, request, pathname, params, runtimeContext) {
    const rendered = await servePageDocument(route, request, pathname, params, runtimeContext)
    // Only a document this handler produced. A redirect carries immutable
    // headers and describes no body to cache, and an error response is not the
    // page whose policy this is.
    if (rendered.status !== 200 || rendered.headers.has('cache-control')) return rendered
    try {
      rendered.headers.set(
        'cache-control',
        rendered.requestScoped
          ? 'no-store'
          : documentCacheControl(route.render.strategy, route.render.revalidate),
      )
    } catch {
      // A response whose headers are immutable already decided.
    }
    return rendered
  }

  async function servePageDocument(route, request, pathname, params, runtimeContext) {
    const strategy = route.render.strategy
    const forcedClaim = claimForcedRevalidation(pathname)
    const forced = forcedClaim !== null

    // A form whose `action` is a server function and whose page has no
    // JavaScript posts to the page's own URL: React writes the reference into
    // hidden fields rather than into an `action` attribute, so there is no
    // other endpoint for it to reach. `posted_form()` recognises this on the
    // native host; nothing here did, so the deployed build re-rendered the page
    // and dropped the submission on the floor — a `200` with the initial state
    // in it, which is indistinguishable from a form that was never submitted.
    //
    // Ahead of the strategy switch, and answering `no-store` whatever the route
    // says: a `ssg` route serves a file to readers and renders to submitters,
    // and the answer belongs to one visitor.
    const formData = await postedFormData(route, request)
    if (formData) {
      const rendered = await renderPage(route, pathname, params, request, formData)
      rendered.headers.set('cache-control', 'no-store')
      return rendered
    }

    if (strategy === 'csr' || strategy === 'ssg') {
      return servePrerendered(route, request, pathname, params, forced, forcedClaim)
    }
    if (strategy === 'isr') {
      return serveIncremental(route, request, pathname, params, runtimeContext, forced, forcedClaim)
    }

    // PPR falls back to a full render here: streaming the shell separately needs
    // a platform hook this generic handler does not have, and a platform wrapper
    // overrides it where one exists.
    if (strategy === 'ppr') {
      const rendered = await renderPage(route, pathname, params, request)
      if (forced) {
        await persistForcedRender(pathname, forcedClaim, rendered, route.render.revalidate ?? 60)
      }
      return rendered
    }

    // SSR (default): full server render, nothing stored — the one strategy
    // whose document may leave while React is still writing it.
    const rendered = await renderPage(route, pathname, params, request, null, mayStream(route))
    if (forced && rendered.status >= 200 && rendered.status < 400) {
      acknowledgeForcedRevalidation(pathname, forcedClaim)
    }
    return rendered
  }

  /**
   * Render a page with the request as ambient context.
   *
   * `requestScoped` reports whether the render read that context. A caller that
   * was about to store the HTML — the ISR cache below — must not, because the
   * document belongs to whoever sent this request.
   */
  /**
   * The submitted form a server-components route should run an action from.
   *
   * `null` for everything else, which is every other request: a different verb,
   * a route that does not render through the pipeline, an empty body, or a
   * content type a `<form>` cannot produce. The two encodings are the ones
   * React asks for when it writes the hidden fields and the one a form that
   * overrode `encType` sends; both decode to `FormData`. Same test as
   * `posted_form()` on the native host, in the same order.
   */
  async function postedFormData(route, request) {
    if (route.render?.serverComponents !== true) return null
    if (!request || request.method !== 'POST') return null
    const essence = (request.headers.get('content-type') ?? '').split(';')[0].trim().toLowerCase()
    if (essence !== 'multipart/form-data' && essence !== 'application/x-www-form-urlencoded') {
      return null
    }
    try {
      const formData = await request.formData()
      // An empty submission carries no reference, so there is nothing to run
      // and the ordinary render is the right answer.
      return [...formData.keys()].length > 0 ? formData : null
    } catch {
      return null
    }
  }

  /**
   * Whether this route's document may leave before it is finished.
   *
   * Three things need the whole string and none of them can have it from a
   * stream: the ISR and pre-render writes, the `requestScoped` check that
   * decides whether a write is even allowed — a `Suspense` child that reads
   * cookies does so long after the shell has gone out, so the answer at return
   * time would be a lie — and `localizeHtmlDocument`, which rewrites the
   * `<html>` tag of a `[locale]` route. So only the plain SSR path streams, and
   * only when i18n has nothing to say about this route.
   */
  function mayStream(route) {
    if (!i18n) return true
    return route.path.split('/')[1] !== `[${i18n.localeParam}]`
  }

  async function renderPage(route, pathname, params, request, formData = null, stream = false) {
    const mod = await importPage(route.id)
    const context = requestContext({
      headerPairs: [...(request?.headers ?? [])],
      method: request?.method ?? 'GET',
      url: pathname,
      params: params ?? {},
    })
    const rendered = await runWithRequestContext(context, () =>
      mod.render({ path: pathname, params: params ?? {}, formData, stream }),
    )
    if (typeof rendered !== 'string') {
      // A streamed document. Nothing downstream stores it — `mayStream` is what
      // guarantees that — so there is no `.text()` here to undo the streaming.
      const response = new Response(rendered, {
        status: 200,
        headers: { 'content-type': 'text/html; charset=utf-8' },
      })
      response.requestScoped = true
      return response
    }
    // Only for a submission. An ordinary page render calling `revalidatePath()`
    // is not a thing this host has ever applied, and starting to would change
    // every route's behaviour; a server function called by the form it was
    // posted to is exactly the case where the native host applies them.
    if (formData) recordRevalidations(collectRevalidations(context))
    const html = localizeHtmlDocument(rendered, route.path, pathname, params ?? {}, i18n)
    const response = new Response(html, {
      status: 200,
      headers: { 'content-type': 'text/html; charset=utf-8' },
    })
    response.requestScoped = usedRequestContext(context)
    return response
  }

  function scheduleRevalidation(route, pathname, params) {
    if (!writePrerendered) return null
    const pending = pendingRevalidations.get(pathname)
    if (pending) return pending
    const revalidation = Promise.resolve().then(async () => {
      try {
        const mod = await importPage(route.id)
        // Deliberately rendered with no request context. A background
        // revalidation has no visitor, so `cookies()` throws its
        // "called outside a request" error rather than quietly producing a
        // page built from nobody's session and caching it for everybody.
        const rendered = await mod.render({ path: pathname, params: params ?? {} })
        const html = localizeHtmlDocument(rendered, route.path, pathname, params ?? {}, i18n)
        await persistPrerendered(pathname, html, route.render.revalidate ?? 60)
      } catch (error) {
        console.error(`[ruvyxa] ISR revalidation failed for ${logValue(pathname)}:`, error)
      } finally {
        pendingRevalidations.delete(pathname)
      }
    })
    pendingRevalidations.set(pathname, revalidation)
    return revalidation
  }
}

/**
 * Whether `src` names a file under the project's `public/` directory.
 *
 * The rule `optimize` in `crates/ruvyxa_dev_server/src/dynamic_image.rs`
 * applies before it touches the disk, and it has to be the same rule: one URL
 * that answers under `ruvyxa start` and 400s on a deployed build — or the
 * reverse — is a difference nothing about the markup or the status code shows.
 *
 * `.`, `..`, a backslash, a colon and the control characters are refused by
 * `isUnsafeSegment`, which is this module's one answer to what may become a
 * path. A query or fragment is refused outright: a `src` names a file rather
 * than a URL, and an adapter that forwards it to a platform optimizer would
 * otherwise hand that optimizer the rest of the string as parameters of its
 * own.
 */
function isPublicImageSource(src) {
  if (typeof src !== 'string') return false
  if (!src.startsWith('/') || src.startsWith('//')) return false
  if (src.includes('?') || src.includes('#')) return false
  const segments = src.split('/').filter((segment) => segment !== '')
  if (segments.length === 0) return false
  return !segments.some(isUnsafeSegment)
}

async function handleDynamicImage(request, optimizer, defaultQuality = DEFAULT_IMAGE_QUALITY) {
  if (typeof optimizer !== 'function') return new Response('Not Found', { status: 404 })
  if (!['GET', 'HEAD'].includes(request.method)) {
    return new Response('Method Not Allowed', { status: 405, headers: { allow: 'GET, HEAD' } })
  }
  const url = new URL(request.url)
  const src = url.searchParams.get('src')
  const width = Number(url.searchParams.get('w'))
  const requestedQuality = url.searchParams.get('q')
  // An absent `q` takes the project's configured quality, the same value
  // `dynamic_image.rs` reaches for. A present one is still validated, so an
  // empty or non-numeric `q` is a 400 rather than a silent fall back to the
  // default — the caller asked for something and got something else.
  const quality = requestedQuality === null ? defaultQuality : Number(requestedQuality)
  if (
    !isPublicImageSource(src) ||
    !Number.isInteger(width) ||
    width < MIN_IMAGE_WIDTH ||
    width > MAX_IMAGE_WIDTH ||
    !Number.isInteger(quality) ||
    quality < MIN_IMAGE_QUALITY ||
    quality > MAX_IMAGE_QUALITY
  ) {
    return new Response('Invalid image request', {
      status: 400,
      headers: { 'content-type': 'text/plain; charset=utf-8' },
    })
  }
  return optimizer(request, { src, width, quality })
}

/**
 * The prefixed path an unprefixed URL belongs at, or `null` to leave it alone.
 *
 * Mirrored by `locale_redirect_path` in `crates/ruvyxa_dev_server/src/i18n.rs`,
 * and both replay `tests/fixtures/i18n-routing-conformance.json`. Two things
 * here used to differ from that host, and neither was visible because every
 * test on both sides passed `detectLocale: true` and an ordinary path:
 *
 * `detectLocale: false` returned `null` from this function, which turned locale
 * routing off rather than locale *detection* off — a deployed build answered
 * every unprefixed URL with a 404 while `ruvyxa dev` served it. The option
 * chooses which signals name the locale; with it off the default locale is the
 * answer, and the redirect still happens. `preferredLocale` holds that now.
 *
 * The reserved namespace was excluded by byte prefix, so `/__ruvyxa-notes` — a
 * page a project may legitimately own — was redirected under `ruvyxa dev` and
 * silently excluded once deployed. It is matched by whole segment now, as
 * `/api` already was.
 *
 * `search` is the request's query string including its leading `?`, or `''`.
 * Both hosts built `Location` from the path alone, so `GET /about?q=hello`
 * answered `/en/about` and every query-bearing entry point on an i18n site lost
 * its parameters on the first, unprefixed hit — search, pagination, UTM tags,
 * an OAuth `?code=`/`?state=` callback. A 307 preserves the method and the body
 * and says nothing about the query: the query is part of the target URI and has
 * to be reproduced explicitly.
 *
 * @param {Request} request
 * @param {string} pathname
 * @param {string} search
 * @param {string} basePath
 * @param {(pathname: string) => { route: { kind: string } } | null} matchRoute
 * @param {Record<string, unknown> | null | undefined} config
 * @returns {string | null}
 */
function localeRedirect(request, pathname, search, basePath, matchRoute, config) {
  if (
    !config ||
    !['GET', 'HEAD'].includes(request.method) ||
    pathname === '/__ruvyxa' ||
    pathname.startsWith('/__ruvyxa/') ||
    pathname === '/api' ||
    pathname.startsWith('/api/') ||
    isStaticAssetPath(pathname) ||
    pathLocale(pathname, config)
  )
    return null

  const preferred = preferredLocale(request.headers, config)
  for (const locale of [preferred, config.defaultLocale]) {
    const candidate = pathname === '/' ? `/${locale}` : `/${locale}${pathname}`
    const matched = matchRoute(candidate)
    if (matched?.route.kind === 'page') {
      // `URL.search` is `''` for both an absent query and a bare `?`, so an
      // empty query never leaves a dangling `?` on the target — which is the
      // same answer `locale_redirect_path` gives after splitting the raw
      // request target.
      return `${basePath === '/' ? '' : basePath}${candidate}${search ?? ''}`
    }
  }
  return null
}

function preferredLocale(headers, config) {
  // Detection off means the cookie and Accept-Language are not consulted, not
  // that locale routing stops — `preferred_locale` in
  // `crates/ruvyxa_dev_server/src/i18n.rs` answers the same way.
  if (config.detectLocale === false) return config.defaultLocale
  const locales = Array.isArray(config.locales) ? config.locales : []
  const canonical = (value) =>
    locales.find((locale) => locale.toLowerCase() === value.toLowerCase())
  const cookie = headers.get('cookie') ?? ''
  for (const part of cookie.split(';')) {
    const separator = part.indexOf('=')
    if (separator < 0 || part.slice(0, separator).trim() !== config.cookie) continue
    const locale = canonical(part.slice(separator + 1).trim())
    if (locale) return locale
  }

  const languages = (headers.get('accept-language') ?? '')
    .split(',')
    .map((entry) => {
      const [language, ...parameters] = entry.trim().split(';')
      const quality = parameters.map((part) => part.trim()).find((part) => part.startsWith('q='))
      return { language, quality: quality ? Number(quality.slice(2)) : 1 }
    })
    .filter(({ language, quality }) => language && language !== '*' && quality > 0)
    .sort((left, right) => right.quality - left.quality)
  for (const { language } of languages) {
    const exact = canonical(language)
    if (exact) return exact
    const primary = language.split('-')[0].toLowerCase()
    const matched = locales.find((locale) => locale.split('-')[0].toLowerCase() === primary)
    if (matched) return matched
  }
  return config.defaultLocale
}

function pathLocale(pathname, config) {
  const first = pathname.replace(/^\//, '').split('/')[0]
  return config.locales?.find((locale) => locale.toLowerCase() === first.toLowerCase()) ?? null
}

function localizeHtmlDocument(html, routePath, pathname, params, config) {
  if (!config || typeof html !== 'string') return html
  const marker = `[${config.localeParam}]`
  if (routePath.split('/')[1] !== marker) return html
  const locale = pathLocale(`/${String(params[config.localeParam] ?? '')}`, config)
  if (!locale) return html
  const rest = pathname.replace(/^\//, '').split('/').slice(1).join('/')
  const localizedPath = (alternate) => (rest ? `/${alternate}/${rest}` : `/${alternate}`)
  const links = [
    ...config.locales.map(
      (alternate) =>
        `<link rel="alternate" hreflang="${escapeHtmlAttribute(alternate)}" href="${escapeHtmlAttribute(localizedPath(alternate))}">`,
    ),
    `<link rel="alternate" hreflang="x-default" href="${escapeHtmlAttribute(localizedPath(config.defaultLocale))}">`,
  ].join('')
  let document = html.replace(/<html(?:\s[^>]*)?>/i, (tag) => {
    if (/\slang\s*=/i.test(tag))
      return tag.replace(/\slang\s*=\s*(["']).*?\1/i, ` lang="${locale}"`)
    return tag.replace(/>$/, ` lang="${locale}">`)
  })
  if (!document.includes('hreflang=')) {
    document = /<\/head>/i.test(document)
      ? document.replace(/<\/head>/i, `${links}</head>`)
      : document.replace(/<body(?:\s[^>]*)?>/i, `${links}$&`)
  }
  return document
}

// Exported for `tests/packages/ruvyxa/document-head-parity.test.mjs`, which
// replays the shared escaping table against this copy and the two others. An
// unexported copy is one the fixture cannot reach, which is the whole failure
// this table exists to stop.
export function escapeHtmlAttribute(value) {
  return (
    String(value)
      .replaceAll('&', '&amp;')
      .replaceAll('"', '&quot;')
      .replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;')
      // `&#39;`, not `&apos;`: HTML 4 does not define the latter. Kept level with
      // `escape_html` on the native host, which escapes the same values for the
      // same documents -- a character taught to one and not the other is how the
      // two hosts come to emit different bytes for one input.
      .replaceAll("'", '&#39;')
  )
}

export const MAX_TRACKED_RATE_LIMIT_KEYS = 10_000

/**
 * The widest a tracked rate-limit key may be, in characters.
 *
 * Matches the sixty-four hex digits `bounded_key` in
 * `crates/ruvyxa_middleware/src/builtin.rs` produces, so
 * `tests/fixtures/rate-limit-conformance.json` can hold one bound over both
 * hosts.
 */
const MAX_RATE_LIMIT_KEY_LENGTH = 64

/**
 * Sixteen hex digits that separate two identities sharing a long prefix.
 *
 * Not a cryptographic hash and not offered as one: two thirty-two-bit FNV-1a
 * passes with different offset bases, spliced. It is only ever appended *after*
 * the first characters of the identity it describes, so producing the same
 * bounded key as another client still means reproducing that client's prefix —
 * which is the part an attacker does not have. `Math.imul` keeps it in the
 * integer fast path; a BigInt loop over a 16 KB header value would hand the
 * flood this function exists to bound a second cost to inflict.
 *
 * @param {string} value
 */
function identityDigest(value) {
  let low = 0x811c9dc5
  let high = 0x01000193
  const bytes = new TextEncoder().encode(value)
  for (let index = 0; index < bytes.length; index += 1) {
    low = Math.imul(low ^ bytes[index], 0x01000193)
    high = Math.imul(high ^ bytes[index], 0x85ebca6b)
  }
  return `${(low >>> 0).toString(16).padStart(8, '0')}${(high >>> 0).toString(16).padStart(8, '0')}`
}

/**
 * The fixed-width map key one client identity is tracked under.
 *
 * The identity is not bounded and is not ours: `key: "header:x-api-key"` takes
 * the header verbatim, and `crates/ruvyxa_middleware/src/stack.rs` accepts any
 * valid header name for `key:`, so the only limit on a tracked key was the
 * server's header size
 * limit — ten thousand of those retain tens of megabytes. The crate's "bounded
 * memory" promise was true of the *count* and not of the *size*. This makes it
 * true of both.
 *
 * The native twin hashes with blake3, which this host cannot reach: this module
 * is copied verbatim into edge bundles where `node:crypto` does not exist, and
 * `crypto.subtle` is async while this limiter is not. So an identity already
 * inside the bound is kept as it is, and a longer one is truncated onto a digest
 * of the whole. What the two hosts must agree on is the observable contract —
 * bounded length, one key per identity, and no two identities collapsing into
 * one bucket — and that is what
 * `tests/fixtures/rate-limit-conformance.json` holds them to.
 *
 * @param {string} identity
 */
export function boundedKey(identity) {
  const value = String(identity)
  if (value.length <= MAX_RATE_LIMIT_KEY_LENGTH) return value
  return `${value.slice(0, MAX_RATE_LIMIT_KEY_LENGTH - 17)}#${identityDigest(value)}`
}

/** Compile validated built-in middleware into a Fetch-native wrapper. */
function createFetchMiddleware(config, trustedProxies = [], ingressHeaders = [], logAs = 'text') {
  const builtin = config?.builtin
  if (!builtin || typeof builtin !== 'object') {
    return async (_request, next) => next()
  }

  const cors = builtin.cors && typeof builtin.cors === 'object' ? builtin.cors : null
  const rate = builtin.rate && typeof builtin.rate === 'object' ? builtin.rate : null
  const customHeaders = validHeaderEntries(builtin.headers)
  const buckets = new Map()
  let nextRequestId = 1

  return async function applyFetchMiddleware(request, next) {
    const started = nowMilliseconds()
    const requestId =
      normalizedRequestId(request.headers.get('x-request-id')) ??
      `ruvyxa-${(nextRequestId++).toString(16)}`

    let response
    const preflight = corsPreflightResponse(request, cors)
    if (preflight) {
      response = preflight
    } else {
      const limited = rateLimitResponse(request, rate, buckets, trustedProxies, ingressHeaders)
      response = limited ?? (await next())
      response = withCorsHeaders(response, request, cors)
    }

    const headers = new Headers(response.headers)
    for (const [name, value] of customHeaders) headers.set(name, value)
    const elapsed = Math.max(0, nowMilliseconds() - started)
    if (builtin.timing === true) headers.set('x-response-time', `${Math.floor(elapsed)}ms`)
    if (builtin.log === true) headers.set('x-request-id', requestId)

    const result = new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers,
    })
    if (builtin.log === true) {
      logRecord(logAs, 'info', 'request', {
        request_id: requestId,
        method: request.method,
        path: new URL(request.url).pathname,
        status: result.status,
        duration_ms: Math.floor(elapsed),
      })
    }
    return result
  }
}

function validHeaderEntries(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return []
  const entries = []
  for (const [name, headerValue] of Object.entries(value)) {
    try {
      const headers = new Headers([[name, String(headerValue)]])
      entries.push([name, headers.get(name)])
    } catch {
      // Project config validation rejects these. Direct createHandler callers
      // still fail closed instead of crashing every request at runtime.
    }
  }
  return entries
}

function corsPreflightResponse(request, cors) {
  if (!cors || request.method !== 'OPTIONS') return null
  const requestedMethod = request.headers.get('access-control-request-method')
  if (!requestedMethod || !isAllowedCorsOrigin(request.headers.get('origin'), cors)) return null
  return withCorsHeaders(new Response(null, { status: 204 }), request, cors, true)
}

/**
 * Attach the CORS headers this response is entitled to.
 *
 * `Allow-Methods`, `Allow-Headers`, and `Max-Age` answer a preflight question,
 * and the Fetch standard has the browser read them only from a preflight
 * response. Sending them on every actual response is not merely redundant: it
 * advertises the whole method and header allowlist to any origin that gets a
 * response at all, and it invites a proxy to cache a `Max-Age` that was never
 * negotiated. `Allow-Origin`, `Allow-Credentials`, and `Vary` do belong on
 * both, because the browser checks those on the actual response too.
 *
 * Mirrored by `apply_cors_headers` in `crates/ruvyxa_middleware/src/builtin.rs`;
 * `tests/packages/ruvyxa/serverless-handler.test.mjs` holds the split for this
 * host and `builtin.rs`'s own tests hold it for the other.
 */
function withCorsHeaders(response, request, cors, preflight = false) {
  if (!cors) return response
  const headers = new Headers(response.headers)
  const origin = request.headers.get('origin')
  if (isAllowedCorsOrigin(origin, cors)) {
    headers.set('access-control-allow-origin', origin)
    appendVaryOrigin(headers)
    if (cors.credentials === true) headers.set('access-control-allow-credentials', 'true')
    if (preflight) {
      const methods = Array.isArray(cors.methods) ? cors.methods : []
      const allowedHeaders = Array.isArray(cors.headers) ? cors.headers : []
      if (methods.length > 0) headers.set('access-control-allow-methods', methods.join(', '))
      if (allowedHeaders.length > 0) {
        headers.set('access-control-allow-headers', allowedHeaders.join(', '))
      }
      headers.set('access-control-max-age', String(cors.maxAge ?? 86400))
    }
  } else {
    appendVaryOrigin(headers)
  }
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  })
}

function isAllowedCorsOrigin(origin, cors) {
  return (
    typeof origin === 'string' &&
    Array.isArray(cors.origins) &&
    !(cors.credentials === true && cors.origins.includes('*')) &&
    (cors.origins.includes('*') || cors.origins.includes(origin))
  )
}

function appendVaryOrigin(headers) {
  const values = (headers.get('vary') ?? '')
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean)
  if (!values.some((value) => value.toLowerCase() === 'origin')) values.push('Origin')
  headers.set('vary', values.join(', '))
}

function rateLimitResponse(request, rate, buckets, trustedProxies, ingressHeaders) {
  if (!rate) return null
  const max = Number(rate.max)
  const windowSeconds = Number(rate.window)
  if (!Number.isInteger(max) || max < 1 || !Number.isFinite(windowSeconds) || windowSeconds <= 0) {
    return new Response('Rate limit configuration error', { status: 500 })
  }
  return consumeFixedWindow(
    buckets,
    rateLimitKey(request, rate.key, trustedProxies, ingressHeaders),
    max,
    windowSeconds,
  )
}

/**
 * The key of the bucket whose window started earliest, or `undefined` if none.
 *
 * The bucket to give up when the map is full and the sweep freed nothing: the
 * client that has gone quietest. `RateLimitLayer::allow` picks the same one
 * with `min_by_key` over `last_refill`. A tie is broken by whichever the map
 * yields first, which differs between the two hosts and is deliberately not
 * part of the shared contract.
 *
 * @param {Map<string, {remaining: number, startedAt: number}>} buckets
 */
function leastRecentlyStartedKey(buckets) {
  let oldestKey
  let oldestStartedAt = Infinity
  for (const [trackedKey, tracked] of buckets) {
    if (tracked.startedAt < oldestStartedAt) {
      oldestStartedAt = tracked.startedAt
      oldestKey = trackedKey
    }
  }
  return oldestKey
}

/**
 * Consume one unit from a fixed-window bucket, or return the 429 to send.
 *
 * The built-in `rate` middleware and the server-action endpoint both need this,
 * and running two counters with slightly different eviction rules is how a
 * limiter ends up enforcing two different policies depending on which door a
 * request came through.
 *
 * Mirrors `RateLimitLayer::allow` in `crates/ruvyxa_middleware/src/builtin.rs`,
 * and `tests/fixtures/rate-limit-conformance.json` is what holds the two to one
 * answer.
 */
export function consumeFixedWindow(
  buckets,
  key,
  max,
  windowSeconds,
  { message = 'Rate limit exceeded' } = {},
) {
  const now = Date.now()
  const windowMs = windowSeconds * 1000
  let bucket = buckets.get(key)
  if (bucket && now - bucket.startedAt >= windowMs) {
    buckets.delete(key)
    bucket = undefined
  }
  if (!bucket) {
    if (buckets.size >= MAX_TRACKED_RATE_LIMIT_KEYS) {
      for (const [trackedKey, tracked] of buckets) {
        if (now - tracked.startedAt >= windowMs) buckets.delete(trackedKey)
      }
      // The sweep frees only a bucket whose *whole* window has elapsed, so
      // inside one window it can free nothing at all — which is exactly the
      // state one client produces by sending a distinct `X-Api-Key` per
      // request. Refusing here would hand that one client the whole
      // deployment: every visitor the map has not already seen gets a 429
      // until the window rolls.
      //
      // "Fail closed" is the right answer when a limiter cannot answer. This
      // one can: it is not out of answers, it is out of slots, and a slot can
      // be taken back. Evict the least recently started bucket — the client
      // that has gone quietest — and admit the new one. The evicted client is
      // re-admitted with a full allowance the moment it returns, so the cost
      // of the flood falls on the strictness of the limit rather than on
      // availability. Scanning for the oldest rather than trusting the Map's
      // insertion order costs what the native host already pays for the same
      // decision, and does not quietly become wrong the day a bucket is
      // restarted in place.
      while (buckets.size >= MAX_TRACKED_RATE_LIMIT_KEYS) {
        const oldestKey = leastRecentlyStartedKey(buckets)
        if (oldestKey === undefined) break
        buckets.delete(oldestKey)
      }
    }
    bucket = { remaining: max, startedAt: now }
    buckets.set(key, bucket)
  }
  if (bucket.remaining > 0) {
    bucket.remaining -= 1
    return null
  }
  return rateLimited(message, Math.max(1, Math.ceil((windowMs - (now - bucket.startedAt)) / 1000)))
}

function rateLimited(message, retryAfterSeconds) {
  return new Response(message, {
    status: 429,
    headers: {
      'content-type': 'text/plain; charset=utf-8',
      'retry-after': String(retryAfterSeconds),
    },
  })
}

/**
 * Which identity one request is rate-limited under, bounded before it is kept.
 *
 * Mirrors `RateLimitLayer::extract_key` in
 * `crates/ruvyxa_middleware/src/builtin.rs`.
 *
 * A configured header that is absent — or present and empty, which identifies
 * nobody either — falls back to the client address rather than to a shared
 * literal. A request missing the header is not the *same* client as every other
 * request missing it, and bucketing them together turns the limiter into the
 * outage it exists to prevent: one caller that never sends the header drains a
 * counter every other such caller has to share.
 */
export function rateLimitKey(request, configuredKey, trustedProxies, ingressHeaders) {
  if (typeof configuredKey === 'string' && configuredKey.startsWith('header:')) {
    const configured = request.headers.get(configuredKey.slice('header:'.length))
    if (configured) return boundedKey(configured)
  }
  return boundedKey(clientAddress(request.headers, trustedProxies, ingressHeaders))
}

// ─── Client Identity ────────────────────────────────────────────────────────

/**
 * Best available client address for rate limiting.
 *
 * `ingressHeaders` names the headers **this deployment's own ingress writes and
 * overwrites**, and is declared by the adapter that knows which platform the
 * handler was emitted for — never guessed from the request. An edge function
 * exposes no transport peer, so on Cloudflare `CF-Connecting-IP` and on Vercel
 * `X-Vercel-Forwarded-For` are authoritative: the function is reachable only
 * through the ingress that set them, and that ingress replaces whatever the
 * client sent.
 *
 * Reading that list unconditionally is what this used to do, and it was wrong
 * everywhere the premise does not hold. The standalone server the node, bun,
 * deno, aws, railway, and render adapters emit is an ordinary `0.0.0.0` HTTP
 * server — the README says Docker, PM2, systemd, any PaaS — so `CF-Connecting-IP`
 * on it is a header the caller typed. One client rotating a fresh value per
 * request got a fresh bucket per request, which defeats the built-in `rate`
 * middleware, the server-action rate limiter, and the action replay guard's
 * per-client quota at once. `stack.rs` already refuses `rate.key:
 * "header:cf-connecting-ip"` for exactly that reason; the default path must not
 * do quietly what the configured path is rejected for.
 *
 * Failing that — a standalone server behind nginx, Traefik, or a service mesh —
 * `X-Forwarded-For` is scanned from the right, skipping addresses listed in
 * `security.trustedProxyIps`. Each hop appends the peer it actually saw, so
 * rightmost entries are proxy-written while leftmost entries arrive from the
 * client and are forgeable. Taking the leftmost entry would let one client
 * rotate fabricated addresses straight through the limiter, which is the bug
 * `forwarded_client_ip` in the native server exists to avoid.
 */
export function clientAddress(headers, trustedProxies, ingressHeaders = []) {
  for (const name of ingressHeaders) {
    // Parsed rather than trimmed. A value that is not an address identifies
    // nobody, and returning it verbatim is the same unbounded-key rotation the
    // forwarded chain below is careful to avoid — the ingress that writes this
    // header writes an address, so anything else came from somewhere else.
    const address = parseIpAddress(String(headers.get(name) ?? '').trim())
    if (address) return formatAddress(address)
  }

  const forwarded = headers.get('x-forwarded-for') ?? headers.get('x-real-ip')
  if (typeof forwarded !== 'string') return 'unknown'
  // Only hops that parse as an address are considered. The header is
  // client-writable, so returning raw text would let one caller rotate
  // arbitrary junk through the limiter and get a fresh bucket every request —
  // the limiter would count to one, forever.
  const hops = forwarded
    .split(',')
    .map((value) => parseIpAddress(value.trim()))
    .filter(Boolean)
  for (let index = hops.length - 1; index >= 0; index -= 1) {
    if (!isTrustedProxyAddress(hops[index], trustedProxies)) return formatAddress(hops[index])
  }
  // Every hop is a configured proxy, or none parsed. Falling back to one shared
  // bucket limits more aggressively than the traffic warrants, which is the
  // direction a limiter is allowed to be wrong in.
  return 'unknown'
}

/** Stable text form of a parsed address, for use as a bucket key. */
function formatAddress(address) {
  if (address.length === 4) return address.join('.')
  const hextets = []
  for (let index = 0; index < 16; index += 2) {
    hextets.push(((address[index] << 8) | address[index + 1]).toString(16))
  }
  return hextets.join(':')
}

/**
 * Normalize an adapter's `clientIpHeaders` declaration into lookup names.
 *
 * A header name is compared case-insensitively by `Headers`, but the list is
 * lowercased here so a declaration written `CF-Connecting-IP` and one written
 * `cf-connecting-ip` cannot become two different deployments. Anything that is
 * not a non-empty string is dropped rather than throwing: narrowing the list
 * narrows trust, which is the direction this is allowed to be wrong in.
 */
export function parseIngressHeaders(values) {
  if (!Array.isArray(values)) return []
  return values
    .filter((value) => typeof value === 'string' && value.trim() !== '')
    .map((value) => value.trim().toLowerCase())
}

/** Parse `security.trustedProxyIps` into matchable prefixes, skipping bad entries. */
export function parseTrustedProxies(values) {
  if (!Array.isArray(values)) return []
  const prefixes = []
  for (const value of values) {
    if (typeof value !== 'string') continue
    const prefix = parseIpPrefix(value)
    // `ruvyxa build` validates these, so an unparseable entry here means a
    // handler constructed by hand. Skipping it narrows trust rather than
    // widening it, which is the safe direction to fail.
    if (prefix) prefixes.push(prefix)
  }
  return prefixes
}

function isTrustedProxyAddress(address, trustedProxies) {
  if (isLoopbackAddress(address)) return true
  return trustedProxies.some((prefix) => prefixContains(prefix, address))
}

/** Parse `10.0.0.0/8`, `2001:db8::/32`, or a bare address, masking host bits. */
function parseIpPrefix(value) {
  const text = value.trim()
  const slash = text.indexOf('/')
  const addressText = slash < 0 ? text : text.slice(0, slash)
  const address = parseIpAddress(addressText)
  if (!address) return null
  const hostBits = address.length * 8
  let prefixLength = hostBits
  if (slash >= 0) {
    const declared = text.slice(slash + 1).trim()
    if (!/^\d{1,3}$/.test(declared)) return null
    prefixLength = Number(declared)
    if (prefixLength > hostBits) return null
  }
  return { bytes: maskAddress(address, prefixLength), prefixLength }
}

function prefixContains(prefix, address) {
  if (address.length !== prefix.bytes.length) return false
  const masked = maskAddress(address, prefix.prefixLength)
  return masked.every((byte, index) => byte === prefix.bytes[index])
}

function maskAddress(address, prefixLength) {
  const masked = new Uint8Array(address.length)
  const wholeBytes = prefixLength >> 3
  for (let index = 0; index < wholeBytes; index += 1) masked[index] = address[index]
  const remainder = prefixLength & 7
  if (remainder !== 0 && wholeBytes < address.length) {
    masked[wholeBytes] = address[wholeBytes] & ((0xff << (8 - remainder)) & 0xff)
  }
  return masked
}

function isLoopbackAddress(address) {
  if (address.length === 4) return address[0] === 127
  return address.every((byte, index) => (index === 15 ? byte === 1 : byte === 0))
}

/**
 * Parse an address into bytes, collapsing IPv4-mapped IPv6 to its IPv4 form.
 *
 * The collapse matters: a dual-stack listener reports an IPv4 peer as
 * `::ffff:10.0.0.9`, and comparing that byte-wise against an IPv4 prefix would
 * never match — an IPv4 proxy allowlist would silently stop working.
 */
function parseIpAddress(value) {
  const address = value.includes(':') ? parseIpv6(value) : parseIpv4(value)
  if (!address || address.length !== 16) return address
  const mappedPrefix = address.slice(0, 10).every((byte) => byte === 0)
  if (mappedPrefix && address[10] === 0xff && address[11] === 0xff) return address.slice(12)
  return address
}

function parseIpv4(value) {
  const parts = value.split('.')
  if (parts.length !== 4) return null
  const bytes = new Uint8Array(4)
  for (let index = 0; index < 4; index += 1) {
    if (!/^\d{1,3}$/.test(parts[index])) return null
    const octet = Number(parts[index])
    if (octet > 255) return null
    bytes[index] = octet
  }
  return bytes
}

function parseIpv6(value) {
  let text = value.trim()
  if (text.startsWith('[') && text.endsWith(']')) text = text.slice(1, -1)
  // A trailing dotted-quad (`::ffff:1.2.3.4`) is rewritten into two hextets so
  // the group parser below only ever sees hexadecimal.
  const lastColon = text.lastIndexOf(':')
  if (lastColon >= 0 && text.slice(lastColon + 1).includes('.')) {
    const embedded = parseIpv4(text.slice(lastColon + 1))
    if (!embedded) return null
    const high = ((embedded[0] << 8) | embedded[1]).toString(16)
    const low = ((embedded[2] << 8) | embedded[3]).toString(16)
    text = `${text.slice(0, lastColon + 1)}${high}:${low}`
  }

  const halves = text.split('::')
  if (halves.length > 2) return null
  const head = halves[0] === '' ? [] : halves[0].split(':')
  let groups
  if (halves.length === 1) {
    groups = head
  } else {
    const tail = halves[1] === '' ? [] : halves[1].split(':')
    const zeros = 8 - head.length - tail.length
    if (zeros < 1) return null
    groups = [...head, ...Array.from({ length: zeros }, () => '0'), ...tail]
  }
  if (groups.length !== 8) return null

  const bytes = new Uint8Array(16)
  for (let index = 0; index < 8; index += 1) {
    if (!/^[0-9a-fA-F]{1,4}$/.test(groups[index])) return null
    const hextet = Number.parseInt(groups[index], 16)
    bytes[index * 2] = hextet >> 8
    bytes[index * 2 + 1] = hextet & 0xff
  }
  return bytes
}

// ─── Request Limits ─────────────────────────────────────────────────────────

function positiveInteger(value) {
  return Number.isInteger(value) && value > 0 ? value : undefined
}

/**
 * Reject a request whose declared body length exceeds `limit`.
 *
 * `GET` and `HEAD` are exempt because they carry no body. This is the cheap
 * half of the cap: it rejects before a byte is read, but only when the client
 * declared a length. `limitBodyStream` covers the rest.
 */
function declaredBodyTooLarge(request, limit, message = 'Request body is too large') {
  if (request.method === 'GET' || request.method === 'HEAD') return null
  const declared = request.headers.get('content-length')
  if (declared === null) return null
  const length = Number(declared)
  if (!Number.isFinite(length) || length < 0) {
    return textResponse(400, 'Invalid Content-Length')
  }
  return length > limit ? textResponse(413, message) : null
}

/**
 * Return an equivalent request whose body cannot yield more than `limit` bytes.
 *
 * `Content-Length` alone is not a cap. A chunked upload declares no length, and
 * a `Request` a platform hands us is not required to carry the header at all —
 * a constructed `Request` in undici does not. Enforcing on the header only
 * would have left the deployed runtimes with no effective body limit, which is
 * the gap this whole change exists to close, so the bytes themselves are
 * counted as the route handler reads them.
 *
 * Reconstruction is attempted rather than assumed: a runtime that refuses a
 * stream body (or `duplex`) keeps the original request and the declared-length
 * check remains its bound. Failing to wrap must never fail the request.
 */
function limitBodyStream(request, limit) {
  if (!request.body || typeof TransformStream === 'undefined') return request
  let seen = 0
  const limiter = new TransformStream({
    transform(chunk, controller) {
      seen += chunk?.byteLength ?? 0
      if (seen > limit) {
        controller.error(new Error(`RUV2212 Request body exceeded the ${limit} byte limit`))
        return
      }
      controller.enqueue(chunk)
    },
  })
  try {
    // The method comes from `request`, and the guard at the top already returned
    // for a request with no body, so a GET can never reach here carrying one.
    // oxlint-disable-next-line unicorn/no-invalid-fetch-options
    return new Request(request, { body: request.body.pipeThrough(limiter), duplex: 'half' })
  } catch {
    return request
  }
}

/** True when an error came from `limitBodyStream` rather than from user code. */
function isBodyLimitError(error) {
  for (let current = error, depth = 0; current && depth < 4; current = current.cause, depth += 1) {
    if (typeof current.message === 'string' && current.message.includes('RUV2212')) return true
  }
  return false
}

function textResponse(status, message) {
  return new Response(message, {
    status,
    headers: { 'content-type': 'text/plain; charset=utf-8' },
  })
}

function normalizedRequestId(value) {
  return typeof value === 'string' && value.length > 0 && value.length <= 128 ? value : null
}

function nowMilliseconds() {
  return globalThis.performance?.now?.() ?? Date.now()
}

/**
 * Weak comparison of `if-none-match` against a validator.
 *
 * Weak is what a revalidation asks for: `*` matches any stored representation,
 * and a `W/` prefix on either side is ignored rather than making the comparison
 * fail. Mirrors `etag_matches` on the native host.
 */
function etagMatches(header, etag) {
  const value = String(header ?? '').trim()
  if (value === '') return false
  if (value === '*') return true
  const bare = (candidate) => candidate.trim().replace(/^W\//, '')
  return value.split(',').some((candidate) => bare(candidate) === bare(etag))
}

/**
 * The weak validator for a document's bytes.
 *
 * `crypto.subtle` rather than `node:crypto`: this module is copied verbatim into
 * a Cloudflare Worker and a Vercel Edge Function, where the Node built-in is not
 * there to import. Weak, because the same document leaves here identity-encoded
 * or gzipped depending on what the client accepts, and those are equivalent
 * representations rather than byte-identical ones — which is exactly what a weak
 * validator states.
 *
 * Truncated to sixteen hex characters, the width `compute_etag` on the native
 * host uses. The two hosts deliberately do not produce the *same* value — one
 * hashes with blake3 and one with SHA-256 — because a validator is opaque and
 * scoped to the origin that issued it, and no client ever holds one from both.
 * What has to agree is which documents get one at all, which is
 * `DOCUMENT_VALIDATOR_STRATEGIES`.
 */
async function documentValidator(body) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(body))
  const hex = Array.from(new Uint8Array(digest, 0, 8), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')
  return `W/"${hex}"`
}

/**
 * Attach a validator to a stored document, and answer a revalidation of one.
 *
 * Only responses the strategy layer marked, so nothing else pays for this: an
 * unmarked response is returned untouched, with its body never read. A marked
 * one has its body consumed here, which is free — it was a string.
 */
async function withDocumentValidator(request, response) {
  if (!response.headers.has(DOCUMENT_VALIDATOR_HEADER)) return response

  const body = await response.text()
  const etag = await documentValidator(body)
  const headers = new Headers(response.headers)
  headers.delete(DOCUMENT_VALIDATOR_HEADER)
  headers.set('etag', etag)

  if (!etagMatches(request.headers.get('if-none-match'), etag)) {
    return new Response(body, {
      status: response.status,
      statusText: response.statusText,
      headers,
    })
  }

  // A 304 keeps what guides the cache — `cache-control`, `vary`, the ISR status
  // — and drops what describes a body it is not sending. `content-length` beside
  // an empty body is a framing error the client reads as a truncated response.
  headers.delete('content-length')
  headers.delete('content-type')
  return new Response(null, { status: 304, headers })
}

function withDefaultSecurityHeaders(response) {
  const headers = new Headers(response.headers)
  for (const [name, value] of Object.entries(DEFAULT_SECURITY_HEADERS)) {
    if (!headers.has(name)) headers.set(name, value)
  }
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  })
}

function normalizeCacheEntry(value) {
  if (typeof value === 'string') return { html: value, stale: true }
  if (!value || typeof value !== 'object' || typeof value.html !== 'string') return null
  return { html: value.html, stale: value.stale === true }
}

// ─── Prerender Cache Paths ──────────────────────────────────────────────────

/**
 * Reject a path segment that could escape, or misname, the cache directory.
 *
 * Written as explicit character tests rather than a regular expression: this
 * guard decides what reaches the file system, and it must stay obvious that
 * separators, control characters, and Windows stream/drive separators are all
 * covered.
 */
function isUnsafeSegment(segment) {
  if (segment === '.' || segment === '..') return true
  for (const char of segment) {
    if (char === '/' || char === '\\' || char === ':') return true
    const code = char.codePointAt(0)
    // C0 *and* C1. Rust's `char::is_control()` covers U+0080–U+009F and this
    // copy stopped at U+007F, so the writer refused a segment the readers would
    // have accepted — a divergence the shared table had no case for until now.
    // `line 346` above already spells the full range; these two agree again.
    if (code < 0x20 || (code >= 0x7f && code <= 0x9f)) return true
  }
  return false
}

/**
 * Map a request path to the relative location of its pre-rendered HTML.
 *
 * Mirrors the build writer, which stores `<prerenderDir>/<path>/index.html`
 * from its canonical route path. Request handlers canonicalize before calling
 * this mapper; direct callers must provide the path representation they store.
 *
 * Returns `null` when the path cannot be mapped to a contained location.
 * Adapters join the result onto their cache directory and touch the file
 * system, so this is the single place that decides what is in bounds — the
 * platform URL parser is not a substitute, because adapters may be handed a
 * path from a source that never went through it.
 *
 * @param {string} pathname Request path, beginning with `/`.
 * @returns {string|null} A `.../index.html` relative path, or null if unsafe.
 */
export function prerenderRelativePath(pathname) {
  if (typeof pathname !== 'string' || !pathname.startsWith('/')) return null

  const segments = []
  for (const segment of pathname.split('/')) {
    if (segment === '') continue
    if (isUnsafeSegment(segment)) return null
    segments.push(segment)
  }

  return segments.length === 0 ? 'index.html' : `${segments.join('/')}/index.html`
}

// ─── Static Asset Paths ─────────────────────────────────────────────────────

/**
 * Parse a single-range `bytes=` specifier against a known length.
 *
 * A media element does not download a file and play it; it asks for the bytes
 * it needs as it needs them. A server that ignores `Range` restarts the
 * download from zero on every seek, and a strict player refuses a resource
 * whose server will not answer its opening `Range: bytes=0-1` with a 206 at
 * all.
 *
 * Multi-range requests are answered whole: a `multipart/byteranges` body is
 * more machinery than any client of this server needs, and RFC 9110 lets a
 * server ignore a `Range` it does not wish to honour — which is why an
 * unparsable specifier also falls back to the whole file rather than to 416.
 * Only a syntactically valid range this file cannot satisfy is a 416.
 *
 * Mirrors `parse_single_byte_range` in
 * `crates/ruvyxa_dev_server/src/static_assets.rs`; both answer
 * `tests/fixtures/byte-range-conformance.json`.
 *
 * @param {string} value raw `Range` header value
 * @param {number} length size of the file being served
 * @returns {{kind: 'whole'} | {kind: 'unsatisfiable'} | {kind: 'partial', start: number, end: number}}
 */
export function parseByteRange(value, length) {
  const whole = { kind: 'whole' }
  const unsatisfiable = { kind: 'unsatisfiable' }
  if (typeof value !== 'string') return whole
  const trimmed = value.trim()
  if (!trimmed.startsWith('bytes=')) return whole
  const spec = trimmed.slice('bytes='.length).trim()
  if (spec.includes(',')) return whole
  const dash = spec.indexOf('-')
  if (dash === -1) return whole
  const first = spec.slice(0, dash).trim()
  const last = spec.slice(dash + 1).trim()

  // Integers only: Number() would accept '1e3', '0x10', and ' ' as positions
  // the Rust side rejects.
  const position = (text) => (/^\d+$/.test(text) ? Number(text) : null)

  if (first === '') {
    // `bytes=-N`: the final N bytes, clamped to the file.
    const suffix = position(last)
    if (suffix === null) return whole
    // An empty file has no byte to name, and a zero-length suffix names none.
    if (suffix === 0 || length === 0) return unsatisfiable
    return { kind: 'partial', start: Math.max(0, length - suffix), end: length - 1 }
  }

  const start = position(first)
  if (start === null) return whole
  if (start >= length) return unsatisfiable
  let end
  if (last === '') {
    end = length - 1
  } else {
    const parsed = position(last)
    if (parsed === null) return whole
    // A last-byte position past the end is clamped: a client asking for one
    // megabyte from here should get whatever of it exists.
    end = Math.min(parsed, length - 1)
  }
  if (end < start) return unsatisfiable
  return { kind: 'partial', start, end }
}

/**
 * Extensions that only ever name a build or public asset. Kept to images,
 * fonts, media, and emitted web assets: these are never a plausible value for
 * a dynamic route parameter, so refusing them cannot swallow a real page.
 * Mirrors `is_static_asset_request` in `crates/ruvyxa_dev_server/src/static_assets.rs`.
 */
const STATIC_ASSET_EXTENSIONS = new Set([
  'apng',
  'avif',
  'bmp',
  'css',
  'eot',
  'gif',
  'ico',
  'jpeg',
  'jpg',
  'js',
  'map',
  'mjs',
  'mov',
  'mp3',
  'mp4',
  'ogg',
  'otf',
  'png',
  'svg',
  'ttf',
  'wav',
  'webm',
  'webp',
  'woff',
  'woff2',
])

/**
 * Well-known crawler files that are never a page.
 *
 * `.txt` and `.xml` are deliberately absent from `STATIC_ASSET_EXTENSIONS` — a
 * route may legitimately end in either — but these exact paths are fixed by
 * convention. Letting `/[lang]` answer `/robots.txt` returns 200 with an HTML
 * body, which is what Lighthouse's `robots-txt` audit fails on. Mirrors
 * `is_crawler_discovery_path()` in
 * `crates/ruvyxa_dev_server/src/static_assets.rs`.
 */
const CRAWLER_DISCOVERY_PATHS = new Set(['/robots.txt', '/sitemap.xml', '/sitemap_index.xml'])

/** True when the last path segment names a static asset file. */
export function isStaticAssetPath(pathname) {
  if (typeof pathname !== 'string') return false
  if (CRAWLER_DISCOVERY_PATHS.has(pathname.replace(/\/+$/, ''))) return true
  const lastSlash = pathname.lastIndexOf('/')
  const segment = lastSlash === -1 ? pathname : pathname.slice(lastSlash + 1)
  const dot = segment.lastIndexOf('.')
  if (dot <= 0 || dot === segment.length - 1) return false
  return STATIC_ASSET_EXTENSIONS.has(segment.slice(dot + 1).toLowerCase())
}

/** True when the route pattern contains a dynamic, catch-all, or optional segment. */
function hasDynamicSegment(routePath) {
  return typeof routePath === 'string' && routePath.includes('[')
}

// ─── Route Matching ─────────────────────────────────────────────────────────

/**
 * Remove `basePath` from a request path.
 *
 * Returns the remaining path, or `null` when the request falls outside the
 * base path and must not be served by this handler.
 */
function stripBasePath(pathname, basePath) {
  if (!basePath) return pathname

  const prefix = basePath.endsWith('/') ? basePath.slice(0, -1) : basePath
  if (!prefix) return pathname
  if (pathname === prefix) return '/'
  // Require a segment boundary so `/appointments` is not treated as `/app`
  // plus `ointments`.
  if (!pathname.startsWith(`${prefix}/`)) return null
  return pathname.slice(prefix.length) || '/'
}

/**
 * Decode a request path exactly once while preserving segment boundaries.
 *
 * Thin wrapper over the shared `canonicalRoutePath`, which answers "is this
 * path acceptable, and what are its segments?" for every JavaScript host. The
 * handler needs the rejection as an exception so `dispatch` can turn it into a
 * 400 with a message, while the client matcher wants a null — the decision
 * itself must stay in one place, so only the reporting differs here.
 */
function canonicalRequestPath(rawPathname) {
  if (typeof rawPathname !== 'string' || !rawPathname.startsWith('/')) {
    throw new URIError('Request path must start with "/"')
  }
  const canonical = canonicalRoutePath(rawPathname)
  if (canonical === null) {
    throw new URIError('Request path contains an unsafe encoded segment')
  }
  return canonical
}

/**
 * Match a request path against a route table, exposed for cross-implementation
 * testing.
 *
 * The handler, `@ruvyxa/react`'s router, and the standalone server all use the
 * shared route matcher, so a link click and a page reload resolve the same URL
 * to the same route and params by construction rather than by review. This
 * entry point exists so the conformance suite can drive the handler's own
 * dispatch path — including its base-path and error reporting behaviour —
 * against the shared case table in
 * `tests/fixtures/route-match-conformance.json`, alongside the Rust router.
 * It is not part of the handler's runtime path.
 */
export function resolveRouteForTesting(routes, pathname) {
  const matchRoute = createCanonicalRouteMatcher(routes)
  try {
    const canonicalPathname = canonicalRequestPath(pathname)
    const matched = matchRoute(canonicalPathname)
    return matched
      ? { path: matched.route.path, params: matched.params, pathname: canonicalPathname }
      : null
  } catch {
    return null
  }
}
