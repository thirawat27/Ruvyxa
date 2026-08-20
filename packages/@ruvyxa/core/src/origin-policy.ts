/**
 * Cross-site request policy — the single JavaScript implementation.
 *
 * Three places in this repository have to answer "is this request provably
 * same-origin?": the action endpoint in
 * `packages/ruvyxa/runtime/action-runtime.mjs` (via the generated
 * `runtime/origin-policy.mjs` copy), the `originGuard` plugin in
 * `packages/ruvyxa/src/plugins/http.ts`, and the native server in
 * `crates/ruvyxa_dev_server/src/action_security.rs`. The first two read this
 * module. The Rust host cannot — it is a different language — so it is held to
 * the same behaviour by a shared case table instead:
 * `tests/fixtures/origin-policy-conformance.json` is replayed by both suites.
 *
 * The three used to hold three ports of these rules, kept in step by a comment
 * saying they mirrored each other. That is exactly the arrangement that let
 * `STATIC_CONTENT_TYPES` and `DEFAULT_SECURITY_HEADERS` drift.
 *
 * This module must stay dependency-free and free of Node and DOM APIs beyond
 * `Headers`. It is copied verbatim into serverless function bundles, where
 * nothing else is resolvable.
 */

/**
 * The request scheme as reported by a proxy the host decided to trust, or
 * `null` when nothing trustworthy stated it.
 *
 * This is the one input the three callers genuinely disagree about, so it is
 * an argument rather than something this module derives. The native server
 * weighs `X-Forwarded-Proto` only when the transport peer is in
 * `security.trustedProxyIps`; a deployed function has no peer address to weigh
 * and treats its platform ingress as trusted by construction; the plugin has
 * no trust policy to consult at all and always passes `null`.
 */
export type ForwardedScheme = 'http' | 'https' | null

export interface OriginPolicyOptions {
  /** Scheme a trusted proxy vouched for. Omit when nothing did. */
  trustedScheme?: ForwardedScheme
  /** Lowercased origins accepted in addition to the request's own. */
  allowOrigins?: ReadonlySet<string>
}

/** True when `Sec-Fetch-Site` explicitly reports a cross-site request. */
export function fetchSiteIsCrossSite(headers: Headers): boolean {
  const site = headers.get('sec-fetch-site')
  return typeof site === 'string' && site.toLowerCase() === 'cross-site'
}

/**
 * Read `X-Forwarded-Proto` into a scheme, taking the leftmost entry.
 *
 * Callers apply their own trust policy before calling this; a header value
 * that names anything other than http or https states nothing usable.
 */
export function parseForwardedScheme(value: string | null | undefined): ForwardedScheme {
  if (typeof value !== 'string') return null
  const scheme = value.split(',')[0]?.trim().toLowerCase()
  return scheme === 'https' || scheme === 'http' ? scheme : null
}

/**
 * True when the request is not provably same-origin.
 *
 * With no `Origin`, a `Sec-Fetch-Site: same-origin` header is the only accepted
 * substitute — failing closed keeps a stripped-origin cross-site form from
 * reaching a mutation endpoint.
 *
 * The host comparison is the load-bearing check: a browser sets `Origin`
 * itself and a cross-site page cannot forge it, so a matching host already
 * establishes same-origin intent. The scheme is asserted only when
 * `trustedScheme` says something observed it. Treating its absence as proof of
 * `http` once rejected every deployment whose TLS-terminating proxy was not
 * loopback and not listed as trusted.
 */
export function originIsCrossSite(
  headers: Headers,
  host: string,
  options: OriginPolicyOptions = {},
): boolean {
  const origin = headers.get('origin')
  if (typeof origin !== 'string' || origin === '') {
    const site = headers.get('sec-fetch-site')
    return !(typeof site === 'string' && site.toLowerCase() === 'same-origin')
  }
  if (options.allowOrigins?.has(origin.toLowerCase())) return false
  if (typeof host !== 'string' || host === '') return true

  const separator = origin.indexOf('://')
  if (separator < 0) return true
  const originScheme = origin.slice(0, separator)
  const originHost = origin.slice(separator + 3)
  if (originHost === '' || originHost.includes('/')) return true
  if (originHost.toLowerCase() !== host.toLowerCase()) return true

  const trusted = options.trustedScheme ?? null
  return trusted === null ? false : originScheme.toLowerCase() !== trusted
}
