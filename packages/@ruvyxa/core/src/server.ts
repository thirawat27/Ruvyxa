export interface LoaderContext {
  params: Record<string, string>
  request: Request
  cache: typeof cache
}

/** Values a Flight route may send to the browser. */
export type FlightValue =
  | null
  | boolean
  | number
  | string
  | readonly FlightValue[]
  | { readonly [key: string]: FlightValue }

/** Public, path-only context passed to a page's optional `flight` export. */
export interface FlightContext {
  path: string
  params: Record<string, string | string[] | undefined>
}

/**
 * A public server-component payload producer.
 *
 * The runtime refuses Flight requests that carry cookies or authorization, so
 * this function must derive its result solely from public route inputs.
 */
export type FlightHandler = (context: FlightContext) => FlightValue | Promise<FlightValue>

export interface ActionContext<TInput> {
  input: TInput
  request: Request
  user?: unknown
  invalidate(key: string): void
}

export type LoaderHandler<TResult> = (ctx: LoaderContext) => TResult | Promise<TResult>

export interface Loader<TResult> {
  (ctx?: Partial<LoaderContext>): Promise<TResult>
  ruvyxa: {
    kind: 'loader'
  }
}

export function loader<TResult>(handler: LoaderHandler<TResult>): Loader<TResult> {
  const callable = async (ctx: Partial<LoaderContext> = {}) => {
    return handler({
      params: ctx.params ?? {},
      request: ctx.request ?? new Request('http://localhost/'),
      cache,
    })
  }

  return Object.assign(callable, {
    ruvyxa: {
      kind: 'loader' as const,
    },
  })
}

export interface Schema<TInput> {
  parse(value: unknown): TInput
}

export interface ActionBuilder<TInput = unknown> {
  input<TNextInput>(schema: Schema<TNextInput>): ActionBuilder<TNextInput>
  /** Publish an action event after a successful invocation. Omit channels to use the route channel. */
  realtime(channels?: string | readonly string[]): ActionBuilder<TInput>
  handler<TResult>(
    handler: (ctx: ActionContext<TInput>) => TResult | Promise<TResult>,
  ): ServerAction<TInput, TResult>
}

export interface ServerAction<TInput, TResult> {
  (input: TInput, ctx?: Partial<ActionContext<TInput>>): Promise<TResult>
  ruvyxa: {
    kind: 'action'
    realtime?: ActionRealtimeOptions
  }
}

export interface ActionRealtimeOptions {
  /** Explicit subscription channels. An empty list resolves to `route:<request pathname>`. */
  channels: readonly string[]
}

export const action: ActionBuilder = createActionBuilder()

function createActionBuilder<TInput>(
  schema?: Schema<TInput>,
  realtimeOptions?: ActionRealtimeOptions,
): ActionBuilder<TInput> {
  return {
    input<TNextInput>(nextSchema: Schema<TNextInput>) {
      return createActionBuilder(nextSchema, realtimeOptions)
    },
    realtime(channels: string | readonly string[] = []) {
      const values = typeof channels === 'string' ? [channels] : [...channels]
      if (values.length > 16) {
        throw new TypeError('action.realtime() accepts at most 16 channels')
      }
      for (const [index, channel] of values.entries()) {
        if (typeof channel !== 'string' || !/^[A-Za-z0-9:._/-]{1,128}$/.test(channel.trim())) {
          throw new TypeError(
            `action.realtime() channels[${index}] must use 1-128 letters, digits, colon, dot, underscore, slash, or dash`,
          )
        }
      }
      return createActionBuilder(schema, {
        channels: Object.freeze([...new Set(values.map((channel) => channel.trim()))]),
      })
    },
    handler<TResult>(handler: (ctx: ActionContext<TInput>) => TResult | Promise<TResult>) {
      const callable = async (rawInput: TInput, ctx: Partial<ActionContext<TInput>> = {}) => {
        const input = schema ? schema.parse(rawInput) : rawInput
        return handler({
          input,
          request: ctx.request ?? new Request('http://localhost/'),
          user: ctx.user,
          invalidate: ctx.invalidate ?? (() => {}),
        })
      }

      return Object.assign(callable, {
        ruvyxa: {
          kind: 'action' as const,
          ...(realtimeOptions ? { realtime: realtimeOptions } : {}),
        },
      })
    },
  }
}

// --- Production-grade Cache ---
// LRU-bounded, stale-while-revalidate, error-isolated cache store.
// Prevents unbounded memory growth in long-running production servers.

export interface CacheBuilder {
  /** Set time-to-live (e.g. "30s", "5m", "1h", "1d"). Default: 60s. */
  ttl(value: string): CacheBuilder
  /** Set stale-while-revalidate window (serves stale data while refreshing in background). */
  swr(value: string): CacheBuilder
  /** Attach invalidation tags. Tags are deployment-local and bounded. */
  tags(...values: string[]): CacheBuilder
  /** Keep the value only for this request instead of sharing it across requests. */
  scope(value: 'deployment' | 'request'): CacheBuilder
  /** Retrieve or compute a value. Producer errors are isolated and don't crash the server. */
  get<T>(producer: () => T | Promise<T>): Promise<T>
}

export interface CacheEntry {
  value: unknown
  expiresAt: number
  staleUntil: number
  refreshing: boolean
  tags: readonly string[]
}

interface PendingCacheWrite {
  token: symbol
  promise: Promise<unknown>
}

/** Maximum cache entries before LRU eviction kicks in. */
const CACHE_MAX_ENTRIES = 1024

/**
 * Production in-memory TTL cache with LRU eviction and stale-while-revalidate.
 *
 * Features:
 * - Bounded to CACHE_MAX_ENTRIES to prevent memory leaks
 * - Stale-while-revalidate: serves expired data while refreshing in background
 * - Error isolation: producer failures return stale data when available
 * - Periodic cleanup of fully expired entries
 */
class CacheStore {
  #entries = new Map<string, CacheEntry>()
  #accessOrder: string[] = []
  #pendingWrites = new Map<string, PendingCacheWrite>()
  #maxEntries: number

  constructor(maxEntries = CACHE_MAX_ENTRIES) {
    this.#maxEntries = maxEntries
  }

  get(key: string): CacheEntry | undefined {
    const entry = this.#entries.get(key)
    if (entry) {
      // Move to end of access order (most recently used)
      this.#touchAccessOrder(key)
    }
    return entry
  }

  peek(key: string): CacheEntry | undefined {
    return this.#entries.get(key)
  }

  set(key: string, entry: CacheEntry): void {
    // Updating an existing key does not increase the cache size. Evicting before
    // that check would discard an unrelated LRU entry on every refresh at capacity.
    while (!this.#entries.has(key) && this.#entries.size >= this.#maxEntries) {
      if (!this.#evictOldest()) {
        // The access order is internal bookkeeping that every write path keeps
        // in step with `#entries`. If a future change ever breaks that, this
        // loop would spin forever inside a request rather than fail — so an
        // eviction that frees nothing rebuilds the order from the entries that
        // actually exist and tries once more. Same recovery, and same reason,
        // as `RenderCache::put` in `crates/ruvyxa_dev_server/src/render_cache.rs`.
        this.#accessOrder = [...this.#entries.keys()]
        if (!this.#evictOldest()) break
      }
    }

    this.#entries.set(key, entry)
    this.#touchAccessOrder(key)
  }

  delete(key: string): boolean {
    this.#accessOrder = this.#accessOrder.filter((k) => k !== key)
    this.#pendingWrites.delete(key)
    return this.#entries.delete(key)
  }

  clear(): void {
    this.#entries.clear()
    this.#accessOrder = []
    this.#pendingWrites.clear()
  }

  invalidate(keyOrPrefix?: string): void {
    if (keyOrPrefix === undefined) {
      this.clear()
      return
    }

    const keys = new Set([...this.#entries.keys(), ...this.#pendingWrites.keys()])
    for (const key of keys) {
      if (key === keyOrPrefix || key.startsWith(keyOrPrefix + ':')) {
        this.delete(key)
      }
    }
  }

  invalidateTag(tag: string): void {
    for (const [key, entry] of this.#entries) {
      if (entry.tags.includes(tag)) this.delete(key)
    }
  }

  runSingleFlight<T>(
    key: string,
    producer: (token: symbol) => T | Promise<T>,
  ): { promise: Promise<T>; started: boolean } {
    const pending = this.#pendingWrites.get(key)
    if (pending) {
      return { promise: pending.promise as Promise<T>, started: false }
    }

    const token = Symbol(key)
    const promise = Promise.resolve()
      .then(() => producer(token))
      .finally(() => this.finishWrite(key, token))
    this.#pendingWrites.set(key, { token, promise })
    return { promise, started: true }
  }

  commitWrite(key: string, token: symbol, entry: CacheEntry, expectedEntry?: CacheEntry): boolean {
    if (this.#pendingWrites.get(key)?.token !== token) return false
    if (expectedEntry && this.#entries.get(key) !== expectedEntry) return false
    this.set(key, entry)
    return true
  }

  finishWrite(key: string, token: symbol): void {
    if (this.#pendingWrites.get(key)?.token === token) {
      this.#pendingWrites.delete(key)
    }
  }

  /** Remove all entries that have fully expired (past staleUntil). */
  prune(): number {
    const now = Date.now()
    let pruned = 0
    for (const [key, entry] of this.#entries) {
      if (entry.staleUntil < now) {
        this.delete(key)
        pruned++
      }
    }
    if (pruned > 0) {
      this.#accessOrder = this.#accessOrder.filter((k) => this.#entries.has(k))
    }
    return pruned
  }

  get size(): number {
    return this.#entries.size
  }

  #touchAccessOrder(key: string): void {
    const idx = this.#accessOrder.indexOf(key)
    if (idx !== -1) {
      this.#accessOrder.splice(idx, 1)
    }
    this.#accessOrder.push(key)
  }

  /** Evict the least recently used entry. `false` when nothing was freed. */
  #evictOldest(): boolean {
    const oldest = this.#accessOrder.shift()
    if (oldest === undefined) return false
    return this.delete(oldest)
  }
}

const cacheStore = new CacheStore()

// Periodic cleanup every 60s to reclaim memory from fully expired entries
let cleanupTimer: ReturnType<typeof setInterval> | undefined
if (typeof setInterval !== 'undefined') {
  cleanupTimer = setInterval(() => cacheStore.prune(), 60_000)
  // Don't hold the process open
  if (cleanupTimer && typeof cleanupTimer === 'object' && 'unref' in cleanupTimer) {
    ;(cleanupTimer as { unref(): void }).unref()
  }
}

function parseTtl(value: string): number {
  const match = value.match(/^(\d+)\s*(ms|s|m|h|d)$/)
  if (!match) {
    throw invalidCacheDuration(value)
  }
  const amount = Number(match[1])
  if (!Number.isSafeInteger(amount) || amount <= 0) {
    throw invalidCacheDuration(value)
  }

  const multiplier = (() => {
    switch (match[2]) {
      case 'ms':
        return 1
      case 's':
        return 1000
      case 'm':
        return 60_000
      case 'h':
        return 3_600_000
      case 'd':
        return 86_400_000
      default: {
        throw new Error(`Unsupported cache duration unit: ${match[2]}`)
      }
    }
  })()
  const duration = amount * multiplier
  if (!Number.isSafeInteger(duration)) {
    throw invalidCacheDuration(value)
  }
  return duration
}

function invalidCacheDuration(value: string): Error {
  return new Error(
    `Invalid cache duration "${value}". Use a positive value within JavaScript's safe integer range, such as "30s", "5m", "1h", or "1d".`,
  )
}

function validateCacheTag(tag: string): string {
  if (typeof tag !== 'string' || !/^[A-Za-z0-9:._/-]{1,128}$/.test(tag)) {
    throw new TypeError(
      'cache tag must use 1-128 letters, digits, colon, dot, underscore, slash, or dash',
    )
  }
  return tag
}

function assertSharedCachePrivacy(): void {
  if (host()?.wasRead?.()) {
    throw new Error(
      "RUV1840 shared cache producer read request state; use cache().scope('request') or pass an explicit safe partition key",
    )
  }
}

function assertCacheSerializable(value: unknown): void {
  const ancestors = new Set<object>()
  const visit = (current: unknown, depth: number): void => {
    if (depth > 64) throw new TypeError('RUV1841 cache value nesting exceeds 64 levels')
    if (
      current === null ||
      typeof current === 'string' ||
      typeof current === 'boolean' ||
      (typeof current === 'number' && Number.isFinite(current))
    ) {
      return
    }
    if (typeof current !== 'object') {
      throw new TypeError(`RUV1841 cache cannot serialize ${typeof current}`)
    }
    if (ancestors.has(current)) throw new TypeError('RUV1841 cache cannot serialize cyclic values')
    const prototype = Object.getPrototypeOf(current)
    if (!Array.isArray(current) && prototype !== Object.prototype && prototype !== null) {
      throw new TypeError('RUV1841 cache accepts only arrays and plain objects')
    }
    ancestors.add(current)
    for (const child of Array.isArray(current) ? current : Object.values(current)) {
      visit(child, depth + 1)
    }
    ancestors.delete(current)
  }
  visit(value, 0)
}

/**
 * Create a cache builder for the given key.
 *
 * Usage:
 * ```ts
 * const data = await cache("users:list").ttl("5m").swr("1m").get(async () => {
 *   return db.users.findMany()
 * })
 * ```
 */
export function cache(key: string): CacheBuilder {
  if (typeof key !== 'string' || key.length > 8192) {
    throw new TypeError('cache() key must contain at most 8192 characters')
  }
  let ttlMs = 60_000 // default 60 seconds
  let swrMs = 0 // default: no stale-while-revalidate
  let tags: string[] = []
  let scope: 'deployment' | 'request' = 'deployment'

  return {
    ttl(value: string) {
      ttlMs = parseTtl(value)
      return this
    },
    swr(value: string) {
      swrMs = parseTtl(value)
      return this
    },
    tags(...values: string[]) {
      if (values.length > 32) throw new TypeError('cache().tags() accepts at most 32 tags')
      tags = [...new Set(values.map(validateCacheTag))].sort()
      return this
    },
    scope(value: 'deployment' | 'request') {
      if (value !== 'deployment' && value !== 'request') {
        throw new TypeError('cache().scope() must be "deployment" or "request"')
      }
      scope = value
      return this
    },
    async get<T>(producer: () => T | Promise<T>): Promise<T> {
      if (scope === 'request') {
        const context = host()?.peek?.()
        if (!context) throw new Error('request-scoped cache used outside a request')
        context.cache ??= new Map<string, unknown>()
        if (context.cache.has(key)) return context.cache.get(key) as T
        const value = await producer()
        assertCacheSerializable(value)
        context.cache.set(key, value)
        return value
      }
      const now = Date.now()
      const cached = cacheStore.get(key)

      // Fresh hit: return immediately
      if (cached && cached.expiresAt > now) {
        return cached.value as T
      }

      // Stale hit with SWR: return stale value and refresh in background
      if (cached && cached.staleUntil > now) {
        if (!cached.refreshing) {
          // Fire-and-forget background refresh. All concurrent stale readers
          // receive the stale value; only the first reader starts the refresh.
          const refresh = cacheStore.runSingleFlight(key, async (writeToken) => {
            try {
              const value = await producer()
              assertSharedCachePrivacy()
              assertCacheSerializable(value)
              const populatedAt = Date.now()
              const committed = cacheStore.commitWrite(
                key,
                writeToken,
                {
                  value,
                  expiresAt: populatedAt + ttlMs,
                  staleUntil: populatedAt + ttlMs + swrMs,
                  refreshing: false,
                  tags,
                },
                cached,
              )
              // A rejected commit leaves the old entry in place. Without
              // clearing its flag the entry claims a refresh is still running
              // and no later reader ever starts another one, so it serves
              // stale until it falls out of the window entirely.
              if (!committed && cacheStore.peek(key) === cached) cached.refreshing = false
            } catch {
              // Producer failed during background refresh — keep serving stale
              if (cacheStore.peek(key) === cached) cached.refreshing = false
            }
          })
          if (refresh.started) cached.refreshing = true
          // The task catches producer failures itself so this is only a guard
          // against a future bookkeeping regression becoming unhandled work.
          void refresh.promise.catch(() => {})
        }
        return cached.value as T
      }

      // Miss or fully expired: produce fresh value with error isolation
      const pending = cacheStore.runSingleFlight<T>(key, async (writeToken) => {
        try {
          const value = await producer()
          assertSharedCachePrivacy()
          assertCacheSerializable(value)
          const populatedAt = Date.now()
          cacheStore.commitWrite(key, writeToken, {
            value,
            expiresAt: populatedAt + ttlMs,
            staleUntil: populatedAt + ttlMs + swrMs,
            refreshing: false,
            tags,
          })
          return value
        } catch (error) {
          // If we have stale data, return it rather than propagating the error
          if (cached && cacheStore.peek(key) === cached) {
            return cached.value as T
          }
          throw error
        }
      })
      return pending.promise
    },
  }
}

/**
 * Invalidate a specific cache key, all keys matching a prefix, or the entire cache.
 *
 * @param keyOrPrefix - If omitted, clears the entire cache. If provided, clears the
 *   exact key and any keys that start with `keyOrPrefix:`.
 */
export function invalidateCache(keyOrPrefix?: string): void {
  cacheStore.invalidate(keyOrPrefix)
}

/** Invalidate deployment-cache entries carrying one exact tag. */
export function revalidateTag(tag: string): void {
  cacheStore.invalidateTag(validateCacheTag(tag))
}

/**
 * Get current cache statistics for observability.
 */
export function cacheStats(): { size: number; maxEntries: number } {
  return { size: cacheStore.size, maxEntries: CACHE_MAX_ENTRIES }
}

export function redirect(location: string, status = 302): Response {
  if (status < 300 || status > 399) {
    throw new Error(`redirect() status must be 3xx, got ${status}`)
  }
  return new Response(null, {
    status,
    headers: {
      Location: location,
    },
  })
}

export function notFound(message = 'Not found'): Response {
  return new Response(message, { status: 404 })
}

export function json(data: unknown, init?: ResponseInit): Response {
  return Response.json(data, init)
}

/**
 * Ambient access to the request being served.
 *
 * A page component is called by the renderer, not by the router, so it has no
 * parameter through which a `Request` could reach it. `cookies()`, `headers()`,
 * and `draftMode()` close that gap the way Next.js does: the host installs a
 * per-request store before rendering, and these read it.
 *
 * ## Why the store lives on `globalThis`
 *
 * The host that installs the store (`packages/ruvyxa/runtime/*.mjs`) and the
 * page that reads it are compiled separately and may each end up with their own
 * copy of this module — the SSR bundle aliases `ruvyxa/server` to the workspace
 * source, while a dependency importing it resolves `dist`. A module-level
 * variable would be per-copy, so a page would read a store the host never set.
 * A well-known key on `globalThis` is the one thing both copies agree on. The
 * same reasoning already governs `__RUVYXA_ROUTE_CONTEXT__`.
 *
 * ## Why the store is not created here
 *
 * Isolating concurrent renders needs `AsyncLocalStorage`, and importing
 * `node:async_hooks` from this module would put a Node built-in in every edge
 * and browser bundle that touches `@ruvyxa/core/server`. The host owns that
 * import and installs an implementation; this module only reads one.
 */

/** One request's data, as the host provides it. */
export interface RequestContext {
  /** Request headers in wire order, so repeated names survive. */
  headers: readonly (readonly [string, string])[]
  /** Request method, uppercased. */
  method: string
  /** Path and query of the request target. */
  url: string
  /** Whether draft mode is enabled for this request. */
  draft: boolean
  /** URLs {@link revalidatePath} has queued for the host to refresh. */
  revalidate?: Set<string>
  /**
   * `Set-Cookie` values a server action or API route has queued.
   *
   * Absent during page rendering: the response headers are already being
   * written by the time a page renders, so a cookie set there would be
   * silently dropped. `cookies().set()` reports that rather than pretending.
   */
  setCookies?: string[]
  /** Values isolated to this request by `cache().scope('request')`. */
  cache?: Map<string, unknown>
}

/** The seam a host installs on `globalThis`. */
export interface RequestContextHost {
  /** The context for the request being served on this call stack, if any. */
  current(): RequestContext | null
  /**
   * The context without recording a read.
   *
   * `current()` marks the render as depending on this request, which is what
   * keeps a personalised page out of a shared cache. `revalidatePath()` needs
   * the context but is not a read of request state, so it uses this instead —
   * otherwise queuing a revalidation would quietly make the calling page
   * uncacheable. Optional so an older host still works, with `current()` as the
   * fallback.
   */
  peek?(): RequestContext | null
  /** Whether this request has read cookies, headers, or draft state. */
  wasRead?(): boolean
}

const CONTEXT_KEY = '__RUVYXA_REQUEST_CONTEXT__'

function host(): RequestContextHost | null {
  return (globalThis as Record<string, unknown>)[CONTEXT_KEY] as RequestContextHost | null
}

/**
 * Install the per-request store. Called by Ruvyxa's runtime hosts, not by
 * applications.
 */
export function installRequestContextHost(implementation: RequestContextHost): void {
  ;(globalThis as Record<string, unknown>)[CONTEXT_KEY] = implementation
}

/**
 * The active request, or an error naming the accessor that needed one.
 *
 * Deliberately not named `require`: a local function by that name is rewritten
 * to a module load by bundlers targeting CommonJS, which turned every
 * `cookies()` call into an import of a package called `cookies()`.
 */
function activeRequest(api: string): RequestContext {
  const context = host()?.current()
  if (!context) {
    throw new Error(
      `${api} was called outside a request.\n\n` +
        'It is available while a page, API route, or server action is being served. ' +
        'Calling it at module scope runs at import time, when there is no request to read — ' +
        'move the call inside the component or handler.',
    )
  }
  return context
}

/** Read-only view of one request's cookies. */
export interface RequestCookies {
  get(name: string): string | undefined
  has(name: string): boolean
  /** Every cookie on the request, in the order the header listed them. */
  getAll(): { name: string; value: string }[]
}

/**
 * Cookies sent with the request being served.
 *
 * Reading a cookie makes a page's output depend on who is asking, so a route
 * that calls this is served per request and is never stored in a shared render
 * cache. See `route_reads_request_state` in `crates/ruvyxa_graph/src/lib.rs`.
 *
 * @example
 * ```tsx
 * export default function Page() {
 *   const theme = cookies().get('theme') ?? 'light'
 *   return <main data-theme={theme} />
 * }
 * ```
 */
export function cookies(): RequestCookies {
  const context = activeRequest('cookies()')
  const parsed = parseCookieHeader(headerValue(context, 'cookie'))
  return {
    get: (name) => parsed.find((entry) => entry.name === name)?.value,
    has: (name) => parsed.some((entry) => entry.name === name),
    getAll: () => parsed.map((entry) => ({ ...entry })),
  }
}

/**
 * Headers sent with the request being served.
 *
 * Returns a standard read-only `Headers`, so `get`, `has`, `getSetCookie`, and
 * iteration all behave as they do on a `Request`.
 */
export function headers(): Headers {
  const context = activeRequest('headers()')
  const collected = new Headers()
  for (const [name, value] of context.headers) collected.append(name, value)
  return collected
}

/** Draft mode state for the request being served. */
export interface DraftMode {
  /** Whether this request is in draft mode. */
  readonly isEnabled: boolean
}

/**
 * Whether the request is previewing unpublished content.
 *
 * Enabled by the `__ruvyxa_draft` cookie, which an API route sets after
 * checking whatever secret the CMS shares with the application. A request in
 * draft mode is never served from a static or incrementally regenerated cache,
 * for the same reason a request that reads cookies is not.
 */
export function draftMode(): DraftMode {
  const context = activeRequest('draftMode()')
  return { isEnabled: context.draft }
}

/**
 * Ask the server to re-render one URL on its next request.
 *
 * The URL is a concrete path (`/blog/hello`), not a route pattern. It reaches
 * the server with the handler's response, so the invalidation and the write
 * that caused it land together: a client that follows a successful action with
 * a navigation cannot arrive before the cache has been cleared.
 *
 * Every rendering strategy is covered. For SSR and CSR the cached document is
 * dropped; for SSG, ISR, and PPR the next request additionally bypasses the
 * HTML the build wrote to disk, which is the document that would otherwise keep
 * being served.
 *
 * This invalidates documents by URL. Use `revalidateTag()` instead when the
 * cached function was labelled through `cache(...).tags(...)`; tag
 * invalidation deliberately does not imply a route/document invalidation.
 *
 * @example
 * ```ts
 * export const POST = async ({ request }: { request: Request }) => {
 *   const { slug } = await request.json()
 *   revalidatePath(`/blog/${slug}`)
 *   return new Response(null, { status: 204 })
 * }
 * ```
 *
 * One request may queue at most 64 distinct paths. Each path is limited to
 * 2,048 characters; exceeding either bound throws instead of dropping work.
 */
const MAX_REVALIDATIONS_PER_REQUEST = 64
const MAX_REVALIDATION_PATH_LENGTH = 2_048

export function revalidatePath(path: string): void {
  if (!path.startsWith('/') || path.length > MAX_REVALIDATION_PATH_LENGTH) {
    throw new Error(
      `revalidatePath() needs an absolute path of at most ${MAX_REVALIDATION_PATH_LENGTH} characters, got ${JSON.stringify(path)}.\n\n` +
        'Pass the URL a visitor would request, such as "/blog/hello" — not a route ' +
        'pattern like "/blog/[slug]" and not a relative path.',
    )
  }
  const host = (globalThis as Record<string, unknown>)[CONTEXT_KEY] as
    RequestContextHost | undefined
  const context = host?.peek?.() ?? host?.current() ?? null
  if (!context) {
    throw new Error(
      'revalidatePath() was called outside a request.\n\n' +
        'It queues work onto the response of the API route or server action that ' +
        'calls it, so there has to be one. Calling it at module scope runs at ' +
        'import time, when there is no response to attach it to.',
    )
  }
  if (
    context.revalidate &&
    !context.revalidate.has(path) &&
    context.revalidate.size >= MAX_REVALIDATIONS_PER_REQUEST
  ) {
    throw new Error(
      `revalidatePath() accepts at most ${MAX_REVALIDATIONS_PER_REQUEST} distinct paths in one request. ` +
        'Split larger invalidations across requests so the host can apply every path.',
    )
  }
  context.revalidate?.add(path)
}

/** Cookie name that turns draft mode on. Shared with the Rust request path. */
export const DRAFT_MODE_COOKIE = '__ruvyxa_draft'

function headerValue(context: RequestContext, name: string): string {
  const lowered = name.toLowerCase()
  const values = context.headers
    .filter(([header]) => header.toLowerCase() === lowered)
    .map(([, value]) => value)
  return values.join('; ')
}

/**
 * Split a `Cookie` header into name/value pairs.
 *
 * Deliberately tolerant: a malformed pair is skipped rather than throwing,
 * because the header is attacker-controlled and a page must not fail to render
 * because a browser extension wrote something odd. A value is returned exactly
 * as sent apart from surrounding whitespace and one layer of double quotes —
 * percent-decoding is the application's choice, since not every cookie is
 * percent-encoded and decoding one that is not can throw.
 */
export function parseCookieHeader(header: string): { name: string; value: string }[] {
  const entries: { name: string; value: string }[] = []
  for (const part of header.split(';')) {
    const separator = part.indexOf('=')
    if (separator <= 0) continue
    const name = part.slice(0, separator).trim()
    if (!name) continue
    let value = part.slice(separator + 1).trim()
    if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
      value = value.slice(1, -1)
    }
    entries.push({ name, value })
  }
  return entries
}
