/**
 * `notFound()` — render the nearest `not-found.tsx` for the current route.
 *
 * Calling it throws a tagged error that the route's generated boundary catches
 * and replaces with the not-found UI, exactly like Next.js's `notFound()`.
 *
 * ## The contract with the generated boundary
 *
 * The generated route tree wraps the page in an inline boundary (see
 * `routeBoundaryPrelude` in `packages/ruvyxa/runtime/entry-templates.mjs` and
 * its Rust mirror in `crates/ruvyxa_bundler/src/output.rs`). That boundary
 * cannot import this package — an app may render plain React pages and never
 * install `@ruvyxa/react` — so the two sides meet on a well-known own property
 * name rather than a shared symbol or class: an error is a not-found signal
 * when `error.__ruvyxaNotFound === true`. Keep {@link NOT_FOUND_PROPERTY} in
 * step with the string both generators check.
 */

/** Own-property name that marks a thrown error as a not-found signal. */
export const NOT_FOUND_PROPERTY = '__ruvyxaNotFound' as const

/** A thrown value produced by {@link notFound}. */
export interface NotFoundError extends Error {
  __ruvyxaNotFound: true
}

/**
 * Abort rendering the current route and show its `not-found.tsx`.
 *
 * Returns `never`: it always throws, so TypeScript narrows the code after a
 * `notFound()` call as unreachable, the same way `throw` would.
 *
 * ```tsx
 * const post = await getPost(params.slug)
 * if (!post) notFound()
 * return <Article post={post} />
 * ```
 */
export function notFound(): never {
  const error = new Error('RUVYXA_NOT_FOUND') as NotFoundError
  error.__ruvyxaNotFound = true
  throw error
}

/** Whether `value` is the error thrown by {@link notFound}. */
export function isNotFoundError(value: unknown): value is NotFoundError {
  return (
    value instanceof Error && (value as { __ruvyxaNotFound?: unknown }).__ruvyxaNotFound === true
  )
}
