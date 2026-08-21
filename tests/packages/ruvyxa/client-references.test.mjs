/**
 * The identity and the emitted shapes a `'use client'` module carries into a
 * server-components graph.
 *
 * React Server Components split one application into two module graphs that
 * never share an instance. The server graph never compiles a `'use client'`
 * module — it emits a *reference* — and the browser resolves that reference
 * back to the real module. Three things have to agree about the id: the server
 * graph that emits it, the client build that registers it, and the manifest
 * React serialises against. They are all produced by
 * `packages/ruvyxa/runtime/client-references.mjs`, which is what this pins.
 *
 * Every shape asserted here was checked against a real
 * `react-server-dom-webpack@19.2.8` render before it was written down, not
 * inferred from its documentation.
 */
import assert from 'node:assert/strict'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const modulePath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/client-references.mjs')

const {
  CLIENT_MODULE_REGISTRY_GLOBAL,
  clientManifest,
  clientModuleId,
  clientProxyModuleSource,
  clientReferenceRuntimePrelude,
  clientRegistrationSource,
  isClientModuleId,
  normalizeClientModulePath,
} = await import(`file://${modulePath.replaceAll('\\', '/')}`)

describe('client module identity', () => {
  it('is stable for one path and different for another', () => {
    const first = clientModuleId('app/gallery/counter.tsx')
    assert.equal(first, clientModuleId('app/gallery/counter.tsx'))
    assert.notEqual(first, clientModuleId('app/gallery/counter2.tsx'))
    assert.ok(isClientModuleId(first), first)
  })

  it('does not depend on which separator the host used', () => {
    // The build walks paths on Windows and POSIX and must not produce two ids
    // for one module — the manifest lookup would miss and React would report a
    // bundler bug rather than a Ruvyxa one.
    assert.equal(
      clientModuleId('app\\gallery\\counter.tsx'),
      clientModuleId('app/gallery/counter.tsx'),
    )
    assert.equal(clientModuleId('./app/counter.tsx'), clientModuleId('app/counter.tsx'))
    assert.equal(normalizeClientModulePath('.\\a\\b.tsx'), 'a/b.tsx')
  })

  it('keeps case, because two files differing only in case are two files', () => {
    assert.notEqual(clientModuleId('app/Counter.tsx'), clientModuleId('app/counter.tsx'))
  })

  it('is derived from the path, not the contents', () => {
    // A payload the browser is already holding names ids. Deriving them from
    // module contents would invalidate every one of them on any edit.
    assert.equal(clientModuleId('app/counter.tsx'), clientModuleId('app/counter.tsx'))
  })

  it('refuses an id it did not produce', () => {
    for (const value of ['m_0123456789abcdef', 'ruv:m_XYZ', 'ruv:m_0123', '', null, 7]) {
      assert.equal(isClientModuleId(value), false, String(value))
    }
  })
})

describe('server graph emission', () => {
  it('assigns the proxy to module.exports so every import form resolves', () => {
    // Ruvyxa's linker turns `import { Badge } from './counter'` into a property
    // read on the module's exports object. Making that object the proxy is what
    // lets default, named, and namespace imports all reach React's reference —
    // and it is why this needs no list of the module's export names.
    const id = clientModuleId('app/counter.tsx')
    const source = clientProxyModuleSource(id)
    assert.match(source, /createClientModuleProxy as __ruvyxaClientProxy/)
    assert.match(source, /from "react-server-dom-webpack\/server\.node"/)
    assert.match(source, /^module\.exports = __ruvyxaClientProxy\("ruv:m_[a-f0-9]{16}"\)$/m)
    assert.ok(!source.includes('export default'), 'a named export would not carry every export')
  })

  it('refuses to emit a proxy for an id it did not produce', () => {
    assert.throws(() => clientProxyModuleSource('not-an-id'), /RUV1860/)
    assert.throws(() => clientRegistrationSource('not-an-id', './x.js'), /RUV1860/)
  })
})

describe('browser registration', () => {
  it('publishes the whole namespace under the id', () => {
    const id = clientModuleId('app/counter.tsx')
    const source = clientRegistrationSource(id, '/abs/app/counter.tsx')
    assert.match(source, /import \* as __ruvyxaClientModule from "\/abs\/app\/counter\.tsx"/)
    assert.ok(source.includes(`(globalThis.${CLIENT_MODULE_REGISTRY_GLOBAL} ||= {})["${id}"]`))
  })

  it('installs exactly the two globals React asks for', () => {
    const prelude = clientReferenceRuntimePrelude()
    assert.match(prelude, /globalThis\.__webpack_chunk_load__ =/)
    assert.match(prelude, /globalThis\.__webpack_require__ =/)
  })

  it('loads each chunk once and reports a reference nothing registered', async () => {
    const previousChunkLoad = globalThis.__webpack_chunk_load__
    const previousRequire = globalThis.__webpack_require__
    const previousRegistry = globalThis[CLIENT_MODULE_REGISTRY_GLOBAL]
    try {
      // The prelude is source, so it is evaluated the way a browser would.
      new Function(clientReferenceRuntimePrelude())()
      const id = clientModuleId('app/counter.tsx')

      // Two references in one module must not evaluate its chunk twice, or the
      // registry would hold two instances of the same component.
      const chunk = 'data:text/javascript,globalThis.__ruvyxaLoads=(globalThis.__ruvyxaLoads??0)+1'
      await globalThis.__webpack_chunk_load__(chunk)
      await globalThis.__webpack_chunk_load__(chunk)
      assert.equal(globalThis.__ruvyxaLoads, 1)

      assert.throws(() => globalThis.__webpack_require__(id), /RUV1861/)
      globalThis[CLIENT_MODULE_REGISTRY_GLOBAL] = { [id]: { default: 'Counter' } }
      assert.deepEqual(globalThis.__webpack_require__(id), { default: 'Counter' })
    } finally {
      globalThis.__webpack_chunk_load__ = previousChunkLoad
      globalThis.__webpack_require__ = previousRequire
      globalThis[CLIENT_MODULE_REGISTRY_GLOBAL] = previousRegistry
      delete globalThis.__ruvyxaLoads
    }
  })
})

describe('client manifest', () => {
  it('answers any export name for a registered module', () => {
    // React looks up `"<id>#<export>"` for every export it encounters and
    // cannot know the names in advance, so a plain object cannot answer. This
    // is why the manifest is a Proxy.
    const id = clientModuleId('app/counter.tsx')
    const manifest = clientManifest([{ id, chunks: ['/c/counter.js'] }])
    assert.deepEqual(manifest[`${id}#default`], {
      id,
      chunks: ['/c/counter.js'],
      name: 'default',
    })
    assert.deepEqual(manifest[`${id}#Badge`], { id, chunks: ['/c/counter.js'], name: 'Badge' })
    assert.ok(`${id}#anything` in manifest)
  })

  it('answers nothing for a module it does not know', () => {
    const manifest = clientManifest([])
    assert.equal(manifest['ruv:m_0123456789abcdef#default'], undefined)
    assert.equal('ruv:m_0123456789abcdef#default' in manifest, false)
    assert.equal(manifest['no-separator'], undefined)
  })
})
