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

The strategy decides _when_ HTML is produced. `export const serverComponents = true` decides _which
graphs_ produce it, and composes with every strategy above except PPR — see
[React Server Components](#react-server-components).

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

## Choosing a runtime

`export const runtime = 'edge'` declares that a route uses only what a Web-standards runtime offers
— `Request`, `Response`, `fetch`, `URL`, `crypto`. `'nodejs'` is the default and does not need
writing. Both are spelled the way Next.js spells them.

```tsx
// app/ping/route.ts
export const runtime = 'edge'

export function GET({ request }: { request: Request }) {
  return Response.json({ ok: true, from: new URL(request.url).hostname })
}
```

The declaration is **checked, not just recorded**. Anything in the route's module graph that imports
a Node built-in a V8 isolate does not have — `fs`, `child_process`, `net`, `worker_threads`, and the
rest of the list — fails the build with `RUV1013`, naming the module and the import. A value that is
neither `'edge'` nor `'nodejs'` is `RUV1012` rather than a silent fall back to Node, because the
export exists precisely to say where the route belongs.

Where the route physically runs is then the adapter's decision. The build writes the declaration
into the `deploy` section of `manifest.json`, so an adapter that can place work on an edge network
has what it needs; an adapter whose only host is Node serves the route from its function, which is
always correct — every API an edge route may use exists in Node too, so the declaration narrows what
the route may do rather than changing what can answer it.

That is the trade to hold in mind: declaring `'edge'` buys a route the option of running on an edge
network and costs it the Node standard library. A route that reads a file wants the default.

## React Server Components

`export const serverComponents = true` renders a route through React's server-components pipeline.
The page and its layouts run in a module graph resolved with React's `react-server` condition, and
only the modules marked `'use client'` reach the browser.

```tsx
// app/dashboard/page.tsx
import { readFile } from 'node:fs/promises'
import Chart from './chart'

export const serverComponents = true

export default async function Dashboard() {
  const rows = JSON.parse(await readFile('./data/metrics.json', 'utf8'))
  return <Chart rows={rows} />
}
```

```tsx
// app/dashboard/chart.tsx
'use client'
import { useState } from 'react'

export default function Chart({ rows }: { rows: Row[] }) {
  const [range, setRange] = useState('30d')
  // ...
}
```

`page.tsx` above is never bundled for the browser. `chart.tsx` is, and it is the only module from
this route that is. The page is turned into a payload — a serialised element tree in which `Chart`
appears as a reference id rather than as code — which the server renders to HTML and the browser
replays to hydrate. Both halves read the same payload, so what hydrates is what was rendered.

The payload rides in a `<script type="application/json">` data block, like the route bootstrap: a
`Content-Security-Policy` without `'unsafe-inline'` does not block it, and no nonce is needed.

### Installing the runtime

Server components need `react-server-dom-webpack`, at **exactly** the version of React your project
already resolves — not merely a compatible range. Read it, then install that:

```bash
npm ls react
npm install react-server-dom-webpack@<the version that printed>
```

It is optional: an app that never writes the export does not need it, and a route that does gets
`RUV1863` naming it. The package lists `webpack` as a peer for one file — its bundler plugin, which
Ruvyxa never loads — so tell your package manager to skip it. For pnpm:

```yaml
# pnpm-workspace.yaml
peerDependencyRules:
  ignoreMissing:
    - webpack
```

Node's own APIs are ordinary code inside a server component. TypeScript needs to know they exist,
which means `@types/node` and `"types": ["node"]` in `tsconfig.json` — with the caveat that this
makes Node's globals visible to your `'use client'` files too, where the boundary checks, not the
type checker, are what stop them being used.

### What a server component cannot do

The `react-server` build of React has no `useState`, no `useEffect`, and no `createContext`, so a
server component cannot hold state, run an effect, or provide context. That is the boundary, not a
limitation to work around: move those parts into a `'use client'` module and pass data down as
props. `Suspense` works in both graphs, so `loading.tsx` behaves as it does on any other route.

`error.tsx` and `not-found.tsx` are class-based boundaries, which the server graph cannot run. On a
server-components route they must be `'use client'` modules — the same rule React itself imposes.
`ruvyxa build` reports a route whose `error.tsx` lacks the directive, because the server still
renders it: without the warning you would see your own error page whenever the failure happened
during the server render, and a different one whenever it happened in the browser.

A server component that throws inside `<Suspense>` does not abort the response. The shell has
already been streamed by then, so the document goes out with the fallback in place and the reader
gets a page. The error travels in the payload as well, and the browser raises it while reading —
which is why the route is always wrapped in a boundary, whether or not you wrote `error.tsx`. With
one, yours renders. Without, a plain message with a retry button does. Neither is a blank page,
which is what an unhandled one produces: React unmounts the document and leaves a single console
line behind.

To keep the rest of the page and lose only the part that failed, put an error boundary _inside_ the
`<Suspense>` that owns it. `<Suspense>` handles promises, not errors, so nothing else can contain
one to that subtree.

`@ruvyxa/react` is safe to import from a server component. `Link`, the routing hooks, `Script`,
`RuvyxaErrorBoundary`, and `useRuvyxaLoader` declare `'use client'` themselves, so a root layout can
render its `<Link>` nav on a server-components route with no change at all — the server graph gets a
reference and the browser resolves it. `Image`, `Seo`, and `notFound()` have no browser half and are
rendered by the server component itself.

The same applies to any package you install: a component that uses hooks has to declare
`'use client'` in its published files, or the server graph will compile it against a build of React
that has no hooks in it.

### Server functions

A function behind `'use server'` runs on the server and is callable from the browser. Ruvyxa
supports both of React's spellings.

A whole module, which is the form a `'use client'` component imports:

```ts
// app/dashboard/actions.ts
'use server'

export async function rename(id: string, name: string) {
  await db.rename(id, name)
  return db.get(id)
}
```

```tsx
// app/dashboard/row.tsx
'use client'
import { rename } from './actions'

export function Row({ id }: { id: string }) {
  return <button onClick={() => rename(id, 'new')}>Rename</button>
}
```

None of `actions.ts` is in the browser bundle. `rename` there is a _reference_: calling it posts the
arguments to the server, runs the real function, and resolves to what it returned — including an
element tree, because the reply is a Flight payload rather than JSON.

Or one function inside the server component that uses it, which is the form that needs no second
file:

```tsx
// app/dashboard/page.tsx
export const serverComponents = true

export async function markAllRead(userId: string) {
  'use server'
  await db.markAllRead(userId)
}

export default async function Dashboard() {
  return <Toolbar onClear={markAllRead} />
}
```

The function is handed to a `'use client'` component as an ordinary prop and arrives there as a
reference, exactly as an imported one does.

**An inline server function has to be at the top level of its module.** One declared inside another
function closes over that call's variables, and a call arriving later — from a different request, in
a different process — has no way to reconstruct them. Rather than compile such a function into one
that reads values from a render that ended, Ruvyxa refuses it with `RUV1867` and names the line.
Move it to the top level, or into a module that opens with the directive.

Calls go to `POST /__ruvyxa/rsc`, the same endpoint that serves a route's payload, with the
visitor's cookies attached and a same-origin header a cross-origin page cannot set. A server
function is reachable from the route whose page or client components import it.

A `<form action={fn}>` works **before its JavaScript has loaded, and without any**. React writes the
function's reference into hidden fields while rendering the form, so a browser that has run none of
the page's bundle can still submit it: the post goes to the page's own URL, Ruvyxa recognises the
fields, runs the function, and answers with a freshly rendered document that already contains the
result. `useActionState` is what carries that result into the markup — the value the action returned
is replayed into the hook, so the same component renders the same answer whether it was reached over
`fetch` or over a form submission.

```tsx
// app/search/form.tsx
'use client'

import { useActionState } from 'react'

import { lookup } from './actions'

export default function Search() {
  const [answer, submit] = useActionState(lookup, null)
  return (
    <form action={submit}>
      <input name="q" />
      <output>{answer ?? 'nothing looked up yet'}</output>
    </form>
  )
}
```

Once the bundle has loaded React intercepts the submit instead, calls the same function over
`fetch`, and updates only what changed. Nothing about the form is written twice.

A submitted form is answered by the route's own render rather than by its rendering strategy: a
pre-rendered or cached document was produced before the action ran, so the response is rendered
fresh and carries `Cache-Control: no-store`. Anything the action passed to `revalidatePath()` is
applied before the response is returned. Server functions need a `react-server` graph to be resolved
against, so this applies to server-components routes; a `POST` to any other page renders it exactly
as it always did.

Ruvyxa's own server actions are unchanged and still available from a `'use client'` component on a
server-components route: see [Data, actions, and API routes](05-data-actions-api.md).

### Streaming the document

A server-components route whose document is produced per request is **streamed**. React sends the
shell as soon as it has it and each `Suspense` boundary as the server resolves it, so a slow server
component delays the part of the page waiting on it and nothing else.

```tsx
// app/dashboard/page.tsx
import { Suspense } from 'react'

export const serverComponents = true
export const dynamic = 'force-dynamic'

async function Revenue() {
  return <Chart rows={await db.revenue()} />
}

export default function Dashboard() {
  return (
    <main>
      <h1>Dashboard</h1>
      <Suspense fallback={<Skeleton />}>
        <Revenue />
      </Suspense>
    </main>
  )
}
```

Without `Suspense` the whole document still waits for `db.revenue()` — a boundary is what gives the
server something to send first.

**Only a per-request document streams.** `export const dynamic = 'force-dynamic'`, or anything else
that makes a route dynamic, is what selects it. A pre-rendered, `revalidate`, or statically rendered
route has to become a string to be written to disk or held in a cache, and a stream is the wrong
shape for that — those routes are still sent whole. A route without server components does not
stream either: its render resolves in one step, so there is nothing to send early.

A streamed response carries `Cache-Control: no-store` and no `Content-Length`, and it is not held in
the render cache. That follows from the same fact: the document never exists as a string the server
could store.

The Flight payload is still written into the document, at the end, in the same
`<script type="application/json">` data block. It is complete only when the render is, and the
browser needs it to hydrate rather than to paint — hydration cannot start until the document has
been parsed, which is after the last byte either way.

### Combinations Ruvyxa refuses

Each of these would build cleanly and then do nothing, so discovery fails instead (`RUV1011`):

| Combination                                    | Why                                                                                 |
| ---------------------------------------------- | ----------------------------------------------------------------------------------- |
| `'use client'` page + `serverComponents`       | a page that runs entirely in the browser has no server half to render               |
| `export const ppr = true` + `serverComponents` | partial pre-rendering streams a shell through an entry this pipeline does not build |
| an intercepting route + `serverComponents`     | interception is matched from a client route registry this pipeline does not publish |

Client-side navigation works in both directions. Entering a server-components route fetches its
payload from `/__ruvyxa/rsc` and renders it in place — no document load, and the page underneath is
replaced the way any other route change replaces it. That endpoint is a _render_: it carries the
visitor's cookies exactly as a full request would, so its response is never cacheable, and it
requires a same-origin header a cross-origin page cannot set without a preflight.

### Deploying

A pre-rendered server-components route deploys anywhere: its payload is already inside the HTML file
the adapter copies.

A route that still needs a server at request time — `ssr`, `isr`, or
`export const dynamic = 'force-dynamic'` — deploys to any adapter that runs one. The build compiles
that route's `react-server` graph and its SSR registry into the function bundle, and the generated
route module renders through the same server-components pipeline `ruvyxa start` uses: the document
carries its Flight payload and hydrates. `/__ruvyxa/rsc` answers on those targets too, so a soft
navigation into the route fetches a payload rather than reloading the document.

The one target that cannot is a static one. A published site has no server left to run the Flight
pass, so a dynamic server-components route on `--adapter static` is refused with `RUV2202`, naming
the route and the strategy — the same diagnostic any unsupported strategy gets. Let it pre-render,
or choose an adapter that runs a server.

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

### Intercepting routes

`(.)`, `(..)`, `(..)(..)`, and `(...)` mark a folder as an **overlay** on a route that already
exists. A soft navigation to the URL it names renders it into a parallel-route slot while the page
underneath stays mounted; a hard load of the same URL renders the ordinary page.

```text
app/gallery/
├── @modal/
│   ├── (.)photo/page.tsx   ← shown over /gallery when the router navigates to /gallery/photo
│   └── default.tsx         ← shown the rest of the time
├── layout.tsx              ← receives `modal` alongside `children`
├── page.tsx
└── photo/page.tsx          ← what /gallery/photo renders on its own
```

The marker says which level the segment after it belongs to, counted in **URL** levels — a route
group or a slot folder contributes none. For `app/gallery/@modal/(.)photo`, `(.)` means the level
`app/gallery`, so the target is `/gallery/photo`. `(..)` climbs one level, `(..)(..)` two, and
`(...)` starts from the app root.

Three rules make this predictable rather than magic:

- **The real route must exist.** An interception is an overlay, so a reload, a shared link, or a new
  tab still has to render something. A marker whose target no page answers fails the build with
  **RUV1006**.
- **The folder must live inside an `@name` slot.** That is the thing an overlay replaces; outside
  one there is nowhere to put it, and the build fails with **RUV1005**.
- **Only a soft navigation intercepts.** The overlay ships inside the bundle of the page you are
  standing on, which is what lets it open with no request at all — and also why arriving from
  anywhere else shows the real page.

`router.back()` closes an overlay: the interception pushed one history entry, so popping it returns
the URL to the page still mounted underneath.

While an overlay is open, `usePathname()` **inside the route tree** still reports the mounted page —
that page is what is mounted, and `template.tsx` is keyed on it, so reporting the overlay's URL
would remount the very page the overlay sits on. The overlay component receives the intercepted URL
and its parameters as its own `requestPath` and `params` props, and the router snapshot (what a
component outside the tree sees) follows the address bar.

### A complete route-state boundary### A complete route-state boundary

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
