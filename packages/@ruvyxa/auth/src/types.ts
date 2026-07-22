import type { RuvyxaPlugin } from '@ruvyxa/core/config'

export interface AuthUser {
  id: string
  email?: string
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
  rateLimit?: { max?: number; windowSeconds?: number }
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
