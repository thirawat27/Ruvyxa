# Routing and rendering

> **Tutorial goal:** add a dynamic route and deliberately choose how it renders. **Start from:** the
> route conventions in [Project structure](03-project-structure.md). **Checkpoint:** visit one
> concrete dynamic URL in development, then run the application check.

Route discovery turns the file tree into a manifest. Run `npm run routes` while developing to
inspect it; use `npm run routes:json` when a script needs machine-readable output. A page's strategy
is selected from its exports and the `render` configuration.

| Strategy | Selection evidenced in source             | When HTML is produced                                       |
| -------- | ----------------------------------------- | ----------------------------------------------------------- |
| SSR      | default, or `render.strategy: 'ssr'`      | every request                                               |
| SSG      | static route/static parameter discovery   | build time                                                  |
| ISR      | `export const revalidate = 60`            | build time, then revalidated after TTL                      |
| CSR      | `'use client'` page                       | browser after a minimal shell                               |
| PPR      | `export const ppr = true` with `Suspense` | static shell at build; dynamic slot streams at request time |

## Markdown, MDX, and shared components

Create `page.md` for Markdown or `page.mdx` for JSX, expressions, and imports; neither format needs
compiler configuration. To style native MDX elements or provide shared components, add the nearest
`mdx-components.tsx` (also `.ts`, `.jsx`, `.js`, `.mts`, or `.mjs`) in the page's directory or an
ancestor:

```tsx
// app/mdx-components.tsx
export function useMDXComponents(components = {}) {
  return {
    ...components,
    h1: (props) => <h1 className="docs-title" {...props} />,
  }
}
```

The closest provider wins, so `app/docs/mdx-components.tsx` can specialize only the routes below
`app/docs/`. Explicit `components` passed to the page are given to `useMDXComponents`; merge them in
the returned object when they should remain available. Providers are normal client-graph modules, so
server-only imports and private environment variables are rejected by `ruvyxa check`.

Ruvyxa includes `@mdx-js/mdx` and GFM. Add unified-compatible remark, rehype, or recma plugins to
the application's dependencies, import them in `ruvyxa.config.ts`, and register them once for both
`.md` and `.mdx`:

```ts
import rehypeAutolinkHeadings from 'rehype-autolink-headings'
import rehypeSlug from 'rehype-slug'
import remarkToc from 'remark-toc'
import { config } from 'ruvyxa/config'

export default config({
  markdown: {
    remarkPlugins: [[remarkToc, { heading: 'contents', maxDepth: 3 }]],
    rehypePlugins: [rehypeSlug, [rehypeAutolinkHeadings, { behavior: 'append' }]],
  },
})
```

Plugin order is preserved. Ruvyxa collects `headings` after application rehype plugins, so a custom
heading `id` is also the exported slug. A remark or rehype plugin can update
`file.data.ruvyxa.frontmatter`; the final value must remain a JSON-compatible object. GFM is on by
default and can be disabled with `markdown.gfm: false`. Raw HTML written in `.md` remains escaped;
use `.mdx` when executable JSX is intentional.

## Dynamic SSG

For a dynamic SSG/ISR page, export `getStaticParams`. It receives all discovered routes and the
current route description, and it returns objects (or a single-segment string/number shorthand). The
result can be wrapped with `{ params, cache }`, where `cache` accepts seconds or a string such as
`"10m"`.

```tsx
// app/blog/[slug]/page.tsx
import type { GetStaticParams, PageProps } from 'ruvyxa'

export const getStaticParams: GetStaticParams<{ slug: string }> = () => [
  { slug: 'first-post' },
  { slug: 'release-notes' },
]

export default function Post({ params }: PageProps<{ slug: string }>) {
  return (
    <article>
      <h1>{params.slug}</h1>
    </article>
  )
}
```

`generateStaticParams` and `staticParams` are accepted as names for the same export, so a page
brought over from Next.js declares its parameters without being renamed.

## Overriding the rendering strategy

Ruvyxa picks a strategy automatically, and `export const dynamic` overrides it — the same route
segment config Next.js uses, with the same precedence. `'force-dynamic'` takes the route off the
pre-render path even if it also exports `revalidate`; `'force-static'` and `'error'` put it on;
`'auto'` is the default. `export const revalidate = <seconds>` opts into ISR and
`export const ppr = true` into partial pre-rendering.

`export const metadata` is **not** read: Next's metadata object is nested where Ruvyxa's `meta` is
flat, so the two are not interchangeable. Use `export const meta` below.

## Route metadata and boundaries

`export const meta` accepts a `Meta` object or `MetaFactory`. Layout metadata merges from root to
leaf; the most specific value wins. A lower-level title is formatted by the nearest ancestor
`titleTemplate`.

```tsx
// app/layout.tsx
import type { Meta } from '@ruvyxa/react'
export const meta: Meta = { titleTemplate: '%s — Example', siteName: 'Example' }

// app/blog/[slug]/page.tsx
export const meta = ({ params }: { params: Record<string, string> }) => ({
  title: params.slug,
  canonical: `https://example.test/blog/${params.slug}`,
})
```

`error.tsx` receives `{ error, reset, retry }`; `loading.tsx` and `not-found.tsx` are plain
components. To select the nearest `not-found.tsx`, import `notFound` from `@ruvyxa/react` and call
it (it throws a tagged signal). Do not confuse it with `notFound` from `ruvyxa/server`, which
creates an HTTP `Response` with status 404.

### `template.tsx`

A `template.tsx` wraps its level's children the way a layout does, and differs in the one respect
that is the reason to reach for it: it is keyed by the request path, so navigating within the same
layout **remounts** it — state resets and effects run again — while the layout above it stays
mounted. Use it for an enter animation, a per-navigation `useEffect`, or state that must not survive
a move between sibling routes.

```tsx
// app/dashboard/template.tsx
'use client'
import { useEffect } from 'react'

export default function DashboardTemplate({ children }: { children: React.ReactNode }) {
  useEffect(() => {
    // Runs again on every navigation inside app/dashboard/, unlike a layout.
  }, [])
  return <section className="fade-in">{children}</section>
}
```

Layouts and templates nest `layout > template > children` at each level, so a level may have either,
both, or neither. Templates declare no metadata; `export const meta` belongs on a layout or a page.

### Parallel routes

A `@name` folder beside a `layout.tsx` declares a slot that layout receives as a prop, alongside the
page it already renders as `children`. Use it when one screen is several independent panels rather
than one page.

```text
app/dashboard/
├── @activity/
│   ├── default.tsx
│   └── page.tsx
├── @team/
│   ├── page.tsx
│   └── reports/page.tsx
├── layout.tsx
├── page.tsx
└── reports/page.tsx
```

```tsx
// app/dashboard/layout.tsx
export default function DashboardLayout({
  children,
  team,
  activity,
}: {
  children: React.ReactNode
  team?: React.ReactNode
  activity?: React.ReactNode
}) {
  return (
    <div className="grid">
      <aside>{team}</aside>
      <aside>{activity}</aside>
      <main>{children}</main>
    </div>
  )
}
```

Each slot matches the URL independently of the page. At `/dashboard/reports` the page comes from
`reports/page.tsx` and `team` from `@team/reports/page.tsx`; `activity` has nothing for that URL and
renders its `default.tsx`. A slot with neither a matching page nor a `default.tsx` is left out — the
layout does not receive the prop at all. A `@name` folder never becomes a route of its own.

Two limits worth knowing: a slot's own nested `layout.tsx` or `loading.tsx` is not composed into the
slot subtree, and an unmatched slot falls back to `default.tsx` on every navigation rather than
keeping what it last rendered.

### Intercepting routes are not implemented

Ruvyxa does not implement the `(.)`, `(..)`, `(..)(..)`, and `(...)` folder conventions. A folder
whose name opens with one of them fails route discovery with **RUV1005**, wherever it sits under
`app/` — including inside a `@slot` folder.

The check exists because the convention used to do something else quietly. A route group needs a
trailing `)`, so `app/feed/(.)photo/` was not stripped as one: it became a literal URL segment and
published a real page at `/feed/(.)photo` — a view written to be shown over another route, given its
own public address. Inside a `@slot` the same folder matched no URL and rendered nothing at all.

Rename the folder to an ordinary segment, and render the overlay from a route the layout already
composes — a parallel slot, or client state that keeps the underlying page mounted.

### A complete route-state boundary

Put the three special files beside the segment they should protect. The closest matching file wins,
so this structure gives all product pages a loading UI, error retry, and product-specific 404
without changing every page.

```text
app/products/
├── [slug]/
│   └── page.tsx
├── error.tsx
├── loading.tsx
└── not-found.tsx
```

```tsx
// app/products/[slug]/page.tsx
import { notFound } from '@ruvyxa/react'

const products = { notebook: 'A plain notebook' }

export default function Product({ params }: { params: { slug: string } }) {
  const product = products[params.slug as keyof typeof products]
  if (!product) notFound()
  return (
    <main>
      <h1>{product}</h1>
    </main>
  )
}
```

```tsx
// app/products/loading.tsx
export default function Loading() {
  return <main aria-busy="true">Loading products…</main>
}

// app/products/not-found.tsx
export default function ProductNotFound() {
  return (
    <main>
      <h1>Product not found</h1>
    </main>
  )
}
```

```tsx
// app/products/error.tsx
'use client'
import type { RouteErrorProps } from '@ruvyxa/react'

export default function ProductError({ error, reset }: RouteErrorProps) {
  return (
    <main>
      <h1>We could not load this product</h1>
      <p>{error.message}</p>
      <button type="button" onClick={reset}>
        Try again
      </button>
    </main>
  )
}
```

`loading.tsx` does two jobs. On the server it is the route's Suspense fallback. In the browser it is
also the route's **loading shell**: when you navigate to a route that has one, Ruvyxa paints its
layouts and its `loading.tsx` as soon as the route's bundle is available, without waiting for the
page's server data. The content replaces the fallback when the payload arrives.

That is what makes a navigation to a slow route feel immediate rather than like a dead click, and it
costs no extra request — the layouts and the loading component are already in the bundle that
`<Link prefetch>` warms. A route with no `loading.tsx` has no declared loading state, so the
previous page stays on screen until the new one is ready, exactly as before.

`error.tsx` gets both recovery paths. `reset()` clears the boundary and re-renders against the data
the client already has, which recovers from a fault in the render itself. `retry()` re-requests the
route from the server first, which is what you want when the failure _was_ the data — it returns a
promise that resolves once the boundary has been reset. Outside a mounted router `retry()` falls
back to `reset()`.

`not-found.tsx` can render on the server, but `reset()` re-renders after hydration, so make an
`error.tsx` with a retry control a client component. Keep error text safe for end users; log
diagnostic detail on the server or through your observability integration instead of rendering
secrets or stack traces.

## i18n route policy

`i18n.locales` and `i18n.defaultLocale` are configuration fields. Locale routing is file-system
based (for example `app/[lang]/about/page.tsx`); the default parameter name is `lang`. With locale
detection enabled, the server considers the configured cookie (default `RUVYXA_LOCALE`) and
`Accept-Language`.

**Previous:** [Project structure](03-project-structure.md) · **Next:**
[Data, actions, and API routes](05-data-actions-api.md)
