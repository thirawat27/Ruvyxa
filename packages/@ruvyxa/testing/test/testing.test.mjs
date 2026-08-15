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
