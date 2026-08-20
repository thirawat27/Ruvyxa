import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

import { repoPath } from '../../repo-root.ts'
import {
  fetchSiteIsCrossSite,
  originIsCrossSite,
  parseForwardedScheme,
} from '../../../packages/@ruvyxa/core/dist/origin-policy.js'

interface OriginCase {
  name: string
  headers: Record<string, string>
  host: string
  trustedScheme: 'http' | 'https' | null
  crossSite: boolean
}

const fixture = JSON.parse(
  readFileSync(repoPath('tests/fixtures/origin-policy-conformance.json'), 'utf8'),
) as {
  cases: OriginCase[]
  forwardedScheme: { cases: Array<{ header: string; scheme: 'http' | 'https' | null }> }
}

/**
 * The native host replays this same file in
 * `crates/ruvyxa_dev_server/src/action_security.rs`.
 *
 * Before the fixture existed, three implementations of this decision were kept
 * in step by a comment saying they mirrored each other: the action endpoint,
 * the native server, and — once it was written — the `originGuard` plugin. Two
 * other cross-language tables held that way (`STATIC_CONTENT_TYPES`,
 * `DEFAULT_SECURITY_HEADERS`) had already drifted in production before gaining
 * a fixture of their own.
 */
describe('origin policy contract', () => {
  for (const entry of fixture.cases) {
    it(entry.name, () => {
      const headers = new Headers(entry.headers)
      assert.equal(
        originIsCrossSite(headers, entry.host, { trustedScheme: entry.trustedScheme }),
        entry.crossSite,
        'the fixture decides the rule; the native host replays the same file',
      )
    })
  }

  for (const entry of fixture.forwardedScheme.cases) {
    it(`reads X-Forwarded-Proto ${JSON.stringify(entry.header)} as ${entry.scheme}`, () => {
      assert.equal(parseForwardedScheme(entry.header), entry.scheme)
    })
  }

  it('treats an absent forwarded header as stating nothing', () => {
    assert.equal(parseForwardedScheme(null), null)
    assert.equal(parseForwardedScheme(undefined), null)
  })

  it('reports an explicit cross-site fetch metadata header', () => {
    assert.equal(fetchSiteIsCrossSite(new Headers({ 'sec-fetch-site': 'cross-site' })), true)
    assert.equal(fetchSiteIsCrossSite(new Headers({ 'sec-fetch-site': 'Cross-Site' })), true)
    assert.equal(fetchSiteIsCrossSite(new Headers({ 'sec-fetch-site': 'same-origin' })), false)
    assert.equal(fetchSiteIsCrossSite(new Headers()), false)
  })

  it('accepts an explicitly allowed origin without consulting the host', () => {
    const headers = new Headers({ host: 'app.test', origin: 'https://partner.example' })
    assert.equal(originIsCrossSite(headers, 'app.test'), true)
    assert.equal(
      originIsCrossSite(headers, 'app.test', {
        allowOrigins: new Set(['https://partner.example']),
      }),
      false,
    )
  })

  it('does not let an allowed origin bypass the stripped-origin rule', () => {
    // The allow-list is consulted only once an `Origin` is present. A request
    // with no origin evidence at all must still fail closed, or naming any
    // partner origin would disable the guard for every request that omits one.
    const headers = new Headers({ host: 'app.test' })
    assert.equal(
      originIsCrossSite(headers, 'app.test', {
        allowOrigins: new Set(['https://partner.example']),
      }),
      true,
    )
  })
})
