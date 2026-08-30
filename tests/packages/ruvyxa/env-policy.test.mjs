import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { envReadIsPrivate, privateEnvReads } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))

function readFixture() {
  return readFile(
    path.join(workspaceRoot, 'tests/fixtures/env-policy-conformance.json'),
    'utf8',
  ).then(JSON.parse)
}

describe('private environment variable policy', () => {
  it('replays the cross-language table the Rust boundary check also replays', async () => {
    const fixture = await readFixture()
    assert.ok(Array.isArray(fixture.cases) && fixture.cases.length > 0)

    for (const testCase of fixture.cases) {
      assert.equal(
        envReadIsPrivate(testCase.name),
        testCase.private,
        `process.env.${testCase.name} — ${testCase.why}`,
      )
    }
  })

  /**
   * The other half of the same policy: which name a read *is*.
   *
   * Classifying correctly is worth nothing if the two graphs extract different
   * names from the same source, and they did — `env_read_name` in
   * `crates/ruvyxa_bundler/src/ast.rs` read a whole identifier while this graph
   * matched upper-case only, so `process.env.databaseUrl` was invisible to
   * `ruvyxa dev` and refused by `ruvyxa build`. The Rust half of this replay is
   * `matches_the_shared_cross_language_env_extraction_table` in `boundary.rs`.
   */
  it('replays the cross-language extraction table the Rust scanner also replays', async () => {
    const fixture = await readFixture()
    const cases = fixture.extraction?.cases
    assert.ok(Array.isArray(cases) && cases.length > 0, 'the fixture must carry extraction cases')

    for (const testCase of cases) {
      assert.deepEqual(
        privateEnvReads(testCase.source),
        testCase.privateNames,
        `${JSON.stringify(testCase.source)} — ${testCase.why}`,
      )
    }
  })

  it('can extract every name the classification table classifies', async () => {
    // The two tables only meet if the extractor can produce the names the
    // classifier judges. It could not: `node_env` and `ruvyxa_public_key` were
    // unreachable from any source, so those two rows tested nothing end to end.
    const fixture = await readFixture()
    for (const testCase of fixture.cases) {
      // The empty name is the one deliberate exception: neither graph reports a
      // zero-length name, so that row exists only as a unit of the predicate.
      if (testCase.name === '' || !testCase.private) continue
      const source = `export const value = process.env[${JSON.stringify(testCase.name)}]\n`
      assert.deepEqual(
        privateEnvReads(source),
        [testCase.name],
        `no source can spell ${testCase.name}, so the classification case is untested end to end`,
      )
    }
  })

  it('is the rule the module scanner actually applies', async () => {
    // Guards against the predicate being exported for the fixture while the
    // scanner keeps an inlined copy of the old comparison.
    const source = await readFile(
      path.join(workspaceRoot, 'packages/ruvyxa/runtime/compiler.mjs'),
      'utf8',
    )
    const scannerUsesThePredicate = /parsed\s*&&\s*envReadIsPrivate\(parsed\.name\)/.test(source)
    assert.ok(
      scannerUsesThePredicate,
      'the private-env scan must call envReadIsPrivate rather than re-testing the name inline',
    )
  })
})
