/**
 * Client-side navigation for Ruvyxa.
 *
 * Ruvyxa route bundles already know how to re-render into an existing root:
 * the generated entry calls `__RUVYXA_ROOT__.render(...)` when one exists and
 * `hydrateRoot(...)` when it does not. This module supplies the missing half —
 * a route table, history integration, and bundle loading — so a link click can
 * swap pages without a document load.
 *
 * ## Contract with the generated entry
 *
 * Every client route bundle publishes two globals (see `build_entry_source` in
 * `crates/ruvyxa_bundler/src/output.rs` and `clientEntrySource` in
 * `packages/ruvyxa/runtime/entry-templates.mjs`):
 *
 * - `globalThis.__RUVYXA_ROUTE_CONTEXT__` — the React context the routing
 *   hooks read. It is created on `globalThis` rather than imported because a
 *   generated entry cannot depend on this package; an app may render plain
 *   React pages with no Ruvyxa components at all.
 * - `globalThis.__RUVYXA_ROUTES__[routePath]` — a function from a route
 *   context to the composed element tree, registered when the bundle executes.
 *
 * A route whose bundle has already executed is re-rendered from that registry
 * directly. `import()` caches by URL and would not re-run the bundle, so
 * navigating away and back would otherwise render nothing.
 */

import { createRouteMatcher, type RouteManifestEntry, type RouteParams } from './route-match.js'

/**
 * The active route, as seen by the routing hooks.
 *
 * Deliberately excludes the query string. A server render has no reliable way
 * to know it in every deployment target, so putting it here would make the
 * provider value differ between the server HTML and the first client render —
 * a hydration mismatch. `useSearchParams` reads it from an external store with
 * an empty server snapshot instead, which React resolves after hydration.
 */
export interface RouteContextValue {
  /** Pathname of the current URL, without search or hash. */
  pathname: string
  /** Parameters extracted from the matched route pattern. */
  params: RouteParams
  /** The matched route pattern, e.g. `/blog/[slug]`. */
  route: string
}

/** Options accepted by the imperative navigation methods. */
export interface NavigateOptions {
  /** Replace the current history entry instead of pushing a new one. */
  replace?: boolean
  /** Scroll to the top after navigating. Defaults to `true`. */
  scroll?: boolean
}

/** Public navigation surface returned by {@link useRouter}. */
export interface RuvyxaRouter {
  push(href: string, options?: NavigateOptions): Promise<void>
  replace(href: string, options?: NavigateOptions): Promise<void>
  back(): void
  forward(): void
  /** Re-render the current route from its already-loaded bundle. */
  refresh(): void
  /** Warm a route's bundle so a later navigation renders immediately. */
  prefetch(href: string): void
  /** `true` while a navigation is loading a bundle. */
  readonly pending: boolean
}

type TreeFactory = (context: RouteContextValue) => unknown

interface RouterGlobals {
  __RUVYXA_ROUTES__?: Record<string, TreeFactory>
  __RUVYXA_ROOT__?: { render(tree: unknown): void }
  __RUVYXA_ROUTE_PARAMS__?: RouteParams
  __RUVYXA_REQUEST_PATH__?: string
  __RUVYXA_ROUTE_MANIFEST__?: { routes?: RouteManifestEntry[] }
  __RUVYXA_ROUTER_INSTANCE__?: RouterInstance
}

const globals = globalThis as unknown as RouterGlobals

/**
 * Where the build publishes the lean client route table.
 *
 * Deliberately not `manifest.json`: that file is the build report and carries
 * absolute source paths and per-route module graphs that must not be shipped
 * to browsers. `route-manifest.json` holds only `{ path, src, sharedChunks }`.
 */
const MANIFEST_URL = '/__ruvyxa/client/route-manifest.json'

/** Internal navigation singleton shared by the routing hooks and `<Link>`. */
export interface RouterInstance {
  subscribe(listener: () => void): () => void
  getSnapshot(): RouteContextValue
  /** Live query string, including the leading `?`. Empty outside a browser. */
  getSearch(): string
  getPending(): boolean
  navigate(
    href: string,
    options: NavigateOptions & { history?: 'push' | 'replace' | 'none' },
  ): Promise<void>
  prefetch(href: string): void
  refresh(): void
}

/**
 * Resolve `href` against the current document, or return `null` when it is not
 * a same-origin navigation this router can own.
 *
 * Cross-origin links, downloads, and non-HTTP schemes must reach the browser
 * untouched; intercepting them would break `mailto:`, `tel:`, and file
 * downloads.
 */
function resolveInternalUrl(href: string): URL | null {
  if (typeof window === 'undefined') return null
  let url: URL
  try {
    url = new URL(href, window.location.href)
  } catch {
    return null
  }
  if (url.origin !== window.location.origin) return null
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return null
  return url
}

function loadManifestRoutes(): RouteManifestEntry[] {
  const inline = globals.__RUVYXA_ROUTE_MANIFEST__?.routes
  return Array.isArray(inline) ? inline : []
}

function createRouter(): RouterInstance {
  const listeners = new Set<() => void>()
  let routes = loadManifestRoutes()
  let match = createRouteMatcher(routes)
  let manifestRequest: Promise<void> | null = null
  let pending = false
  // Guards against a slow first navigation overwriting a faster later one.
  let navigationId = 0

  const initialPathname = typeof window === 'undefined' ? '/' : window.location.pathname

  let snapshot: RouteContextValue = {
    pathname: globals.__RUVYXA_REQUEST_PATH__ ?? initialPathname,
    params: globals.__RUVYXA_ROUTE_PARAMS__ ?? {},
    route: globals.__RUVYXA_REQUEST_PATH__ ?? initialPathname,
  }
  // Cached so `getSnapshot` for `useSyncExternalStore` returns a stable string
  // between navigations; reading `location.search` per call is stable too, but
  // this keeps the value correct inside a `popstate` handler that runs before
  // the listener notification.
  let search = typeof window === 'undefined' ? '' : window.location.search

  function emit(): void {
    for (const listener of listeners) listener()
  }

  /**
   * Fetch the route table once, lazily.
   *
   * Fetching it eagerly on import would cost a request on every page even when
   * the app never navigates client-side.
   */
  function ensureManifest(): Promise<void> {
    if (routes.length > 0) return Promise.resolve()
    manifestRequest ??= fetch(MANIFEST_URL, { credentials: 'same-origin' })
      .then((response) => (response.ok ? response.json() : null))
      .then((manifest: { routes?: RouteManifestEntry[] } | null) => {
        if (manifest?.routes) {
          routes = manifest.routes
          match = createRouteMatcher(routes)
        }
      })
      .catch(() => {
        // A missing or unreadable manifest is not fatal: navigation falls back
        // to a document load, which is what happens without this router at all.
      })
    return manifestRequest
  }

  function renderRoute(context: RouteContextValue): boolean {
    const factory = globals.__RUVYXA_ROUTES__?.[context.route]
    const root = globals.__RUVYXA_ROOT__
    if (!factory || !root) return false
    globals.__RUVYXA_ROUTE_PARAMS__ = context.params
    globals.__RUVYXA_REQUEST_PATH__ = context.pathname
    root.render(factory(context))
    return true
  }

  /**
   * Execute a route bundle so it registers its tree factory.
   *
   * The globals are set first because the bundle reads them to build its
   * initial tree — an already-cached module would otherwise render the
   * previous route's parameters.
   */
  async function loadRoute(
    entry: RouteManifestEntry,
    context: RouteContextValue,
  ): Promise<boolean> {
    if (globals.__RUVYXA_ROUTES__?.[context.route]) return true
    if (!entry.src) return false
    globals.__RUVYXA_ROUTE_PARAMS__ = context.params
    globals.__RUVYXA_REQUEST_PATH__ = context.pathname
    try {
      await import(/* @vite-ignore */ entry.src)
    } catch {
      return false
    }
    return Boolean(globals.__RUVYXA_ROUTES__?.[context.route])
  }

  function hardNavigate(url: URL, replace: boolean): void {
    if (replace) window.location.replace(url.href)
    else window.location.assign(url.href)
  }

  async function navigate(
    href: string,
    options: NavigateOptions & { history?: 'push' | 'replace' | 'none' } = {},
  ): Promise<void> {
    const url = resolveInternalUrl(href)
    if (!url) {
      if (typeof window !== 'undefined') window.location.assign(href)
      return
    }

    const historyMode = options.history ?? (options.replace ? 'replace' : 'push')
    const id = ++navigationId

    await ensureManifest()
    if (id !== navigationId) return

    const matched = match(url.pathname)
    // No client route owns this URL — it may be an API route, a redirect, or a
    // rewrite the server resolves. Hand it to the browser rather than guess.
    if (!matched) {
      hardNavigate(url, historyMode === 'replace')
      return
    }

    const context: RouteContextValue = {
      pathname: url.pathname,
      params: matched.params,
      route: matched.route.path,
    }

    if (!globals.__RUVYXA_ROUTES__?.[context.route]) {
      pending = true
      emit()
      const loaded = await loadRoute(matched.route, context)
      pending = false
      if (id !== navigationId) {
        emit()
        return
      }
      if (!loaded) {
        emit()
        hardNavigate(url, historyMode === 'replace')
        return
      }
    }

    if (historyMode === 'push') window.history.pushState({ ruvyxa: true }, '', url.href)
    else if (historyMode === 'replace') window.history.replaceState({ ruvyxa: true }, '', url.href)

    snapshot = context
    search = url.search
    if (!renderRoute(context)) {
      hardNavigate(url, historyMode === 'replace')
      return
    }
    emit()

    if (options.scroll !== false && historyMode !== 'none') {
      window.scrollTo(0, 0)
    }
  }

  function prefetch(href: string): void {
    const url = resolveInternalUrl(href)
    if (!url) return
    void ensureManifest().then(() => {
      const matched = match(url.pathname)
      if (!matched?.route.src) return
      if (globals.__RUVYXA_ROUTES__?.[matched.route.path]) return
      // `modulepreload` warms the network and the module graph without
      // executing the bundle, so a prefetch cannot register a tree factory
      // built from the wrong parameters.
      if (
        document.querySelector(`link[rel="modulepreload"][href="${CSS.escape(matched.route.src)}"]`)
      ) {
        return
      }
      const link = document.createElement('link')
      link.rel = 'modulepreload'
      link.href = matched.route.src
      document.head.append(link)
      for (const chunk of matched.route.sharedChunks ?? []) {
        const chunkLink = document.createElement('link')
        chunkLink.rel = 'modulepreload'
        chunkLink.href = chunk.src
        document.head.append(chunkLink)
      }
    })
  }

  function refresh(): void {
    renderRoute(snapshot)
    emit()
  }

  if (typeof window !== 'undefined') {
    window.addEventListener('popstate', () => {
      // The browser has already changed the URL; re-pushing it would corrupt
      // the history stack, and restoring scroll is the browser's job here.
      void navigate(window.location.href, { history: 'none', scroll: false })
    })
  }

  return {
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    getSnapshot: () => snapshot,
    getSearch: () => search,
    getPending: () => pending,
    navigate,
    prefetch,
    refresh,
  }
}

/**
 * The router singleton for this document.
 *
 * Kept on `globalThis` so a route bundle and the app's own copy of this
 * package share one instance even if they were bundled separately.
 */
export function getRouterInstance(): RouterInstance {
  globals.__RUVYXA_ROUTER_INSTANCE__ ??= createRouter()
  return globals.__RUVYXA_ROUTER_INSTANCE__
}
