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
  /** Attach invalidation tags. Tags are process-local and bounded. */
  tags(...values: string[]): CacheBuilder
  /**
   * How widely a value is shared.
   *
   * `'request'` keeps it for the current request; use it when the producer
   * reads cookies, headers, or draft mode, so one visitor's data cannot be
   * served to another.
   *
   * `'deployment'` — the default — shares it with every request handled by the
   * same process. Not with the deployment, and not even with the server: a
   * Ruvyxa server is a pool of render workers sized to the host, so one machine
   * already holds several copies, and a second instance, a second serverless
   * container, and every cold start each add their own. The name says how long
   * a value may be considered valid, not how many processes can see it. Put
   * anything that must be identical everywhere in a real store and cache the
   * read.
   */
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
  /**
   * The tags the value being produced will carry.
   *
   * Recorded when the flight starts rather than read off the entry, because a
   * cold key has no entry yet — which is exactly the window in which a
   * `revalidateTag()` has nothing to match and the producer that started before
   * it still commits.
   */
  tags: readonly string[]
}

/**
 * Entries the in-memory tier holds before LRU eviction, when nothing says
 * otherwise.
 *
 * A default rather than a constant. `cache.maxEntries` overrides it, and `0`
 * turns the tier off entirely — which is what a deployment running several
 * instances behind a shared store wants, because a per-instance copy in front
 * of a shared one is the thing that makes two instances disagree. Next.js
 * spells the same decision `cacheMaxMemorySize`, and `0` means the same there.
 *
 * The unit is entries, not bytes, because this store counts entries: it has no
 * size accounting to answer a byte budget with, and inventing one that
 * estimated would be a budget nobody could rely on.
 */
const CACHE_MAX_ENTRIES = 1024

/**
 * The entry bound in force right now.
 *
 * Read per write rather than captured at construction. `cacheStore` is created
 * when this module is evaluated, and the route registry installs the
 * configuration afterwards — a captured value would always be the default. One
 * property read on a path that is already doing a `Map` write.
 *
 * An unusable value is the default, reported once: a bound this process cannot
 * make sense of must not silently become "no cache" or "unbounded", which are
 * the two failure directions that hurt.
 */
/**
 * Bytes the in-memory tier holds before eviction, when nothing says otherwise.
 *
 * The entry bound alone is not a memory bound: 1024 entries of ten megabytes is
 * ten gigabytes, and nothing stopped it. Next.js budgets this in bytes for that
 * reason and defaults to fifty megabytes; this matches the number so a project
 * moving between them gets the memory profile it had.
 *
 * Measured with `JSON.stringify(value).length`, which is an approximation and
 * is the right one available: every cached value has already been through
 * `assertCacheSerializable`, so it is always measurable this way, and a
 * measurement that is within a small factor beats a bound that does not exist.
 * `0` disables the byte budget and leaves the entry bound in charge.
 */
const CACHE_MAX_BYTES = 52_428_800

let reportedBadBound = false
function maxCacheEntries(): number {
  const configured = dataCacheConfig()?.maxEntries
  if (configured === undefined) return CACHE_MAX_ENTRIES
  if (!Number.isInteger(configured) || configured < 0) {
    if (!reportedBadBound) {
      reportedBadBound = true
      console.error(
        `[ruvyxa] cache.maxEntries must be a whole number of entries, got ${JSON.stringify(configured)}; ` +
          `using ${CACHE_MAX_ENTRIES}.`,
      )
    }
    return CACHE_MAX_ENTRIES
  }
  return configured
}

let reportedBadByteBound = false
function maxCacheBytes(): number {
  const configured = dataCacheConfig()?.maxBytes
  if (configured === undefined) return CACHE_MAX_BYTES
  if (!Number.isInteger(configured) || configured < 0) {
    if (!reportedBadByteBound) {
      reportedBadByteBound = true
      console.error(
        `[ruvyxa] cache.maxBytes must be a whole number of bytes, got ${JSON.stringify(configured)}; ` +
          `using ${CACHE_MAX_BYTES}.`,
      )
    }
    return CACHE_MAX_BYTES
  }
  return configured
}

/**
 * Roughly how much memory one entry's value holds.
 *
 * `JSON.stringify` rather than a structural walk: the value has already passed
 * `assertCacheSerializable`, so this always succeeds, and the cost is paid once
 * per write rather than once per read. A value that somehow refuses to
 * stringify is charged nothing rather than crashing a cache write — the entry
 * bound still covers it.
 */
function weigh(value: unknown): number {
  try {
    return JSON.stringify(value)?.length ?? 0
  } catch {
    return 0
  }
}

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
  /**
   * Doubles as the recency order: a `Map` iterates in insertion order, so
   * re-inserting a key on read or write moves it to the most-recent end and
   * `keys().next()` is the least-recent entry.
   *
   * A parallel `string[]` used to carry that order. Keeping the two in step
   * cost an `indexOf`/`splice` on every read and a full `filter` on every
   * delete, which made `invalidateTag` — a delete per matching entry —
   * quadratic in cache size. It also made desynchronization possible at all,
   * which is what the eviction path had to defend against. One structure
   * cannot drift from itself.
   */
  readonly #entries = new Map<string, CacheEntry>()
  /**
   * Bytes charged to each live key, and their sum.
   *
   * A parallel map rather than a field on `CacheEntry`, which is exported and
   * whose shape callers construct. Every site that adds to or removes from
   * `#entries` updates both, and `prune()` recomputes the sum from this map
   * afterwards — incremental accounting drifts, and a byte budget that has
   * drifted is a budget that either evicts nothing or evicts everything.
   */
  readonly #weights = new Map<string, number>()
  #bytes = 0
  readonly #pendingWrites = new Map<string, PendingCacheWrite>()
  /** `undefined` means "ask the configuration"; a number pins it, for tests. */
  readonly #maxEntries: number | undefined

  constructor(maxEntries?: number) {
    this.#maxEntries = maxEntries
  }

  get maxEntries(): number {
    return this.#maxEntries ?? maxCacheEntries()
  }

  get(key: string): CacheEntry | undefined {
    const entry = this.#entries.get(key)
    if (entry) {
      // Move to end of access order (most recently used)
      this.#entries.delete(key)
      this.#entries.set(key, entry)
    }
    return entry
  }

  peek(key: string): CacheEntry | undefined {
    return this.#entries.get(key)
  }

  set(key: string, entry: CacheEntry): void {
    const bound = this.maxEntries
    // Zero is off, not "hold one". The loop below cannot evict from an empty
    // map, so a bound of zero used to store the first entry and then thrash —
    // a cache that is neither on nor off. A deployment that turns the local
    // tier off is asking for every read to reach the shared store, and getting
    // one entry ahead of it is the disagreement it was turning the tier off to
    // avoid.
    if (bound === 0) {
      this.#forget(key)
      return
    }
    // Updating an existing key does not increase the cache size. Evicting before
    // that check would discard an unrelated LRU entry on every refresh at capacity.
    while (!this.#entries.has(key) && this.#entries.size >= bound) {
      // Only an empty map frees nothing, and the loop condition cannot hold
      // once it is empty for any `maxEntries >= 1`. The guard covers a store
      // constructed with a capacity of zero rather than a desynchronized
      // order, which is no longer representable.
      if (!this.#evictOldest()) break
    }

    // Delete first so a rewrite of an existing key is re-inserted at the
    // most-recent end instead of keeping its original position.
    this.#forget(key)
    const weight = weigh(entry.value)
    this.#entries.set(key, entry)
    this.#weights.set(key, weight)
    this.#bytes += weight

    // Then evict until the byte budget holds. Separate from the loop above
    // because the two bounds answer different questions — how many values, and
    // how much memory — and a single entry can breach the second on its own.
    //
    // `size > 1` is what keeps the value this write just stored. Eviction is
    // oldest-first and `set` re-inserts at the newest end, so the last entry
    // standing is always the one being written; stopping at one leaves it
    // alone. A value larger than the whole budget is therefore stored once and
    // evicted by the next write, rather than making a write that reported
    // success leave nothing behind.
    const byteBound = maxCacheBytes()
    if (byteBound > 0) {
      while (this.#bytes > byteBound && this.#entries.size > 1) {
        if (!this.#evictOldest()) break
      }
    }
  }

  /** Drop one key from both maps, keeping the byte sum with them. */
  #forget(key: string): boolean {
    this.#bytes -= this.#weights.get(key) ?? 0
    this.#weights.delete(key)
    return this.#entries.delete(key)
  }

  delete(key: string): boolean {
    this.#pendingWrites.delete(key)
    return this.#forget(key)
  }

  clear(): void {
    this.#entries.clear()
    this.#weights.clear()
    this.#bytes = 0
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

  /**
   * Drop every stored entry carrying `tag`, and every write still producing one.
   *
   * The second half is what `invalidate()` has always done by unioning the two
   * key sets, and for the same reason: a producer that started before the
   * invalidation is holding data from before it. Walking `#entries` alone made
   * the miss the unsafe case — a cold key has no entry to match, so the
   * ordinary mutate-then-`revalidateTag` sequence racing a first reader
   * committed the pre-mutation value under a full TTL.
   *
   * Deleting the pending write is the whole mechanism: `commitWrite` requires
   * its token to still be the registered one, so the producer's own commit
   * becomes a no-op and its caller still receives the value it computed.
   */
  invalidateTag(tag: string): void {
    for (const [key, entry] of this.#entries) {
      if (entry.tags.includes(tag)) this.delete(key)
    }
    for (const [key, pending] of this.#pendingWrites) {
      if (pending.tags.includes(tag)) this.delete(key)
    }
  }

  runSingleFlight<T>(
    key: string,
    producer: (token: symbol) => T | Promise<T>,
    tags: readonly string[] = [],
  ): { promise: Promise<T>; started: boolean } {
    const pending = this.#pendingWrites.get(key)
    if (pending) {
      return { promise: pending.promise as Promise<T>, started: false }
    }

    const token = Symbol(key)
    const promise = Promise.resolve()
      .then(() => producer(token))
      .finally(() => this.finishWrite(key, token))
    this.#pendingWrites.set(key, { token, promise, tags })
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
    // Reconcile rather than trust the running sum. Every add and remove already
    // adjusts it, but incremental accounting drifts on the one path somebody
    // adds later without noticing, and a byte budget that has drifted either
    // evicts nothing or evicts everything. This walk is already happening.
    let total = 0
    for (const key of this.#entries.keys()) total += this.#weights.get(key) ?? 0
    // Deleting the entry a Map iterator is standing on is defined behaviour —
    // it visits insertion order and skips what is already gone — so no copy of
    // the key set is needed to walk it while pruning it.
    for (const key of this.#weights.keys()) {
      if (!this.#entries.has(key)) this.#weights.delete(key)
    }
    this.#bytes = total
    return pruned
  }

  /** Bytes the tier is holding, by the same measure the budget uses. */
  get bytes(): number {
    return this.#bytes
  }

  get size(): number {
    return this.#entries.size
  }

  /** Evict the least recently used entry. `false` when nothing was freed. */
  #evictOldest(): boolean {
    const oldest = this.#entries.keys().next()
    if (oldest.done) return false
    return this.delete(oldest.value)
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

/**
 * Parse a cache duration into milliseconds, or throw.
 *
 * Exported so `@ruvyxa/testing`'s `mockCache` can refuse what `cache()` refuses.
 * A double that accepted `'5 minutes'` reported success for a loader that throws
 * at its first real request — the class of failure a test helper is least able
 * to catch and the one that only ever surfaces in production.
 */
export function parseTtl(value: string): number {
  const match = /^(\d+)\s*(ms|s|m|h|d)$/.exec(value)
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

/** Validate one cache tag and return it. Shared with `@ruvyxa/testing`. */
export function validateCacheTag(tag: string): string {
  if (typeof tag !== 'string' || !/^[A-Za-z0-9:._/-]{1,128}$/.test(tag)) {
    throw new TypeError(
      'cache tag must use 1-128 letters, digits, colon, dot, underscore, slash, or dash',
    )
  }
  return tag
}

/** Longest cache key accepted, in UTF-16 code units. */
const CACHE_KEY_MAX_LENGTH = 8192

/**
 * Validate a cache key.
 *
 * A function rather than an inline check because `@ruvyxa/testing`'s `mockCache`
 * has to answer the same way, and a second copy of the limit is a limit that
 * drifts.
 */
export function assertCacheKey(key: string): void {
  if (typeof key !== 'string' || key.length > CACHE_KEY_MAX_LENGTH) {
    throw new TypeError(`cache() key must contain at most ${CACHE_KEY_MAX_LENGTH} characters`)
  }
}

/**
 * Validate, de-duplicate, and order the tags one cache entry carries.
 *
 * Plain `.sort()` is a UTF-16 code-unit ordering. Tag order is part of the cache
 * entry's identity, and `localeCompare` would make that identity depend on the
 * host's ICU locale.
 */
export function normalizeCacheTags(values: readonly string[]): string[] {
  if (values.length > 32) throw new TypeError('cache().tags() accepts at most 32 tags')
  return [...new Set(values.map(validateCacheTag))].sort()
}

/**
 * Refuse to store a shared-cache value produced from request state (`RUV1840`).
 *
 * Exported for `@ruvyxa/testing`: this is the check that stops one visitor's
 * data being served to another out of a deployment-scoped cache, and a suite
 * built on a double that skipped it never exercised it at all. A no-op when no
 * request-context host is installed — the same answer the real builder gives.
 */
export function assertSharedCachePrivacy(): void {
  if (host()?.wasRead?.()) {
    throw new Error(
      "RUV1840 shared cache producer read request state; use cache().scope('request') or pass an explicit safe partition key",
    )
  }
}

/**
 * Refuse a cached value that is not a JSON-shaped tree (`RUV1841`).
 *
 * Exported for `@ruvyxa/testing`, where a cached `Date`, `Map`, or function used
 * to round-trip through the double and throw only in production.
 */
export function assertCacheSerializable(value: unknown): void {
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
  assertCacheKey(key)
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
      tags = normalizeCacheTags(values)
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
          const refresh = cacheStore.runSingleFlight(
            key,
            async (writeToken) => {
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
                if (committed) writeShared(key, { value, populatedAt, tags })
                if (!committed && cacheStore.peek(key) === cached) cached.refreshing = false
              } catch {
                // Producer failed during background refresh — keep serving stale
                if (cacheStore.peek(key) === cached) cached.refreshing = false
              }
            },
            tags,
          )
          if (refresh.started) cached.refreshing = true
          // The task catches producer failures itself so this is only a guard
          // against a future bookkeeping regression becoming unhandled work.
          void refresh.promise.catch(() => {})
        }
        return cached.value as T
      }

      // Miss or fully expired: ask the project's shared store before producing.
      //
      // Between the two tiers on purpose. The local store answers first because
      // it is the fast one and because Next.js's in-memory tier sits in the same
      // place; the shared store answers next because a value another instance
      // already produced is a value this one should not produce again. Only a
      // miss in both reaches the producer.
      //
      // Inside the single flight, so concurrent readers of one cold key make one
      // store read rather than one each — the same reason the producer is in
      // here.
      const pending = cacheStore.runSingleFlight<T>(
        key,
        async (writeToken) => {
          const pendingShared = readShared(key)
          const shared = pendingShared === null ? null : await pendingShared
          if (shared) {
            // The window is recomputed from this caller's `ttl`/`swr` rather than
            // read from the entry: the store knows when the value was produced,
            // and the code knows how long that is good for.
            const expiresAt = shared.populatedAt + ttlMs
            if (expiresAt > Date.now()) {
              cacheStore.commitWrite(
                key,
                writeToken,
                {
                  value: shared.value,
                  expiresAt,
                  staleUntil: expiresAt + swrMs,
                  refreshing: false,
                  tags,
                },
                cached,
              )
              return shared.value as T
            }
          }
          try {
            const value = await producer()
            assertSharedCachePrivacy()
            assertCacheSerializable(value)
            const populatedAt = Date.now()
            // Gated on the commit for the same reason the background refresh
            // above is: a rejected commit means this value was invalidated while
            // it was being produced, and publishing it to the shared store would
            // hand every other instance the answer this one just refused to keep.
            // The caller still receives it — it is what its own producer
            // computed — but nothing outlives the request.
            const committed = cacheStore.commitWrite(key, writeToken, {
              value,
              expiresAt: populatedAt + ttlMs,
              staleUntil: populatedAt + ttlMs + swrMs,
              refreshing: false,
              tags,
            })
            if (committed) writeShared(key, { value, populatedAt, tags })
            return value
          } catch (error) {
            // If we have stale data, return it rather than propagating the error
            if (cached && cacheStore.peek(key) === cached) {
              return cached.value as T
            }
            throw error
          }
        },
        tags,
      )
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

/**
 * Invalidate cache entries carrying one exact tag.
 *
 * Two things happen, and the second is the one that matters in production.
 * This process's own `cache()` store drops the entries immediately — that is
 * what this function has always done, and it still works at module scope with
 * no request in flight.
 *
 * The tag is *also* queued onto the request, when there is one, so the host can
 * hand it to the project's cache handler after the response. Without that step
 * the invalidation is per-instance: an application running several instances
 * behind one domain clears the one that served the mutation and leaves every
 * other instance answering from the entry it just invalidated. Queued rather
 * than called inline for the same reason `revalidatePath()` is — a store write
 * performed before the response is a write a failed request still made.
 *
 * A project that declares no `cache.handler` sees no change: there is nothing
 * for the host to hand the tag to, and the local invalidation is the whole
 * behaviour, as before.
 */
export function revalidateTag(tag: string): void {
  const validated = validateCacheTag(tag)
  cacheStore.invalidateTag(validated)

  // Outside a request this is a no-op rather than an error. `revalidateTag()`
  // has always been callable from module scope and from a background task, and
  // making it throw there would break callers to add a queue they have no
  // response to attach to.
  const host = (globalThis as Record<string, unknown>)[CONTEXT_KEY] as
    RequestContextHost | undefined
  const context = host?.peek?.() ?? host?.current() ?? null
  if (!context?.revalidateTags) return
  if (
    !context.revalidateTags.has(validated) &&
    context.revalidateTags.size >= MAX_REVALIDATED_TAGS_PER_REQUEST
  ) {
    throw new Error(
      `revalidateTag() accepts at most ${MAX_REVALIDATED_TAGS_PER_REQUEST} distinct tags in one ` +
        'request. Invalidate a broader tag rather than enumerating narrow ones.',
    )
  }
  context.revalidateTags.add(validated)
}

/**
 * Tags one request may queue for the shared store.
 *
 * The same bound `revalidatePath()` uses, for the same reason: the list crosses
 * the worker protocol, and an unbounded one is a request that can make the host
 * do unbounded work after it has already answered.
 */
const MAX_REVALIDATED_TAGS_PER_REQUEST = 64

/**
 * Get current cache statistics for observability.
 */
export function cacheStats(): {
  size: number
  maxEntries: number
  bytes: number
  maxBytes: number
  /** Shared-store writes this process is still waiting on. */
  pendingSharedWrites: number
  /** Shared-store writes dropped because the store could not keep up. */
  droppedSharedWrites: number
} {
  return {
    size: cacheStore.size,
    maxEntries: cacheStore.maxEntries,
    bytes: cacheStore.bytes,
    maxBytes: maxCacheBytes(),
    pendingSharedWrites,
    droppedSharedWrites,
  }
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

/**
 * Statuses HTTP forbids a body on, of the ones a `Response` can carry.
 *
 * `new Response('x', { status: 204 })` throws `Response with null body status
 * cannot have body`, so the message has to be refused before it gets there.
 *
 * `isNullBodyStatus` in `packages/ruvyxa/runtime/plugin-http.mjs` lists 101 and
 * 103 as well, and they are deliberately absent here rather than copied across
 * for symmetry: that function classifies a status read off the wire, where an
 * upgrade or early-hints response exists, while this one builds a `Response`,
 * whose constructor rejects anything below 200 outright. Listing them would be
 * branches that can never run.
 */
const NULL_BODY_STATUSES = new Set([204, 205, 304])

/**
 * A `Response` carrying `code`, for a handler that returns one.
 *
 * ```ts
 * if (!post) return status(404)
 * if (!user) return status(403, 'Forbidden')
 * return json(post)
 * ```
 *
 * **This replaced a `notFound()` that only ever produced 404.** Two things were
 * wrong with that. It collided with `notFound()` from `@ruvyxa/react`, which
 * *throws* a tagged signal for the route boundary to turn into `not-found.tsx`
 * — so a page that imported the server half rendered a `Response` object where
 * React expected an element, and an API route that imported the browser half
 * threw instead of answering, neither failing at the import. And it was the
 * only status with a helper at all: every 401, 403, 409, and 422 was written
 * out as `new Response(message, { status })` by hand.
 *
 * The throwing `notFound()` in `@ruvyxa/react` keeps its name; it is the one
 * that matches Next.js and the one a page wants.
 */
export function status(code: number, message?: string): Response {
  if (!Number.isInteger(code) || code < 200 || code > 599) {
    throw new TypeError(`status() code must be an integer from 200 to 599, got ${code}`)
  }
  if (NULL_BODY_STATUSES.has(code)) {
    if (message !== undefined) {
      throw new TypeError(
        `status() cannot attach a body to ${code}, which HTTP defines as bodiless`,
      )
    }
    return new Response(null, { status: code })
  }
  // No body unless one is given: an empty 404 is what a `Response` produces on
  // its own, and inventing a reason phrase here would mean a hand-maintained
  // table of them going stale in the background.
  if (message === undefined) return new Response(null, { status: code })
  return new Response(message, {
    status: code,
    headers: { 'content-type': 'text/plain; charset=utf-8' },
  })
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
  /** Tags `revalidateTag()` asked the host to drop from the shared store. */
  revalidateTags?: Set<string>
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
  /**
   * Route parameters matched for this request, for {@link params}.
   *
   * Absent for API routes and server actions, which are not matched against a
   * page pattern, and for hosts older than this field — {@link params} reports
   * that rather than inventing an empty object, so a typo in a segment name
   * cannot read as "this route has no parameters".
   */
  params?: Readonly<Record<string, string | string[]>>
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
 * The project's shared data store, installed by the runtime host.
 *
 * A `globalThis` key for the same reason `CONTEXT_KEY` is one: this module is
 * bundled for edge targets and cannot import from `packages/ruvyxa/runtime/`,
 * while the module that knows what `cache.handler` resolved to is emitted into
 * the route registry. They agree on a name and nothing else.
 */
const DATA_CACHE_KEY = '__RUVYXA_DATA_CACHE__'

/**
 * What a shared data store hands back for one key.
 *
 * `populatedAt` rather than an expiry, on purpose. The freshness window belongs
 * to the caller — `cache(key).ttl(...)` — and lives in this process; the store
 * only knows when the value was produced. Recomputing the window locally means
 * one deployment cannot serve a longer TTL than the code asked for because
 * another instance wrote the entry with a different one, and it keeps a clock
 * that runs fast on the writer from extending the window on every reader.
 */
interface SharedCacheEntry {
  value: unknown
  populatedAt: number
  tags?: readonly string[]
}

interface SharedDataCache {
  readData?: (key: string) => Promise<SharedCacheEntry | null> | SharedCacheEntry | null
  writeData?: (key: string, entry: SharedCacheEntry) => Promise<void> | void
  /** `cache.maxEntries`, carried on the same object the host installs. */
  maxEntries?: number
  /** `cache.maxBytes`, the memory budget the entry bound cannot express. */
  maxBytes?: number
  /**
   * Prepended to every key this deployment hands the store.
   *
   * Derived from the build id by the host, not by the application. Two
   * deployments pointed at one managed store otherwise write `cache('user:1')`
   * to the same place and read each other's answer.
   */
  keyPrefix?: string
}

/** The key as the shared store sees it: this deployment's, not just this app's. */
function sharedKey(key: string): string {
  const prefix = dataCacheConfig()?.keyPrefix
  return typeof prefix === 'string' && prefix !== '' ? prefix + key : key
}

function dataCacheConfig(): SharedDataCache | null {
  return ((globalThis as Record<string, unknown>)[DATA_CACHE_KEY] as SharedDataCache | null) ?? null
}

/**
 * Read one key from the project's shared store, or `null` for anything else.
 *
 * A store that throws is a store this process cannot use for this read, not a
 * request that fails: the producer below still runs and still answers. Reported
 * rather than swallowed, because a store that is quietly unreachable is a
 * deployment paying for one and getting per-instance caching.
 */
function readShared(key: string): Promise<SharedCacheEntry | null> | null {
  const store = dataCacheConfig()
  // `null`, not `Promise<null>`, and not an `async` function that returns early.
  // An `async` function yields a microtask even when its first statement
  // returns, and the caller awaits the result — so declaring no handler would
  // have moved every cold producer one tick later than it starts today. The
  // overwhelming majority of deployments are that case; they pay nothing.
  if (typeof store?.readData !== 'function') return null
  return (async () => {
    try {
      const entry = await store.readData!(sharedKey(key))
      if (!entry || typeof entry !== 'object') return null
      if (typeof entry.populatedAt !== 'number' || !Number.isFinite(entry.populatedAt)) return null
      return entry
    } catch (error) {
      console.error('[ruvyxa] cache.handler readData failed:', error)
      return null
    }
  })()
}

/**
 * Publish one produced value to the project's shared store.
 *
 * Not awaited by the caller. The value is already in this process's own store
 * and already returned; making a request wait on a remote write would make the
 * shared cache slower than no cache at all.
 */
const MAX_PENDING_SHARED_WRITES = 256
let pendingSharedWrites = 0
let droppedSharedWrites = 0
let reportedDroppedWrites = false

function writeShared(key: string, entry: SharedCacheEntry): void {
  const store = dataCacheConfig()
  if (typeof store?.writeData !== 'function') return

  // Not awaited, and therefore bounded. Populating a cache must not hold a
  // response — unlike invalidating one, which the host does await. But an
  // unawaited write is also an unowned promise, and a store that has gone slow
  // under load accumulates one per produced value with nothing to stop it:
  // the cache that exists to protect the origin becomes the thing that
  // exhausts the process.
  //
  // Dropping the write is the right failure. The value is already in this
  // process's own tier and already returned; what is lost is that another
  // instance has to produce it too, which is the behaviour of having no shared
  // store at all — where a queue that outgrows memory is the behaviour of
  // having no process at all.
  if (pendingSharedWrites >= MAX_PENDING_SHARED_WRITES) {
    droppedSharedWrites += 1
    if (!reportedDroppedWrites) {
      reportedDroppedWrites = true
      console.error(
        `[ruvyxa] cache.handler writeData is not keeping up; more than ${MAX_PENDING_SHARED_WRITES} ` +
          'writes were in flight, so this one and any others are being dropped. Reads still ' +
          'answer from the local tier. This is reported once; see cacheStats().droppedSharedWrites.',
      )
    }
    return
  }

  pendingSharedWrites += 1
  const settle = () => {
    pendingSharedWrites -= 1
  }
  try {
    Promise.resolve(store.writeData(sharedKey(key), entry)).then(settle, (error) => {
      settle()
      console.error('[ruvyxa] cache.handler writeData failed:', error)
    })
  } catch (error) {
    settle()
    console.error('[ruvyxa] cache.handler writeData failed:', error)
  }
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
 * cache. The flag is set at call time rather than found by scanning source:
 * the accessor marks the request's store, and `usedRequestContext` in
 * `packages/ruvyxa/runtime/request-context.mjs` is what the host reads back.
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

/**
 * Route parameters for the page being rendered, from anywhere inside it.
 *
 * Params are already passed to a page as props; this exists for everything
 * *below* it. A locale segment like `app/[lang]/...` is needed by shared
 * formatters, data loaders, and deeply nested components, and threading it
 * through every intermediate component as a prop is the kind of plumbing that
 * gets skipped — after which each of those helpers grows its own way of
 * guessing the locale from the URL.
 *
 * Unlike {@link cookies} and {@link headers}, reading this does **not** make the
 * render request-dependent. A parameter is part of the route's identity, not of
 * who is asking: `/th/blog/hello` renders the same document for everyone, so a
 * page that reads its own params stays statically renderable and cacheable.
 * That is why this reads the context without recording a request-state read.
 *
 * @example
 * ```tsx
 * // app/[lang]/blog/[slug]/page.tsx — and any component it renders
 * import { params } from 'ruvyxa/server'
 *
 * function PublishedOn({ date }: { date: Date }) {
 *   const { lang } = params()
 *   return <time>{date.toLocaleDateString(lang as string)}</time>
 * }
 * ```
 */
export function params(): Readonly<Record<string, string | string[]>> {
  const contextHost = (globalThis as Record<string, unknown>)[CONTEXT_KEY] as
    RequestContextHost | undefined
  const context = contextHost?.peek?.() ?? contextHost?.current() ?? null
  if (!context) {
    throw new Error(
      'params() was called outside a request.\n\n' +
        'It reads the route parameters of the page being rendered, so there has to ' +
        'be one. Calling it at module scope runs at import time, before any route ' +
        'has been matched — move the call inside the component or handler.',
    )
  }
  if (!context.params) {
    throw new Error(
      'params() is available while a page or API route is being served.\n\n' +
        'A server action is invoked at its own endpoint rather than matched ' +
        'against a route pattern, so it has no route parameters to read. Pass the ' +
        'values the action needs as arguments instead.',
    )
  }
  return context.params
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
