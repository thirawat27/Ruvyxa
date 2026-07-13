# Server & Client Components

## Default: Server Components

Pages are server-rendered by default. All code remains on the server — nothing is sent to the
browser bundle unless explicitly marked.

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

## Server-Only Code

Keep private code out of the client graph. Put database access and secrets in a server-only module
and mark it clearly:

```ts
// server/database.ts
import 'server-only'

export const databaseUrl = process.env.DATABASE_URL
```

## Boundary Validation

Ruvyxa validates imports and environment access at build time:

| Import / Access                  | Client Bundle | Server Bundle |
| -------------------------------- | ------------- | ------------- |
| `import 'server-only'`           | **Rejected**  | Allowed       |
| `import 'client-only'`           | Allowed       | **Rejected**  |
| Module under `server/` directory | **Rejected**  | Allowed       |
| Private environment variables    | **Rejected**  | Allowed       |
| `RUVYXA_PUBLIC_*` variables      | Allowed       | Allowed       |

Do **not** work around these diagnostics by exposing a secret to the browser. Never rename a private
variable with `RUVYXA_PUBLIC_` just to silence validation — that prefix is an explicit decision to
ship the value to the browser bundle.

## Best Practices

- Place components that need browser APIs as client components.
- Pass server data to client components through props.
- Use the `server/` directory prefix for server-only utilities.
- Use `import 'server-only'` for files that must stay server-side.

See [Environment Variables](environment-variables.md) for more details.
