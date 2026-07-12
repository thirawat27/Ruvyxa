# Ruvyxa User Guide

Ruvyxa is a React web framework with file-system routing. Its CLI runs the development server,
validates the application graph, builds production output, and checks development/production parity.
This guide covers the public application workflow. If you are changing the framework itself, read
the [Developer Guide](developer-guide.md).

## 1. Requirements and first application

You need Node.js 22 or later and one package manager: npm, pnpm, Yarn, or Bun. A published Ruvyxa
application does not require a Rust toolchain.

Create and run a project:

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run dev
```

Open `http://localhost:3000`. The generated project intentionally starts small:

```text
my-app/
├── app/
│   ├── globals.css
│   ├── layout.tsx
│   └── page.tsx
├── public/
├── .gitignore
├── package.json
├── ruvyxa.config.ts
└── tsconfig.json
```

The starter ignores `node_modules/`, `.ruvyxa/`, `dist/`, logs, and `.env` files. Do not commit
credentials. Commit an `.env.example` file only when it lists required variable names without real
values.

## 2. Application structure and the first page

Ruvyxa discovers routes under `app/`.

- `app/layout.tsx` wraps every page rendered below it.
- `app/page.tsx` handles the root URL, `/`.
- A nested `page.tsx` creates a nested URL.
- `public/` holds static files served from the site root.
- `ruvyxa.config.ts` controls server, build, rendering, security, caching, and style settings.

Every page file must default-export a React component:

```tsx
// app/products/page.tsx -> /products
export default function ProductsPage() {
  return (
    <main>
      <h1>Products</h1>
    </main>
  )
}
```

Keep layout concerns in `app/layout.tsx`. A layout normally imports global CSS and returns the
document shell:

```tsx
// app/layout.tsx
import './globals.css'

export const meta = {
  title: 'My Ruvyxa App',
  description: 'A production-ready application.',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
```

## 3. Routing

Routes are derived from file names and folders, not a route configuration file.

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

### Dynamic segments

Use `[name]` for one required path segment. The parameter is available through the page props:

```tsx
// app/blog/[slug]/page.tsx
import type { PageProps } from 'ruvyxa/config'

export default function BlogPost({ params }: PageProps<{ slug: string }>) {
  return <h1>Post: {params.slug}</h1>
}
```

Use `[...path]` for one or more remaining segments, and `[[...path]]` when the catch-all segment is
optional. Route groups in parentheses organise files without appearing in the URL.

Folders starting with `_` or `@` are ignored during route discovery. Ruvyxa rejects ambiguous
structures instead of choosing a route silently. For example, do not create two routes that resolve
to the same URL, dynamic siblings such as `[id]` and `[slug]`, or a `page.*` and `route.ts` in the
same directory. Run `ruvyxa analyze` after changing route structure.

## 4. Server and client components

Pages are server-rendered by default. Add the `'use client'` directive only to a module that needs
browser APIs, React state/effects, or event handlers:

```tsx
'use client'

import { useState } from 'react'

export default function Counter() {
  const [count, setCount] = useState(0)
  return <button onClick={() => setCount((value) => value + 1)}>{count}</button>
}
```

Keep private code out of the client graph. Put database access and secrets in a server-only module
and mark it clearly:

```ts
// server/database.ts
import 'server-only'

export const databaseUrl = process.env.DATABASE_URL
```

The validator reports imports of `server-only`, `client-only`, modules under `server/`, and private
environment variables when they cross the wrong boundary. Do not work around those diagnostics by
exposing a secret to the browser.

## 5. API routes

Create `route.ts` and export named HTTP handlers. Handlers receive a standard `Request` and return a
standard `Response`:

```ts
// app/api/health/route.ts
export function GET() {
  return Response.json({ ok: true })
}

export async function POST({ request }: { request: Request }) {
  const body = await request.json()
  return Response.json({ received: body }, { status: 201 })
}
```

Keep input validation, authentication, and error handling close to the handler. API request bodies
are limited to 10 MiB by default. Change the limit only when the endpoint needs it, and retain a
sensible upper bound:

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'

export default config({
  security: {
    apiLimit: 20 * 1024 * 1024,
  },
})
```

## 6. Data loading and cache

Use `loader` for server-side data loading and `cache` for bounded in-memory TTL or
stale-while-revalidate caching. A loader is an explicit helper: call it from a server page or
server-only module.

```ts
// app/products/server.ts
import { cache, loader } from 'ruvyxa/server'

export const getProducts = loader(async () => {
  return cache('products:list')
    .ttl('5m')
    .swr('1m')
    .get(() => database.products.findMany())
})
```

```tsx
// app/products/page.tsx
import { getProducts } from './server'

export default async function ProductsPage() {
  const products = await getProducts()
  return <pre>{JSON.stringify(products, null, 2)}</pre>
}
```

`ttl()` accepts values such as `30s`, `5m`, `1h`, and `1d`. Call `invalidateCache(key)` or the
action context's `invalidate(key)` after a mutation. Cache keys should identify the resource and its
scope, such as `product:123` or `products:category:books`.

## 7. Server actions

Place mutations in an `action.ts` file next to the route that owns them. Parse and validate
untrusted values before performing the mutation:

```ts
// app/todos/action.ts
import { action } from 'ruvyxa/server'

export const createTodo = action
  .input({
    parse(value: unknown) {
      const title =
        typeof value === 'object' && value && 'title' in value ? String(value.title).trim() : ''

      if (!title) throw new Error('Title is required')
      return { title }
    },
  })
  .handler(async ({ input, invalidate }) => {
    const todo = await database.todos.create(input)
    invalidate('todos')
    return todo
  })
```

A progressively enhanced HTML form can submit to the built-in action endpoint:

```tsx
<form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
  <label>
    Title
    <input name="title" required />
  </label>
  <button type="submit">Create</button>
</form>
```

The endpoint accepts JSON and URL-encoded form data. Its defaults are a 1 MiB body limit,
same-origin protection, Fetch Metadata protection, and a limit of 600 requests per client/action per
60 seconds. Configure these values under `security` only after understanding the impact on abuse
protection.

## 8. Rendering strategies

Ruvyxa chooses a rendering strategy per page. Its source detection order is significant: the first
matching rule wins.

| Declaration                               | Strategy      | Appropriate use                              |
| ----------------------------------------- | ------------- | -------------------------------------------- |
| `'use client'` at the start of the file   | CSR           | Browser-only or heavily interactive UI       |
| `export const ppr = true`                 | PPR           | Static shell with dynamic `Suspense` regions |
| `export const revalidate = 60`            | ISR           | Content refreshed after a known interval     |
| `export const getStaticParams = ...`      | SSG           | Dynamic paths known at build time            |
| Static route without dynamic data markers | SSG candidate | Stable pages and content                     |
| No earlier match                          | SSR           | Request-time data and the safe default       |

### Static dynamic pages

For a dynamic SSG route, list every path at build time:

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

### Incremental regeneration

For data that may become stale but does not need a render on every request:

```tsx
export const revalidate = 60

export default async function ProductPage() {
  return <main>Product data refreshed after at most 60 seconds.</main>
}
```

Use `ruvyxa routes` to inspect the discovered strategy and `ruvyxa analyze` to validate the
application after changing these declarations. Prefer an explicit export for content whose
deployment behaviour is important.

## 9. Markdown, MDX, images, and metadata

`page.md` and `page.mdx` are first-class route files. They support frontmatter, Markdown, MDX/JSX,
and the same development/production pipeline as TSX pages.

```mdx
---
title: Welcome
description: A page written in MDX.
---

# {frontmatter.title}

This page can contain **Markdown** and <strong>JSX</strong>.
```

Put static assets in `public/` and reference them from `/`:

```tsx
import { Image, Seo } from '@ruvyxa/react'

export default function Home() {
  return (
    <>
      <Seo title="Home" description="Welcome" canonical="https://example.com" />
      <Image src="/hero.png" alt="Product overview" width={1600} height={900} priority />
    </>
  )
}
```

`Image` converts local PNG/JPEG assets to WebP during a production build when image optimisation is
enabled. Remote URLs are not transformed. Supply intrinsic `width` and `height`, or use `fill`, to
avoid layout shift.

## 10. Environment variables

Browser-safe variables must start with `RUVYXA_PUBLIC_`. All other values are private and belong in
server-only modules, loaders, actions, or API routes.

```dotenv
# .env
RUVYXA_PUBLIC_APP_NAME=Storefront
RUVYXA_PUBLIC_API_URL=https://api.example.com
DATABASE_URL=postgres://private-connection-string
```

```tsx
const appName = import.meta.env.RUVYXA_PUBLIC_APP_NAME
```

Add declarations to `app/ruvyxa-env.d.ts` when TypeScript needs to know the public variables:

```ts
interface ImportMetaEnv {
  RUVYXA_PUBLIC_APP_NAME: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
```

Never rename a private variable with `RUVYXA_PUBLIC_` just to silence validation. That prefix is an
explicit decision to ship the value to the browser bundle.

## 11. Configuration reference

Use `config()` so TypeScript validates the public configuration shape:

```ts
import { config } from 'ruvyxa/config'

export default config({
  appDir: 'app',
  outDir: '.ruvyxa',
  css: { entries: ['styles/theme.css'] },
  server: { host: 'localhost', port: 3000 },
  build: {
    minify: true,
    map: false,
    treeShake: true,
    split: 'route',
    workers: 4,
    jsx: 'classic',
    target: 'es2022',
    manifest: false,
    warm: true,
  },
  render: { strategy: 'ssr', revalidate: 60 },
  cache: { routes: true, css: true, dir: '.ruvyxa/cache/bundler' },
  debug: { overlay: true, traces: false },
  image: { optimize: true, quality: 82, lossless: false, workers: 0 },
  security: {
    actionLimit: 1024 * 1024,
    apiLimit: 10 * 1024 * 1024,
    actionRateLimit: { max: 600, window: 60 },
    sameOrigin: true,
    fetchMeta: true,
    headers: true,
  },
})
```

Important constraints:

- `appDir`, `outDir`, and every `css.entries` value must be a project-relative path inside the
  project root. Absolute paths and `..` traversal are rejected.
- `build.split` accepts `single`, `route`, or `manual`.
- `build.target` accepts `es2018`, `es2019`, `es2020`, `es2022`, or `esnext`.
- `image.quality` is an integer from 1 to 100. `workers: 0` selects the available CPU count.
- Security limits must be positive when set.

You can also configure built-in middleware through `middleware.builtin` for timing, logging, CORS,
rate limiting, and response headers. Keep CORS origins explicit in production.

## 12. Commands, diagnosis, and release checks

The starter uses the standard npm binary form, `ruvyxa <command>`:

| Command                        | Purpose                                                            |
| ------------------------------ | ------------------------------------------------------------------ |
| `npm run dev`                  | Development server, file watching, and HMR                         |
| `npm run build`                | Production build in `.ruvyxa/`                                     |
| `npm run start`                | Serve an existing production build                                 |
| `npm run typecheck`            | Run `tsc --noEmit`                                                 |
| `npm run check`                | Typecheck, build, dev/prod parity, and page smoke render           |
| `npx ruvyxa routes`            | Print routes and their discovered render strategy                  |
| `npx ruvyxa analyze`           | Validate routes, imports, and server/client boundaries             |
| `npx ruvyxa doctor`            | Inspect project setup, tools, dependencies, and diagnostics        |
| `npx ruvyxa trace /blog/:slug` | Inspect one route manifest entry                                   |
| `npx ruvyxa test:parity`       | Compare development and production routes, then smoke render pages |
| `npx ruvyxa clean`             | Remove generated `.ruvyxa/` output                                 |

Use this order when a change is risky:

1. Run `analyze` after changing routes, imports, configuration, or environment use.
2. Run `typecheck` while iterating on TypeScript.
3. Run `check` before handing off or deploying an application.
4. Run `build` followed by `start` to inspect the production output locally.

## 13. Vercel and CI

The error below occurs before Ruvyxa starts:

```text
node_modules/.bin/ruvyxa: Permission denied
```

It means the installed Ruvyxa launcher was published without executable permission. Upgrade to a
Ruvyxa release that includes the executable launcher; the application's standard scripts should
remain:

```json
{
  "scripts": {
    "dev": "ruvyxa dev",
    "build": "ruvyxa build",
    "start": "ruvyxa start",
    "check": "ruvyxa check"
  }
}
```

Set Vercel's Build Command to `npm run build`. The fixed package publishes an executable npm
launcher, so Vercel can run the normal `.bin/ruvyxa` shim and the build no longer exits with
code 126. Pin Node 22 rather than using a broad `>=22` range when you need reproducible CI builds.

An adapter's `build()` function is executed while Ruvyxa loads configuration. Its serializable
`AdapterOutput` and `adapterOptions` are written to `.ruvyxa/build.json` for deployment tooling. An
adapter declaration alone does not create or publish platform functions, so still verify platform
output, routing, and the serving model for your deployment.

## 14. Learn from the demo

`examples/demo/` is an integration application containing static, dynamic, and catch-all routes; API
routes; server actions; MDX; public environment variables; external CSS; and SSR, SSG, ISR, CSR, and
PPR examples. Read its [README](../examples/demo/README.md), run the diagnostic commands, and copy a
proven pattern before adding a new feature to your own app.
