# Routing

Routes in Ruvyxa are derived from file names and folders — no route configuration file needed.

## Route Table

| File                               | URL            |
| ---------------------------------- | -------------- |
| `app/page.tsx`                     | `/`            |
| `app/about/page.tsx`               | `/about`       |
| `app/blog/[slug]/page.tsx`         | `/blog/:slug`  |
| `app/docs/[...path]/page.tsx`      | `/docs/*path`  |
| `app/shop/[[...path]]/page.tsx`    | `/shop/*path?` |
| `app/(marketing)/pricing/page.tsx` | `/pricing`     |
| `app/api/health/route.ts`          | `/api/health`  |
| `app/guide/page.md` or `page.mdx`  | `/guide`       |

## Dynamic Segments

### `[name]` — Required Parameter

Use `[name]` for one required path segment. The parameter is available through the page props:

```tsx
// app/blog/[slug]/page.tsx
import type { PageProps } from 'ruvyxa/config'

export default function BlogPost({ params }: PageProps<{ slug: string }>) {
  return <h1>Post: {params.slug}</h1>
}
```

### `[...path]` — Catch-All (1+ segments)

Matches one or more remaining path segments.

### `[[...path]]` — Optional Catch-All (0+ segments)

Matches zero or more remaining path segments — the route still works when the catch-all is absent.

## Route Groups

Use `(...)` to organize files without affecting the URL:

```text
app/
├── (marketing)/
│   ├── about/page.tsx    → /about
│   └── pricing/page.tsx  → /pricing
└── (dashboard)/
    └── settings/page.tsx → /settings
```

## Naming Rules

- Folders starting with `_` or `@` are **ignored** during route discovery.
- Ruvyxa **rejects ambiguous structures** instead of silently choosing a route:
  - Do not create two routes that resolve to the same URL.
  - Do not create dynamic siblings such as `[id]` and `[slug]` in the same directory.
  - Do not place both `page.*` and `route.ts` in the same directory.
- Run `npx ruvyxa analyze` after changing route structure.

## Validation

```bash
npx ruvyxa analyze   # validate routes, imports, server/client boundaries
npx ruvyxa routes    # print route table with detected render strategies
```

## API Routes

See [API Routes](api-routes.md) for `route.ts` and HTTP method handlers.
