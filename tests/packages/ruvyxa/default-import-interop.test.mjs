/**
 * What a default import reads out of a linked module.
 *
 * Three module shapes reach this expression and each one answers a different
 * question, which is why the obvious versions are both wrong:
 *
 * - `X.default ?? X` — falls back to the namespace when `default` is
 *   `undefined`. A CommonJS module that assigns `module.exports = undefined`
 *   (lodash's `_WeakMap.js`, wherever its native check fails) then hands the
 *   importer a truthy object where the module said nothing, defeating the guard
 *   written for exactly that case. Bun died on `new WeakMap()`; Node did not.
 * - `"default" in X ? X.default : X` — correct for that one and wrong for a
 *   `'use client'` module, which is replaced by a `Proxy` whose `get` mints a
 *   client reference for any name. `in` answers `false` on its target, so the
 *   server serialized the proxy itself and React reported a reference the
 *   client manifest had never heard of.
 *
 * Reading the value first and only then asking whether the property exists
 * keeps all three straight. Both were shipped before this test existed.
 */
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { readFileSync, realpathSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'

import { compileBundleWithMetadata } from '../../../packages/ruvyxa/runtime/compiler.mjs'

/** Compile one entry against one dependency and run the result. */
async function linkAndRun({ dependency, entry }) {
  const root = await mkdtemp(path.join(realpathSync(os.tmpdir()), 'ruvyxa-interop-'))
  try {
    await mkdir(root, { recursive: true })
    await writeFile(path.join(root, 'dep.js'), dependency)
    const outfile = path.join(root, 'out.mjs')
    await compileBundleWithMetadata({
      projectRoot: root,
      entrySource: entry,
      sourcefile: 'entry.ts',
      outfile,
      platform: 'node',
      bundleTarget: 'ssr',
    })
    const code = readFileSync(outfile, 'utf8')
    const module = await import(
      `${path.sep === '\\' ? 'file:///' : 'file://'}${outfile.replaceAll('\\', '/')}`
    )
    return { code, module }
  } finally {
    // The imported module keeps the file open on Windows long enough that an
    // immediate removal races; the temp directory is small and short-lived.
    await rm(root, { recursive: true, force: true }).catch(() => {})
  }
}

describe('module shapes the linker has to carry', () => {
  /**
   * `await` in a module's own body, not inside one of its functions.
   *
   * Every module is emitted as an immediately-invoked function, and `await` is
   * illegal in a synchronous one — so an ESM-only package that initialises
   * itself at import time, or a route that awaits a dynamic import, produced a
   * bundle that would not parse. The wrapper is made `async` for exactly those
   * modules and awaited at the bundle's own top level, which may await.
   */
  it('carries top-level await in a dependency', async () => {
    const { module } = await linkAndRun({
      dependency: 'export const hit = await Promise.resolve("awaited")\n',
      entry: "import { hit } from './dep.js'\nexport const value = hit\n",
    })
    assert.equal(module.value, 'awaited')
  })

  it('carries top-level await in the entry itself', async () => {
    const { module } = await linkAndRun({
      dependency: 'export const hit = "dynamic"\n',
      entry: "const m = await import('./dep.js')\nexport const value = m.hit\n",
    })
    assert.equal(module.value, 'dynamic')
  })

  it('leaves a module that only awaits inside a function synchronous', async () => {
    const { code } = await linkAndRun({
      dependency: 'export async function later() { return await Promise.resolve(1) }\n',
      entry: "import { later } from './dep.js'\nexport const value = typeof later\n",
    })
    assert.ok(!code.includes('await (async () => {'), 'the common case must keep the bytes it had')
  })

  /**
   * `from` is a keyword only where a specifier follows it, and it is also an
   * ordinary binding name. Both linkers claimed this line as a re-export: the
   * JavaScript one dropped the export silently, and the Rust one left the
   * `export` in place and failed the build with RUV1612.
   */
  it('publishes an export aliased to `from`', async () => {
    const { module } = await linkAndRun({
      dependency: 'const source = 1\nexport { source as from }\n',
      entry: "import { from } from './dep.js'\nexport const value = String(from)\n",
    })
    assert.equal(module.value, '1')
  })
})

describe('default import interop', () => {
  it('hands over a deliberate `undefined` instead of the exports object', async () => {
    const { module } = await linkAndRun({
      // Computed, the way lodash writes it: `module.exports = getNative(root,
      // 'WeakMap')`. A literal `module.exports = undefined` is a different
      // shape — the module value itself is `undefined` — and no interop
      // expression can read a property off that one.
      dependency: 'function pick() { return undefined }\nmodule.exports = pick()\n',
      entry: "import value from './dep.js'\nexport const kind = typeof value\n",
    })
    assert.equal(
      module.kind,
      'undefined',
      'a module that exported nothing must not arrive as a truthy object',
    )
  })

  it('hands over the assigned value when there is one', async () => {
    const { module } = await linkAndRun({
      dependency: 'module.exports = function Widget() { return 1 }\n',
      entry: "import Widget from './dep.js'\nexport const result = Widget()\n",
    })
    assert.equal(module.result, 1)
  })

  it('hands over the namespace for a module with only named exports', async () => {
    const { module } = await linkAndRun({
      dependency: 'exports.a = 1\nexports.b = 2\n',
      entry: "import ns from './dep.js'\nexport const sum = ns.a + ns.b\n",
    })
    assert.equal(module.sum, 3)
  })

  it('reads `default` off an exotic object before asking whether it is there', async () => {
    // The `'use client'` proxy in one line: `in` is false on the target while
    // `get` answers for every name.
    const { module } = await linkAndRun({
      dependency:
        'module.exports = new Proxy({}, { get: (_target, key) => "reference:" + String(key) })\n',
      entry: "import Component from './dep.js'\nexport const seen = Component\n",
    })
    assert.equal(
      module.seen,
      'reference:default',
      'a proxy that mints a reference per name must be read, not stepped over',
    )
  })
})
