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

import {
  canonicalRoutePath,
  createRouteMatcher,
  type RouteManifestEntry,
  type RouteParams,
} from '@ruvyxa/core/route-match'
import type { FlightValue } from '@ruvyxa/core/server'

import type { RouteHref } from './route-types.js'

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
  /** Public server-component data for this exact route artifact, when enabled. */
  flight?: FlightValue
  /**
   * The intercepting route showing over this one, when a soft navigation
   * opened it.
   *
   * Absent on the server and on a hard load, which is what makes a refresh
   * render the intercepted URL's real page instead of the overlay.
   */
  intercept?: RouteInterceptState
}

/** An intercepting route the client router has opened over the mounted page. */
export interface RouteInterceptState {
  /** Route id of the level whose layout holds the slot (`app/feed`). */
  level: string
  /** Slot name the overlay replaces. */
  name: string
  /** Route pattern the interception covers. */
  target: string
  /** Parameters matched from the intercepted URL. */
  params: RouteParams
  /** The intercepted URL's pathname. */
  path: string
}

/** What a route's bundle publishes about the URLs it can intercept. */
interface InterceptRegistryEntry {
  level: string
  name: string
  target: string
}

/** Options accepted by the imperative navigation methods. */
export interface NavigateOptions {
  /** Replace the current history entry instead of pushing a new one. */
  replace?: boolean
  /** Scroll to the top after navigating. Defaults to `true`. */
  scroll?: boolean
  /** Animate this navigation with the browser View Transitions API when available. */
  viewTransition?: boolean
}

/**
 * Public navigation surface returned by {@link useRouter}.
 *
 * `href` is narrowed to the project's real routes once
 * `.ruvyxa/types/routes.d.ts` is generated and included by `tsconfig.json`, and
 * is plain `string` otherwise. `RouterInstance` below stays untyped in that
 * respect on purpose: it is the engine seam `<Link>` and the hooks call, and it
 * must accept the URLs the browser hands back on `popstate`.
 */
export interface RuvyxaRouter {
  push(href: RouteHref, options?: NavigateOptions): Promise<void>
  replace(href: RouteHref, options?: NavigateOptions): Promise<void>
  back(): void
  forward(): void
  /** Re-render the current route from its already-loaded bundle. */
  refresh(): void
  /**
   * Re-fetch the current route's server payload, then re-render it.
   *
   * `refresh()` re-runs the client tree against the payload it already has, so
   * it cannot recover from a server-side failure. This discards that payload
   * first, which is what an `error.tsx` retry button needs: the render failed
   * because of what the server sent (or did not send), and rendering the same
   * bytes again would fail the same way.
   */
  retry(): Promise<void>
  /** Warm a route's bundle so a later navigation renders immediately. */
  prefetch(href: RouteHref): void
  /** `true` while a navigation is loading a bundle. */
  readonly pending: boolean
}

type TreeFactory = (context: RouteContextValue) => unknown
type RscTreeFactory = (payload: string, context: RouteContextValue) => unknown

interface RouterGlobals {
  __RUVYXA_ROUTES__?: Record<string, TreeFactory>
  /**
   * Server-components tree factories, keyed by route pattern.
   *
   * Separate from `__RUVYXA_ROUTES__` because the shapes differ: an ordinary
   * factory builds a tree from a context alone, while this one also needs the
   * Flight payload the route was rendered from. One registry answering both
   * would make "which kind of route is this" a guess at every call site.
   */
  __RUVYXA_RSC_ROUTES__?: Record<string, RscTreeFactory>
  /**
   * Loading shells, keyed by route pattern.
   *
   * Separate from `__RUVYXA_ROUTES__` because only a route with a `loading.tsx`
   * has one, and the router has to be able to tell "no shell" from "a tree that
   * would suspend immediately".
   */
  __RUVYXA_SHELLS__?: Record<string, TreeFactory>
  __RUVYXA_INTERCEPTS__?: Record<string, InterceptRegistryEntry[]>
  __RUVYXA_ROOT__?: { render(tree: unknown): void }
  __RUVYXA_ROUTE_PARAMS__?: RouteParams
  __RUVYXA_REQUEST_PATH__?: string
  /**
   * Route pattern the served document was built for, published by the generated
   * entry next to its registry registration. The registry is keyed by pattern,
   * so this is what makes the initial route addressable.
   */
  __RUVYXA_ROUTE_PATTERN__?: string
  __RUVYXA_ROUTE_ARTIFACTS__?: Record<string, string>
  __RUVYXA_ROUTE_MANIFEST__?: { routes?: RouteManifestEntry[] }
  __RUVYXA_FLIGHT__?: FlightValue
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
const FLIGHT_URL = '/__ruvyxa/flight'
/** Where a soft navigation asks for a server-components route's payload. */
const RSC_URL = '/__ruvyxa/rsc'
/**
 * The header that makes {@link RSC_URL} same-origin-only.
 *
 * That endpoint renders with the visitor's cookies and runs server functions,
 * and it carries no origin policy: a cross-origin page being unable to set a
 * non-safelisted header without a preflight nothing answers is the whole
 * defence. Both request hosts check it, and
 * `tests/fixtures/framework-endpoint-conformance.json` requires it of them —
 * this is the browser end of the same rule, named rather than inlined so the
 * grep that finds every spelling finds this one too.
 */
const RSC_REQUEST_HEADER = 'x-ruvyxa-rsc'
const FLIGHT_PROTOCOL = 'ruvyxa.flight'
const FLIGHT_PROTOCOL_VERSION = 1
const FLIGHT_BYTE_LIMIT = 1024 * 1024
const FLIGHT_CACHE_LIMIT = 16

interface FlightEntry {
  controller: AbortController
  promise: Promise<FlightValue>
}

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
  retry(): Promise<void>
}

/**
 * Schemes the router will hand to the browser when the URL is not its own.
 *
 * Deliberately generous: these are the schemes an ordinary `<a href>` reaches
 * for, and refusing one would break a link the browser has always handled. It
 * is an allow-list rather than a deny-list because a `router.push()` argument
 * is frequently *data* — a CMS link field, a `?next=` parameter, a profile URL
 * — and a new scheme should have to be admitted deliberately.
 */
const NAVIGABLE_EXTERNAL_PROTOCOLS = new Set(['http:', 'https:', 'mailto:', 'tel:', 'sms:'])

/**
 * What `href` turned out to be.
 *
 * `internal` is a same-origin page this router can render itself; `external` is
 * a real navigation the browser must perform; `refused` is not a navigation at
 * all. Separating the last two matters: the old `URL | null` answer conflated
 * "not mine, hand it to the browser" with "not a navigation", and the
 * fall-through then replayed the caller's *raw string* into
 * `location.assign` — which executes a `javascript:` URL in this document.
 */
export type NavigationTarget =
  | { kind: 'internal'; url: URL }
  | { kind: 'external'; url: URL }
  /**
   * `reason` is absent only when there is nothing to report — no browser to
   * navigate in at all. Every refusal a caller could have avoided carries one,
   * which is what lets the classifier stay silent and the callers that *asked
   * for a navigation* do the reporting.
   */
  | { kind: 'refused'; reason?: string }

/**
 * A `#fragment` against the page already showing.
 *
 * Same-origin http(s) documents never reach this — the internal arm already
 * claimed them. It exists so a document served over some other scheme (an
 * exported site opened from `file:`) keeps its in-page anchors working.
 */
function isSamePageFragment(url: URL): boolean {
  if (url.hash === '') return false
  const here = new URL(window.location.href)
  here.hash = ''
  const there = new URL(url.href)
  there.hash = ''
  return here.href === there.href
}

/**
 * Classify `href` against the current document, reporting nothing.
 *
 * Cross-origin links, downloads, and `mailto:`/`tel:`/`sms:` must reach the
 * browser untouched; intercepting them would break links that have always
 * worked. Everything else is refused rather than passed along, because
 * `useRouter().push()` is a sink an `<a>` click is not: without the router
 * there is no way for a page to turn a data string into a navigation.
 *
 * Exported for `<Link>`, which has to ask the question *before* it calls
 * `preventDefault()`: a click it suppresses and the router then refuses is a
 * link that does nothing at all. It asks silently because a refusal there is
 * not one — the anchor still carries the href and the browser goes on to
 * handle it, exactly as it did before this router existed. Not part of the
 * package's public surface: `index.ts` does not re-export it and the
 * `exports` map does not expose this module.
 */
export function classifyNavigationTarget(href: string): NavigationTarget {
  // Not a browser: there is nothing to navigate, and nothing to report.
  if (typeof window === 'undefined') return { kind: 'refused' }
  let url: URL
  try {
    url = new URL(href, window.location.href)
  } catch {
    return { kind: 'refused', reason: 'it is not a URL' }
  }
  if (
    url.origin === window.location.origin &&
    (url.protocol === 'http:' || url.protocol === 'https:')
  ) {
    return { kind: 'internal', url }
  }
  if (NAVIGABLE_EXTERNAL_PROTOCOLS.has(url.protocol) || isSamePageFragment(url)) {
    return { kind: 'external', url }
  }
  return {
    kind: 'refused',
    reason: `"${url.protocol}" is not a scheme a router may navigate to`,
  }
}

/**
 * Classify `href` for a caller that asked for a navigation, and say so loudly
 * when the answer is no.
 *
 * `navigate()` and `prefetch()` were handed a URL and are about to do nothing
 * with it; silence there is how a `javascript:` href in a CMS field looks like
 * a dead button rather than a refused one.
 */
function resolveNavigationTarget(href: string): NavigationTarget {
  const target = classifyNavigationTarget(href)
  if (target.kind === 'refused' && target.reason !== undefined) {
    refuseNavigation(href, target.reason)
  }
  return target
}

function refuseNavigation(href: string, reason: string): void {
  console.error(
    `Ruvyxa router refused to navigate to ${JSON.stringify(href)}: ${reason}. ` +
      `Navigable schemes are ${[...NAVIGABLE_EXTERNAL_PROTOCOLS].join(', ')} ` +
      'and same-origin paths.',
  )
}

function loadManifestRoutes(): RouteManifestEntry[] {
  const inline = globals.__RUVYXA_ROUTE_MANIFEST__?.routes
  return Array.isArray(inline) ? inline : []
}

function fallbackPathname(): string {
  return typeof window === 'undefined' ? '/' : window.location.pathname
}

function createRouter(): RouterInstance {
  const listeners = new Set<() => void>()
  let routes = loadManifestRoutes()
  let match = createRouteMatcher(routes)
  let manifestRequest: Promise<void> | null = null
  // Pending state belongs to a navigation generation. A superseded bundle
  // load must never clear the state now owned by a newer navigation.
  let pendingNavigationId: number | null = null
  // Guards against a slow first navigation overwriting a faster later one.
  let navigationId = 0
  let navigationAbort: AbortController | null = null
  const flightCache = new Map<string, FlightEntry>()

  const initialPathname = globals.__RUVYXA_REQUEST_PATH__ ?? fallbackPathname()

  let snapshot: RouteContextValue = {
    pathname: initialPathname,
    params: globals.__RUVYXA_ROUTE_PARAMS__ ?? {},
    // The route is the *pattern*, never the URL. Seeding it from the request
    // path made `__RUVYXA_ROUTES__[snapshot.route]` a guaranteed miss on every
    // dynamic route, which silently turned `refresh()` into a no-op until the
    // first client navigation replaced the snapshot. Prefer the pattern the
    // entry published; fall back to matching the manifest, then to the URL so a
    // page served by an older bundle still reports something usable.
    route:
      globals.__RUVYXA_ROUTE_PATTERN__ ?? match(initialPathname)?.route.path ?? initialPathname,
    flight: globals.__RUVYXA_FLIGHT__,
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
    globals.__RUVYXA_ROUTE_PATTERN__ = context.route
    root.render(factory(context))
    return true
  }

  /**
   * Paint the destination's layouts and its `loading.tsx`, with no server data.
   *
   * Everything this renders is already in the route bundle, so once that bundle
   * has executed there is nothing left to wait for. That is the whole point: a
   * navigation to a slow route otherwise leaves the previous page on screen
   * until the Flight payload lands, which reads as a dead click.
   *
   * `flight` is deliberately left undefined in the context. The shell must not
   * be handed a stale payload from the page being navigated away from, and any
   * component that reads it has to treat "not here yet" as a real state.
   */
  function renderShell(context: RouteContextValue): boolean {
    const factory = globals.__RUVYXA_SHELLS__?.[context.route]
    const root = globals.__RUVYXA_ROOT__
    if (!factory || !root) return false
    globals.__RUVYXA_ROUTE_PARAMS__ = context.params
    globals.__RUVYXA_REQUEST_PATH__ = context.pathname
    globals.__RUVYXA_ROUTE_PATTERN__ = context.route
    root.render(factory({ ...context, flight: undefined }))
    return true
  }

  /**
   * Whether this navigation has to fetch the route's bundle before rendering.
   *
   * Two ways it does: the registry has never seen this route, or it holds a
   * tree built by an earlier deploy. The second is what makes this more than a
   * presence check — after a deploy the registry still answers, but with a
   * factory whose chunks no longer exist on the server.
   */
  function needsRouteLoad(entry: RouteManifestEntry, route: string): boolean {
    if (!globals.__RUVYXA_ROUTES__?.[route]) return true
    if (!entry.artifactVersion) return false
    return globals.__RUVYXA_ROUTE_ARTIFACTS__?.[route] !== entry.artifactVersion
  }

  /**
   * Await the route's server payload, or report that it never arrived.
   *
   * Both navigation branches need the same three-way answer — no payload was
   * expected, one arrived, or the request failed — and both used to spell it
   * out separately, one with a `try`/`catch` and one with a `.catch()` plus an
   * `undefined` check. A route with no Flight payload succeeds with no value,
   * which is different from a route whose payload failed to load.
   */
  async function settleFlight(
    flight: FlightEntry | null,
  ): Promise<{ ok: boolean; value: FlightValue | undefined }> {
    if (!flight) return { ok: true, value: undefined }
    try {
      return { ok: true, value: await flight.promise }
    } catch {
      return { ok: false, value: undefined }
    }
  }

  /**
   * Paint the destination's loading state and commit its URL.
   *
   * Both navigation branches — one that has to fetch the route bundle first and
   * one that already has it — reach this same point: the bundle is present, the
   * data is not, and the shell is the best thing to show. Committing the URL
   * here rather than after the data lands keeps the address bar from lagging
   * behind what the user can already see.
   *
   * Returns whether the shell went up, which is what tells the rest of the
   * navigation that history has already been pushed for it.
   */
  /**
   * The interception this navigation opens, or `null` for an ordinary one.
   *
   * The table belongs to the route the user is *standing on*: an interception
   * is an overlay that the mounted bundle already contains, which is what lets
   * it open without a round trip. A visitor arriving at the same URL from
   * anywhere else — a shared link, a reload, a route with no such table — gets
   * the ordinary page, because nothing here answers.
   */
  function matchIntercept(
    currentRoute: string,
    target: string,
    params: RouteParams,
    pathname: string,
  ): RouteInterceptState | null {
    // Never overlay a route on itself: standing on the real page and following
    // a link to it again is a refresh, not an interception.
    if (currentRoute === target) return null
    const entry = globals.__RUVYXA_INTERCEPTS__?.[currentRoute]?.find(
      (candidate) => candidate.target === target,
    )
    if (!entry) return null
    return { level: entry.level, name: entry.name, target, params, path: pathname }
  }

  /**
   * Open an interception over the mounted page.
   *
   * The route, its parameters, and its payload are the ones already on screen —
   * only the overlay and the URL change — so the page underneath keeps its
   * state and its scroll position, and no bundle or payload is fetched.
   */
  function renderIntercept(
    intercept: RouteInterceptState,
    url: URL,
    historyMode: 'push' | 'replace' | 'none',
  ): boolean {
    // The tree is rendered with the *mounted* page's pathname, not the
    // intercepted one. `template.tsx` is keyed on it, so handing the tree a new
    // path would remount every template on the chain — and with them the page
    // the overlay exists to sit on top of. The overlay receives the intercepted
    // URL and its parameters as props instead.
    const rendered: RouteContextValue = { ...snapshot, intercept }
    if (!renderRoute(rendered)) return false
    pushHistoryEntry(url, historyMode)
    // The snapshot follows the address bar, so `usePathname()` outside the
    // route tree and `useSearchParams()` agree with what the user can see.
    snapshot = { ...rendered, pathname: intercept.path }
    search = url.search
    emit()
    return true
  }

  function commitShell(
    context: RouteContextValue,
    url: URL,
    historyMode: 'push' | 'replace' | 'none',
  ): boolean {
    if (!renderShell(context)) return false
    pushHistoryEntry(url, historyMode)
    snapshot = { ...context, flight: undefined }
    search = url.search
    emit()
    return true
  }

  async function renderRouteWithTransition(
    context: RouteContextValue,
    enabled: boolean | undefined,
  ): Promise<boolean> {
    if (!enabled || typeof document === 'undefined') return renderRoute(context)
    const documentWithTransitions = document as Document & {
      startViewTransition?: (update: () => void) => { updateCallbackDone: Promise<void> }
    }
    const reducedMotion =
      typeof window.matchMedia === 'function' &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches
    if (reducedMotion || !documentWithTransitions.startViewTransition) {
      return renderRoute(context)
    }
    const factory = globals.__RUVYXA_ROUTES__?.[context.route]
    const root = globals.__RUVYXA_ROOT__
    if (!factory || !root) return false
    globals.__RUVYXA_ROUTE_PARAMS__ = context.params
    globals.__RUVYXA_REQUEST_PATH__ = context.pathname
    globals.__RUVYXA_ROUTE_PATTERN__ = context.route
    try {
      const transition = documentWithTransitions.startViewTransition(() => {
        root.render(factory(context))
      })
      await transition.updateCallbackDone
      return true
    } catch {
      return false
    }
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
    if (globals.__RUVYXA_ROUTES__?.[context.route]) {
      return (
        !entry.artifactVersion ||
        globals.__RUVYXA_ROUTE_ARTIFACTS__?.[context.route] === entry.artifactVersion
      )
    }
    if (!entry.src) return false
    globals.__RUVYXA_ROUTE_PARAMS__ = context.params
    globals.__RUVYXA_REQUEST_PATH__ = context.pathname
    try {
      await import(/* @vite-ignore */ entry.src)
    } catch {
      return false
    }
    if (
      entry.artifactVersion &&
      globals.__RUVYXA_ROUTE_ARTIFACTS__?.[context.route] !== entry.artifactVersion
    ) {
      return false
    }
    return Boolean(globals.__RUVYXA_ROUTES__?.[context.route])
  }

  /**
   * The cache key for a prefetchable route, with the version that formed it.
   *
   * Returning the version alongside the key is what lets `startFlight` use it
   * without a non-null assertion: the narrowing that proves it is a string
   * happens here, and a bare `string | null` would throw that proof away at the
   * return statement.
   */
  function flightKey(
    entry: RouteManifestEntry,
    pathname: string,
  ): { key: string; artifactVersion: string } | null {
    if (!entry.flight || !entry.artifactVersion) return null
    return { key: `${entry.artifactVersion}:${pathname}`, artifactVersion: entry.artifactVersion }
  }

  function startFlight(entry: RouteManifestEntry, pathname: string): FlightEntry | null {
    const resolved = flightKey(entry, pathname)
    if (!resolved) return null
    const { key, artifactVersion } = resolved
    const cached = flightCache.get(key)
    if (cached) {
      flightCache.delete(key)
      flightCache.set(key, cached)
      return cached
    }

    while (flightCache.size >= FLIGHT_CACHE_LIMIT) {
      const oldest = flightCache.entries().next().value
      if (!oldest) break
      oldest[1].controller.abort()
      flightCache.delete(oldest[0])
    }

    const controller = new AbortController()
    const requestUrl = new URL(FLIGHT_URL, window.location.origin)
    requestUrl.searchParams.set('path', pathname)
    requestUrl.searchParams.set('artifact', artifactVersion)
    const promise = fetch(requestUrl, {
      credentials: 'omit',
      headers: { 'x-ruvyxa-flight': '1' },
      signal: controller.signal,
    })
      .then(async (response) => {
        if (!response.ok) throw new Error(`Flight request failed with status ${response.status}`)
        const declaredLength = Number(response.headers.get('content-length') ?? '0')
        if (declaredLength > FLIGHT_BYTE_LIMIT) throw new Error('Flight payload is too large')
        const payload = await response.text()
        if (new TextEncoder().encode(payload).byteLength > FLIGHT_BYTE_LIMIT) {
          throw new Error('Flight payload is too large')
        }
        return decodeFlight(payload, artifactVersion, pathname)
      })
      .catch((error: unknown) => {
        flightCache.delete(key)
        throw error
      })
    const flight = { controller, promise }
    flightCache.set(key, flight)
    return flight
  }

  /**
   * Fetch one server-components route's Flight payload.
   *
   * `credentials: 'same-origin'` because this is a render, not the public,
   * cacheable payload `startFlight` above asks for: a server component may read
   * `cookies()` exactly as it does on a full document request. The custom
   * header is what keeps that safe — a cross-origin page cannot set it without
   * a preflight, and the server answers none.
   */
  async function fetchRscPayload(pathname: string, signal: AbortSignal): Promise<string> {
    const requestUrl = new URL(RSC_URL, window.location.origin)
    requestUrl.searchParams.set('path', pathname)
    const response = await fetch(requestUrl, {
      credentials: 'same-origin',
      headers: { [RSC_REQUEST_HEADER]: '1' },
      signal,
    })
    if (!response.ok) {
      throw new Error(`Server-components payload request failed with status ${response.status}`)
    }
    return response.text()
  }

  /**
   * Execute a server-components route's bundle so it registers its factory.
   *
   * The same shape as `loadRoute`, against a different registry: the bundle
   * this imports contains the route's `'use client'` modules and the decoder,
   * not a tree it could build on its own.
   */
  async function loadRscRoute(entry: RouteManifestEntry, route: string): Promise<boolean> {
    if (globals.__RUVYXA_RSC_ROUTES__?.[route]) return true
    if (!entry.src) return false
    try {
      await import(/* @vite-ignore */ entry.src)
    } catch {
      return false
    }
    return Boolean(globals.__RUVYXA_RSC_ROUTES__?.[route])
  }

  /**
   * Navigate into a server-components route, or report that it could not be.
   *
   * Reporting rather than throwing: every failure here — no bundle, no payload,
   * a payload the mounted bundle cannot resolve — has the same correct answer,
   * which is the document load the browser would have done without this router.
   * The caller performs it, so history is pushed exactly once.
   *
   * The render is inside the `try` on purpose. A payload naming a client
   * reference this bundle never registered throws during `render`, and that is
   * precisely the stale-deploy case a full load fixes.
   */
  async function navigateServerComponents(
    entry: RouteManifestEntry,
    context: RouteContextValue,
    url: URL,
    historyMode: 'push' | 'replace' | 'none',
    id: number,
  ): Promise<boolean> {
    const root = globals.__RUVYXA_ROOT__
    if (!root) return false

    const controller = new AbortController()
    navigationAbort = controller
    pendingNavigationId = id
    emit()

    let payload: string
    try {
      // Both requests start together: the bundle may already be cached, and the
      // payload does not depend on it.
      const [loaded, fetched] = await Promise.all([
        loadRscRoute(entry, context.route),
        fetchRscPayload(context.pathname, controller.signal),
      ])
      if (id !== navigationId) return true
      if (!loaded) return false
      payload = fetched
    } catch {
      return id !== navigationId
    }

    const factory = globals.__RUVYXA_RSC_ROUTES__?.[context.route]
    if (!factory) return false
    globals.__RUVYXA_ROUTE_PARAMS__ = context.params
    globals.__RUVYXA_REQUEST_PATH__ = context.pathname
    globals.__RUVYXA_ROUTE_PATTERN__ = context.route
    try {
      root.render(factory(payload, context))
    } catch {
      return false
    }

    pushHistoryEntry(url, historyMode)
    snapshot = context
    search = url.search
    pendingNavigationId = null
    emit()
    return true
  }

  /**
   * Open an overlay for this navigation, or report that none applies.
   *
   * Split out of `navigate` for the same reason
   * {@link enterServerComponentsRoute} is: it is a complete way of answering a
   * navigation, with its own success and fall-through, and reading it inline
   * meant reading two of those at once.
   */
  function tryIntercept(
    matched: { route: RouteManifestEntry; params: RouteParams },
    pathname: string,
    url: URL,
    historyMode: 'push' | 'replace' | 'none',
  ): boolean {
    const intercept = matchIntercept(snapshot.route, matched.route.path, matched.params, pathname)
    if (!intercept) return false
    pendingNavigationId = null
    return renderIntercept(intercept, url, historyMode)
  }

  /**
   * Complete a navigation into a server-components route, or fall back.
   *
   * Split from `navigate` so the ordinary path keeps its shape: this branch has
   * its own loading, failure, and scroll handling, and inlining it made one
   * function responsible for two unrelated ways of producing a tree.
   */
  async function enterServerComponentsRoute(
    entry: RouteManifestEntry,
    context: RouteContextValue,
    url: URL,
    historyMode: 'push' | 'replace' | 'none',
    id: number,
    options: NavigateOptions,
  ): Promise<void> {
    const navigated = await navigateServerComponents(entry, context, url, historyMode, id)
    if (id !== navigationId) return
    pendingNavigationId = null
    if (!navigated) {
      emit()
      hardNavigate(url, historyMode)
      return
    }
    if (options.scroll !== false && historyMode !== 'none') window.scrollTo(0, 0)
  }

  /**
   * Record the navigation in session history.
   *
   * `none` writes nothing: `popstate` navigations are already at the URL the
   * browser moved to, and pushing it again would grow the history stack every
   * time the user pressed Back.
   */
  function pushHistoryEntry(url: URL, history: 'push' | 'replace' | 'none'): void {
    if (history === 'push') window.history.pushState({ ruvyxa: true }, '', url.href)
    else if (history === 'replace') window.history.replaceState({ ruvyxa: true }, '', url.href)
  }

  function hardNavigate(url: URL, history: 'push' | 'replace' | 'none'): void {
    if (history === 'replace') window.location.replace(url.href)
    else window.location.assign(url.href)
  }

  async function navigate(
    href: string,
    options: NavigateOptions & { history?: 'push' | 'replace' | 'none' } = {},
  ): Promise<void> {
    const target = resolveNavigationTarget(href)
    if (target.kind === 'refused') return
    if (target.kind === 'external') {
      // The *parsed* value. Replaying `href` here was the whole defect.
      window.location.assign(target.url.href)
      return
    }
    const url = target.url

    const historyMode = options.history ?? (options.replace ? 'replace' : 'push')
    const id = ++navigationId
    navigationAbort?.abort()
    navigationAbort = null
    if (pendingNavigationId !== null) pendingNavigationId = id

    await ensureManifest()
    if (id !== navigationId) return

    const matched = match(url.pathname)
    // No client route owns this URL — it may be an API route, a redirect, or a
    // rewrite the server resolves. Hand it to the browser rather than guess.
    if (!matched) {
      pendingNavigationId = null
      hardNavigate(url, historyMode)
      return
    }

    const pathname = canonicalRoutePath(url.pathname) ?? url.pathname

    // Checked before anything is fetched: an interception renders from the
    // bundle that is already running, so a navigation it answers costs no
    // request at all. A `false` answer means there was no overlay, or the
    // mounted route could not re-render — either way this continues as the
    // ordinary navigation it would have been.
    if (tryIntercept(matched, pathname, url, historyMode)) return

    const context: RouteContextValue = {
      pathname,
      params: matched.params,
      route: matched.route.path,
    }

    // A server-components route has no tree factory to call: its page is not in
    // any browser bundle. It renders from a payload instead, on its own path.
    if (matched.route.serverComponents) {
      await enterServerComponentsRoute(matched.route, context, url, historyMode, id, options)
      return
    }

    const flight = startFlight(matched.route, context.pathname)
    navigationAbort = flight?.controller ?? null

    // Set by whichever branch paints the loading shell. Once it is up the URL
    // has already been committed, so the tail of this function must not push a
    // second history entry for the same navigation.
    let shellPainted = false

    if (needsRouteLoad(matched.route, context.route)) {
      pendingNavigationId = id
      emit()
      // Awaited separately rather than through one `Promise.all` so the shell
      // can go up the moment the bundle is here, instead of after the data it
      // does not need. Both requests are already in flight — `startFlight` ran
      // above — so nothing is serialised by splitting the waits.
      const loaded = await loadRoute(matched.route, context).catch(() => false)
      if (id !== navigationId) return
      if (!loaded) {
        pendingNavigationId = null
        emit()
        hardNavigate(url, historyMode)
        return
      }

      // The bundle loaded, so this navigation is going to happen. Commit the
      // URL and paint the destination's own loading state; the content replaces
      // it below. Committing history here keeps the address bar from lagging
      // behind what the user can see.
      shellPainted = flight ? commitShell(context, url, historyMode) : false

      const settled = await settleFlight(flight)
      if (id !== navigationId) return
      pendingNavigationId = null
      if (!settled.ok) {
        emit()
        // A full load of the same URL. History already points at it when the
        // shell was painted, so `hardNavigate` must not push it a second time.
        hardNavigate(url, shellPainted ? 'none' : historyMode)
        return
      }
      context.flight = settled.value
    } else {
      if (flight) {
        pendingNavigationId = id
        emit()
        // The bundle is already here, so the shell is one render away.
        shellPainted = commitShell(context, url, historyMode)
        const settled = await settleFlight(flight)
        if (id !== navigationId) return
        if (!settled.ok) {
          pendingNavigationId = null
          emit()
          hardNavigate(url, shellPainted ? 'none' : historyMode)
          return
        }
        context.flight = settled.value
      }
      pendingNavigationId = null
    }

    if (!shellPainted) pushHistoryEntry(url, historyMode)

    snapshot = context
    search = url.search
    if (!(await renderRouteWithTransition(context, options.viewTransition))) {
      hardNavigate(url, historyMode)
      return
    }
    emit()

    if (options.scroll !== false && historyMode !== 'none') {
      window.scrollTo(0, 0)
    }
  }

  /**
   * Append one `modulepreload` hint, or report that the document already has it.
   *
   * The guard is per href rather than per route: shared chunks belong to more
   * than one route by definition, so prefetching a second route that reuses a
   * chunk would otherwise append a duplicate hint for a module the document has
   * already asked for. Returns `false` when the hint was already present.
   */
  function preloadModule(src: string): boolean {
    if (document.querySelector(`link[rel="modulepreload"][href="${CSS.escape(src)}"]`)) {
      return false
    }
    const link = document.createElement('link')
    link.rel = 'modulepreload'
    link.href = src
    document.head.append(link)
    return true
  }

  function prefetch(href: string): void {
    // The same guard as `navigate`, from the same function: a href this router
    // will refuse to navigate to is not one it should warm a bundle for, and
    // the rule must not be written down in only one of the two places.
    const target = resolveNavigationTarget(href)
    if (target.kind !== 'internal') return
    const url = target.url
    void ensureManifest().then(() => {
      const matched = match(url.pathname)
      if (!matched?.route.src) return
      const pathname = canonicalRoutePath(url.pathname)
      if (pathname === null) return
      if (matched.route.flight) {
        startFlight(matched.route, pathname)?.promise.catch(() => {})
      }
      if (globals.__RUVYXA_ROUTES__?.[matched.route.path]) return
      // `modulepreload` warms the network and the module graph without
      // executing the bundle, so a prefetch cannot register a tree factory
      // built from the wrong parameters.
      //
      // A route already hinted was hinted together with its shared chunks, so
      // there is nothing left for this call to do.
      if (!preloadModule(matched.route.src)) return
      for (const chunk of matched.route.sharedChunks ?? []) {
        preloadModule(chunk.src)
      }
    })
  }

  /**
   * Re-render the current route from its already-loaded bundle.
   *
   * A registry miss is reported rather than swallowed. This call used to discard
   * `renderRoute`'s result and emit anyway, so a refresh that rendered nothing
   * looked exactly like one that worked.
   */
  function refresh(): void {
    if (!renderRoute(snapshot)) {
      emit()
      throw new Error(
        `Ruvyxa router cannot refresh "${snapshot.route}": its bundle is not registered. ` +
          'Navigate to the route before refreshing it.',
      )
    }
    emit()
  }

  /**
   * Discard the cached server payload for the current route and render it again.
   *
   * Any in-flight request for the same key is aborted before the entry is
   * dropped, so a retry during a slow fetch does not leave a request running
   * whose result nothing will read.
   *
   * A route with no Flight payload has nothing to re-fetch, so this degrades to
   * `refresh()` rather than reporting a failure the caller cannot act on.
   */
  async function retry(): Promise<void> {
    const matched = match(snapshot.pathname)
    const resolved = matched ? flightKey(matched.route, snapshot.pathname) : null
    if (!matched || !resolved) {
      refresh()
      return
    }

    const existing = flightCache.get(resolved.key)
    if (existing) {
      existing.controller.abort()
      flightCache.delete(resolved.key)
    }

    const flight = startFlight(matched.route, snapshot.pathname)
    if (!flight) {
      refresh()
      return
    }

    pendingNavigationId = navigationId
    emit()
    try {
      const value = await flight.promise
      // Re-render against a *new* context object. Mutating `snapshot` in place
      // would leave `getSnapshot()` returning the same reference, and
      // `useSyncExternalStore` would treat the re-render as a no-op.
      snapshot = { ...snapshot, flight: value }
    } finally {
      // The clear and the notification are one act. Clearing alone left
      // `getPending()` answering `false` to a `useSyncExternalStore` that had
      // never been told to ask again, so the `true` React last rendered stood —
      // a spinner that never stopped after the retry failed, which is the
      // moment a user is most likely to press the button again. On the success
      // path `refresh()` emits a second time and nothing re-renders for it: the
      // snapshot reference is unchanged by then, so every subscriber bails out
      // on the read.
      pendingNavigationId = null
      emit()
    }
    refresh()
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
    retry,
    getSearch: () => search,
    getPending: () => pendingNavigationId !== null,
    navigate,
    prefetch,
    refresh,
  }
}

function decodeFlight(payload: string, artifactVersion: string, pathname: string): FlightValue {
  const envelope: unknown = JSON.parse(payload)
  if (!isRecord(envelope)) throw new Error('Flight payload must be an object')
  if (
    envelope.protocol !== FLIGHT_PROTOCOL ||
    envelope.protocolVersion !== FLIGHT_PROTOCOL_VERSION ||
    envelope.manifestVersion !== artifactVersion ||
    envelope.route !== pathname
  ) {
    throw new Error('Flight payload does not match the requested route artifact')
  }
  assertFlightValue(envelope.tree, 0, { count: 0 })
  return envelope.tree as FlightValue
}

function assertFlightValue(value: unknown, depth: number, state: { count: number }): void {
  state.count += 1
  if (state.count > 10_000 || depth > 64) throw new Error('Flight payload exceeds its value limit')
  if (
    value === null ||
    typeof value === 'string' ||
    typeof value === 'boolean' ||
    (typeof value === 'number' && Number.isFinite(value))
  ) {
    return
  }
  if (Array.isArray(value)) {
    for (const child of value) assertFlightValue(child, depth + 1, state)
    return
  }
  if (!isRecord(value)) throw new Error('Flight payload contains an unsupported value')
  for (const [key, child] of Object.entries(value)) {
    if (key === '__proto__' || key === 'prototype' || key === 'constructor') {
      throw new Error('Flight payload contains an unsafe object key')
    }
    assertFlightValue(child, depth + 1, state)
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
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
