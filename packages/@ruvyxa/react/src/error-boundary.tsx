import { Component, type ErrorInfo, type ReactNode } from 'react'

/**
 * Props passed to the fallback component when an error is caught.
 */
export interface ErrorFallbackProps {
  /** The error that was thrown. */
  error: Error
  /** Call this to reset the error boundary and render its children again. */
  resetError: () => void
  /**
   * Re-fetch the current route from the server, then reset the boundary.
   *
   * `resetError` re-renders against data the client already has, so it can only
   * recover from a fault in the render itself. When the failure was the data —
   * a request that errored, a payload that never arrived — the request has to
   * be repeated, and that is what this does.
   *
   * Outside a mounted router there is nothing to re-fetch from, so it falls
   * back to `resetError`. Resolves once the boundary has been reset.
   */
  retry: () => Promise<void>
}

/**
 * Props for the RuvyxaErrorBoundary component.
 */
export interface ErrorBoundaryProps {
  /** The content to render when no error has occurred. */
  children: ReactNode
  /** Component to render when an error is caught. */
  fallback: (props: ErrorFallbackProps) => ReactNode
  /** Optional callback invoked when an error is caught. Useful for logging/reporting. */
  onError?: (error: Error, info: ErrorInfo) => void
}

interface ErrorBoundaryState {
  error: Error | null
}

/**
 * Production-grade React error boundary for Ruvyxa apps.
 *
 * Catches rendering errors in child components and displays a fallback UI
 * instead of crashing the entire page. Supports error recovery via the
 * `resetError` callback passed to the fallback component.
 *
 * Usage:
 * ```tsx
 * <RuvyxaErrorBoundary
 *   fallback={({ error, resetError }) => (
 *     <div>
 *       <p>Something went wrong: {error.message}</p>
 *       <button onClick={resetError}>Retry</button>
 *     </div>
 *   )}
 *   onError={(error, info) => {
 *     // Send to error reporting service
 *     reportError(error, info.componentStack)
 *   }}
 * >
 *   <App />
 * </RuvyxaErrorBoundary>
 * ```
 */
export class RuvyxaErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props)
    this.state = { error: null }
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    this.props.onError?.(error, info)
  }

  resetError = (): void => {
    this.setState({ error: null })
  }

  retry = async (): Promise<void> => {
    // Reached through the global rather than `useRouter()` because this is a
    // class component, and because the boundary has to keep working on a page
    // that never mounted the client router.
    const router = (globalThis as { __RUVYXA_ROUTER_INSTANCE__?: { retry?: () => Promise<void> } })
      .__RUVYXA_ROUTER_INSTANCE__
    if (typeof router?.retry !== 'function') {
      this.resetError()
      return
    }
    try {
      await router.retry()
      this.resetError()
    } catch (error) {
      // A failed retry replaces the error rather than clearing it: the boundary
      // must not show its children again when the data still is not there.
      this.setState({ error: error instanceof Error ? error : new Error(String(error)) })
    }
  }

  render(): ReactNode {
    if (this.state.error) {
      return this.props.fallback({
        error: this.state.error,
        resetError: this.resetError,
        retry: this.retry,
      })
    }
    return this.props.children
  }
}
