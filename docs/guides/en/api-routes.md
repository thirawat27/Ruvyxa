# API Routes

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
headers, including multiple `Set-Cookie` values.

See [Configuration](configuration.md) for security and middleware settings.
