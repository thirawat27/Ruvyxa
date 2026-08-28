/**
 * The deployed host's half of `tests/fixtures/client-ip-conformance.json`.
 *
 * Every per-client control in the framework is keyed on who the client is: the
 * built-in `rate` middleware, the server-action rate limiter, and the action
 * replay guard's per-client quota. Two implementations answered that question
 * and they disagreed — `RateLimitLayer::extract_key` in
 * `crates/ruvyxa_middleware/src/builtin.rs` read the transport peer and never
 * looked at a forwarded header, so one project with one `middleware.builtin.rate`
 * block was limited per real client once deployed and as a single shared bucket
 * when the native server ran behind a reverse proxy.
 *
 * The Rust half is `forwarded_scan_matches_the_shared_conformance_table` in
 * `crates/ruvyxa_middleware/src/client_ip.rs`.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { describe, it } from 'node:test'
import { fileURLToPath } from 'node:url'

const workspaceRoot = path.resolve(fileURLToPath(new URL('../../..', import.meta.url)))
const handlerPath = path.join(workspaceRoot, 'packages/ruvyxa/runtime/serverless-handler.mjs')

const { clientAddress, parseIngressHeaders, parseTrustedProxies } = await import(
  `file://${handlerPath.replaceAll('\\', '/')}`
)

const contract = JSON.parse(
  readFileSync(path.join(workspaceRoot, 'tests/fixtures/client-ip-conformance.json'), 'utf8'),
)

describe('serverless handler client identity', () => {
  for (const testCase of contract.cases) {
    it(testCase.name, () => {
      const headers = new Headers(testCase.headers)
      const trusted = parseTrustedProxies(testCase.trustedProxyIps)
      // The shared table records "nothing could be attributed" as null. This
      // host spells that `unknown`, which buckets more aggressively than the
      // traffic warrants — the direction a limiter is allowed to be wrong in.
      assert.equal(clientAddress(headers, trusted), testCase.client ?? 'unknown')
    })
  }

  it('ignores an unparseable trusted-proxy entry rather than widening trust', () => {
    // `ruvyxa build` validates these, so a bad entry here means a handler
    // assembled by hand. Skipping it narrows trust; treating it as a wildcard
    // would let the hop it failed to describe speak for the client.
    const trusted = parseTrustedProxies(['10.0.0.0/8', 'not-a-range', 42, null])
    const headers = new Headers({ 'x-forwarded-for': '203.0.113.9, 10.0.0.9' })
    assert.equal(clientAddress(headers, trusted), '203.0.113.9')
  })

  it('reads a declared platform ingress header before any forwarded chain', () => {
    // This step is the deployed host's alone and is why the shared table
    // starts after it: a Worker or an Edge Function has no transport peer to
    // weigh, and is reachable only through the ingress that set this header.
    for (const name of ['cf-connecting-ip', 'x-vercel-forwarded-for']) {
      const headers = new Headers({ [name]: '203.0.113.9', 'x-forwarded-for': '198.51.100.1' })
      assert.equal(clientAddress(headers, [], [name]), '203.0.113.9')
    }
  })

  /**
   * The header list is the adapter's declaration about its own platform, never
   * a guess from the request.
   *
   * Reading it unconditionally is what this did, and the standalone server the
   * node, bun, deno, aws, railway, and render adapters emit is an ordinary
   * `0.0.0.0` HTTP server with no ingress writing anything — so `CF-Connecting-IP`
   * on it is a header the caller typed. One client rotating a fresh value per
   * request got a fresh bucket per request, which is the built-in `rate`
   * middleware, the server-action limiter, and the replay quota defeated at
   * once. `stack.rs` already refuses `rate.key: "header:cf-connecting-ip"` in
   * so many words; the default path must not do quietly what the configured
   * path is rejected for.
   */
  it('ignores a platform ingress header no adapter declared', () => {
    for (const name of ['cf-connecting-ip', 'x-vercel-forwarded-for', 'true-client-ip']) {
      const headers = new Headers({ [name]: '203.0.113.9', 'x-forwarded-for': '198.51.100.1' })
      assert.equal(clientAddress(headers, []), '198.51.100.1')
    }
  })

  it('does not take an unparseable ingress value as an identity', () => {
    // The same rule the forwarded chain already applied: text that is not an
    // address identifies nobody, and handing it back is the unbounded key
    // rotation the limiter exists to prevent.
    const headers = new Headers({
      'cf-connecting-ip': 'not-an-ip-at-all',
      'x-forwarded-for': '203.0.113.9',
    })
    assert.equal(clientAddress(headers, [], ['cf-connecting-ip']), '203.0.113.9')
    assert.equal(
      clientAddress(new Headers({ 'cf-connecting-ip': 'junk' }), [], ['cf-connecting-ip']),
      'unknown',
    )
  })

  it('normalizes a declared header name so two spellings are one deployment', () => {
    const headers = new Headers({ 'cf-connecting-ip': '203.0.113.9' })
    assert.deepEqual(parseIngressHeaders(['CF-Connecting-IP', '  ', 42, null]), [
      'cf-connecting-ip',
    ])
    assert.equal(
      clientAddress(headers, [], parseIngressHeaders(['CF-Connecting-IP'])),
      '203.0.113.9',
    )
  })

  it('selects an IPv6 hop by the same rule as an IPv4 one', () => {
    // IPv6 stays out of the shared table because the two hosts spell an
    // address back differently and a bucket key never leaves the process that
    // made it. Which hop is chosen is the part that has to agree, so each host
    // covers it here.
    const trusted = parseTrustedProxies(['2001:db8::/32'])
    const headers = new Headers({ 'x-forwarded-for': '2001:db9::5, 2001:db8::2' })
    assert.equal(clientAddress(headers, trusted), '2001:db9:0:0:0:0:0:5')
  })
})
