# @ruvyxa/auth

Production-shaped authentication for Ruvyxa with explicit state and provider contracts. It supports
credentials, OAuth 2.0 with PKCE, magic links, and delegated WebAuthn verification without storing
passwords or pinning an ORM/Redis vendor.

```ts
import { createAuth, google } from '@ruvyxa/auth'

export const auth = createAuth({
  secret: process.env.AUTH_SECRET!,
  origin: 'https://app.example.com',
  store: redisAuthStore,
  rateLimitStore: redisRateLimitStore,
  providers: {
    email: {
      type: 'credentials',
      authorize: ({ email, password }, request) => verifyUser(email, password),
    },
    google: google({
      clientId: process.env.GOOGLE_CLIENT_ID!,
      clientSecret: process.env.GOOGLE_CLIENT_SECRET!,
    }),
  },
})
```

Register `auth.plugin` in `ruvyxa.config.ts` for the self-hosted Node/Bun middleware path. On
serverless/edge hosts, expose `auth.handle(request)` from an API route so authentication runs in the
same platform request lifecycle. Both paths use the same endpoints under `/__ruvyxa/auth`.

The session cookie is opaque, HttpOnly, SameSite, and Secure on HTTPS. Session and one-time token
keys are HMAC-derived. `AuthStore.take()` and `AuthRateLimitStore.consume()` must be atomic; the
included memory implementations require `{ development: true }` and production builds reject them.
OAuth state is additionally bound to an HttpOnly browser cookie, protocol parameters cannot be
overridden, and non-local provider endpoints must use HTTPS.

Set `onError(error, request)` to send full server-side failures to application observability. Public
500 responses remain generic even if that hook fails.

WebAuthn challenge generation and signature/attestation verification are deliberately delegated to a
standards-compliant adapter because correct verification depends on RP ID, origin, authenticator
policy, and credential persistence. Successful verification enters the same session pipeline.
