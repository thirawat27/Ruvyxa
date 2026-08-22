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
  serverModuleId,
  serverProxyModuleSource,
  serverRegistrationSource,
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
  RSC_ENDPOINT,
  SERVER_ACTION_HEADER,
  createServerCaller,
  enqueueServerModule,
  flushServerModules,
  parseServerReference,
  resolveServerReference,
  serverReferenceProxy,
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

  it('names a dependency by its package, not by where the tree was measured from', () => {
    // `ruvyxa build` measures a reference from the project directory and
    // `ruvyxa start` measures the same module from the tree staged under
    // `.ruvyxa/server`, two directories deeper. A framework component used by
    // a layout therefore got one id in the browser bundle and another in the
    // payload rendered at request time: a direct load of the page worked and
    // every soft navigation into it failed with RUV1861.
    const fromProject = 'node_modules/@ruvyxa/react/dist/link.js'
    const fromStagedServer = '../../node_modules/@ruvyxa/react/dist/link.js'
    assert.equal(clientModuleId(fromProject), clientModuleId(fromStagedServer))
    assert.equal(
      normalizeClientModulePath(fromStagedServer),
      'node_modules/@ruvyxa/react/dist/link.js',
    )
  })

  it('names a package by its own subpath, whatever the store layout put around it', () => {
    // pnpm reaches one package through `.pnpm/<name>@<version>/node_modules/`
    // and npm reaches it directly. The last `node_modules` is the one that
    // starts the package's own path, so both layouts land on one id.
    assert.equal(
      normalizeClientModulePath('node_modules/.pnpm/pkg@1.0.0/node_modules/pkg/dist/widget.js'),
      'node_modules/pkg/dist/widget.js',
    )
    assert.equal(
      clientModuleId('node_modules/.pnpm/pkg@1.0.0/node_modules/pkg/dist/widget.js'),
      clientModuleId('node_modules/pkg/dist/widget.js'),
    )
  })

  it('keeps a project file distinct from a package of the same shape', () => {
    // The `node_modules/` segment is put back rather than stripped, so an app
    // directory that happens to mirror a package name cannot collide with it.
    assert.notEqual(
      clientModuleId('node_modules/pkg/dist/widget.js'),
      clientModuleId('pkg/dist/widget.js'),
    )
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

describe('server function identity', () => {
  it('cannot be confused with a client reference', () => {
    // The two travel through one registry and one `__webpack_require__`. An id
    // that satisfied both checks would let a component be invoked as a
    // function, or a function be rendered, and fail deep inside React.
    const path = 'app/actions.ts'
    assert.notEqual(serverModuleId(path), clientModuleId(path))
    assert.match(serverModuleId(path), /^ruv:s_[a-f0-9]{16}$/)
    assert.equal(isClientModuleId(serverModuleId(path)), false)
  })

  it('names a dependency by its package, exactly as a client reference does', () => {
    assert.equal(
      serverModuleId('../../node_modules/pkg/dist/actions.js'),
      serverModuleId('node_modules/pkg/dist/actions.js'),
    )
  })

  it('splits the reference React composes with a hash', () => {
    const module = serverModuleId('app/actions.ts')
    assert.deepEqual(parseServerReference(`${module}#save`), { module, name: 'save' })
  })

  it('refuses anything that is not a reference this framework produced', () => {
    for (const value of [
      'ruv:s_0123456789abcdef',
      `${clientModuleId('app/x.tsx')}#save`,
      'ruv:s_0123456789abcdef#',
      'save',
      '',
      null,
    ]) {
      assert.equal(parseServerReference(value), null, String(value))
    }
  })
})

describe('server function emission', () => {
  const id = serverModuleId('app/actions.ts')

  it('registers a whole module lazily, because the linker appends its exports', () => {
    // The registration sits at the bottom of the module body, and Ruvyxa's
    // linker emits every `__exports.name = name` assignment *after* that. Read
    // eagerly the exports object is still empty; the thunk is what defers it.
    const source = serverRegistrationSource(id)
    assert.match(source, /enqueueServerModule as __ruvyxaEnqueueServer/)
    assert.match(source, /__ruvyxaEnqueueServer\("ruv:s_[a-f0-9]{16}", \(\) => module\.exports,/)
  })

  it('registers only the functions named, when only some of them are server functions', () => {
    // A page component lives in the same file as an inline server function and
    // must not become callable from a browser alongside it.
    const source = serverRegistrationSource(id, ['save', 'remove'])
    assert.match(source, /\(\) => \(\{ save, remove \}\)/)
    assert.doesNotMatch(source, /module\.exports/)
  })

  it('refuses an id it did not produce', () => {
    for (const bad of [clientModuleId('app/x.tsx'), 'ruv:s_zz', '']) {
      assert.throws(() => serverRegistrationSource(bad), /RUV1864/)
      assert.throws(() => serverProxyModuleSource(bad, 'pkg'), /RUV1864/)
    }
  })

  it('declares the browser proxy a client module, so no lane rule refuses it', () => {
    // Both lane rules read the directive before the filename, and the file this
    // replaces is called `actions.ts` in every codebase that follows React's
    // convention — which is the action lane by name alone.
    const source = serverProxyModuleSource(id, 'react-server-dom-webpack/client.browser')
    assert.ok(source.startsWith("'use client'\n"), source.slice(0, 40))
  })

  it('imports the runtime by path for a browser graph and by alias for a server one', () => {
    // A browser graph is compiled by two bundlers and only one knows the alias;
    // a server graph is compiled by one, where an absolute path outside the
    // project would stay external and leave a bare import behind.
    const browser = serverProxyModuleSource(id, 'react-server-dom-webpack/client.browser')
    const ssr = serverProxyModuleSource(id, 'react-server-dom-webpack/client.edge')
    assert.match(browser, /rsc-client-runtime\.mjs"/)
    assert.ok(!browser.includes(RSC_CLIENT_RUNTIME_SPECIFIER), browser)
    assert.ok(ssr.includes(RSC_CLIENT_RUNTIME_SPECIFIER), ssr)
  })
})

describe('the server function runtime', () => {
  it('registers a queued module only once flushed, and answers by reference', () => {
    const scope = {}
    const registered = []
    const save = () => {}
    const exports = {}
    enqueueServerModule(
      'ruv:s_0123456789abcdef',
      () => exports,
      (fn, id, name) => {
        registered.push([id, name])
        fn.$$id = `${id}#${name}`
      },
      scope,
    )
    // The exports arrive after the body has run, which is the whole reason the
    // thunk exists.
    exports.save = save
    exports.LIMIT = 10

    flushServerModules(scope)
    assert.deepEqual(registered, [['ruv:s_0123456789abcdef', 'save']])
    assert.equal(
      resolveServerReference('ruv:s_0123456789abcdef#save', scope),
      save,
      'the registry answers the reference React will send back',
    )
    // Draining means a second render registers nothing again.
    flushServerModules(scope)
    assert.equal(registered.length, 1)
  })

  it('explains each way a reference can fail to resolve', () => {
    const scope = { __RUVYXA_CLIENT_MODULES__: { 'ruv:s_0123456789abcdef': { total: 4 } } }
    assert.throws(
      () => resolveServerReference('nonsense', scope),
      /RUV1865.*not a server function/s,
    )
    assert.throws(
      () => resolveServerReference('ruv:s_ffffffffffffffff#save', scope),
      /RUV1865.*does not reach/s,
    )
    assert.throws(
      () => resolveServerReference('ruv:s_0123456789abcdef#total', scope),
      /RUV1865.*not a function/s,
    )
  })

  it('hands out one reference per export name, for as long as the module lives', () => {
    // React compares a form action by identity across renders. A proxy that
    // minted a new function per property read would remount the form every time.
    const made = []
    const proxy = serverReferenceProxy(
      'ruv:s_0123456789abcdef',
      (reference) => {
        made.push(reference)
        return { reference }
      },
      () => {},
    )
    assert.equal(proxy.save, proxy.save)
    assert.deepEqual(made, ['ruv:s_0123456789abcdef#save'])
    assert.equal(proxy.save.reference, 'ruv:s_0123456789abcdef#save')
  })

  it('answers nothing for the names module interop reads', () => {
    // `then` above all: a namespace that looked thenable would hang anything
    // that awaited an import of it.
    const proxy = serverReferenceProxy(
      'ruv:s_0123456789abcdef',
      () => ({}),
      () => {},
    )
    for (const hidden of ['then', '__esModule', '$$typeof']) {
      assert.equal(proxy[hidden], undefined, hidden)
    }
  })

  it('posts a call to the payload endpoint with the route it belongs to', async () => {
    const sent = []
    const scope = {
      __RUVYXA_ROUTE_PATTERN__: '/blog/[slug]',
      fetch(url, init) {
        sent.push({ url, init })
        return Promise.resolve('response')
      },
    }
    const callServer = createServerCaller({
      encodeReply: async (args) => JSON.stringify(args),
      createFromFetch: async (response) => ({ decoded: await response }),
      scope,
    })
    const result = await callServer('ruv:s_0123456789abcdef#save', [1, 'two'])

    assert.deepEqual(result, { decoded: 'response' })
    assert.equal(sent.length, 1)
    assert.equal(sent[0].url, `${RSC_ENDPOINT}?path=%2Fblog%2F%5Bslug%5D`)
    assert.equal(sent[0].init.method, 'POST')
    assert.equal(sent[0].init.headers[SERVER_ACTION_HEADER], 'ruv:s_0123456789abcdef#save')
    assert.equal(sent[0].init.body, '[1,"two"]')
    // A string body needs a content type; multipart must not be given one, or
    // the boundary the browser chose never reaches the other side.
    assert.equal(sent[0].init.headers['content-type'], 'text/plain;charset=UTF-8')
  })

  it('lets the browser set the boundary when the arguments carry a file', async () => {
    const sent = []
    const form = new FormData()
    const callServer = createServerCaller({
      encodeReply: async () => form,
      createFromFetch: async () => null,
      scope: {
        fetch(_url, init) {
          sent.push(init)
          return Promise.resolve(null)
        },
      },
    })
    await callServer('ruv:s_0123456789abcdef#upload', [])
    assert.equal(sent[0].body, form)
    assert.equal(sent[0].headers['content-type'], undefined)
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
