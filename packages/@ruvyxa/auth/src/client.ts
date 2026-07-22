import type { AuthSession } from './types.js'

export type { AuthSession, AuthUser } from './types.js'

export interface AuthClientOptions {
  basePath?: string
  fetch?: typeof globalThis.fetch
}

/** Browser helper for the built-in auth endpoints. */
export function createAuthClient(options: AuthClientOptions = {}) {
  const basePath = normalizeBasePath(options.basePath ?? '/__ruvyxa/auth')
  const request = options.fetch ?? globalThis.fetch
  return Object.freeze({
    async login(provider: string, input: Record<string, unknown>): Promise<AuthSession> {
      return authRequest<AuthSession>(
        request,
        `${basePath}/login/${encodeURIComponent(provider)}`,
        {
          method: 'POST',
          body: JSON.stringify(input),
        },
      )
    },
    async logout(): Promise<void> {
      await authRequest(request, `${basePath}/logout`, { method: 'POST' })
    },
    async session(): Promise<AuthSession | null> {
      return authRequest<AuthSession | null>(request, `${basePath}/session`)
    },
    oauth(provider: string, returnTo = '/'): void {
      const url = new URL(
        `${basePath}/oauth/${encodeURIComponent(provider)}/start`,
        location.origin,
      )
      url.searchParams.set('returnTo', returnTo)
      location.assign(url)
    },
  })
}

async function authRequest<TResult>(
  request: typeof globalThis.fetch,
  url: string,
  init: RequestInit = {},
): Promise<TResult> {
  const response = await request(url, {
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json', ...init.headers },
    ...init,
  })
  const payload = (await response.json()) as { data?: TResult; error?: string }
  if (!response.ok)
    throw new Error(payload.error ?? `Authentication request failed (${response.status})`)
  return payload.data as TResult
}

function normalizeBasePath(value: string): string {
  if (!value.startsWith('/') || value.endsWith('/')) {
    throw new TypeError('Auth client basePath must start with "/" and must not end with "/"')
  }
  return value
}
