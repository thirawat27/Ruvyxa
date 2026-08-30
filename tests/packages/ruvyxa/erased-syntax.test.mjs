/**
 * The JavaScript half of the erased-syntax contract.
 *
 * The emitted bundle is plain JavaScript wrapped in a function. Anything the
 * language does not allow there has to be gone by the time the linker runs, and
 * both compilers have to agree on what "gone" means — the Rust one builds the
 * browser bundle, `runtime/compiler.mjs` builds every server render, and a rule
 * applied by only one of them is a route that works in the browser and throws on
 * the server.
 *
 * The Rust half replays this table in
 * `crates/ruvyxa_bundler/src/compiler.rs`.
 */
import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { readFileSync, realpathSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { compileBundleWithMetadata } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const fixture = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/source-scanner-conformance.json'), 'utf8'),
).erasedSyntax

/**
 * Compile one dependency and return the emitted text, and the loaded module
 * when `load` is asked for.
 *
 * A JSX case cannot be loaded: the project is a bare temporary directory, so the
 * `react/jsx-runtime` import the transform emits has nothing to resolve against.
 * The emitted text is what those cases are about anyway — whether the bytes the
 * route would send still hold what the author wrote.
 */
async function compile(dependency, entry, { extension = '.ts', load = true } = {}) {
  const root = await mkdtemp(path.join(realpathSync(os.tmpdir()), 'ruvyxa-erased-'))
  try {
    await writeFile(path.join(root, `dep${extension}`), dependency)
    const outfile = path.join(root, 'out.mjs')
    await compileBundleWithMetadata({
      projectRoot: root,
      entrySource: entry,
      sourcefile: `entry${extension}`,
      outfile,
      platform: 'node',
      bundleTarget: 'ssr',
    })
    return {
      code: readFileSync(outfile, 'utf8'),
      module: load ? await import(pathToFileURL(outfile).href) : undefined,
    }
  } finally {
    await rm(root, { recursive: true, force: true }).catch(() => {})
  }
}

describe('syntax the linker has to erase', () => {
  it('removes every decorator the shared table lists', async () => {
    for (const source of fixture.decorators.stripped) {
      const { code } = await compile(
        `function log() { return (_t, _k, d) => d }\nfunction tag(c) { return c }\nexport ${source}export const hit = 1\n`,
        "import { hit } from './dep.ts'\nexport const value = hit\n",
      )
      assert.ok(!code.includes('@log'), `a decorator survived:\n${code.slice(0, 400)}`)
      assert.ok(!code.includes('@tag'), `a decorator survived:\n${code.slice(0, 400)}`)
    }
  })

  it('leaves an `@` that is not a decorator alone', async () => {
    const { module } = await compile(
      'export const email = "a@b.test"\nexport const pattern = /@handle/\n',
      "import { email, pattern } from './dep.ts'\nexport const value = email + String(pattern.test('@handle'))\n",
    )
    assert.equal(module.value, 'a@b.testtrue')
  })

  /**
   * The half of the decorator rule that deletes rendered text when it is wrong.
   *
   * `@` is not a JavaScript operator, so a code-position one begins a decorator
   * — and the scanner used to call JSX children a code position. In
   * `<p>write to @support</p>` the `@` follows an alphanumeric, which is exactly
   * where a decorator on a class member sits, so `stripDecorators` removed it
   * and the page rendered `write to `. Silently, on every server render.
   *
   * Compiled rather than masked, because the mask is the thing under test: this
   * asserts on what the route would actually send.
   */
  it('leaves an `@` in JSX text alone', async () => {
    const jsx = fixture.decorators.untouched.filter((source) => source.includes('<'))
    assert.ok(jsx.length > 0, 'the shared table must carry the JSX cases')
    for (const source of jsx) {
      const { code } = await compile(
        `${source.replace(/^const /, 'export const ')}`,
        "import { el } from './dep.tsx'\nexport const value = el\n",
        { extension: '.tsx', load: false },
      )
      for (const handle of source.match(/@[a-z]+/g) ?? []) {
        assert.ok(code.includes(handle), `${handle} was deleted from the page:\n${source}`)
      }
    }
  })

  it('removes a shebang without moving the lines below it', async () => {
    const { code, module } = await compile(
      '#!/usr/bin/env node\nexport const hit = "after-shebang"\n',
      "import { hit } from './dep.ts'\nexport const value = hit\n",
    )
    assert.equal(module.value, 'after-shebang')
    assert.ok(!code.includes('#!'), `a shebang survived:\n${code.slice(0, 300)}`)
  })

  it('leaves a `#!` that is not a shebang alone', async () => {
    const { module } = await compile(
      'export const url = "https://a.test/#!/route"\n',
      "import { url } from './dep.ts'\nexport const value = url\n",
    )
    assert.equal(module.value, 'https://a.test/#!/route')
  })
})
