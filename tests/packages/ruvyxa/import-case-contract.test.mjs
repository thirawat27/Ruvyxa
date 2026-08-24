/**
 * The server graph's half of `tests/fixtures/import-case-conformance.json`.
 *
 * `existsSync` and `is_file()` both answer case-insensitively on Windows and on
 * default macOS, so `import './Header'` resolves `header.tsx` and the project
 * builds — and resolves nothing on Linux. Both module graphs run the same
 * comparison so the answer does not depend on which graph built the lane: a
 * rule enforced by one alone is a build that refuses under `ruvyxa build` and
 * passes at prerender, or the reverse.
 *
 * The Rust half is `import_case_comparison_matches_the_shared_conformance_table`
 * in `crates/ruvyxa_bundler/src/resolver.rs`.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const compilerPath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/compiler.mjs')

const { importCaseMismatch } = await import(`file://${compilerPath.replaceAll('\\', '/')}`)

const contract = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/import-case-conformance.json'), 'utf8'),
)

describe('import case comparison', () => {
  for (const testCase of contract.cases) {
    it(testCase.name, () => {
      const actual = importCaseMismatch(testCase.requested, testCase.resolved)
      assert.deepEqual(actual, testCase.mismatch)
    })
  }

  it('covers both answers, so a function returning one constant cannot pass', () => {
    assert.ok(
      contract.cases.some((entry) => entry.mismatch === null),
      'the fixture needs a case with no mismatch',
    )
    assert.ok(
      contract.cases.some((entry) => entry.mismatch !== null),
      'the fixture needs a case with a mismatch',
    )
  })
})
