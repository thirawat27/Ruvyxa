/**
 * Handler function for hydration errors.
 */
export type HydrationErrorHandler = (error: unknown, context: {
  componentStack?: string
  digest?: string
}) => void

/**
 * Options for the hydrate() helper.
 */
export interface HydrationOptions {
  /** The root element or document to hydrate into. */
  root?: Element | Document
  /** Custom error handler for hydration mismatches. */
  onError?: HydrationErrorHandler
}

// Internal registry for error handlers
let globalErrorHandler: HydrationErrorHandler | undefined

/**
 * Signal that hydration is complete and register optional error handlers.
 *
 * Call this after your app has hydrated to enable Ruvyxa's client-side
 * error reporting for hydration mismatches and runtime errors.
 *
 * Usage:
 * ```ts
 * import { hydrate } from "@ruvyxa/react"
 *
 * hydrate({
 *   onError: (error, { componentStack }) => {
 *     // Report to your error tracking service
 *     myErrorService.captureException(error, { componentStack })
 *   }
 * })
 * ```
 */
export function hydrate(options: HydrationOptions = {}): void {
  if (options.onError) {
    globalErrorHandler = options.onError
  }

  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent("ruvyxa:hydrate"))
  }
}

/**
 * Report a hydration error through the registered handler.
 *
 * This is called automatically by Ruvyxa's hydration runtime when a mismatch
 * is detected. You can also call it manually for custom error reporting.
 *
 * In production mode, errors are silently reported to the handler without
 * crashing the UI. In development, they are also logged to the console.
 */
export function reportHydrationError(
  error: unknown,
  context: { componentStack?: string; digest?: string } = {},
): void {
  if (globalErrorHandler) {
    try {
      globalErrorHandler(error, context)
    } catch {
      // Never let error reporting crash the app
    }
  }

  // In non-production environments, also log to console for visibility
  if (typeof globalThis !== "undefined" && (globalThis as Record<string, unknown>).__RUVYXA_DEV__) {
    console.error("[ruvyxa] Hydration error:", error, context)
  }
}
