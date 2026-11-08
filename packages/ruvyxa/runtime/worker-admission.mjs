/**
 * Bounded FIFO admission control for the JavaScript render worker.
 *
 * This module deliberately knows nothing about NDJSON, rendering, or process
 * shutdown. `worker-pool.mjs` owns those concerns and uses this class to keep
 * admission state transitions small, synchronous, and directly testable.
 */

export class WorkerAdmissionController {
  #activeRequests = 0
  #closed = false
  #queue = []
  #rejectedRequests = 0

  /**
   * @param {{ maxConcurrentRequests: number, maxQueuedRequests: number }} options
   */
  constructor({ maxConcurrentRequests, maxQueuedRequests }) {
    assertPositiveInteger('maxConcurrentRequests', maxConcurrentRequests)
    assertPositiveInteger('maxQueuedRequests', maxQueuedRequests)
    this.maxConcurrentRequests = maxConcurrentRequests
    this.maxQueuedRequests = maxQueuedRequests
  }

  /** Number of requests that currently own an execution slot. */
  get activeRequests() {
    return this.#activeRequests
  }

  /**
   * Acquire an execution slot or join the bounded FIFO wait queue.
   *
   * `false` means the queue is full, the controller is closed, or `signal`
   * aborted before a slot was handed over. Neither closing nor a withdrawal is
   * counted as overload: both are intentional, not a sign of saturation.
   *
   * A waiter whose `signal` aborts leaves the queue on the same turn. Without
   * that, a request the host had abandoned kept its place until a slot freed
   * and only then noticed — so the bound counted requests nobody was waiting
   * for and turned live ones away. An abort after admission changes nothing
   * here: the owner still holds the slot and releases it itself.
   *
   * @param {AbortSignal} [signal]
   * @returns {true | false | Promise<boolean>}
   */
  acquire(signal) {
    if (this.#closed || signal?.aborted) return false
    if (this.#activeRequests < this.maxConcurrentRequests) {
      this.#activeRequests++
      return true
    }
    if (this.#queue.length >= this.maxQueuedRequests) {
      this.#rejectedRequests++
      return false
    }
    return new Promise((resolve) => {
      const withdraw = () => {
        const index = this.#queue.indexOf(settle)
        if (index === -1) return
        this.#queue.splice(index, 1)
        resolve(false)
      }
      const settle = (admitted) => {
        signal?.removeEventListener('abort', withdraw)
        resolve(admitted)
      }
      this.#queue.push(settle)
      signal?.addEventListener('abort', withdraw, { once: true })
    })
  }

  /** Release one owned slot and admit the oldest waiter, if any. */
  release() {
    if (this.#activeRequests === 0) {
      throw new Error('Worker admission release called without an active request')
    }
    const next = this.#queue.shift()
    if (next) {
      // Ownership transfers directly; the active count stays unchanged.
      next(true)
      return
    }
    this.#activeRequests--
  }

  /**
   * Stop future admission and settle every parked waiter as not admitted.
   * Active owners keep their slots until they finish and call `release()`.
   */
  close() {
    if (this.#closed) return
    this.#closed = true
    const queued = this.#queue.splice(0)
    for (const settle of queued) settle(false)
  }

  /** Return an immutable operational snapshot for health checks and metrics. */
  snapshot() {
    return Object.freeze({
      activeRequests: this.#activeRequests,
      queuedRequests: this.#queue.length,
      maxConcurrentRequests: this.maxConcurrentRequests,
      maxQueuedRequests: this.maxQueuedRequests,
      rejectedRequests: this.#rejectedRequests,
      admissionClosed: this.#closed,
    })
  }
}

function assertPositiveInteger(name, value) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new TypeError(`${name} must be a positive safe integer`)
  }
}
