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
const runtimeUrl = (file) =>
  `file://${path.join(workspaceRoot, 'packages/ruvyxa/runtime', file).replaceAll('\\', '/')}`

const {
  CLIENT_MODULE_REGISTRY_GLOBAL,
  RSC_CLIENT_RUNTIME_SPECIFIER,
  clientManifest,
  clientModuleId,
  clientProxyModuleSource,
  clientRegistrySource,
  isClientModuleId,
  normalizeClientModulePath,
} = await import(runtimeUrl('client-references.mjs'))

// The browser half is a separate module because the client bundle inlines it
// and it therefore cannot import `node:crypto`. Testing it from here keeps the
// id rule and the registry that answers for an id in one file.
const {
  RSC_PAYLOAD_ELEMENT_ID,
  installClientReferenceRuntime,
  payloadStream,
  readInlinePayload,
  registerClientModule,
} = await import(runtimeUrl('rsc-client-runtime.mjs'))

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
    // `.edge`, not `.node`: one server entry for every runtime the adapters
    // target, and one entry means one copy of the server runtime per bundle.
    assert.match(source, /from "react-server-dom-webpack\/server\.edge"/)
    assert.match(source, /^module\.exports = __ruvyxaClientProxy\("ruv:m_[a-f0-9]{16}"\)$/m)
    assert.ok(!source.includes('export default'), 'a named export would not carry every export')
  })

  it('refuses to emit for an id it did not produce', () => {
    assert.throws(() => clientProxyModuleSource('not-an-id'), /RUV1860/)
    assert.throws(() => clientRegistrySource([{ id: 'not-an-id', file: '/x.js' }]), /RUV1860/)
  })
})

describe('registry emission', () => {
  it('imports the reference runtime before the modules that need it', () => {
    // Order is load-bearing, not cosmetic: `client.browser` reads
    // `__webpack_require__.u` while its own module body runs, and the linker
    // evaluates a module's dependencies in the order they are imported.
    const id = clientModuleId('app/counter.tsx')
    const { imports, statements } = clientRegistrySource([{ id, file: '/abs/app/counter.tsx' }])
    assert.match(imports[0], new RegExp(`from "${RSC_CLIENT_RUNTIME_SPECIFIER}"$`))
    assert.match(imports[1], /^import \* as __ruvyxaClient0 from "\/abs\/app\/counter\.tsx"$/)
    assert.deepEqual(statements, [`__ruvyxaRegisterClient("${id}", __ruvyxaClient0)`])
  })

  it('publishes whole namespaces under distinct locals', () => {
    // `import * as` because React resolves `#default` and every named export off
    // the registry entry, and a build cannot know which names a payload asks for.
    const { imports, statements } = clientRegistrySource([
      { id: clientModuleId('a.tsx'), file: '/a.tsx' },
      { id: clientModuleId('b.tsx'), file: '/b.tsx' },
    ])
    assert.equal(imports.length, 3)
    assert.ok(imports.slice(1).every((line) => line.startsWith('import * as ')))
    assert.equal(new Set(imports).size, imports.length, 'each module needs its own local name')
    assert.equal(statements.length, 2)
  })

  it('still imports the runtime for a route with no client components', () => {
    // Such a route still decodes a payload, so the runtime still has to be there.
    const { imports, statements } = clientRegistrySource([])
    assert.equal(imports.length, 1)
    assert.deepEqual(statements, [])
  })
})

describe('importing the pipeline', () => {
  it('does not tell the process it is running inside webpack', async () => {
    // `compiler.mjs` reaches `client-references.mjs` to compute reference ids on
    // every build, RSC or not. When that import installed `__webpack_require__`
    // as a side effect, `sass` — which reads `typeof __webpack_require__` and
    // then reaches for `__non_webpack_require__` — failed on every SCSS module
    // in the project, several frames away from anything server-components.
    const before = globalThis.__webpack_require__
    await import(runtimeUrl('client-references.mjs'))
    await import(runtimeUrl('compiler.mjs'))
    assert.equal(globalThis.__webpack_require__, before)
  })
})

describe('the realm runtime', () => {
  it('installs exactly the two globals React asks for', () => {
    const scope = {}
    installClientReferenceRuntime(scope)
    assert.equal(typeof scope.__webpack_chunk_load__, 'function')
    assert.equal(typeof scope.__webpack_require__, 'function')
    // `client.browser` wraps `.u` at module scope; leaving it undefined turns a
    // missing chunk into a confusing failure several frames from the cause.
    assert.equal(typeof scope.__webpack_require__.u, 'function')
  })

  it('does not reinstall over a decoder that already wrapped it', () => {
    const scope = {}
    installClientReferenceRuntime(scope)
    const wrapped = () => 'wrapped'
    scope.__webpack_require__.u = wrapped
    installClientReferenceRuntime(scope)
    assert.equal(scope.__webpack_require__.u, wrapped)
  })

  it('loads each chunk once and reports a reference nothing registered', async () => {
    const scope = {}
    installClientReferenceRuntime(scope)
    const id = clientModuleId('app/counter.tsx')

    // Two references in one chunk must not evaluate it twice, or the registry
    // would hold two instances of the same component.
    const chunk = 'data:text/javascript,globalThis.__ruvyxaLoads=(globalThis.__ruvyxaLoads??0)+1'
    try {
      await scope.__webpack_chunk_load__(chunk)
      await scope.__webpack_chunk_load__(chunk)
      assert.equal(globalThis.__ruvyxaLoads, 1)
    } finally {
      delete globalThis.__ruvyxaLoads
    }

    assert.throws(() => scope.__webpack_require__(id), /RUV1861/)
    registerClientModule(id, { default: 'Counter' }, scope)
    assert.deepEqual(scope.__webpack_require__(id), { default: 'Counter' })
  })

  it('replaces a registration rather than keeping the first one', () => {
    // A rebuild hands over a new namespace for the same id; keeping the first
    // would serve the pre-edit component for the life of the process.
    const scope = {}
    const id = clientModuleId('app/counter.tsx')
    registerClientModule(id, { default: 'old' }, scope)
    registerClientModule(id, { default: 'new' }, scope)
    installClientReferenceRuntime(scope)
    assert.deepEqual(scope.__webpack_require__(id), { default: 'new' })
  })

  it('registers without claiming the realm is webpack', () => {
    // Defining `__webpack_require__` is how a library decides it is inside
    // webpack — `sass` reads it and then reaches for `__non_webpack_require__`.
    // Registering a module is Ruvyxa's own bookkeeping and must make no such
    // claim, so a server that has merely loaded a route's client modules has
    // not changed what every other library in the process sees.
    const scope = {}
    registerClientModule(clientModuleId('a.tsx'), { default: 'A' }, scope)
    assert.equal(scope.__webpack_require__, undefined)
    assert.equal(scope.__webpack_chunk_load__, undefined)
    assert.deepEqual(Object.keys(scope[CLIENT_MODULE_REGISTRY_GLOBAL]), [clientModuleId('a.tsx')])
  })

  it('installs the escape hatch webpack would have substituted', () => {
    const scope = { require: () => 'real require' }
    installClientReferenceRuntime(scope)
    assert.equal(scope.__non_webpack_require__, scope.require)

    // A browser has no `require` to hand back, so the pair is not invented there.
    const browser = {}
    installClientReferenceRuntime(browser)
    assert.equal(browser.__non_webpack_require__, undefined)
  })

  it('reads the payload the document was served with, and tolerates its absence', () => {
    const element = { textContent: JSON.stringify('0:["$","main",null,{}]\n') }
    const scope = {
      document: { getElementById: (id) => (id === RSC_PAYLOAD_ELEMENT_ID ? element : null) },
    }
    assert.equal(readInlinePayload(scope), '0:["$","main",null,{}]\n')
    assert.equal(readInlinePayload({ document: { getElementById: () => null } }), null)
    assert.equal(readInlinePayload({}), null)
    // A truncated response leaves the served HTML alone rather than blanking it.
    assert.equal(
      readInlinePayload({ document: { getElementById: () => ({ textContent: '{' }) } }),
      null,
    )
  })

  it('turns a payload into a stream the decoder can read', async () => {
    const payload = '0:["$","main",null,{}]\n'
    const chunks = []
    for await (const chunk of payloadStream(payload)) chunks.push(chunk)
    assert.equal(new TextDecoder().decode(Buffer.concat(chunks.map(Buffer.from))), payload)
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
