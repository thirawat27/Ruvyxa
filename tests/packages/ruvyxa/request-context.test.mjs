/**
 * The per-request store behind `cookies()`, `headers()`, and `draftMode()`.
 *
 * The two halves of this feature live in files that cannot import each other:
 * `@ruvyxa/core/src/server.ts` declares the accessors and is bundled
 * for edge targets, while `packages/ruvyxa/runtime/request-context.mjs` owns
 * the storage and is copied into function bundles that resolve no bare
 * specifiers. They agree only on a `globalThis` key and a cookie name. Nothing
 * but a test can hold that agreement, so the first suite below does.
 */

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, it } from 'node:test'

import {
  collectRevalidations,
  DRAFT_MODE_COOKIE as RUNTIME_DRAFT_COOKIE,
  collectRevalidatedTags,
  requestContext,
  runWithRequestContext,
  usedRequestContext,
} from '../../../packages/ruvyxa/runtime/request-context.mjs'
import {
  DRAFT_MODE_COOKIE as CORE_DRAFT_COOKIE,
  cookies,
  draftMode,
  headers,
  parseCookieHeader,
  revalidatePath,
  revalidateTag,
} from '../../../packages/@ruvyxa/core/dist/server.js'

const CONTEXT_KEY = '__RUVYXA_REQUEST_CONTEXT__'
const DATA_CACHE_KEY = '__RUVYXA_DATA_CACHE__'
const REVALIDATION_CONFORMANCE = JSON.parse(
  sourceOf('../../fixtures/revalidation-conformance.json'),
)

function sourceOf(relative) {
  return readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')
}

describe('the two halves agree', () => {
  it('uses the same draft cookie name on both sides', () => {
    assert.equal(RUNTIME_DRAFT_COOKIE, CORE_DRAFT_COOKIE)
    assert.equal(CORE_DRAFT_COOKIE, '__ruvyxa_draft')
  })

  it('uses the same globalThis key on both sides', () => {
    // A rename on one side alone would not fail to build or import; it would
    // make every accessor report "called outside a request" at runtime.
    for (const file of [
      '../../../packages/@ruvyxa/core/src/server.ts',
      '../../../packages/ruvyxa/runtime/request-context.mjs',
    ]) {
      assert.ok(sourceOf(file).includes(CONTEXT_KEY), `${file} must reference ${CONTEXT_KEY}`)
    }
    assert.equal(typeof globalThis[CONTEXT_KEY]?.current, 'function')
  })

  // The second handshake, and the same problem. `cache()` reads a project's
  // shared data store off `globalThis` because `server.ts` is bundled for edge
  // targets and cannot import from `runtime/`; the registry prelude in
  // `adapter-runner.mjs` is what puts it there. A rename in one file and not
  // the other produces a deployment whose `cache.handler` is never consulted
  // and whose build reports nothing — every instance quietly caching alone.
  it('uses the same shared-data-cache key on both sides', () => {
    for (const file of [
      '../../../packages/@ruvyxa/core/src/server.ts',
      '../../../packages/ruvyxa/runtime/adapter-runner.mjs',
    ]) {
      assert.ok(sourceOf(file).includes(DATA_CACHE_KEY), `${file} must reference ${DATA_CACHE_KEY}`)
    }
  })
})

describe('cookie parsing', () => {
  it('splits pairs and trims whitespace', () => {
    assert.deepEqual(parseCookieHeader('a=1; b=2'), [
      { name: 'a', value: '1' },
      { name: 'b', value: '2' },
    ])
  })

  it('keeps a value containing "=" intact', () => {
    // Base64 and JWT cookie values routinely end in padding.
    assert.deepEqual(parseCookieHeader('token=abc=='), [{ name: 'token', value: 'abc==' }])
  })

  it('unwraps one layer of quoting', () => {
    assert.deepEqual(parseCookieHeader('a="hello world"'), [{ name: 'a', value: 'hello world' }])
  })

  it('skips malformed pairs rather than throwing', () => {
    // The header is attacker-controlled; a page must not fail to render
    // because something wrote junk into it.
    assert.deepEqual(parseCookieHeader('broken; =novalue; a=1'), [{ name: 'a', value: '1' }])
  })

  it('returns nothing for an empty header', () => {
    assert.deepEqual(parseCookieHeader(''), [])
  })
})

describe('accessors inside a request', () => {
  const context = () =>
    requestContext({
      headerPairs: [
        ['cookie', 'theme=dark; session=abc'],
        ['x-forwarded-for', '203.0.113.1'],
        ['accept-language', 'th-TH'],
      ],
      method: 'get',
      url: '/dashboard',
    })

  it('reads cookies', () => {
    runWithRequestContext(context(), () => {
      assert.equal(cookies().get('theme'), 'dark')
      assert.equal(cookies().get('session'), 'abc')
      assert.equal(cookies().get('missing'), undefined)
      assert.ok(cookies().has('theme'))
      assert.deepEqual(
        cookies()
          .getAll()
          .map((entry) => entry.name),
        ['theme', 'session'],
      )
    })
  })

  it('reads headers as a standard Headers', () => {
    runWithRequestContext(context(), () => {
      assert.equal(headers().get('accept-language'), 'th-TH')
      assert.equal(headers().get('Accept-Language'), 'th-TH', 'lookup is case-insensitive')
      assert.equal(headers().get('x-missing'), null)
    })
  })

  it('reports draft mode from the cookie', () => {
    runWithRequestContext(context(), () => {
      assert.equal(draftMode().isEnabled, false)
    })
    const drafting = requestContext({
      headerPairs: [['cookie', `theme=dark; ${CORE_DRAFT_COOKIE}=1`]],
    })
    runWithRequestContext(drafting, () => {
      assert.equal(draftMode().isEnabled, true)
    })
  })

  it('survives an await, so a render that suspends keeps its request', async () => {
    await runWithRequestContext(context(), async () => {
      await new Promise((resolve) => setTimeout(resolve, 1))
      assert.equal(cookies().get('theme'), 'dark')
    })
  })

  it('keeps concurrent requests apart', async () => {
    // The failure this guards against is a cross-user data leak, which a
    // plain module-level variable would produce the first time two renders
    // interleave.
    const read = (value) =>
      runWithRequestContext(
        requestContext({ headerPairs: [['cookie', `who=${value}`]] }),
        async () => {
          await new Promise((resolve) => setTimeout(resolve, value === 'a' ? 10 : 1))
          return cookies().get('who')
        },
      )

    assert.deepEqual(await Promise.all([read('a'), read('b')]), ['a', 'b'])
  })
})

describe('use tracking', () => {
  it('reports nothing used when no accessor is called', () => {
    const store = requestContext({ headerPairs: [['cookie', 'a=1']] })
    runWithRequestContext(store, () => 'rendered without reading anything')
    assert.equal(usedRequestContext(store), false, 'such a page stays cacheable')
  })

  it('reports used after any accessor reads the request', () => {
    for (const read of [() => cookies().get('a'), () => headers().get('a'), () => draftMode()]) {
      const store = requestContext({ headerPairs: [['cookie', 'a=1']] })
      runWithRequestContext(store, read)
      assert.equal(usedRequestContext(store), true)
    }
  })
})

describe('outside a request', () => {
  it('throws an actionable error rather than returning empty data', () => {
    // Returning empty cookies here would turn "I called this at module scope"
    // into a logged-out page with no explanation.
    for (const [name, accessor] of [
      ['cookies()', cookies],
      ['headers()', headers],
      ['draftMode()', draftMode],
    ]) {
      assert.throws(accessor, (error) => {
        assert.match(error.message, new RegExp(`^${name.replace('()', '\\(\\)')} was called`))
        assert.match(error.message, /move the call inside the component or handler/)
        return true
      })
    }
  })
})

describe('revalidateTag', () => {
  // It used to clear this process's `cache()` store and stop there. For one
  // container that is the whole job; for an application running several
  // instances behind one domain it clears the instance that served the mutation
  // and leaves every other one answering from the entry it just invalidated.
  // Queuing the tag is what lets the host hand it to a shared store, which is
  // what `CacheHandler.revalidateTag` is for in Next.js.
  it('collects tags for the host to hand to the shared store', () => {
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => {
      revalidateTag('products')
      revalidateTag('reviews')
    })
    assert.deepEqual(collectRevalidatedTags(store), ['products', 'reviews'])
  })

  it('collapses a tag revalidated more than once', () => {
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => {
      revalidateTag('products')
      revalidateTag('products')
    })
    assert.deepEqual(collectRevalidatedTags(store), ['products'])
  })

  // The path queue and the tag queue name different things — one document
  // against whatever the application labelled — and collapsing them would make
  // a tag drop a page nobody asked to drop.
  it('does not queue a path, and revalidatePath does not queue a tag', () => {
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => {
      revalidateTag('products')
      revalidatePath('/blog/hello')
    })
    assert.deepEqual(collectRevalidatedTags(store), ['products'])
    assert.deepEqual(collectRevalidations(store), ['/blog/hello'])
  })

  it('does not make the caller uncacheable', () => {
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => revalidateTag('products'))
    assert.equal(usedRequestContext(store), false)
  })

  // Callable at module scope and from a background task since it existed.
  // Adding a queue must not turn that into an error — there is no response to
  // attach one to, and the local invalidation is still the whole behaviour.
  it('stays a local invalidation outside a request', () => {
    assert.doesNotThrow(() => revalidateTag('products'))
  })
})

describe('revalidatePath', () => {
  it('collects absolute paths for the host to act on', () => {
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => {
      revalidatePath('/blog/hello')
      revalidatePath('/blog/world')
    })
    assert.deepEqual(collectRevalidations(store), ['/blog/hello', '/blog/world'])
  })

  it('collapses a path revalidated more than once', () => {
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => {
      revalidatePath('/blog/hello')
      revalidatePath('/blog/hello')
    })
    assert.deepEqual(collectRevalidations(store), ['/blog/hello'])
  })

  it('does not make the caller uncacheable', () => {
    // Queuing a revalidation says nothing about who sent the request, so it
    // must not trip the flag that keeps a personalised render out of the cache.
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => revalidatePath('/blog/hello'))
    assert.equal(usedRequestContext(store), false)
  })

  it('rejects a route pattern and a relative path', () => {
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => {
      for (const bad of ['blog/hello', './hello', '']) {
        assert.throws(() => revalidatePath(bad), /needs an absolute path/)
      }
      // A pattern is absolute, so it passes the check — and is still wrong.
      // The error text says so; the type system cannot.
      assert.doesNotThrow(() => revalidatePath('/blog/[slug]'))
    })
  })

  it('fails explicitly when one request exceeds revalidation bounds', () => {
    const store = requestContext({ headerPairs: [] })
    runWithRequestContext(store, () => {
      for (let index = 0; index < REVALIDATION_CONFORMANCE.maxPathsPerRequest; index++) {
        revalidatePath(`/posts/${index}`)
      }
      // Repeating an already queued path does not consume another slot.
      assert.doesNotThrow(() => revalidatePath('/posts/0'))
      assert.throws(
        () => revalidatePath('/posts/overflow'),
        new RegExp(`at most ${REVALIDATION_CONFORMANCE.maxPathsPerRequest} distinct paths`),
      )
      assert.throws(
        () => revalidatePath(`/${'x'.repeat(REVALIDATION_CONFORMANCE.maxPathLength)}`),
        new RegExp(`at most ${REVALIDATION_CONFORMANCE.maxPathLength} characters`),
      )
    })
    assert.equal(collectRevalidations(store).length, REVALIDATION_CONFORMANCE.maxPathsPerRequest)
  })

  it('throws outside a request', () => {
    assert.throws(() => revalidatePath('/blog/hello'), /was called outside a request/)
  })
})
