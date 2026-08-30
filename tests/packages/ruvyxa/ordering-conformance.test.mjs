/**
 * The JavaScript half of the shared ordering table.
 *
 * `crates/ruvyxa_bundler/src/resolver.rs` replays the same file against Rust's
 * `str::cmp`, which is the ordering every sorted artifact on that side comes
 * out in. This one drives `compareCodePoints`, which is the ordering every
 * sorted artifact on this side comes out in. A disagreement between them is one
 * project building to two different outputs.
 */

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

import { compareCodePoints, compareEntryKeys } from '../../../packages/ruvyxa/runtime/order.mjs'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const fixture = JSON.parse(
  readFileSync(path.join(repoRoot, 'tests/fixtures/ordering-conformance.json'), 'utf8'),
)

describe('the shared string ordering', () => {
  it('answers every case the way the Rust side answers it', () => {
    assert.ok(fixture.cases.length > 0, 'an empty table asserts nothing')
    for (const { name, left, right, expect } of fixture.cases) {
      assert.equal(compareCodePoints(left, right), expect, name)
    }
  })

  it('is antisymmetric, so a sort cannot depend on comparison order', () => {
    for (const { name, left, right, expect } of fixture.cases) {
      // `-0` is not `0` under strict equality, and an equal pair reverses to
      // itself rather than to negative zero.
      assert.equal(compareCodePoints(right, left), expect === 0 ? 0 : -expect, `reversed: ${name}`)
    }
  })

  /**
   * The case the rename was for. `<` puts the private-use character above the
   * emoji because it compares the leading surrogate `\uD83D` (below ``);
   * Rust puts the emoji above because it compares scalars. If this ever passes
   * with `<`, the fixture has stopped covering the divergence.
   */
  it('disagrees with a code-unit comparison exactly where it should', () => {
    const bmp = 'x'
    const astral = '\u{1F600}x'
    assert.equal(bmp < astral, false, 'code units put the private-use character second')
    assert.equal(compareCodePoints(bmp, astral), -1, 'code points put it first, as Rust does')
  })

  it('orders entry pairs by key through the same rule', () => {
    const entries = [
      ['\u{1F600}', 1],
      ['', 2],
      ['a', 3],
    ]
    assert.deepEqual(
      [...entries].sort(compareEntryKeys).map(([key]) => key),
      ['a', '', '\u{1F600}'],
    )
  })
})
