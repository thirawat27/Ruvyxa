<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/realtime</h1>

<p align="center">
  Action-driven realtime updates using Ruvyxa's native Axum WebSocket transport. No Socket.IO or<br/>
  application-owned WebSocket server is required.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/realtime"><img src="https://img.shields.io/npm/v/@ruvyxa/realtime?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/realtime"><img src="https://img.shields.io/node/v/@ruvyxa/realtime?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { realtime } from '@ruvyxa/realtime/plugin'

export default config({ plugins: [realtime()] })
```

```ts
// app/todos/action.ts
import { action } from 'ruvyxa/server'

export const updateTodo = action
  .realtime('todos')
  .handler(async ({ input }) => db.todos.update(input))
```

```ts
// browser code
import { createRealtimeClient } from '@ruvyxa/realtime/client'

const realtime = createRealtimeClient()
const unsubscribe = realtime.subscribe('todos', () => refetchTodos())
```

Calling `.realtime()` without channels publishes to `route:<pathname>`. Events contain action name,
route, channel names, and cache invalidation keys—not action results, credentials, or database rows.
Long route names use the same deterministic `route-hash:<id>` mapping in the worker and browser.
Clients reconnect with bounded exponential backoff and receive a `resync` event if their server-side
broadcast queue lagged, allowing the application to refetch authoritative state.

The native transport is production-ready for long-lived Node/Bun processes, including Railway and
Render, where one process owns WebSocket lifecycle. Static, Edge, Vercel, Netlify, Cloudflare,
Firebase, and AWS Amplify builds fail with `RUV3201` instead of silently deploying a non-functional
socket. Horizontal multi-instance fan-out requires a future external broker adapter and is not
claimed by this release.

`realtime()` exclusively claims the framework-owned `realtime@1` native socket and validates the
deployment during build completion. The main package also re-exports the factory; `./plugin` makes
the lifecycle entry explicit.
