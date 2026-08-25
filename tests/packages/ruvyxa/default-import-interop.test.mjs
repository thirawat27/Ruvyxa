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
