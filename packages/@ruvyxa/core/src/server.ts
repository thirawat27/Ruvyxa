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
  handler<TResult>(
    handler: (ctx: ActionContext<TInput>) => TResult | Promise<TResult>,
  ): ServerAction<TInput, TResult>
}

export interface ServerAction<TInput, TResult> {
  (input: TInput, ctx?: Partial<ActionContext<TInput>>): Promise<TResult>
  ruvyxa: {
    kind: 'action'
  }
}

export const action: ActionBuilder = createActionBuilder()

function createActionBuilder<TInput>(schema?: Schema<TInput>): ActionBuilder<TInput> {
  return {
    input<TNextInput>(nextSchema: Schema<TNextInput>) {
      return createActionBuilder(nextSchema)
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

  set(key: string, entry: CacheEntry): void {
    // Evict LRU entries if over capacity
    while (this.#entries.size >= this.#maxEntries) {
      this.#evictOldest()
    }

    this.#entries.set(key, entry)
    this.#touchAccessOrder(key)
  }

  delete(key: string): boolean {
    this.#accessOrder = this.#accessOrder.filter((k) => k !== key)
    return this.#entries.delete(key)
  }

  clear(): void {
    this.#entries.clear()
    this.#accessOrder = []
  }

  /** Remove all entries that have fully expired (past staleUntil). */
  prune(): number {
    const now = Date.now()
    let pruned = 0
    for (const [key, entry] of this.#entries) {
      if (entry.staleUntil < now) {
        this.#entries.delete(key)
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

  /** Iterate all keys (for prefix invalidation). */
  keys(): IterableIterator<string> {
    return this.#entries.keys()
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
      this.#entries.delete(oldest)
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
    throw new Error(
      `Invalid cache duration "${value}". Use a positive value such as "30s", "5m", "1h", or "1d".`,
    )
  }
  const amount = parseInt(match[1], 10)
  switch (match[2]) {
    case 'ms':
      return amount
    case 's':
      return amount * 1000
    case 'm':
      return amount * 60_000
    case 'h':
      return amount * 3_600_000
    case 'd':
      return amount * 86_400_000
    default: {
      throw new Error(`Unsupported cache duration unit: ${match[2]}`)
    }
  }
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
      if (cached && cached.staleUntil > now && !cached.refreshing) {
        cached.refreshing = true
        // Fire-and-forget background refresh
        Promise.resolve()
          .then(() => producer())
          .then((value) => {
            cacheStore.set(key, {
              value,
              expiresAt: Date.now() + ttlMs,
              staleUntil: Date.now() + ttlMs + swrMs,
              refreshing: false,
            })
          })
          .catch(() => {
            // Producer failed during background refresh — keep serving stale
            cached.refreshing = false
          })
        return cached.value as T
      }

      // Miss or fully expired: produce fresh value with error isolation
      try {
        const value = await producer()
        cacheStore.set(key, {
          value,
          expiresAt: now + ttlMs,
          staleUntil: now + ttlMs + swrMs,
          refreshing: false,
        })
        return value
      } catch (error) {
        // If we have stale data, return it rather than propagating the error
        if (cached) {
          return cached.value as T
        }
        throw error
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
  if (!keyOrPrefix) {
    cacheStore.clear()
    return
  }
  for (const key of cacheStore.keys()) {
    if (key === keyOrPrefix || key.startsWith(keyOrPrefix + ':')) {
      cacheStore.delete(key)
    }
  }
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
