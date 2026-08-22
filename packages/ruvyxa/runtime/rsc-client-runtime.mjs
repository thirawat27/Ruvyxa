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
 * Whether a string is a server-reference module id this framework produced.
 *
 * Here rather than beside `serverModuleId`, which needs `node:crypto` and so
 * lives in `client-references.mjs`: the generated entries that resolve a call
 * run in bundles this file is inlined into, and a second spelling of the id
 * shape would be a second answer to what a valid reference looks like.
 */
export function isServerModuleId(value) {
  return typeof value === 'string' && /^ruv:s_[a-f0-9]{16}$/.test(value)
}

/**
 * Split the `"<module>#<export>"` id a server reference carries.
 *
 * `registerServerReference(fn, id, name)` composes it with a `#` and
 * `createServerReference` hands the whole string back to `callServer`, so this
 * is the only place that composition is taken apart. `lastIndexOf` because a
 * module id is fixed-width and an export name is not.
 *
 * @returns {{ module: string, name: string }|null} `null` when the string is
 *   not a server reference this framework produced.
 */
export function parseServerReference(reference) {
  if (typeof reference !== 'string') return null
  const separator = reference.lastIndexOf('#')
  if (separator < 0) return null
  const module = reference.slice(0, separator)
  const name = reference.slice(separator + 1)
  if (!isServerModuleId(module) || name.length === 0) return null
  return { module, name }
}

/** Global holding `'use server'` modules whose exports are not registered yet. */
export const PENDING_SERVER_MODULES_GLOBAL = '__RUVYXA_PENDING_SERVER_MODULES__'

/**
 * Note a `'use server'` module whose exports must become server references.
 *
 * Called by the module itself, at the bottom of its own body, in the
 * `react-server` graph — see `serverRegistrationSource`. It only *enqueues*,
 * because Ruvyxa's linker emits a module's `__exports.name = name` assignments
 * after its body: read at this point the exports object is still empty, and the
 * thunk is what defers the read to {@link flushServerModules}.
 *
 * `register` travels with the entry rather than being imported here, because
 * this module has no imports: it is inlined into browser bundles, where
 * `react-server-dom-webpack/server` neither exists nor could be loaded.
 */
export function enqueueServerModule(id, exports, register, scope = globalThis) {
  const pending = (scope[PENDING_SERVER_MODULES_GLOBAL] ??= [])
  pending.push({ id, exports, register })
}

/**
 * Register every enqueued module's function exports, and empty the queue.
 *
 * Two things happen per module, and both are needed: React is told which
 * exports are callable references, and the namespace joins the same registry
 * client references live in, so the one `__webpack_require__` installed above
 * answers for both kinds.
 *
 * Called before anything is rendered or any reference is resolved — the
 * generated `react-server` entry does it at the top of `flight()`, and the
 * action entry before it looks an id up. Repeated calls are cheap: the queue is
 * drained, so a second render registers nothing again.
 *
 * Non-function exports are left alone. An actions file may export a constant
 * beside its functions, and registering one would attach `$$typeof` to a value
 * a server component reads as data.
 */
export function flushServerModules(scope = globalThis) {
  const pending = scope[PENDING_SERVER_MODULES_GLOBAL]
  if (!pending || pending.length === 0) return
  const registry = (scope[CLIENT_MODULE_REGISTRY_GLOBAL] ??= Object.create(null))
  scope[PENDING_SERVER_MODULES_GLOBAL] = []
  for (const { id, exports, register } of pending) {
    const namespace = exports()
    registry[id] = namespace
    for (const name of Object.keys(namespace)) {
      const value = namespace[name]
      if (typeof value === 'function') register(value, id, name)
    }
  }
}

/**
 * The function one `"<module>#<export>"` reference names, in this bundle.
 *
 * Every failure is its own message because they mean different things to
 * whoever reads the log: a malformed id is a request that did not come from
 * this framework, an unregistered module is a call routed to a bundle that
 * cannot answer it, and a non-function export is a client asking to call
 * something that was never a server function.
 */
export function resolveServerReference(reference, scope = globalThis) {
  const parsed = parseServerReference(reference)
  if (!parsed) {
    throw new Error(`RUV1865 ${JSON.stringify(reference)} is not a server function reference`)
  }
  const namespace = scope[CLIENT_MODULE_REGISTRY_GLOBAL]?.[parsed.module]
  if (!namespace) {
    throw new Error(
      `RUV1865 server function ${reference} belongs to a module this route does not reach`,
    )
  }
  const found = namespace[parsed.name]
  if (typeof found !== 'function') {
    throw new Error(`RUV1865 server function ${reference} names an export that is not a function`)
  }
  return found
}

/**
 * Route the browser posts a server-function call to.
 *
 * The same endpoint that serves a route's payload, because it is the same
 * question asked twice: `GET` renders the route, `POST` runs one of its server
 * functions and returns what that function produced. Making it a second path
 * would mean a second reserved route, a second entry in every host's reserved
 * list, and a second place the same-origin header is checked.
 */
export const RSC_ENDPOINT = '/__ruvyxa/rsc'

/** Header naming the server function a `POST` to {@link RSC_ENDPOINT} calls. */
export const SERVER_ACTION_HEADER = 'x-ruvyxa-action'

/** Header that keeps {@link RSC_ENDPOINT} out of reach of a cross-origin page. */
export const RSC_REQUEST_HEADER = 'x-ruvyxa-rsc'

/**
 * Build the `callServer` React hands every server reference in this realm.
 *
 * React's contract is one function: `callServer(id, args)` returns a promise of
 * what the server function returned. Everything about *how* — the endpoint, the
 * headers, which route the call belongs to — is Ruvyxa's to decide, and this is
 * where it is decided.
 *
 * The route pattern travels with the call because the host needs it to know
 * which graph to look the id up in: a server function is reachable from the
 * route whose page or client components import it, and asking for the route is
 * cheaper than maintaining a build-wide table of every action in the app.
 *
 * The reply is itself a Flight payload, so `createFromFetch` decodes it — which
 * is what lets a server function return an element tree and not just data.
 */
export function createServerCaller({ encodeReply, createFromFetch, scope = globalThis }) {
  return async function callServer(id, args) {
    const body = await encodeReply(args)
    const route = scope.__RUVYXA_ROUTE_PATTERN__ ?? scope.location?.pathname ?? '/'
    const headers = { [RSC_REQUEST_HEADER]: '1', [SERVER_ACTION_HEADER]: id }
    // `encodeReply` returns a string for plain arguments and `FormData` when
    // one of them is a file or a stream. A string needs a content type; letting
    // the browser set the multipart boundary for the other is the only way it
    // can be parsed on the far side, so it must not be given one.
    if (typeof body === 'string') headers['content-type'] = 'text/plain;charset=UTF-8'
    const response = scope.fetch(`${RSC_ENDPOINT}?path=${encodeURIComponent(route)}`, {
      method: 'POST',
      headers,
      body,
      credentials: 'same-origin',
    })
    return createFromFetch(response)
  }
}

/**
 * A namespace standing in for a `'use server'` module in a client graph.
 *
 * One `createServerReference` per export name, made on first read and kept, so
 * the function React sees for `save` is the same object every render. The
 * property names below are answered as "not an export" rather than as
 * references: `then` would make the namespace look like a promise to anything
 * that awaits it, and the rest are read by module interop rather than by user
 * code.
 */
export function serverReferenceProxy(id, createServerReference, callServer) {
  const made = new Map()
  const hidden = new Set(['then', '__esModule', '$$typeof', 'constructor', 'prototype'])
  return new Proxy(Object.create(null), {
    get(_target, property) {
      if (typeof property !== 'string' || hidden.has(property)) return undefined
      let reference = made.get(property)
      if (!reference) {
        reference = createServerReference(`${id}#${property}`, callServer)
        made.set(property, reference)
      }
      return reference
    },
    has(_target, property) {
      return typeof property === 'string' && !hidden.has(property)
    },
  })
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
