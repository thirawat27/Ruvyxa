<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/auth</h1>

<p align="center">
  Production-shaped authentication for Ruvyxa with explicit state and provider contracts. It<br/>
  supports credentials, OAuth 2.0 with PKCE, magic links, and delegated WebAuthn verification<br/>
  without storing passwords or pinning an ORM/Redis vendor.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/auth"><img src="https://img.shields.io/npm/v/@ruvyxa/auth?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/auth"><img src="https://img.shields.io/node/v/@ruvyxa/auth?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```ts
import { createClient } from 'redis'
import {
  createAuth,
  google,
  nodeRedisCommandPort,
  redisAuthStore,
  redisRateLimitStore,
} from '@ruvyxa/auth'

const redis = nodeRedisCommandPort(await createClient({ url: process.env.REDIS_URL }).connect())

export const auth = createAuth({
  secret: process.env.AUTH_SECRET!,
  origin: 'https://app.example.com',
  store: redisAuthStore(redis),
  rateLimitStore: redisRateLimitStore(redis),
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

Register `auth.plugin` in `ruvyxa.config.ts`. The same plugin serves the endpoints under
`/__ruvyxa/auth` on every host — `ruvyxa dev`/`start`, and every deployed build through the
adapters' request handler — so nothing changes between a self-hosted process and a serverless
function. `auth.handle(request)` is also exported for an application that wants to mount the
endpoints itself.

```ts
import { config } from 'ruvyxa/config'
import { auth } from './server/auth.js'

export default config({ plugins: [auth.plugin] })
```

The package exposes `@ruvyxa/auth/plugin` for integration authors who need `createAuthPlugin()` with
an explicit request/build bridge. Normal applications should use the `auth.plugin` value created by
`createAuth()` so the handler, store validation, and plugin stay aligned.

**Stores.** `AuthStore.take()` and `AuthRateLimitStore.consume()` must be atomic: a read-then-write
in the application process lets two concurrent requests both claim one single-use token, or both
pass one rate-limit slot. `redisAuthStore(port)` and `redisRateLimitStore(port)` run each as a
single Lua script on the server, so several instances behind one load balancer share one truth. The
package pins no Redis client: build the `port` with `nodeRedisCommandPort(client)` for node-redis or
`ioredisCommandPort(client)` for ioredis, or hand in any object with `get`, `set`, `del`, and
`eval`. The included memory stores are for tests and development, require `{ development: true }`,
and are refused by production builds with `RUV3105`.

The session cookie is opaque, HttpOnly, SameSite, and Secure on HTTPS. Session and one-time token
keys are HMAC-derived. OAuth state is additionally bound to an HttpOnly browser cookie, protocol
parameters cannot be overridden, and non-local provider endpoints must use HTTPS.

Account identity is `session.user.id` — `google:${sub}`, `github:${id}` — and never
`session.user.email`. An address is a claim the identity provider may or may not stand behind, so
`session.user.emailVerified` records which it was: `true` from Google only when the profile carried
`email_verified: true`, and `true` from GitHub because its `/user` email is selected from the
addresses GitHub confirmed. Absent means the provider said nothing. Do not link an OAuth login to an
existing account, grant a role from an address domain, or authorize anything on `user.email` unless
`user.emailVerified` is `true` — an unverified address is an address the person signing in chose.

Set `onError(error, request)` to send full server-side failures to application observability. Public
500 responses remain generic even if that hook fails.

WebAuthn challenge generation and signature/attestation verification are deliberately delegated to a
standards-compliant adapter because correct verification depends on RP ID, origin, authenticator
policy, and credential persistence. Successful verification enters the same session pipeline.
