import type { AuthRateLimitStore, AuthStore, RateLimitDecision } from './types.js'

export interface MemoryStoreOptions {
  /** Required acknowledgement that process-local state is only for tests/development. */
  development: true
}

interface MemoryValue {
  value: string
  expiresAt: number
}

/**
 * Entry ceiling for the process-local stores.
 *
 * Expiry alone only reclaims a key that someone reads again, and rate-limit
 * keys are derived from client IPs — a key per attacker address, never read
 * twice. A long-running dev server or a load test would grow the map without
 * bound, so writes sweep expired entries first and then evict the oldest.
 */
const MEMORY_STORE_MAX_ENTRIES = 10_000

/**
 * Drop expired entries, then oldest-first, until the map is under the ceiling.
 * `Map` preserves insertion order, so the first keys are the oldest writes.
 */
function enforceCeiling<T>(values: Map<string, T>, isExpired: (entry: T) => boolean): void {
  if (values.size < MEMORY_STORE_MAX_ENTRIES) return
  for (const [key, entry] of values) {
    if (isExpired(entry)) values.delete(key)
  }
  for (const key of values.keys()) {
    if (values.size < MEMORY_STORE_MAX_ENTRIES) break
    values.delete(key)
  }
}

/** Create a bounded-lifecycle process-local auth store for tests and development only. */
export function memoryAuthStore(options: MemoryStoreOptions): AuthStore {
  assertDevelopment(options)
  const values = new Map<string, MemoryValue>()
  const read = (key: string): MemoryValue | undefined => {
    const entry = values.get(key)
    if (entry && entry.expiresAt > Date.now()) return entry
    if (entry) values.delete(key)
    return undefined
  }
  return {
    name: 'memory',
    durable: false,
    async get(key) {
      return read(key)?.value ?? null
    },
    async set(key, value, ttlSeconds) {
      // Delete first so a re-set moves the key to the end of the insertion
      // order and is not treated as one of the oldest entries.
      values.delete(key)
      enforceCeiling(values, (entry) => entry.expiresAt <= Date.now())
      values.set(key, { value, expiresAt: Date.now() + ttlSeconds * 1000 })
    },
    async delete(key) {
      values.delete(key)
    },
    async take(key) {
      const value = read(key)?.value ?? null
      values.delete(key)
      return value
    },
  }
}

/** Create a process-local fixed-window rate limiter for tests and development only. */
export function memoryRateLimitStore(options: MemoryStoreOptions): AuthRateLimitStore {
  assertDevelopment(options)
  const values = new Map<string, { count: number; resetAt: number }>()
  return {
    name: 'memory',
    durable: false,
    async consume(key, limit, windowSeconds): Promise<RateLimitDecision> {
      const now = Date.now()
      let entry = values.get(key)
      if (!entry || entry.resetAt <= now) {
        values.delete(key)
        enforceCeiling(values, (candidate) => candidate.resetAt <= now)
        entry = { count: 0, resetAt: now + windowSeconds * 1000 }
        values.set(key, entry)
      }
      entry.count++
      return {
        allowed: entry.count <= limit,
        remaining: Math.max(0, limit - entry.count),
        retryAfterSeconds: Math.max(1, Math.ceil((entry.resetAt - now) / 1000)),
      }
    },
  }
}

function assertDevelopment(options: MemoryStoreOptions): void {
  if (!options || options.development !== true) {
    throw new TypeError('Memory auth stores require { development: true }')
  }
}
