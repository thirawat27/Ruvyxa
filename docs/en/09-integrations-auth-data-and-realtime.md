# Integrations: authentication, data, realtime, adapters, and testing

> **Tutorial goal:** connect an application concern to the framework without assuming unsupported
> infrastructure. **Start from:** route policy in
> [Plugins and middleware](08-plugins-middleware.md). **Checkpoint:** choose one integration,
> implement its smallest flow, and test the failure path too.

## Authentication

`@ruvyxa/auth` exports `createAuth`, provider helpers `google` and `github`, the development-only
memory stores, the Redis stores `redisAuthStore` and `redisRateLimitStore` with their
`nodeRedisCommandPort` and `ioredisCommandPort` client adapters, types, and `AuthError`. Its package
exports `@ruvyxa/auth/client` and `@ruvyxa/auth/plugin` separately. Supported provider contracts
include credentials, OAuth, magic link, and WebAuthn. The memory stores are process-local and
refused by production builds; the Redis stores are the durable, shared implementation.

```ts
import { createAuth, memoryAuthStore, memoryRateLimitStore } from '@ruvyxa/auth'

const auth = createAuth({
  secret: process.env.RUVYXA_AUTH_SECRET!,
  origin: 'https://example.test',
  store: memoryAuthStore({ development: true }),
  rateLimitStore: memoryRateLimitStore({ development: true }),
  providers: {},
})
```

The exact `AuthOptions` contract is exported by the package; do not pass this example's placeholder
as a real secret. Register the plugin returned by the auth runtime, then use the separate browser
entry point only in client code:

```ts
// ruvyxa.config.ts
export default config({ plugins: [auth.plugin] })

// a client module
import { createAuthClient } from '@ruvyxa/auth/client'
const authClient = createAuthClient()
```

The default auth path is `/__ruvyxa/auth`. The client exposes `login`, `logout`, `session`, and
`oauth`; `createAuth` also exposes `handle`, `login`, `getSession`, and `logout` for server-side
integration. The memory stores require `{ development: true }` and deliberately fail the production
build with `RUV3105`. Pass `redisAuthStore(port)` and `redisRateLimitStore(port)` instead, where
`port` is `nodeRedisCommandPort(client)` for node-redis or `ioredisCommandPort(client)` for ioredis:
`take` and `consume` each run as one Lua script, so two instances behind a load balancer cannot both
accept one magic link or both pass one rate-limit slot. Any other `AuthStore` and
`AuthRateLimitStore` whose `take` and `consume` are atomic satisfies the same contracts.
`createAuthPlugin(bridge)` is available when a custom bridge is needed.

```ts
import { createClient } from 'redis'
import { createAuth, nodeRedisCommandPort, redisAuthStore, redisRateLimitStore } from '@ruvyxa/auth'

const redis = nodeRedisCommandPort(await createClient({ url: process.env.REDIS_URL }).connect())

export const auth = createAuth({
  secret: process.env.AUTH_SECRET!,
  origin: 'https://app.example.com',
  store: redisAuthStore(redis),
  rateLimitStore: redisRateLimitStore(redis),
  providers: {},
})
```

## Database

`@ruvyxa/database` is a typed normalized-operation layer, not an ORM migration system.
`createDatabase<TSchema>(adapter)` creates model delegates for `findMany`, `findFirst`,
`findUnique`, `create`, `createMany`, `update`, `updateMany`, `delete`, `deleteMany`, and `count`.
It supplies `prismaAdapter`, `dynamoAdapter`, and `defineDatabaseAdapter`; adapter errors use
`RUV3001`–`RUV3003`.

```ts
import { createDatabase, defineDatabaseAdapter } from '@ruvyxa/database'
const adapter = defineDatabaseAdapter({
  name: 'example',
  execute: async (operation) => {
    throw new Error(`implement ${operation.kind}`)
  },
})
const db = createDatabase<{ todo: { id: string; title: string } }>(adapter)
```

The framework does not ship a database server, migration engine, or backup service. Those remain
application/infrastructure responsibilities.

## Realtime and adapters

> **Decide hosting before you build on this.** Both realtime plugins need a process that stays alive
> to own the WebSocket, so they are served by `ruvyxa dev`, `ruvyxa start`, and `ruvyxa preview` —
> and by no build artifact at all: not a serverless function, and not the standalone server the
> node, bun, deno, railway, and render adapters emit, which speaks plain HTTP with no upgrade path.
> `ruvyxa dev` prints a line naming the capability and its path, `ruvyxa build` reports `RUV2205`
> naming the endpoint every adapter build will lack, and `ruvyxa test:parity` reports the gap — but
> replacing the transport afterwards is an application rewrite, not a configuration change.

`@ruvyxa/realtime/plugin` exports `realtime()`, which claims the native `realtime@1` capability and
decides nothing about deployment: whether a target can serve the socket is the business of the host
that serves it, so the plugin refuses no build. A deployment that depends on the socket runs
`ruvyxa start` as its process. `@ruvyxa/realtime/client` exports `createRealtimeClient`; it caps
active channels at 16 and reconnects with bounded exponential backoff.

## Real-time collaboration

`@ruvyxa/realtime/plugin` also exports `collab()`, which claims the native `presence@1` capability
and serves bidirectional collaboration rooms at `/__ruvyxa/collab`. It carries the same deployment
shape as `realtime()`: served by the Axum host, reported as `RUV2205` by every adapter build.

```ts
import { config } from 'ruvyxa/config'
import { collab } from '@ruvyxa/realtime'

export default config({ plugins: [collab()] })
```

A room carries two kinds of state, and they behave differently on purpose:

| State        | Retained            | Semantics                                         |
| ------------ | ------------------- | ------------------------------------------------- |
| Presence     | For the connection  | Replaced wholesale; dropped when the peer leaves  |
| Shared state | For the room's life | Last-writer-wins per key, sequenced by the server |

The server is the only sequencer, so "last writer" means "last frame to reach the process" — no
client clock is involved, and two peers writing one key converge on the same winner. **Shared state
is not a CRDT.** Concurrent writes to one key do not merge; the later write replaces the earlier
one. Split a document across many keys when concurrent edits must all survive.

`@ruvyxa/realtime/react` exports `CollabProvider`, `usePresence`, `useSharedState`, `useCollabRoom`,
and `useCollabClient`. One provider owns one socket; hooks read it through `useSyncExternalStore`.

```tsx
import { CollabProvider, usePresence, useSharedState } from '@ruvyxa/realtime/react'

function Editor() {
  const others = usePresence({ cursor: [x, y], name: 'Ada' })
  const [title, setTitle] = useSharedState('title', 'Untitled')
  return (
    <>
      <input value={title} onChange={(event) => setTitle(event.target.value)} />
      {others.map((peer) => (
        <Cursor key={peer.id} state={peer.state} />
      ))}
    </>
  )
}

export default function Page() {
  return (
    <CollabProvider room="doc:1">
      <Editor />
    </CollabProvider>
  )
}
```

`@ruvyxa/realtime/collab` exports `createCollabClient` for use without React.

Rooms are process-local and ephemeral: they hold no storage, and a room is discarded once its last
peer leaves. Two server processes behind a load balancer own two unrelated copies of every room, so
a collaborative deployment must pin one room's peers to one process. Persist anything that must
survive the last peer through a loader or Server Action.

Server-enforced limits: 64 peers and 256 shared-state keys per room, 1024 rooms per process, 32 keys
per write, 32 KiB per frame, and 120 frames per second per connection. A connection that exceeds the
frame budget is closed; a peer that falls behind the room's broadcast buffer receives a `resync` and
reconnects for a fresh snapshot.

First-party adapter packages exist for Node, Bun, Deno, static, Vercel, Netlify, Cloudflare,
Railway, Render, Firebase, and AWS. Build selection is `npm run build -- --adapter <name>` or config
`adapter`; see [Deploy, run, and operate](15-deploy-run-and-operate.md). `@ruvyxa/testing` exports
`mockLoader`, `mockAction`, and `mockCache` for unit tests.

**Previous:** [Plugins and middleware](08-plugins-middleware.md) · **Next:**
[CLI reference](10-cli.md)
