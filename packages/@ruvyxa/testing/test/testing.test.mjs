import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { mockAction, mockCache, mockLoader } from '../dist/index.js'

describe('@ruvyxa/testing', () => {
  it('records normalized loader calls without a server', async () => {
    const load = mockLoader(({ params }) => ({ slug: params.slug }))
    assert.deepEqual(await load({ params: { slug: 'hello' } }), { slug: 'hello' })
    assert.equal(load.ruvyxa.kind, 'loader')
    assert.equal(load.calls[0].request.url, 'http://localhost/__ruvyxa/test')
  })

  it('records action invalidations and preserves action metadata', async () => {
    const save = mockAction(({ input, invalidate }) => {
      invalidate('posts')
      return input.id
    })
    assert.equal(await save({ id: 7 }), 7)
    assert.deepEqual(save.invalidations, ['posts'])
    assert.equal(save.ruvyxa.kind, 'action')
  })

  it('provides deterministic cache hits and observable policies', async () => {
    const cache = mockCache({ users: ['seed'] })
    assert.deepEqual(
      await cache('users')
        .ttl('5m')
        .tags('team', 'users', 'team')
        .get(() => ['fresh']),
      ['seed'],
    )
    assert.deepEqual(
      await cache('posts')
        .swr('1m')
        .scope('request')
        .get(() => ['fresh']),
      ['fresh'],
    )
    assert.deepEqual(cache.calls, [
      {
        key: 'users',
        ttl: '5m',
        swr: undefined,
        tags: ['team', 'users'],
        scope: 'deployment',
        hit: true,
      },
      {
        key: 'posts',
        ttl: undefined,
        swr: '1m',
        tags: [],
        scope: 'request',
        hit: false,
      },
    ])
  })
})

/**
 * The double must refuse everything the real `cache()` refuses.
 *
 * A helper that accepts an invalid duration or an unserializable value reports
 * success for a loader that throws at its first real request — the one class of
 * failure a suite built on `mockCache` can never otherwise reach.
 */
describe('mockCache contract parity', () => {
  it('rejects a key that is not a string of at most 8192 characters', () => {
    const cache = mockCache()
    assert.throws(() => cache('k'.repeat(8193)), /at most 8192 characters/)
    assert.throws(() => cache(7), /at most 8192 characters/)
    // The boundary itself is accepted.
    assert.ok(cache('k'.repeat(8192)))
  })

  it('rejects a ttl or swr the real parser refuses', () => {
    const cache = mockCache()
    assert.throws(() => cache('posts').ttl('5 minutes'), /Invalid cache duration/)
    assert.throws(() => cache('posts').ttl('0s'), /Invalid cache duration/)
    assert.throws(() => cache('posts').swr('1 hour'), /Invalid cache duration/)
    // The accepted spellings still pass.
    assert.ok(cache('posts').ttl('5m').swr('30s'))
  })

  it('rejects an invalid tag and more than 32 tags', () => {
    const cache = mockCache()
    assert.throws(() => cache('posts').tags('a b'), /cache tag must use/)
    assert.throws(() => cache('posts').tags(''), /cache tag must use/)
    assert.throws(
      () => cache('posts').tags(...Array.from({ length: 33 }, (_, index) => `t${index}`)),
      /at most 32 tags/,
    )
  })

  it('rejects a scope outside the two literals', () => {
    const cache = mockCache()
    assert.throws(() => cache('posts').scope('global'), /must be "deployment" or "request"/)
  })

  it('rejects a request-scoped read when the double was given no request context', async () => {
    const cache = mockCache({}, { requestContext: false })
    await assert.rejects(
      () =>
        cache('posts')
          .scope('request')
          .get(() => ['fresh']),
      /request-scoped cache used outside a request/,
    )
  })

  it('rejects a value that is not a JSON-shaped tree', async () => {
    const cache = mockCache()
    await assert.rejects(() => cache('posts').get(() => ({ published: new Date() })), /RUV1841/)
    await assert.rejects(
      () =>
        cache('fns').get(() => ({
          render: () => null,
        })),
      /RUV1841/,
    )
    await assert.rejects(
      () =>
        cache('request-scoped')
          .scope('request')
          .get(() => new Map()),
      /RUV1841/,
    )
  })

  it('validates the produced value only after the producer has run', async () => {
    const cache = mockCache()
    let ran = 0
    await assert.rejects(
      () =>
        cache('posts').get(() => {
          ran += 1
          return new Date()
        }),
      /RUV1841/,
    )
    // The real builder awaits the producer before asserting on its value, so a
    // double that checked earlier would diverge in the other direction.
    assert.equal(ran, 1)
    // A rejected value is never cached.
    assert.deepEqual(cache.calls.length, 1)
    assert.equal(cache.calls[0].hit, false)
  })
})
