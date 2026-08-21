/**
 * The server graph's half of `tests/fixtures/es-target-conformance.json`.
 *
 * `build.target` is applied by two compilers — `crates/ruvyxa_bundler` for the
 * client graph and `packages/ruvyxa/runtime/compiler.mjs` for the server and
 * prerender graph — and a project renders through whichever one built it. A
 * value one accepts and the other refuses is a build that succeeds on the
 * client and fails at prerender.
 *
 * The Rust half is `accepted_targets_match_the_shared_conformance_table` in
 * `crates/ruvyxa_bundler/src/compiler.rs`.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const compilerPath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/compiler.mjs')

const { resolveEsTarget, runtimeHelperImports } = await import(
  `file://${compilerPath.replaceAll('\\', '/')}`
)

const contract = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/es-target-conformance.json'), 'utf8'),
)

describe('build.target accepted values', () => {
  it('accepts every value the shared table names', () => {
    for (const target of contract.accepted) {
      assert.equal(resolveEsTarget(target), target)
    }
  })

  it('resolves every alias the shared table names', () => {
    for (const [written, expected] of Object.entries(contract.aliases)) {
      assert.equal(resolveEsTarget(written), expected)
    }
  })

  it('refuses every value the shared table refuses', () => {
    for (const written of contract.rejected) {
      assert.throws(
        () => resolveEsTarget(written),
        /RUV1601/,
        `the shared fixture refuses ${JSON.stringify(written)} and this host accepted it`,
      )
    }
  })

  it('treats an unset value as the default rather than an error', () => {
    // Absence is the ordinary case: a project that configures nothing keeps
    // emitting the bytes it always did. Each host spells absence its own way,
    // which is why it is not in the shared table.
    assert.equal(resolveEsTarget(undefined), 'esnext')
    assert.equal(resolveEsTarget(null), 'esnext')
  })
})

describe('helper-runtime guard', () => {
  it('finds a helper import oxc placed after the directive prologue', () => {
    const code = [
      '"use client";',
      'import _init from "@oxc-project/runtime/helpers/classPrivateFieldInitSpec";',
      'import _get from "@oxc-project/runtime/helpers/classPrivateFieldGet2";',
      'import React from "react";',
      'export const x = 1',
    ].join('\n')
    assert.deepEqual(runtimeHelperImports(code), [
      'classPrivateFieldInitSpec',
      'classPrivateFieldGet2',
    ])
  })

  it('does not mistake a string that spells an import for one', () => {
    // The scan stops at the first statement that is not an import, so a string
    // literal never reaches it. Matching the output text instead would flag a
    // module that imported nothing.
    const code =
      'export const note = "\\nimport _x from \\"@oxc-project/runtime/helpers/typeof\\";\\n"'
    assert.deepEqual(runtimeHelperImports(code), [])
  })

  it('reports nothing for output that imports nothing', () => {
    assert.deepEqual(runtimeHelperImports('export const x = 1'), [])
    assert.deepEqual(runtimeHelperImports(''), [])
  })
})
