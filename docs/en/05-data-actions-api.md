# Data, actions, and API routes

> **Tutorial goal:** move data access and writes to the server, then expose a small HTTP API safely.
> **Start from:** a route that renders correctly from
> [Routing and rendering](04-routing-rendering.md). **Checkpoint:** exercise one loader, action, or
> API route with valid and invalid input.

## Loaders and the in-memory cache

`loader(handler)` creates an async callable marked as a Ruvyxa loader. Its handler receives
`{ params, request, cache }`. `cache(key)` is an in-process cache with an LRU limit of 1024 entries,
default TTL of 60 seconds, optional stale-while-revalidate, and prefix invalidation. It is not a
distributed cache.

```ts
// app/products/server.ts
import { cache, loader } from 'ruvyxa/server'

export const products = loader(async ({ cache }) =>
  cache('products:list')
    .ttl('5m')
    .swr('1m')
    .get(async () => {
      const response = await fetch('https://example.test/products')
      if (!response.ok) throw new Error(`Upstream returned ${response.status}`)
      return response.json()
    }),
)
```

Cache durations accept a positive integer plus `ms`, `s`, `m`, `h`, or `d`.
`invalidateCache('products')` removes `products` and keys beginning `products:`; no argument clears
the complete process cache. Call `cacheStats()` to obtain `{ size, maxEntries }`.

## Public Flight payloads

A page can opt into a version-bound public payload by exporting `flight`. The function receives only
the canonical path and route params, and must return JSON-safe data. Client components read the
matched payload with `useFlight<T>()` from `@ruvyxa/react`. Ruvyxa rejects Flight requests with
cookies or authorization and falls back to a full navigation when the request fails or the browser
artifact version does not match.

```ts
// app/products/[id]/page.tsx
import type { FlightHandler } from 'ruvyxa/server'

export const flight: FlightHandler = async ({ params }) => ({
  productId: params.id,
  summary: 'Public product details',
})
```

Add a leading `'use cache'` module directive when this public payload may be cached. The directive
requires a `flight` export, uses Ruvyxa's bounded cache with a deterministic route/parameter key,
and is rejected by static-only adapters. Reading private request state from a cached producer fails
closed; authenticated data belongs in an API route or server action.

The transport endpoint is internal (`/__ruvyxa/flight`); call it through Ruvyxa navigation rather
than treating it as a general API. Production builds emit the concise `references.json`,
`actions.json`, and `flight.json` manifests. Their contract names are stable; numeric compatibility
is carried separately by `schemaVersion`, `protocolVersion`, and route `artifactVersion` fields.

## Server actions

Build an action with `action.input(schema).handler(handler)`. The schema only needs a synchronous
`parse(value)` method. The action handler receives the parsed `input`, the request, optional user
data, and `invalidate(key)`. `.realtime(channels?)` publishes after successful invocation when the
realtime capability is configured.

```ts
// app/todos/action.ts
import { action } from 'ruvyxa/server'

export const createTodo = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== 'object' || !('title' in value))
        throw new Error('title required')
      return { title: String(value.title).trim() }
    },
  })
  .realtime('todos')
  .handler(async ({ input, invalidate }) => {
    if (!input.title) throw new Error('title required')
    invalidate('todos')
    return { id: crypto.randomUUID(), ...input, completed: false }
  })
```

An action accepts at most 16 realtime channels. Channel names use 1–128 letters, digits, `:`, `.`,
`_`, `/`, or `-`. Set action payload and rate restrictions under `security`; see
[Security](13-security.md).

## API routes

Put a `route.ts` in the target folder and export an upper-case method function. The demo's
`app/api/echo/route.ts` exports `POST({ request })`, reads JSON, and returns `Response.json`. Use
the standards-based response helpers when useful: `json(data, init)`, `redirect(location, status)`,
and `notFound(message)` from `ruvyxa/server`.

```ts
// app/api/health/route.ts
export function GET() {
  return Response.json({ ok: true })
}
```

Route handlers must validate untrusted bodies before using them. API payload limits are governed by
`security.apiLimit`; action payloads use `security.actionLimit`.

**Previous:** [Routing and rendering](04-routing-rendering.md) · **Next:**
[UI, navigation, metadata, and assets](06-ui-navigation-metadata-and-assets.md)

## Reading the request

`cookies()`, `headers()`, and `draftMode()` read the request currently being served. They work in a
page component, an API route handler, and a server action. There is no parameter to thread: the
runtime installs a per-request store around each render and handler, and these read it.

```tsx
// app/dashboard/page.tsx
import { cookies, draftMode, headers } from 'ruvyxa/server'

export default function Dashboard() {
  const theme = cookies().get('theme') ?? 'light'
  const locale = headers().get('accept-language') ?? 'en'
  if (draftMode().isEnabled) return <DraftPreview locale={locale} />
  return <main data-theme={theme} lang={locale} />
}
```

- `cookies()` returns `{ get, has, getAll }` over the request's `Cookie` header. Values are returned
  as sent, minus surrounding whitespace and one layer of quoting; percent-decoding is yours to do,
  because not every cookie is encoded and decoding one that is not throws.
- `headers()` returns a read-only standard `Headers`, so `get`, `has`, and iteration behave exactly
  as they do on a `Request`.
- `draftMode()` reports whether the `__ruvyxa_draft` cookie is present. Set it from an API route
  after checking whatever secret your CMS shares with the application.

### These calls change how the page is cached

Calling any of them tells Ruvyxa the rendered HTML belongs to one visitor. That document is not
stored in the render cache, and on a prerendered strategy it is not written to the ISR cache either.
Nothing is declared and no export is needed — the call is the declaration.

The consequence worth knowing: a page that reads the request renders on every request. If you only
need one cookie for a small part of the page, keeping that part in a `'use client'` island leaves
the rest cacheable.

### Route parameters, from anywhere in the page

`params()` returns the route parameters matched for the page being rendered. A page already receives
them as props; `params()` is for everything _below_ it.

```tsx
// app/[lang]/blog/[slug]/page.tsx
import { params } from 'ruvyxa/server'

function PublishedOn({ date }: { date: Date }) {
  const { lang } = params()
  return <time dateTime={date.toISOString()}>{date.toLocaleDateString(lang as string)}</time>
}

export default function Post() {
  const { slug } = params()
  return (
    <article>
      <PublishedOn date={publishedAt(slug as string)} />
    </article>
  )
}
```

Unlike the three calls above, **this one does not change how the page is cached**. A parameter is
part of the route's identity rather than of who is asking: `/th/blog/hello` renders the same
document for every visitor, so a page that reads its own params stays statically renderable and
keeps its ISR cache entry.

- A segment declared as catch-all arrives as an array, exactly as the matcher produced it.
- A parameter that is not in the route is `undefined`, the same as reading a missing key.
- It works in a page and in an API route handler. A server action is invoked at its own endpoint
  rather than matched against a route pattern, so it has no route parameters and `params()` says so
  rather than returning an empty object — otherwise a mistyped segment name would read as "this
  route has no such parameter."

Inside a `'use client'` component, use `useParams()` from `@ruvyxa/react` instead: the browser has
no request-scoped store to read.

### Calling them outside a request

Both cases throw with a message naming the accessor:

- **At module scope.** Module bodies run at import time, when there is no request. Move the call
  inside the component or handler.
- **During a background ISR revalidation.** A scheduled re-render has no visitor. This is
  deliberate: the alternative is a page built from nobody's session and then cached for everybody.

## On-demand revalidation

`revalidatePath(path)` asks the server to re-render one URL on its next successful request. Call it
from an API route or a server action — the instruction travels back with that handler's response, so
a client that navigates on success cannot arrive before the cache has been cleared.

```ts
// app/api/revalidate/route.ts
import { revalidatePath } from 'ruvyxa/server'

export async function POST({ request }: { request: Request }) {
  const { path } = await request.json()
  revalidatePath(path)
  return Response.json({ revalidated: path })
}
```

The argument is a concrete URL (`/blog/hello`), not a route pattern (`/blog/[slug]`). Every
rendering strategy is covered: the cached document is dropped, and for SSG, ISR, PPR, and CSR the
next request additionally bypasses the HTML the build wrote to disk — otherwise that file would keep
being served. That next successful render also replaces the file, so the revalidation finishes
instead of having to bypass the same stale document for the rest of the process. A build artifact
that does not exist is left absent rather than created. A failed render, or a prerender directory
the server cannot write, keeps the revalidation pending for retry — a server that can never write
one logs a warning as the pending set fills. Revalidating a URL nothing has requested yet is fine
and is the normal webhook case. One request may queue at most 64 distinct URLs, and each URL may
contain at most 2,048 characters. `revalidatePath()` throws when either bound is exceeded; split a
larger batch across requests so no invalidation is silently dropped.

Ruvyxa has no `revalidateTag()`. In Next.js a tag labels a `fetch()` cache entry; Ruvyxa has no
fetch cache for one to label, so tags would need a page-level tag declaration and a tag-to-route
index — a design decision rather than an addition to this function.

On a serverless deployment, `revalidatePath` clears the calling function instance and the next
request rewrites the stored document for every later request. A different instance that is already
warm keeps serving its copy until its own TTL expires, which is the bound ISR already has.
