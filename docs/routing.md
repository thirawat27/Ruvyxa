# File Routing

Ruvyxa uses file-system routing. Routes are discovered automatically from the `app/` directory — no manual registration, no configuration file.

---

## Conventions

| File | Purpose |
|------|---------|
| `page.tsx` | A renderable page route |
| `route.ts` | An API route (no UI) |
| `layout.tsx` | A layout that wraps child pages |
| `server.ts` | A server-side data loader |
| `action.ts` | Server action definitions |
| `client.tsx` | An explicit client hydration module |
| `global.css` | Global styles (imported from layout) |

---

## Route Mapping

The folder structure under `app/` maps directly to URL paths:

| File Path | URL |
|-----------|-----|
| `app/page.tsx` | `/` |
| `app/about/page.tsx` | `/about` |
| `app/blog/page.tsx` | `/blog` |
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

This matches `/docs/intro`, `/docs/guides/routing`, `/docs/a/b/c`, etc.,
but not `/docs`.

---

## Optional Catch-All Routes

Use `[[...name]]` when the catch-all may consume zero segments:

```
app/shop/[[...category]]/page.tsx  →  /shop/*category?
```

This matches `/shop`, `/shop/electronics`, and
`/shop/electronics/phones`. Ruvyxa currently passes a matched catch-all value
as a slash-joined string (for example, `"electronics/phones"`).

`[[name]]` is not a valid App Router convention and fails discovery with
`RUV1002`.

---

## Route Groups

Wrap a folder name in parentheses to create a group that does not affect the URL path:

```
app/(marketing)/pricing/page.tsx  →  /pricing
app/(marketing)/about/page.tsx    →  /about
app/(dashboard)/settings/page.tsx →  /settings
```

Route groups are useful for organizing code and sharing layouts without adding URL nesting.

---

## Slot Routes

Prefix a folder with `@` to create a named slot (excluded from URL):

```
app/@sidebar/page.tsx  →  (not routable, used as a slot)
```

Slot trees are excluded from standalone route discovery. Full parallel-route
rendering semantics are not implemented yet.

---

## Private Folders

Prefix a folder with `_` to keep the entire subtree out of routing:

```
app/blog/_components/page.tsx  →  (not routable)
```

Use private folders to colocate implementation files without accidentally
publishing a URL.

---

## Layouts

`layout.tsx` files wrap all pages at the same level and below:

```
app/
├── layout.tsx          ← wraps everything
├── page.tsx
└── blog/
    ├── layout.tsx      ← wraps /blog and /blog/:slug
    ├── page.tsx
    └── [slug]/page.tsx
```

Layout nesting is automatic. A page at `/blog/hello` receives the layout chain: root layout → blog layout → page.

---

## API Routes

`route.ts` files export HTTP method handlers:

```ts
// app/api/users/route.ts
export function GET() {
  return Response.json({ users: [] })
}

export function POST(request: Request) {
  // handle creation
  return new Response(null, { status: 201 })
}
```

Supported methods: `GET`, `POST`, `PUT`, `PATCH`, `DELETE`.

API routes do not render HTML and cannot coexist with `page.tsx` in the same folder.

---

## Route Manifest

During build, Ruvyxa writes a route manifest to `.ruvyxa/manifest.json` containing every discovered route with its:

- Route path and ID
- Route kind (page or API)
- File location
- Layout chain
- Server modules
- Client modules
- Runtime target

---

## Inspecting Routes

List all routes in your project:

```bash
ruvyxa routes
```

Inspect how the server matches a specific URL:

```bash
curl "http://localhost:3000/__ruvyxa/trace?path=/blog/hello"
```

The trace endpoint returns the matched route, parsed params, layout chain, server modules, and runtime mode.

---

## Conflict Detection

Ruvyxa detects and reports overlapping route paths at discovery time. This
includes route groups that resolve to the same URL, dynamic routes that differ
only by parameter name, and a `page.tsx` plus `route.ts` at the same segment.
The build fails with `RUV1003` and identifies the conflicting file.

---

## Related

- [Data Loading](data.md) — co-locate `server.ts` loaders with your pages
- [Server Actions](actions.md) — co-locate `action.ts` mutations with your pages
- [Debugging](debugging.md) — route tracing and diagnostics
