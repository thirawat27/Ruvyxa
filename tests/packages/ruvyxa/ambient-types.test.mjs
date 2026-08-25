/**
 * Every type entry point offers the same ambient declarations.
 *
 * `packages/ruvyxa/types/` has one entry file per package export — `index`,
 * `config`, `server` — and each pulls in the ambient `.d.ts` files a project
 * needs by reference. Which entry a project loads is decided by what it happens
 * to import, and the templates import `ruvyxa/config` and nothing else. So an
 * ambient file referenced from only some of them is present or absent depending
 * on an unrelated choice, with no error either way — `import.meta.env` was
 * added to `index.d.ts` alone and stayed untyped for every scaffolded app,
 * which is the shape `AGENTS.md` calls a registration list.
 *
 * The rule is not "reference these three files" — it is that the entries agree.
 * A new ambient file is added to one of them and this test names the rest.
 */
import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const typesDir = path.join(workspaceRoot, 'packages/ruvyxa/types')

/** The `/// <reference path="./x.d.ts" />` targets of one file. */
function referencedAmbients(file) {
  const source = readFileSync(path.join(typesDir, file), 'utf8')
  return new Set(
    [...source.matchAll(/\/\/\/\s*<reference\s+path="\.\/([^"]+)"\s*\/>/g)].map((m) => m[1]),
  )
}

const entries = readdirSync(typesDir).filter(
  (name) => name.endsWith('.d.ts') && referencedAmbients(name).size > 0,
)

describe('ambient type declarations', () => {
  it('has entry points to check', () => {
    assert.ok(entries.length > 1, `expected several entry files, found ${entries.join(', ')}`)
  })

  it('offers the same ambient files from every entry point', () => {
    const [first, ...rest] = entries
    const expected = [...referencedAmbients(first)].sort()
    for (const entry of rest) {
      assert.deepEqual(
        [...referencedAmbients(entry)].sort(),
        expected,
        `${entry} and ${first} disagree — a project that imports one gets ambient ` +
          `declarations the other does not provide, and nothing reports it`,
      )
    }
  })

  it('references only files that exist', () => {
    const present = new Set(readdirSync(typesDir))
    for (const entry of entries) {
      for (const target of referencedAmbients(entry)) {
        assert.ok(present.has(target), `${entry} references missing ${target}`)
      }
    }
  })

  it('declares the environment shape the configuration docs use', () => {
    const source = readFileSync(path.join(typesDir, 'env.d.ts'), 'utf8')
    assert.match(source, /interface ImportMetaEnv/)
    assert.match(source, /interface ImportMeta\b/)
    // Only the published prefix is typed: a private name must not be reachable
    // through `import.meta.env` by accident.
    assert.match(source, /RUVYXA_PUBLIC_/)
  })
})
