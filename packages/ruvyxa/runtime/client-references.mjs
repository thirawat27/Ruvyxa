/**
 * The identity a `'use client'` module has across the three graphs that must
 * agree about it.
 *
 * React Server Components split one application into two module graphs that
 * never share an instance. A `'use client'` module is compiled for the browser,
 * and the server graph gets a *reference* to it instead: an id React writes
 * into the Flight payload, which the browser then resolves back to the real
 * module. Three places have to spell that id the same way — the server graph
 * that emits the reference, the client build that registers the module, and the
 * manifest that maps one to the other — so it is computed here and nowhere
 * else, and `tests/packages/ruvyxa/client-references.test.mjs` pins it.
 *
 * There is no Rust mirror and therefore no cross-language fixture: the Rust
 * bundler does not build server-components graphs yet. When it does, this rule
 * gains a second implementation and needs a shared table on the day that lands,
 * not after — writing the table while adding the second implementation is what
 * caught the byte-range disagreement neither host would have noticed alone.
 *
 * The id is derived from the module's project-relative path rather than from
 * its contents: it has to survive an edit to the component, or every rebuild
 * would invalidate a payload the browser is still holding.
 *
 * This module must import nothing outside `node:` builtins and its own runtime
 * sibling: it is read by `compiler.mjs`, which is copied into worker and
 * function directories where nothing else is resolvable.
 */

import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'

// Re-exported rather than restated: the browser half of this contract lives in
// a module with no imports, because it is inlined into the client bundle, and
// two spellings of the registry global would be two registries.
import { compareCodeUnits } from './order.mjs'
import { isServerModuleId, parseServerReference } from './rsc-client-runtime.mjs'

export {
  CLIENT_MODULE_REGISTRY_GLOBAL,
  RSC_PAYLOAD_ELEMENT_ID,
  RSC_ENDPOINT,
  RSC_REQUEST_HEADER,
  SERVER_ACTION_HEADER,
} from './rsc-client-runtime.mjs'
export { isServerModuleId, parseServerReference }

/** Prefix every Ruvyxa client-reference id carries, so a stray id is obvious. */
export const CLIENT_MODULE_ID_PREFIX = 'ruv:'

/**
 * The package the server graph imports its client-reference proxy from.
 *
 * `.edge` rather than `.node`: it is the only server entry whose streams are
 * web streams, so one server graph runs unchanged on Node, Bun, Deno, and the
 * worker runtimes the adapters target. It was measured to export everything
 * `.node` does except `renderToPipeableStream` and `decodeReplyFromBusboy`,
 * neither of which this framework calls. Emitting the proxy from a *different*
 * entry than the renderer would link two copies of the server runtime into one
 * bundle, so this constant is the single answer for both.
 */
export const RSC_SERVER_PACKAGE = 'react-server-dom-webpack/server.edge'

/**
 * The package the SSR pass reads a Flight stream with.
 *
 * `.edge` for the same reason as {@link RSC_SERVER_PACKAGE}: web streams. This
 * one is resolved *without* the `react-server` condition, because turning a
 * payload into HTML is client-React work that happens to run on a server.
 */
export const RSC_SSR_PACKAGE = 'react-server-dom-webpack/client.edge'

/** The package the browser reads a Flight stream with. */
export const RSC_BROWSER_PACKAGE = 'react-server-dom-webpack/client.browser'

/**
 * Specifier generated entries import the client-reference runtime by.
 *
 * Synthetic rather than a real path, for two reasons. The bundle is compiled
 * against the *app's* resolver and `ruvyxa/runtime/...` is not one of the
 * package's exported subpaths; and an absolute path to a file outside the
 * project resolves but does not *bundle* on a server target, leaving the output
 * with a bare `D:/...` import no ESM loader accepts. An alias always bundles.
 *
 * `runtimeAliases()` in `compiler.mjs` maps it to `runtime/rsc-client-runtime.mjs`,
 * so every host that already passes those aliases can compile a
 * server-components entry without knowing this exists.
 */
export const RSC_CLIENT_RUNTIME_SPECIFIER = 'ruvyxa:rsc-client-runtime'

/**
 * How a generated route registry reaches the server-components renderer.
 *
 * An alias for the same reason `RSC_CLIENT_RUNTIME_SPECIFIER` is one: the file
 * lives outside the project, and a server target leaves an absolute path
 * external instead of bundling it — emitting an import no ESM loader accepts.
 * A deployed function has to carry the renderer inside its own bundle, because
 * it resolves no sibling specifiers at run time.
 */
export const RSC_RENDERER_SPECIFIER = 'ruvyxa:server-components'

/**
 * Absolute path of the module whose import installs the client-reference
 * globals.
 *
 * Only a browser entry imports it, and only as its first import: see
 * `rsc-client-install.mjs` for why that side effect is a file of its own.
 */
export function clientReferenceInstallPath() {
  return normalizeClientModulePath(
    fileURLToPath(new URL('./rsc-client-install.mjs', import.meta.url)),
  )
}

/**
 * Absolute path {@link RSC_CLIENT_RUNTIME_SPECIFIER} resolves to.
 *
 * A *browser* entry imports the runtime by this path rather than by the alias
 * above, because that entry is compiled by two different bundlers: this
 * package's compiler during `ruvyxa dev`, and the Rust bundler during
 * `ruvyxa build`. Only the first knows the alias. Both bundle an absolute path
 * into a browser target, so the path is the spelling both understand.
 */
export function clientReferenceRuntimePath() {
  return normalizeClientModulePath(
    fileURLToPath(new URL('./rsc-client-runtime.mjs', import.meta.url)),
  )
}

/**
 * Normalise a project-relative path to the form the id is computed from.
 *
 * Separators are forward slashes and any leading `./` is dropped, so a module
 * reached as `app/x.tsx` on one host and `app\x.tsx` on the other gets one id.
 * Case is left alone: two files differing only in case are two files on the
 * platforms that matter, and folding it would collide them.
 *
 * A module inside a package is named by the package instead: everything up to
 * and including the last `node_modules/` is dropped, and one `node_modules/`
 * is put back so a dependency can never collide with a project file of the
 * same shape. The relative path to a dependency is not stable — `ruvyxa build`
 * measures from the project and `ruvyxa start` measures from the tree staged
 * under `.ruvyxa/server`, two directories deeper — so a framework component
 * used by a layout got one id in the browser bundle and a different one in the
 * payload rendered at request time. A direct load of such a page worked and a
 * soft navigation into it failed with RUV1861.
 */
export function normalizeClientModulePath(relativePath) {
  const slashed = String(relativePath)
    .replaceAll('\\', '/')
    .replace(/^\.\//, '')
    .replace(/^\/+/, '')
  const marker = slashed.lastIndexOf('node_modules/')
  if (marker === -1) return slashed
  return `node_modules/${slashed.slice(marker + 'node_modules/'.length)}`
}

/**
 * The stable id for one `'use client'` module.
 *
 * `ruv:` + the first 16 hex characters of a SHA-256 over the normalised path.
 * Sixteen characters is the same width `flight.mjs` already validates for a
 * client reference, and the truncation is safe here for the same reason it is
 * there: the id names a module inside one build, not a security boundary.
 */
export function clientModuleId(relativePath) {
  const normalized = normalizeClientModulePath(relativePath)
  const digest = createHash('sha256').update(normalized).digest('hex').slice(0, 16)
  return `${CLIENT_MODULE_ID_PREFIX}m_${digest}`
}

/** Whether a string is an id this framework produced. */
export function isClientModuleId(value) {
  return typeof value === 'string' && /^ruv:m_[a-f0-9]{16}$/.test(value)
}

/**
 * The stable id for one `'use server'` module.
 *
 * `s_` rather than `m_`, over the same normalised path, so the two kinds of
 * reference cannot be confused for one another. They travel through the same
 * registry and the same `__webpack_require__`, and a client reference invoked
 * as a server function — or the reverse — would otherwise fail somewhere inside
 * React with an id that looks valid.
 */
export function serverModuleId(relativePath) {
  const normalized = normalizeClientModulePath(relativePath)
  const digest = createHash('sha256').update(normalized).digest('hex').slice(0, 16)
  return `${CLIENT_MODULE_ID_PREFIX}s_${digest}`
}

/**
 * The module the server graph compiles in place of a `'use client'` module.
 *
 * `createClientModuleProxy` returns one object standing in for every export, so
 * this needs no list of export names — which matters, because the names would
 * otherwise have to be scanned identically by two languages.
 *
 * It is assigned to `module.exports` rather than exported by name because
 * Ruvyxa's linker turns `import { Badge } from './counter'` into a property
 * read on the module's exports object. Making that object the proxy is what
 * lets every import form — default, named, namespace — resolve through it.
 */
export function clientProxyModuleSource(id) {
  if (!isClientModuleId(id)) {
    throw new Error(`RUV1860 invalid client reference id: ${JSON.stringify(id)}`)
  }
  return [
    `import { createClientModuleProxy as __ruvyxaClientProxy } from ${JSON.stringify(RSC_SERVER_PACKAGE)}`,
    `module.exports = __ruvyxaClientProxy(${JSON.stringify(id)})`,
    '',
  ].join('\n')
}

/**
 * What the `react-server` graph appends to a `'use server'` module.
 *
 * Unlike a `'use client'` module — which is *replaced* by a proxy, because its
 * code belongs to the other graph — a server function's code belongs here and
 * runs here. All that is added is the registration React needs to recognise its
 * exports as callable references when they appear in a payload.
 *
 * Appended rather than prefixed so the module's own line numbers, and therefore
 * its source map, are untouched. Import declarations are hoisted, so the
 * position of the one below is a formatting question rather than a semantic one.
 *
 * It enqueues rather than registers, and hands over a thunk rather than the
 * exports object, for one reason each. Ruvyxa's linker is line-based and emits
 * every `__exports.name = name` assignment *after* the module body, so code at
 * the bottom of that body sees an exports object that is still empty — the
 * enqueued thunk is read later, from `flushServerModules()`, which the entry
 * calls before it renders anything. And a module that assigns `module.exports`
 * wholesale replaces the object, so capturing the object rather than the way to
 * reach it would register the wrong one.
 *
 * The registration reads the exports instead of a list of export names, for the
 * same reason `createClientModuleProxy` needs no list: enumerating them would
 * mean scanning declarations, and every scanner in this repository that had to
 * agree with another one has drifted at least once.
 */
export function serverRegistrationSource(id, names = null) {
  if (!isServerModuleId(id)) {
    throw new Error(`RUV1864 invalid server reference id: ${JSON.stringify(id)}`)
  }
  // `module.exports` when the whole module is behind the directive, and a
  // literal of just the named functions when only some of them are. The second
  // form is what keeps a page component — which lives in the same file as an
  // inline server function and is emphatically not callable from a browser —
  // from being registered alongside it.
  const exposed = names === null ? 'module.exports' : `({ ${names.join(', ')} })`
  return [
    '',
    `import { registerServerReference as __ruvyxaServerRef } from ${JSON.stringify(RSC_SERVER_PACKAGE)}`,
    `import { enqueueServerModule as __ruvyxaEnqueueServer } from ${JSON.stringify(RSC_CLIENT_RUNTIME_SPECIFIER)}`,
    `__ruvyxaEnqueueServer(${JSON.stringify(id)}, () => ${exposed}, __ruvyxaServerRef)`,
    '',
  ].join('\n')
}

/**
 * The module a *client* graph compiles in place of a `'use server'` module.
 *
 * The mirror image of {@link clientProxyModuleSource}: there, the browser owns
 * the code and the server holds a reference; here the server owns the code and
 * the browser holds a reference. A component that imports `save` from an
 * actions file gets a function that posts its arguments and resolves to what
 * the server returned.
 *
 * Both client-side realms use this. The browser passes
 * `react-server-dom-webpack/client.browser`, and the SSR pass passes
 * `client.edge` — so a `<form action={save}>` renders the same markup in both,
 * which is the whole reason hydration matches.
 *
 * References are memoised per export name. React compares a form action by
 * identity across renders, and a proxy that minted a new function on every
 * property read would make every re-render look like a different action.
 */
export function serverProxyModuleSource(id, clientPackage) {
  if (!isServerModuleId(id)) {
    throw new Error(`RUV1864 invalid server reference id: ${JSON.stringify(id)}`)
  }
  return [
    // The proxy *is* a client module: it holds references and runs in the
    // browser. Saying so is not a trick to get past the lane rules — it is what
    // both of them read first, ahead of the filename, and the file this
    // replaced is named `actions.ts` in every codebase that follows React's own
    // convention. Without it `ruvyxa build` refuses the bundle with RUV1820 for
    // a crossing that no longer exists.
    `'use client'`,
    `import { createServerReference as __ruvyxaServerRef, createFromFetch as __ruvyxaFromFetch, encodeReply as __ruvyxaEncodeReply } from ${JSON.stringify(clientPackage)}`,
    `import { createServerCaller as __ruvyxaServerCaller, serverReferenceProxy as __ruvyxaServerProxy } from ${JSON.stringify(serverProxyRuntimeSpecifier(clientPackage))}`,
    `const __ruvyxaCallServer = __ruvyxaServerCaller({ encodeReply: __ruvyxaEncodeReply, createFromFetch: __ruvyxaFromFetch })`,
    `module.exports = __ruvyxaServerProxy(${JSON.stringify(id)}, __ruvyxaServerRef, __ruvyxaCallServer)`,
    '',
  ].join('\n')
}

/**
 * How a server-reference proxy names the runtime it calls into.
 *
 * Derived from the decoder package rather than passed in, because the two
 * always answer together: `client.browser` appears only in a browser graph and
 * `client.edge` only in a server one. A browser graph is compiled by two
 * different bundlers — this package's during `ruvyxa dev`, the Rust one during
 * `ruvyxa build` — and only the first knows the `ruvyxa:` alias, so that graph
 * gets the absolute path both understand. A server graph is only ever compiled
 * here, where an absolute path to a file outside the project would resolve but
 * stay external and leave a bare `D:/…` import behind.
 */
function serverProxyRuntimeSpecifier(clientPackage) {
  return clientPackage === RSC_BROWSER_PACKAGE
    ? clientReferenceRuntimePath()
    : RSC_CLIENT_RUNTIME_SPECIFIER
}

/**
 * The manifest `decodeReply` resolves a server reference in its arguments with.
 *
 * A client may pass one action to another — `<form action={remove.bind(null, id)}>`
 * is the ordinary way that happens — so the arguments React decodes can name a
 * reference of their own. Answering anything, like {@link clientManifest} does,
 * is what lets that work without a build-time table: `__webpack_require__`
 * resolves the module id against the same registry the registration wrote into,
 * and an id no module registered fails there with `RUV1861` rather than here
 * with a missing manifest entry.
 */
export function serverManifest() {
  return new Proxy(Object.create(null), {
    get(_target, property) {
      const parsed = parseServerReference(typeof property === 'string' ? property : '')
      if (!parsed) return undefined
      return { id: parsed.module, chunks: [], name: parsed.name }
    },
    has(_target, property) {
      return parseServerReference(typeof property === 'string' ? property : '') !== null
    },
  })
}

/**
 * The imports and statements that publish a build's client modules by id.
 *
 * Returned as two lists rather than as one module source because both callers
 * splice them into a larger generated entry: the browser bundle, and the SSR
 * pass that renders the payload to HTML before the browser has run anything.
 * One shape, spliced twice, is what keeps the two realms resolving an id to the
 * same module — the failure mode otherwise being a page that renders on the
 * server and blanks on hydration, or the reverse.
 *
 * The first import is the runtime that installs the two globals React reads.
 * Its position matters: `react-server-dom-webpack/client.browser` touches
 * `__webpack_require__.u` while its own module body runs, and the linker
 * evaluates a module's dependencies in the order they are imported.
 *
 * `import * as` — the whole namespace — because React resolves `#default` and
 * every named export off the registry entry, and a build cannot know which
 * names a payload will ask for.
 */
export function clientRegistrySource(references, runtimeSpecifier = RSC_CLIENT_RUNTIME_SPECIFIER) {
  const imports = [
    `import { registerClientModule as __ruvyxaRegisterClient } from ${JSON.stringify(runtimeSpecifier)}`,
  ]
  const statements = []
  references.forEach((reference, index) => {
    if (!isClientModuleId(reference.id)) {
      throw new Error(`RUV1860 invalid client reference id: ${JSON.stringify(reference.id)}`)
    }
    const local = `__ruvyxaClient${index}`
    imports.push(`import * as ${local} from ${JSON.stringify(toModuleSpecifier(reference.file))}`)
    statements.push(`__ruvyxaRegisterClient(${JSON.stringify(reference.id)}, ${local})`)
  })
  return { imports, statements }
}

/** Absolute path in the form a generated import statement accepts. */
function toModuleSpecifier(filePath) {
  return String(filePath).replaceAll('\\', '/')
}

/**
 * The client manifest React needs to serialise a reference.
 *
 * Keyed `"<id>#<export>"`, and React looks up every export name it encounters —
 * which it cannot know in advance. `createClientModuleProxy` answers any
 * property, so the manifest has to as well; a plain object cannot, and a Proxy
 * can. React only ever reads keys off it, so this is enough.
 */
export function clientManifest(references) {
  const chunks = new Map()
  for (const reference of references) {
    chunks.set(reference.id, reference.chunks ?? [])
  }
  return new Proxy(Object.create(null), {
    get(_target, property) {
      if (typeof property !== 'string') return undefined
      const separator = property.lastIndexOf('#')
      if (separator < 0) return undefined
      const id = property.slice(0, separator)
      const name = property.slice(separator + 1)
      if (!chunks.has(id)) return undefined
      return { id, chunks: chunks.get(id), name }
    },
    has(_target, property) {
      if (typeof property !== 'string') return false
      const separator = property.lastIndexOf('#')
      return separator >= 0 && chunks.has(property.slice(0, separator))
    },
  })
}

/**
 * Every `'use server'` module a route can reach, from both sides of it.
 *
 * The union is not an optimisation. An actions file imported by the page is in
 * the `react-server` graph and nowhere else; one imported only by a
 * `'use client'` component is in the browser graph and nowhere else, because a
 * client reference's own imports are never walked by the server graph. A call
 * may name either, so a bundle built from one list alone cannot answer half of
 * them.
 *
 * Lives here rather than beside either caller because both hosts build that
 * bundle and must agree on its contents: `worker-pool.mjs` for `ruvyxa dev`,
 * `start`, and the prerender pass, and `adapter-runner.mjs` for a deployment.
 */
export function mergeServerReferences(...lists) {
  const merged = new Map()
  for (const list of lists) {
    for (const reference of list ?? []) merged.set(reference.id, reference)
  }
  return [...merged.values()].sort((left, right) => compareCodeUnits(left.id, right.id))
}
