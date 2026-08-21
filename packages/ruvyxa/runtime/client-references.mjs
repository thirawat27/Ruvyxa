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
 * This module must stay dependency-free apart from `node:crypto`: it is read by
 * `compiler.mjs`, which is copied into worker and function directories where
 * nothing else is resolvable.
 */

import { createHash } from 'node:crypto'

/** Global the browser registers evaluated client modules into, keyed by id. */
export const CLIENT_MODULE_REGISTRY_GLOBAL = '__RUVYXA_CLIENT_MODULES__'

/** Prefix every Ruvyxa client-reference id carries, so a stray id is obvious. */
export const CLIENT_MODULE_ID_PREFIX = 'ruv:'

/** The package the server graph imports its client-reference proxy from. */
export const RSC_SERVER_PACKAGE = 'react-server-dom-webpack/server.node'

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
 * The browser module that publishes one client module under its id.
 *
 * `__webpack_require__(id)` reads this registry. The entry is the module
 * namespace, so React resolves `#default` and every named export off it.
 */
export function clientRegistrationSource(id, importPath) {
  if (!isClientModuleId(id)) {
    throw new Error(`RUV1860 invalid client reference id: ${JSON.stringify(id)}`)
  }
  return [
    `import * as __ruvyxaClientModule from ${JSON.stringify(importPath)}`,
    `;(globalThis.${CLIENT_MODULE_REGISTRY_GLOBAL} ||= {})[${JSON.stringify(id)}] = __ruvyxaClientModule`,
    '',
  ].join('\n')
}

/**
 * The browser prelude that lets React resolve a client reference.
 *
 * `react-server-dom-webpack/client` asks for exactly two globals, and they are
 * the whole of webpack's contract: load the chunks a reference names, then hand
 * back the module for its id. Neither has anything webpack-specific about it —
 * this is a chunk loader and a registry — so a fifteen-line shim over Ruvyxa's
 * own registry satisfies it. The `webpack` variant is used because it is the
 * only one of the three React publishes whose Node server build runs without
 * webpack's own runtime present, which was measured rather than assumed.
 *
 * `__webpack_chunk_load__` is idempotent per URL: React asks for a chunk once
 * per reference, and two references in the same module would otherwise evaluate
 * it twice and register two module instances.
 */
export function clientReferenceRuntimePrelude() {
  return `const __ruvyxaChunks = new Map();
globalThis.__webpack_chunk_load__ = (chunk) => {
  let pending = __ruvyxaChunks.get(chunk);
  if (!pending) {
    pending = import(chunk);
    __ruvyxaChunks.set(chunk, pending);
  }
  return pending;
};
globalThis.__webpack_require__ = (id) => {
  const registry = globalThis.${CLIENT_MODULE_REGISTRY_GLOBAL};
  const found = registry && registry[id];
  if (!found) {
    throw new Error(
      "RUV1861 client reference " + id + " was not registered; its chunk did not load or did not register itself",
    );
  }
  return found;
};
`
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
