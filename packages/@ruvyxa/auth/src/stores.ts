import type { AuthRateLimitStore, AuthStore, RateLimitDecision } from './types.js'

export interface MemoryStoreOptions {
  /** Required acknowledgement that process-local state is only for tests/development. */
  development: true
}

interface MemoryValue {
  value: string
  expiresAt: number
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
