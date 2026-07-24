/**
 * Routing hooks backed by the context every Ruvyxa route bundle provides.
 *
 * The context object is created on `globalThis` rather than exported from a
 * module because the generated route entry has to provide it without importing
 * this package — an app may render plain React pages and never install
 * `@ruvyxa/react`. Both sides reach the same object through
 * `globalThis.__RUVYXA_ROUTE_CONTEXT__`, so provider and consumer match
 * whichever module happens to load first.
 */

import { createContext, useContext, useSyncExternalStore, type Context } from 'react'

import {
  getRouterInstance,
  type NavigateOptions,
  type RouteContextValue,
  type RuvyxaRouter,
} from './router.js'
import type { RouteParams } from './route-match.js'

const CONTEXT_KEY = '__RUVYXA_ROUTE_CONTEXT__'

function sharedRouteContext(): Context<RouteContextValue | null> {
  const store = globalThis as unknown as Record<string, unknown>
  const existing = store[CONTEXT_KEY]
  if (existing) return existing as Context<RouteContextValue | null>
  const created = createContext<RouteContextValue | null>(null)
  store[CONTEXT_KEY] = created
  return created
}

/** The React context a Ruvyxa route bundle populates for the active route. */
export const RouteContext: Context<RouteContextValue | null> = sharedRouteContext()

/**
 * Read the active route context.
 *
 * Falls back to the router snapshot so a component rendered outside a Ruvyxa
 * route tree — a test harness, a portal, a standalone story — still sees the
 * current URL instead of throwing.
 */
export function useRouteContext(): RouteContextValue {
  const fromProvider = useContext(RouteContext)
  if (fromProvider) return fromProvider
  return getRouterInstance().getSnapshot()
}

/** The current pathname, without search string or hash. */
export function usePathname(): string {
  return useRouteContext().pathname
}

/** Parameters extracted from the matched route pattern. */
export function useParams(): RouteParams {
  return useRouteContext().params
}

/**
 * The current query string as a `URLSearchParams`.
 *
 * Reads from the router rather than the route context: a server render cannot
 * see the query string in every deployment target, so this returns an empty
 * set during SSR and the real values once hydrated. Routing that must be
 * identical in the server HTML belongs in the path, not the query.
 *
 * A new `URLSearchParams` is built per render because it is mutable; handing
 * out a shared one would let a caller mutate another component's view of the
 * URL.
 */
export function useSearchParams(): URLSearchParams {
  const instance = getRouterInstance()
  const search = useSyncExternalStore(instance.subscribe, instance.getSearch, () => '')
  return new URLSearchParams(search)
}

/** The matched route pattern, e.g. `/blog/[slug]`. */
export function useSelectedRoute(): string {
  return useRouteContext().route
}

/**
 * Imperative navigation.
 *
 * `pending` is `true` while a navigation waits on a route bundle, which is the
 * hook a progress bar or a disabled submit button subscribes to.
 */
export function useRouter(): RuvyxaRouter {
  const instance = getRouterInstance()
  const pending = useSyncExternalStore(
    instance.subscribe,
    instance.getPending,
    // Nothing is ever pending during a server render.
    () => false,
  )

  return {
    push: (href: string, options?: NavigateOptions) => instance.navigate(href, { ...options }),
    replace: (href: string, options?: NavigateOptions) =>
      instance.navigate(href, { ...options, replace: true }),
    back: () => {
      if (typeof window !== 'undefined') window.history.back()
    },
    forward: () => {
      if (typeof window !== 'undefined') window.history.forward()
    },
    refresh: () => instance.refresh(),
    prefetch: (href: string) => instance.prefetch(href),
    pending,
  }
}
