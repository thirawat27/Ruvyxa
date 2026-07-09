import { useCallback, useEffect, useRef, useState } from 'react'

/**
 * Options for the useRuvyxaLoader hook.
 */
export interface UseLoaderOptions {
  /** If false, the loader will not execute automatically. Default: true. */
  enabled?: boolean
  /** Dependencies that trigger a refetch when changed. */
  deps?: unknown[]
}

/**
 * Result returned by the useRuvyxaLoader hook.
 */
export interface UseLoaderResult<T> {
  /** The loaded data, or undefined if still loading or errored. */
  data: T | undefined
  /** Whether the loader is currently fetching. */
  loading: boolean
  /** Error thrown by the loader, if any. */
  error: Error | undefined
  /** Manually trigger a refetch. */
  refetch: () => void
}

/**
 * React hook for consuming Ruvyxa loaders on the client side.
 *
 * Provides loading, error, and data states with automatic refetch support.
 * Handles race conditions from stale closures and component unmounting.
 *
 * Usage:
 * ```tsx
 * function UserProfile({ userId }: { userId: string }) {
 *   const { data, loading, error, refetch } = useRuvyxaLoader(
 *     () => fetch(`/api/users/${userId}`).then(r => r.json()),
 *     { deps: [userId] }
 *   )
 *
 *   if (loading) return <p>Loading...</p>
 *   if (error) return <p>Error: {error.message}</p>
 *   return <div>{data.name} <button onClick={refetch}>Refresh</button></div>
 * }
 * ```
 */
export function useRuvyxaLoader<T>(
  loader: () => Promise<T>,
  options: UseLoaderOptions = {},
): UseLoaderResult<T> {
  const { enabled = true, deps = [] } = options

  const [data, setData] = useState<T | undefined>(undefined)
  const [loading, setLoading] = useState(enabled)
  const [error, setError] = useState<Error | undefined>(undefined)

  // Track the current request to handle race conditions
  const requestIdRef = useRef(0)
  const mountedRef = useRef(true)

  const execute = useCallback(() => {
    if (!enabled) return

    const currentId = ++requestIdRef.current
    setLoading(true)
    setError(undefined)

    loader()
      .then((result) => {
        // Only update state if this is still the latest request and component is mounted
        if (mountedRef.current && currentId === requestIdRef.current) {
          setData(result)
          setLoading(false)
        }
      })
      .catch((err) => {
        if (mountedRef.current && currentId === requestIdRef.current) {
          setError(err instanceof Error ? err : new Error(String(err)))
          setLoading(false)
        }
      })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, loader])

  useEffect(() => {
    mountedRef.current = true
    execute()
    return () => {
      mountedRef.current = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [execute, ...deps])

  const refetch = useCallback(() => {
    execute()
  }, [execute])

  return { data, loading, error, refetch }
}
