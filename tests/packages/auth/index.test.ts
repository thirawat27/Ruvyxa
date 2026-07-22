import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  createAuth,
  memoryAuthStore,
  memoryRateLimitStore,
  type AuthUser,
} from '../../../packages/@ruvyxa/auth/dist/index.js'

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
    const auth = runtime({
      providers: {
        magic: {
          type: 'magic-link',
          async send(message: { url: string }) {
            sentUrl = message.url
          },
          async resolveUser(email: string) {
            return email === user.email ? user : null
          },
        },
      },
    })
    const start = await auth.handle(
      new Request(`${origin}/__ruvyxa/auth/magic-link`, {
        method: 'POST',
        headers: { origin },
        body: JSON.stringify({ email: user.email }),
      }),
    )
    assert.equal(start?.status, 200)
    const first = await auth.handle(new Request(sentUrl))
    const replay = await auth.handle(new Request(sentUrl))
    assert.equal(first?.status, 303)
    assert.equal(replay?.status, 400)
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
    await auth.plugin.setup({
      addMiddleware() {},
      resolveId() {},
      transform() {},
      enableRealtime() {},
      onBuildComplete(value) {
        hook = value as typeof hook
      },
    })
    await assert.rejects(
      async () => hook?.({ manifest: { profile: 'production' } }),
      /RUV3105|production auth requires durable/,
    )
    assert.throws(() => memoryAuthStore({} as never), /development: true/)
  })
})
