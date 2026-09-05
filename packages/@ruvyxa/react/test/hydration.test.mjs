/**
 * `hydrate({ onError })` and the reporter contract behind it.
 *
 * The generated client entry cannot import this package, so React's error
 * callbacks and this module meet on two globals: a reporter function, and a
 * queue for everything reported before the reporter existed — which is every
 * hydration mismatch, because hydration runs before any application code can
 * install a handler. The entry side of that contract is the root-options
 * prelude, held to both bundlers by `tests/packages/ruvyxa/entry-prelude-parity.test.mjs`;
 * this file covers the reading side.
 */
import assert from 'node:assert/strict'
import { afterEach, describe, it } from 'node:test'

import { hydrate, reportHydrationError } from '../dist/hydration.js'

const REPORTER = '__RUVYXA_HYDRATION_REPORTER__'
const QUEUE = '__RUVYXA_HYDRATION_ERRORS__'

afterEach(() => {
  delete globalThis[REPORTER]
  delete globalThis[QUEUE]
  delete globalThis.window
})

describe('hydrate()', () => {
  it('installs the reporter the generated entry hands React errors to', () => {
    const seen = []
    hydrate({ onError: (error, context) => seen.push([error, context]) })
    assert.equal(typeof globalThis[REPORTER], 'function')

    const mismatch = new Error('Hydration failed')
    globalThis[REPORTER](mismatch, { kind: 'recoverable', componentStack: '\n    at App' })
    assert.deepEqual(seen, [[mismatch, { kind: 'recoverable', componentStack: '\n    at App' }]])
  })

  it('drains the errors React reported before the handler existed, in order', () => {
    const first = new Error('first')
    const second = new Error('second')
    globalThis[QUEUE] = [
      { error: first, context: { kind: 'recoverable' } },
      { error: second, context: { kind: 'uncaught', digest: 'abc' } },
    ]
    const seen = []
    hydrate({ onError: (error, context) => seen.push([error, context]) })
    assert.deepEqual(seen, [
      [first, { kind: 'recoverable' }],
      [second, { kind: 'uncaught', digest: 'abc' }],
    ])
    assert.deepEqual(globalThis[QUEUE], [], 'a drained queue is not replayed by a second hydrate()')
  })

  it('never lets a throwing handler reach React or the entry', () => {
    hydrate({
      onError: () => {
        throw new Error('reporting is down')
      },
    })
    assert.doesNotThrow(() => globalThis[REPORTER](new Error('x'), { kind: 'caught' }))
    assert.doesNotThrow(() => reportHydrationError(new Error('y')))
  })

  it('leaves an existing reporter alone when called without a handler', () => {
    const seen = []
    hydrate({ onError: (error) => seen.push(error) })
    hydrate()
    const error = new Error('still routed')
    globalThis[REPORTER](error, {})
    assert.deepEqual(seen, [error])
  })

  it('dispatches the hydration event on the root it was given', () => {
    const events = []
    globalThis.window = { dispatchEvent: (event) => events.push(['window', event.type]) }
    const root = { dispatchEvent: (event) => events.push(['root', event.type]) }
    hydrate({ root })
    hydrate()
    assert.deepEqual(events, [
      ['root', 'ruvyxa:hydrate'],
      ['window', 'ruvyxa:hydrate'],
    ])
  })
})

describe('reportHydrationError()', () => {
  it('reaches the installed handler, marked as a manual report', () => {
    const seen = []
    hydrate({ onError: (error, context) => seen.push([error, context]) })
    const error = new Error('manual')
    reportHydrationError(error, { componentStack: 'stack' })
    assert.deepEqual(seen, [[error, { kind: 'manual', componentStack: 'stack' }]])
  })

  it('queues for a handler that has not been installed yet', () => {
    const error = new Error('early')
    reportHydrationError(error)
    const seen = []
    hydrate({ onError: (reported) => seen.push(reported) })
    assert.deepEqual(seen, [error])
  })
})
