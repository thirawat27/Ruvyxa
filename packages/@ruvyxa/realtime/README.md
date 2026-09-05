<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/realtime</h1>

<p align="center">
  Action-driven realtime updates using Ruvyxa's native Axum WebSocket transport. No Socket.IO or<br/>
  application-owned WebSocket server is required.<br/><br/>
  <strong>Served by <code>ruvyxa start</code>, and by no build artifact.</strong> The socket lives in the<br/>
  Axum host — <code>ruvyxa dev</code>, <code>ruvyxa start</code>, <code>ruvyxa preview</code>. Not a serverless<br/>
  function and not the standalone server the node, bun, deno, railway, and render adapters emit can hold<br/>
  one, so <code>ruvyxa build</code> reports <code>RUV2205</code> naming the endpoint every adapter build will lack.
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

**Where it runs.** The transport is served by the Axum host and nothing else. A deployment that
depends on it runs `ruvyxa start` as its process — on Railway, Render, a VM, a container. Every
adapter, including node, bun, deno, railway, and render, emits a build artifact that speaks plain
HTTP with no upgrade path, and `ruvyxa build` says so once with `RUV2205`, naming the plugin, the
capability, and the path that will answer 404 in that deployment. The plugin itself decides nothing
about deployment: it claims the capability, and the host that serves the socket owns the rule about
which targets can. Horizontal multi-instance fan-out requires a future external broker adapter and
is not claimed by this release.

`realtime()` exclusively claims the framework-owned `realtime@1` native socket; `collab()` claims
`presence@1` and serves collaboration rooms at `/__ruvyxa/collab` under the same rule. The main
package also re-exports both factories; `./plugin` makes the lifecycle entry explicit.
