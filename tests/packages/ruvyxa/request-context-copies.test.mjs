/**
 * Two loaded copies of `request-context.mjs` must not split the pair.
 *
 * The reader half is assigned onto `globalThis` unconditionally, last writer
 * wins. The writer half — `runWithRequestContext` — used to be per module
 * instance, so which copy the host imported decided whether the two halves
 * agreed. The module's own comment stated the premise: "the last one loaded is
 * the one whose `runWithRequestContext` the host will call". That is an
 * assumption about module load order, not something the code could enforce, and
 * a function bundle can carry both the copy aliased into the SSR bundle and a
 * second copy reached through a dependency's `dist`.
 *
 * If the order inverted, `cookies()`, `headers()` and `draftMode()` would throw
 * "was called outside a request" for every request in a deployed build — and,
 * the dangerous half, `usedRequestContext` would report `false`, letting a
 * request-scoped render be stored in a cache shared with other users. Closed on
 * the accessors, open on the cacheability flag.
 *
 * Two distinct copies are obtained with a `?v=` query on the file URL, which is
 * what makes the module registry treat them as different modules.
 */

import assert from 'node:assert/strict'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const moduleUrl = pathToFileURL(
  path.join(repoRoot, 'packages/ruvyxa/runtime/request-context.mjs'),
).href

const first = await import(`${moduleUrl}?copy=first`)
// Loaded second, so this copy's storage is the one installed on `globalThis`.
const second = await import(`${moduleUrl}?copy=second`)

describe('two copies of the request context', () => {
  it('are genuinely distinct module instances', () => {
    assert.notEqual(first.runWithRequestContext, second.runWithRequestContext)
  })

  it('lets the copy that did not win the install still fill the installed store', () => {
    const context = first.requestContext({
      headerPairs: [['cookie', 'a=1']],
      method: 'GET',
      url: '/',
    })

    const seen = first.runWithRequestContext(context, () => {
      const installed = globalThis.__RUVYXA_REQUEST_CONTEXT__
      return { current: installed.current(), read: installed.wasRead() }
    })

    assert.notEqual(
      seen.current,
      null,
      'the accessors read the installed store; a copy that filled its own would ' +
        'throw "called outside a request" for every request in a deployed build',
    )
    assert.equal(seen.current, context)
    assert.equal(seen.read, true)
  })

  it('records the read on the context, so cacheability is decided correctly', () => {
    const context = second.requestContext({ headerPairs: [], method: 'GET', url: '/' })

    // Run through the copy that did *not* install, read through the global.
    first.runWithRequestContext(context, () => {
      globalThis.__RUVYXA_REQUEST_CONTEXT__.current()
    })

    // Both copies answer the same question about the same context object.
    assert.equal(first.usedRequestContext(context), true)
    assert.equal(
      second.usedRequestContext(context),
      true,
      'a render that read request state must never be reported as cacheable',
    )
  })

  it('leaves a context that was never read reported as cacheable', () => {
    const context = second.requestContext({ headerPairs: [], method: 'GET', url: '/' })
    first.runWithRequestContext(context, () => {})
    assert.equal(second.usedRequestContext(context), false)
  })
})
