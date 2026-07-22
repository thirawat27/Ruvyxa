# Server & Client Components

## The mental model

Every Ruvyxa app has **two worlds**:

- **Server world** — runs on your machine/host. Can read the database, use secrets, and touch the
  filesystem. Its code is _never_ sent to the browser.
- **Client world** — runs in the visitor's browser. Can use `useState`, click handlers, and browser
  APIs. Everything here ships as JavaScript to every visitor.

A page starts in the server world. You opt individual components into the client world with one
directive — and the framework **verifies the boundary at build time**, so a secret cannot leak by
accident.

```text
Server world  (default)                Client world  ('use client')
─────────────────────────              ─────────────────────────────
app/page.tsx                    →      app/_components/Counter.tsx
  reads db, env, files                   useState, onClick, window
  renders HTML                           hydrates in the browser
```

## Default: Server Components

Pages are server-rendered by default. All code remains on the server — nothing is sent to the
browser bundle unless explicitly marked. This is why a Ruvyxa page can read data directly:

```tsx
// app/products/page.tsx — server component, safe to touch server data
import { db } from '../../lib/db'

export default async function ProductsPage() {
  const products = await db.products.findMany({ take: 20 })
  return (
    <ul>
      {products.map((product) => (
        <li key={product.id}>{product.title}</li>
      ))}
    </ul>
  )
}
```

## Client Components

Add the `'use client'` directive **only** to a module that needs:

- Browser APIs (`window`, `document`, `localStorage`, etc.)
- React state / effects (`useState`, `useEffect`, `useReducer`)
- Event handlers (`onClick`, `onChange`, etc.)

```tsx
'use client'

import { useState } from 'react'

export default function Counter() {
  const [count, setCount] = useState(0)
  return <button onClick={() => setCount((value) => value + 1)}>{count}</button>
}
```

### Compose them: server page, client islands

The pattern you will use every day — a server page fetches data and renders mostly-static HTML, with
small interactive client components inside:

```tsx
// app/products/page.tsx  (server — no directive)
import { AddToCart } from './_components/AddToCart' // 'use client' inside

export default async function ProductsPage() {
  const products = await getProducts() // server-side fetch
  return (
    <main>
      {products.map((product) => (
        <article key={product.id}>
          <h2>{product.title}</h2>
          <AddToCart productId={product.id} /> {/* only this hydrates */}
        </article>
      ))}
    </main>
  )
}
```

Pass server data **down through props**. Props must be JSON-serializable — functions and class
instances cannot cross the boundary.

### Which one do I need?

| The component…                        | World  | Why                          |
| ------------------------------------- | ------ | ---------------------------- |
| Shows data, no interaction            | Server | Zero JS shipped for it       |
| Has a button/input/form with handlers | Client | Needs event handlers         |
| Uses `useState`/`useEffect`           | Client | React state lives in browser |
| Reads the database or private env     | Server | Secrets stay on the server   |
| Uses `window`/`localStorage`          | Client | Browser-only APIs            |

When in doubt: start server-side and add `'use client'` only when the build or your editor tells you
a hook or handler needs it. Smaller client bundles are the reward.

## Server-Only Code

Keep private code out of the client graph. Put database access and secrets in a server-only module
and mark it clearly:

```ts
// server/database.ts
import 'server-only'

export const databaseUrl = process.env.DATABASE_URL
```

Anything under a `server/` directory is treated the same way automatically. The official state
packages are server-only by definition: importing `@ruvyxa/auth` or `@ruvyxa/database` from client
code is rejected — browser code uses `@ruvyxa/auth/client` and `@ruvyxa/realtime/client` instead.

## Boundary Validation

Ruvyxa validates imports and environment access at build time:

| Import / Access                    | Client Bundle | Server Bundle | Diagnostic |
| ---------------------------------- | ------------- | ------------- | ---------- |
| `import 'server-only'`             | **Rejected**  | Allowed       | `RUV1007`  |
| `import '@ruvyxa/auth'` (root)     | **Rejected**  | Allowed       | `RUV1007`  |
| `import '@ruvyxa/database'` (root) | **Rejected**  | Allowed       | `RUV1007`  |
| Private environment variables      | **Rejected**  | Allowed       | `RUV1008`  |
| `import 'client-only'`             | Allowed       | **Rejected**  | `RUV1009`  |
| Module under `server/` directory   | **Rejected**  | Allowed       | `RUV1010`  |
| `RUVYXA_PUBLIC_*` variables        | Allowed       | Allowed       | —          |

Do **not** work around these diagnostics by exposing a secret to the browser. Never rename a private
variable with `RUVYXA_PUBLIC_` just to silence validation — that prefix is an explicit decision to
ship the value to the browser bundle.

## Fixing the common boundary errors

| Error                           | Typical cause                                        | Fix                                                     |
| ------------------------------- | ---------------------------------------------------- | ------------------------------------------------------- |
| `RUV1007` on a component file   | A `'use client'` file imports `lib/db.ts` or similar | Fetch in the server page, pass results down as props    |
| `RUV1007` on `@ruvyxa/auth`     | Browser code imported the root package               | Import `createAuthClient` from `@ruvyxa/auth/client`    |
| `RUV1008` private env in client | `process.env.SECRET` inside a client component       | Read it server-side; pass derived, non-secret data down |
| `RUV1009` client-only in SSR    | A browser-only lib imported from a server page       | Move it into a `'use client'` component                 |

The error message names the exact import chain — follow it from the client file to the offending
module and break the chain at the first link that doesn't need to be client-side.

## Best Practices

- Keep pages server-side; push interactivity to small leaf client components.
- Pass server data to client components through serializable props.
- Use the `server/` directory prefix for server-only utilities.
- Use `import 'server-only'` for files that must stay server-side even outside `server/`.
- Treat every `'use client'` as a cost: it and everything it imports ships to the browser.

See [Environment Variables](environment-variables.md) for the public/private variable rules and
[Official Packages](official-packages.md) for the server-only state packages.
