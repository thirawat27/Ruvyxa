import type { RuvyxaPlugin } from '@ruvyxa/core/plugin'

export interface AuthUser {
  id: string
  email?: string
  /**
   * Whether the provider that returned `email` vouched for it.
   *
   * OIDC Core §5.7 is explicit that `email` is not a verified identifier unless
   * `email_verified` is true, and an address nobody verified is an address
   * anybody can claim. Account identity in this package is keyed on `id`
   * (`google:${sub}`, `github:${id}`), so a session is safe without reading
   * this — but an application that *links* an OAuth login to an existing
   * account by address, or authorizes on the domain part, is deciding who
   * somebody is from this field and must check it.
   *
   * Absent means the provider said nothing, which is not the same as `false`
   * and is never a reason to treat the address as verified. Present without an
   * `email` is not produced: there is no claim to make about an address that
   * was not returned.
   */
  emailVerified?: boolean
  name?: string
  image?: string
  roles?: readonly string[]
  claims?: Readonly<Record<string, unknown>>
}

export interface AuthSession {
  id: string
  user: AuthUser
  createdAt: string
  expiresAt: string
  remember: boolean
}

/** Durable stores must implement atomic `take` to prevent token replay. */
export interface AuthStore {
  readonly name: string
  readonly durable: boolean
  get(key: string): Promise<string | null>
  set(key: string, value: string, ttlSeconds: number): Promise<void>
  delete(key: string): Promise<void>
  take(key: string): Promise<string | null>
}

export interface RateLimitDecision {
  allowed: boolean
  remaining: number
  retryAfterSeconds: number
}

/** Production implementations must increment and expire a key atomically. */
export interface AuthRateLimitStore {
  readonly name: string
  readonly durable: boolean
  consume(key: string, limit: number, windowSeconds: number): Promise<RateLimitDecision>
}

export interface CredentialsProvider {
  type: 'credentials'
  authorize(input: Record<string, unknown>, request: Request): Promise<AuthUser | null>
}

export interface OAuthTokenSet {
  accessToken: string
  tokenType?: string
  scope?: string
  expiresIn?: number
  refreshToken?: string
  raw: Readonly<Record<string, unknown>>
}

export interface OAuthProvider {
  type: 'oauth'
  id: string
  authorizationUrl: string
  tokenUrl: string
  userInfoUrl: string
  clientId: string
  clientSecret?: string
  scopes: readonly string[]
  authorizationParams?: Readonly<Record<string, string>>
  mapProfile(profile: unknown, tokens: OAuthTokenSet): AuthUser | Promise<AuthUser>
}

export interface MagicLinkProvider {
  type: 'magic-link'
  send(message: { email: string; url: string; expiresAt: Date }): Promise<void>
  resolveUser(email: string): Promise<AuthUser | null>
}

export interface WebAuthnProvider {
  type: 'webauthn'
  options(input: unknown, request: Request): Promise<unknown>
  verify(input: unknown, request: Request): Promise<AuthUser | null>
}

export type AuthProvider =
  CredentialsProvider | OAuthProvider | MagicLinkProvider | WebAuthnProvider

export interface AuthOptions {
  /** At least 32 characters; used as the HMAC key for opaque token indexes. */
  secret: string
  /** Canonical application origin, for example `https://app.example.com`. */
  origin: string
  store: AuthStore
  rateLimitStore: AuthRateLimitStore
  providers: Readonly<Record<string, AuthProvider>>
  basePath?: string
  session?: {
    ttlSeconds?: number
    rememberTtlSeconds?: number
    cookieName?: string
    secure?: boolean
    sameSite?: 'Lax' | 'Strict'
  }
  /**
   * The base ceiling every authentication bucket is derived from: `max`
   * requests per `windowSeconds` from one client against one scope.
   *
   * Two wider ceilings are derived from it and are not separately configurable.
   * One client may make `max × 5` requests across every scope in the same
   * window, which is what bounds a sweep across many accounts from one source.
   * One account may be attempted `max × 20` times — or mailed a magic link
   * `max × 1` times — by all clients combined, in a window of at most a minute,
   * which is what bounds a sweep of one account from many sources. A magic-link
   * send additionally costs a fifth of `max` per client, because sending mail is
   * not the same cost as checking a password.
   */
  rateLimit?: { max?: number; windowSeconds?: number }
  /**
   * Resolve the client IP for rate-limit bucketing. Off by default because
   * forwarded headers are attacker-controlled unless a trusted proxy sets
   * them; without an IP the limiter falls back to the user-agent, which a
   * client can rotate. Deployments behind a proxy or platform edge should
   * provide this — `forwardedClientIp` covers the common `x-forwarded-for`
   * case, or read a platform header directly, for example
   * `(request) => request.headers.get('cf-connecting-ip')`.
   * A thrown error or empty result falls back to the user-agent-only key.
   *
   * What survives the fallback and what does not: the account-wide ceilings
   * have no client in their key, so rotating the user-agent cannot lift them —
   * one address stays bounded either way. The per-client ceiling is what a
   * rotated user-agent escapes, so without a resolver an attacker can still
   * spread requests thinly across *many different* addresses. Configure this
   * for any deployment where a `magic-link` provider can be reached from the
   * internet.
   */
  clientIp?(request: Request): string | null | undefined
  /** Observe full server-side failures without exposing them in HTTP responses. */
  onError?(error: unknown, request: Request): void | Promise<void>
}

export interface AuthResult {
  user: AuthUser
  session: AuthSession
  headers: Headers
}

export interface AuthRuntime {
  readonly plugin: RuvyxaPlugin
  readonly basePath: string
  handle(request: Request): Promise<Response | undefined>
  login(provider: string, input: Record<string, unknown>, request?: Request): Promise<AuthResult>
  getSession(request: Request): Promise<AuthSession | null>
  logout(request: Request): Promise<Headers>
}
