import { Component, type ErrorInfo, type ReactNode } from "react"

/**
 * Props passed to the fallback component when an error is caught.
 */
export interface ErrorFallbackProps {
  /** The error that was thrown. */
  error: Error
  /** Call this to reset the error boundary and retry rendering. */
  resetError: () => void
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

  render(): ReactNode {
    if (this.state.error) {
      return this.props.fallback({
        error: this.state.error,
        resetError: this.resetError,
      })
    }
    return this.props.children
  }
}
