import { definePlugin } from '@ruvyxa/core/plugin'
import type { RuvyxaPlugin } from '@ruvyxa/core/plugin'

import {
  boundedRateLimitKey,
  consumeFixedWindow,
  normalizePublicFilePath,
  normalizePublicPath,
  writePublicAsset,
} from './shared.js'
import type { FixedWindowBucket } from './shared.js'

// ─── webVitals ───────────────────────────────────────────────────────────────

export interface WebVitalsEntry {
  /** Metric name: `LCP`, `CLS`, `INP`, `FCP`, or `TTFB`. */
  name: string
  value: number
  /** Pathname the metric was measured on. */
  pathname: string
  /** Present when the browser supplied one. */
  rating?: string
}

export interface WebVitalsOptions {
  /**
   * Path the browser reports to. Also the route this plugin serves.
   * @default "/__metrics/web-vitals"
   */
  endpoint?: string
  /** Path the client script is published at. @default "/web-vitals.js" */
  scriptPath?: string
  /** Fraction of page loads that report, from 0 to 1. @default 1 */
  sampleRate?: number
  /** Receives each reported metric. Defaults to a single-line JSON log. */
  logger?: (entry: WebVitalsEntry) => void
  /**
   * The ceiling on accepted records, or `false` to accept every beacon.
   *
   * `max` records per `windowSeconds` from one client, and — always, because a
   * client cannot be identified without `clientIp` — fifty times that from
   * every client combined. The wider ceiling is derived rather than separately
   * configurable, the same way `@ruvyxa/auth` derives its account-wide ones
   * from `rateLimit.max`.
   *
   * The default is deliberately loose. This endpoint exists to measure what
   * real visitors experienced, and a limit tight enough to catch a flood is
   * also tight enough to drop the beacons of a large shared egress — an office,
   * a mobile carrier — which does not merely lose data, it biases the metric
   * towards whoever was not behind a NAT. Raise it before narrowing it.
   *
   * Set `false` only where something else already bounds the endpoint, which on
   * a platform with a WAF in front of the origin is the better place for it.
   *
   * @default { max: 120, windowSeconds: 60 }
   */
  rateLimit?: { max?: number; windowSeconds?: number } | false
  /**
   * Resolve the client the per-client budget is counted against.
   *
   * Off by default and shaped exactly like `@ruvyxa/auth`'s, because forwarded
   * headers are written by whoever spoke last: only the deployment knows
   * whether a trusted proxy or platform edge overwrites them. Read the header
   * the platform guarantees — `(request) => request.headers.get('cf-connecting-ip')`
   * — or the rightmost `x-forwarded-for` hop behind a proxy you control.
   *
   * Without it there is no per-client bucket at all, and the endpoint ceiling
   * is the only thing left. A user-agent fallback is deliberately not offered:
   * for a login endpoint it separates a few callers, and for a metrics endpoint
   * it puts every visitor on one browser into one bucket, which is the outage
   * a limiter exists to prevent.
   */
  clientIp?(request: Request): string | null | undefined
}

const METRIC_NAMES = new Set(['LCP', 'CLS', 'INP', 'FCP', 'TTFB'])

/**
 * How much wider the endpoint's own ceiling is than one client's.
 *
 * Derived, not configurable: `@ruvyxa/auth` settled the same question the same
 * way, and a second number to tune is a second number to get wrong in the
 * direction that silently discards real measurements.
 */
const ENDPOINT_BUDGET_MULTIPLE = 50

/**
 * The endpoint-wide ceiling is counted in a map of its own rather than under
 * a reserved key beside the clients: a resolver may return any string, and a
 * shared namespace is a collision waiting for the one caller that guesses the
 * reserved name and drains everybody else's allowance.
 */
const ENDPOINT_BUCKET = 'endpoint'

/**
 * Collects Core Web Vitals from the browser and reports them server-side.
 *
 * `observability` measures what the server did; this measures what the visitor
 * experienced, which server timing cannot see. The two are complementary and
 * deliberately separate plugins.
 *
 * The client script is published as a build asset and loaded with `src` rather
 * than inlined into `<head>`. An inline snippet would force `'unsafe-inline'`
 * into every `script-src` policy that wanted this plugin, so the plugin that
 * measures performance would have quietly cost the application its CSP.
 */
export function webVitals(options: WebVitalsOptions = {}): RuvyxaPlugin {
  const endpoint = normalizePublicPath(
    options.endpoint ?? '/__metrics/web-vitals',
    'webVitals.endpoint',
  )
  const scriptPath = normalizePublicFilePath(
    options.scriptPath ?? '/web-vitals.js',
    'webVitals.scriptPath',
  )
  if (endpoint === scriptPath) {
    throw new TypeError('webVitals: endpoint and scriptPath must differ')
  }
  const sampleRate = options.sampleRate ?? 1
  if (typeof sampleRate !== 'number' || !(sampleRate >= 0) || !(sampleRate <= 1)) {
    throw new TypeError('webVitals: sampleRate must be a number from 0 to 1')
  }
  if (options.logger !== undefined && typeof options.logger !== 'function') {
    throw new TypeError('webVitals: logger must be a function')
  }
  if (options.clientIp !== undefined && typeof options.clientIp !== 'function') {
    throw new TypeError('webVitals: clientIp must be a function')
  }
  const budget = normalizeWebVitalsRateLimit(options.rateLimit)
  const script = createWebVitalsScript(endpoint, sampleRate)
  // One map per ceiling. Both are per process: a deployed build runs as many
  // instances as the platform started, so the effective ceiling scales with
  // them, which is the honest trade for a limiter that needs no shared store.
  // `@ruvyxa/auth` takes a store instead, because a login attempt has to be
  // counted across every process and a dropped beacon does not.
  const clientBuckets = new Map<string, FixedWindowBucket>()
  const endpointBuckets = new Map<string, FixedWindowBucket>()

  return definePlugin({
    name: 'ruvyxa:web-vitals',
    head: { tag: 'script', attrs: { src: scriptPath, defer: true } },
    register({ http, build }) {
      http.onRequest({
        match: [scriptPath],
        handler({ request }) {
          if (new URL(request.url).pathname !== scriptPath) return undefined
          return new Response(script, {
            headers: { 'content-type': 'text/javascript; charset=utf-8' },
          })
        },
      })
      http.route({
        path: endpoint,
        method: 'POST',
        async handler({ request }) {
          // Refuse before the body is read: a beacon past the budget must cost
          // the parse it was going to cost, not more.
          const retryAfter = budget
            ? refusedBeacon(request, budget, options.clientIp, clientBuckets, endpointBuckets)
            : null
          if (retryAfter !== null) {
            // 429 rather than a silent 204. `sendBeacon` reads nothing back
            // either way, so the only reader is the operator — and a collector
            // that quietly discards measurements biases the numbers it exists
            // to report, which is worse than reporting fewer of them.
            return new Response(null, {
              status: 429,
              headers: { 'retry-after': String(retryAfter) },
            })
          }
          await collectWebVitals(request, options.logger)
          // The browser sends this with `sendBeacon` during page teardown and
          // reads nothing back; a body here would only be discarded.
          return new Response(null, { status: 204 })
        },
      })
      build.onComplete((context) => writePublicAsset(context, scriptPath, script))
    },
  })
}

interface WebVitalsBudget {
  max: number
  windowSeconds: number
}

/**
 * The ceilings this collector enforces, or `null` when it enforces none.
 *
 * A zero is refused rather than read as "off": `rateLimit: { max: 0 }` reads
 * like a very strict limit and would be an endpoint that accepts nothing, and
 * a plugin whose configuration can switch it off by looking strict is a
 * configuration nobody can review. `false` is the way to turn it off, and it
 * says so.
 */
function normalizeWebVitalsRateLimit(value: WebVitalsOptions['rateLimit']): WebVitalsBudget | null {
  if (value === false) return null
  if (value === undefined) return { max: 120, windowSeconds: 60 }
  if (typeof value !== 'object' || value === null) {
    throw new TypeError('webVitals: rateLimit must be an object or false')
  }
  const max = value.max ?? 120
  const windowSeconds = value.windowSeconds ?? 60
  if (!Number.isInteger(max) || max < 1) {
    throw new TypeError('webVitals: rateLimit.max must be a positive integer')
  }
  if (!Number.isInteger(windowSeconds) || windowSeconds < 1) {
    throw new TypeError('webVitals: rateLimit.windowSeconds must be a positive integer')
  }
  return { max, windowSeconds }
}

/**
 * The seconds to wait, or `null` when this beacon may be accepted.
 *
 * The endpoint ceiling is spent first and unconditionally, because it is the
 * one that bounds what the log sink is billed for; the per-client ceiling
 * exists only where the deployment told us how to recognise a client.
 */
function refusedBeacon(
  request: Request,
  budget: WebVitalsBudget,
  clientIp: WebVitalsOptions['clientIp'],
  clientBuckets: Map<string, FixedWindowBucket>,
  endpointBuckets: Map<string, FixedWindowBucket>,
): number | null {
  const endpoint = consumeFixedWindow(
    endpointBuckets,
    ENDPOINT_BUCKET,
    budget.max * ENDPOINT_BUDGET_MULTIPLE,
    budget.windowSeconds,
  )
  if (endpoint !== null) return endpoint
  const identity = resolveWebVitalsClient(request, clientIp)
  if (identity === null) return null
  return consumeFixedWindow(clientBuckets, identity, budget.max, budget.windowSeconds)
}

/**
 * Who this beacon is counted against, or `null` when nothing identified it.
 *
 * A resolver that throws is not allowed to take the endpoint down with it, the
 * same call `@ruvyxa/auth` makes: an unattributed beacon still meets the
 * endpoint ceiling, so the failure costs precision rather than protection.
 */
function resolveWebVitalsClient(
  request: Request,
  clientIp: WebVitalsOptions['clientIp'],
): string | null {
  if (!clientIp) return null
  let resolved: unknown
  try {
    resolved = clientIp(request)
  } catch {
    return null
  }
  if (typeof resolved !== 'string') return null
  const trimmed = resolved.trim()
  return trimmed === '' ? null : boundedRateLimitKey(trimmed)
}

async function collectWebVitals(
  request: Request,
  logger: WebVitalsOptions['logger'],
): Promise<void> {
  let payload: unknown
  try {
    payload = await request.json()
  } catch {
    return
  }
  const entry = normalizeWebVitalsEntry(payload)
  if (!entry) return
  try {
    if (logger) logger(entry)
    else console.info(JSON.stringify({ metric: 'web-vitals', ...entry }))
  } catch {
    // A telemetry sink must never turn a beacon into an error the browser
    // retries. `observability` fails the same way for the same reason.
  }
}

/**
 * Accept only the shape this plugin's own script sends.
 *
 * The endpoint is reachable by anyone, so an unvalidated payload would let a
 * third party write arbitrary strings and numbers into the application's logs.
 */
function normalizeWebVitalsEntry(payload: unknown): WebVitalsEntry | null {
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) return null
  const { name, value, pathname, rating } = payload as Record<string, unknown>
  if (typeof name !== 'string' || !METRIC_NAMES.has(name)) return null
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return null
  if (typeof pathname !== 'string' || !pathname.startsWith('/') || pathname.length > 2048) {
    return null
  }
  return {
    name,
    value,
    pathname,
    ...(rating === 'good' || rating === 'needs-improvement' || rating === 'poor' ? { rating } : {}),
  }
}

/**
 * The published client script.
 *
 * Built on `PerformanceObserver` directly rather than by depending on the
 * `web-vitals` package: this plugin ships inside the framework, and a runtime
 * dependency here would land in every application that enables it.
 */
function createWebVitalsScript(endpoint: string, sampleRate: number): string {
  return `// Generated by ruvyxa/plugins webVitals. Do not edit.
(function () {
  if (Math.random() >= ${JSON.stringify(sampleRate)}) return
  var endpoint = ${JSON.stringify(endpoint)}
  var sent = Object.create(null)

  function report(name, value) {
    if (sent[name] || typeof value !== 'number' || !isFinite(value) || value < 0) return
    sent[name] = true
    var body = JSON.stringify({ name: name, value: value, pathname: location.pathname })
    if (navigator.sendBeacon) {
      navigator.sendBeacon(endpoint, new Blob([body], { type: 'application/json' }))
    } else {
      fetch(endpoint, {
        method: 'POST',
        body: body,
        keepalive: true,
        headers: { 'content-type': 'application/json' },
      })
    }
  }

  function observe(type, handler, options) {
    try {
      var observer = new PerformanceObserver(function (list) { handler(list.getEntries()) })
      observer.observe(Object.assign({ type: type, buffered: true }, options || {}))
    } catch (error) {
      // A browser without this entry type reports the metrics it does support.
    }
  }

  var navigation = performance.getEntriesByType('navigation')[0]
  if (navigation) report('TTFB', navigation.responseStart)

  observe('paint', function (entries) {
    for (var i = 0; i < entries.length; i++) {
      if (entries[i].name === 'first-contentful-paint') report('FCP', entries[i].startTime)
    }
  })

  var lcp = 0
  observe('largest-contentful-paint', function (entries) {
    var last = entries[entries.length - 1]
    if (last) lcp = last.startTime
  })

  var cls = 0
  observe('layout-shift', function (entries) {
    for (var i = 0; i < entries.length; i++) {
      if (!entries[i].hadRecentInput) cls += entries[i].value
    }
  })

  var inp = 0
  observe('event', function (entries) {
    for (var i = 0; i < entries.length; i++) {
      if (entries[i].duration > inp) inp = entries[i].duration
    }
  }, { durationThreshold: 40 })

  // Flush when the page is hidden rather than on 'unload': a page restored
  // from the back/forward cache never unloads, and its metrics would be lost.
  // 'report' is idempotent per metric, so both listeners firing is harmless.
  function flush() {
    report('LCP', lcp)
    report('CLS', cls)
    report('INP', inp)
  }
  addEventListener('visibilitychange', function () {
    if (document.visibilityState === 'hidden') flush()
  })
  addEventListener('pagehide', flush)
})()
`
}
