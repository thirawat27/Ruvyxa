# Server Actions

## Creating Actions

Place mutations in an `action.ts` file next to the route that owns them. Parse and validate
untrusted values before performing the mutation:

```ts
// app/todos/action.ts
import { action } from 'ruvyxa/server'

export const createTodo = action
  .input({
    parse(value: unknown) {
      const title =
        typeof value === 'object' && value && 'title' in value ? String(value.title).trim() : ''

      if (!title) throw new Error('Title is required')
      return { title }
    },
  })
  .handler(async ({ input, invalidate }) => {
    const todo = await database.todos.create(input)
    invalidate('todos')
    return todo
  })
```

## HTML Form Integration

Actions support progressive enhancement — you can submit via a standard HTML form:

```tsx
<form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
  <label>
    Title
    <input name="title" required />
  </label>
  <button type="submit">Create</button>
</form>
```

The action endpoint pattern: `/__ruvyxa/action?path=<route>&name=<exportName>`

## Input Validation

`action.input({ parse })` must:

- Accept `unknown` input (never trust client-side types).
- Return a parsed value (typed as the handler expects).
- Throw an error when invalid (returned to the client as an error response).

```ts
export const updateProfile = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== 'object') throw new Error('Expected object')
      const obj = value as Record<string, unknown>

      const name = String(obj.name ?? '').trim()
      if (!name || name.length > 100) throw new Error('name: 1–100 chars required')

      const email = String(obj.email ?? '').trim()
      if (!email.includes('@')) throw new Error('email: must be valid')

      return { name, email }
    },
  })
  .handler(async ({ input }) => {
    return database.users.update(input)
  })
```

## Supported Content Types

| Content-Type                        | Format           |
| ----------------------------------- | ---------------- |
| `application/json`                  | JSON body        |
| `application/x-www-form-urlencoded` | URL-encoded form |

## Security

Actions are protected by default:

| Protection           | Default                             | Config Key                 |
| -------------------- | ----------------------------------- | -------------------------- |
| Body size limit      | 1 MiB                               | `security.actionLimit`     |
| Same-origin check    | Enabled                             | `security.sameOrigin`      |
| Fetch Metadata guard | Enabled                             | `security.fetchMeta`       |
| Rate limiting        | 600 requests / client-action / 60 s | `security.actionRateLimit` |

### Configuring Security

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'

export default config({
  security: {
    actionLimit: 2 * 1024 * 1024,
    actionRateLimit: { max: 300, window: 60 },
    sameOrigin: true,
    fetchMeta: true,
  },
})
```

### Behind a Reverse Proxy

Loopback proxies are trusted by default. For a proxy on another host, explicitly allowlist its exact
IP before Ruvyxa accepts `X-Forwarded-For`, `X-Real-IP`, or `X-Forwarded-Proto`. This keeps a client
on a private network from forging those headers to bypass rate limits or origin checks.

```ts
export default config({
  security: {
    trustedProxyIps: ['10.0.0.2'],
  },
})
```

The proxy must overwrite forwarded headers from the incoming request rather than pass client-sent
values through.

## Response-Phase Wasm Plugin Limits

Wasm plugins running in the response phase must buffer the complete response:

| Config        | Default | Max     |
| ------------- | ------- | ------- |
| `pluginLimit` | 32 MiB  | 256 MiB |

```ts
export default config({
  security: {
    pluginLimit: 64 * 1024 * 1024,
  },
})
```

Choose the smallest limit that covers the response types handled by the plugin. Use a streaming
route or bypass response-phase plugins for file downloads and payloads above 256 MiB.

## Cache Invalidation from Actions

```ts
.handler(async ({ input, invalidate }) => {
  await database.todos.create(input)
  invalidate('todos')        // invalidate a specific key
  invalidate()               // invalidate all
  return { ok: true }
})
```

See [Data Loading & Cache](data-loading-and-cache.md) for cache details.
