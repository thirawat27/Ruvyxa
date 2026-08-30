import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

import { repoPath } from '../../repo-root.ts'
import {
  compilePattern,
  createCanonicalRouteMatcher,
  routeSpecificity,
} from '../../../packages/@ruvyxa/core/dist/route-match.js'

/**
 * The JavaScript half of the dynamic-segment syntax rule.
 *
 * `crates/ruvyxa_graph/src/lib.rs::replays_the_shared_route_pattern_conformance_table`
 * drives the same file through route discovery, which decides which `app/`
 * folder names become routes at all. This file drives `compilePattern`, which
 * decides which of those segments capture anything.
 *
 * The invariant the two replays share: **for a bracketed segment, discovery
 * accepts it if and only if the matcher compiles it to a parameter.** Discovery
 * used to be the wider of the two, so `app/blog/[post-id]/page.tsx` reached the
 * manifest and then compiled here to the literal path `/blog/[post-id]` — a
 * route that existed everywhere and matched nothing.
 */
interface SegmentCase {
  segment: string
  kind: 'static' | 'dynamic' | 'catchAll' | 'optionalCatchAll' | 'rejected'
  param: string | null
  why: string
}

const fixture = JSON.parse(
  readFileSync(repoPath('tests/fixtures/route-pattern-conformance.json'), 'utf8'),
) as { segments: SegmentCase[] }

/** Every case is replayed in the final position of a two-segment route. */
function patternFor(segment: string): string {
  return `/blog/${segment}`
}

describe('shared route-pattern conformance table', () => {
  for (const testCase of fixture.segments) {
    const { segment, kind, param, why } = testCase

    it(`compiles ${segment} as ${kind === 'rejected' ? 'a literal it must never be handed' : kind}`, () => {
      const compiled = compilePattern(patternFor(segment))

      // A rejected name is refused by discovery precisely because this half
      // would silently read it as a literal. Asserting that here is what makes
      // the refusal load-bearing rather than arbitrary.
      if (kind === 'rejected' || kind === 'static') {
        assert.deepEqual(compiled.paramNames, [], why)
        assert.equal(compiled.catchAll, null, why)
        assert.ok(
          compiled.regex.test(patternFor(segment)),
          'an unrecognised segment matches only its own literal spelling',
        )
        assert.equal(
          compiled.regex.test('/blog/hello'),
          false,
          'an unrecognised segment captures nothing, so no other URL reaches it',
        )
        assert.deepEqual(routeSpecificity(patternFor(segment)), [0, 0], why)
        return
      }

      assert.deepEqual(compiled.paramNames, [param], why)

      if (kind === 'dynamic') {
        assert.equal(compiled.catchAll, null, why)
        assert.deepEqual(routeSpecificity(patternFor(segment)), [0, 1], why)
        assert.ok(compiled.regex.test('/blog/hello'))
        assert.equal(
          compiled.regex.test('/blog/hello/world'),
          false,
          'a dynamic segment spans exactly one path segment',
        )
        return
      }

      const optional = kind === 'optionalCatchAll'
      assert.deepEqual(compiled.catchAll, { name: param, optional }, why)
      assert.deepEqual(routeSpecificity(patternFor(segment)), [0, optional ? 3 : 2], why)
      assert.ok(compiled.regex.test('/blog/hello/world'))
      assert.equal(
        compiled.regex.test('/blog'),
        optional,
        'only an optional catch-all also matches its parent path',
      )
    })
  }

  it('binds every accepted parameter name as a usable key', () => {
    for (const { segment, kind, param } of fixture.segments) {
      if (kind !== 'dynamic') continue
      const match = createCanonicalRouteMatcher([{ path: patternFor(segment) }])('/blog/hello')
      assert.ok(match, `${segment} must match /blog/hello`)
      assert.deepEqual(match.params, { [param as string]: 'hello' })
    }
  })

  /**
   * RUV-H20 as a single assertion: the exact call that proved the divergence.
   * Discovery now refuses the folder, so this pattern can no longer reach a
   * manifest — but the matcher's answer is what made it a 404, and it is the
   * half that must not quietly start accepting it either.
   */
  it('reads a hyphenated segment as a literal, which is why discovery refuses it', () => {
    const match = createCanonicalRouteMatcher([{ path: '/blog/[post-id]' }])
    assert.equal(match('/blog/hello'), null)
    assert.ok(match('/blog/[post-id]'))
  })
})
