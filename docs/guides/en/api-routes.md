# API Routes

> 🟡 **Intermediate** · ⏱️ ~6 min read
>
> **You'll learn:** create JSON endpoints with `route.ts`, handle each HTTP method, and validate
> incoming request bodies safely.

An API route is a backend endpoint without a page — a URL that returns JSON (or anything else)
instead of HTML. Use one when the browser, a mobile app, or another service needs to call your
server: form submissions from external sites, webhooks, health checks, a public API. If you only
need to mutate data from your own pages, [Server Actions](server-actions.md) are usually simpler.

## Creating API Routes

Create `route.ts` and export named HTTP method handlers. Handlers receive a standard `Request` and
return a standard `Response`:

```ts
// app/api/health/route.ts
export function GET() {
  return Response.json({ ok: true })
}

export async function POST({ request }: { request: Request }) {
  const body = await request.json()
  return Response.json({ received: body }, { status: 201 })
}

export function PUT() {
  return new Response('Method Not Allowed', { status: 405 })
}
```

Supported methods: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`.

Each handler receives `{ request, params }`:

- `request` — standard Web API `Request` object
- `params` — dynamic route parameters: `[id]` is a `string`, `[...slug]` is a `string[]`, and an
  omitted `[[...slug]]` is `undefined`.

## Response Types

Handlers must return a `Response` object (or a Promise resolving to one):

```ts
// Plain text
export function GET() {
  return new Response('Hello', { headers: { 'Content-Type': 'text/plain' } })
}

// JSON
export function GET() {
  return Response.json({ data: [1, 2, 3] })
}

// Redirect
export function GET() {
  return Response.redirect('/dashboard', 302)
}

// Error
export function GET() {
  return new Response('Not Found', { status: 404 })
}
```

## Input Validation

Validate all input close to the handler:

```ts
export async function POST({ request }: { request: Request }) {
  const body = await request.json()

  if (!body.name || typeof body.name !== 'string') {
    return Response.json({ error: 'name is required' }, { status: 400 })
  }

  return Response.json({ created: body.name }, { status: 201 })
}
```

## Body Size Limits

| Limit       | Default | Config Key             |
| ----------- | ------- | ---------------------- |
| API body    | 10 MiB  | `security.apiLimit`    |
| Action body | 1 MiB   | `security.actionLimit` |

Change the limit only when the endpoint needs it, and retain a sensible upper bound:

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'

export default config({
  security: {
    apiLimit: 20 * 1024 * 1024,
  },
})
```

## Streaming Responses

API handlers can return a `Response` backed by a `ReadableStream`. Ruvyxa forwards the status and
headers first, then streams binary-safe body chunks through the persistent worker boundary into the
HTTP response. The runtime does not materialize the complete response as one text value.

```ts
export function GET() {
  const encoder = new TextEncoder()
  const body = new ReadableStream({
    start(controller) {
      controller.enqueue(encoder.encode('first\n'))
      controller.enqueue(encoder.encode('second\n'))
      controller.close()
    },
  })

  return new Response(body, {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  })
}
```

Worker IPC uses bounded 64 KiB frames and a bounded per-response queue. A slow consumer applies
backpressure to the worker instead of truncating an already-started HTTP response; a producer that
stalls beyond the idle timeout still fails that response instead of allowing pending memory to grow
without a bound. Automatic gzip and Brotli compression is skipped for live streams whose final size
is not yet known; buffered responses with a complete size continue to use HTTP compression normally.
The interactive default is 30 seconds between worker response events and can be changed with
`RUVYXA_WORKER_TIMEOUT_MS`. Rust and Node use the same normalized value. This is automatic; route
handlers do not need a Ruvyxa-specific response type.

## Unsupported Methods

When a handler does not export a given method, the server responds with `405 Method Not Allowed`:

```json
{
  "ok": true,
  "status": 405,
  "headers": { "content-type": "text/plain; charset=utf-8" },
  "body": "Method DELETE is not allowed"
}
```

## Middleware & Security Headers

API routes automatically receive security headers, rate limiting, and middleware configured in
`ruvyxa.config.ts`.

Ruvyxa forwards the original URL query string, request bytes, and repeated request headers to the
standard `Request` object without converting binary data to text. Responses also preserve repeated
headers, including multiple `Set-Cookie` values, and preserve binary response bodies while streaming
them.

See [Configuration](configuration.md) for security and middleware settings.
