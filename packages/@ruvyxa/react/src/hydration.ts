/**
 * Hydration and runtime error reporting.
 *
 * The generated client entry passes React's `onRecoverableError`,
 * `onCaughtError`, and `onUncaughtError` a function that hands every report to
 * `globalThis.__RUVYXA_HYDRATION_REPORTER__` — or, until {@link hydrate}
 * installs one, queues it on `globalThis.__RUVYXA_HYDRATION_ERRORS__`. The
 * queue is what makes the API mean anything: hydration runs before any
 * application code can install a handler, so a mismatch reported straight to a
 * handler that did not exist yet was a mismatch nobody heard about, which is
 * what this used to be.
 *
 * The entry cannot import this package — an app may render plain React pages
 * and never install it — so the two halves meet on those two globals. The
 * writing side is `rootOptionsPrelude()` in
 * `packages/ruvyxa/runtime/entry-templates.mjs` and `ROOT_OPTIONS_PRELUDE` in
 * `crates/ruvyxa_bundler/src/output.rs`, held to one text by
 * `tests/packages/ruvyxa/entry-prelude-parity.test.mjs`; this module is the
 * only reader.
 */

/** Which React callback, or which caller, produced a report. */
export type HydrationErrorKind = 'recoverable' | 'caught' | 'uncaught' | 'manual'

/** What accompanies a reported error. */
export interface HydrationErrorContext {
  componentStack?: string
  digest?: string
  /**
   * `recoverable` is what a hydration mismatch is: React discarded the server
   * markup and rendered on the client. `caught` reached an error boundary;
   * `uncaught` reached none and unmounted the root. `manual` came from
   * {@link reportHydrationError}.
   */
  kind?: HydrationErrorKind
}

/**
 * Handler function for hydration errors.
 */
export type HydrationErrorHandler = (error: unknown, context: HydrationErrorContext) => void

/**
 * Options for the hydrate() helper.
 */
export interface HydrationOptions {
  /** The root element or document to hydrate into. */
  root?: Element | Document
  /** Custom error handler for hydration mismatches. */
  onError?: HydrationErrorHandler
}

/**
 * Same shim `link.tsx` declares: both linkers open every module factory with a
 * `process` fallback, so this reads correctly in a browser and lets the
 * production fold delete the development-only branch below.
 */
declare const process: { env: { NODE_ENV?: string } }

const REPORTER_GLOBAL = '__RUVYXA_HYDRATION_REPORTER__'
const QUEUE_GLOBAL = '__RUVYXA_HYDRATION_ERRORS__'
/** Mirrors the bound in the prelude: a render loop must not grow the queue forever. */
const QUEUE_LIMIT = 100

interface QueuedReport {
  error: unknown
  context: HydrationErrorContext
}

const store = globalThis as Record<string, unknown>

function installedReporter(): HydrationErrorHandler | undefined {
  const value = store[REPORTER_GLOBAL]
  return typeof value === 'function' ? (value as HydrationErrorHandler) : undefined
}

function queue(): QueuedReport[] {
  const existing = store[QUEUE_GLOBAL]
  if (Array.isArray(existing)) return existing as QueuedReport[]
  const created: QueuedReport[] = []
  store[QUEUE_GLOBAL] = created
  return created
}

/**
 * Signal that hydration is complete and register optional error handlers.
 *
 * Installing a handler also delivers every error React reported before this
 * call — the hydration mismatches themselves, which happen before any
 * application code runs. The handler is wrapped so that a failure inside it
 * never reaches React or the entry.
 *
 * Usage:
 * ```ts
 * import { hydrate } from "@ruvyxa/react"
 *
 * hydrate({
 *   onError: (error, { componentStack, kind }) => {
 *     // Report to your error tracking service
 *     myErrorService.captureException(error, { componentStack, kind })
 *   }
 * })
 * ```
 */
export function hydrate(options: HydrationOptions = {}): void {
  if (options.onError) {
    const handler = options.onError
    const reporter: HydrationErrorHandler = (error, context) => {
      try {
        handler(error, context)
      } catch {
        // Never let error reporting crash the app
      }
    }
    store[REPORTER_GLOBAL] = reporter
    const pending = queue().splice(0)
    for (const report of pending) reporter(report.error, report.context)
  }

  if (typeof window !== 'undefined') {
    const target = options.root ?? window
    target.dispatchEvent(new CustomEvent('ruvyxa:hydrate'))
  }
}

/**
 * Report an error through the registered handler.
 *
 * The generated entry reports React's own errors without going through here;
 * this is for application code that wants a failure of its own in the same
 * stream. With no handler installed yet the report is queued for the next
 * `hydrate({ onError })`, and in development it is also logged.
 */
export function reportHydrationError(
  error: unknown,
  context: Omit<HydrationErrorContext, 'kind'> = {},
): void {
  const enriched: HydrationErrorContext = { kind: 'manual', ...context }
  const reporter = installedReporter()
  if (reporter) {
    try {
      reporter(error, enriched)
    } catch {
      // A reporter installed by something other than hydrate() is not wrapped.
    }
  } else {
    const pending = queue()
    if (pending.length < QUEUE_LIMIT) pending.push({ error, context: enriched })
  }

  if (process.env.NODE_ENV !== 'production') {
    console.error('[ruvyxa] Hydration error:', error, enriched)
  }
}
