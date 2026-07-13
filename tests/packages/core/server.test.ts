import { beforeEach, describe, it } from 'node:test'
import assert from 'node:assert/strict'

import {
  action,
  cache,
  cacheStats,
  invalidateCache,
  loader,
  redirect,
} from '../../../packages/@ruvyxa/core/src/server.ts'

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

  it('creates redirect responses', () => {
    const response = redirect('/login')
    assert.equal(response.status, 302)
    assert.equal(response.headers.get('Location'), '/login')
  })

  it('rejects non-3xx redirect status codes', () => {
    assert.throws(() => redirect('/login', 200), /redirect\(\) status must be 3xx/)
  })
})

describe('cache', () => {
  beforeEach(() => {
    invalidateCache()
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
})
