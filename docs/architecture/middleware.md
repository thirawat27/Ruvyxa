# Middleware Architecture

Ruvyxa has one middleware model with two implementation locations:

1. Built-in middleware is native Rust/Tower and remains the low-overhead path for CORS, rate
   limiting, timing, logging, compression, and security headers.
2. Plugins register request/response middleware through `definePlugin` in `ruvyxa.config.ts`. They
   execute in the same persistent Node/Bun runtime as build hooks.

## Configuration

```ts
import { config, definePlugin } from 'ruvyxa/config'

export default config({
  middleware: { builtin: { timing: true, log: true } },
  plugins: [
    definePlugin({
      name: 'auth',
      setup({ addMiddleware }) {
        addMiddleware({
          routes: ['/api/*'],
          onRequest(request, context) {
            if (request.headers.has('authorization')) return undefined
            return new Response('Unauthorized', { status: 401 })
          },
          onResponse(request, response) {
            const output = new Response(response.body, response)
            output.headers.set('x-plugin', context.plugin)
            return output
          },
        })
      },
    }),
  ],
})
```

`undefined` continues the chain. A returned `Request` replaces the request and a returned `Response`
short-circuits request processing. Response hooks receive cloned Fetch objects and may return a
replacement `Response`; `undefined` preserves the current response.

## Runtime boundary

The Rust dev server owns the HTTP socket, route matching, request body limits, and Axum response.
`PluginHost` starts `runtime/plugin-runtime.mjs` once per server and exchanges versioned NDJSON:

```text
Rust request -> { hook: "middlewareRequest", method, path, headers, bodyBase64 }
Node result  -> { kind: "request", request: ... } | { kind: "response", response: ... }
Rust response -> { hook: "middlewareResponse", request: ..., response: ... }
Node result  -> { response: ... }
```

Headers are represented as ordered pairs so duplicate values survive the bridge. Bodies are base64
encoded so binary uploads and responses are lossless. Rust validates every result before converting
it to Axum types and enforces `security.pluginLimit` while buffering response hooks.

## Ordering and failures

Built-in Tower layers are installed by `MiddlewareStack` in a deterministic order. Plugin middleware
runs in plugin registration order after the request enters the server and before the response leaves
it. Hook exceptions terminate the current call with a Ruvyxa diagnostic; they do not silently become
a malformed HTTP response. The runtime reports plugin names and hook phases so a failure is
actionable.

There is no custom-layer configuration or separate plugin ABI. This keeps framework middleware
native while making application middleware ordinary code with standard Fetch primitives.
