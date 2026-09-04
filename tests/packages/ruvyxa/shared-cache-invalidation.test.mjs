import assert from 'node:assert/strict'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerModule = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')
const serverModule = path.join(workspaceRoot, 'packages/@ruvyxa/core/dist/server.js')

// Importing the handler installs the request-context host on `globalThis`, so
// the accessors this test calls from a route see the request the handler is
// serving. Order matters: import it before the module that reads the host.
const { createHandler } = await import(`file://${handlerModule.replaceAll('\\', '/')}`)
const { invalidateCache, revalidateTag } = await import(
  `file://${serverModule.replaceAll('\\', '/')}`
)

/**
 * Record what the project's `cache.handler` was asked to drop, and whether the
 * handler waited for it.
 *
 * `settled` is the point of the double. An invalidation that the response does
 * not wait for is an invalidation a process killed after the response never
 * performs, and the caller was already told the old value is gone.
 */
function invalidationSpy() {
  const calls = { tags: [], keys: [] }
  let settled = false
  return {
    calls,
    get settled() {
      return settled
    },
    revalidateTags: async (tags) => {
      await new Promise((resolve) => setTimeout(resolve, 5))
      calls.tags.push(tags)
      settled = true
    },
    deleteData: async (keys) => {
      await new Promise((resolve) => setTimeout(resolve, 5))
      calls.keys.push(keys)
      settled = true
    },
  }
}

const apiRoute = { id: 'api', path: '/api', kind: 'api', file: 'api.ts', render: {} }

function apiHandler(spy) {
  return createHandler({
    routes: [apiRoute],
    importPage: async () => ({}),
    importApi: async () => ({
      POST: () => {
        revalidateTag('products')
        invalidateCache('products')
        return new Response('ok')
      },
    }),
    revalidateTags: spy.revalidateTags,
    deleteData: spy.deleteData,
  })
}

describe('a mutation hands its invalidations to the project cache handler', () => {
  // `revalidateTags` has been a `createHandler` option since the shared cache
  // landed and nothing ever called the handler through it — the drain was held
  // by no test at all, in either direction.
  it('hands over every tag revalidateTag() queued', async () => {
    const spy = invalidationSpy()
    const response = await apiHandler(spy)(
      new Request('https://app.example/api', { method: 'POST' }),
    )
    assert.equal(response.status, 200)
    assert.deepEqual(spy.calls.tags, [['products']])
  })

  // The half that did not exist. Clearing the local tier and stopping there is
  // undone by the very next read: the key is gone from this process, the shared
  // store still holds it, and the miss reads it back and re-commits it under a
  // full TTL.
  it('hands over every key invalidateCache() queued, with its prefix', async () => {
    const spy = invalidationSpy()
    await apiHandler(spy)(new Request('https://app.example/api', { method: 'POST' }))
    assert.deepEqual(spy.calls.keys, [[{ key: 'products', prefix: 'products:' }]])
  })

  it('waits for the store before answering', async () => {
    const spy = invalidationSpy()
    await apiHandler(spy)(new Request('https://app.example/api', { method: 'POST' }))
    assert.equal(spy.settled, true, 'the response was returned before the store was written')
  })

  // A project that declares no handler keeps the behaviour it had: the local
  // invalidation happens and nothing else is attempted.
  it('is inert when the project declares no handler', async () => {
    const handler = createHandler({
      routes: [apiRoute],
      importPage: async () => ({}),
      importApi: async () => ({
        POST: () => {
          invalidateCache('products')
          return new Response('ok')
        },
      }),
    })
    const response = await handler(new Request('https://app.example/api', { method: 'POST' }))
    assert.equal(response.status, 200)
  })
})

describe('a server function invoked by a native form submission', () => {
  const pageRoute = {
    id: 'page',
    path: '/',
    kind: 'page',
    file: 'page.tsx',
    render: { strategy: 'ssr', serverComponents: true },
  }

  function formHandler(spy) {
    return createHandler({
      routes: [pageRoute],
      supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
      importPage: async () => ({
        render: () => {
          revalidateTag('products')
          invalidateCache('products')
          return '<!doctype html><html><body>ok</body></html>'
        },
      }),
      importApi: async () => ({}),
      revalidateTags: spy.revalidateTags,
      deleteData: spy.deleteData,
    })
  }

  function submission() {
    return new Request('https://app.example/', {
      method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: '$ACTION_ID_x=1',
    })
  }

  // The progressive-enhancement path drained `revalidatePath()` and nothing
  // else, so the same action that invalidated correctly over `/__ruvyxa/rsc`
  // dropped both `revalidateTag()` and `invalidateCache()` when the browser
  // posted the form itself — which is the path that runs with JavaScript off,
  // and the one nobody watches.
  it('hands over its tags rather than dropping them', async () => {
    const spy = invalidationSpy()
    const response = await formHandler(spy)(submission())
    assert.equal(response.status, 200)
    assert.deepEqual(spy.calls.tags, [['products']])
  })

  it('hands over its keys rather than dropping them', async () => {
    const spy = invalidationSpy()
    await formHandler(spy)(submission())
    assert.deepEqual(spy.calls.keys, [[{ key: 'products', prefix: 'products:' }]])
  })
})

describe('a server action posted to /__ruvyxa/action', () => {
  const pageRoute = {
    id: 'target',
    path: '/target',
    kind: 'page',
    file: 'page.tsx',
    render: { strategy: 'ssr' },
  }

  // The third completion site, and the one the other two describe blocks do not
  // reach. `runAction` owns the request context and returned only
  // `collectRevalidations(context)`, so this host had nothing left to hand the
  // store: `revalidatePath()` from a server action worked and `revalidateTag()`
  // and `invalidateCache()` from the same action reached nothing. The native
  // host settles all three from its own copy of the action loop, so the two
  // hosts disagreed about what a mutation invalidates.
  function actionHandler(spy) {
    const submit = async () => {
      revalidateTag('products')
      invalidateCache('products')
      return { ok: true }
    }
    submit.ruvyxa = { kind: 'action' }
    return createHandler({
      routes: [pageRoute],
      importPage: async () => ({}),
      importApi: async () => ({}),
      importAction: async () => ({ submit }),
      revalidateTags: spy.revalidateTags,
      deleteData: spy.deleteData,
    })
  }

  function invocation() {
    return new Request('http://localhost/__ruvyxa/action?path=/target&name=submit', {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        host: 'localhost',
        origin: 'http://localhost',
        'sec-fetch-site': 'same-origin',
      },
      body: '{}',
    })
  }

  it('hands over its tags rather than dropping them', async () => {
    const spy = invalidationSpy()
    const response = await actionHandler(spy)(invocation())
    assert.equal(response.status, 200)
    assert.deepEqual(spy.calls.tags, [['products']])
  })

  it('hands over its keys rather than dropping them', async () => {
    const spy = invalidationSpy()
    const response = await actionHandler(spy)(invocation())
    assert.equal(response.status, 200)
    assert.deepEqual(spy.calls.keys, [[{ key: 'products', prefix: 'products:' }]])
  })

  it('waits for the store before answering', async () => {
    const spy = invalidationSpy()
    await actionHandler(spy)(invocation())
    assert.equal(spy.settled, true, 'the response was returned before the store was written')
  })
})
