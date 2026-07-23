import { definePlugin } from '@ruvyxa/core/config'

import type {
  AuthOptions,
  AuthProvider,
  AuthResult,
  AuthRuntime,
  AuthSession,
  AuthUser,
  OAuthProvider,
  OAuthTokenSet,
} from './types.js'

export * from './providers.js'
export * from './stores.js'
export type * from './types.js'

const MAX_BODY_BYTES = 32 * 1024
const PROVIDER_TIMEOUT_MS = 10_000
const OAUTH_STATE_TTL_SECONDS = 600
const MAGIC_LINK_TTL_SECONDS = 900
const RESERVED_OAUTH_PARAMETERS = new Set([
  'client_id',
  'redirect_uri',
  'response_type',
  'scope',
  'state',
  'code_challenge',
  'code_challenge_method',
])

/** Create an isolated auth runtime, its direct Request handler, and its Ruvyxa plugin. */
export function createAuth(options: AuthOptions): AuthRuntime {
  const settings = normalizeOptions(options)
  const plugin = definePlugin({
    name: 'ruvyxa:auth',
    setup({ addMiddleware, onBuildComplete }) {
      addMiddleware({
        routes: [`${settings.basePath}/*`],
        onRequest(request) {
          return handle(request)
        },
      })
      onBuildComplete(({ manifest }) => {
        if (manifest.profile === 'production') {
          if (!settings.store.durable || !settings.rateLimitStore.durable) {
            throw new AuthError(
              'RUV3105',
              'production auth requires durable session/token and rate-limit stores',
              500,
            )
          }
        }
      })
    },
  })

  async function handle(request: Request): Promise<Response | undefined> {
    try {
      return await dispatch(request)
    } catch (error) {
      try {
        await settings.onError?.(error, request)
      } catch {
        // Authentication responses must remain fail-closed even if observability fails.
      }
      return authFailure(error)
    }
  }

  async function dispatch(request: Request): Promise<Response | undefined> {
    const url = new URL(request.url)
    if (url.pathname === `${settings.basePath}/session` && request.method === 'GET') {
      return json(await getSession(request))
    }
    if (url.pathname === `${settings.basePath}/logout` && request.method === 'POST') {
      assertSameOrigin(request, settings.origin)
      return json(null, { headers: await logout(request) })
    }
    const loginMatch = matchPath(url.pathname, `${settings.basePath}/login/`)
    if (loginMatch && request.method === 'POST') {
      assertSameOrigin(request, settings.origin)
      const result = await login(loginMatch, await readJson(request), request)
      return json(result.session, { headers: result.headers })
    }
    const oauthMatch = matchOAuthPath(url.pathname, settings.basePath)
    if (oauthMatch) {
      const provider = oauthProvider(settings.providers[oauthMatch.provider], oauthMatch.provider)
      if (oauthMatch.phase === 'start' && request.method === 'GET') {
        return startOAuth(provider, request, settings)
      }
      if (oauthMatch.phase === 'callback' && request.method === 'GET') {
        return finishOAuth(provider, request, settings)
      }
    }
    if (url.pathname === `${settings.basePath}/magic-link` && request.method === 'POST') {
      assertSameOrigin(request, settings.origin)
      await startMagicLink(await readJson(request), request, settings)
      return json({ sent: true })
    }
    if (url.pathname === `${settings.basePath}/magic-link/callback` && request.method === 'GET') {
      // GET must not consume the token: email security scanners prefetch
      // links, and a consuming GET burns the token before the user clicks.
      return magicLinkConfirmPage(request, settings)
    }
    if (url.pathname === `${settings.basePath}/magic-link/callback` && request.method === 'POST') {
      assertSameOrigin(request, settings.origin)
      return finishMagicLink(request, settings)
    }
    if (url.pathname === `${settings.basePath}/webauthn/options` && request.method === 'POST') {
      assertSameOrigin(request, settings.origin)
      const provider = findProvider(settings.providers, 'webauthn')
      if (!provider || provider.type !== 'webauthn') throw notConfigured('webauthn')
      await consumeRateLimit(request, 'webauthn', settings)
      return json(await provider.options(await readJson(request), request))
    }
    if (url.pathname === `${settings.basePath}/webauthn/verify` && request.method === 'POST') {
      assertSameOrigin(request, settings.origin)
      const provider = findProvider(settings.providers, 'webauthn')
      if (!provider || provider.type !== 'webauthn') throw notConfigured('webauthn')
      await consumeRateLimit(request, 'webauthn', settings)
      const user = await provider.verify(await readJson(request), request)
      if (!user) throw invalidCredentials()
      const result = await issueSession(user, false, settings)
      return json(result.session, { headers: result.headers })
    }
    return undefined
  }

  async function login(
    providerName: string,
    input: Record<string, unknown>,
    request = new Request(`${settings.origin}${settings.basePath}/login/${providerName}`, {
      method: 'POST',
      headers: { origin: settings.origin },
    }),
  ): Promise<AuthResult> {
    const provider = settings.providers[providerName]
    if (!provider || provider.type !== 'credentials') throw notConfigured(providerName)
    await consumeRateLimit(request, loginRateKey(providerName, input), settings)
    const user = await provider.authorize(input, request)
    if (!user) throw invalidCredentials()
    validateUser(user)
    return issueSession(user, input.remember === true, settings)
  }

  async function getSession(request: Request): Promise<AuthSession | null> {
    const token = readCookie(request.headers.get('cookie'), settings.cookieName)
    if (!token) return null
    const key = await tokenKey('session', token, settings.secret)
    const serialized = await settings.store.get(key)
    if (!serialized) return null
    const session = parseSession(serialized)
    if (!session || Date.parse(session.expiresAt) <= Date.now()) {
      await settings.store.delete(key)
      return null
    }
    return session
  }

  async function logout(request: Request): Promise<Headers> {
    const token = readCookie(request.headers.get('cookie'), settings.cookieName)
    if (token) await settings.store.delete(await tokenKey('session', token, settings.secret))
    return new Headers({ 'set-cookie': deleteCookie(settings) })
  }

  return Object.freeze({ plugin, basePath: settings.basePath, handle, login, getSession, logout })
}

type NormalizedOptions = ReturnType<typeof normalizeOptions>

async function issueSession(
  user: AuthUser,
  remember: boolean,
  settings: NormalizedOptions,
): Promise<AuthResult> {
  validateUser(user)
  const token = randomToken(32)
  const ttlSeconds = remember ? settings.rememberTtlSeconds : settings.ttlSeconds
  const createdAt = new Date()
  const session: AuthSession = {
    id: await tokenKey('id', token, settings.secret),
    user: structuredClone(user),
    createdAt: createdAt.toISOString(),
    expiresAt: new Date(createdAt.getTime() + ttlSeconds * 1000).toISOString(),
    remember,
  }
  await settings.store.set(
    await tokenKey('session', token, settings.secret),
    JSON.stringify(session),
    ttlSeconds,
  )
  return {
    user: session.user,
    session,
    headers: new Headers({ 'set-cookie': sessionCookie(token, ttlSeconds, settings) }),
  }
}

async function startOAuth(
  provider: OAuthProvider,
  request: Request,
  settings: NormalizedOptions,
): Promise<Response> {
  const url = new URL(request.url)
  const returnTo = safeReturnTo(url.searchParams.get('returnTo'))
  await consumeRateLimit(request, `oauth:${provider.id}`, settings)
  const state = randomToken(24)
  const verifier = randomToken(48)
  await settings.store.set(
    await tokenKey('oauth', state, settings.secret),
    JSON.stringify({ verifier, returnTo }),
    OAUTH_STATE_TTL_SECONDS,
  )
  const target = new URL(provider.authorizationUrl)
  for (const [name, value] of Object.entries(provider.authorizationParams ?? {})) {
    target.searchParams.set(name, value)
  }
  target.searchParams.set('client_id', provider.clientId)
  target.searchParams.set(
    'redirect_uri',
    `${settings.origin}${settings.basePath}/oauth/${provider.id}/callback`,
  )
  target.searchParams.set('response_type', 'code')
  target.searchParams.set('scope', provider.scopes.join(' '))
  target.searchParams.set('state', state)
  target.searchParams.set('code_challenge_method', 'S256')
  target.searchParams.set('code_challenge', await sha256Base64Url(verifier))
  return new Response(null, {
    status: 302,
    headers: {
      location: target.href,
      'set-cookie': oauthStateCookie(state, settings),
      'cache-control': 'no-store',
    },
  })
}

async function finishOAuth(
  provider: OAuthProvider,
  request: Request,
  settings: NormalizedOptions,
): Promise<Response> {
  const url = new URL(request.url)
  const state = url.searchParams.get('state')
  const code = url.searchParams.get('code')
  if (!state || !code)
    throw new AuthError('RUV3103', 'OAuth callback is missing code or state', 400)
  if (readCookie(request.headers.get('cookie'), oauthStateCookieName(settings)) !== state) {
    throw new AuthError('RUV3103', 'OAuth state does not match the initiating browser', 400)
  }
  const stateValue = await settings.store.take(await tokenKey('oauth', state, settings.secret))
  if (!stateValue) throw new AuthError('RUV3103', 'OAuth state is invalid or expired', 400)
  const parsed = JSON.parse(stateValue) as { verifier?: unknown; returnTo?: unknown }
  if (typeof parsed.verifier !== 'string')
    throw new AuthError('RUV3103', 'OAuth state is invalid', 400)
  const body = new URLSearchParams({
    grant_type: 'authorization_code',
    code,
    client_id: provider.clientId,
    redirect_uri: `${settings.origin}${settings.basePath}/oauth/${provider.id}/callback`,
    code_verifier: parsed.verifier,
  })
  if (provider.clientSecret) body.set('client_secret', provider.clientSecret)
  const tokenResponse = await fetchWithTimeout(provider.tokenUrl, {
    method: 'POST',
    headers: { accept: 'application/json', 'content-type': 'application/x-www-form-urlencoded' },
    body,
  })
  if (!tokenResponse.ok) throw new AuthError('RUV3104', 'OAuth token exchange failed', 502)
  const tokenPayload = record(await tokenResponse.json())
  if (typeof tokenPayload.access_token !== 'string') {
    throw new AuthError('RUV3104', 'OAuth provider returned no access token', 502)
  }
  const tokens: OAuthTokenSet = {
    accessToken: tokenPayload.access_token,
    ...(typeof tokenPayload.token_type === 'string' ? { tokenType: tokenPayload.token_type } : {}),
    ...(typeof tokenPayload.scope === 'string' ? { scope: tokenPayload.scope } : {}),
    ...(typeof tokenPayload.expires_in === 'number' ? { expiresIn: tokenPayload.expires_in } : {}),
    ...(typeof tokenPayload.refresh_token === 'string'
      ? { refreshToken: tokenPayload.refresh_token }
      : {}),
    raw: tokenPayload,
  }
  const profileResponse = await fetchWithTimeout(provider.userInfoUrl, {
    headers: { authorization: `Bearer ${tokens.accessToken}`, accept: 'application/json' },
  })
  if (!profileResponse.ok) throw new AuthError('RUV3104', 'OAuth profile request failed', 502)
  const user = await provider.mapProfile(await profileResponse.json(), tokens)
  const result = await issueSession(user, true, settings)
  const headers = new Headers(result.headers)
  headers.append('set-cookie', deleteOAuthStateCookie(settings))
  headers.set('location', safeReturnTo(typeof parsed.returnTo === 'string' ? parsed.returnTo : '/'))
  return new Response(null, { status: 303, headers })
}

async function startMagicLink(
  input: Record<string, unknown>,
  request: Request,
  settings: NormalizedOptions,
): Promise<void> {
  const provider = findProvider(settings.providers, 'magic-link')
  if (!provider || provider.type !== 'magic-link') throw notConfigured('magic-link')
  const email = typeof input.email === 'string' ? input.email.trim().toLowerCase() : ''
  if (!/^\S+@\S+\.\S+$/.test(email) || email.length > 254) {
    throw new AuthError('RUV3101', 'A valid email address is required', 400)
  }
  await consumeRateLimit(request, `magic:${email}`, settings)
  const token = randomToken(32)
  const tokenStoreKey = await tokenKey('magic', token, settings.secret)
  await settings.store.set(tokenStoreKey, email, MAGIC_LINK_TTL_SECONDS)
  const target = new URL(`${settings.origin}${settings.basePath}/magic-link/callback`)
  target.searchParams.set('token', token)
  try {
    await provider.send({
      email,
      url: target.href,
      expiresAt: new Date(Date.now() + MAGIC_LINK_TTL_SECONDS * 1000),
    })
  } catch (error) {
    await settings.store.delete(tokenStoreKey)
    throw new AuthError('RUV3100', 'Magic link delivery failed', 503, undefined, { cause: error })
  }
}

/** Accepts base64url tokens from `randomToken` with headroom for store-specific sizes. */
const MAGIC_LINK_TOKEN_PATTERN = /^[A-Za-z0-9_-]{16,256}$/

/**
 * Render the confirmation page for a magic-link visit without consuming the
 * token. Consumption happens in the POST handler the page's form submits to,
 * so a prefetching mail scanner cannot invalidate the link.
 */
async function magicLinkConfirmPage(
  request: Request,
  settings: NormalizedOptions,
): Promise<Response> {
  const provider = findProvider(settings.providers, 'magic-link')
  if (!provider || provider.type !== 'magic-link') throw notConfigured('magic-link')
  const token = new URL(request.url).searchParams.get('token')
  if (!token || !MAGIC_LINK_TOKEN_PATTERN.test(token)) {
    return htmlPage(
      400,
      'Sign-in link is invalid',
      '<p>This sign-in link is invalid. Request a new one and try again.</p>',
    )
  }
  // Peek (never take): expiry feedback belongs on the page, consumption
  // belongs to the POST.
  const email = await settings.store.get(await tokenKey('magic', token, settings.secret))
  if (!email) {
    return htmlPage(
      400,
      'Sign-in link has expired',
      '<p>This sign-in link is invalid or has already been used. Request a new one.</p>',
    )
  }
  const action = escapeHtmlAttribute(`${settings.basePath}/magic-link/callback`)
  return htmlPage(
    200,
    'Confirm your sign-in',
    `<p>Select continue to finish signing in.</p>` +
      `<form method="post" action="${action}">` +
      `<input type="hidden" name="token" value="${escapeHtmlAttribute(token)}">` +
      `<button type="submit">Continue</button></form>`,
  )
}

async function finishMagicLink(request: Request, settings: NormalizedOptions): Promise<Response> {
  const provider = findProvider(settings.providers, 'magic-link')
  if (!provider || provider.type !== 'magic-link') throw notConfigured('magic-link')
  const token = await readCallbackToken(request)
  if (!token) throw new AuthError('RUV3103', 'Magic link token is missing', 400)
  const email = await settings.store.take(await tokenKey('magic', token, settings.secret))
  if (!email) throw new AuthError('RUV3103', 'Magic link is invalid or expired', 400)
  const user = await provider.resolveUser(email)
  if (!user) throw invalidCredentials()
  const result = await issueSession(user, true, settings)
  const headers = new Headers(result.headers)
  headers.set('location', '/')
  return new Response(null, { status: 303, headers })
}

/**
 * Read the magic-link token from the callback POST body: the confirmation
 * page submits `application/x-www-form-urlencoded`, programmatic clients may
 * send JSON.
 */
async function readCallbackToken(request: Request): Promise<string | null> {
  const contentType = request.headers.get('content-type') ?? ''
  let token: unknown
  if (contentType.includes('application/x-www-form-urlencoded')) {
    token = new URLSearchParams(await readBoundedBody(request)).get('token')
  } else {
    token = (await readJson(request)).token
  }
  return typeof token === 'string' && MAGIC_LINK_TOKEN_PATTERN.test(token) ? token : null
}

async function consumeRateLimit(
  request: Request,
  scope: string,
  settings: NormalizedOptions,
): Promise<void> {
  // Bind the bucket to the client IP whenever the deployment can vouch for
  // one; the user-agent fallback is client-rotatable and only used when no
  // resolver is configured. The resolver is opt-in (see AuthOptions.clientIp):
  // trusting forwarded headers without a proxy would hand attackers the same
  // rotation the IP is meant to prevent. The IP must be the whole key — mixing
  // the user-agent back in would reopen bucket rotation via UA rotation.
  const clientIp = resolveClientIp(request, settings)
  const clientKey =
    clientIp ?? `ua:${request.headers.get('user-agent')?.slice(0, 128) ?? 'unknown'}`
  const key = await tokenKey('rate', `${scope}:${clientKey}`, settings.secret)
  const decision = await settings.rateLimitStore.consume(
    key,
    settings.rateLimitMax,
    settings.rateLimitWindowSeconds,
  )
  if (!decision.allowed) {
    throw new AuthError(
      'RUV3102',
      'Too many authentication attempts',
      429,
      decision.retryAfterSeconds,
    )
  }
}

function resolveClientIp(request: Request, settings: NormalizedOptions): string | null {
  if (!settings.clientIp) return null
  try {
    const resolved = settings.clientIp(request)
    const trimmed = typeof resolved === 'string' ? resolved.trim().slice(0, 64) : ''
    return trimmed === '' ? null : `ip:${trimmed}`
  } catch {
    // A broken resolver must not take authentication down; fall back to the
    // user-agent bucket instead.
    return null
  }
}

/**
 * Read the client IP from `x-forwarded-for`, taking the rightmost entry: each
 * proxy appends the peer address it actually saw, so the rightmost value is
 * the one written by the nearest (trusted) proxy, while the leftmost entries
 * are client-supplied and spoofable. Use as `clientIp: forwardedClientIp`
 * only when a trusted proxy or platform edge sits in front of the app.
 */
export function forwardedClientIp(request: Request): string | null {
  const header = request.headers.get('x-forwarded-for')
  if (!header) return null
  const entries = header
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean)
  return entries.at(-1) ?? null
}

function normalizeOptions(options: AuthOptions) {
  if (!options || typeof options !== 'object') throw new TypeError('createAuth() requires options')
  if (typeof options.secret !== 'string' || options.secret.length < 32) {
    throw new TypeError('createAuth() secret must contain at least 32 characters')
  }
  const origin = new URL(options.origin)
  if (!['http:', 'https:'].includes(origin.protocol) || origin.pathname !== '/') {
    throw new TypeError('createAuth() origin must be an HTTP(S) origin without a path')
  }
  const basePath = normalizeBasePath(options.basePath ?? '/__ruvyxa/auth')
  if (
    !options.store?.take ||
    !options.store?.set ||
    !options.store?.get ||
    !options.store?.delete
  ) {
    throw new TypeError('createAuth() store must implement get, set, delete, and atomic take')
  }
  if (!options.rateLimitStore?.consume) {
    throw new TypeError('createAuth() requires an atomic rateLimitStore')
  }
  for (const [name, provider] of Object.entries(options.providers ?? {})) {
    if (!/^[a-z][a-z0-9-]{0,63}$/.test(name) || !provider || typeof provider !== 'object') {
      throw new TypeError(`createAuth() provider key "${name}" is invalid`)
    }
    if (provider.type === 'oauth' && provider.id !== name) {
      throw new TypeError(`createAuth() OAuth provider key "${name}" must match provider.id`)
    }
    if (provider.type === 'oauth') validateOAuthProvider(provider)
  }
  const secure = options.session?.secure ?? origin.protocol === 'https:'
  if (origin.protocol === 'https:' && !secure) {
    throw new TypeError('createAuth() cannot disable secure session cookies for an HTTPS origin')
  }
  const cookieName =
    options.session?.cookieName ?? (secure ? '__Host-ruvyxa.session' : 'ruvyxa.session')
  if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(cookieName)) {
    throw new TypeError('createAuth() session cookieName is invalid')
  }
  return {
    ...options,
    origin: origin.origin,
    basePath,
    secure,
    cookieName,
    sameSite: options.session?.sameSite ?? ('Lax' as const),
    ttlSeconds: boundedInteger(
      options.session?.ttlSeconds ?? 86_400,
      300,
      2_592_000,
      'session.ttlSeconds',
    ),
    rememberTtlSeconds: boundedInteger(
      options.session?.rememberTtlSeconds ?? 2_592_000,
      3600,
      31_536_000,
      'session.rememberTtlSeconds',
    ),
    rateLimitMax: boundedInteger(options.rateLimit?.max ?? 10, 1, 1000, 'rateLimit.max'),
    rateLimitWindowSeconds: boundedInteger(
      options.rateLimit?.windowSeconds ?? 60,
      1,
      3600,
      'rateLimit.windowSeconds',
    ),
  }
}

function assertSameOrigin(request: Request, origin: string): void {
  const requestOrigin = request.headers.get('origin')
  if (requestOrigin !== origin) {
    throw new AuthError('RUV3101', 'Cross-origin authentication request blocked', 403)
  }
}

async function readBoundedBody(request: Request): Promise<string> {
  const declared = Number(request.headers.get('content-length') ?? 0)
  if (Number.isFinite(declared) && declared > MAX_BODY_BYTES) {
    throw new AuthError('RUV3101', 'Authentication request body is too large', 413)
  }
  const reader = request.body?.getReader()
  const chunks: Uint8Array[] = []
  let total = 0
  if (reader) {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      total += value.byteLength
      if (total > MAX_BODY_BYTES) {
        await reader.cancel()
        throw new AuthError('RUV3101', 'Authentication request body is too large', 413)
      }
      chunks.push(value)
    }
  }
  const bodyBytes = new Uint8Array(total)
  let offset = 0
  for (const chunk of chunks) {
    bodyBytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  return new TextDecoder().decode(bodyBytes)
}

async function readJson(request: Request): Promise<Record<string, unknown>> {
  const body = await readBoundedBody(request)
  try {
    return record(body ? JSON.parse(body) : {})
  } catch {
    throw new AuthError('RUV3101', 'Authentication request must contain valid JSON', 400)
  }
}

function escapeHtmlAttribute(value: string): string {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;')
}

function htmlPage(status: number, title: string, body: string): Response {
  const escapedTitle = escapeHtmlAttribute(title)
  const html =
    '<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8">\n' +
    '<meta name="viewport" content="width=device-width, initial-scale=1">\n' +
    // Sign-in URLs carry tokens: keep them out of search indexes and referrers.
    '<meta name="robots" content="noindex">\n<meta name="referrer" content="no-referrer">\n' +
    `<title>${escapedTitle}</title>\n` +
    '<style>body{font-family:system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0}' +
    'main{max-width:24rem;padding:2rem;text-align:center}' +
    'button{font:inherit;padding:.6rem 1.4rem;cursor:pointer}</style>\n' +
    `</head>\n<body><main><h1>${escapedTitle}</h1>${body}</main></body>\n</html>\n`
  return new Response(html, {
    status,
    headers: { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' },
  })
}

function sessionCookie(token: string, ttlSeconds: number, settings: NormalizedOptions): string {
  return [
    `${settings.cookieName}=${token}`,
    'Path=/',
    'HttpOnly',
    `SameSite=${settings.sameSite}`,
    `Max-Age=${ttlSeconds}`,
    ...(settings.secure ? ['Secure'] : []),
  ].join('; ')
}

function deleteCookie(settings: NormalizedOptions): string {
  return [
    `${settings.cookieName}=`,
    'Path=/',
    'HttpOnly',
    `SameSite=${settings.sameSite}`,
    'Max-Age=0',
    ...(settings.secure ? ['Secure'] : []),
  ].join('; ')
}

function oauthStateCookieName(settings: NormalizedOptions): string {
  return `${settings.cookieName}.oauth`
}

function oauthStateCookie(state: string, settings: NormalizedOptions): string {
  return [
    `${oauthStateCookieName(settings)}=${state}`,
    'Path=/',
    'HttpOnly',
    'SameSite=Lax',
    `Max-Age=${OAUTH_STATE_TTL_SECONDS}`,
    ...(settings.secure ? ['Secure'] : []),
  ].join('; ')
}

function deleteOAuthStateCookie(settings: NormalizedOptions): string {
  return [
    `${oauthStateCookieName(settings)}=`,
    'Path=/',
    'HttpOnly',
    'SameSite=Lax',
    'Max-Age=0',
    ...(settings.secure ? ['Secure'] : []),
  ].join('; ')
}

function readCookie(header: string | null, name: string): string | null {
  for (const value of header?.split(';') ?? []) {
    const [key, ...rest] = value.trim().split('=')
    if (key === name) return rest.join('=') || null
  }
  return null
}

async function tokenKey(kind: string, token: string, secret: string): Promise<string> {
  const key = await crypto.subtle.importKey(
    'raw',
    new TextEncoder().encode(secret),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  )
  const signature = await crypto.subtle.sign(
    'HMAC',
    key,
    new TextEncoder().encode(`${kind}:${token}`),
  )
  return `${kind}:${base64Url(new Uint8Array(signature))}`
}

async function sha256Base64Url(value: string): Promise<string> {
  return base64Url(
    new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value))),
  )
}

function randomToken(bytes: number): string {
  const value = new Uint8Array(bytes)
  crypto.getRandomValues(value)
  return base64Url(value)
}

function base64Url(value: Uint8Array): string {
  let binary = ''
  for (const byte of value) binary += String.fromCharCode(byte)
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '')
}

function json(data: unknown, init: ResponseInit = {}): Response {
  const headers = new Headers(init.headers)
  headers.set('content-type', 'application/json; charset=utf-8')
  headers.set('cache-control', 'no-store')
  return new Response(JSON.stringify({ data }), { ...init, headers })
}

function authFailure(error: unknown): Response {
  const authError =
    error instanceof AuthError ? error : new AuthError('RUV3100', 'Authentication failed', 500)
  const headers = new Headers({
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
  })
  if (authError.retryAfterSeconds) headers.set('retry-after', String(authError.retryAfterSeconds))
  const publicMessage =
    authError.status >= 500 ? 'Authentication service unavailable' : authError.message
  return new Response(JSON.stringify({ error: publicMessage, code: authError.code }), {
    status: authError.status,
    headers,
  })
}

function validateUser(user: AuthUser): void {
  if (!user || typeof user.id !== 'string' || user.id.trim() === '' || user.id.length > 256) {
    throw new AuthError('RUV3100', 'Auth provider returned an invalid user', 500)
  }
}

function parseSession(value: string): AuthSession | null {
  try {
    const session = JSON.parse(value) as AuthSession
    validateUser(session.user)
    if (typeof session.id !== 'string' || typeof session.expiresAt !== 'string') return null
    return session
  } catch {
    return null
  }
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new AuthError('RUV3101', 'Expected a JSON object', 400)
  }
  return value as Record<string, unknown>
}

function oauthProvider(provider: AuthProvider | undefined, name: string): OAuthProvider {
  if (!provider || provider.type !== 'oauth') throw notConfigured(name)
  return provider
}

function validateOAuthProvider(provider: OAuthProvider): void {
  if (!provider.clientId.trim() || provider.scopes.length === 0) {
    throw new TypeError(`createAuth() OAuth provider "${provider.id}" requires clientId and scopes`)
  }
  for (const [name, value] of Object.entries(provider.authorizationParams ?? {})) {
    if (RESERVED_OAUTH_PARAMETERS.has(name)) {
      throw new TypeError(
        `createAuth() OAuth provider "${provider.id}" cannot override reserved parameter "${name}"`,
      )
    }
    if (typeof value !== 'string') {
      throw new TypeError(
        `createAuth() OAuth provider "${provider.id}" parameter "${name}" must be a string`,
      )
    }
  }
  for (const [name, value] of [
    ['authorizationUrl', provider.authorizationUrl],
    ['tokenUrl', provider.tokenUrl],
    ['userInfoUrl', provider.userInfoUrl],
  ] as const) {
    const endpoint = new URL(value)
    const localHttp =
      endpoint.protocol === 'http:' &&
      ['localhost', '127.0.0.1', '[::1]'].includes(endpoint.hostname)
    if (endpoint.protocol !== 'https:' && !localHttp) {
      throw new TypeError(
        `createAuth() OAuth provider "${provider.id}" ${name} must use HTTPS (HTTP is allowed only for localhost)`,
      )
    }
  }
}

function findProvider(
  providers: Readonly<Record<string, AuthProvider>>,
  type: AuthProvider['type'],
): AuthProvider | undefined {
  return Object.values(providers).find((provider) => provider.type === type)
}

function matchPath(pathname: string, prefix: string): string | null {
  if (!pathname.startsWith(prefix)) return null
  const value = pathname.slice(prefix.length)
  return value && !value.includes('/') ? safeDecodeComponent(value) : null
}

function matchOAuthPath(pathname: string, basePath: string) {
  const match = pathname.match(
    new RegExp(`^${escapeRegex(basePath)}/oauth/([^/]+)/(start|callback)$`),
  )
  if (!match) return null
  const provider = safeDecodeComponent(match[1]!)
  return provider ? { provider, phase: match[2] as 'start' | 'callback' } : null
}

/**
 * Decode a path segment, treating malformed percent-encoding such as `%ZZ` as
 * a non-match instead of letting the URIError surface as a 500.
 */
function safeDecodeComponent(value: string): string | null {
  try {
    return decodeURIComponent(value)
  } catch {
    return null
  }
}

function normalizeBasePath(value: string): string {
  if (!value.startsWith('/') || value.endsWith('/') || value.includes('*') || value.includes('?')) {
    throw new TypeError('Auth basePath must be an absolute path without a trailing slash')
  }
  return value
}

function safeReturnTo(value: string | null): string {
  if (typeof value !== 'string' || !value.startsWith('/')) return '/'
  // A naive "starts with / but not //" check still lets "/\evil.com" through:
  // browsers fold the backslash (and tabs/newlines) into "//authority", turning
  // the Location header into a cross-origin redirect. Resolve against a fixed
  // base and require the origin to survive — only a genuine same-origin path
  // round-trips, so every normalization trick collapses to "/".
  try {
    const resolved = new URL(value, 'http://ruvyxa.invalid')
    if (resolved.origin !== 'http://ruvyxa.invalid') return '/'
    return `${resolved.pathname}${resolved.search}${resolved.hash}`
  } catch {
    return '/'
  }
}

function loginRateKey(provider: string, input: Record<string, unknown>): string {
  const identity = typeof input.email === 'string' ? input.email.trim().toLowerCase() : 'anonymous'
  return `login:${provider}:${identity.slice(0, 254)}`
}

function boundedInteger(value: number, min: number, max: number, name: string): number {
  if (!Number.isSafeInteger(value) || value < min || value > max) {
    throw new TypeError(`createAuth() ${name} must be between ${min} and ${max}`)
  }
  return value
}

function notConfigured(provider: string): AuthError {
  return new AuthError('RUV3101', `Authentication provider "${provider}" is not configured`, 404)
}

function invalidCredentials(): AuthError {
  return new AuthError('RUV3101', 'Invalid credentials', 401)
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

async function fetchWithTimeout(input: string, init: RequestInit): Promise<Response> {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), PROVIDER_TIMEOUT_MS)
  try {
    return await fetch(input, { ...init, signal: controller.signal })
  } catch (error) {
    throw new AuthError('RUV3104', 'OAuth provider request failed', 502, undefined, {
      cause: error,
    })
  } finally {
    clearTimeout(timeout)
  }
}

export class AuthError extends Error {
  constructor(
    readonly code: 'RUV3100' | 'RUV3101' | 'RUV3102' | 'RUV3103' | 'RUV3104' | 'RUV3105',
    message: string,
    readonly status: number,
    readonly retryAfterSeconds?: number,
    options?: ErrorOptions,
  ) {
    super(message, options)
    this.name = 'AuthError'
  }
}
