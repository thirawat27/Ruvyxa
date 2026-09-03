/**
 * The per-request store behind `cookies()`, `headers()`, and `draftMode()`.
 *
 * `@ruvyxa/core/server` declares those accessors but deliberately creates no
 * store: it is bundled for edge and browser targets where `node:async_hooks`
 * does not exist. This module is the Node-side half. Importing it installs the
 * store on `globalThis`, which is where both copies of the accessor module —
 * the one aliased into the SSR bundle and the one a dependency resolved from
 * `dist` — look for it.
 *
 * ## Why `AsyncLocalStorage` rather than a variable
 *
 * A worker serves one request at a time today, but a page render is a chain of
 * awaits and nothing in the protocol promises that stays true. A plain variable
 * assigned before `render()` and cleared after would hand request B's cookies
 * to request A the first time two renders interleave — a cross-user data leak
 * that would appear only under load. `AsyncLocalStorage` makes the store follow
 * the call stack instead, so the guarantee does not depend on the pool's
 * scheduling.
 *
 * ## Recording use
 *
 * Each store carries a `used` flag set by the first accessor call. That is what
 * tells the host the render depended on this particular request, and therefore
 * that its HTML must not be stored in a cache shared with other users. Detecting
 * it at call time rather than by scanning source is both exact and free: no
 * import pattern to recognize, no false positive from the word `cookies` in a
 * comment, and nothing to pay on the pages that never call it.
 */

/** Cookie that enables draft mode. Must match `DRAFT_MODE_COOKIE` in core. */
export const DRAFT_MODE_COOKIE = '__ruvyxa_draft'

/**
 * `AsyncLocalStorage`, or a stand-in for runtimes without `node:async_hooks`.
 *
 * Some deployment targets — Cloudflare Workers without `nodejs_compat`, Deno
 * Deploy — do not provide it. The stand-in keeps one context in a variable,
 * which is correct as long as renders do not interleave, and *refuses* rather
 * than guesses when they do: a nested `run` throws instead of letting the inner
 * request read the outer request's cookies. Serving the wrong user's page is
 * the one outcome that must not be possible, so the fallback fails loudly at
 * the moment it cannot keep that promise.
 */
const storage = await createStorage()

async function createStorage() {
  try {
    const { AsyncLocalStorage } = await import('node:async_hooks')
    return new AsyncLocalStorage()
  } catch {
    let active = null
    return {
      getStore: () => active,
      run(store, task) {
        if (active) {
          throw new Error(
            'Ruvyxa cannot isolate concurrent requests on this runtime: it provides no ' +
              'node:async_hooks, and two renders are in flight at once. Enable Node.js ' +
              'compatibility for this deployment target, or stop reading cookies(), headers(), ' +
              'and draftMode() during rendering.',
          )
        }
        active = store
        try {
          const result = task()
          // A promise must hold the context until it settles, and without
          // AsyncLocalStorage the only way to express that is to keep the
          // variable set for exactly that long.
          if (result && typeof result.then === 'function') {
            return result.finally(() => {
              active = null
            })
          }
          active = null
          return result
        } catch (error) {
          active = null
          throw error
        }
      },
    }
  }
}

/**
 * Install the reader *and* the writer on `globalThis`.
 *
 * Assigned unconditionally rather than with `??=`: two copies of this module in
 * one process share the `globalThis` key but not `storage`. This used to install
 * the reader alone, on the stated premise that "the last one loaded is the one
 * whose `runWithRequestContext` the host will call" -- which is an assumption
 * about module load order, not something the code could enforce. A function
 * bundle can carry both the copy aliased into the SSR bundle and a second copy
 * reached through a dependency's `dist`, and nothing decides which of those the
 * host imported from.
 *
 * If that order ever inverted, the reader would be looking at one copy's storage
 * while the host filled the other's: `cookies()`, `headers()` and `draftMode()`
 * would throw "was called outside a request" for every request in a deployed
 * build, and -- the dangerous half -- `usedRequestContext` would report `false`,
 * letting a request-scoped render be stored in a cache shared with other users.
 * It fails closed on the accessors and open on the cacheability flag.
 *
 * So `run` is installed beside the readers and `runWithRequestContext` goes
 * through it. Whichever copy wins the assignment then owns both halves, and the
 * pair cannot be split no matter which copy the host holds.
 */
globalThis.__RUVYXA_REQUEST_CONTEXT__ = {
  /** The writer half, so it cannot be separated from the readers below. */
  run(context, task) {
    return storage.run(context, task)
  },
  current() {
    const store = storage.getStore()
    if (!store) return null
    // Reading request state is what makes a render uncacheable. `revalidatePath`
    // also goes through here but must not set this: it writes an instruction for
    // the host and tells you nothing about who sent the request.
    store.used = true
    return store
  },
  /** Reach the store without recording a read. */
  peek() {
    return storage.getStore() ?? null
  },
  wasRead() {
    return storage.getStore()?.used === true
  },
}

/**
 * Build a context from a worker request.
 *
 * `headerPairs` is the protocol's ordered header list. The collapsed `headers`
 * object is accepted as a fallback so a request framed by an older host still
 * produces a usable context rather than an empty one.
 */
export function requestContext({ headerPairs, headers, method = 'GET', url = '/', params } = {}) {
  const pairs = Array.isArray(headerPairs)
    ? headerPairs.map(([name, value]) => [String(name), String(value)])
    : Object.entries(headers ?? {}).map(([name, value]) => [String(name), String(value)])

  return {
    headers: pairs,
    method: String(method).toUpperCase(),
    url: String(url),
    draft: hasDraftCookie(pairs),
    // Only pages carry route parameters. Left undefined elsewhere on purpose:
    // `params()` distinguishes "not a page" from "a page with no parameters",
    // and an empty object here would collapse the two.
    ...(params ? { params: Object.freeze({ ...params }) } : {}),
    used: false,
    // URLs `revalidatePath()` asked the server to refresh. A Set because the
    // same path revalidated twice in one handler is one instruction, and the
    // host has to send each one across the worker protocol.
    revalidate: new Set(),
    // Tags `revalidateTag()` asked the server to drop from the shared store.
    //
    // Separate from `revalidate` because the two mean different things: a path
    // names one document, a tag names whatever the project labelled with it,
    // and only the project's own store knows what that is. Both travel the same
    // way — collected after the response, acted on by the host — because a
    // store write that happens before the response is a write that a failed
    // request still performed.
    revalidateTags: new Set(),
    // Keys `invalidateCache()` asked the server to drop from the shared store.
    //
    // An array rather than a Set because each entry is an `{ key?, prefix }`
    // pair: `invalidateCache('products')` clears `products` and everything
    // under `products:` but not `productsXYZ`, which one string cannot say.
    // `invalidateCache()` de-duplicates against what is already queued.
    invalidatedKeys: [],
  }
}

/** URLs this request asked to revalidate, for the host to act on. */
export function collectRevalidations(context) {
  return context?.revalidate ? [...context.revalidate] : []
}

/** Tags this request asked to revalidate, for the host to act on. */
export function collectRevalidatedTags(context) {
  return context?.revalidateTags ? [...context.revalidateTags] : []
}

/** Shared-store keys this request asked to drop, for the host to act on. */
export function collectCacheInvalidations(context) {
  return Array.isArray(context?.invalidatedKeys) ? [...context.invalidatedKeys] : []
}

/**
 * Run `task` with `context` as the ambient request.
 *
 * Through the installed object rather than this copy's own `storage`, so a
 * second copy of this module cannot end up filling one store while the
 * accessors read another. The fallback covers a host that installed an older
 * shape with no `run`, where this copy's storage is the only one there is.
 */
export function runWithRequestContext(context, task) {
  const installed = globalThis.__RUVYXA_REQUEST_CONTEXT__
  return typeof installed?.run === 'function'
    ? installed.run(context, task)
    : storage.run(context, task)
}

/**
 * Did anything in this render read request state?
 *
 * The host asks after the render so it can decide whether the output is
 * cacheable. A context built but never read reports `false`, which is the
 * common case and the one that must stay fast.
 */
export function usedRequestContext(context) {
  return Boolean(context?.used)
}

function hasDraftCookie(pairs) {
  for (const [name, value] of pairs) {
    if (name.toLowerCase() !== 'cookie') continue
    for (const part of String(value).split(';')) {
      const separator = part.indexOf('=')
      if (separator <= 0) continue
      if (part.slice(0, separator).trim() === DRAFT_MODE_COOKIE) return true
    }
  }
  return false
}
