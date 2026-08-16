import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { envReadIsPrivate } from '../../../packages/ruvyxa/runtime/compiler.mjs'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))

describe('private environment variable policy', () => {
  it('replays the cross-language table the Rust boundary check also replays', async () => {
    const fixture = JSON.parse(
      await readFile(
        path.join(workspaceRoot, 'tests/fixtures/env-policy-conformance.json'),
        'utf8',
      ),
    )
    assert.ok(Array.isArray(fixture.cases) && fixture.cases.length > 0)

    for (const testCase of fixture.cases) {
      assert.equal(
        envReadIsPrivate(testCase.name),
        testCase.private,
        `process.env.${testCase.name} — ${testCase.why}`,
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
