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

/**
 * The four Redis commands the durable stores need, in one client-neutral shape.
 *
 * This package pins no Redis client. node-redis and ioredis disagree on the
 * signatures of `SET` with an expiry and of `EVAL` — object options against
 * positional arguments — and sniffing which client arrived is a guess that
 * sends a malformed command to the session store. So the caller says which one
 * it has, through {@link nodeRedisCommandPort} or {@link ioredisCommandPort},
 * and anything else that can answer these four calls is a valid port too.
 */
export interface RedisCommandPort {
  get(key: string): Promise<string | null>
  /** `SET key value EX ttlSeconds`. */
  set(key: string, value: string, ttlSeconds: number): Promise<unknown>
  del(key: string): Promise<unknown>
  /** `EVAL script numkeys KEYS... ARGV...`, resolving to the script's reply. */
  eval(script: string, keys: readonly string[], args: readonly string[]): Promise<unknown>
}

/** The surface of a connected node-redis (`redis` on npm, v4+) client this package uses. */
export interface NodeRedisClientLike {
  get(key: string): Promise<string | null>
  set(key: string, value: string, options: { EX: number }): Promise<unknown>
  del(key: string): Promise<unknown>
  eval(script: string, options: { keys: string[]; arguments: string[] }): Promise<unknown>
}

/** The surface of a connected ioredis client this package uses. */
export interface IoredisClientLike {
  get(key: string): Promise<string | null>
  set(key: string, value: string, mode: 'EX', ttlSeconds: number): Promise<unknown>
  del(key: string): Promise<unknown>
  eval(script: string, numberOfKeys: number, ...keysAndArguments: string[]): Promise<unknown>
}

/** Adapt a node-redis client (`import { createClient } from 'redis'`). */
export function nodeRedisCommandPort(client: NodeRedisClientLike): RedisCommandPort {
  assertClient(client, 'node-redis')
  return {
    get: (key) => client.get(key),
    set: (key, value, ttlSeconds) => client.set(key, value, { EX: ttlSeconds }),
    del: (key) => client.del(key),
    eval: (script, keys, args) => client.eval(script, { keys: [...keys], arguments: [...args] }),
  }
}

/** Adapt an ioredis client (`import Redis from 'ioredis'`). */
export function ioredisCommandPort(client: IoredisClientLike): RedisCommandPort {
  assertClient(client, 'ioredis')
  return {
    get: (key) => client.get(key),
    set: (key, value, ttlSeconds) => client.set(key, value, 'EX', ttlSeconds),
    del: (key) => client.del(key),
    eval: (script, keys, args) => client.eval(script, keys.length, ...keys, ...args),
  }
}

export interface RedisStoreOptions {
  /**
   * Prefix for every key this store writes, so one Redis can hold more than one
   * application. Letters, digits, and `:._/-` only. @default "ruvyxa:auth:"
   */
  prefix?: string
}

/**
 * Read a single-use token and delete it in one server-side step.
 *
 * `GET` then `DEL` from this process is two round trips, and two concurrent
 * callbacks racing between them both receive the token — the replay the
 * `take` contract exists to rule out. A script runs atomically on the server.
 * `GETDEL` would do the same in one command but only exists since Redis 6.2;
 * `EVAL` has been there since 2.6, so this asks nothing of the deployment's
 * Redis version.
 */
const REDIS_TAKE_SCRIPT =
  "local value = redis.call('GET', KEYS[1])\n" +
  "if value then redis.call('DEL', KEYS[1]) end\n" +
  'return value'

/**
 * Count one request against a fixed window and report where the window stands.
 *
 * `INCR` and `EXPIRE` from this process are the same two-step race as above,
 * and a counter whose `EXPIRE` never landed — the process died between the two
 * calls — counts forever, which locks its client out for good. So the script
 * reads the TTL after incrementing and sets the window whenever there is none:
 * on the first request of a window, and on a counter that lost its expiry. The
 * repair is deliberate and the test suite pins it: a version of this check with
 * the condition inverted passed a fake that only modelled the happy path.
 */
const REDIS_CONSUME_SCRIPT =
  "local count = redis.call('INCR', KEYS[1])\n" +
  "local ttl = redis.call('TTL', KEYS[1])\n" +
  'if ttl < 0 then\n' +
  "  redis.call('EXPIRE', KEYS[1], ARGV[1])\n" +
  '  ttl = tonumber(ARGV[1])\n' +
  'end\n' +
  'return {count, ttl}'

/**
 * A durable, shared session and one-time-token store on Redis.
 *
 * `take` is atomic on the server, so two instances behind one load balancer
 * cannot both accept a magic link or an OAuth state. Driver errors propagate:
 * the auth runtime answers them with a generic 500, which is the fail-closed
 * behaviour a store outage should have.
 */
export function redisAuthStore(port: RedisCommandPort, options: RedisStoreOptions = {}): AuthStore {
  assertPort(port)
  const prefix = normalizePrefix(options.prefix)
  return {
    name: 'redis',
    durable: true,
    get: (key) => port.get(prefix + key),
    async set(key, value, ttlSeconds) {
      await port.set(prefix + key, value, redisTtl(ttlSeconds))
    },
    async delete(key) {
      await port.del(prefix + key)
    },
    async take(key) {
      const reply = await port.eval(REDIS_TAKE_SCRIPT, [prefix + key], [])
      return typeof reply === 'string' ? reply : null
    },
  }
}

/** A durable, shared fixed-window rate limiter on Redis. */
export function redisRateLimitStore(
  port: RedisCommandPort,
  options: RedisStoreOptions = {},
): AuthRateLimitStore {
  assertPort(port)
  const prefix = normalizePrefix(options.prefix)
  return {
    name: 'redis',
    durable: true,
    async consume(key, limit, windowSeconds): Promise<RateLimitDecision> {
      const reply = await port.eval(
        REDIS_CONSUME_SCRIPT,
        [prefix + key],
        [String(redisTtl(windowSeconds))],
      )
      const [count, ttl] = Array.isArray(reply) ? reply.map(Number) : [Number.NaN, Number.NaN]
      if (!Number.isSafeInteger(count) || !Number.isSafeInteger(ttl)) {
        throw new TypeError('Redis rate limit script returned an unexpected reply')
      }
      return {
        allowed: count <= limit,
        remaining: Math.max(0, limit - count),
        retryAfterSeconds: Math.max(1, ttl),
      }
    },
  }
}

/**
 * Whole seconds, at least one: `EX 0` is a Redis error and a sub-second TTL
 * rounded down would store nothing at all.
 */
function redisTtl(seconds: number): number {
  if (!Number.isFinite(seconds) || seconds <= 0) {
    throw new TypeError('Redis auth stores require a positive TTL in seconds')
  }
  return Math.max(1, Math.ceil(seconds))
}

function normalizePrefix(value: string | undefined): string {
  if (value === undefined) return 'ruvyxa:auth:'
  if (typeof value !== 'string' || !/^[A-Za-z0-9:._/-]*$/.test(value)) {
    throw new TypeError(
      'Redis auth store prefix must contain only letters, digits, colon, dot, underscore, slash, or dash',
    )
  }
  return value
}

function assertPort(port: RedisCommandPort): void {
  for (const method of ['get', 'set', 'del', 'eval'] as const) {
    if (typeof port?.[method] !== 'function') {
      throw new TypeError(`Redis auth stores require a RedisCommandPort with ${method}()`)
    }
  }
}

function assertClient(client: unknown, name: string): void {
  const value = client as Record<string, unknown> | null
  for (const method of ['get', 'set', 'del', 'eval']) {
    if (typeof value?.[method] !== 'function') {
      throw new TypeError(`${name} command port requires a connected client with ${method}()`)
    }
  }
}
