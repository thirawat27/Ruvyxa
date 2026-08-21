import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, it } from 'node:test'

import { parseByteRange } from '../../../packages/ruvyxa/runtime/serverless-handler.mjs'

const here = path.dirname(fileURLToPath(import.meta.url))
const fixture = JSON.parse(
  readFileSync(path.join(here, '../../fixtures/byte-range-conformance.json'), 'utf8'),
)

/**
 * `ruvyxa start` and a standalone/node deployment serve the same `public/`
 * directory from two different programs. A `<video>` that scrubs under one has
 * to scrub under the other, and the arithmetic that decides it — inclusive
 * ends, suffix ranges longer than the file, a start exactly at the length — is
 * written out in Rust and again in JavaScript.
 *
 * `crates/ruvyxa_dev_server/src/static_assets.rs` replays this same file, so a
 * rule changed in one language and not the other fails here rather than after
 * a deploy.
 */
describe('byte-range conformance', () => {
  it('carries cases', () => {
    assert.ok(fixture.cases.length > 0, 'the fixture must carry cases')
  })

  for (const singleCase of fixture.cases) {
    const { value, length, kind, why } = singleCase
    it(`${JSON.stringify(value)} against ${length} bytes is ${kind} — ${why}`, () => {
      const parsed = parseByteRange(value, length)
      assert.equal(parsed.kind, kind)
      if (kind === 'partial') {
        assert.equal(parsed.start, singleCase.start)
        assert.equal(parsed.end, singleCase.end)
      }
    })
  }

  /**
   * A header this server never sees from a browser but will see from a script.
   * `Number()` accepts all three of these as positions; the Rust side does not,
   * so neither may this.
   */
  it('reads positions as plain integers, not as anything Number accepts', () => {
    for (const value of ['bytes=1e1-', 'bytes=0x2-', 'bytes= -', 'bytes=+1-']) {
      assert.equal(parseByteRange(value, 100).kind, 'whole', `${value} must not parse`)
    }
  })

  /** A missing header is the ordinary case and must never be a refusal. */
  it('treats an absent header as a request for the whole file', () => {
    assert.equal(parseByteRange(undefined, 10).kind, 'whole')
    assert.equal(parseByteRange('', 10).kind, 'whole')
  })
})
