import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  createAuth,
  github,
  google,
  memoryAuthStore,
  memoryRateLimitStore,
  type AuthUser,
  type OAuthProvider,
} from '../../../packages/@ruvyxa/auth/dist/index.js'
import { createAuthPlugin } from '../../../packages/@ruvyxa/auth/dist/plugin.js'

const origin = 'https://app.example.com'
const secret = 'test-secret-that-is-at-least-thirty-two-characters'

function runtime(overrides: Record<string, unknown> = {}) {
  return createAuth({
    secret,
    origin,
    store: memoryAuthStore({ development: true }),
    rateLimitStore: memoryRateLimitStore({ development: true }),
    providers: {
      email: {
        type: 'credentials',
        async authorize(input) {
          return input.email === 'ada@example.com' && input.password === 'correct'
            ? { id: 'user-1', email: 'ada@example.com', name: 'Ada' }
            : null
        },
      },
    },
    ...overrides,
  } as Parameters<typeof createAuth>[0])
}

function magicLinkRuntime(
  user: AuthUser,
  onSend: (url: string) => void,
  overrides: Record<string, unknown> = {},
) {
  return runtime({
    providers: {
      magic: {
        type: 'magic-link',
        async send(message: { url: string }) {
          onSend(message.url)
        },
        async resolveUser(email: string) {
          return email === user.email ? user : null
        },
      },
    },
    ...overrides,
  })
}

/**
 * The `Origin` a browser puts on a form-POST navigation submitted from
 * `pageHtml`. WHATWG Fetch, _Append a request `Origin` header_, step 3.1: for a
 * request whose mode is not `cors` and whose method is neither `GET` nor `HEAD`
 * — exactly a form-POST navigation — the serialized origin is replaced by the
 * literal `null` under `no-referrer`, and under `same-origin` only when the
 * request's origin differs from the target's. Every remaining policy nulls it
 * only on an https-to-http downgrade, which does not apply between two https
 * origins, so they all send the origin unchanged.
 */
function formPostOrigin(pageHtml: string, pageOrigin: string, targetOrigin: string): string {
  const policy = /<meta name="referrer" content="([^"]*)"/.exec(pageHtml)?.[1] ?? ''
  switch (policy) {
    case 'no-referrer':
      return 'null'
    case 'same-origin':
      return pageOrigin === targetOrigin ? pageOrigin : 'null'
    default:
      return pageOrigin
  }
}

describe('@ruvyxa/auth', () => {
  it('issues an HttpOnly secure session and resolves it from a request', async () => {
    const auth = runtime()
    const response = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/login/email`, {
        method: 'POST',
        headers: { origin, 'content-type': 'application/json' },
        body: JSON.stringify({ email: 'ada@example.com', password: 'correct', remember: true }),
      }),
    )
    assert.equal(response?.status, 200)
    const cookie = response?.headers.get('set-cookie') ?? ''
    assert.match(cookie, /__Host-ruvyxa\.session=/)
    assert.match(cookie, /HttpOnly/)
    assert.match(cookie, /Secure/)

    const session = await auth.getSession(
      new Request(`${origin}/dashboard`, { headers: { cookie: cookie.split(';')[0]! } }),
    )
    assert.equal(session?.user.email, 'ada@example.com')
    assert.equal(session?.remember, true)
  })

  it('derives session keys from the secret alone, so runtimes are interchangeable', async () => {
    // The derived HMAC key is owned by each runtime rather than by a process-global
    // map keyed on the secret. That ownership change must stay invisible to the
    // token bytes: a session issued by one runtime has to remain readable by the
    // next one started from the same secret (a redeploy, a second worker), and has
    // to stay unreadable to a runtime holding a different secret.
    const store = memoryAuthStore({ development: true })
    const rateLimitStore = memoryRateLimitStore({ development: true })

    const issuer = runtime({ store, rateLimitStore })
    const response = await issuer.handle(
      new Request(`${origin}/__ruvyxa/auth/login/email`, {
        method: 'POST',
        headers: { origin, 'content-type': 'application/json' },
        body: JSON.stringify({ email: 'ada@example.com', password: 'correct' }),
      }),
    )
    assert.ok(response)
    assert.equal(response.status, 200)
    const cookie = response.headers.get('set-cookie')!.split(';')[0]!

    const sameSecret = runtime({ store, rateLimitStore })
    const resolved = await sameSecret.getSession(
      new Request(`${origin}/dashboard`, { headers: { cookie } }),
    )
    assert.equal(resolved?.user.email, 'ada@example.com')

    const otherSecret = runtime({
      store,
      rateLimitStore,
      secret: 'a-completely-different-secret-of-sufficient-length',
    })
    assert.equal(
      await otherSecret.getSession(new Request(`${origin}/dashboard`, { headers: { cookie } })),
      null,
    )
  })

  it('rejects and deletes stored sessions with an invalid expiration date', async () => {
    const values = new Map<string, string>()
    let sessionKey = ''
    const store = {
      name: 'test-session-store',
      durable: true,
      async get(key: string) {
        return values.get(key) ?? null
      },
      async set(key: string, value: string) {
        sessionKey = key
        values.set(key, value)
      },
      async delete(key: string) {
        values.delete(key)
      },
      async take(key: string) {
        const value = values.get(key) ?? null
        values.delete(key)
        return value
      },
    }
    const auth = runtime({ store })
    const result = await auth.login('email', {
      email: 'ada@example.com',
      password: 'correct',
    })
    const cookie = result.headers.get('set-cookie')!.split(';')[0]!
    const persisted = JSON.parse(values.get(sessionKey)!) as Record<string, unknown>
    persisted.expiresAt = 'not-a-date'
    values.set(sessionKey, JSON.stringify(persisted))

    const session = await auth.getSession(new Request(origin, { headers: { cookie } }))

    assert.equal(session, null)
    assert.equal(values.has(sessionKey), false)
  })

  it('treats malformed percent-encoding in the provider path as no match', async () => {
    const auth = runtime()
    const response = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/login/%ZZ`, {
        method: 'POST',
        headers: { origin, 'content-type': 'application/json' },
        body: JSON.stringify({}),
      }),
    )
    // Not an auth route: the handler passes it through instead of failing 500.
    assert.equal(response, undefined)
  })

  it('blocks cross-origin login and does not reveal credential details', async () => {
    const auth = runtime()
    const crossOrigin = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/login/email`, {
        method: 'POST',
        headers: { origin: 'https://evil.example' },
        body: '{}',
      }),
    )
    assert.equal(crossOrigin?.status, 403)
    const response = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/login/email`, {
        method: 'POST',
        headers: { origin },
        body: JSON.stringify({ email: 'ada@example.com', password: 'wrong' }),
      }),
    )
    assert.equal(response?.status, 401)
    assert.deepEqual(await response?.json(), { error: 'Invalid credentials', code: 'RUV3101' })
  })

  it('reports full provider failures server-side without leaking them to the client', async () => {
    const providerError = new Error('database host and secret detail')
    let observed: unknown
    const auth = runtime({
      providers: {
        email: {
          type: 'credentials',
          async authorize() {
            throw providerError
          },
        },
      },
      onError(error: unknown) {
        observed = error
      },
    })
    const response = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/login/email`, {
        method: 'POST',
        headers: { origin },
        body: JSON.stringify({ email: 'ada@example.com', password: 'wrong' }),
      }),
    )
    assert.equal(observed, providerError)
    assert.equal(response?.status, 500)
    assert.doesNotMatch(await response!.text(), /database host|secret detail/)
  })

  it('applies the shared auth rate limit without trusting forwarded IP headers', async () => {
    const auth = runtime({ rateLimit: { max: 1, windowSeconds: 60 } })
    const request = () =>
      new Request(`${origin}/__ruvyxa/auth/login/email`, {
        method: 'POST',
        headers: { origin, 'user-agent': 'test-agent', 'x-forwarded-for': crypto.randomUUID() },
        body: JSON.stringify({ email: 'ada@example.com', password: 'wrong' }),
      })
    assert.equal((await auth.handle(request()))?.status, 401)
    const limited = await auth.handle(request())
    assert.equal(limited?.status, 429)
    assert.equal(limited?.headers.get('retry-after'), '60')
  })

  it('binds rate-limit buckets to the resolved client IP when configured', async () => {
    const auth = runtime({
      rateLimit: { max: 1, windowSeconds: 60 },
      clientIp: (request: Request) => request.headers.get('x-test-ip'),
    })
    const request = (userAgent: string, ip: string) =>
      new Request(`${origin}/__ruvyxa/auth/login/email`, {
        method: 'POST',
        headers: { origin, 'user-agent': userAgent, 'x-test-ip': ip },
        body: JSON.stringify({ email: 'ada@example.com', password: 'wrong' }),
      })
    // Rotating the user-agent no longer rotates the bucket: the IP pins it.
    assert.equal((await auth.handle(request('agent-a', '203.0.113.7')))?.status, 401)
    assert.equal((await auth.handle(request('agent-b', '203.0.113.7')))?.status, 429)
    // A different client IP is a different bucket.
    assert.equal((await auth.handle(request('agent-a', '203.0.113.8')))?.status, 401)
  })

  it('caps how many identities one client may attempt, not just attempts per identity', async () => {
    // The bucket key contains the email, so a per-identity limit alone let one
    // source try `max` passwords against an unlimited number of accounts —
    // exactly the shape of credential stuffing and account enumeration. A
    // second bucket keyed only by the client caps the total.
    const auth = runtime({
      rateLimit: { max: 1, windowSeconds: 60 },
      clientIp: (request: Request) => request.headers.get('x-test-ip'),
    })
    const attempt = (email: string, ip: string) =>
      auth.handle(
        new Request(`${origin}/__ruvyxa/auth/login/email`, {
          method: 'POST',
          headers: { origin, 'x-test-ip': ip },
          body: JSON.stringify({ email, password: 'wrong' }),
        }),
      )

    // max = 1, client multiplier = 5, so the client budget is 5 attempts total
    // however many distinct identities they are spread across.
    const statuses: number[] = []
    for (let index = 0; index < 7; index += 1) {
      statuses.push((await attempt(`victim-${index}@example.com`, '203.0.113.9'))!.status)
    }

    assert.deepEqual(
      statuses,
      [401, 401, 401, 401, 401, 429, 429],
      'the client-only bucket must stop the sweep once its budget is spent',
    )

    // A different client is unaffected — the cap is per client, not global.
    assert.equal((await attempt('victim-0@example.com', '203.0.113.10'))!.status, 401)
  })

  it('still limits a single identity before the client budget is reached', async () => {
    // The per-identity bucket must keep working: hammering one account has to
    // stop at `max`, well before the larger client budget.
    const auth = runtime({
      rateLimit: { max: 2, windowSeconds: 60 },
      clientIp: (request: Request) => request.headers.get('x-test-ip'),
    })
    const attempt = () =>
      auth.handle(
        new Request(`${origin}/__ruvyxa/auth/login/email`, {
          method: 'POST',
          headers: { origin, 'x-test-ip': '203.0.113.11' },
          body: JSON.stringify({ email: 'ada@example.com', password: 'wrong' }),
        }),
      )

    assert.equal((await attempt())!.status, 401)
    assert.equal((await attempt())!.status, 401)
    assert.equal((await attempt())!.status, 429)
  })

  it('caps how many clients may sweep one identity, not just attempts per client', async () => {
    // The transposed case of the test above, and the one neither of the two
    // original buckets could see: both fold the client into the key, so one
    // identity swept from arbitrarily many clients had no ceiling at all. A
    // third bucket keyed on the scope alone is the only thing that bounds it.
    const auth = runtime({
      rateLimit: { max: 1, windowSeconds: 60 },
      clientIp: (request: Request) => request.headers.get('x-test-ip'),
    })
    const attempt = (email: string, ip: string) =>
      auth.handle(
        new Request(`${origin}/__ruvyxa/auth/login/email`, {
          method: 'POST',
          headers: { origin, 'x-test-ip': ip },
          body: JSON.stringify({ email, password: 'wrong' }),
        }),
      )

    // max = 1, identity multiplier = 20, so one account absorbs 20 attempts in
    // a window however many distinct clients they arrive from. Without the
    // third bucket all 24 of these pass: each client is fresh for both of the
    // client-keyed buckets.
    const statuses: number[] = []
    for (let index = 0; index < 24; index += 1) {
      statuses.push((await attempt('ada@example.com', `198.51.100.${index}`))!.status)
    }

    assert.deepEqual(
      statuses,
      [...Array.from({ length: 20 }, () => 401), 429, 429, 429, 429],
      'the identity bucket must stop a distributed sweep of one account',
    )

    // The ceiling is per identity, not global: a fresh client attempting a
    // different account is unaffected, so burning one account's budget cannot
    // take the whole login endpoint down.
    assert.equal((await attempt('grace@example.com', '198.51.100.200'))!.status, 401)
  })

  it('gives a magic-link send a tighter identity budget than a credential attempt', async () => {
    // `startMagicLink` sends mail before any user exists, deliberately, so as
    // not to leak enumeration — which makes the endpoint an outbound-mail
    // amplifier unless its own budget is smaller than a password check's.
    const sent: string[] = []
    const auth = magicLinkRuntime(
      { id: 'magic-flood', email: 'flood@example.com' },
      (url) => sent.push(url),
      { clientIp: (request: Request) => request.headers.get('x-test-ip') },
    )
    const request = (ip: string) =>
      auth.handle(
        new Request(`${origin}/__ruvyxa/auth/magic-link`, {
          method: 'POST',
          headers: { origin, 'x-test-ip': ip },
          body: JSON.stringify({ email: 'flood@example.com' }),
        }),
      )

    // Default max = 10, so a credential attempt gets an identity ceiling of
    // 200; mail gets 10. Each request here comes from a distinct client, so
    // nothing but the identity bucket can refuse them.
    const statuses: number[] = []
    for (let index = 0; index < 14; index += 1) {
      statuses.push((await request(`198.51.100.${index}`))!.status)
    }

    assert.deepEqual(
      statuses,
      [...Array.from({ length: 10 }, () => 200), 429, 429, 429, 429],
      'one address must not be mailable from an unbounded number of clients',
    )
    assert.equal(sent.length, 10, 'a refused request must not have sent mail')
  })

  it('spends one client magic-link budget faster than its login budget', async () => {
    const sent: string[] = []
    const auth = magicLinkRuntime(
      { id: 'magic-single', email: 'single@example.com' },
      (url) => sent.push(url),
      { clientIp: (request: Request) => request.headers.get('x-test-ip') },
    )
    const request = () =>
      auth.handle(
        new Request(`${origin}/__ruvyxa/auth/magic-link`, {
          method: 'POST',
          headers: { origin, 'x-test-ip': '198.51.100.50' },
          body: JSON.stringify({ email: 'single@example.com' }),
        }),
      )

    // Default max = 10 buys ten wrong passwords from one client, but only two
    // emails: sending mail is not the same cost as checking a password.
    const statuses: number[] = []
    for (let index = 0; index < 3; index += 1) {
      statuses.push((await request())!.status)
    }
    assert.deepEqual(statuses, [200, 200, 429])
    assert.equal(sent.length, 2)
  })

  it('keeps a scope that names no account free of any shared ceiling', async () => {
    // `webauthn` and `oauth:<provider>` are mechanisms, not identities. A
    // bucket keyed on one of those alone would be a single global ceiling for
    // every user of the endpoint — a denial-of-service primitive, not a
    // defence — so the identity bucket must not be applied to them.
    const auth = runtime({
      rateLimit: { max: 1, windowSeconds: 60 },
      clientIp: (request: Request) => request.headers.get('x-test-ip'),
      providers: {
        passkey: {
          type: 'webauthn',
          async options() {
            return { challenge: 'adapter-owned' }
          },
          async verify() {
            return { id: 'passkey-1' }
          },
        },
      },
    })
    const request = (ip: string) =>
      auth.handle(
        new Request(`${origin}/__ruvyxa/auth/webauthn/options`, {
          method: 'POST',
          headers: { origin, 'x-test-ip': ip },
          body: '{}',
        }),
      )

    const statuses: number[] = []
    for (let index = 0; index < 40; index += 1) {
      statuses.push((await request(`198.51.100.${index}`))!.status)
    }
    assert.deepEqual(
      statuses.filter((status) => status !== 200),
      [],
      'a shared mechanism scope must stay reachable from any number of clients',
    )
  })

  it('expires logout cookies and deletes the stored session', async () => {
    const auth = runtime()
    const result = await auth.login('email', { email: 'ada@example.com', password: 'correct' })
    const cookie = result.headers.get('set-cookie')!.split(';')[0]!
    const request = new Request(`${origin}/__ruvyxa/auth/logout`, {
      method: 'POST',
      headers: { origin, cookie },
    })
    const response = await auth.handle(request)
    assert.match(response?.headers.get('set-cookie') ?? '', /Max-Age=0/)
    assert.equal(await auth.getSession(new Request(origin, { headers: { cookie } })), null)
  })

  it('consumes magic links exactly once', async () => {
    let sentUrl = ''
    const user: AuthUser = { id: 'magic-1', email: 'magic@example.com' }
    const auth = magicLinkRuntime(user, (url) => {
      sentUrl = url
    })
    const start = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/magic-link`, {
        method: 'POST',
        headers: { origin },
        body: JSON.stringify({ email: user.email }),
      }),
    )
    assert.equal(start?.status, 200)

    // GET renders a confirmation page without consuming the token, so a mail
    // scanner prefetching the link cannot invalidate it.
    const scannerVisit = await auth.handle(new Request(sentUrl))
    assert.equal(scannerVisit?.status, 200)
    assert.match(await scannerVisit!.text(), /method="post"/)
    const userVisit = await auth.handle(new Request(sentUrl))
    assert.equal(userVisit?.status, 200)
    const confirmPage = await userVisit!.text()

    // The page's form POST consumes the token exactly once. The `Origin` is the
    // one a real browser derives from the page's own referrer policy, not one
    // the test hands the endpoint: under `no-referrer` that is the literal
    // `null` and the endpoint's `assertSameOrigin` refuses the page's own form.
    const token = new URL(sentUrl).searchParams.get('token')!
    const browserOrigin = formPostOrigin(confirmPage, origin, origin)
    const confirm = () =>
      auth.handle(
        new Request(`${origin}/__ruvyxa/auth/magic-link/callback`, {
          method: 'POST',
          headers: {
            origin: browserOrigin,
            'content-type': 'application/x-www-form-urlencoded',
          },
          body: new URLSearchParams({ token }).toString(),
        }),
      )
    const first = await confirm()
    const replay = await confirm()
    assert.equal(first?.status, 303)
    assert.match(first?.headers.get('set-cookie') ?? '', /HttpOnly/)
    assert.equal(replay?.status, 400)

    // A consumed token renders the expired page instead of a fresh form.
    const staleVisit = await auth.handle(new Request(sentUrl))
    assert.equal(staleVisit?.status, 400)
  })

  it('never pairs a rendered form with a referrer policy that nulls its own Origin', async () => {
    // Two correct decisions that must not meet on one page: keep the sign-in
    // token out of referrers, and require `Origin` on a state-changing POST.
    // A `no-referrer` document makes the browser send `Origin: null` on its own
    // form submission, which `assertSameOrigin` then refuses. This walks every
    // page `htmlPage` renders so the pair cannot be reintroduced silently.
    let sentUrl = ''
    const user: AuthUser = { id: 'magic-2', email: 'referrer@example.com' }
    const auth = magicLinkRuntime(user, (url) => {
      sentUrl = url
    })
    const callback = `${origin}/__ruvyxa/auth/magic-link/callback`
    await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/magic-link`, {
        method: 'POST',
        headers: { origin },
        body: JSON.stringify({ email: user.email }),
      }),
    )

    const rendered = [
      // Malformed token: the "invalid link" page.
      await auth.handle(new Request(`${callback}?token=not+a+token`)),
      // Well-formed token that no longer exists: the "expired link" page.
      await auth.handle(new Request(`${callback}?token=${'A'.repeat(32)}`)),
      // The live token: the confirmation page, which carries the form.
      await auth.handle(new Request(sentUrl)),
    ]

    let formsWalked = 0
    for (const page of rendered) {
      assert.ok(page)
      const html = await page.text()
      assert.match(html, /<meta name="referrer" content="/)
      if (!html.includes('<form')) continue
      formsWalked += 1
      assert.equal(
        formPostOrigin(html, origin, origin),
        origin,
        'a page rendering a form must not declare a referrer policy that nulls its own Origin',
      )
    }
    assert.ok(formsWalked > 0, 'no form page was walked, so the guard proved nothing')
  })

  it('delegates WebAuthn verification and applies the shared session policy', async () => {
    const auth = runtime({
      providers: {
        passkey: {
          type: 'webauthn',
          async options() {
            return { challenge: 'adapter-owned' }
          },
          async verify() {
            return { id: 'passkey-1' }
          },
        },
      },
    })
    const options = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/webauthn/options`, {
        method: 'POST',
        headers: { origin },
        body: '{}',
      }),
    )
    assert.deepEqual(await options?.json(), { data: { challenge: 'adapter-owned' } })
    const verified = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/webauthn/verify`, {
        method: 'POST',
        headers: { origin },
        body: '{}',
      }),
    )
    assert.equal(verified?.status, 200)
    assert.match(verified?.headers.get('set-cookie') ?? '', /HttpOnly/)
  })

  it('rate limits WebAuthn challenge options like every other credential endpoint', async () => {
    const auth = runtime({
      rateLimit: { max: 1, windowSeconds: 60 },
      providers: {
        passkey: {
          type: 'webauthn',
          async options() {
            return { challenge: 'adapter-owned' }
          },
          async verify() {
            return { id: 'passkey-1' }
          },
        },
      },
    })
    const request = () =>
      new Request(`${origin}/__ruvyxa/auth/webauthn/options`, {
        method: 'POST',
        headers: { origin },
        body: '{}',
      })
    assert.equal((await auth.handle(request()))?.status, 200)
    const limited = await auth.handle(request())
    assert.equal(limited?.status, 429)
    assert.equal(limited?.headers.get('retry-after'), '60')
  })

  it('uses OAuth PKCE, consumes state once, and never returns provider tokens', async () => {
    const originalFetch = globalThis.fetch
    const fetchCalls: Array<{ url: string; init?: RequestInit }> = []
    globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input)
      fetchCalls.push({ url, init })
      if (url === 'https://provider.example/token') {
        return Response.json({ access_token: 'provider-secret-token', token_type: 'Bearer' })
      }
      return Response.json({ sub: 'oauth-1', email: 'oauth@example.com' })
    }) as typeof fetch
    try {
      const auth = runtime({
        providers: {
          example: {
            type: 'oauth',
            id: 'example',
            authorizationUrl: 'https://provider.example/authorize',
            tokenUrl: 'https://provider.example/token',
            userInfoUrl: 'https://provider.example/me',
            clientId: 'client-id',
            clientSecret: 'client-secret',
            scopes: ['openid'],
            mapProfile(profile: unknown) {
              const value = profile as { sub: string; email: string }
              return { id: value.sub, email: value.email }
            },
          },
        },
      })
      const start = await auth.handle(
        new Request(`${origin}/__ruvyxa/auth/oauth/example/start?returnTo=%2Fdashboard`, {
          headers: { 'user-agent': 'oauth-test' },
        }),
      )
      assert.equal(start?.status, 302)
      const authorization = new URL(start?.headers.get('location') ?? '')
      assert.equal(authorization.searchParams.get('code_challenge_method'), 'S256')
      assert.ok(authorization.searchParams.get('code_challenge'))
      const state = authorization.searchParams.get('state')!
      const callback = `${origin}/__ruvyxa/auth/oauth/example/callback?code=one&state=${state}`
      const stateCookie = start?.headers.get('set-cookie')?.split(';')[0]
      assert.match(stateCookie ?? '', /\.oauth=/)
      const completed = await auth.handle(
        new Request(callback, { headers: { cookie: stateCookie! } }),
      )
      const replay = await auth.handle(new Request(callback, { headers: { cookie: stateCookie! } }))
      assert.equal(completed?.status, 303)
      assert.equal(completed?.headers.get('location'), '/dashboard')
      assert.doesNotMatch(await completed!.clone().text(), /provider-secret-token/)
      assert.equal(replay?.status, 400)
      assert.match(String(fetchCalls[0]?.init?.body), /code_verifier=/)
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  it('neutralizes backslash open-redirect payloads in returnTo', async () => {
    const originalFetch = globalThis.fetch
    globalThis.fetch = (async (input: string | URL | Request) => {
      if (String(input) === 'https://provider.example/token') {
        return Response.json({ access_token: 'provider-secret-token', token_type: 'Bearer' })
      }
      return Response.json({ sub: 'oauth-1', email: 'oauth@example.com' })
    }) as typeof fetch
    try {
      const auth = runtime({
        providers: {
          example: {
            type: 'oauth',
            id: 'example',
            authorizationUrl: 'https://provider.example/authorize',
            tokenUrl: 'https://provider.example/token',
            userInfoUrl: 'https://provider.example/me',
            clientId: 'client-id',
            clientSecret: 'client-secret',
            scopes: ['openid'],
            mapProfile(profile: unknown) {
              const value = profile as { sub: string; email: string }
              return { id: value.sub, email: value.email }
            },
          },
        },
      })
      // Attacker sends "/\evil.com": a browser folds the backslash into
      // "//evil.com" and follows the Location cross-origin. The callback must
      // collapse the payload back to a same-origin "/".
      const start = await auth.handle(
        new Request(`${origin}/__ruvyxa/auth/oauth/example/start?returnTo=%2F%5Cevil.com`, {
          headers: { 'user-agent': 'oauth-redirect-test' },
        }),
      )
      const authorization = new URL(start?.headers.get('location') ?? '')
      const state = authorization.searchParams.get('state')!
      const stateCookie = start?.headers.get('set-cookie')?.split(';')[0]
      const completed = await auth.handle(
        new Request(`${origin}/__ruvyxa/auth/oauth/example/callback?code=one&state=${state}`, {
          headers: { cookie: stateCookie! },
        }),
      )
      assert.equal(completed?.status, 303)
      assert.equal(completed?.headers.get('location'), '/')
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  it('binds OAuth state to the initiating browser and protects PKCE parameters', async () => {
    const provider = {
      type: 'oauth' as const,
      id: 'example',
      authorizationUrl: 'https://provider.example/authorize',
      tokenUrl: 'https://provider.example/token',
      userInfoUrl: 'https://provider.example/me',
      clientId: 'client-id',
      scopes: ['openid'],
      mapProfile() {
        return { id: 'oauth-user' }
      },
    }
    const auth = runtime({ providers: { example: provider } })
    const start = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/oauth/example/start`, {
        headers: { 'user-agent': 'oauth-binding-test' },
      }),
    )
    const authorization = new URL(start?.headers.get('location') ?? '')
    const state = authorization.searchParams.get('state')!
    const callback = `${origin}/__ruvyxa/auth/oauth/example/callback?code=one&state=${state}`
    assert.equal((await auth.handle(new Request(callback)))?.status, 400)

    assert.throws(
      () =>
        runtime({
          providers: {
            example: { ...provider, authorizationParams: { state: 'attacker-controlled' } },
          },
        }),
      /cannot override reserved parameter/,
    )
  })

  it('refuses development stores in production plugin builds', async () => {
    const auth = runtime()
    let hook: ((context: unknown) => void | Promise<void>) | undefined
    await auth.plugin.register({
      environment: 'production',
      http: { onRequest() {}, onResponse() {}, route() {} },
      build: {
        onStart() {},
        onResolve() {},
        onLoad() {},
        onTransform() {},
        onComplete(value) {
          hook = value as typeof hook
        },
      },
      dev: { onFileChange() {} },
      diagnostics: { report() {} },
      native: { claim() {} },
    })
    await assert.rejects(
      async () => hook?.({ manifest: { profile: 'production' } }),
      /RUV3105|production auth requires durable/,
    )
    assert.throws(() => memoryAuthStore({} as never), /development: true/)
  })

  it('exposes the explicit plugin integration entry', () => {
    const plugin = createAuthPlugin({
      basePath: '/auth',
      async handle() {
        return undefined
      },
      validateBuild() {},
    })

    assert.equal(plugin.name, 'ruvyxa:auth')
  })

  it('keeps token keys bound to their own secret across runtimes', async () => {
    // Derived HMAC keys are memoized per secret. If that cache ever collapsed
    // two secrets onto one key, a session minted by one deployment would
    // resolve in another — so assert the isolation directly rather than
    // trusting the cache key.
    const shared = memoryAuthStore({ development: true })
    const first = runtime({ store: shared })
    const second = runtime({
      store: shared,
      secret: 'a-completely-different-secret-of-sufficient-length',
    })

    const login = async (auth: ReturnType<typeof runtime>) =>
      (
        await auth.handle(
          new Request(`${origin}/__ruvyxa/auth/login/email`, {
            method: 'POST',
            headers: { origin, 'content-type': 'application/json' },
            body: JSON.stringify({ email: 'ada@example.com', password: 'correct' }),
          }),
        )
      )?.headers
        .get('set-cookie')
        ?.split(';')[0]

    const cookie = await login(first)
    assert.ok(cookie, 'login must issue a session cookie')

    const request = () => new Request(`${origin}/dashboard`, { headers: { cookie } })
    // Same secret, repeated derivations: the memoized key must stay usable.
    assert.equal((await first.getSession(request()))?.user.email, 'ada@example.com')
    assert.equal((await first.getSession(request()))?.user.email, 'ada@example.com')
    // Different secret over the same store: must not resolve.
    assert.equal(await second.getSession(request()), null)
  })

  it('records whether the identity provider verified the address it returned', async () => {
    // OIDC Core §5.7: `email` is not a verified identifier unless
    // `email_verified` is true. Account identity here is keyed on `id`, so this
    // is not a takeover by itself — but an application that links an OAuth
    // login to an existing credentials account by address has no way to ask the
    // question unless the mapping carries the answer forward.
    const tokens = { accessToken: 'token', raw: {} }
    const map = async (provider: OAuthProvider, profile: unknown): Promise<AuthUser> =>
      provider.mapProfile(profile, tokens)
    const provider = google({ clientId: 'id', clientSecret: 'secret' })
    assert.equal(
      (await map(provider, { sub: '1', email: 'ada@example.com', email_verified: true }))
        .emailVerified,
      true,
    )
    assert.equal(
      (await map(provider, { sub: '1', email: 'ada@example.com', email_verified: false }))
        .emailVerified,
      false,
    )
    // Absent is not verified. A provider that says nothing has not vouched.
    assert.equal((await map(provider, { sub: '1', email: 'ada@example.com' })).emailVerified, false)
    // No address, no claim about one.
    assert.equal((await map(provider, { sub: '1' })).emailVerified, undefined)
    // GitHub's `/user` email is selected from the account's verified addresses.
    assert.equal(
      (
        await map(github({ clientId: 'id', clientSecret: 'secret' }), {
          id: 7,
          email: 'ada@example.com',
        })
      ).emailVerified,
      true,
    )
  })

  it('carries emailVerified through the session it stores', async () => {
    const originalFetch = globalThis.fetch
    globalThis.fetch = (async (input: string | URL | Request) => {
      if (String(input) === 'https://oauth2.googleapis.com/token') {
        return Response.json({ access_token: 'provider-secret-token', token_type: 'Bearer' })
      }
      return Response.json({ sub: 'g-1', email: 'ada@example.com', email_verified: false })
    }) as typeof fetch
    try {
      const auth = runtime({
        providers: { google: google({ clientId: 'client-id', clientSecret: 'client-secret' }) },
      })
      const start = await auth.handle(
        new Request(`${origin}/__ruvyxa/auth/oauth/google/start`, {
          headers: { 'user-agent': 'oauth-email-verified' },
        }),
      )
      const state = new URL(start?.headers.get('location') ?? '').searchParams.get('state')!
      const stateCookie = start?.headers.get('set-cookie')?.split(';')[0]
      const completed = await auth.handle(
        new Request(`${origin}/__ruvyxa/auth/oauth/google/callback?code=one&state=${state}`, {
          headers: { cookie: stateCookie! },
        }),
      )
      assert.equal(completed?.status, 303)
      const sessionCookie = completed?.headers
        .get('set-cookie')
        ?.split(',')
        .map((value) => value.trim().split(';')[0])
        .find((value) => value.startsWith('__Host-ruvyxa.session='))
      assert.ok(sessionCookie, 'the callback must issue a session cookie')
      const session = await auth.getSession(
        new Request(`${origin}/dashboard`, { headers: { cookie: sessionCookie } }),
      )
      assert.equal(session?.user.email, 'ada@example.com')
      assert.equal(session?.user.emailVerified, false)
    } finally {
      globalThis.fetch = originalFetch
    }
  })
})
