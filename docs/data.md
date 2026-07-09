# Data Loading

Ruvyxa keeps data fetching on the server by default. Co-locate a `server.ts` file next to any page
to define loaders that run during SSR — never in the browser.

---

## Loaders

A loader is a server-side function that fetches data for a page. Place it in `server.ts` beside the
page that needs the data:

```
app/blog/[slug]/
├── page.tsx      ← renders the post
└── server.ts     ← fetches the post data
```

```ts
// app/blog/[slug]/server.ts
import { loader } from 'ruvyxa/server'

export const getPost = loader(async ({ params }) => {
  const post = await db.posts.findBySlug(params.slug)
  return post
})
```

### What loaders receive

| Property  | Type                     | Description                      |
| --------- | ------------------------ | -------------------------------- |
| `params`  | `Record<string, string>` | Matched dynamic route parameters |
| `request` | `Request`                | The incoming HTTP request        |
| `cache`   | `CacheHelper`            | Built-in caching utility         |

---

## Caching

Use the `cache` helper for time-based caching with optional stale-while-revalidate:

```ts
export const getPost = loader(async ({ params, cache }) => {
  return cache(`post:${params.slug}`)
    .ttl('5m')
    .swr('1h')           // serve stale while revalidating in background
    .get(async () => {
      return db.posts.findBySlug(params.slug)
    })
})
```

### TTL format

| Value   | Duration   |
| ------- | ---------- |
| `"30s"` | 30 seconds |
| `"5m"`  | 5 minutes  |
| `"1h"`  | 1 hour     |
| `"1d"`  | 1 day      |

The cache uses a FIFO eviction policy (1024 max entries) with periodic cleanup every 60 seconds.
It is per-process and invalidated on server restart. For distributed caching, connect your own
client inside the loader body.

### Cache API

| Function                    | Description                        |
| --------------------------- | ---------------------------------- |
| `cache(key)`                | Create a cache builder for a key   |
| `.ttl(value)`               | Set time-to-live                   |
| `.swr(value)`               | Set stale-while-revalidate window  |
| `.get(producer)`            | Get cached value or run producer   |
| `invalidateCache(key)`      | Invalidate by exact key or prefix  |
| `invalidateCache()`         | Clear entire cache                 |
| `cacheStats()`              | Get current cache size / max       |

---

## Multiple Loaders

A single `server.ts` can export multiple loaders:

```ts
import { loader } from 'ruvyxa/server'

export const getPost = loader(async ({ params }) => db.posts.findBySlug(params.slug))
export const getRelatedPosts = loader(async ({ params }) => db.posts.findRelated(params.slug, { limit: 5 }))
```

---

## Using Environment Variables

Loaders run on the server, so they have access to all environment variables:

```ts
import { loader } from 'ruvyxa/server'

export const getUser = loader(async ({ params }) => {
  const res = await fetch(`${process.env.API_BASE_URL}/users/${params.id}`, {
    headers: { Authorization: `Bearer ${process.env.API_SECRET}` },
  })
  return res.json()
})
```

Private env vars (`process.env.DATABASE_URL`, `process.env.API_SECRET`, etc.) are never exposed to
the browser.

---

## Server/Client Boundary

Ruvyxa enforces a strict server/client boundary at build time:

- Loader code in `server.ts` is server-only. It never reaches the browser bundle.
- Page code in `page.tsx` is server-rendered but also hydrated in the browser.
- If a page imports a module marked `"server-only"`, the build fails with `RUV1007`.
- If browser-reachable code reads a private `process.env.*` variable, the build fails with
  `RUV1008`.

### Safe patterns

```ts
// server.ts — safe: runs only on the server
import { loader } from 'ruvyxa/server'
import { db } from '../../lib/db'     // server-only database client

export const getData = loader(async () => db.query('SELECT * FROM posts'))
```

### Unsafe patterns

```tsx
// page.tsx — unsafe: this code reaches the browser
import { db } from '../../lib/db'     // RUV1007 if db imports "server-only"

export default function Page() {
  const url = process.env.DATABASE_URL  // RUV1008: private env in client
  return <p>{url}</p>
}
```

---

## Validation

Run `ruvyxa check` before deploying to catch boundary violations and type errors:

```bash
ruvyxa check
```

This walks the import graph of every page, reports server-only code or private env vars reachable
from client bundles, and smoke-renders page routes in both dev and production mode.

---

## Common Mistakes

| Mistake                                      | Diagnostic | Fix                                              |
| -------------------------------------------- | ---------- | ------------------------------------------------ |
| Importing `server-only` module from a page   | `RUV1007`  | Move the import into `server.ts`                 |
| Reading `process.env.SECRET` in page code    | `RUV1008`  | Use the value in a loader and pass data as props |
| Putting shared utilities in `server/` folder | `RUV1010`  | Move browser-safe code outside `server/`         |

---

## Related

- [Server Actions](actions.md) — for mutations and writes
- [Routing](routing.md) — co-located file conventions
- [Debugging](debugging.md) — diagnostic codes and boundary errors