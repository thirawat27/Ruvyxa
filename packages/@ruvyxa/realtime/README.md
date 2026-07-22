# @ruvyxa/realtime

Action-driven realtime updates using Ruvyxa's native Axum WebSocket transport. No Socket.IO or
application-owned WebSocket server is required.

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { realtime } from '@ruvyxa/realtime'

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

The native transport is production-ready for self-hosted Node/Bun (`ruvyxa start`) where one Rust
process owns WebSocket lifecycle. Static, Edge, Vercel, Netlify, and Cloudflare builds fail with
`RUV3201` instead of silently deploying a non-functional socket. Horizontal multi-instance fan-out
requires a future external broker adapter and is not claimed by this release.
