# Data

Ruvyxa keeps data access server-side by default. Put route-specific loaders in `server.ts` next to a page.

```ts
import { loader } from "ruvyxa/server"

export const getPost = loader(async ({ params, cache }) => {
  return cache(`post:${params.slug}`).ttl("5m").get(async () => {
    return { slug: params.slug, title: "Hello Ruvyxa" }
  })
})
```

Loaders receive route params, the request, and the cache helper. Private environment variables and database clients should stay in loader or API code, not browser components.

## Common Mistakes

- Importing `server-only` code from a hydrated page fails validation.
- Reading `process.env.DATABASE_URL` from client-reachable code fails validation.
- Shared utilities that are browser-safe should live outside `server/`.

Run this before production builds:

```bash
ruvyxa analyze
```
