import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  stat,
  symlink,
  writeFile,
} from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { after, describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

import {
  clearCompilerCache,
  compilerCacheStats,
  compileBundle,
  compileBundleWithMetadata,
  compileContentSource,
  inlineServerFunctions,
  invalidateCompilerCache,
  MDX_COMPONENT_EXTENSIONS,
  runtimeAliases,
  toImportPath,
} from '../../../packages/ruvyxa/runtime/compiler.mjs'
import { createFixtureWorkspace } from './fixture-workspace.mjs'
import { loadTsconfigPaths, resolveTsconfigPath } from '../../../packages/ruvyxa/runtime/paths.mjs'
import { expandImportMetaGlob } from '../../../packages/ruvyxa/runtime/glob.mjs'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const exampleRoot = path.join(workspaceRoot, 'examples/demo')
const configRenderer = path.join(workspaceRoot, 'packages/ruvyxa/runtime/config-renderer.mjs')
const pluginRuntime = path.join(workspaceRoot, 'packages/ruvyxa/runtime/plugin-runtime.mjs')
const fixtureWorkspace = await createFixtureWorkspace('ruvyxa-compiler-tests-', exampleRoot)
after(() => rm(fixtureWorkspace, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }))

/**
 * A runtime module and every sibling it imports, transitively.
 *
 * Reads the relative specifiers out of the source instead of restating them,
 * so a new sibling import cannot leave a copy of the module unable to load.
 */
async function runtimeModuleClosure(entry) {
  const runtimeDir = path.join(workspaceRoot, 'packages/ruvyxa/runtime')
  const seen = new Set()
  const queue = [entry]
  while (queue.length > 0) {
    const name = queue.shift()
    if (seen.has(name)) continue
    seen.add(name)
    const source = await readFile(path.join(runtimeDir, name), 'utf8')
    for (const match of source.matchAll(/from\s+'\.\/([\w.-]+\.mjs)'/g)) {
      queue.push(match[1])
    }
  }
  return [...seen]
}

describe('runtime compiler', () => {
  /**
   * A dependency reachable only through the importer's *real* path.
   *
   * This is the shape every pnpm install has: `node_modules/pkg` is a link into
   * a store directory whose siblings are that package's own dependencies.
   * Walking the link path instead of the real one reaches the project's
   * `node_modules`, where a transitive dependency was never installed — so the
   * bundler emitted a bare `import "dep"` that no browser can resolve, and
   * every hydration in `ruvyxa dev` failed on it silently.
   */
  it('resolves a dependency through a linked package, as Node does', async () => {
    await withFixture(async ({ root, outDir }) => {
      const store = path.join(root, 'store', 'pkg-a@1', 'node_modules')
      await mkdir(path.join(store, 'pkg-a'), { recursive: true })
      await mkdir(path.join(store, 'dep'), { recursive: true })
      await writeFile(
        path.join(store, 'pkg-a', 'package.json'),
        JSON.stringify({ name: 'pkg-a', main: 'index.js' }),
      )
      await writeFile(
        path.join(store, 'pkg-a', 'index.js'),
        "const dep = require('dep')\nmodule.exports = dep.value\n",
      )
      // No `main`: `index.js` is found by the directory-layout fallback, the
      // same way `scheduler` is.
      await writeFile(path.join(store, 'dep', 'package.json'), JSON.stringify({ name: 'dep' }))
      await writeFile(path.join(store, 'dep', 'index.js'), "module.exports = { value: 'linked' }\n")

      const projectModules = path.join(root, 'node_modules')
      await mkdir(projectModules, { recursive: true })
      await symlink(path.join(store, 'pkg-a'), path.join(projectModules, 'pkg-a'), 'junction')

      const outfile = path.join(outDir, 'linked.js')
      await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: "import value from 'pkg-a'\nexport const x = value\n",
        sourcefile: 'ruvyxa:entry.ts',
        outfile,
        platform: 'browser',
        sourceMap: false,
      })
      const code = await readFile(outfile, 'utf8')

      assert.ok(code.includes('linked'), 'the linked package pulled its dependency in')
      const bare = code.split('\n').filter((line) => /^import .* from "[^./]/.test(line))
      assert.deepEqual(bare, [], 'a browser bundle must carry no bare specifier')
    })
  })

  /**
   * Which functions a module declares `'use server'` on, and which it refuses.
   *
   * The table is the contract. The scan decides whether a function becomes
   * callable from a browser, and the two ways to be wrong are opposites: miss
   * one and a mutation silently never reaches a server, accept one it cannot
   * make callable and the call fails at run time with values from a render that
   * ended long ago. Every refusal below is a case where the second would happen.
   */
  it('finds module-scope server functions and refuses the ones it cannot hoist', () => {
    const supported = [
      ['async function save(fd) { "use server"; }', 'declaration'],
      ['/** doc */\nexport async function save(fd) { "use server" }', 'after a doc comment'],
      ['const save = async (fd) => { "use server" }', 'arrow'],
      ['const save = async fd => { "use server" }', 'unparenthesised arrow parameter'],
      ['export const save = async function (fd) { "use server" }', 'function expression'],
      ['async function save(x = (1)) { "use server" }', 'parenthesised default'],
      ['const r = /[{]/; async function save() { "use server" }', 'after a regex literal'],
    ]
    for (const [source, label] of supported) {
      assert.deepEqual(inlineServerFunctions(source), { names: ['save'], unsupported: [] }, label)
    }

    const refused = [
      // Closes over the enclosing call's variables. Hoisting it means binding
      // what it captured, which needs a scope-resolving parser.
      ['function outer() { async function save() { "use server" } }', 'nested'],
      ['const o = { async save() { "use server" } }', 'object method'],
      ['export default async function () { "use server" }', 'anonymous default export'],
      ['doSomething(function save() { "use server" })', 'callback'],
    ]
    for (const [source, label] of refused) {
      const found = inlineServerFunctions(source)
      assert.deepEqual(found.names, [], label)
      assert.deepEqual(found.unsupported, [1], label)
    }

    // Neither a string nor a comment is a directive, whatever it spells.
    for (const source of [
      'const s = "function f() { use server }"',
      '// async function f() { "use server" }',
      '/* async function f() { "use server" } */',
    ]) {
      assert.deepEqual(inlineServerFunctions(source), { names: [], unsupported: [] }, source)
    }
  })

  /**
   * A `'use server'` module is compiled by the server graph and referenced by
   * every other one — the mirror image of `'use client'`.
   */
  it('keeps a server-function module on the server and references it from a browser', async () => {
    await withFixture(async ({ root, outDir }) => {
      await mkdir(path.join(root, 'app'), { recursive: true })
      await writeFile(
        path.join(root, 'app', 'actions.ts'),
        '"use server"\nexport async function save(value) { return `saved ${value}` }\n',
      )
      await writeFile(
        path.join(root, 'app', 'button.tsx'),
        '"use client"\nimport { save } from "./actions"\nexport const run = () => save(1)\n',
      )

      const server = await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: 'import { save } from "./app/actions"\nexport const call = save\n',
        sourcefile: 'ruvyxa:server.tsx',
        outfile: path.join(outDir, 'server.mjs'),
        platform: 'node',
        bundleTarget: 'react-server',
        bundlePackages: false,
        external: ['react-server-dom-webpack/server.edge'],
        aliases: runtimeAliases(),
        sourceMap: false,
      })
      const serverCode = await readFile(path.join(outDir, 'server.mjs'), 'utf8')
      assert.ok(serverCode.includes('saved '), 'the server graph compiles the real function')
      assert.match(serverCode, /__ruvyxaEnqueueServer/)
      assert.equal(server.serverReferences.length, 1)
      assert.equal(server.serverReferences[0].relativePath, 'app/actions.ts')

      const browser = await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: 'import { run } from "./app/button"\nexport const go = run\n',
        sourcefile: 'ruvyxa:client.tsx',
        outfile: path.join(outDir, 'client.js'),
        platform: 'browser',
        serverReferenceClient: 'react-server-dom-webpack/client.browser',
        externalUrls: {
          react: '/react',
          'react/jsx-runtime': '/jsx',
          'react-server-dom-webpack/client.browser': '/rsc',
        },
        aliases: runtimeAliases(),
        sourceMap: false,
      })
      const browserCode = await readFile(path.join(outDir, 'client.js'), 'utf8')
      assert.ok(
        !browserCode.includes('saved '),
        'the server function body must not reach the browser',
      )
      assert.match(browserCode, /createServerReference/)
      assert.deepEqual(
        browser.serverReferences.map((reference) => reference.id),
        server.serverReferences.map((reference) => reference.id),
        'both graphs must name the module the same way, or a call cannot be routed',
      )
    })
  })

  /**
   * `RUV1007` still describes a real mistake: a route with no Flight machinery
   * has no way to call a server function, so importing one into its bundle is
   * exactly the crossing that check exists to refuse.
   */
  it('still refuses a server module in a browser bundle that cannot call one', async () => {
    await withFixture(async ({ root, outDir }) => {
      await mkdir(path.join(root, 'app'), { recursive: true })
      await writeFile(
        path.join(root, 'app', 'actions.ts'),
        '"use server"\nexport async function save() {}\n',
      )
      await assert.rejects(
        compileBundleWithMetadata({
          projectRoot: root,
          entrySource: 'import { save } from "./app/actions"\nexport const go = save\n',
          sourcefile: 'ruvyxa:client.tsx',
          outfile: path.join(outDir, 'plain.js'),
          platform: 'browser',
          sourceMap: false,
        }),
        /RUV1007/,
      )
    })
  })

  /**
   * `external` decides, even when the bundle was told to carry its packages.
   *
   * The option used to hold only by accident: a server bundle leaves
   * `node_modules` alone anyway, so nothing noticed the list was never
   * consulted. The server-components SSR registry then asked for both — it
   * inlines client modules lifted out of their own packages, so it has to
   * carry their dependencies — and resolved its own React despite listing it.
   * Client components rendered against that second copy read a null dispatcher
   * and threw on the first hook.
   */
  it('keeps a listed external external even when packages are bundled', async () => {
    await withFixture(async ({ root, outDir }) => {
      const modules = path.join(root, 'node_modules')
      await mkdir(path.join(modules, 'keep-me'), { recursive: true })
      await mkdir(path.join(modules, 'carry-me'), { recursive: true })
      await writeFile(
        path.join(modules, 'keep-me', 'package.json'),
        JSON.stringify({ name: 'keep-me', main: 'index.js' }),
      )
      await writeFile(
        path.join(modules, 'keep-me', 'index.js'),
        "module.exports = { marker: 'KEEP_ME_INLINED' }\n",
      )
      await writeFile(
        path.join(modules, 'carry-me', 'package.json'),
        JSON.stringify({ name: 'carry-me', main: 'index.js' }),
      )
      await writeFile(
        path.join(modules, 'carry-me', 'index.js'),
        "module.exports = { marker: 'CARRY_ME_INLINED' }\n",
      )

      const outfile = path.join(outDir, 'externals.js')
      await compileBundleWithMetadata({
        projectRoot: root,
        entrySource:
          "import keep from 'keep-me'\nimport carry from 'carry-me'\nexport const x = [keep, carry]\n",
        sourcefile: 'ruvyxa:entry.ts',
        outfile,
        platform: 'node',
        bundlePackages: true,
        external: ['keep-me'],
        sourceMap: false,
      })
      const code = await readFile(outfile, 'utf8')

      assert.ok(!code.includes('KEEP_ME_INLINED'), 'the listed external must not be inlined')
      assert.ok(code.includes('CARRY_ME_INLINED'), 'the unlisted package must be carried along')
      assert.match(code, /^import \* as \w+ from "keep-me";$/m)
    })
  })

  /**
   * A browser bundle sees the `NODE_ENV` the process compiling it sees.
   *
   * Browsers have no `process`, so every wrapped module gets a stand-in, and it
   * used to say `production` unconditionally. React reads it to choose between
   * the two builds it ships, so `ruvyxa dev` — which deliberately leaves
   * `NODE_ENV` unset and therefore renders with development React — served a
   * bundle that hydrated with production React. Ordinary hydration tolerated
   * the mismatch; a Flight payload did not, and the server-components route
   * failed with React refusing to read a development payload on a production
   * client.
   */
  it('gives a browser bundle the NODE_ENV the compiling process has', async () => {
    await withFixture(async ({ root, outDir }) => {
      const previous = process.env.NODE_ENV
      try {
        for (const [value, expected] of [
          ['production', 'production'],
          ['development', 'development'],
          [undefined, 'development'],
        ]) {
          if (value === undefined) delete process.env.NODE_ENV
          else process.env.NODE_ENV = value

          const outfile = path.join(outDir, `node-env-${expected}-${String(value)}.js`)
          await compileBundleWithMetadata({
            projectRoot: root,
            entrySource: 'export const mode = process.env.NODE_ENV\n',
            sourcefile: 'ruvyxa:entry.ts',
            outfile,
            platform: 'browser',
            sourceMap: false,
          })
          const code = await readFile(outfile, 'utf8')
          assert.match(
            code,
            new RegExp(`globalThis\\.process \\?\\? \\{ env: \\{ NODE_ENV: "${expected}" \\} \\}`),
            `NODE_ENV=${String(value)} must compile to ${expected}`,
          )
        }
      } finally {
        if (previous === undefined) delete process.env.NODE_ENV
        else process.env.NODE_ENV = previous
      }
    })
  })

  /**
   * `nodeEnv` overrides the ambient value, and on an edge target it is the only
   * value there is.
   *
   * A deployment passes it because a build artifact is a production build
   * whichever way the host starts it. On Node the emitted statement assigns to
   * the real `process.env`, so the wrapper's literal is never consulted — but a
   * Cloudflare worker has no `process` at all, and that literal is the only
   * `NODE_ENV` its code will ever read. Reasoning that an outer bundle's runtime
   * pin covered a bundle linked into it was true on Node and false on edge, and
   * it left every worker's server-components SSR pass compiling the client
   * modules it inlines under `"development"`.
   */
  it('pins a bundle to the NODE_ENV it was compiled for, whatever the host exports', async () => {
    await withFixture(async ({ root, outDir }) => {
      const previous = process.env.NODE_ENV
      process.env.NODE_ENV = 'development'
      try {
        for (const platform of ['browser', 'node']) {
          const outfile = path.join(outDir, `pinned-${platform}.js`)
          await compileBundleWithMetadata({
            projectRoot: root,
            entrySource: 'export const mode = process.env.NODE_ENV\n',
            sourcefile: 'ruvyxa:entry.ts',
            outfile,
            platform,
            nodeEnv: 'production',
            sourceMap: false,
          })
          const code = await readFile(outfile, 'utf8')
          // The stand-in literal, which is the whole story on a host with no
          // `process` — and it must not still say what the build machine did.
          assert.match(
            code,
            /globalThis\.process \?\? \{ env: \{ NODE_ENV: "production" \} \}/,
            `${platform}: the module stand-in was not pinned`,
          )
          assert.doesNotMatch(code, /NODE_ENV: "development"/, `${platform}`)
          // And the runtime assignment, for a host that does have one. Ahead of
          // every module factory, because React reads the value while its own
          // factory runs.
          const pin = code.indexOf('globalThis.process.env.NODE_ENV = "production"')
          assert.notEqual(pin, -1, `${platform}: no runtime pin`)
          assert.ok(pin < code.indexOf('const __m'), `${platform}: the pin runs too late`)
        }
      } finally {
        if (previous === undefined) delete process.env.NODE_ENV
        else process.env.NODE_ENV = previous
      }
    })
  })

  /**
   * A module imported only for its side effect runs where the source put it.
   *
   * The specifier scan runs one regex per import *form*, and the side-effect
   * form is the last of them. Appending each pattern's matches ordered the
   * forms rather than the imports, so `import './first.js'` written first was
   * evaluated last — after everything it was there to prepare. That is how the
   * server-components entry, whose first import installs the globals React's
   * decoder reads while it loads, ended up installing them afterwards.
   */
  it('evaluates a side-effect import where the source wrote it', async () => {
    await withFixture(async ({ root, outDir }) => {
      await writeFile(path.join(root, 'first.js'), "globalThis.__order = ['first']\n")
      await writeFile(
        path.join(root, 'second.js'),
        "globalThis.__order = [...(globalThis.__order ?? []), 'second']\nexport const value = 2\n",
      )

      const outfile = path.join(outDir, 'ordered.mjs')
      await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: [
          "import './first.js'",
          "import { value } from './second.js'",
          'export const total = value',
          '',
        ].join('\n'),
        sourcefile: 'ruvyxa:entry.ts',
        outfile,
        platform: 'node',
        sourceMap: false,
      })

      // Executed rather than read: the contract is evaluation order, and the
      // emitted text normalises quotes and spacing in ways an index comparison
      // would be reading by accident.
      delete globalThis.__order
      await import(`${pathToFileURL(outfile).href}?order`)
      assert.deepEqual(globalThis.__order, ['first', 'second'])
      delete globalThis.__order
    })
  })

  it('runs the React Compiler only when explicitly enabled and remains deterministic', async (t) => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-react-compiler-'))
    t.after(() => rm(root, { recursive: true, force: true }))
    const source = `export function Counter({ count }: { count: number }) {
      return <span>{count}</span>
    }`
    const baselineFile = path.join(root, 'baseline.js')
    const compiledFile = path.join(root, 'compiled.js')
    const repeatedFile = path.join(root, 'compiled-again.js')

    await compileBundleWithMetadata({
      projectRoot: root,
      entrySource: source,
      sourcefile: 'app/Counter.tsx',
      outfile: baselineFile,
      platform: 'browser',
    })
    await compileBundleWithMetadata({
      projectRoot: root,
      entrySource: source,
      sourcefile: 'app/Counter.tsx',
      outfile: compiledFile,
      platform: 'browser',
      reactCompiler: true,
    })
    await compileBundleWithMetadata({
      projectRoot: root,
      entrySource: source,
      sourcefile: 'app/Counter.tsx',
      outfile: repeatedFile,
      platform: 'browser',
      reactCompiler: true,
    })

    const baseline = await readFile(baselineFile, 'utf8')
    const compiled = await readFile(compiledFile, 'utf8')
    assert.doesNotMatch(baseline, /react\/compiler-runtime/)
    assert.match(compiled, /react\/compiler-runtime/)
    const withoutMapUrl = (value) => value.replace(/^\/\/# sourceMappingURL=.*$/m, '')
    assert.equal(withoutMapUrl(await readFile(repeatedFile, 'utf8')), withoutMapUrl(compiled))
    const sourceMap = JSON.parse(await readFile(`${compiledFile}.map`, 'utf8'))
    assert.ok(sourceMap.sources.some((sourceName) => sourceName.endsWith('app/Counter.tsx')))
  })

  it('expands import.meta.glob lazily and eagerly with aliases and stable inputs', async (t) => {
    const contract = JSON.parse(
      await readFile(path.join(workspaceRoot, 'tests/fixtures/glob-contract.json')),
    )
    assert.equal(contract.contract, 'ruvyxa.glob')
    assert.equal(contract.schemaVersion, 2)
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-glob-'))
    t.after(() => rm(root, { recursive: true, force: true }))
    await mkdir(path.join(root, 'posts', 'nested'), { recursive: true })
    await mkdir(path.join(root, 'content.v1'), { recursive: true })
    await writeFile(path.join(root, 'posts', 'one.ts'), "export const value = 'one'\n")
    await writeFile(path.join(root, 'posts', 'nested', 'two.ts'), "export const value = 'two'\n")
    await writeFile(path.join(root, 'content.v1', 'version.ts'), "export const value = 'v1'\n")
    await mkdir(path.join(root, 'posts', 'node_modules'), { recursive: true })
    await writeFile(path.join(root, 'posts', 'node_modules', 'hidden.ts'), 'throw new Error()\n')
    await writeFile(
      path.join(root, 'tsconfig.json'),
      JSON.stringify({ compilerOptions: { paths: { '@/*': ['./*'] } } }),
    )
    const outfile = path.join(root, '.ruvyxa', 'glob.mjs')
    const result = await compileBundleWithMetadata({
      projectRoot: root,
      entrySource: `
        export const lazy = import.meta.glob('./posts/**/*.ts')
        export const eager = import.meta.glob('./posts/*.ts', { eager: true })
        export const aliased = import.meta.glob('@/posts/*.ts')
        export const dottedDirectory = import.meta.glob('./content.v1/*.ts')
      `,
      outfile,
      bundlePackages: true,
    })
    const output = await readFile(outfile, 'utf8')
    assert.doesNotMatch(output, /import\.meta\.glob/)
    assert.match(output, /posts\/nested\/two/)
    assert.match(output, /posts\/one/)
    assert.ok(result.inputs.includes('posts'))
    assert.ok(result.inputs.includes('content.v1'))
  })

  // Replays the ordering, lowering, and scanning halves of the shared glob
  // contract against the same fixture the Rust expander replays, so the two
  // module graphs cannot drift. Each assertion here corresponds to a defect the
  // previous version of this suite could not see.
  it('replays the cross-language glob ordering, lowering, and scanning contract', async (t) => {
    const contract = JSON.parse(
      await readFile(path.join(workspaceRoot, 'tests/fixtures/glob-contract.json')),
    )
    assert.equal(contract.contract, 'ruvyxa.glob')
    assert.equal(contract.schemaVersion, 2)
    const { ordering, lowering, scanning } = contract

    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-glob-contract-'))
    t.after(() => rm(root, { recursive: true, force: true }))
    await mkdir(path.join(root, ordering.directory), { recursive: true })
    for (const file of ordering.files) {
      await writeFile(path.join(root, ordering.directory, file), 'export const value = 1\n')
    }

    // Keys follow code-unit order. localeCompare would produce
    // ordering.rejectedLocaleOrder, which is what this graph used to emit.
    const lazy = await expandImportMetaGlob(
      `export const all = import.meta.glob('${ordering.pattern}')\n`,
      root,
      root,
      {},
    )
    const keys = [...lazy.source.matchAll(/"([^"]+)":/g)].map((match) => match[1])
    assert.deepEqual(keys, ordering.expectedKeyOrder)
    assert.notDeepEqual(keys, ordering.rejectedLocaleOrder)

    // Eager matches lower to hoisted namespace imports, never to require().
    const eager = await expandImportMetaGlob(
      `export const all = import.meta.glob('${ordering.pattern}', { eager: true })\n`,
      root,
      root,
      {},
    )
    for (const forbidden of lowering.forbiddenInOutput) {
      assert.ok(
        !eager.source.includes(forbidden),
        `eager lowering must not emit ${forbidden}: ${eager.source}`,
      )
    }
    assert.match(eager.source, /import \* as __ruvyxaGlob0_0 from/)

    // A 'use client' directive must stay the first statement in the module.
    const directive = await expandImportMetaGlob(
      `'use client'\nexport const all = import.meta.glob('${ordering.pattern}', { eager: true })\n`,
      root,
      root,
      {},
    )
    assert.ok(
      directive.source.trimStart().startsWith("'use client'"),
      `generated imports must not displace the directive prologue: ${directive.source}`,
    )

    // A regex literal containing a quote must not hide the call after it.
    const scanned = await expandImportMetaGlob(
      `${scanning.mustExpandAfter}\nexport const all = import.meta.glob('${ordering.pattern}')\n`,
      root,
      root,
      {},
    )
    assert.doesNotMatch(scanned.source, /import\.meta\.glob/)

    // Occurrences that are not calls must be left completely alone.
    const inert = [
      `// import.meta.glob('${ordering.pattern}')`,
      `/* import.meta.glob('${ordering.pattern}') */`,
      `const text = "import.meta.glob('${ordering.pattern}')"`,
      "const template = `import.meta.glob('./x/*.ts')`",
    ].join('\n')
    const untouched = await expandImportMetaGlob(`${inert}\n`, root, root, {})
    assert.equal(untouched.source, `${inert}\n`)
  })

  it('rejects dynamic and root-escaping import.meta.glob patterns', async (t) => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-glob-invalid-'))
    t.after(() => rm(root, { recursive: true, force: true }))
    await assert.rejects(
      compileBundleWithMetadata({
        projectRoot: root,
        entrySource: "const pattern = './*.ts'; export default import.meta.glob(pattern)",
        outfile: path.join(root, '.ruvyxa', 'dynamic.mjs'),
      }),
      /RUV1810.*string literal/,
    )
    await assert.rejects(
      compileBundleWithMetadata({
        projectRoot: root,
        entrySource: "export default import.meta.glob('../*.ts')",
        outfile: path.join(root, '.ruvyxa', 'escape.mjs'),
      }),
      /RUV1810.*escapes the project root/,
    )
  })

  it('expands import.meta.glob inside a template interpolation without touching template text', async (t) => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-glob-template-'))
    t.after(() => rm(root, { recursive: true, force: true }))
    await writeFile(path.join(root, 'post.ts'), "export const value = 'post'\n")
    const outfile = path.join(root, '.ruvyxa', 'template.mjs')
    await compileBundleWithMetadata({
      projectRoot: root,
      entrySource:
        "export const value = `literal import.meta.glob('./ignored/*.ts') ${Object.keys(import.meta.glob('./*.ts')).length}`",
      outfile,
      bundlePackages: true,
    })
    const output = await readFile(outfile, 'utf8')
    assert.match(output, /literal import\.meta\.glob/)
    assert.match(output, /post\.ts/)
  })

  it('replays inherited tsconfig path aliases and fingerprints every config input', async (t) => {
    const fixture = JSON.parse(
      await readFile(path.join(workspaceRoot, 'tests/fixtures/path-alias-contract.json'), 'utf8'),
    )
    assert.equal(fixture.contract, 'ruvyxa.path-alias')
    assert.equal(fixture.schemaVersion, 1)
    const parent = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-tsconfig-paths-'))
    const root = path.join(parent, 'project')
    t.after(() => rm(parent, { recursive: true, force: true }))
    for (const [relative, source] of Object.entries({ ...fixture.files, ...fixture.configs })) {
      const file = path.join(root, relative)
      await mkdir(path.dirname(file), { recursive: true })
      await writeFile(file, source)
    }
    for (const [relative, source] of Object.entries(fixture.outsideFiles)) {
      const file = path.join(parent, relative)
      await mkdir(path.dirname(file), { recursive: true })
      await writeFile(file, source)
    }

    const model = loadTsconfigPaths(root)
    for (const expected of fixture.cases) {
      const resolved = resolveTsconfigPath(model, expected.specifier, resolveFixtureFile)
      const actual = resolved ? path.relative(root, resolved).replaceAll('\\', '/') : null
      assert.equal(actual, expected.expected, expected.name)
    }

    const outfile = path.join(root, '.ruvyxa', 'alias-entry.mjs')
    const result = await compileBundleWithMetadata({
      projectRoot: root,
      entrySource: "import { value } from '@/button'; export default value",
      outfile,
      bundlePackages: true,
    })
    const output = await readFile(outfile, 'utf8')
    assert.match(output, /exact/)
    assert.deepEqual(
      result.fingerprintInputs.filter((file) => file.includes('tsconfig')),
      ['config/tsconfig.base.json', 'tsconfig.json'],
    )
    assert.deepEqual(
      result.inputs.filter((file) => file.includes('tsconfig')),
      ['config/tsconfig.base.json', 'tsconfig.json'],
    )
  })

  // A `baseUrl` inherited through `extends` must anchor the child's `paths`.
  // Both Ruvyxa graphs used to anchor them to the directory of the config that
  // declared them instead, ignoring the inherited `baseUrl`. The two agreed
  // with each other, so no parity fixture caught it, while the editor and the
  // type checker resolved these imports somewhere else entirely.
  it('anchors child path aliases to a baseUrl inherited through extends', async (t) => {
    const fixture = JSON.parse(
      await readFile(path.join(workspaceRoot, 'tests/fixtures/path-alias-contract.json'), 'utf8'),
    )
    const scenario = fixture.inheritedBaseUrl
    const root = path.join(
      await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-inherited-base-')),
      'project',
    )
    t.after(() => rm(path.dirname(root), { recursive: true, force: true }))
    for (const [relative, source] of Object.entries({ ...scenario.files, ...scenario.configs })) {
      const file = path.join(root, relative)
      await mkdir(path.dirname(file), { recursive: true })
      await writeFile(file, source)
    }

    const model = loadTsconfigPaths(root)
    for (const expected of scenario.cases) {
      const resolved = resolveTsconfigPath(model, expected.specifier, resolveFixtureFile)
      const actual = resolved ? path.relative(root, resolved).replaceAll('\\', '/') : null
      assert.equal(actual, expected.expected, expected.name)
    }
  })

  it('parses JSONC trailing commas without changing comma-brace text in alias targets', async (t) => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa-tsconfig-jsonc-'))
    t.after(() => rm(root, { recursive: true, force: true }))
    await mkdir(path.join(root, 'src'), { recursive: true })
    await writeFile(path.join(root, 'src', 'comma,}.ts'), 'export const value = 1\n')
    await writeFile(
      path.join(root, 'tsconfig.json'),
      `{
        // This comma belongs to the string and must survive JSONC parsing.
        "compilerOptions": {
          "paths": { "@value": ["./src/comma,}.ts",], },
        },
      }`,
    )

    const model = loadTsconfigPaths(root)
    assert.equal(model.problem, null)
    assert.equal(
      resolveTsconfigPath(model, '@value', resolveFixtureFile),
      path.join(root, 'src', 'comma,}.ts'),
    )
  })

  /// The host parses this process's stdout as NDJSON, so every exit has to be
  /// one. `fail()` is async and only exits once its line is written; calling it
  /// without `await` let execution reach `path.resolve(undefined)` on the next
  /// line, which throws above the try block — the host saw an unhandled
  /// `ERR_INVALID_ARG_TYPE` stack instead of the RUV1601 it handles.
  it('reports a missing project root as a diagnostic rather than a crash', async () => {
    const { code, stdout, stderr } = await runRaw(configRenderer, [])

    assert.equal(code, 1)
    assert.doesNotMatch(stderr, /ERR_INVALID_ARG_TYPE/, `unhandled throw: ${stderr}`)
    const parsed = JSON.parse(stdout)
    assert.equal(parsed.ok, false)
    assert.equal(parsed.code, 'RUV1601')
  })

  it('resolves runtime aliases when the runtime path contains spaces', async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), 'ruvyxa runtime path '))
    try {
      const packageRoot = path.join(root, 'package')
      const runtimeDir = path.join(packageRoot, 'runtime')
      const sourceDir = path.join(packageRoot, 'src')
      await mkdir(runtimeDir, { recursive: true })
      await mkdir(sourceDir, { recursive: true })
      // compiler.mjs and the local modules it imports, since the copy is
      // loaded as a module rather than read as text. The set is read out of
      // the source rather than written down here: it was a literal pair, and
      // the first sibling import added after that turned this into a
      // module-not-found in a temp directory with a space in its name, which
      // is a long way from the line that caused it.
      for (const runtimeFile of await runtimeModuleClosure('compiler.mjs')) {
        await copyFile(
          path.join(workspaceRoot, 'packages/ruvyxa/runtime', runtimeFile),
          path.join(runtimeDir, runtimeFile),
        )
      }
      await writeFile(path.join(sourceDir, 'index.ts'), 'export {}\n')

      const copiedCompiler = await import(
        `${pathToFileURL(path.join(runtimeDir, 'compiler.mjs')).href}?t=${Date.now()}`
      )
      const aliases = copiedCompiler.runtimeAliases()

      assert.equal(await realpath(aliases.ruvyxa), await realpath(path.join(sourceDir, 'index.ts')))
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })

  it('replaces a use-client module with a reference in the server-components graph', async () => {
    // The whole point of the react-server graph: a `'use client'` module is not
    // compiled into it. Its imports must not be walked either, or the server
    // bundle would pull in browser-only code and a `useState` that the
    // react-server build of React does not export.
    await withFixture(async ({ root, outDir }) => {
      const clientFile = path.join(root, 'counter.jsx')
      const serverFile = path.join(root, 'server-page.jsx')
      const outfile = path.join(outDir, 'rsc.mjs')
      await writeFile(
        clientFile,
        `'use client'
import { useState } from 'react'
export default function Counter({ start }) { const [n] = useState(start); return n }
export const Badge = () => null
`,
      )
      await writeFile(
        serverFile,
        `import Counter, { Badge } from './counter.jsx'
export default function Page() { return [Counter, Badge] }
`,
      )

      const result = await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: `export { default } from './server-page.jsx'
`,
        sourcefile: 'ruvyxa:rsc-entry.tsx',
        outfile,
        platform: 'node',
        bundleTarget: 'react-server',
      })
      const code = await readFile(outfile, 'utf8')

      assert.equal(result.clientReferences.length, 1)
      assert.match(result.clientReferences[0].id, /^ruv:m_[a-f0-9]{16}$/)
      assert.equal(result.clientReferences[0].relativePath, 'counter.jsx')
      assert.ok(code.includes('createClientModuleProxy'), 'the proxy replaces the module')
      assert.ok(!code.includes('useState'), 'the client module must not be compiled in')
    })
  })

  it('leaves a use-client module alone for every other bundle target', async () => {
    // The transform is scoped to the server-components graph. An ordinary
    // server or browser bundle must compile the module exactly as before.
    await withFixture(async ({ root, outDir }) => {
      const clientFile = path.join(root, 'counter.jsx')
      const outfile = path.join(outDir, 'plain.mjs')
      await writeFile(
        clientFile,
        `'use client'
import { useState } from 'react'
export default function Counter() { const [n] = useState(1); return n }
`,
      )

      const result = await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: `export { default } from './counter.jsx'
`,
        sourcefile: 'ruvyxa:entry.tsx',
        outfile,
        platform: 'node',
      })
      const code = await readFile(outfile, 'utf8')

      assert.deepEqual(result.clientReferences, [])
      assert.ok(code.includes('useState'), 'the module is compiled normally')
      assert.ok(!code.includes('createClientModuleProxy'))
    })
  })

  it('compiles Markdown and MDX modules with frontmatter and components', async () => {
    await withFixture(async ({ root, outDir }) => {
      const cardFile = path.join(root, 'Card.js')
      const pageFile = path.join(root, 'page.mdx')
      const outfile = path.join(outDir, 'content.mjs')
      await writeFile(cardFile, 'export default function Card({ children }) { return children }\n')
      await writeFile(
        pageFile,
        `---
title: Built-in MDX
draft: false
---
import Card from './Card.js'

# Hello MDX

<Card>{2 + 2}</Card>
`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { default, frontmatter, headings } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:content-entry.ts',
        outfile,
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /Built-in MDX/)
      assert.match(output, /Hello MDX/)
      assert.match(output, /frontmatter/)
      assert.match(output, /2 \+ 2/)
    })
  })

  it('loads the nearest conventional MDX component provider', async () => {
    await withFixture(async ({ root, outDir }) => {
      const docs = path.join(root, 'app', 'docs')
      const pageFile = path.join(docs, 'page.mdx')
      const outfile = path.join(outDir, 'mdx-provider.mjs')
      await mkdir(docs, { recursive: true })
      await writeFile(
        path.join(root, 'app', 'mdx-components.tsx'),
        `export function useMDXComponents(components) {
          function BrandedHeading(props) { return props.children }
          return { ...components, h1: BrandedHeading }
        }\n`,
      )
      await writeFile(pageFile, '# Provider heading\n')

      const result = await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:mdx-provider-entry.ts',
        outfile,
        platform: 'node',
        external: ['react', 'react/js-runtime', 'react/jsx-runtime'],
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /BrandedHeading/)
      assert.ok(result.inputs.includes('app/mdx-components.tsx'), result.inputs)
    })
  })

  it('keeps MDX component extension priority aligned with the shared fixture', async () => {
    const fixture = JSON.parse(
      await readFile(
        path.join(workspaceRoot, 'tests/fixtures/mdx-components-conformance.json'),
        'utf8',
      ),
    )
    assert.deepEqual(
      MDX_COMPONENT_EXTENSIONS.map((extension) => extension.slice(1)),
      fixture.extensions,
    )
  })

  it('keeps nested YAML, GFM, footnotes, and heading slugs aligned in the Node compiler', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'parity.mdx')
      const outfile = path.join(outDir, 'content-parity.mjs')
      await writeFile(
        pageFile,
        `---
title: "Ruvyxa: Content"
author:
  name: Ada
tags:
  - rust
  - mdx
summary: |
  First line.
  Second line.
---
# Repeat
# Repeat
## ภาษาไทย
## 🚀
## ✨
## Istanbul

| Left | Right |
| :--- | ----: |
| one | two |

- [x] ~~shipped~~

A note[^1]

[^1]: Footnote body.
`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { default, frontmatter, headings } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:content-parity-entry.ts',
        outfile,
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })

      const output = await readFile(outfile, 'utf8')
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.deepEqual(mod.frontmatter, {
        title: 'Ruvyxa: Content',
        author: { name: 'Ada' },
        tags: ['rust', 'mdx'],
        summary: 'First line.\nSecond line.\n',
      })
      assert.deepEqual(mod.headings, [
        { depth: 1, slug: 'repeat', text: 'Repeat' },
        { depth: 1, slug: 'repeat-1', text: 'Repeat' },
        { depth: 2, slug: 'ภาษาไทย', text: 'ภาษาไทย' },
        { depth: 2, slug: 'section', text: '🚀' },
        { depth: 2, slug: 'section-1', text: '✨' },
        // Locale-independent case folding. A host whose ICU default is Turkish
        // lowercases `I` to `ı`, while the native compiler's `slugify` uses
        // Rust's `char::to_lowercase` and gives `i`. Asserting `istanbul`
        // pins the slug to the cross-language contract rather than the host.
        { depth: 2, slug: 'istanbul', text: 'Istanbul' },
      ])
      assert.match(output, /id:\s*"repeat-1"/)
      assert.match(output, /contains-task-list/)
      assert.match(output, /task-list-item/)
      assert.match(output, /data-footnotes/)
      assert.match(output, /textAlign/)
    })
  })

  it('runs remark, rehype, and recma plugins while preserving safe Markdown contracts', async () => {
    await withFixture(async ({ root }) => {
      let recmaRuns = 0
      const visit = (node, callback) => {
        callback(node)
        for (const child of node?.children ?? []) visit(child, callback)
      }
      const remarkPlugin = () => (tree, file) => {
        visit(tree, (node) => {
          if (node.type === 'text') node.value = node.value.replace('Original', 'Remarked')
        })
        file.data.ruvyxa.frontmatter.generated = true
      }
      const rehypePlugin = () => (tree) => {
        visit(tree, (node) => {
          if (node.type === 'element' && node.tagName === 'h1') {
            node.properties = { ...node.properties, id: 'plugin-heading', dataEnhanced: 'yes' }
          }
        })
      }
      const recmaPlugin = () => () => {
        recmaRuns += 1
      }

      const mdx = await compileContentSource(
        '---\ntitle: Plugins\n---\n# Original',
        path.join(root, 'page.mdx'),
        root,
        {
          remarkPlugins: [remarkPlugin],
          rehypePlugins: [rehypePlugin],
          recmaPlugins: [recmaPlugin],
        },
      )
      assert.equal(recmaRuns, 1)
      assert.match(mdx.source, /Remarked/)
      assert.match(mdx.source, /plugin-heading/)
      assert.match(mdx.source, /"data-enhanced":\s*"yes"/)
      assert.match(mdx.source, /"generated":true/)
      assert.match(mdx.source, /className:\s*"ruvyxa-content"/)
      assert.match(mdx.source, /data-content-format/)

      const markdown = await compileContentSource(
        '<script>globalThis.compromised = true</script>',
        path.join(root, 'page.md'),
        root,
        false,
      )
      assert.match(markdown.source, /<script>globalThis\.compromised = true<\/script>/)
      assert.doesNotMatch(markdown.source, /_jsx\("script"/)
      assert.doesNotMatch(markdown.source, /dangerouslySetInnerHTML/)
    })
  })

  it('shares configured Markdown plugins between runtime bundles and the native bridge', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.mdx')
      await writeFile(pageFile, '# Original heading\n')
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { config } from "ruvyxa/config"

          function remarkConfigured() {
            return (tree) => {
              for (const node of tree.children ?? []) {
                for (const child of node.children ?? []) {
                  if (child.type === "text") child.value = child.value.replace("Original", "Configured")
                }
              }
            }
          }

          function rehypeConfigured() {
            return (tree) => {
              for (const node of tree.children ?? []) {
                if (node.type === "element" && node.tagName === "h1") {
                  node.properties = { ...node.properties, id: "configured-id" }
                }
              }
            }
          }

          export default config({
            markdown: {
              remarkPlugins: [remarkConfigured],
              rehypePlugins: [rehypeConfigured],
            },
          })
        `,
      )

      const renderedConfig = await runJson(configRenderer, [root], {})
      assert.equal(renderedConfig.config.markdown, true)
      assert.equal(
        await readFile(
          path.join(root, '.ruvyxa', 'cache', 'config', 'runtime-config.mjs'),
          'utf8',
        ).then((source) => source.includes('export default config?.markdown')),
        true,
      )

      const bridged = await runJson(pluginRuntime, [root, 'content.compile'], {
        code: await readFile(pageFile, 'utf8'),
        id: pageFile,
        environment: 'client',
      })
      assert.match(bridged.result.code, /Configured heading/)
      assert.match(bridged.result.code, /configured-id/)

      const outfile = path.join(outDir, 'configured-markdown.mjs')
      clearCompilerCache()
      await compileBundle({
        projectRoot: root,
        entrySource: `export { default, headings } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:configured-markdown-entry.ts',
        outfile,
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })
      const runtimeOutput = await readFile(outfile, 'utf8')
      assert.match(runtimeOutput, /Configured heading/)
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.deepEqual(mod.headings, [
        { depth: 1, slug: 'configured-id', text: 'Configured heading' },
      ])

      const configFile = path.join(root, 'ruvyxa.config.ts')
      await writeFile(
        configFile,
        (await readFile(configFile, 'utf8')).replaceAll('Configured', 'Updated'),
      )
      await runJson(configRenderer, [root], {})
      const updatedOutfile = path.join(outDir, 'updated-markdown.mjs')
      await compileBundle({
        projectRoot: root,
        entrySource: `export { default, headings } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:updated-markdown-entry.ts',
        outfile: updatedOutfile,
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })
      const updatedOutput = await readFile(updatedOutfile, 'utf8')
      assert.match(updatedOutput, /Updated heading/)
      assert.doesNotMatch(updatedOutput, /Configured heading/)
    })
  })

  it('preserves MDX metadata exported through aliases, functions, and classes', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'metadata-exports.mdx')
      const outfile = path.join(outDir, 'metadata-exports.mjs')
      await writeFile(
        pageFile,
        `export const customHeadings = [{ depth: 1, slug: 'custom', text: 'Custom' }]
export { customHeadings as headings }
export function meta() { return 'custom-meta' }
export async function frontmatter() { return {} }
export class contentFormat {}

# Generated heading
`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { headings, meta, frontmatter, contentFormat } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:metadata-exports-entry.ts',
        outfile,
        platform: 'node',
        external: ['react', 'react/jsx-runtime'],
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.deepEqual(mod.headings, [{ depth: 1, slug: 'custom', text: 'Custom' }])
      assert.equal(mod.meta(), 'custom-meta')
      assert.equal(typeof mod.frontmatter, 'function')
      assert.equal(typeof mod.contentFormat, 'function')
    })
  })

  it('links a module whose regex literal contains a quote', async () => {
    // The scanner masks strings before classifying lines. Without regex-literal
    // handling, the `"` inside `/("[^"]*")/` opened a phantom string that ran to
    // the next quote later in the file, so every `export` in between was read as
    // string content and survived into the bundle — a syntax error at runtime,
    // because linked modules are wrapped in an IIFE where `export` is illegal.
    await withFixture(async ({ root, outDir }) => {
      const moduleFile = path.join(root, 'quote-in-regex.ts')
      const outfile = path.join(outDir, 'quote-in-regex.mjs')
      await writeFile(
        moduleFile,
        `const ATTRIBUTE = /\\slang\\s*=\\s*("[^"]*"|'[^']*'|[^\\s>]+)/i

export function replaceLang(tag: string, value: string): string {
  return ATTRIBUTE.test(tag) ? tag.replace(ATTRIBUTE, ' lang="' + value + '"') : tag
}

export const marker = 'reached'
`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { replaceLang, marker } from ${JSON.stringify(toImportPath(moduleFile))}`,
        sourcefile: 'ruvyxa:quote-in-regex-entry.ts',
        outfile,
        platform: 'node',
      })

      const bundled = await readFile(outfile, 'utf8')
      assert.doesNotMatch(bundled, /^\s+export /m, bundled)

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.marker, 'reached')
      assert.equal(mod.replaceLang('<html lang="en">', 'th'), '<html lang="th">')
    })
  })

  it('rejects invalid and non-mapping YAML frontmatter in the Node compiler', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'invalid.md')
      const outfile = path.join(outDir, 'invalid-content.mjs')
      const compile = () =>
        compileBundle({
          projectRoot: root,
          entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:invalid-content-entry.ts',
          outfile,
          platform: 'node',
          external: ['react', 'react/jsx-runtime'],
        })

      await writeFile(pageFile, '---\nauthor: [broken\n---\n# Page\n')
      await assert.rejects(compile(), /RUV1312 .*invalid YAML frontmatter/)

      clearCompilerCache()
      await writeFile(pageFile, '---\nhello\n---\n# Page\n')
      await assert.rejects(compile(), /RUV1312 .*frontmatter must be a YAML mapping/)

      clearCompilerCache()
      await writeFile(pageFile, '---\nvalue: .inf\n---\n# Page\n')
      await assert.rejects(compile(), /RUV1312 .*JSON-compatible values/)

      clearCompilerCache()
      await writeFile(pageFile, '---\n1: numeric key\n---\n# Page\n')
      await assert.rejects(compile(), /RUV1312 .*YAML mapping keys must be strings/)
    })
  })

  it('resolves local dynamic imports without an external bundler', async () => {
    await withFixture(async ({ root, outDir }) => {
      await writeFile(path.join(root, 'lazy.ts'), 'export const value = 42\n')
      const outfile = path.join(outDir, 'dynamic.mjs')

      await compileBundle({
        projectRoot: root,
        entrySource: `
          export async function load() {
            const mod = await import("./lazy.js")
            return mod.value
          }
        `,
        sourcefile: 'ruvyxa:dynamic-entry.ts',
        outfile,
        platform: 'node',
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(await mod.load(), 42)
    })
  })

  it('lowers TypeScript enums and namespaces through Oxc', async () => {
    await withFixture(async ({ root, outDir }) => {
      const moduleFile = path.join(root, 'typed.ts')
      const outfile = path.join(outDir, 'typed.mjs')
      await writeFile(
        moduleFile,
        `
          enum Mode { Development, Production = 4 }
          namespace BuildInfo { export const label: string = 'ready' }
          export const mode = Mode.Production
          export const label = BuildInfo.label
        `,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { mode, label } from ${JSON.stringify(toImportPath(moduleFile))}`,
        sourcefile: 'ruvyxa:typed-entry.ts',
        outfile,
        platform: 'node',
      })

      const output = await readFile(outfile, 'utf8')
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.doesNotMatch(output, /\benum\s+Mode\b|\bnamespace\s+BuildInfo\b/)
      assert.equal(mod.mode, 4)
      assert.equal(mod.label, 'ready')
    })
  })

  it('keeps code valid when runtime compiler minification is requested', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      const outfile = path.join(outDir, 'minified.mjs')
      await writeFile(
        pageFile,
        `
          export const label = 'preserve  internal  whitespace'
          // This comment must not consume the following export.
          export const answer = 42
        `,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { label, answer } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:minify-entry.ts',
        outfile,
        platform: 'browser',
        minify: true,
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.label, 'preserve  internal  whitespace')
      assert.equal(mod.answer, 42)
    })
  })

  it('initializes shared dependencies before importers across client graph branches', async () => {
    await withFixture(async ({ root, outDir }) => {
      const reactFile = path.join(root, 'react.js')
      const rendererFile = path.join(root, 'renderer.js')
      const pageFile = path.join(root, 'page.js')
      const outfile = path.join(outDir, 'dependency-order.mjs')

      await writeFile(
        reactFile,
        `
          export function useState(value) { return [value] }
          export function useEffect() {}
        `,
      )
      await writeFile(
        rendererFile,
        `
          import { useState } from 'react'
          export function render(Page) { return Page(useState) }
        `,
      )
      await writeFile(
        pageFile,
        `
          'use client'
          import { useEffect, useState } from 'react'
          export default function Page(rendererHook) {
            return rendererHook === useState && typeof useEffect === 'function'
          }
        `,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `
          import { render } from ${JSON.stringify(toImportPath(rendererFile))}
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export const initialized = render(Page)
        `,
        sourcefile: 'ruvyxa:client-dependency-order-entry.tsx',
        outfile,
        platform: 'browser',
        aliases: { react: reactFile },
      })

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.initialized, true)
    })
  })

  it('rejects circular local dependencies before emitting an invalid bundle', async () => {
    await withFixture(async ({ root, outDir }) => {
      const firstFile = path.join(root, 'first.js')
      const secondFile = path.join(root, 'second.js')
      const outfile = path.join(outDir, 'circular.mjs')

      await writeFile(
        firstFile,
        `import { second } from './second.js'\nexport const first = 'first:' + second\n`,
      )
      await writeFile(
        secondFile,
        `import { first } from './first.js'\nexport const second = 'second:' + first\n`,
      )

      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: `export { first } from ${JSON.stringify(toImportPath(firstFile))}`,
          sourcefile: 'ruvyxa:circular-entry.js',
          outfile,
          platform: 'browser',
        }),
        /RUV1803 circular dependency detected: first\.js -> second\.js -> first\.js/,
      )
    })
  })

  it('rewrites executable CommonJS requires without changing literal examples', async () => {
    await withFixture(async ({ root, outDir }) => {
      const dependencyFile = path.join(root, 'dependency.cjs')
      const entryFile = path.join(root, 'entry.js')
      const outfile = path.join(outDir, 'commonjs-literals.mjs')
      const specifier = './dependency.cjs'

      await writeFile(dependencyFile, `module.exports = { value: 42 }\n`)
      await writeFile(
        entryFile,
        [
          `const dependency = require(${JSON.stringify(specifier)})`,
          `const example = ${JSON.stringify(`require(${JSON.stringify(specifier)})`)}`,
          `const template = \`require(${JSON.stringify(specifier)})\``,
          `// require(${JSON.stringify(specifier)}) must stay documentation`,
          `export const result = { value: dependency.value, example, template }`,
          '',
        ].join('\n'),
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { result } from ${JSON.stringify(toImportPath(entryFile))}`,
        sourcefile: 'ruvyxa:commonjs-literal-entry.js',
        outfile,
        platform: 'browser',
      })

      const output = await readFile(outfile, 'utf8')
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.deepEqual(mod.result, {
        value: 42,
        example: `require(${JSON.stringify(specifier)})`,
        template: `require(${JSON.stringify(specifier)})`,
      })
      assert.match(output, /must stay documentation/)
    })
  })

  it('rewrites external CommonJS requires to ESM imports', async () => {
    await withFixture(async ({ root, outDir }) => {
      const entryFile = path.join(root, 'entry.cjs')
      const outfile = path.join(outDir, 'commonjs-external.mjs')
      await writeFile(
        entryFile,
        `const util = require('node:util')\nmodule.exports = { encoder: util.TextEncoder.name }\n`,
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { encoder } from ${JSON.stringify(toImportPath(entryFile))}`,
        sourcefile: 'ruvyxa:commonjs-external-entry.js',
        outfile,
        platform: 'node',
      })

      const output = await readFile(outfile, 'utf8')
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.encoder, 'TextEncoder')
      assert.doesNotMatch(output, /require\(['"]node:util['"]\)/)
    })
  })

  it('recompiles a changed source after compiler-cache invalidation', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      const outfile = path.join(outDir, 'cache-invalidation.mjs')
      const compile = () =>
        compileBundle({
          projectRoot: root,
          entrySource: `export { value } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:cache-invalidation-entry.ts',
          outfile,
          platform: 'node',
        })

      await writeFile(pageFile, `export const value = 'first'\n`)
      await compile()
      await writeFile(pageFile, `export const value = 'other'\n`)
      invalidateCompilerCache([pageFile])
      await compile()

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(mod.value, 'other')
      clearCompilerCache()
    })
  })

  it('bounds compiler derivation caches across many unique bundles', async () => {
    await withFixture(async ({ root, outDir }) => {
      const outfile = path.join(outDir, 'bounded-cache.mjs')
      clearCompilerCache()
      for (let index = 0; index < 513; index++) {
        await compileBundle({
          projectRoot: root,
          entrySource: `export const value = ${index}\n`,
          sourcefile: `ruvyxa:bounded-cache-${index}.ts`,
          outfile,
          platform: 'node',
        })
      }

      const stats = compilerCacheStats()
      assert.equal(stats.rewrites, stats.maxEntries)
      assert.equal(stats.transforms, stats.maxEntries)
      assert.ok(stats.sources <= stats.maxEntries)
      assert.ok(stats.content <= stats.maxEntries)
      clearCompilerCache()
    })
  })

  it('reuses transformed modules across bundles with the same inputs', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      await writeFile(
        pageFile,
        'export default function Page() { return <main>cached transform</main> }\n',
      )
      clearCompilerCache()
      const input = {
        projectRoot: root,
        entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:transform-cache-entry.ts',
        platform: 'node',
      }

      await compileBundle({ ...input, outfile: path.join(outDir, 'first.mjs') })
      const afterFirst = compilerCacheStats()
      await compileBundle({ ...input, outfile: path.join(outDir, 'second.mjs') })
      const afterSecond = compilerCacheStats()

      assert.ok(afterFirst.transforms > 0)
      assert.equal(afterSecond.transforms, afterFirst.transforms)
      clearCompilerCache()
    })
  })

  it('emits source maps and skips unchanged bundle writes', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      const outfile = path.join(outDir, 'mapped.mjs')
      await writeFile(pageFile, 'export const answer = 42\n')

      const input = {
        projectRoot: root,
        entrySource: `export * from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:mapped-entry.ts',
        outfile,
        platform: 'node',
      }

      await compileBundle(input)
      const before = await stat(outfile)
      const map = JSON.parse(await readFile(`${outfile}.map`, 'utf8'))
      assert.equal(map.version, 3)
      assert.equal(map.file, path.basename(outfile))
      assert.ok(map.sources.some((source) => source.endsWith('/page.ts')))
      assert.ok(map.sourcesContent.some((source) => source.includes('answer = 42')))

      await new Promise((resolve) => setTimeout(resolve, 25))
      await compileBundle(input)
      const after = await stat(outfile)
      assert.equal(after.mtimeMs, before.mtimeMs)
    })
  })

  it('handles TSX fragments, spread props, and JSX comments', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'jsx.mjs')
      await writeFile(
        pageFile,
        `
          export default function Page(props) {
            return <><main {...props} className="shell">{/* ignored */}<span>{"ok"}</span></main></>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:jsx-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /React\.Fragment/)
      assert.match(output, /Object\.assign/)
      assert.doesNotMatch(output, /ignored/)
    })
  })

  it('uses the automatic JSX runtime by default and keeps classic mode opt-in', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const automaticOutfile = path.join(outDir, 'automatic-jsx.mjs')
      const classicOutfile = path.join(outDir, 'classic-jsx.mjs')
      await writeFile(pageFile, `export default function Page() { return <main>ready</main> }`)

      const input = {
        projectRoot: root,
        entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:jsx-runtime-entry.tsx',
        platform: 'browser',
        external: ['react', 'react/jsx-runtime'],
      }
      await compileBundle({ ...input, outfile: automaticOutfile })
      await compileBundle({ ...input, outfile: classicOutfile, jsxRuntime: 'classic' })

      const automatic = await readFile(automaticOutfile, 'utf8')
      const classic = await readFile(classicOutfile, 'utf8')
      assert.match(automatic, /jsx/)
      assert.doesNotMatch(automatic, /React\.createElement/)
      assert.match(classic, /React\.createElement/)
    })
  })

  it('uses a unique diagnostic code for invalid JSX runtime configuration', async () => {
    await withFixture(async ({ root, outDir }) => {
      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: 'export default function Page() { return null }',
          sourcefile: 'ruvyxa:invalid-jsx-runtime.tsx',
          outfile: path.join(outDir, 'invalid-jsx-runtime.mjs'),
          platform: 'browser',
          jsxRuntime: 'invalid',
        }),
        /RUV1804 JSX runtime must be `classic` or `automatic`, got `invalid`/,
      )
    })
  })

  it('rewrites named class exports before wrapping modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const classFile = path.join(root, 'boundary.js')
      const outfile = path.join(outDir, 'class-export.mjs')
      await writeFile(classFile, `export class Boundary {\n  message() { return 'ready' }\n}\n`)

      await compileBundle({
        projectRoot: root,
        entrySource: `export { Boundary } from ${JSON.stringify(toImportPath(classFile))}`,
        sourcefile: 'ruvyxa:class-export-entry.js',
        outfile,
        platform: 'browser',
      })

      const output = await readFile(outfile, 'utf8')
      assert.doesNotMatch(output, /export class Boundary/)
      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.equal(new mod.Boundary().message(), 'ready')
    })
  })

  /**
   * Export shapes that broke the line-based rewriters in both module graphs.
   *
   * A generator's `*` binds to the keyword with no space, so
   * `export function* gen()` matched neither the Node rewriter's
   * `/^export\s+(async\s+)?function\s+/` nor the Rust linker's list of
   * literal prefixes. The `export` survived, and Node reported
   * `RUV1700 Unexpected token 'export'` from inside generated code.
   *
   * The module is imported and run, not matched: a rewrite that produced
   * syntactically valid output binding the wrong thing would pass a regular
   * expression and fail a caller.
   */
  it('rewrites generator and reserved-word exports before wrapping modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const moduleFile = path.join(root, 'shapes.js')
      const outfile = path.join(outDir, 'export-shapes.mjs')
      await writeFile(
        moduleFile,
        [
          'export function* counter() { yield 1; yield 2 }',
          'export async function* stream() { yield "a" }',
          // `from` is ordinary English inside a string, and a reserved word is
          // a legal property name. Neither is an ESM statement.
          'export const note = "copied from here"',
          'const conditions = { import: "./index.mjs", export: "./index.js" }',
          'export const entry = conditions.import',
          '',
        ].join('\n'),
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { counter, stream, note, entry } from ${JSON.stringify(toImportPath(moduleFile))}`,
        sourcefile: 'ruvyxa:export-shapes-entry.js',
        outfile,
        platform: 'browser',
      })

      const output = await readFile(outfile, 'utf8')
      assert.doesNotMatch(output, /^\s*export function\*/m, `an export survived:\n${output}`)
      assert.doesNotMatch(output, /^\s*export async function\*/m, `an export survived:\n${output}`)

      const mod = await import(pathToFileURL(outfile).href + `?t=${Date.now()}`)
      assert.deepEqual([...mod.counter()], [1, 2])
      const streamed = []
      for await (const value of mod.stream()) streamed.push(value)
      assert.deepEqual(streamed, ['a'])
      assert.equal(mod.note, 'copied from here')
      assert.equal(mod.entry, './index.mjs')
    })
  })

  it('handles JSX returned from ternaries and map callbacks', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'jsx-expressions.mjs')
      await writeFile(
        pageFile,
        `
          export default function Page({ items = ["one"], active = true }) {
            return (
              <main>
                {active ? <strong>Active</strong> : <span>Idle</span>}
                <ul>{items.map((item) => <li key={item}>{item}</li>)}</ul>
              </main>
            )
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:jsx-expression-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /React\.createElement\("strong"/)
      assert.match(output, /items\.map\(\(item\) => React\.createElement\("li"/)
      assert.doesNotMatch(output, /=> <li/)
    })
  })

  it('handles fragments in ternaries and dotted paths in code elements', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'jsx-edge-cases.mjs')
      await writeFile(
        pageFile,
        `
          export default function Page({ ready = true }) {
            return (
              <main>
                {ready ? <><span>Ready</span></> : <><span>Waiting</span></>}
                <code>.ruvyxa/prerender/static-page/index.html</code>
              </main>
            )
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from 'react'
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:jsx-edge-cases-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /React\.createElement\(React\.Fragment/)
      assert.match(output, /\.ruvyxa\/prerender\/static-page\/index\.html/)
      assert.doesNotMatch(output, /\? <>/)
    })
  })

  it('ignores import, export, and private env examples inside strings', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'string-examples.mjs')
      await writeFile(
        pageFile,
        `
          const snippet = \`
            import secret from "./missing"
            export function POST() {}
            export const createTodo = action
            process.env.DATABASE_URL
          \`

          export default function Page() {
            return <main>{snippet}</main>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:string-example-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /process\.env\.DATABASE_URL/)
      assert.doesNotMatch(output, /__exports\.POST/)
      assert.doesNotMatch(output, /__exports\.createTodo/)
    })
  })

  it('rejects private environment reads inside template expressions in client bundles', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      await writeFile(pageFile, 'export default `${process.env.DATABASE_URL}`\n')

      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:template-env-entry.ts',
          outfile: path.join(outDir, 'template-env.mjs'),
          platform: 'browser',
        }),
        /RUV1008: Private environment variable DATABASE_URL used in client bundle/,
      )
    })
  })

  it('rejects official server-only auth and database entrypoints in client bundles', async () => {
    await withFixture(async ({ root, outDir }) => {
      for (const packageName of ['@ruvyxa/auth', '@ruvyxa/database']) {
        const pageFile = path.join(root, `${packageName.split('/')[1]}.ts`)
        await writeFile(
          pageFile,
          `import * as serverApi from '${packageName}'\nexport default serverApi\n`,
        )
        await assert.rejects(
          compileBundle({
            projectRoot: root,
            entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
            sourcefile: 'ruvyxa:official-server-only-entry.ts',
            outfile: path.join(outDir, 'official-server-only.mjs'),
            platform: 'browser',
            external: [packageName],
          }),
          /RUV1007: Server-only module imported into client bundle/,
        )
      }
    })
  })

  it('rejects private environment reads that follow a regular expression literal', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      // A quote inside the character class used to open a string that ran to
      // end-of-file, so the env read below was never seen and the secret
      // shipped to the browser without a diagnostic.
      await writeFile(
        pageFile,
        'const quoted = /[\'"]/g\nexport default () => quoted.test(process.env.DATABASE_URL)\n',
      )

      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:regex-env-entry.ts',
          outfile: path.join(outDir, 'regex-env.mjs'),
          platform: 'browser',
        }),
        /RUV1008: Private environment variable DATABASE_URL used in client bundle/,
      )
    })
  })

  it('treats division as division when checking the client boundary', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      await writeFile(
        pageFile,
        'export const ratio = (a: number, b: number) => a / b / 2\nexport default () => ratio(1, 2)\n',
      )

      await compileBundle({
        projectRoot: root,
        entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:division-entry.ts',
        outfile: path.join(outDir, 'division.mjs'),
        platform: 'browser',
      })
    })
  })

  it('drops side-effect asset imports from wrapped modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'asset-import.mjs')
      await writeFile(path.join(root, 'global.css'), 'body { margin: 0; }\n')
      await writeFile(
        pageFile,
        `
          import "./global.css"

          export default function Page() {
            return <main>ok</main>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:asset-import-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.doesNotMatch(output, /import "\.\/global\.css"/)
    })
  })

  it('exports deterministic class maps for CSS and SCSS modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.ts')
      const outfile = path.join(outDir, 'style-modules.mjs')
      await writeFile(
        path.join(root, 'card.module.css'),
        `.base { color: navy; }
.card {
  composes: base;
  & .title { color: white; }
  :global(.theme-dark) .icon { color: black; }
}
`,
      )
      await writeFile(path.join(root, '_tokens.scss'), '$accent: rebeccapurple;\n')
      await writeFile(
        path.join(root, 'panel.module.scss'),
        "@use './tokens' as t; .panel { color: t.$accent; }\n",
      )
      await writeFile(
        pageFile,
        `
          import card from './card.module.css'
          import panel from './panel.module.scss'
          export const classes = [card.card, card.base, card.title, card.icon, card['theme-dark'], panel.panel]
        `,
      )

      const result = await compileBundleWithMetadata({
        projectRoot: root,
        entrySource: `export { classes } from ${JSON.stringify(toImportPath(pageFile))}`,
        sourcefile: 'ruvyxa:style-module-entry.ts',
        outfile,
        platform: 'node',
      })
      const mod = await import(`${pathToFileURL(outfile).href}?t=${Date.now()}`)

      assert.deepEqual(mod.classes[0].split(' '), ['card_card__a0c386682b31a0c2', mod.classes[1]])
      assert.match(mod.classes[2], /^card_title__/)
      assert.match(mod.classes[3], /^card_icon__/)
      assert.equal(mod.classes[4], undefined)
      assert.equal(mod.classes[5], 'panel_panel__9ffbc1bad8f2e789')
      assert.ok(result.inputs.includes('card.module.css'))
      assert.ok(result.inputs.includes('panel.module.scss'))
      assert.ok(result.inputs.includes('_tokens.scss'))
    })
  })

  it('derives a bundle content hash that only moves when the emitted code changes', async () => {
    // `contentHash` is the ESM import token used by the persistent worker.
    // Node never releases a loaded module URL, so an unchanged rebuild must
    // produce an unchanged hash or the worker retains one module graph per
    // rebuild for the life of the process.
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'content-hash.ts')
      const outfile = path.join(outDir, 'content-hash.mjs')
      const build = () =>
        compileBundleWithMetadata({
          projectRoot: root,
          entrySource: `export { value } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:content-hash-entry.ts',
          outfile,
          platform: 'node',
        })

      await writeFile(pageFile, "export const value = 'first'\n")
      const first = await build()
      assert.match(first.contentHash, /^[a-f0-9]{16}$/)

      clearCompilerCache()
      const rebuilt = await build()
      assert.equal(
        rebuilt.contentHash,
        first.contentHash,
        'recompiling unchanged sources must reuse the import token',
      )

      await writeFile(pageFile, "export const value = 'second'\n")
      clearCompilerCache()
      const changed = await build()
      assert.notEqual(
        changed.contentHash,
        first.contentHash,
        'changed output must produce a new import token',
      )
    })
  })

  it('reports stable Sass diagnostics for invalid modules', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'invalid-style.ts')
      const outfile = path.join(outDir, 'invalid-style.mjs')
      await writeFile(path.join(root, 'broken.module.scss'), '.broken { color: $missing; }\n')
      await writeFile(
        pageFile,
        "import broken from './broken.module.scss'; export default broken\n",
      )

      await assert.rejects(
        compileBundle({
          projectRoot: root,
          entrySource: `export { default } from ${JSON.stringify(toImportPath(pageFile))}`,
          sourcefile: 'ruvyxa:invalid-style-entry.ts',
          outfile,
          platform: 'node',
        }),
        /RUV1402 Sass compilation failed/,
      )
    })
  })

  it('preserves runtime CSS-in-JS style objects and style elements', async () => {
    await withFixture(async ({ root, outDir }) => {
      const pageFile = path.join(root, 'page.tsx')
      const outfile = path.join(outDir, 'css-in-js.mjs')
      await writeFile(
        pageFile,
        `
          const accent = "rebeccapurple"
          export default function Page() {
            return <main style={{ color: accent }}><style>{\`.card { color: \${accent}; }\`}</style>ok</main>
          }
        `,
      )

      await compileBundle({
        projectRoot: exampleRoot,
        entrySource: `
          import React from "react"
          import Page from ${JSON.stringify(toImportPath(pageFile))}
          export default Page
        `,
        sourcefile: 'ruvyxa:css-in-js-entry.tsx',
        outfile,
        platform: 'browser',
        external: ['react'],
        jsxRuntime: 'classic',
      })

      const output = await readFile(outfile, 'utf8')
      assert.match(output, /style:\s*\{\s*color:\s*accent\s*\}/)
      assert.match(output, /React\.createElement\("style"/)
      assert.match(output, /\.card \{ color:/)
    })
  })

  it('loads TypeScript plugin metadata and executes registered transform hooks', async () => {
    await withFixture(async ({ root }) => {
      const pageFile = path.join(root, 'page.tsx')
      await writeFile(pageFile, 'export const label = "Original"\n')
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { config } from "ruvyxa/config"
          import { definePlugin } from "ruvyxa/plugin"

          export default config({
            css: { entries: ["styles/global.css"] },
            plugins: [
              definePlugin({
                name: "replace-label",
                register({ build }) {
                  build.onTransform(({ code, id, environment }) => {
                    if (environment !== "client" || !id.endsWith("page.tsx")) return null
                    return { code: code.replace("Original", "Transformed") }
                  })
                },
              }),
            ],
          })
        `,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.ok, true)
      assert.deepEqual(config.config.css.entries, ['styles/global.css'])
      assert.equal(config.config.plugins[0].name, 'replace-label')

      const transformed = await runJson(pluginRuntime, [root, 'build.transform'], {
        code: await readFile(pageFile, 'utf8'),
        id: pageFile,
        environment: 'client',
      })

      assert.equal(transformed.ok, true)
      assert.match(transformed.result.code, /Transformed/)
    })
  })

  it('runs Fetch-native middleware and build-complete hooks from one plugin registry', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { writeFile } from "node:fs/promises"
          import { definePlugin } from "ruvyxa/plugin"

          export default {
            plugins: [definePlugin({
              name: "native-hooks",
              register({ http, build }) {
                http.onRequest({
                  match: ["/api/*"],
                  handler({ request, plugin }) {
                    const headers = new Headers(request.headers)
                    headers.set("x-plugin", plugin)
                    return new Request(request, { headers })
                  },
                })
                http.onResponse({
                  match: ["/api/*"],
                  handler({ response }) {
                    const headers = new Headers(response.headers)
                    headers.set("x-after", "yes")
                    return new Response(response.body, { status: response.status, headers })
                  },
                })
                build.onComplete(({ outDir, manifest }) =>
                  writeFile(outDir + "/plugin-complete.json", JSON.stringify(manifest))
                )
              },
            })],
          }
        `,
      )

      const described = await runJson(pluginRuntime, [root, 'describe'], {})
      assert.deepEqual(described.result, {
        plugins: ['native-hooks'],
        // `describe` reports the environment the host stated. It is the one
        // place the flag is visible if it ever stops arriving.
        environment: 'production',
        http: {
          request: 1,
          response: 1,
          routes: 0,
          requestMatch: ['/api/*'],
          responseMatch: ['/api/*'],
        },
        build: { start: 0, resolve: 0, load: 0, transform: 0, complete: 1 },
        dev: { fileChange: 0 },
        diagnostics: [],
        capabilities: [],
      })

      const request = await runJson(pluginRuntime, [root, 'http.request'], {
        request: { method: 'GET', path: '/api/users?active=1', headers: [] },
      })
      assert.equal(request.result.kind, 'request')
      assert.deepEqual(request.result.request.headers, [['x-plugin', 'native-hooks']])
      assert.equal(request.result.request.path, '/api/users?active=1')

      const response = await runJson(pluginRuntime, [root, 'http.response'], {
        request: request.result.request,
        response: {
          status: 200,
          headers: [
            ['content-type', 'application/octet-stream'],
            ['set-cookie', 'a=1; Path=/'],
            ['set-cookie', 'b=2; Path=/'],
          ],
          bodyBase64: Buffer.from([0, 255, 1]).toString('base64'),
        },
      })
      assert.equal(response.result.response.bodyBase64, Buffer.from([0, 255, 1]).toString('base64'))
      assert.equal(response.result.response.headers.find(([name]) => name === 'x-after')[1], 'yes')
      assert.deepEqual(
        response.result.response.headers.filter(([name]) => name === 'set-cookie'),
        [
          ['set-cookie', 'a=1; Path=/'],
          ['set-cookie', 'b=2; Path=/'],
        ],
      )

      const outDir = path.join(root, 'dist')
      await mkdir(outDir)
      const manifest = { routes: [{ path: '/' }] }
      const complete = await runJson(pluginRuntime, [root, 'build.complete'], { outDir, manifest })
      assert.equal(complete.ok, true)
      assert.deepEqual(
        JSON.parse(await readFile(path.join(outDir, 'plugin-complete.json'), 'utf8')),
        manifest,
      )
    })
  })

  it('connects route, build, dev, and diagnostic sockets through the plugin host', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import path from "node:path"
          import { writeFile } from "node:fs/promises"
          import { definePlugin } from "ruvyxa/plugin"

          export default {
            plugins: [definePlugin({
              name: "all-sockets",
              register({ http, build, dev, diagnostics }) {
                http.route({
                  method: "GET",
                  path: "/plugin-health",
                  handler: () => Response.json({ ok: true }),
                })
                build.onStart(({ outDir }) => writeFile(path.join(outDir, "started.txt"), "yes"))
                build.onResolve(({ id, root }) =>
                  id === "virtual:greeting" ? path.join(root, "virtual-greeting.ts") : undefined
                )
                build.onLoad(({ id }) =>
                  id.endsWith("virtual-greeting.ts")
                    ? { code: 'export const greeting = "hello"', map: { version: 3, mappings: "" } }
                    : undefined
                )
                dev.onFileChange({
                  match: ["content/*"],
                  handler: ({ root, paths }) =>
                    writeFile(path.join(root, "changed.json"), JSON.stringify(paths)),
                })
                diagnostics.report({
                  level: "warning",
                  code: "ALL001",
                  message: "All sockets are active",
                })
              },
            })],
          }
        `,
      )

      const described = await runJson(pluginRuntime, [root, 'describe'], {})
      assert.equal(described.result.http.routes, 1)
      assert.deepEqual(described.result.build, {
        start: 1,
        resolve: 1,
        load: 1,
        transform: 0,
        complete: 0,
      })
      assert.deepEqual(described.result.dev, { fileChange: 1 })
      assert.deepEqual(described.result.diagnostics, [
        {
          plugin: 'all-sockets',
          level: 'warning',
          code: 'ALL001',
          message: 'All sockets are active',
        },
      ])

      const route = await runJson(pluginRuntime, [root, 'http.request'], {
        request: { method: 'GET', path: '/plugin-health', headers: [] },
      })
      assert.equal(route.result.kind, 'response')
      assert.deepEqual(
        JSON.parse(Buffer.from(route.result.response.bodyBase64, 'base64').toString('utf8')),
        { ok: true },
      )

      const outDir = path.join(root, 'dist')
      await mkdir(outDir)
      await runJson(pluginRuntime, [root, 'build.start'], { outDir })
      assert.equal(await readFile(path.join(outDir, 'started.txt'), 'utf8'), 'yes')

      const resolved = await runJson(pluginRuntime, [root, 'build.resolve'], {
        id: 'virtual:greeting',
        environment: 'server',
      })
      assert.equal(resolved.result, path.join(root, 'virtual-greeting.ts'))
      const loaded = await runJson(pluginRuntime, [root, 'build.load'], {
        id: resolved.result,
        environment: 'server',
      })
      assert.match(loaded.result.code, /greeting = "hello"/)
      assert.equal(JSON.parse(loaded.result.map).version, 3)

      await runJson(pluginRuntime, [root, 'dev.fileChange'], {
        paths: ['content/guide.md', 'app/page.tsx'],
      })
      assert.deepEqual(JSON.parse(await readFile(path.join(root, 'changed.json'), 'utf8')), [
        'content/guide.md',
      ])
    })
  })

  it('rejects invalid contracts and duplicate plugin routes', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { plugins: [{ name: "invalid" }] }`,
      )
      const invalid = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(invalid.exitCode, 1)
      assert.match(invalid.parsed.message, /must provide register\(api\)/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [
            { name: "one", register({ http }) { http.route({ method: "GET", path: "/same", handler: () => new Response() }) } },
            { name: "two", register({ http }) { http.route({ method: "GET", path: "/same", handler: () => new Response() }) } },
          ],
        }`,
      )
      const duplicate = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(duplicate.exitCode, 1)
      assert.match(duplicate.parsed.message, /route GET \/same conflicts with plugin "one"/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{
            name: "empty-match",
            register({ http }) {
              http.onRequest({ match: [], handler: () => undefined })
            },
          }],
        }`,
      )
      const emptyMatch = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(emptyMatch.exitCode, 1)
      assert.match(emptyMatch.parsed.message, /match must contain at least one pattern/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{
            name: "invalid-method",
            register({ http }) {
              http.route({ method: "GET /wrong", path: "/wrong", handler: () => new Response() })
            },
          }],
        }`,
      )
      const invalidMethod = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(invalidMethod.exitCode, 1)
      assert.match(invalidMethod.parsed.message, /method must contain valid HTTP method tokens/)

      await writeFile(path.join(root, 'ruvyxa.config.ts'), `export default { plugins: [] }`)
      const empty = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(empty.exitCode, 0)
      assert.deepEqual(empty.parsed.result.plugins, [])
    })
  })

  it('matches plugin HTTP paths after percent-decoding, like the development router', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{
            name: "unicode-path",
            register({ http }) {
              http.route({
                method: "GET",
                path: "/café",
                handler: () => new Response("route matched"),
              })
              http.onResponse({
                match: ["/café"],
                handler: ({ response }) => {
                  const headers = new Headers(response.headers)
                  headers.set("x-plugin-path", "decoded")
                  return new Response(response.body, { status: response.status, headers })
                },
              })
            },
          }],
        }`,
      )

      const route = await runJson(pluginRuntime, [root, 'http.request'], {
        request: { method: 'GET', path: '/caf%C3%A9', headers: [] },
      })
      assert.equal(route.result.kind, 'response')
      assert.equal(
        Buffer.from(route.result.response.bodyBase64, 'base64').toString('utf8'),
        'route matched',
      )

      const response = await runJson(pluginRuntime, [root, 'http.response'], {
        request: { method: 'GET', path: '/caf%C3%A9', headers: [] },
        response: { status: 200, headers: [], bodyBase64: Buffer.from('ok').toString('base64') },
      })
      assert.equal(
        response.result.response.headers.find(([name]) => name === 'x-plugin-path')[1],
        'decoded',
      )
    })
  })

  it('loads first-party plugins through the public ruvyxa/plugins entrypoint', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { contentEngine, observability, openApi } from "ruvyxa/plugins"

          export default {
            plugins: [
              observability({ routes: ["/api/*"], log: false }),
              contentEngine({
                siteUrl: "https://example.com",
                title: "Fixture content",
                description: "Fixture articles",
              }),
              openApi({
                info: { title: "Fixture API", version: "1.0.0" },
                operations: [{ method: "get", path: "/api/health" }],
              }),
            ],
          }
        `,
      )

      const described = await runJson(pluginRuntime, [root, 'describe'], {})
      assert.deepEqual(described.result, {
        plugins: ['ruvyxa:observability', 'ruvyxa:content-engine', 'ruvyxa:openapi'],
        environment: 'production',
        http: {
          request: 3,
          response: 1,
          routes: 0,
          requestMatch: [
            '/api/*',
            '/content.json',
            '/search-index.json',
            '/rss.xml',
            '/sitemap.xml',
            '/llms.txt',
            '/openapi.json',
          ],
          responseMatch: ['/api/*'],
        },
        build: { start: 0, resolve: 0, load: 0, transform: 0, complete: 2 },
        dev: { fileChange: 0 },
        diagnostics: [],
        capabilities: [],
      })
      const configCache = path.join(root, '.ruvyxa', 'cache', 'config')
      const compiledConfigs = await Promise.all(
        (await readdir(configCache))
          .filter((name) => name.endsWith('.mjs'))
          .map((name) => readFile(path.join(configCache, name), 'utf8')),
      )
      assert.doesNotMatch(compiledConfigs.join('\n'), /^import \* as \w+ from ["']yaml["'];$/m)

      const requestResult = await runJson(pluginRuntime, [root, 'http.request'], {
        request: { method: 'GET', path: '/api/health', headers: [] },
      })
      assert.equal(requestResult.result.kind, 'request')
      assert.match(
        requestResult.result.request.headers.find(([name]) => name === 'x-request-id')[1],
        /^[0-9a-f-]{36}$/,
      )

      const specResult = await runJson(pluginRuntime, [root, 'http.request'], {
        request: { method: 'GET', path: '/openapi.json', headers: [] },
      })
      assert.equal(specResult.result.kind, 'response')
      assert.equal(
        JSON.parse(Buffer.from(specResult.result.response.bodyBase64, 'base64')).info.title,
        'Fixture API',
      )
    })
  })

  it('turns top-level content configuration into the built-in content engine', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          site: {
            url: "https://example.com",
            title: "Example content",
            description: "News from Example.",
            language: "en",
          },
          content: true,
        }`,
      )

      const rendered = await runJson(configRenderer, [root], {})
      assert.deepEqual(rendered.config.site, {
        url: 'https://example.com',
        title: 'Example content',
        description: 'News from Example.',
        language: 'en',
      })
      assert.equal(rendered.config.content, true)
      assert.deepEqual(rendered.config.plugins, [{ name: 'ruvyxa:content-engine' }])

      const described = await runJson(pluginRuntime, [root, 'describe'], {})
      assert.deepEqual(described.result.plugins, ['ruvyxa:content-engine'])
      assert.deepEqual(described.result.http.requestMatch, [
        '/content.json',
        '/search-index.json',
        '/rss.xml',
        '/sitemap.xml',
        '/llms.txt',
      ])
      assert.equal(described.result.build.complete, 1)
    })
  })

  it('rejects incomplete or duplicate top-level content configuration', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { site: { url: "https://example.com" }, content: true }`,
      )
      const incomplete = await runJsonResult(configRenderer, [root], {})
      assert.equal(incomplete.exitCode, 1)
      assert.match(incomplete.parsed.message, /site\.title must be a non-empty string/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `import { contentEngine } from "ruvyxa/plugins"
         export default {
           site: {
             url: "https://example.com",
             title: "Example",
             description: "Example content",
           },
           content: true,
           plugins: [contentEngine({
             siteUrl: "https://example.com",
             title: "Example",
             description: "Example content",
           })],
         }`,
      )
      const duplicate = await runJsonResult(configRenderer, [root], {})
      assert.equal(duplicate.exitCode, 1)
      assert.match(duplicate.parsed.message, /content engine is configured twice/)
    })
  })

  it('rejects middleware route patterns that can never match a pathname', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{
            name: 'invalid-route',
            register({ http }) {
              http.onRequest({ match: ['api/*'], handler() {} })
            },
          }],
        }`,
      )

      const failed = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(failed.exitCode, 1)
      assert.equal(failed.parsed.ok, false)
      assert.match(failed.parsed.message, /onRequest\(\)\.match\[0\].*start with "\/"/)
    })
  })

  it('describes one validated native realtime transport', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{
            name: 'realtime',
            register({ native }) {
              native.claim('realtime@1', { path: '/events', heartbeatMs: 10000, capacity: 64 })
            },
          }],
        }`,
      )

      const described = await runJson(pluginRuntime, [root, 'describe'], {})
      assert.deepEqual(described.result.capabilities[0], {
        id: 'realtime@1',
        plugin: 'realtime',
        path: '/events',
        heartbeatMs: 10_000,
        capacity: 64,
      })
    })
  })

  it('describes a native presence transport alongside realtime', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [
            { name: 'realtime', register({ native }) { native.claim('realtime@1') } },
            { name: 'collab', register({ native }) { native.claim('presence@1', { path: '/rooms', heartbeatMs: 15000 }) } },
          ],
        }`,
      )

      const described = await runJson(pluginRuntime, [root, 'describe'], {})
      // Both transports are separate capabilities, so a project may claim one,
      // the other, or both.
      assert.deepEqual(described.result.capabilities, [
        {
          id: 'realtime@1',
          plugin: 'realtime',
          path: '/__ruvyxa/realtime',
          heartbeatMs: 25_000,
          capacity: 256,
        },
        { id: 'presence@1', plugin: 'collab', path: '/rooms', heartbeatMs: 15_000 },
      ])
    })
  })

  it('rejects invalid, reserved, or duplicate presence registrations', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{ name: 'one', register({ native }) { native.claim('presence@1', { path: 'rooms' }) } }],
        }`,
      )
      const invalid = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(invalid.exitCode, 1)
      assert.match(invalid.parsed.message, /presence path must be an exact absolute path/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{ name: 'one', register({ native }) { native.claim('presence@1', { heartbeatMs: 1000 }) } }],
        }`,
      )
      const heartbeat = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(heartbeat.exitCode, 1)
      assert.match(heartbeat.parsed.message, /presence heartbeatMs must be between 5000 and 120000/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [{ name: 'one', register({ native }) { native.claim('presence@1', { path: '/__ruvyxa/image' }) } }],
        }`,
      )
      const reserved = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(reserved.exitCode, 1)
      assert.match(reserved.parsed.message, /collides with a reserved framework route/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [
            { name: 'one', register({ native }) { native.claim('presence@1') } },
            { name: 'two', register({ native }) { native.claim('presence@1') } },
          ],
        }`,
      )
      const duplicate = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(duplicate.exitCode, 1)
      assert.match(duplicate.parsed.message, /already owned by plugin "one"/)
    })
  })

  it('rejects invalid or duplicate realtime transport registrations', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [
            { name: 'one', register({ native }) { native.claim('realtime@1', { path: 'events' }) } },
            { name: 'two', register({ native }) { native.claim('realtime@1') } },
          ],
        }`,
      )
      const invalid = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(invalid.exitCode, 1)
      assert.match(invalid.parsed.message, /realtime path must be an exact absolute path/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [
            { name: 'one', register({ native }) { native.claim('realtime@1', { path: '/__ruvyxa/hmr' }) } },
          ],
        }`,
      )
      const reserved = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(reserved.exitCode, 1)
      assert.match(reserved.parsed.message, /collides with a reserved framework route/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          plugins: [
            { name: 'one', register({ native }) { native.claim('realtime@1') } },
            { name: 'two', register({ native }) { native.claim('realtime@1') } },
          ],
        }`,
      )
      const duplicate = await runJsonResult(pluginRuntime, [root, 'describe'], {})
      assert.equal(duplicate.exitCode, 1)
      assert.match(duplicate.parsed.message, /already owned by plugin "one"/)
    })
  })

  it('changes the config dependency fingerprint when imported plugin code changes', async () => {
    await withFixture(async ({ root }) => {
      const pluginFile = path.join(root, 'plugin.ts')
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          import { plugin } from "./plugin.js"
          export default { plugins: [plugin] }
        `,
      )
      await writeFile(
        pluginFile,
        `export const plugin = { name: "label", register({ build }) { build.onTransform(({ code }) => code + "\\n// one") } }\n`,
      )

      const first = await runJson(configRenderer, [root], {})
      await writeFile(
        pluginFile,
        `export const plugin = { name: "label", register({ build }) { build.onTransform(({ code }) => code + "\\n// two") } }\n`,
      )
      const second = await runJson(configRenderer, [root], {})

      assert.match(first.dependencyHash, /^[a-f0-9]{64}$/)
      assert.match(second.dependencyHash, /^[a-f0-9]{64}$/)
      assert.notEqual(second.dependencyHash, first.dependencyHash)
    })
  })

  it('returns JSON for missing and failing config files', async () => {
    await withFixture(async ({ root }) => {
      const missing = await runJson(configRenderer, [root], {})
      assert.equal(missing.ok, true)
      assert.deepEqual(missing.config, {})
      assert.equal(missing.dependencyHash, 'no-config')

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `
          throw new Error("bad config")
          export default {}
        `,
      )

      const failed = await runJsonResult(configRenderer, [root], {})
      assert.equal(failed.exitCode, 1)
      assert.equal(failed.parsed.ok, false)
      assert.match(failed.parsed.message, /bad config/)
    })
  })

  it('serializes WebP image encoding controls', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          image: {
            optimize: true,
            quality: 91,
            lossless: true,
            keepOriginal: false,
            variantWidths: [640, 1280],
            workers: 2,
          },
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.image, {
        optimize: true,
        quality: 91,
        lossless: true,
        keepOriginal: false,
        variantWidths: [640, 1280],
        workers: 2,
      })
    })
  })

  it('serializes validated i18n routing and on-demand image controls', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          i18n: {
            locales: ['en', 'th'],
            defaultLocale: 'th',
            localeParam: 'lang',
            detectLocale: true,
            cookie: 'RUVYXA_LOCALE',
          },
          image: { onDemand: { enabled: true, maxWidth: 2048 } },
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.i18n, {
        locales: ['en', 'th'],
        defaultLocale: 'th',
        localeParam: 'lang',
        detectLocale: true,
        cookie: 'RUVYXA_LOCALE',
      })
      assert.deepEqual(config.config.image, {
        onDemand: { enabled: true, maxWidth: 2048 },
      })
    })
  })

  it('rejects malformed on-demand image config at the renderer boundary', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { image: { onDemand: { enabled: 'yes', maxWidth: 2048 } } }`,
      )

      const result = await runJsonResult(configRenderer, [root], {})
      assert.equal(result.exitCode, 1)
      assert.match(result.parsed.message, /config\.image\.onDemand\.enabled must be boolean/)
    })
  })

  it('forwards the site block that drives robots.txt and sitemap.xml', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          site: { url: 'https://ruvyxa.dev', sitemap: true, robots: false },
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.site, {
        url: 'https://ruvyxa.dev',
        sitemap: true,
        robots: false,
      })
    })
  })

  it('serializes structured production sitemap and robots options', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          site: {
            url: 'https://ruvyxa.dev',
            sitemap: {
              exclude: ['/admin/*', '/drafts/*'],
              additionalPaths: ['/products/ชาไทย'],
              defaults: {
                lastModified: new Date('2026-07-29T04:30:00.000Z'),
                changeFrequency: 'weekly',
                priority: 0.5,
              },
              entries: [{
                url: '/about',
                lastModified: '2026-07-28',
                changeFrequency: 'monthly',
                priority: 0.8,
                alternates: { languages: { th: 'https://ruvyxa.dev/th/about' } },
                images: ['https://cdn.ruvyxa.dev/about.jpg'],
                videos: [{
                  title: 'About Ruvyxa',
                  thumbnail_loc: 'https://cdn.ruvyxa.dev/thumb.jpg',
                  description: 'Framework overview',
                  duration: 120,
                  family_friendly: 'yes',
                  restriction: { relationship: 'allow', content: 'TH US' },
                  tag: ['framework', 'rust'],
                }],
              }],
            },
            robots: {
              rules: [
                { userAgent: ['Googlebot', 'Bingbot'], allow: '/', disallow: ['/admin/'], crawlDelay: 5 },
                { userAgent: 'GPTBot', disallow: '/' },
              ],
              sitemap: ['https://ruvyxa.dev/sitemap.xml', 'https://ruvyxa.dev/news.xml'],
              host: 'https://ruvyxa.dev',
            },
          },
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.site, {
        url: 'https://ruvyxa.dev',
        sitemap: {
          exclude: ['/admin/*', '/drafts/*'],
          additionalPaths: ['/products/ชาไทย'],
          defaults: {
            lastModified: '2026-07-29T04:30:00.000Z',
            changeFrequency: 'weekly',
            priority: 0.5,
          },
          entries: [
            {
              url: '/about',
              lastModified: '2026-07-28',
              changeFrequency: 'monthly',
              priority: 0.8,
              alternates: { languages: { th: 'https://ruvyxa.dev/th/about' } },
              images: ['https://cdn.ruvyxa.dev/about.jpg'],
              videos: [
                {
                  title: 'About Ruvyxa',
                  thumbnail_loc: 'https://cdn.ruvyxa.dev/thumb.jpg',
                  description: 'Framework overview',
                  duration: 120,
                  family_friendly: 'yes',
                  restriction: { relationship: 'allow', content: 'TH US' },
                  tag: ['framework', 'rust'],
                },
              ],
            },
          ],
        },
        robots: {
          rules: [
            {
              userAgent: ['Googlebot', 'Bingbot'],
              allow: '/',
              disallow: ['/admin/'],
              crawlDelay: 5,
            },
            { userAgent: 'GPTBot', disallow: '/' },
          ],
          sitemap: ['https://ruvyxa.dev/sitemap.xml', 'https://ruvyxa.dev/news.xml'],
          host: 'https://ruvyxa.dev',
        },
      })
    })
  })

  it('rejects an unknown key inside the site block', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { site: { siteUrl: 'https://ruvyxa.dev' } }`,
      )

      const failed = await runJsonResult(configRenderer, [root], {})
      assert.equal(failed.exitCode, 1)
      assert.equal(failed.parsed.ok, false)
      assert.match(failed.parsed.message, /unknown config\.site field: siteUrl/)
    })
  })

  it('rejects invalid nested crawler discovery configuration', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { site: { sitemap: { excludes: ['/private'] } } }`,
      )
      const unknown = await runJsonResult(configRenderer, [root], {})
      assert.equal(unknown.exitCode, 1)
      assert.match(unknown.parsed.message, /unknown config\.site\.sitemap field: excludes/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { site: { robots: { rules: { userAgent: '*', crawlDelay: -1 } } } }`,
      )
      const invalidDelay = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidDelay.exitCode, 1)
      assert.match(invalidDelay.parsed.message, /crawlDelay must be a non-negative safe integer/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { site: { robots: { sitemap: [42] } } }`,
      )
      const invalidSitemap = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidSitemap.exitCode, 1)
      assert.match(invalidSitemap.parsed.message, /robots\.sitemap must be string or string\[\]/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { site: { sitemap: { entries: [{ url: '/about', priority: 2 }] } } }`,
      )
      const invalidPriority = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidPriority.exitCode, 1)
      assert.match(invalidPriority.parsed.message, /priority must be between 0 and 1/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { site: { sitemap: { entries: [{ url: '/about', videos: [{}] }] } } }`,
      )
      const invalidVideo = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidVideo.exitCode, 1)
      assert.match(invalidVideo.parsed.message, /videos\[0\]\.title must be a non-empty string/)
    })
  })

  it('serializes scalable action, API, and plugin security limits', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          security: {
            actionLimit: 2 * 1024 * 1024,
            apiLimit: 20 * 1024 * 1024,
            pluginLimit: 64 * 1024 * 1024,
            actionRateLimit: { max: 1200, window: 30 },
            trustedProxyIps: ['10.0.0.2', '2001:db8::1']
          }
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.security, {
        actionLimit: 2 * 1024 * 1024,
        apiLimit: 20 * 1024 * 1024,
        pluginLimit: 64 * 1024 * 1024,
        actionRateLimit: { max: 1200, window: 30 },
        trustedProxyIps: ['10.0.0.2', '2001:db8::1'],
      })
    })
  })

  it('forwards render and middleware configuration to the Ruvyxa CLI', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          reactCompiler: true,
          build: { prerenderCache: false },
          render: { strategy: 'isr', revalidate: 90 },
          middleware: {
            workers: 2,
            timeoutMs: 15000,
            builtin: { timing: false, headers: { 'X-Frame-Options': 'DENY' } }
          }
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.config.reactCompiler, true)
      assert.deepEqual(config.config.render, {
        strategy: 'isr',
        revalidate: 90,
      })
      assert.deepEqual(config.config.build, { prerenderCache: false })
      assert.deepEqual(config.config.middleware, {
        workers: 2,
        timeoutMs: 15000,
        builtin: { timing: false, headers: { 'X-Frame-Options': 'DENY' } },
      })
    })
  })

  it('serializes the selected JavaScript runtime for the CLI', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(path.join(root, 'ruvyxa.config.ts'), `export default { runtime: 'bun' }`)

      const config = await runJson(configRenderer, [root], {})
      assert.equal(config.config.runtime, 'bun')
    })
  })

  it('executes adapters and serializes their deployment metadata', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default {
          outDir: '.output',
          adapterOptions: { region: 'iad1' },
          adapter: {
            name: 'fixture',
            target: 'static',
            build({ root, outDir }) {
              return {
                name: 'fixture',
                target: 'static',
                platform: 'static',
                entry: outDir + '/static',
                assetsDir: outDir + '/assets',
                clientDir: outDir + '/client',
                root,
              }
            },
          },
        }`,
      )

      const config = await runJson(configRenderer, [root], {})
      assert.deepEqual(config.config.adapter, {
        name: 'fixture',
        target: 'static',
        platform: 'static',
        entry: '.output/static',
        assetsDir: '.output/assets',
        clientDir: '.output/client',
        root,
      })
      assert.deepEqual(config.config.adapterOptions, { region: 'iad1' })
    })
  })

  it('rejects unknown config fields instead of silently ignoring them', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { debug: { overlay: true, tracez: true } }`,
      )

      const failed = await runJsonResult(configRenderer, [root], {})
      assert.equal(failed.exitCode, 1)
      assert.equal(failed.parsed.ok, false)
      assert.match(failed.parsed.message, /RUV1602 unknown config\.debug field: tracez/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { image: { formats: ["avif", "webp"] } }`,
      )
      const obsolete = await runJsonResult(configRenderer, [root], {})
      assert.equal(obsolete.exitCode, 1)
      assert.match(obsolete.parsed.message, /RUV1602 unknown config\.image field: formats/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { render: { fallback: 'blocking' } }`,
      )
      const removedRenderingOption = await runJsonResult(configRenderer, [root], {})
      assert.equal(removedRenderingOption.exitCode, 1)
      assert.match(
        removedRenderingOption.parsed.message,
        /RUV1602 unknown config\.render field: fallback/,
      )

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { build: { sourcemap: false } }`,
      )
      const legacyBuildKey = await runJsonResult(configRenderer, [root], {})
      assert.equal(legacyBuildKey.exitCode, 1)
      assert.match(legacyBuildKey.parsed.message, /RUV1602 unknown config\.build field: sourcemap/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { security: { actionRateLimit: { burst: 10 } } }`,
      )
      const invalidRateLimit = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidRateLimit.exitCode, 1)
      assert.match(
        invalidRateLimit.parsed.message,
        /RUV1602 unknown config\.security\.actionRateLimit field: burst/,
      )

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { middleware: { builtin: { logging: false } } }`,
      )
      const legacyMiddlewareKey = await runJsonResult(configRenderer, [root], {})
      assert.equal(legacyMiddlewareKey.exitCode, 1)
      assert.match(
        legacyMiddlewareKey.parsed.message,
        /RUV1602 unknown config\.middleware\.builtin field: logging/,
      )

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { markdown: { html: true } }`,
      )
      const unknownMarkdownKey = await runJsonResult(configRenderer, [root], {})
      assert.equal(unknownMarkdownKey.exitCode, 1)
      assert.match(
        unknownMarkdownKey.parsed.message,
        /RUV1602 unknown config\.markdown field: html/,
      )
    })
  })

  it('rejects config values whose scalar types do not match the schema', async () => {
    await withFixture(async ({ root }) => {
      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { server: { port: '3000' } }`,
      )

      const invalidNumber = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidNumber.exitCode, 1)
      assert.match(invalidNumber.parsed.message, /RUV1602 config\.server\.port must be number/)

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { security: { trustedProxyIps: '127.0.0.1' } }`,
      )
      const invalidArray = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidArray.exitCode, 1)
      assert.match(
        invalidArray.parsed.message,
        /RUV1602 config\.security\.trustedProxyIps must be string\[\]/,
      )

      await writeFile(
        path.join(root, 'ruvyxa.config.ts'),
        `export default { markdown: { remarkPlugins: 'remark-gfm' } }`,
      )
      const invalidMarkdownPlugins = await runJsonResult(configRenderer, [root], {})
      assert.equal(invalidMarkdownPlugins.exitCode, 1)
      assert.match(
        invalidMarkdownPlugins.parsed.message,
        /RUV1602 config\.markdown\.remarkPlugins must be an array/,
      )
    })
  })
  it('keeps scanning code after a template literal that hides a backtick', async () => {
    // The compiler used to carry its own source scanner beside `scanner.mjs`,
    // and that copy had no interpolation state: a backtick inside a string
    // inside a `${…}` closed the template early, so every `import` after it in
    // the file read as string content. The dependency was dropped from the
    // bundle silently — no diagnostic, just a module that is not there.
    await withFixture(async ({ root, outDir }) => {
      const entryFile = path.join(root, 'entry.js')
      await writeFile(path.join(root, 'dep.js'), "export const value = 'dependency-was-bundled'\n")
      await writeFile(
        entryFile,
        [
          'const fence = `x${"`"}y`',
          "import { value } from './dep.js'",
          'export default value + fence',
          '',
        ].join('\n'),
      )

      const outfile = path.join(outDir, 'template-scan.mjs')
      clearCompilerCache()
      await compileBundle({
        projectRoot: root,
        entrySource: `export { default } from ${JSON.stringify(toImportPath(entryFile))}`,
        sourcefile: 'ruvyxa:template-scan-entry.ts',
        outfile,
        platform: 'node',
      })

      assert.match(await readFile(outfile, 'utf8'), /dependency-was-bundled/)
    })
  })
})

function resolveFixtureFile(candidate) {
  for (const file of [
    candidate,
    `${candidate}.ts`,
    `${candidate}.tsx`,
    path.join(candidate, 'index.ts'),
  ]) {
    try {
      if (existsSync(file)) return path.resolve(file)
    } catch {
      // Keep probing deterministic candidates.
    }
  }
  return null
}

/** Spawn a runtime script and report exactly what it wrote, ok or not. */
function runRaw(script, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', reject)
    child.on('close', (code) => resolve({ code, stdout, stderr }))
    child.stdin.end('')
  })
}

function runJson(script, args, payload) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', reject)
    child.on('close', (code) => {
      try {
        const parsed = JSON.parse(stdout)
        if (code === 0 && parsed.ok) {
          resolve(parsed)
        } else {
          reject(new Error(`script failed (${code}): ${stdout || stderr}`))
        }
      } catch (error) {
        reject(
          new Error(
            `invalid JSON from script: ${error.message}; stdout=${stdout}; stderr=${stderr}`,
          ),
        )
      }
    })
    child.stdin.end(JSON.stringify(script === pluginRuntime ? { ...payload } : payload))
  })
}

function runJsonResult(script, args, payload) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [script, ...args], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => {
      stdout += chunk
    })
    child.stderr.on('data', (chunk) => {
      stderr += chunk
    })
    child.on('error', reject)
    child.on('close', (exitCode) => {
      try {
        resolve({ exitCode, parsed: JSON.parse(stdout), stderr })
      } catch (error) {
        reject(
          new Error(
            `invalid JSON from script: ${error.message}; stdout=${stdout}; stderr=${stderr}`,
          ),
        )
      }
    })
    child.stdin.end(JSON.stringify(script === pluginRuntime ? { ...payload } : payload))
  })
}

async function withFixture(run) {
  const root = await mkdtemp(path.join(fixtureWorkspace, 'fixture-'))
  const outDir = path.join(root, '.ruvyxa', 'cache')
  await mkdir(outDir, { recursive: true })

  try {
    await run({ root, outDir })
  } finally {
    await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
}
