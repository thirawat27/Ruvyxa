import { definePlugin } from '@ruvyxa/core/plugin'
import type { RuvyxaPlugin } from '@ruvyxa/core/plugin'

import { normalizePublicFilePath, normalizePublicPath, writePublicAsset } from './shared.js'

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
}

const METRIC_NAMES = new Set(['LCP', 'CLS', 'INP', 'FCP', 'TTFB'])

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
  const script = createWebVitalsScript(endpoint, sampleRate)

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
