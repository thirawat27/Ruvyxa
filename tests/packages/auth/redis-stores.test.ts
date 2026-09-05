import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  createAuth,
  ioredisCommandPort,
  nodeRedisCommandPort,
  redisAuthStore,
  redisRateLimitStore,
  type RedisCommandPort,
} from '../../../packages/@ruvyxa/auth/dist/index.js'

/**
 * The two scripts, pinned as text.
 *
 * There is no Redis in this suite, so a fake that only *modelled* the effect
 * could pass an inverted condition — and did, once, on the TTL repair. The fake
 * below recognises these exact strings and nothing else, so a change to a
 * script is a change to this file, made on purpose, and `scripts/smoke-redis-stores.mjs`
 * runs the same strings against a real server.
 */
const TAKE_SCRIPT =
  "local value = redis.call('GET', KEYS[1])\n" +
  "if value then redis.call('DEL', KEYS[1]) end\n" +
  'return value'

const CONSUME_SCRIPT =
  "local count = redis.call('INCR', KEYS[1])\n" +
  "local ttl = redis.call('TTL', KEYS[1])\n" +
  'if ttl < 0 then\n' +
  "  redis.call('EXPIRE', KEYS[1], ARGV[1])\n" +
  '  ttl = tonumber(ARGV[1])\n' +
  'end\n' +
  'return {count, ttl}'

interface Entry {
  value: string
  /** Absolute expiry in fake milliseconds, or `null` for a key with no TTL. */
  expiresAt: number | null
}

/** A Redis with a manual clock, speaking the five-command port directly. */
class FakeRedis implements RedisCommandPort {
  now = 1_000_000
  readonly entries = new Map<string, Entry>()
  readonly log: string[] = []

  private live(key: string): Entry | undefined {
    const entry = this.entries.get(key)
    if (!entry) return undefined
    if (entry.expiresAt !== null && entry.expiresAt <= this.now) {
      this.entries.delete(key)
      return undefined
    }
    return entry
  }

  async get(key: string) {
    this.log.push(`GET ${key}`)
    return this.live(key)?.value ?? null
  }

  async set(key: string, value: string, ttlSeconds: number) {
    this.log.push(`SET ${key} EX ${ttlSeconds}`)
    this.entries.set(key, { value, expiresAt: this.now + ttlSeconds * 1000 })
    return 'OK'
  }

  async del(key: string) {
    this.log.push(`DEL ${key}`)
    return this.entries.delete(key) ? 1 : 0
  }

  /** Drop a key's TTL the way `PERSIST` would, to model a counter that lost it. */
  persist(key: string) {
    const entry = this.live(key)
    if (entry) entry.expiresAt = null
  }

  async eval(script: string, keys: readonly string[], args: readonly string[]) {
    const [key] = keys
    if (script === TAKE_SCRIPT) {
      this.log.push(`EVAL take ${key}`)
      const entry = this.live(key)
      if (entry) this.entries.delete(key)
      return entry?.value ?? null
    }
    if (script === CONSUME_SCRIPT) {
      this.log.push(`EVAL consume ${key} ${args[0]}`)
      const entry = this.live(key)
      const count = entry ? Number(entry.value) + 1 : 1
      const windowMs = Number(args[0]) * 1000
      // -1 is what Redis reports for a key with no TTL; a missing key never
      // reaches TTL here because INCR creates it first.
      let ttl = -1
      if (entry && entry.expiresAt !== null) ttl = Math.ceil((entry.expiresAt - this.now) / 1000)
      const next: Entry = { value: String(count), expiresAt: entry?.expiresAt ?? null }
      if (ttl < 0) {
        next.expiresAt = this.now + windowMs
        ttl = Number(args[0])
      }
      this.entries.set(key, next)
      return [count, ttl]
    }
    throw new Error(`unpinned script:\n${script}`)
  }
}

describe('redisAuthStore()', () => {
  it('is durable, prefixes every key, and honours the TTL it was given', async () => {
    const redis = new FakeRedis()
    const store = redisAuthStore(redis)
    assert.equal(store.durable, true)
    assert.equal(store.name, 'redis')

    await store.set('session:abc', '{"id":1}', 60)
    assert.deepEqual(redis.log, ['SET ruvyxa:auth:session:abc EX 60'])
    assert.equal(await store.get('session:abc'), '{"id":1}')

    redis.now += 61_000
    assert.equal(await store.get('session:abc'), null, 'expired keys read as absent')

    await store.set('session:abc', 'again', 60)
    await store.delete('session:abc')
    assert.equal(await store.get('session:abc'), null)
  })

  it('takes a token in one atomic script and never twice', async () => {
    const redis = new FakeRedis()
    const store = redisAuthStore(redis, { prefix: 'app:' })
    await store.set('magic:tok', 'ada@example.com', 900)
    redis.log.length = 0

    assert.equal(await store.take('magic:tok'), 'ada@example.com')
    assert.equal(await store.take('magic:tok'), null)
    // One EVAL per take, no GET/DEL pair the caller could be raced between.
    assert.deepEqual(redis.log, ['EVAL take app:magic:tok', 'EVAL take app:magic:tok'])
  })

  it('rounds a fractional TTL up rather than down to zero', async () => {
    const redis = new FakeRedis()
    await redisAuthStore(redis).set('k', 'v', 0.2)
    assert.deepEqual(redis.log, ['SET ruvyxa:auth:k EX 1'])
  })

  it('refuses a port missing a command and a prefix that is not a string', () => {
    const redis = new FakeRedis()
    const withoutEval = {
      get: redis.get.bind(redis),
      set: redis.set.bind(redis),
      del: redis.del.bind(redis),
    }
    assert.throws(() => redisAuthStore(withoutEval as never), /RedisCommandPort.*eval/)
    assert.throws(() => redisAuthStore(redis, { prefix: 'has space:' }), /prefix/)
    assert.throws(() => redisRateLimitStore(redis, { prefix: 7 as never }), /prefix/)
  })
})

describe('redisRateLimitStore()', () => {
  it('counts a fixed window atomically and reports the window remainder', async () => {
    const redis = new FakeRedis()
    const limiter = redisRateLimitStore(redis)
    assert.equal(limiter.durable, true)

    const first = await limiter.consume('rate:x', 3, 60)
    assert.deepEqual(first, { allowed: true, remaining: 2, retryAfterSeconds: 60 })
    redis.now += 10_000
    await limiter.consume('rate:x', 3, 60)
    const third = await limiter.consume('rate:x', 3, 60)
    assert.deepEqual(third, { allowed: true, remaining: 0, retryAfterSeconds: 50 })
    const fourth = await limiter.consume('rate:x', 3, 60)
    assert.deepEqual(fourth, { allowed: false, remaining: 0, retryAfterSeconds: 50 })

    redis.now += 50_000
    const fresh = await limiter.consume('rate:x', 3, 60)
    assert.deepEqual(fresh, { allowed: true, remaining: 2, retryAfterSeconds: 60 })
    assert.ok(redis.log.every((line) => line.startsWith('EVAL consume ruvyxa:auth:rate:x 60')))
  })

  it('repairs a counter that lost its TTL instead of locking the client out forever', async () => {
    const redis = new FakeRedis()
    const limiter = redisRateLimitStore(redis)
    await limiter.consume('rate:y', 2, 30)
    redis.persist('ruvyxa:auth:rate:y')

    const repaired = await limiter.consume('rate:y', 2, 30)
    assert.deepEqual(repaired, { allowed: true, remaining: 0, retryAfterSeconds: 30 })
    redis.now += 31_000
    const after = await limiter.consume('rate:y', 2, 30)
    assert.equal(after.allowed, true, 'the repaired window expired and the count reset')
  })

  it('never reports a retry of zero seconds', async () => {
    const redis = new FakeRedis()
    const limiter = redisRateLimitStore(redis)
    await limiter.consume('rate:z', 1, 1)
    redis.now += 999
    const denied = await limiter.consume('rate:z', 1, 1)
    assert.equal(denied.allowed, false)
    assert.equal(denied.retryAfterSeconds, 1)
  })

  it('fails closed on a reply the script did not shape', async () => {
    const redis = new FakeRedis()
    redis.eval = async () => 'nonsense'
    await assert.rejects(
      () => redisRateLimitStore(redis).consume('rate:q', 1, 1),
      /Redis rate limit script returned an unexpected reply/,
    )
  })
})

describe('command ports', () => {
  it('speaks node-redis: object options for SET and EVAL', async () => {
    const calls: unknown[][] = []
    const client = {
      get: async (...args: unknown[]) => (calls.push(['get', ...args]), 'v'),
      set: async (...args: unknown[]) => (calls.push(['set', ...args]), 'OK'),
      del: async (...args: unknown[]) => (calls.push(['del', ...args]), 1),
      eval: async (...args: unknown[]) => (calls.push(['eval', ...args]), [1, 9]),
    }
    const port = nodeRedisCommandPort(client)
    assert.equal(await port.get('k'), 'v')
    await port.set('k', 'v', 30)
    await port.del('k')
    assert.deepEqual(await port.eval('return 1', ['k'], ['30']), [1, 9])
    assert.deepEqual(calls, [
      ['get', 'k'],
      ['set', 'k', 'v', { EX: 30 }],
      ['del', 'k'],
      ['eval', 'return 1', { keys: ['k'], arguments: ['30'] }],
    ])
  })

  it('speaks ioredis: positional EX and a numkeys-first EVAL', async () => {
    const calls: unknown[][] = []
    const client = {
      get: async (...args: unknown[]) => (calls.push(['get', ...args]), null),
      set: async (...args: unknown[]) => (calls.push(['set', ...args]), 'OK'),
      del: async (...args: unknown[]) => (calls.push(['del', ...args]), 0),
      eval: async (...args: unknown[]) => (calls.push(['eval', ...args]), null),
    }
    const port = ioredisCommandPort(client)
    assert.equal(await port.get('k'), null)
    await port.set('k', 'v', 30)
    await port.del('k')
    assert.equal(await port.eval('return nil', ['k'], ['30']), null)
    assert.deepEqual(calls, [
      ['get', 'k'],
      ['set', 'k', 'v', 'EX', 30],
      ['del', 'k'],
      ['eval', 'return nil', 1, 'k', '30'],
    ])
  })

  it('refuses a client that is not a Redis client', () => {
    assert.throws(() => nodeRedisCommandPort({} as never), /node-redis/)
    assert.throws(() => ioredisCommandPort(null as never), /ioredis/)
  })
})

describe('createAuth() over Redis', () => {
  it('issues and resolves sessions, and passes the production build gate', async () => {
    const redis = new FakeRedis()
    const origin = 'https://app.example.com'
    const auth = createAuth({
      secret: 'test-secret-that-is-at-least-thirty-two-characters',
      origin,
      store: redisAuthStore(redis),
      rateLimitStore: redisRateLimitStore(redis),
      providers: {
        email: {
          type: 'credentials',
          async authorize(input) {
            return input.password === 'correct' ? { id: 'user-1' } : null
          },
        },
      },
    })

    const response = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/login/email`, {
        method: 'POST',
        headers: { origin, 'content-type': 'application/json' },
        body: JSON.stringify({ email: 'ada@example.com', password: 'correct' }),
      }),
    )
    assert.equal(response?.status, 200)
    const cookie = response?.headers.get('set-cookie')?.split(';')[0]
    assert.ok(cookie)
    const session = await auth.getSession(new Request(`${origin}/`, { headers: { cookie } }))
    assert.equal(session?.user.id, 'user-1')
    assert.ok(
      [...redis.entries.keys()].some((key) => key.startsWith('ruvyxa:auth:session:')),
      'the session lives in Redis under the prefix',
    )

    let hook: ((context: unknown) => void | Promise<void>) | undefined
    await auth.plugin.register({
      environment: 'production',
      http: { onRequest() {}, onResponse() {}, route() {} },
      build: {
        onStart() {},
        onResolve() {},
        onLoad() {},
        onTransform() {},
        onComplete(value) {
          hook = value as typeof hook
        },
      },
      dev: { onFileChange() {} },
      diagnostics: { report() {} },
      native: { claim() {} },
    })
    await assert.doesNotReject(async () => hook?.({ manifest: { profile: 'production' } }))
  })
})
