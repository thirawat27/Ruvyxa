import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { params } from '../../../packages/@ruvyxa/core/dist/server.js'

interface StubContext {
  params?: Readonly<Record<string, string | string[]>>
}

type Globals = typeof globalThis & {
  __RUVYXA_REQUEST_CONTEXT__?: {
    current(): StubContext | null
    peek?(): StubContext | null
    wasRead?(): boolean
  }
}

/**
 * Run `task` with a stubbed request-context host.
 *
 * `reads` counts how many times the host was asked for the context in a way
 * that records a request-state read — the thing that makes a render
 * request-scoped and therefore uncacheable.
 */
function withHost(context: StubContext | null, task: () => void): { recordedReads: number } {
  const globals = globalThis as Globals
  const previous = globals.__RUVYXA_REQUEST_CONTEXT__
  let recordedReads = 0
  globals.__RUVYXA_REQUEST_CONTEXT__ = {
    current: () => {
      recordedReads += 1
      return context
    },
    peek: () => context,
    wasRead: () => recordedReads > 0,
  }
  try {
    task()
  } finally {
    if (previous) globals.__RUVYXA_REQUEST_CONTEXT__ = previous
    else delete globals.__RUVYXA_REQUEST_CONTEXT__
  }
  return { recordedReads }
}

describe('params()', () => {
  it('returns the route parameters of the page being served', () => {
    withHost({ params: { lang: 'th', slug: 'hello' } }, () => {
      assert.deepEqual(params(), { lang: 'th', slug: 'hello' })
    })
  })

  it('reads catch-all segments as the array the matcher produced', () => {
    withHost({ params: { path: ['docs', 'intro'] } }, () => {
      assert.deepEqual(params().path, ['docs', 'intro'])
    })
  })

  /**
   * The property that separates this from `cookies()` and `headers()`.
   *
   * A parameter is part of the route's identity, not of who is asking:
   * `/th/blog/hello` renders the same document for every visitor. Recording a
   * request-state read here would mark the render request-scoped, and a page
   * that reads its own params would silently stop being statically renderable
   * and drop out of the ISR cache — the opposite of what this API is for.
   */
  it('does not make the render request-scoped', () => {
    const { recordedReads } = withHost({ params: { lang: 'th' } }, () => {
      params()
      params()
    })
    assert.equal(recordedReads, 0, 'params() must not record a request-state read')
  })

  it('names the mistake when called outside a request', () => {
    const globals = globalThis as Globals
    const previous = globals.__RUVYXA_REQUEST_CONTEXT__
    delete globals.__RUVYXA_REQUEST_CONTEXT__
    try {
      assert.throws(() => params(), /params\(\) was called outside a request/)
    } finally {
      if (previous) globals.__RUVYXA_REQUEST_CONTEXT__ = previous
    }
  })

  /**
   * A server action is invoked at its own endpoint rather than matched against
   * a route pattern, so its context carries no params. Reporting that is the
   * point: returning `{}` would make a mistyped segment name read as "this
   * route has no such parameter" instead of "there are no parameters here".
   */
  it('distinguishes no-params-here from a missing parameter', () => {
    withHost({}, () => {
      assert.throws(() => params(), /available while a page or API route is being served/)
    })
    withHost({ params: { lang: 'th' } }, () => {
      assert.equal(params().slug, undefined, 'a missing key is undefined, not an error')
    })
  })

  it('hands out a frozen view rather than the live context object', () => {
    withHost({ params: Object.freeze({ lang: 'th' }) }, () => {
      assert.throws(() => {
        ;(params() as Record<string, string>).lang = 'en'
      }, TypeError)
    })
  })
})
