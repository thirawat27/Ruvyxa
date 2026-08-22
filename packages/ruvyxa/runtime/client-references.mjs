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
export { CLIENT_MODULE_REGISTRY_GLOBAL, RSC_PAYLOAD_ELEMENT_ID } from './rsc-client-runtime.mjs'

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
 */
export function normalizeClientModulePath(relativePath) {
  return String(relativePath).replaceAll('\\', '/').replace(/^\.\//, '').replace(/^\/+/, '')
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
