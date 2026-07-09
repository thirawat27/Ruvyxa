# File Routing

Ruvyxa uses file-system routing. Routes are discovered automatically from the `app/` directory — no
manual registration, no configuration file.

---

## Conventions

| File         | Purpose                              |
| ------------ | ------------------------------------ |
| `page.tsx`   | A renderable page route              |
| `page.jsx`   | A renderable page route (JSX)        |
| `route.ts`   | An API route (no UI)                 |
| `route.js`   | An API route (no UI, JS)            |
| `layout.tsx` | A layout that wraps child pages      |
| `server.ts`  | A server-side data loader            |
| `server.js`  | A server-side data loader (JS)       |
| `action.ts`  | Server action definitions            |
| `action.js`  | Server action definitions (JS)       |
| `client.tsx` | An explicit client hydration module  |
| `global.css` | Global styles (imported from layout) |

---

## Route Mapping

The folder structure under `app/` maps directly to URL paths:

| File Path                 | URL           |
| ------------------------- | ------------- |
| `app/page.tsx`            | `/`           |
| `app/about/page.tsx`      | `/about`      |
| `app/blog/page.tsx`       | `/blog`       |
| `app/api/health/route.ts` | `/api/health` |

---

## Dynamic Routes

Wrap a folder name in brackets to create a dynamic segment:

```
app/blog/[slug]/page.tsx  →  /blog/:slug
```

The matched value is passed to your component via `params`:

```tsx
export default function BlogPost({ params }: { params: { slug: string } }) {
  return <h1>{params.slug}</h1>
}
```

---

## Catch-All Routes

Use `[...name]` to match one or more segments:

```
app/docs/[...path]/page.tsx  →  /docs/*path
```

Matches `/docs/intro`, `/docs/guides/routing`, `/docs/a/b/c`, etc., but not `/docs`.

---

## Optional Catch-All Routes

Use `[[...name]]` when the catch-all may consume zero segments:

```
app/shop/[[...category]]/page.tsx  →  /shop/*category?
```

Matches `/shop`, `/shop/electronics`, and `/shop/electronics/phones`. The catch-all value is
passed as a slash-joined string (e.g., `"electronics/phones"`).

`[[name]]` (single optional segment) is not a valid convention and fails with `RUV1002`.

---

## Route Groups

Wrap a folder name in parentheses to create a group that does not affect the URL path:

```
app/(marketing)/pricing/page.tsx  →  /pricing
app/(dashboard)/settings/page.tsx  →  /settings
```

Route groups organize code and share layouts without adding URL nesting.

---

## Private Folders

Prefix a folder with `_` to keep the entire subtree out of routing:

```
app/blog/_components/page.tsx  →  (not routable)
```

Folders starting with `_` or `@` are excluded entirely from route discovery.

---

## Layouts

`layout.tsx` files wrap all pages at the same level and below. Nesting is automatic:

```
app/
├── layout.tsx          ← wraps everything
├── page.tsx
└── blog/
    ├── layout.tsx      ← wraps /blog and /blog/:slug
    ├── page.tsx
    └── [slug]/page.tsx
```

A page at `/blog/hello` receives the layout chain: root layout → blog layout → page.

---

## API Routes

`route.ts` files export HTTP method handlers:

```ts
export function GET() {
  return Response.json({ users: [] })
}

export async function POST({ request }: { request: Request }) {
  const body = await request.json()
  return Response.json({ received: body }, { status: 201 })
}
```

Supported methods: `GET`, `POST`, `PUT`, `PATCH`, `DELETE`.

API routes do not render HTML and cannot coexist with `page.tsx` in the same folder.

---

## Route Manifest

During build, Ruvyxa writes `.ruvyxa/manifest.json` containing every discovered route with its:

- Route path and ID
- Route kind (`page` or `api`)
- File location
- Layout chain
- Server modules (`server.ts`, `action.ts`, etc.)
- Client modules (`client.tsx`)
- Runtime target (`node`, `edge`, `static`)
- Render strategy (SSR, SSG, ISR, CSR, PPR) with revalidation metadata

---

## Inspecting Routes

List all routes:

```bash
ruvyxa routes
```

Inspect a specific URL:

```bash
ruvyxa trace /blog/hello
```

The trace endpoint returns the matched route, parsed params, layout chain, server modules, client
modules, and runtime mode.

---

## Conflict Detection

Ruvyxa detects overlapping route paths at discovery time, including:

- Route groups that resolve to the same URL
- Dynamic routes differing only by parameter name
- `page.tsx` plus `route.ts` at the same segment

The build fails with `RUV1003` and identifies the conflicting file.

---

## Server Modules Detection

Server-only modules are detected as sibling files beside route pages:

- `server.ts` — data loaders
- `server.js` — data loaders (JS)
- `action.ts` — server actions
- `action.js` — server actions (JS)

These files are excluded from client bundles and enforce the server/client boundary at build time.

---

## Related

- [Data Loading](data.md) — co-locate `server.ts` loaders with your pages
- [Server Actions](actions.md) — co-locate `action.ts` mutations with your pages
- [Debugging](debugging.md) — route tracing and diagnostics