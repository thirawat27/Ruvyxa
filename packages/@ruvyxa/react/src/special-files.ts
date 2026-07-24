/**
 * Prop contracts for the route special files (`error.tsx`, `loading.tsx`,
 * `not-found.tsx`).
 *
 * The generated route tree renders these components directly (see
 * `routeTreeFunction` / `routeBoundaryPrelude` in
 * `packages/ruvyxa/runtime/entry-templates.mjs`). Only `error.tsx` receives
 * props; `loading.tsx` and `not-found.tsx` are rendered with none, so they are
 * plain `() => ReactNode` components and need no dedicated type here.
 */

/**
 * Props passed to a route's `error.tsx` component.
 *
 * Mirrors Next.js: the caught `error`, and a `reset` that re-mounts the route
 * subtree to retry. `reset` only re-renders after hydration, so an interactive
 * retry button requires the page to be hydrated (the default).
 */
export interface RouteErrorProps {
  /** The error the boundary caught. */
  error: Error
  /** Clear the error and re-render the route subtree. */
  reset: () => void
}
