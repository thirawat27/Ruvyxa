<p align="center">
  <img src="examples/basic-app/public/ruvyxa.png" alt="Ruvyxa" width="120" height="120" />
</p>

<h1 align="center">Ruvyxa</h1>

<p align="center">
  The Rust-powered full-stack React framework.<br/>
  File routing. Server-side rendering. Server actions. Production builds. One CLI.
</p>

<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa/actions"><img src="https://img.shields.io/github/actions/workflow/status/thirawat27/ruvyxa/ci.yml?branch=main&style=flat-square" alt="CI" /></a>
  <a href="https://www.npmjs.com/package/ruvyxa"><img src="https://img.shields.io/npm/v/ruvyxa?style=flat-square&color=blue" alt="npm" /></a>
  <a href="https://github.com/thirawat27/ruvyxa/blob/main/LICENSE"><img src="https://img.shields.io/github/license/thirawat27/ruvyxa?style=flat-square" alt="License" /></a>
</p>

---

## Why Ruvyxa

- **Fast by default** — Rust CLI handles route discovery, validation, and dev serving. No JavaScript toolchain startup tax.
- **Honest builds** — `ruvyxa analyze` catches server/client boundary leaks _before_ production output is emitted.
- **SSR-first React** — Pages render on the server through ReactDOMServer. Hydration bundles are route-level, tree-shaken, and BLAKE3-hashed.
- **Server actions** — Typed, validated mutations with origin checks, rate limiting, and Fetch Metadata guards built in.
- **One command, both modes** — `ruvyxa dev` and `ruvyxa start` share route semantics. What works in dev works in production.
- **Deploy anywhere** — First-party adapters for Node, Vercel, Cloudflare Workers, Netlify, Bun, and static export.

---

## Quick Start

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npx ruvyxa dev
```

Open [http://localhost:3000](http://localhost:3000).

### From source (monorepo)

```bash
pnpm install
cargo run -p ruvyxa_cli -- dev --root examples/basic-app
```

---

## Project Structure

```
my-app/
├── app/
│   ├── layout.tsx          # Root layout
│   ├── page.tsx            # Home page  →  /
│   ├── about/page.tsx      # Static page → /about
│   ├── blog/[slug]/
│   │   ├── page.tsx        # Dynamic page → /blog/:slug
│   │   └── server.ts       # Server-side loader
│   ├── todos/
│   │   ├── page.tsx        # Page with actions
│   │   └── action.ts       # Server action
│   └── api/health/route.ts # API route → /api/health
├── public/                 # Static assets
├── ruvyxa.config.ts        # Framework config
└── package.json
```

---

## Features

### File Routing

Routes are discovered from the `app/` directory. No manual registration required.

| File | Route |
|------|-------|
| `app/page.tsx` | `/` |
| `app/about/page.tsx` | `/about` |
| `app/blog/[slug]/page.tsx` | `/blog/:slug` |
| `app/docs/[...path]/page.tsx` | `/docs/*path` |
| `app/(marketing)/pricing/page.tsx` | `/pricing` |
| `app/api/health/route.ts` | `/api/health` |

### Server-Side Rendering

Every `page.tsx` is rendered on the server by default. Dynamic params are injected automatically:

```tsx
export default function BlogPost({ params }: { params: { slug: string } }) {
  return <h1>{params.slug}</h1>
}
```

### Data Loading

Keep data access server-side with co-located loaders:

```ts
// app/blog/[slug]/server.ts
import { loader } from "ruvyxa/server"

export const getPost = loader(async ({ params, cache }) => {
  return cache(`post:${params.slug}`).ttl("5m").get(async () => {
    return db.posts.findBySlug(params.slug)
  })
})
```

### Server Actions

Type-safe mutations with built-in validation:

```ts
// app/todos/action.ts
import { action } from "ruvyxa/server"

export const createTodo = action
  .input({ parse: (value: any) => ({ title: String(value.title).trim() }) })
  .handler(async ({ input, invalidate }) => {
    invalidate("todos")
    return { title: input.title, completed: false }
  })
```

Actions are secured by default: origin validation, Fetch Metadata checks, content-type guards, 64 KB body limit, and per-client rate limiting.

### Styling

Tailwind CSS v4 works out of the box:

```css
/* app/global.css */
@import "tailwindcss";

@source "../app";
@source "../components";
```

### Environment Variables

Server-side code has access to all env vars. Browser-reachable code is restricted to `RUVYXA_PUBLIC_*` prefixed variables. The boundary is enforced at build time.

---

## CLI Commands

| Command | Description |
|---------|-------------|
| `ruvyxa dev` | Start the development server with HMR |
| `ruvyxa build` | Create an optimized production build |
| `ruvyxa start` | Serve the production build |
| `ruvyxa routes` | List all discovered routes |
| `ruvyxa analyze` | Validate server/client boundaries |
| `ruvyxa doctor` | Check project health and dependencies |
| `ruvyxa trace <path>` | Inspect route matching for a URL |
| `ruvyxa bench` | Benchmark route discovery, validation, and builds |
| `ruvyxa test:parity` | Verify dev/production route consistency |
| `ruvyxa clean` | Remove `.ruvyxa` build output |

---

## Configuration

```ts
// ruvyxa.config.ts
import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  appDir: "app",
  outDir: ".ruvyxa",
  runtime: "node",
  react: true,
  server: {
    port: 3000,
    host: "localhost",
  },
})
```

---

## Deployment

Ruvyxa builds to a self-contained `.ruvyxa/` directory. Deploy with first-party adapters:

```ts
import { nodeAdapter } from "@ruvyxa/adapter-node"
import { vercelAdapter } from "@ruvyxa/adapter-vercel"
import { cloudflareAdapter } from "@ruvyxa/adapter-cloudflare"
import { netlifyAdapter } from "@ruvyxa/adapter-netlify"
import { bunAdapter } from "@ruvyxa/adapter-bun"
import { staticAdapter } from "@ruvyxa/adapter-static"
```

See [docs/deployment.md](docs/deployment.md) for platform-specific guides.

---

## Build Output

```
.ruvyxa/
├── server/       # Production route source
├── client/       # BLAKE3-hashed hydration bundles
├── assets/       # Static assets from public/
├── manifest.json # Route manifest
└── build.json    # Build metadata and security config
```

---

## Packages

| Package | Description |
|---------|-------------|
| [`ruvyxa`](packages/ruvyxa) | CLI + runtime — the main framework package |
| [`create-ruvyxa`](packages/create-ruvyxa) | Project scaffolding tool |
| [`@ruvyxa/core`](packages/@ruvyxa/core) | Shared primitives: config, loaders, actions, adapters |
| [`@ruvyxa/react`](packages/@ruvyxa/react) | React SSR and hydration integration |
| [`@ruvyxa/adapter-node`](packages/@ruvyxa/adapter-node) | Node.js deployment adapter |
| [`@ruvyxa/adapter-vercel`](packages/@ruvyxa/adapter-vercel) | Vercel deployment adapter |
| [`@ruvyxa/adapter-cloudflare`](packages/@ruvyxa/adapter-cloudflare) | Cloudflare Workers adapter |
| [`@ruvyxa/adapter-netlify`](packages/@ruvyxa/adapter-netlify) | Netlify deployment adapter |
| [`@ruvyxa/adapter-bun`](packages/@ruvyxa/adapter-bun) | Bun runtime adapter |
| [`@ruvyxa/adapter-static`](packages/@ruvyxa/adapter-static) | Static site export adapter |

---

## Documentation

- [Getting Started](docs/getting-started.md)
- [File Routing](docs/routing.md)
- [Data Loading](docs/data.md)
- [Server Actions](docs/actions.md)
- [Deployment](docs/deployment.md)
- [Debugging & Diagnostics](docs/debugging.md)
- [Performance](docs/performance.md)
- [Dev/Prod Parity](docs/parity.md)
- [Production Readiness](docs/production-readiness.md)
- [Publishing](docs/publishing.md)

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, conventions, and PR guidelines.

---

## License

[MIT](LICENSE)
