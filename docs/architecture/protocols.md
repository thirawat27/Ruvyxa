# Wire Protocols

Communication contracts between Ruvyxa components.

---

## 1. Node Worker NDJSON Protocol

### Transport

- **Medium**: stdin/stdout pipes
- **Format**: newline-delimited JSON (one JSON object per line)
- **Encoding**: UTF-8
- **Termination**: Node process reads stdin line-by-line, writes stdout line-by-line

### Request messages (`WorkerRequest`)

All messages tagged with `"type"` field. Common field: `"id"` (UUID v4 for correlation).

#### SSR Render

```json
{
  "type": "ssr",
  "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "projectRoot": "/Users/project",
  "appDir": "/Users/project/app",
  "pageFile": "/Users/project/app/blog/[slug]/page.tsx",
  "requestPath": "/blog/hello-world",
  "params": { "slug": "hello-world" }
}
```

#### API Route

```json
{
  "type": "api",
  "id": "a1b2c3d4-...",
  "projectRoot": "/Users/project",
  "routeFile": "/Users/project/app/api/search/route.ts",
  "method": "GET",
  "requestPath": "/api/search?q=hello",
  "headers": { "accept": "application/json" },
  "headerPairs": [
    ["accept", "application/json"],
    ["cookie", "sess=abc"]
  ],
  "body": null,
  "bodyBase64": null,
  "streamResponse": true,
  "params": {}
}
```

Headers available in two forms:

- `headers: HashMap<String, String>` — last-value-wins for simple lookups
- `headerPairs: Vec<(String, String)>` — preserves all values and order

#### Action

```json
{
  "type": "action",
  "id": "b2c3d4e5-...",
  "projectRoot": "/Users/project",
  "actionFile": "/Users/project/app/action.ts",
  "actionName": "createTodo",
  "payloadJson": "{\"title\":\"Buy milk\"}",
  "contentType": "application/json",
  "requestPath": "/todos"
}
```

#### Client Bundle

```json
{
  "type": "client",
  "id": "c3d4e5f6-...",
  "projectRoot": "/Users/project",
  "appDir": "/Users/project/app",
  "pageFile": "/Users/project/app/page.tsx",
  "requestPath": "/",
  "params": {}
}
```

#### SSG / PPR Render

```json
{
  "type": "ssg",
  "id": "d4e5f6a7-...",
  "projectRoot": "/Users/project",
  "appDir": "/Users/project/app",
  "pageFile": "/Users/project/app/blog/[slug]/page.tsx",
  "requestPath": "/blog/hello-world",
  "params": { "slug": "hello-world" },
  "mode": "ppr",
  "fresh": false
}
```

- `mode`: `"full"` (complete render including dynamic) | `"ppr"` (static shell only)
- `fresh`: `true` = skip stale-while-revalidate, render fresh

#### Static Params Resolution

```json
{
  "type": "staticParams",
  "id": "e5f6a7b8-...",
  "projectRoot": "/Users/project",
  "pageFile": "/Users/project/app/blog/[slug]/page.tsx",
  "routePath": "/blog/[slug]",
  "segments": ["slug"],
  "routes": [
    { "id": "...", "path": "/posts/[id]", "file": "...", ... }
  ]
}
```

#### Invalidation

```json
{
  "type": "invalidate",
  "id": "f6a7b8c9-...",
  "paths": ["/Users/project/app/components/Button.tsx", "/Users/project/app/page.tsx"]
}
```

#### Ping

```json
{ "type": "ping", "id": "a7b8c9d0-..." }
```

#### Warmup

```json
{
  "type": "warmup",
  "id": "b8c9d0e1-...",
  "projectRoot": "/Users/project",
  "routes": [{ "pageFile": "...", "requestPath": "/", "params": {} }]
}
```

### Response messages (`WorkerResponse`)

#### Successful SSR response

```json
{
  "id": "f47ac10b-...",
  "ok": true,
  "html": "<!doctype html><html lang=\"en\"><head>...</head><body>...</body></html>"
}
```

#### Successful API response (non-streaming)

```json
{
  "id": "a1b2c3d4-...",
  "ok": true,
  "status": 200,
  "headers": { "content-type": "application/json" },
  "headerPairs": [["content-type", "application/json"]],
  "body": "{\"results\":[1,2,3]}"
}
```

#### Successful API response (streaming)

```
{"id":"a1b2c3d4-...","ok":true,"frame":"api-start","status":200,"headers":{"content-type":"text/plain"},"headerPairs":[["content-type","text/plain"]]}
{"id":"a1b2c3d4-...","ok":true,"frame":"api-chunk","bodyBase64":"SGVsbG8="}
{"id":"a1b2c3d4-...","ok":true,"frame":"api-chunk","bodyBase64":"IHdvcmxk"}
{"id":"a1b2c3d4-...","ok":true,"frame":"api-end"}
```

Frames:

- `"api-start"` — stream begins, includes status + headers
- `"api-chunk"` — body chunk, `bodyBase64` encoded
- `"api-end"` — stream complete, terminal

#### Successful action response

```json
{
  "id": "b2c3d4e5-...",
  "ok": true,
  "status": 200,
  "headers": { "content-type": "application/json" },
  "body": "{\"ok\":true,\"id\":42}"
}
```

#### Successful client bundle response

```json
{
  "id": "c3d4e5f6-...",
  "ok": true,
  "script": "var __ruvyxa_shared_modules__=(globalThis.__RUVYXA_SHARED_MODULES__||(globalThis.__RUVYXA_SHARED_MODULES__={}));..."
}
```

#### Successful ping

```json
{ "id": "...", "ok": true, "pong": true }
```

#### Successful warmup

```json
{ "id": "...", "ok": true, "warmed": 42, "moduleCacheSize": 128 }
```

#### Static params response

```json
{
  "id": "...",
  "ok": true,
  "params": [{ "slug": "hello-world" }, { "slug": "about" }]
}
```

#### Error response

```json
{
  "id": "...",
  "ok": false,
  "code": "RUV1100",
  "message": "React SSR failed: Cannot read properties of undefined",
  "stack": "TypeError: Cannot read properties of undefined\n    at Page (file:///...)\n    at renderToString (node:...)"
}
```

Streaming errors use `frame`:

```json
{
  "id": "...",
  "ok": false,
  "frame": "api-error",
  "message": "Database connection failed",
  "code": "DB_CONN"
}
```

---

## 2. HMR WebSocket Protocol

### Transport

- **Medium**: WebSocket (`ws://` or `wss://`)
- **Direction**: Server → Browser (unidirectional)
- **Format**: JSON text frames

### Message format

```json
{
  "type": "css-update" | "component-update" | "full-reload",
  "paths": ["/abs/path/to/changed/file.scss", "..."],
  "affectedRoutes": ["/", "/blog/[slug]"],
  "fullReload": false
}
```

### Event types

| Type                 | Trigger                                       | Browser action                                     |
| -------------------- | --------------------------------------------- | -------------------------------------------------- |
| `"css-update"`       | Only `.css`/`.scss`/`.sass` files changed     | Replace `<style data-ruvyxa-css>` with updated CSS |
| `"component-update"` | Known component file(s) changed               | React Fast Refresh (re-render changed components)  |
| `"full-reload"`      | Layout changed, or unknown/destructive change | `window.location.reload()`                         |

### Connection lifecycle

```
Browser: WebSocket connect to ws://host/__ruvyxa/hmr
Server:  subscribes to reload_tx broadcast channel
         sends JSON on each file change event
Browser: receives message, dispatches to appropriate handler
         auto-reconnects on disconnect (exponential backoff)
```

### Client-side handler (injected in HTML `<script>`)

```javascript
;(function () {
  const protocol = location.protocol === 'https:' ? 'wss' : 'ws'
  const socket = new WebSocket(`${protocol}://${location.host}/__ruvyxa/hmr`)

  socket.addEventListener('message', async (event) => {
    const msg = JSON.parse(event.data)

    if (msg.type === 'css-update') {
      const style = document.querySelector('style[data-ruvyxa-css]')
      if (style) {
        const resp = await fetch(location.href)
        const html = await resp.text()
        const match = html.match(/<style data-ruvyxa-css>([\s\S]*?)<\/style>/)
        if (match) style.textContent = match[1]
      }
    } else if (msg.type === 'component-update') {
      // React Fast Refresh implementation
      if (window.__RUVYXA_FAST_REFRESH__) {
        window.__RUVYXA_FAST_REFRESH__(msg.paths)
      } else {
        location.reload()
      }
    } else {
      location.reload()
    }
  })

  socket.addEventListener('close', () => {
    // Reconnect with backoff
    setTimeout(() => connectHMR(), 1000)
  })
})()
```

---

## ## 3. Plugin Protocol

Plugins communicate with Rust through the persistent `runtime/plugin-runtime.mjs` process using
newline-delimited JSON (NDJSON). The same registry serves build hooks and HTTP middleware.

### Build hooks

```json
{ "hook": "transform", "code": "...", "id": "/app/page.tsx", "environment": "client" }
```

The runtime returns `{ "ok": true, "result": { "code": "...", "map": "..." } }`, or a structured
error containing a Ruvyxa diagnostic code, message, and stack.

### HTTP middleware

Request calls use ordered header pairs and optional base64 bodies:

```json
{
  "hook": "middlewareRequest",
  "request": {
    "method": "POST",
    "path": "/api/items?draft=1",
    "headers": [["content-type", "application/octet-stream"]],
    "bodyBase64": "AAE="
  }
}
```

A request hook returns either a replacement request or a response short-circuit. Response hooks
receive the request and current response and return a replacement response. Rust validates methods,
paths, headers, status codes, and body limits before converting values to Axum types.

### Lifecycle

1. The CLI or dev server starts the runtime and sends `describe`.
2. The runtime loads `ruvyxa.config.ts`, validates plugin names, and executes each `setup` once.
3. Rust sends serialized hook calls over the persistent process.
4. The runtime returns one JSON response per input line; diagnostics go to stderr.
5. The process is terminated with the owning build or server lifecycle.
