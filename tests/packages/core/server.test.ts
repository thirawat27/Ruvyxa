import { beforeEach, describe, it } from 'node:test'
import assert from 'node:assert/strict'

import {
  action,
  cache,
  cacheStats,
  invalidateCache,
  loader,
  status,
  redirect,
  revalidateTag,
} from '../../../packages/@ruvyxa/core/dist/server.js'

describe('server API', () => {
  beforeEach(() => {
    invalidateCache()
  })

  it('runs loaders with default context', async () => {
    const getValue = loader(async ({ params }) => params.id ?? 'missing')
    assert.equal(await getValue(), 'missing')
    assert.equal(await getValue({ params: { id: '123' } }), '123')
  })

  it('validates action input through schema', async () => {
    const save = action
      .input({ parse: (value: unknown) => String(value).trim() })
      .handler(async ({ input }) => input.toUpperCase())

    assert.equal(await save(' hello '), 'HELLO')
  })

  it('records validated realtime channels without changing action execution', async () => {
    const save = action
      .input({ parse: (value: unknown) => String(value) })
      .realtime(['users', 'users', 'dashboard'])
      .handler(async ({ input }) => input)

    assert.equal(await save('ok'), 'ok')
    assert.deepEqual(save.ruvyxa.realtime?.channels, ['users', 'dashboard'])
    assert.throws(() => action.realtime(' '), /1-128 letters/)
    assert.throws(
      () => action.realtime(Array.from({ length: 17 }, (_, index) => `channel-${index}`)),
      /at most 16/,
    )
  })

  it('uses route-scoped realtime when no channel is provided', () => {
    const refresh = action.realtime().handler(async () => true)
    assert.deepEqual(refresh.ruvyxa.realtime?.channels, [])
  })

  it('creates redirect responses', () => {
    const response = redirect('/login')
    assert.equal(response.status, 302)
    assert.equal(response.headers.get('Location'), '/login')
  })

  it('rejects non-3xx redirect status codes', () => {
    assert.throws(() => redirect('/login', 200), /redirect\(\) status must be 3xx/)
  })

  it('builds a response for any status, with or without a body', async () => {
    const bare = status(404)
    assert.equal(bare.status, 404)
    assert.equal(await bare.text(), '')

    const custom = status(404, 'Post missing')
    assert.equal(custom.status, 404)
    assert.equal(await custom.text(), 'Post missing')
    assert.equal(custom.headers.get('content-type'), 'text/plain; charset=utf-8')

    // The point of replacing the 404-only helper: every other status was being
    // written out as `new Response(message, { status })` by hand.
    const forbidden = status(403, 'Forbidden')
    assert.equal(forbidden.status, 403)
    assert.equal(await forbidden.text(), 'Forbidden')
  })

  it('refuses a status outside the response range', () => {
    assert.throws(() => status(99), /must be an integer from 200 to 599/)
    assert.throws(() => status(600), /must be an integer from 200 to 599/)
    assert.throws(() => status(404.5), /must be an integer from 200 to 599/)
  })

  it('refuses a body on a status HTTP defines as bodiless', async () => {
    // `new Response('x', { status: 204 })` throws inside the platform with a
    // message that names neither the helper nor the caller.
    assert.throws(() => status(204, 'ignored'), /cannot attach a body to 204/)
    // 101 and 103 are absent on purpose: `Response` rejects them itself.
    for (const code of [204, 205, 304]) {
      const response = status(code)
      assert.equal(response.status, code)
      assert.equal(response.body, null)
    }
  })
})

describe('cache', () => {
  beforeEach(() => {
    invalidateCache()
  })

  it('invalidates exact tags and rejects non-serializable deployment values', async () => {
    let calls = 0
    const read = () =>
      cache('tagged')
        .tags('posts')
        .get(() => ({ value: ++calls }))
    assert.deepEqual(await read(), { value: 1 })
    assert.deepEqual(await read(), { value: 1 })
    revalidateTag('posts')
    assert.deepEqual(await read(), { value: 2 })
    await assert.rejects(
      cache('bad-value').get(() => new Date()),
      /RUV1841/,
    )
  })

  it('fails shared writes after request state reads and isolates request scope', async () => {
    const globalRecord = globalThis as typeof globalThis & {
      __RUVYXA_REQUEST_CONTEXT__?: {
        peek(): { cache?: Map<string, unknown> }
        wasRead(): boolean
      }
    }
    const previous = globalRecord.__RUVYXA_REQUEST_CONTEXT__
    let context: { cache?: Map<string, unknown> } = {}
    globalRecord.__RUVYXA_REQUEST_CONTEXT__ = {
      peek: () => context,
      wasRead: () => true,
    }
    try {
      await assert.rejects(
        cache('private').get(() => 'secret'),
        /RUV1840/,
      )
      let calls = 0
      const read = () =>
        cache('request-value')
          .scope('request')
          .get(() => ++calls)
      assert.equal(await read(), 1)
      assert.equal(await read(), 1)
      context = {}
      assert.equal(await read(), 2)
    } finally {
      if (previous) globalRecord.__RUVYXA_REQUEST_CONTEXT__ = previous
      else delete globalRecord.__RUVYXA_REQUEST_CONTEXT__
    }
  })

  it('caches values and returns them on subsequent calls', async () => {
    let calls = 0
    const producer = () => {
      calls++
      return 'value'
    }

    const first = await cache('test-key').ttl('10s').get(producer)
    const second = await cache('test-key').ttl('10s').get(producer)

    assert.equal(first, 'value')
    assert.equal(second, 'value')
    assert.equal(calls, 1)
  })

  it('starts the TTL after a slow producer finishes', async () => {
    let calls = 0
    const producer = async () => {
      calls++
      await new Promise((resolve) => setTimeout(resolve, 30))
      return 'slow-value'
    }

    const first = await cache('slow-producer').ttl('10ms').get(producer)
    const second = await cache('slow-producer').ttl('10ms').get(producer)

    assert.equal(first, 'slow-value')
    assert.equal(second, 'slow-value')
    assert.equal(calls, 1)
  })

  it('shares one producer across concurrent cold misses', async () => {
    let calls = 0
    let resolveProducer: (value: string) => void = () => {}
    const producerResult = new Promise<string>((resolve) => {
      resolveProducer = resolve
    })
    const producer = () => {
      calls++
      return producerResult
    }

    const first = cache('single-flight').ttl('10s').get(producer)
    const second = cache('single-flight').ttl('10s').get(producer)
    await Promise.resolve()

    assert.equal(calls, 1)
    resolveProducer('shared')
    assert.deepEqual(await Promise.all([first, second]), ['shared', 'shared'])
  })

  it('cleans up a rejected single-flight so the key can retry', async () => {
    let calls = 0
    const producer = () => {
      calls++
      throw new Error('temporary failure')
    }

    const first = cache('single-flight-rejection').ttl('10s').get(producer)
    const second = cache('single-flight-rejection').ttl('10s').get(producer)
    await assert.rejects(first, /temporary failure/)
    await assert.rejects(second, /temporary failure/)
    assert.equal(calls, 1)

    const recovered = await cache('single-flight-rejection')
      .ttl('10s')
      .get(() => {
        calls++
        return 'recovered'
      })
    assert.equal(recovered, 'recovered')
    assert.equal(calls, 2)
  })

  it('does not let an in-flight producer repopulate an invalidated key', async () => {
    let resolveProducer: (value: string) => void = () => {}
    const producerResult = new Promise<string>((resolve) => {
      resolveProducer = resolve
    })

    const pending = cache('pending-write')
      .ttl('10s')
      .get(() => producerResult)
    await Promise.resolve()
    invalidateCache('pending-write')
    resolveProducer('obsolete')
    assert.equal(await pending, 'obsolete')

    let calls = 0
    const current = await cache('pending-write')
      .ttl('10s')
      .get(() => {
        calls++
        return 'current'
      })

    assert.equal(current, 'current')
    assert.equal(calls, 1)
  })

  it('does not let an obsolete completion overwrite a newer generation', async () => {
    let resolveOld: (value: string) => void = () => {}
    let resolveCurrent: (value: string) => void = () => {}
    const oldResult = new Promise<string>((resolve) => {
      resolveOld = resolve
    })
    const currentResult = new Promise<string>((resolve) => {
      resolveCurrent = resolve
    })

    const old = cache('ordered-generation')
      .ttl('10s')
      .get(() => oldResult)
    await Promise.resolve()
    invalidateCache('ordered-generation')
    const current = cache('ordered-generation')
      .ttl('10s')
      .get(() => currentResult)
    await Promise.resolve()

    resolveCurrent('current')
    assert.equal(await current, 'current')
    resolveOld('obsolete')
    assert.equal(await old, 'obsolete')

    let producerCalls = 0
    const cached = await cache('ordered-generation')
      .ttl('10s')
      .get(() => {
        producerCalls++
        return 'unexpected'
      })
    assert.equal(cached, 'current')
    assert.equal(producerCalls, 0)
  })

  it('invalidates by exact key', async () => {
    let calls = 0
    const producer = () => {
      calls++
      return `call-${calls}`
    }

    await cache('k1').ttl('10s').get(producer)
    invalidateCache('k1')
    const result = await cache('k1').ttl('10s').get(producer)

    assert.equal(result, 'call-2')
    assert.equal(calls, 2)
  })

  it('treats an empty cache key as a key instead of a clear-all sentinel', async () => {
    await cache('')
      .ttl('10s')
      .get(() => 'empty')
    await cache('retained')
      .ttl('10s')
      .get(() => 'retained')

    invalidateCache('')

    let emptyCalls = 0
    const empty = await cache('')
      .ttl('10s')
      .get(() => {
        emptyCalls++
        return 'repopulated'
      })
    let retainedCalls = 0
    const retained = await cache('retained')
      .ttl('10s')
      .get(() => {
        retainedCalls++
        return 'unexpected'
      })
    assert.equal(empty, 'repopulated')
    assert.equal(emptyCalls, 1)
    assert.equal(retained, 'retained')
    assert.equal(retainedCalls, 0)
  })

  it('invalidates by prefix', async () => {
    await cache('users:list')
      .ttl('10s')
      .get(() => 'list')
    await cache('users:detail:1')
      .ttl('10s')
      .get(() => 'detail')
    await cache('posts:list')
      .ttl('10s')
      .get(() => 'posts')

    invalidateCache('users')

    let userCalls = 0
    let postCalls = 0
    await cache('users:list')
      .ttl('10s')
      .get(() => {
        userCalls++
        return 'new-list'
      })
    await cache('posts:list')
      .ttl('10s')
      .get(() => {
        postCalls++
        return 'new-posts'
      })

    assert.equal(userCalls, 1) // was invalidated, so producer ran
    assert.equal(postCalls, 0) // was NOT invalidated, still cached
  })

  it('reports cache stats', async () => {
    await cache('a')
      .ttl('10s')
      .get(() => 1)
    await cache('b')
      .ttl('10s')
      .get(() => 2)

    const stats = cacheStats()
    assert.equal(stats.size, 2)
    assert.equal(stats.maxEntries, 1024)
  })

  it('does not evict an unrelated entry when refreshing a full cache', async () => {
    for (let index = 0; index < 1024; index++) {
      await cache(`capacity:${index}`)
        .ttl('10s')
        .get(() => index)
    }

    await cache('capacity:0')
      .ttl('10s')
      .get(() => 'refreshed')

    let producerCalls = 0
    const retained = await cache('capacity:1')
      .ttl('10s')
      .get(() => {
        producerCalls++
        return 'unexpected'
      })
    assert.equal(retained, 1)
    assert.equal(producerCalls, 0)
    assert.equal(cacheStats().size, 1024)
  })

  it('evicts an empty-string key when it is the least recently used entry', async () => {
    await cache('')
      .ttl('10s')
      .get(() => 'oldest')
    for (let index = 1; index < 1024; index++) {
      await cache(`empty-key-capacity:${index}`)
        .ttl('10s')
        .get(() => index)
    }

    await cache('empty-key-capacity:new')
      .ttl('10s')
      .get(() => 'new')

    let producerCalls = 0
    const value = await cache('')
      .ttl('10s')
      .get(() => {
        producerCalls++
        return 'repopulated'
      })
    assert.equal(value, 'repopulated')
    assert.equal(producerCalls, 1)
    assert.equal(cacheStats().size, 1024)
  })

  it('returns stale value when producer fails and stale data exists', async () => {
    await cache('fragile')
      .ttl('1ms')
      .get(() => 'good')

    // Wait for TTL to expire
    await new Promise((r) => setTimeout(r, 5))

    const result = await cache('fragile')
      .ttl('1ms')
      .get(() => {
        throw new Error('oops')
      })
    assert.equal(result, 'good')
  })

  it('serves stale data to concurrent readers while one refresh runs', async () => {
    await cache('swr-concurrent')
      .ttl('1ms')
      .swr('1s')
      .get(() => 'stale')
    await new Promise((resolve) => setTimeout(resolve, 5))

    let refreshCalls = 0
    let resolveRefresh: (value: string) => void = () => {}
    const refresh = new Promise<string>((resolve) => {
      resolveRefresh = resolve
    })
    const producer = () => {
      refreshCalls++
      return refresh
    }

    const first = await cache('swr-concurrent').ttl('1ms').swr('1s').get(producer)
    await Promise.resolve()
    const second = await cache('swr-concurrent')
      .ttl('1ms')
      .swr('1s')
      .get(() => {
        refreshCalls++
        return 'unexpected'
      })

    assert.equal(first, 'stale')
    assert.equal(second, 'stale')
    assert.equal(refreshCalls, 1)
    resolveRefresh('fresh')
  })

  it('throws when producer fails and no stale data exists', async () => {
    await assert.rejects(
      cache('nonexistent')
        .ttl('10s')
        .get(() => {
          throw new Error('fail')
        }),
      /fail/,
    )
  })

  it('rejects invalid cache duration strings instead of silently using the default TTL', () => {
    assert.throws(() => cache('invalid-duration').ttl('soon'), /Invalid cache duration "soon"/)
    assert.throws(() => cache('invalid-swr').swr('1 week'), /Invalid cache duration "1 week"/)
  })

  it('rejects cache durations that are zero or exceed safe millisecond precision', () => {
    assert.throws(() => cache('zero-duration').ttl('0s'), /Invalid cache duration "0s"/)
    assert.throws(
      () => cache('unsafe-amount').ttl('9007199254740992ms'),
      /Invalid cache duration "9007199254740992ms"/,
    )
    assert.throws(
      () => cache('unsafe-product').swr('9007199254740991d'),
      /Invalid cache duration "9007199254740991d"/,
    )
  })
})

describe('cache() through a project shared store', () => {
  const DATA_CACHE_KEY = '__RUVYXA_DATA_CACHE__'
  const globals = globalThis as Record<string, unknown>

  function install(store: unknown): () => void {
    const previous = globals[DATA_CACHE_KEY]
    globals[DATA_CACHE_KEY] = store
    return () => {
      globals[DATA_CACHE_KEY] = previous
    }
  }

  // The whole point: an instance that never ran the producer still answers,
  // because another one already did and wrote it where both can see. Without
  // this every instance caches alone, which is per-instance caching wearing the
  // word "cache".
  it('answers from the shared store without running the producer', async () => {
    let produced = 0
    const restore = install({
      readData: () => ({ value: 'from the shared store', populatedAt: Date.now() }),
      writeData: () => {},
    })
    try {
      const value = await cache(`shared-hit-${Math.random()}`)
        .ttl('60s')
        .get(() => {
          produced += 1
          return 'produced locally'
        })
      assert.equal(value, 'from the shared store')
      assert.equal(produced, 0, 'the producer ran even though the shared store answered')
    } finally {
      restore()
    }
  })

  // The freshness window belongs to the caller, not to the entry. A store that
  // hands back something produced an hour ago must not satisfy a one-minute
  // `ttl`, however the writer labelled it.
  it('recomputes the window from this caller ttl, not from the entry', async () => {
    let produced = 0
    const restore = install({
      readData: () => ({ value: 'stale', populatedAt: Date.now() - 3_600_000 }),
      writeData: () => {},
    })
    try {
      const value = await cache(`shared-expired-${Math.random()}`)
        .ttl('60s')
        .get(() => {
          produced += 1
          return 'produced locally'
        })
      assert.equal(value, 'produced locally')
      assert.equal(produced, 1)
    } finally {
      restore()
    }
  })

  it('publishes a produced value to the shared store', async () => {
    const written: Array<{ key: string; value: unknown }> = []
    const key = `shared-write-${Math.random()}`
    const restore = install({
      readData: () => null,
      writeData: (writtenKey: string, entry: { value: unknown; populatedAt: number }) => {
        written.push({ key: writtenKey, value: entry.value })
        assert.equal(typeof entry.populatedAt, 'number')
      },
    })
    try {
      await cache(key)
        .ttl('60s')
        .get(() => 'produced locally')
      // Not awaited by the caller — a request must not wait on a remote write —
      // so the publish lands on a later tick.
      await new Promise((resolve) => setTimeout(resolve, 10))
      assert.deepEqual(written, [{ key, value: 'produced locally' }])
    } finally {
      restore()
    }
  })

  // A store this process cannot reach is a slower cache, not a failed request.
  it('produces normally when the shared store throws', async () => {
    const restore = install({
      readData: () => {
        throw new Error('store unreachable')
      },
      writeData: () => {
        throw new Error('store unreachable')
      },
    })
    try {
      const value = await cache(`shared-throws-${Math.random()}`)
        .ttl('60s')
        .get(() => 'produced locally')
      assert.equal(value, 'produced locally')
    } finally {
      restore()
    }
  })

  // A project that declares no handler must behave exactly as it did.
  it('is untouched when no shared store is installed', async () => {
    const restore = install(undefined)
    try {
      const key = `no-store-${Math.random()}`
      const first = await cache(key)
        .ttl('60s')
        .get(() => 'produced once')
      const second = await cache(key)
        .ttl('60s')
        .get(() => 'produced twice')
      assert.equal(first, 'produced once')
      assert.equal(second, 'produced once', 'the local store must still answer the second read')
    } finally {
      restore()
    }
  })
})

describe('cache.maxEntries', () => {
  const DATA_CACHE_KEY = '__RUVYXA_DATA_CACHE__'
  const globals = globalThis as Record<string, unknown>

  function install(config: unknown): () => void {
    const previous = globals[DATA_CACHE_KEY]
    globals[DATA_CACHE_KEY] = config
    return () => {
      globals[DATA_CACHE_KEY] = previous
    }
  }

  it('defaults to 1024 when nothing is configured', () => {
    const restore = install(undefined)
    try {
      assert.equal(cacheStats().maxEntries, 1024)
    } finally {
      restore()
    }
  })

  it('reports the configured bound', () => {
    const restore = install({ maxEntries: 8 })
    try {
      assert.equal(cacheStats().maxEntries, 8)
    } finally {
      restore()
    }
  })

  // Zero is off, not "hold one". The eviction loop cannot evict from an empty
  // map, so a bound of zero used to store the first entry and thrash from
  // there — a cache that is neither on nor off. A deployment that turns the
  // local tier off is asking for every read to reach the shared store.
  it('stores nothing at all when the bound is zero', async () => {
    const restore = install({ maxEntries: 0 })
    try {
      let produced = 0
      const key = `bound-zero-${Math.random()}`
      const read = () =>
        cache(key)
          .ttl('60s')
          .get(() => {
            produced += 1
            return produced
          })
      // A delta, not an absolute: the store is module-level and every test in
      // this file shares it, so its size here is whatever the file has already
      // put in it.
      const before = cacheStats().size
      assert.equal(await read(), 1)
      assert.equal(await read(), 2, 'the second read was answered from a tier that is turned off')
      assert.equal(cacheStats().size, before, 'a tier that is off still grew')
    } finally {
      restore()
    }
  })

  // The bound is read per write, not captured when the store was constructed:
  // this module is evaluated before the route registry installs anything, so a
  // captured value would always be the default and the setting would do
  // nothing in exactly the deployments that set it.
  it('takes effect after the store already exists', async () => {
    const restore = install({ maxEntries: 1 })
    try {
      await cache(`bound-late-a-${Math.random()}`)
        .ttl('60s')
        .get(() => 'a')
      await cache(`bound-late-b-${Math.random()}`)
        .ttl('60s')
        .get(() => 'b')
      assert.ok(cacheStats().size <= 1, `a bound of 1 held ${cacheStats().size} entries`)
    } finally {
      restore()
    }
  })

  // An unusable bound must not silently become "no cache" or "unbounded" —
  // those are the two directions that hurt, and both look like working code.
  it('falls back to the default for a bound it cannot use', () => {
    for (const bad of [-1, 1.5, Number.NaN]) {
      const restore = install({ maxEntries: bad })
      try {
        assert.equal(cacheStats().maxEntries, 1024, `${bad} should not change the bound`)
      } finally {
        restore()
      }
    }
  })
})
