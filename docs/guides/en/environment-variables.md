# Environment Variables

## Public vs Private

| Type    | Prefix            | Access                                                          |
| ------- | ----------------- | --------------------------------------------------------------- |
| Public  | `RUVYXA_PUBLIC_*` | Client bundle + Server                                          |
| Private | Everything else   | Server-only (server-only modules, loaders, actions, API routes) |

## .env File

```dotenv
# .env
RUVYXA_PUBLIC_APP_NAME=Storefront
RUVYXA_PUBLIC_API_URL=https://api.example.com
DATABASE_URL=postgres://private-connection-string
```

## Using Public Variables

```tsx
const appName = import.meta.env.RUVYXA_PUBLIC_APP_NAME
```

## TypeScript Declarations

Add declarations to `app/ruvyxa-env.d.ts` so TypeScript knows the public variables:

```ts
interface ImportMetaEnv {
  RUVYXA_PUBLIC_APP_NAME: string
  RUVYXA_PUBLIC_API_URL: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
```

## Private Variables

Private variables are accessible only in:

- Server-only modules (`import 'server-only'`)
- `loader` functions
- `action` handlers
- API routes (`route.ts`)
- Modules under the `server/` directory

```ts
// server/database.ts
import 'server-only'

export const databaseUrl = process.env.DATABASE_URL
```

## Security Rules

### Do Not

- Rename a private variable with `RUVYXA_PUBLIC_` just to silence validation.
- Import a private variable into a client component.
- Pass private env values through props to a client component.
- Expose secrets through an API route unintentionally.

### Remember

`RUVYXA_PUBLIC_` is an explicit decision to ship the value to the browser bundle. Use it only for
values that are safe to expose.

## Validation

Ruvyxa validates environment variable usage during analysis:

```bash
npx ruvyxa analyze   # detects private env in client code
npx ruvyxa check     # full check including env validation
```

## .env.example

For shared projects, create `.env.example` with **only variable names, not real values**:

```dotenv
# .env.example
RUVYXA_PUBLIC_APP_NAME=
RUVYXA_PUBLIC_API_URL=
# DATABASE_URL=   (private — do not include in example)
```
