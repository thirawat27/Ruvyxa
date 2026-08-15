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
 * Install the reader half on `globalThis`.
 *
 * Assigned unconditionally rather than with `??=`: two copies of this module in
 * one process share the `globalThis` key but not `storage`, and the last one
 * loaded is the one whose `runWithRequestContext` the host will call. Keeping
 * the reader paired with the most recently installed storage is what makes that
 * consistent.
 */
globalThis.__RUVYXA_REQUEST_CONTEXT__ = {
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
export function requestContext({ headerPairs, headers, method = 'GET', url = '/' } = {}) {
  const pairs = Array.isArray(headerPairs)
    ? headerPairs.map(([name, value]) => [String(name), String(value)])
    : Object.entries(headers ?? {}).map(([name, value]) => [String(name), String(value)])

  return {
    headers: pairs,
    method: String(method).toUpperCase(),
    url: String(url),
    draft: hasDraftCookie(pairs),
    used: false,
    // URLs `revalidatePath()` asked the server to refresh. A Set because the
    // same path revalidated twice in one handler is one instruction, and the
    // host has to send each one across the worker protocol.
    revalidate: new Set(),
  }
}

/** URLs this request asked to revalidate, for the host to act on. */
export function collectRevalidations(context) {
  return context?.revalidate ? [...context.revalidate] : []
}

/** Run `task` with `context` as the ambient request. */
export function runWithRequestContext(context, task) {
  return storage.run(context, task)
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
