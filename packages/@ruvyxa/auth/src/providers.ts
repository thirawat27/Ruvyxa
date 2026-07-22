import type { AuthUser, OAuthProvider, OAuthTokenSet } from './types.js'

export interface BuiltinOAuthOptions {
  clientId: string
  clientSecret: string
  scopes?: readonly string[]
}

/** Google OAuth 2.0 provider with PKCE and an explicit profile mapping. */
export function google(options: BuiltinOAuthOptions): OAuthProvider {
  validateOAuthSecrets('google', options)
  return {
    type: 'oauth',
    id: 'google',
    authorizationUrl: 'https://accounts.google.com/o/oauth2/v2/auth',
    tokenUrl: 'https://oauth2.googleapis.com/token',
    userInfoUrl: 'https://openidconnect.googleapis.com/v1/userinfo',
    clientId: options.clientId,
    clientSecret: options.clientSecret,
    scopes: options.scopes ?? ['openid', 'email', 'profile'],
    mapProfile(profile) {
      const value = record(profile)
      return userFromProfile('google', value.sub, value.email, value.name, value.picture)
    },
  }
}

/** GitHub OAuth provider with PKCE and a conservative public-profile mapping. */
export function github(options: BuiltinOAuthOptions): OAuthProvider {
  validateOAuthSecrets('github', options)
  return {
    type: 'oauth',
    id: 'github',
    authorizationUrl: 'https://github.com/login/oauth/authorize',
    tokenUrl: 'https://github.com/login/oauth/access_token',
    userInfoUrl: 'https://api.github.com/user',
    clientId: options.clientId,
    clientSecret: options.clientSecret,
    scopes: options.scopes ?? ['read:user', 'user:email'],
    mapProfile(profile, _tokens: OAuthTokenSet): AuthUser {
      const value = record(profile)
      return userFromProfile(
        'github',
        value.id,
        value.email,
        value.name ?? value.login,
        value.avatar_url,
      )
    },
  }
}

function validateOAuthSecrets(name: string, options: BuiltinOAuthOptions): void {
  if (!options?.clientId?.trim() || !options.clientSecret?.trim()) {
    throw new TypeError(`${name}() requires non-empty clientId and clientSecret`)
  }
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError('OAuth provider returned an invalid profile')
  }
  return value as Record<string, unknown>
}

function userFromProfile(
  provider: string,
  idValue: unknown,
  emailValue: unknown,
  nameValue: unknown,
  imageValue: unknown,
): AuthUser {
  if ((typeof idValue !== 'string' && typeof idValue !== 'number') || String(idValue) === '') {
    throw new TypeError(`${provider} profile is missing an id`)
  }
  return {
    id: `${provider}:${String(idValue)}`,
    ...(typeof emailValue === 'string' ? { email: emailValue } : {}),
    ...(typeof nameValue === 'string' ? { name: nameValue } : {}),
    ...(typeof imageValue === 'string' ? { image: imageValue } : {}),
  }
}
