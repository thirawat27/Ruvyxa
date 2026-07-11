# Getting Started

## Create a New Project

The fastest way to start is with the scaffolding tool:

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npx ruvyxa dev
```

Open [http://localhost:3000](http://localhost:3000) to see your app.

> `pnpm`, `yarn`, and `bun` also work.

---

## Manual Setup

Install the framework package into any existing project:

```bash
npm install ruvyxa react react-dom
```

Add scripts to your `package.json`:

```json
{
  "scripts": {
    "dev": "ruvyxa dev",
    "build": "ruvyxa build",
    "start": "ruvyxa start",
    "check": "ruvyxa check",
    "typecheck": "tsc --noEmit"
  }
}
```

Create a config file:

```ts
// ruvyxa.config.ts
import { defineConfig } from 'ruvyxa/config'

export default defineConfig({
  appDir: 'app',
  outDir: '.ruvyxa',
  server: { port: 3000, host: 'localhost' },
  build: {
    minify: true,
    sourcemap: false,
    treeShaking: true,
    splitStrategy: 'route',
    parallelism: 4,
  },
  cache: {
    routeManifest: true,
    css: true,
  },
  debug: { overlay: true, traces: true },
  images: {
    optimize: true,
    formats: ['avif', 'webp'],
    quality: 80,
  },
})
```

---

## Your First Page

Create `app/page.tsx`:

```tsx
export default function Home() {
  return (
    <main>
      <h1>Hello Ruvyxa</h1>
      <p>Full-stack TypeScript, powered by Rust.</p>
    </main>
  )
}
```

Every `page.tsx` is server-rendered by default. No client-side JavaScript ships unless you add a
hydration bundle.

---

## Add a Layout

Create `app/layout.tsx` to wrap all pages:

```tsx
import './globals.css'

export const meta = {
  title: 'My App',
  description: 'Built with Ruvyxa.',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
```

Layouts nest automatically. A layout in `app/blog/layout.tsx` wraps all pages under `/blog/*`.

---

## Dynamic Routes

Use bracket notation for dynamic segments:

```tsx
// app/blog/[slug]/page.tsx
export default function BlogPost({ params }: { params: { slug: string } }) {
  return <h1>Post: {params.slug}</h1>
}
```

The `params` object is injected during SSR with the matched URL segments.

---

## API Routes

Create `app/api/health/route.ts`:

```ts
export function GET() {
  return Response.json({ ok: true })
}
```

API routes support `GET`, `POST`, `PUT`, `PATCH`, and `DELETE` handlers.

---

## Server Actions

Create `app/todos/action.ts` beside your page:

```ts
import { action } from 'ruvyxa/server'

export const createTodo = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== 'object' || !('title' in value))
        throw new Error('Title is required')
      return { title: String(value.title).trim() }
    },
  })
  .handler(async ({ input, invalidate }) => {
    invalidate('todos')
    return { title: input.title, completed: false }
  })
```

Call it from a form:

```tsx
export default function Todos() {
  return (
    <form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
      <input name="title" placeholder="New todo" />
      <button type="submit">Add</button>
    </form>
  )
}
```

---

## Data Loading

Co-locate server-side data fetching with your pages:

```ts
// app/blog/[slug]/server.ts
import { loader } from 'ruvyxa/server'

export const getPost = loader(async ({ params, cache }) => {
  return cache(`post:${params.slug}`)
    .ttl('5m')
    .get(async () => db.posts.findBySlug(params.slug))
})
```

Loaders run on the server only, with access to all environment variables and databases.

---

## Styling

Stylesheets are dependency-driven: import a `.css` file from a page, layout, or any local module
reachable from `app/`. The stylesheet itself can live anywhere inside the project:

```tsx
// app/layout.tsx
import '../styles/site.css'
```

Use `css.entries` for global files or directories that are not imported by application code:

```ts
export default defineConfig({
  css: { entries: ['styles/theme.css', 'vendor/styles'] },
})
```

Runtime CSS-in-JS works through React `style` objects and `<style>` elements. Libraries that require
a library-specific compiler or SSR extraction step can integrate through Ruvyxa transform plugins.

### Tailwind CSS

Tailwind entry files can also live outside `app/`. Import one from application code or list it in
`css.entries`:

```css
@import 'tailwindcss';
@source "../app";
@source "../components";
```

Install the Tailwind dependencies:

```bash
npm install tailwindcss @tailwindcss/cli
```

Ruvyxa detects the `@import "tailwindcss"` directive, runs the Tailwind CLI, and injects compiled
CSS into your pages automatically.

---

## Environment Variables

Create `.env.example` to document required keys:

```env
# Public — exposed to browser code
RUVYXA_PUBLIC_APP_NAME=My App

# Private — server-only
DATABASE_URL=postgres://localhost:5432/mydb
```

Rules:

- `RUVYXA_PUBLIC_*` variables are available everywhere.
- All other variables are server-only (SSR, API routes, actions, loaders).
- `ruvyxa check` catches private env usage in client-reachable code.

---

## Build for Production

```bash
npx ruvyxa build
npx ruvyxa start
```

The build validates your app, bundles client-side code with tree-shaking and minification, and emits
everything to `.ruvyxa/`. SSG/ISR/PPR/CSR routes are pre-rendered at build time.

Set `build.emitChunkManifest: true` when deployment tooling needs `client/chunk-manifest.json`.

---

## Validate Before Deploy

```bash
npx ruvyxa check
```

Runs:

- TypeScript type checking (when `tsconfig.json` is present)
- Production build validation
- Dev/prod route parity
- Runtime smoke rendering for every page route

Fix all diagnostics before deploying.

---

## Next Steps

- [File Routing](routing.md) — dynamic segments, catch-all routes, route groups
- [Data Loading](data.md) — loaders, caching, and server-side data patterns
- [Server Actions](actions.md) — mutations, validation, and security
- [Deployment](deployment.md) — adapters for Node, Vercel, Cloudflare, and more
- [Debugging](debugging.md) — diagnostics, tracing, and the doctor command
