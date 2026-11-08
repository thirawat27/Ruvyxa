import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { WorkerAdmissionController } from '../../../packages/ruvyxa/runtime/worker-admission.mjs'

describe('WorkerAdmissionController', () => {
  it('rejects invalid bounds at construction', () => {
    for (const maxConcurrentRequests of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
      assert.throws(
        () => new WorkerAdmissionController({ maxConcurrentRequests, maxQueuedRequests: 1 }),
        /maxConcurrentRequests must be a positive safe integer/,
      )
    }
    assert.throws(
      () => new WorkerAdmissionController({ maxConcurrentRequests: 1, maxQueuedRequests: 0 }),
      /maxQueuedRequests must be a positive safe integer/,
    )
  })

  it('admits immediately up to capacity and rejects beyond the queue bound', () => {
    const admission = new WorkerAdmissionController({
      maxConcurrentRequests: 2,
      maxQueuedRequests: 1,
    })

    assert.equal(admission.acquire(), true)
    assert.equal(admission.acquire(), true)
    assert.ok(admission.acquire() instanceof Promise)
    assert.equal(admission.acquire(), false)
    assert.deepEqual(admission.snapshot(), {
      activeRequests: 2,
      queuedRequests: 1,
      maxConcurrentRequests: 2,
      maxQueuedRequests: 1,
      rejectedRequests: 1,
      admissionClosed: false,
    })
  })

  it('transfers released slots to waiters in FIFO order', async () => {
    const admission = new WorkerAdmissionController({
      maxConcurrentRequests: 1,
      maxQueuedRequests: 2,
    })
    assert.equal(admission.acquire(), true)

    const order = []
    const first = admission.acquire().then((admitted) => order.push(['first', admitted]))
    const second = admission.acquire().then((admitted) => order.push(['second', admitted]))

    admission.release()
    await first
    assert.deepEqual(order, [['first', true]])
    assert.equal(admission.activeRequests, 1)

    admission.release()
    await second
    assert.deepEqual(order, [
      ['first', true],
      ['second', true],
    ])
    admission.release()
    assert.equal(admission.activeRequests, 0)
  })

  it('settles queued and future work on close without counting overload', async () => {
    const admission = new WorkerAdmissionController({
      maxConcurrentRequests: 1,
      maxQueuedRequests: 2,
    })
    assert.equal(admission.acquire(), true)
    const queued = admission.acquire()

    admission.close()
    admission.close()

    assert.equal(await queued, false)
    assert.equal(admission.acquire(), false)
    assert.deepEqual(admission.snapshot(), {
      activeRequests: 1,
      queuedRequests: 0,
      maxConcurrentRequests: 1,
      maxQueuedRequests: 2,
      rejectedRequests: 0,
      admissionClosed: true,
    })
    admission.release()
    assert.equal(admission.activeRequests, 0)
  })

  it('withdraws a queued waiter whose signal aborts, freeing its queue place', async () => {
    // A request the host cancels while it waits must not keep its place: the
    // queue is deepest exactly when clients are giving up, and a withdrawn
    // waiter that still counts against the bound turns live requests away.
    const admission = new WorkerAdmissionController({
      maxConcurrentRequests: 1,
      maxQueuedRequests: 1,
    })
    assert.equal(admission.acquire(), true)

    const cancellation = new AbortController()
    const withdrawn = admission.acquire(cancellation.signal)
    assert.equal(admission.snapshot().queuedRequests, 1)
    cancellation.abort()

    assert.equal(await withdrawn, false)
    assert.equal(admission.snapshot().queuedRequests, 0)
    assert.equal(admission.snapshot().rejectedRequests, 0, 'a withdrawal is not overload')

    // The place it held is available to the next request, and a release goes
    // to that request rather than to the withdrawn one.
    const next = admission.acquire()
    assert.ok(next instanceof Promise)
    admission.release()
    assert.equal(await next, true)
    assert.equal(admission.activeRequests, 1)
    admission.release()
    assert.equal(admission.activeRequests, 0)
  })

  it('does not admit a request whose signal is already aborted', () => {
    const admission = new WorkerAdmissionController({
      maxConcurrentRequests: 1,
      maxQueuedRequests: 1,
    })
    assert.equal(admission.acquire(AbortSignal.abort()), false)
    assert.equal(admission.activeRequests, 0)
    assert.equal(admission.snapshot().rejectedRequests, 0)
  })

  it('an abort after admission does not take the slot back', async () => {
    const admission = new WorkerAdmissionController({
      maxConcurrentRequests: 1,
      maxQueuedRequests: 1,
    })
    assert.equal(admission.acquire(), true)
    const cancellation = new AbortController()
    const queued = admission.acquire(cancellation.signal)
    admission.release()
    assert.equal(await queued, true)
    cancellation.abort()
    assert.equal(admission.activeRequests, 1, 'the owner releases its own slot')
    admission.release()
    assert.equal(admission.activeRequests, 0)
  })

  it('refuses release without slot ownership', () => {
    const admission = new WorkerAdmissionController({
      maxConcurrentRequests: 1,
      maxQueuedRequests: 1,
    })
    assert.throws(() => admission.release(), /without an active request/)
  })
})
