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

/** Compile one dependency and return both the emitted text and the loaded module. */
async function compile(dependency, entry) {
  const root = await mkdtemp(path.join(realpathSync(os.tmpdir()), 'ruvyxa-erased-'))
  try {
    await writeFile(path.join(root, 'dep.ts'), dependency)
    const outfile = path.join(root, 'out.mjs')
    await compileBundleWithMetadata({
      projectRoot: root,
      entrySource: entry,
      sourcefile: 'entry.ts',
      outfile,
      platform: 'node',
      bundleTarget: 'ssr',
    })
    return {
      code: readFileSync(outfile, 'utf8'),
      module: await import(pathToFileURL(outfile).href),
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
