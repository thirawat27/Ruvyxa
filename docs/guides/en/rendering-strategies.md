# Rendering Strategies

Ruvyxa chooses a rendering strategy per page. The source detection order is significant — the
**first matching rule wins**.

## Detection Order

| Priority | Declaration                               | Strategy | Appropriate Use                              |
| -------- | ----------------------------------------- | -------- | -------------------------------------------- |
| 1        | `'use client'` at the start of the file   | CSR      | Browser-only or heavily interactive UI       |
| 2        | `export const ppr = true`                 | PPR      | Static shell with dynamic `Suspense` regions |
| 3        | `export const revalidate = 60`            | ISR      | Content refreshed after a known interval     |
| 4        | `export const getStaticParams = ...`      | SSG      | Dynamic paths known at build time            |
| 5        | Static route without dynamic data markers | SSG      | Stable pages and content                     |
| 6        | No earlier match                          | SSR      | Request-time data — the safe default         |

## SSR — Server-Side Rendering (Default)

Rendered on every request:

```tsx
export default async function ProductPage() {
  const products = await db.products.findMany()
  return <ProductList items={products} />
}
```

## SSG — Static Site Generation

### Static pages

Static routes without dynamic data markers and without `'use client'` are auto-detected as SSG
candidates. They are pre-rendered at build time and served as static HTML.

### Dynamic SSG with `getStaticParams`

For dynamic routes whose paths are known at build time:

```tsx
// app/articles/[slug]/page.tsx
import type { GetStaticParams, PageProps } from 'ruvyxa/config'

export const getStaticParams: GetStaticParams<{ slug: string }> = async () => [
  { slug: 'getting-started' },
  { slug: 'deployment' },
]

export default function Article({ params }: PageProps<{ slug: string }>) {
  return <article>{params.slug}</article>
}
```

#### Constraints

- Each entry must be an object with string values for every dynamic segment.
- Values cannot contain path traversal, query, or fragment characters (`..`, `/`, `\`, `?`, `#`).
- Generated output stays inside `.ruvyxa/prerender`.

## ISR — Incremental Static Regeneration

For data that may become stale but does not need a render on every request:

```tsx
export const revalidate = 60 // seconds

export default async function ProductPage() {
  return <main>Product data refreshed after at most 60 seconds.</main>
}
```

## PPR — Partial Pre-rendering

Static shell with dynamic `Suspense` regions:

```tsx
export const ppr = true

export default function PPRPage() {
  return (
    <main>
      <h1>Static Shell</h1>
      <Suspense fallback={<p>Loading…</p>}>
        <DynamicContent />
      </Suspense>
    </main>
  )
}
```

Only the static shell is pre-rendered; dynamic slots are streamed at request time.

## CSR — Client-Side Rendering

```tsx
'use client'

import { useState, useEffect } from 'react'

export default function InteractiveDashboard() {
  const [data, setData] = useState(null)
  useEffect(() => {
    fetch('/api/dashboard')
      .then((r) => r.json())
      .then(setData)
  }, [])
  // ...
}
```

At build time, a minimal shell HTML is emitted for CSR routes.

## Pre-render Output

SSG, ISR, PPR, and CSR routes are pre-rendered at build time:

```text
.ruvyxa/prerender/
├── manifest.json          # route list with strategy and revalidate
├── index.html             # /
├── about/index.html       # /about
└── blog/
    └── hello-world/
        └── index.html     # /blog/hello-world
```

## Best Practices

1. Let SSR be the default — opt into other strategies only when you have a clear reason.
2. Prefer an explicit export (`ppr`, `revalidate`, `getStaticParams`) for routes whose deployment
   behaviour matters.
3. Inspect detected strategies with `npx ruvyxa routes`.
4. Validate route structure with `npx ruvyxa analyze`.
5. `getStaticParams` should return paths known definitively at build time.
