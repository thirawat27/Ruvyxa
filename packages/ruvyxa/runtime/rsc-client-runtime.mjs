/**
 * What a realm needs before it can resolve a client reference.
 *
 * A Flight payload names client components by id. Turning an id back into a
 * module is the bundler's job, and `react-server-dom-webpack` asks for it
 * through exactly two globals — `__webpack_chunk_load__` and
 * `__webpack_require__`. Neither has anything webpack-specific about it: one
 * loads a URL once, the other answers a registry. That is the whole contract,
 * measured against a real render rather than read from documentation, and this
 * module is Ruvyxa's side of it.
 *
 * Both realms that read a payload use this file: the browser entry, and the SSR
 * pass that turns the payload into HTML before the browser ever sees it. A
 * second implementation for the server would be a second answer to "which
 * module is this id", and the two would drift the first time an id rule
 * changed — the failure mode being a page that hydrates in the browser and
 * renders an empty shell on the server, or the reverse.
 *
 * It therefore has **no imports at all**, not even `node:` ones: the browser
 * bundle inlines it. `client-references.mjs` — which does need `node:crypto` to
 * compute ids — imports the two names below from here rather than restating
 * them, so the registry global and the payload element have one spelling.
 *
 * Installing is a **deliberate call**, never a side effect of importing this
 * file. Defining `__webpack_require__` is how a library decides it is running
 * inside webpack: `sass` reads `typeof __webpack_require__` and then reaches for
 * `__non_webpack_require__`, so merely importing the module that owns Ruvyxa's
 * reference registry used to break every SCSS build in the same process. The
 * browser entry installs through `rsc-client-install.mjs`, whose only job is
 * that side effect and which it imports before the decoder; the server installs
 * around a render, in `server-components.mjs`.
 *
 * Installation is idempotent: a page may carry more than one bundle that reaches
 * this module, and re-running it would discard the wrapper
 * `react-server-dom-webpack/client.browser` installs over
 * `__webpack_require__.u` when it loads.
 */

/** Global the realm registers evaluated client modules into, keyed by id. */
export const CLIENT_MODULE_REGISTRY_GLOBAL = '__RUVYXA_CLIENT_MODULES__'

/**
 * Element the server-rendered document carries its Flight payload in.
 *
 * `type="application/json"` — a data block, not executable script — for the
 * same reason `BOOTSTRAP_ELEMENT_ID` in `entry-templates.mjs` is one: a
 * `Content-Security-Policy` without `'unsafe-inline'` blocks an inline script,
 * and a payload that differs per request cannot be covered by a hash either.
 */
export const RSC_PAYLOAD_ELEMENT_ID = '__ruvyxa-rsc'

/** Marker recording that this realm's globals are already installed. */
const INSTALLED_FLAG = '__ruvyxaClientReferenceRuntime'

/**
 * Make `scope` able to resolve client references, and return its registry.
 *
 * `__webpack_chunk_load__` is idempotent per URL: React asks for a chunk once
 * per reference, and two references in the same chunk would otherwise evaluate
 * it twice and register two instances of the same component — which reads
 * downstream as a component whose state resets, not as a loader bug.
 *
 * `.u` is defined even though nothing here calls it. `client.browser` reads it
 * at module scope and wraps it, so leaving it undefined turns a missing chunk
 * into `webpackGetChunkFilename is not a function` several frames away from the
 * cause.
 */
export function installClientReferenceRuntime(scope = globalThis) {
  const registry = (scope[CLIENT_MODULE_REGISTRY_GLOBAL] ??= Object.create(null))
  if (scope[INSTALLED_FLAG]) return registry
  scope[INSTALLED_FLAG] = true

  const pending = new Map()
  scope.__webpack_chunk_load__ = (chunk) => {
    let loading = pending.get(chunk)
    if (!loading) {
      loading = import(chunk)
      pending.set(chunk, loading)
    }
    return loading
  }
  // The other half of webpack's pair. Code compiled by webpack and shipped
  // unbundled — `sass` is one — tests `typeof __webpack_require__` and, finding
  // it, reaches for the escape hatch webpack normally substitutes. Installing
  // one without the other leaves that code with a `ReferenceError` several
  // frames from anything Ruvyxa wrote. `createRequire` is unavailable in a
  // browser, so this half only appears where it means something.
  if (typeof scope.__non_webpack_require__ !== 'function' && typeof scope.require === 'function') {
    scope.__non_webpack_require__ = scope.require
  }
  const require_ = (id) => {
    const found = scope[CLIENT_MODULE_REGISTRY_GLOBAL]?.[id]
    if (!found) {
      throw new Error(
        `RUV1861 client reference ${id} was not registered; its chunk did not load or did not register itself`,
      )
    }
    return found
  }
  require_.u = (chunk) => chunk
  scope.__webpack_require__ = require_
  return registry
}

/**
 * Publish one evaluated module namespace under its client-reference id.
 *
 * Deliberately does *not* install the webpack globals: the registry is Ruvyxa's
 * own object and costs nothing, while the globals are a claim about the realm
 * that other libraries read. The SSR registry bundle calls this on the server,
 * where nothing may be claimed until a render actually needs it.
 *
 * Assignment rather than `??=`: a rebuild hands over a new namespace for the
 * same id, and keeping the first one would serve the pre-edit component for the
 * life of the process.
 */
export function registerClientModule(id, namespace, scope = globalThis) {
  const registry = (scope[CLIENT_MODULE_REGISTRY_GLOBAL] ??= Object.create(null))
  registry[id] = namespace
  return namespace
}

/**
 * Read the Flight payload the document was served with, or `null`.
 *
 * The element holds the payload as a JSON *string* rather than as JSON: a
 * Flight payload is line-delimited and not itself a JSON document, and quoting
 * it is what lets the same `safe_json_for_script` escaping the bootstrap block
 * already uses apply to it unchanged.
 */
export function readInlinePayload(scope = globalThis, elementId = RSC_PAYLOAD_ELEMENT_ID) {
  const element = scope.document?.getElementById(elementId)
  if (!element) return null
  try {
    const value = JSON.parse(element.textContent ?? '""')
    return typeof value === 'string' ? value : null
  } catch {
    return null
  }
}

/** A one-chunk `ReadableStream` of a payload's UTF-8 bytes. */
export function payloadStream(payload) {
  const bytes = new TextEncoder().encode(payload)
  return new ReadableStream({
    start(controller) {
      controller.enqueue(bytes)
      controller.close()
    },
  })
}
