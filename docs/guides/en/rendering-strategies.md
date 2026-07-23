# Rendering Strategies

> 🟢 **Beginner friendly** · ⏱️ ~8 min read
>
> **You'll learn:** the five ways a page can be rendered, how to pick one in a single question, and
> how each is declared. Safe default: declare nothing.

## Which One Should I Use?

New to rendering strategies? Answer one question — _when should this page's HTML be produced?_ — and
pick from the table. You don't configure a strategy globally; each page declares (or auto-detects)
its own.

| Your page is…                                             | Use     | How                                                      |
| --------------------------------------------------------- | ------- | -------------------------------------------------------- |
| The same for everyone, rarely changes (about, docs)       | **SSG** | Do nothing — static pages are detected for you           |
| Fresh data on every request (dashboard, search results)   | **SSR** | Do nothing — pages with request-time data default to SSR |
| Mostly static but should refresh sometimes (blog listing) | **ISR** | `export const revalidate = 60`                           |
| Heavily interactive, browser-only (editor, canvas, game)  | **CSR** | `'use client'` at the top of the file                    |
| A static shell with a few slow dynamic parts              | **PPR** | `export const ppr = true` + `<Suspense>`                 |

Not sure? Do nothing — Ruvyxa picks SSG for static pages and SSR for dynamic ones, which is correct
for most pages. Run `npx ruvyxa routes` anytime to see what each page resolved to.

## Detection Order

Ruvyxa chooses a rendering strategy per page. The source detection order is significant — the
**first matching rule wins**.

| Priority | Declaration                                | Strategy | Appropriate Use                              |
| -------- | ------------------------------------------ | -------- | -------------------------------------------- |
| 1        | `'use client'` at the start of the file    | CSR      | Browser-only or heavily interactive UI       |
| 2        | `export const ppr = true`                  | PPR      | Static shell with dynamic `Suspense` regions |
| 3        | `export const revalidate = 60`             | ISR      | Content refreshed after a known interval     |
| 4        | `getStaticParams` or `staticParams` export | SSG      | Dynamic paths known at build time            |
| 5        | Static route without dynamic data markers  | SSG      | Stable pages and content                     |
| 6        | No earlier match                           | SSR      | Request-time data — the safe default         |

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

### Direct parameters with `staticParams`

When values are already known, export them without a function. A scalar shorthand is accepted when
the route has exactly one dynamic segment:

```tsx
// app/articles/[slug]/page.tsx
export const staticParams = ['getting-started', 'deployment']
```

Objects remain available for routes with multiple dynamic segments:

```tsx
export const staticParams = [
  { category: 'guides', slug: 'getting-started' },
  { category: 'news', slug: 'release-1-0-15' },
]
```

### Asynchronous parameters with `getStaticParams`

For dynamic routes whose paths are known at build time:

```tsx
// app/articles/[slug]/page.tsx
import type { GetStaticParams, PageProps } from 'ruvyxa/config'

export const getStaticParams: GetStaticParams<{ slug: string }> = async ({ route, routes }) => {
  console.log(`Generating ${route.path}; ${routes.length} routes discovered`)
  return ['getting-started', 'deployment']
}

export default function Article({ params }: PageProps<{ slug: string }>) {
  return <article>{params.slug}</article>
}
```

The context contains the current route path, its dynamic segment metadata, and all discovered
`{ path, id }` route entries. Use object entries when a route has multiple dynamic segments. For a
catch-all segment, a scalar shorthand becomes a one-item string array.

### Persistent parameter cache

Expensive parameter discovery can opt into a persistent TTL cache:

```tsx
export const getStaticParams: GetStaticParams<{ slug: string }> = async () => {
  const posts = await fetchPosts()
  return {
    params: posts.map((post) => post.slug),
    cache: '10m',
  }
}
```

`cache` accepts seconds as a positive integer or `s`, `m`, `h`, and `d` durations from one second
through 365 days. The cached parameter list is reused across worker and build invocations until the
TTL expires. A change to the page, any bundled dependency, the current route metadata, or the route
manifest invalidates it early. Returning a plain array keeps the previous uncached behaviour.

#### Constraints

- Scalar entries require exactly one dynamic segment. Otherwise each entry must be an object with a
  value for every required dynamic segment.
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

Cached output remains available while regeneration runs. Ruvyxa starts that background work only
after the configured interval and coalesces concurrent requests for the same route into one refresh.

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

## Zero-JS Pages — `export const hydrate = false`

Any server-rendered page (SSR, SSG, ISR, PPR) can opt out of client hydration entirely:

```tsx
// app/terms/page.tsx — ships zero JavaScript to the browser
export const hydrate = false

export default function TermsPage() {
  return (
    <main>
      <h1>Terms of Service</h1>
      <p>Pure content — no React runtime, no hydration bundle.</p>
    </main>
  )
}
```

What changes for that page:

- The served and prerendered HTML contains **no `<script>` tags** (dev mode keeps only the HMR
  reload client).
- The production build **skips the client bundle** for that route — nothing is emitted or shipped.
- The page cannot be interactive: `'use client'` islands inside it will render their server HTML but
  never hydrate. Event handlers and state do not run.

Use it for content that never needs JavaScript — terms, privacy, changelogs, blog posts, docs. It is
a per-page decision, so an app can mix zero-JS content pages with fully interactive pages freely.
`'use client'` (CSR) pages ignore the export — the directive wins.

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
2. Prefer an explicit export (`ppr`, `revalidate`, `staticParams`, `getStaticParams`) for routes
   whose deployment behaviour matters.
3. Inspect detected strategies with `npx ruvyxa routes`.
4. Validate route structure with `npx ruvyxa analyze`.
5. Static parameters should describe paths known definitively at build time; cache only discovery
   work whose result can safely remain unchanged for the selected TTL.
