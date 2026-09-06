import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

import { repoPath } from '../../repo-root.ts'
import {
  applyHeaderRules,
  compileHeaderRules,
  compileMatcher,
  compileRedirectRules,
  compileRewriteRules,
  compileSource,
  evaluateConditions,
  matchRedirect,
  matchRewrite,
  matchSource,
  matcherMatches,
  normalizeMatcher,
  normalizeRewrites,
  type RuleRequest,
} from '../../../packages/@ruvyxa/core/dist/route-rules.js'

// The deployed handler imports the runtime copy, so the copy is what has to
// agree with the fixture too — a stale copy is the drift `sync:runtime --check`
// exists for, and this makes it fail here as well.
// @ts-expect-error the generated copy ships no declarations; it is the same source.
const runtimeCopy = (await import('../../../packages/ruvyxa/runtime/route-rules.mjs')) as Record<
  string,
  never
>

const fixture = JSON.parse(
  readFileSync(repoPath('tests/fixtures/route-rules-conformance.json'), 'utf8'),
)

function request(value: Record<string, unknown>): RuleRequest {
  return {
    path: value.path as string,
    query: value.query as string | undefined,
    headers: Object.entries((value.headers as Record<string, string>) ?? {}),
    host: value.host as string | undefined,
  }
}

for (const [host, api] of [
  [
    '@ruvyxa/core/route-rules',
    {
      compileSource,
      matchSource,
      evaluateConditions,
      compileHeaderRules,
      applyHeaderRules,
      compileRedirectRules,
      matchRedirect,
      compileRewriteRules,
      matchRewrite,
      compileMatcher,
      matcherMatches,
      normalizeMatcher,
    },
  ],
  ['runtime/route-rules.mjs', runtimeCopy],
] as const) {
  describe(`route rules conformance (${host})`, () => {
    for (const entry of fixture.sources) {
      const compiled = api.compileSource(entry.source)
      for (const [path, expected] of Object.entries(entry.cases)) {
        it(`${entry.source} against ${path}`, () => {
          assert.deepEqual(api.matchSource(compiled, path), expected)
        })
      }
    }

    for (const source of fixture.invalidSources) {
      it(`refuses the source ${JSON.stringify(source)}`, () => {
        assert.throws(() => api.compileSource(source), /RUV1602/)
      })
    }

    for (const testCase of fixture.conditions) {
      it(`conditions: ${testCase.$why}`, () => {
        assert.deepEqual(
          api.evaluateConditions(testCase.has, testCase.missing, request(testCase.request)),
          testCase.result,
        )
      })
    }

    for (const testCase of fixture.headers) {
      it(`headers: ${testCase.$why}`, () => {
        const rules = api.compileHeaderRules(testCase.rules)
        assert.deepEqual(api.applyHeaderRules(rules, request(testCase.request)), testCase.result)
      })
    }

    for (const testCase of fixture.redirects) {
      it(`redirects: ${testCase.$why}`, () => {
        const rules = api.compileRedirectRules(testCase.rules)
        assert.deepEqual(api.matchRedirect(rules, request(testCase.request)), testCase.result)
      })
    }

    for (const testCase of fixture.rewrites) {
      it(`rewrites: ${testCase.$why}`, () => {
        const rules = api.compileRewriteRules(testCase.rules)
        assert.equal(api.matchRewrite(rules, request(testCase.request)), testCase.result)
      })
    }

    for (const testCase of fixture.matcher) {
      it(`matcher ${JSON.stringify(testCase.matcher)} against ${testCase.request.path}`, () => {
        const entries = api.compileMatcher(api.normalizeMatcher(testCase.matcher) ?? [])
        assert.equal(api.matcherMatches(entries, request(testCase.request)), testCase.result)
      })
    }
  })
}

describe('route rules validation', () => {
  it('refuses a redirect that names neither permanent nor statusCode', () => {
    assert.throws(
      () => compileRedirectRules([{ source: '/a', destination: '/b' }]),
      /permanent or statusCode/,
    )
  })

  it('refuses a matcher entry without a source', () => {
    assert.throws(() => normalizeMatcher([{} as never]), /need a source/)
  })

  it('reads a bare rewrite list as afterFiles and keeps an absent matcher absent', () => {
    assert.deepEqual(normalizeRewrites([{ source: '/a', destination: '/b' }]), {
      beforeFiles: [],
      afterFiles: [{ source: '/a', destination: '/b' }],
      fallback: [],
    })
    assert.equal(normalizeMatcher(undefined), undefined)
    assert.deepEqual(normalizeMatcher('/about'), [{ source: '/about' }])
  })
})
