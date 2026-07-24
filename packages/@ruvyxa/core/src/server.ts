export interface LoaderContext {
  params: Record<string, string>
  request: Request
  cache: typeof cache
}

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
  /** Retrieve or compute a value. Producer errors are isolated and don't crash the server. */
  get<T>(producer: () => T | Promise<T>): Promise<T>
}

export interface CacheEntry {
  value: unknown
  expiresAt: number
  staleUntil: number
  refreshing: boolean
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
  #pendingWrites = new Map<string, Set<symbol>>()
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
      this.#evictOldest()
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
    if (!keyOrPrefix) {
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

  beginWrite(key: string): symbol {
    const token = Symbol(key)
    const writes = this.#pendingWrites.get(key) ?? new Set<symbol>()
    writes.add(token)
    this.#pendingWrites.set(key, writes)
    return token
  }

  commitWrite(key: string, token: symbol, entry: CacheEntry, expectedEntry?: CacheEntry): boolean {
    if (!this.#pendingWrites.get(key)?.has(token)) return false
    if (expectedEntry && this.#entries.get(key) !== expectedEntry) return false
    this.set(key, entry)
    return true
  }

  finishWrite(key: string, token: symbol): void {
    const writes = this.#pendingWrites.get(key)
    if (!writes) return
    writes.delete(token)
    if (writes.size === 0) this.#pendingWrites.delete(key)
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

  #evictOldest(): void {
    const oldest = this.#accessOrder.shift()
    if (oldest) {
      this.delete(oldest)
    }
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
  let ttlMs = 60_000 // default 60 seconds
  let swrMs = 0 // default: no stale-while-revalidate

  return {
    ttl(value: string) {
      ttlMs = parseTtl(value)
      return this
    },
    swr(value: string) {
      swrMs = parseTtl(value)
      return this
    },
    async get<T>(producer: () => T | Promise<T>): Promise<T> {
      const now = Date.now()
      const cached = cacheStore.get(key)

      // Fresh hit: return immediately
      if (cached && cached.expiresAt > now) {
        return cached.value as T
      }

      // Stale hit with SWR: return stale value and refresh in background
      if (cached && cached.staleUntil > now) {
        if (!cached.refreshing) {
          cached.refreshing = true
          const writeToken = cacheStore.beginWrite(key)
          // Fire-and-forget background refresh. All concurrent stale readers
          // receive the stale value; only the first reader starts the refresh.
          Promise.resolve()
            .then(() => producer())
            .then((value) => {
              const populatedAt = Date.now()
              const committed = cacheStore.commitWrite(
                key,
                writeToken,
                {
                  value,
                  expiresAt: populatedAt + ttlMs,
                  staleUntil: populatedAt + ttlMs + swrMs,
                  refreshing: false,
                },
                cached,
              )
              // A rejected commit leaves the old entry in place. Without
              // clearing its flag the entry claims a refresh is still running
              // and no later reader ever starts another one, so it serves
              // stale until it falls out of the window entirely.
              if (!committed && cacheStore.peek(key) === cached) cached.refreshing = false
            })
            .catch(() => {
              // Producer failed during background refresh — keep serving stale
              if (cacheStore.peek(key) === cached) cached.refreshing = false
            })
            .finally(() => cacheStore.finishWrite(key, writeToken))
        }
        return cached.value as T
      }

      // Miss or fully expired: produce fresh value with error isolation
      const writeToken = cacheStore.beginWrite(key)
      try {
        const value = await producer()
        const populatedAt = Date.now()
        cacheStore.commitWrite(key, writeToken, {
          value,
          expiresAt: populatedAt + ttlMs,
          staleUntil: populatedAt + ttlMs + swrMs,
          refreshing: false,
        })
        return value
      } catch (error) {
        // If we have stale data, return it rather than propagating the error
        if (cached && cacheStore.peek(key) === cached) {
          return cached.value as T
        }
        throw error
      } finally {
        cacheStore.finishWrite(key, writeToken)
      }
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
