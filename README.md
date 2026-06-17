<p align="center">
  <img src="examples/basic-app/public/ruvyxa.png" alt="Ruvyxa" width="120" height="120" />
</p>

<h1 align="center">Ruvyxa</h1>

<p align="center">
  Ruvyxa is a production-minded web framework built around clarity, speed, and control.<br/>
  It keeps routing, server logic, validation, builds, and runtime output in one predictable workflow.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="License" />
  <img src="https://img.shields.io/badge/node-%3E%3D20-blue?style=flat-square" alt="Node 22" />
  <img src="https://img.shields.io/badge/rust-1.80%2B-orange?style=flat-square" alt="Rust 1.80+" />
  <img src="https://img.shields.io/badge/pnpm-10%2B-yellow?style=flat-square" alt="pnpm 10+" />
</p>

---

## Why Ruvyxa

- **Clean starter** - new apps start with the same small surface you expect from Next.js-style app routers.
- **Fast core** - Rust handles routing, validation, production builds, and the dev server.
- **Cached runtime hot path** - server instances reuse route manifests and compiled CSS until file changes invalidate them.
- **Parallel production bundling** - page client bundles are emitted concurrently and written back in deterministic route order.
- **Honest builds** - `ruvyxa analyze` catches server/client boundary leaks before output is emitted.
- **SSR-first React** - pages render on the server, with route-level client bundles when needed.
- **Secure server actions** - validation hooks, origin checks, Fetch Metadata guards, a 64 KB body limit, and rate limiting are built in.
- **Dev/prod parity** - `dev` and `start` share routing, rendering, static asset, and security-header semantics.
- **Readable CLI output** - commands print compact summaries, aligned tables, and color only when the terminal supports it.

---

## Quick Start

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npx ruvyxa dev
```

Open [http://localhost:3000](http://localhost:3000).

`pnpm`, `yarn`, and `bun` work too. The generated app keeps the first screen focused:

```text
my-app/
├── app/
│   ├── api/health/route.ts
│   ├── global.css
│   ├── layout.tsx
│   └── page.tsx
├── public/
│   └── ruvyxa.png
├── AGENTS.md
├── CLAUDE.md
├── package.json
├── ruvyxa.config.ts
└── tsconfig.json
```

For a fuller integration app with dynamic routes, API routes, loaders, and server actions, see [examples/basic-app](examples/basic-app).

---

## From Source

```bash
pnpm install
cargo run -p ruvyxa_cli -- dev --root examples/basic-app
```

Build all packages:

```bash
cargo test --workspace
pnpm -r build
pnpm -r test
```

---

## App Router

Routes are discovered from `app/`:

| File | Route |
|------|-------|
| `app/page.tsx` | `/` |
| `app/about/page.tsx` | `/about` |
| `app/blog/[slug]/page.tsx` | `/blog/:slug` |
| `app/docs/[...path]/page.tsx` | `/docs/*path` |
| `app/(marketing)/pricing/page.tsx` | `/pricing` |
| `app/api/health/route.ts` | `/api/health` |

Every `page.tsx` must export a default component:

```tsx
export default function Home() {
  return <main>Hello Ruvyxa</main>
}
```

---

## Server APIs

Co-locate server-only data and mutations beside routes:

```ts
import { action, loader } from "ruvyxa/server"

export const getPost = loader(async ({ params, cache }) => {
  return cache(`post:${params.slug}`).ttl("5m").get(async () => {
    return db.posts.findBySlug(params.slug)
  })
})

export const createTodo = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== "object" || !("title" in value)) {
        throw new Error("Title is required")
      }

      return { title: String(value.title).trim() }
    },
  })
  .handler(async ({ input, invalidate }) => {
    invalidate("todos")
    return { title: input.title, completed: false }
  })
```

Ruvyxa checks server/client boundaries during analysis and production builds.

---

## Configuration

Most apps only need paths and server defaults:

```ts
import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  appDir: "app",
  outDir: ".ruvyxa",
  server: {
    host: "localhost",
    port: 3000,
  },
  build: {
    minify: true,
    sourcemap: false,
    splitStrategy: "route",
    parallelism: 4,
  },
  cache: {
    routeManifest: true,
    css: true,
  },
})
```

The CLI reads `ruvyxa.config.ts` for `appDir`, `outDir`, server defaults, build minification, build parallelism, and runtime cache settings. `runtime: "node"` and `react: true` are default framework behavior and are not required in starter projects.

---

## CLI

| Command | Purpose |
|---------|---------|
| `ruvyxa dev` | Start the development server with HMR |
| `ruvyxa build` | Validate and emit `.ruvyxa/` production output |
| `ruvyxa start` | Serve production output with the same runtime semantics as dev |
| `ruvyxa preview` | Alias-style production preview command |
| `ruvyxa routes` | Show discovered page and API routes |
| `ruvyxa analyze` | Print structured validation JSON and fail on diagnostics |
| `ruvyxa doctor` | Check dependencies, route counts, environment docs, and native CLI status |
| `ruvyxa trace <path>` | Print route matching details for a URL |
| `ruvyxa bench` | Benchmark discovery, validation, and builds |
| `ruvyxa test:parity` | Compare dev and production route metadata |
| `ruvyxa clean` | Remove `.ruvyxa/` |

Human-facing commands use one compact TUI style: short headings, aligned fields, status labels, and terminal color only when stdout is a TTY. JSON commands stay machine-readable.

---

## Build Output

```text
.ruvyxa/
├── server/        # Production route source
├── client/        # BLAKE3-hashed client bundles
├── assets/        # Copied public assets
├── manifest.json  # Route manifest
└── build.json     # Build metadata and security defaults
```

The client manifest records route-level bundle metadata and the parallelism used during build.

---

## Packages

| Package | Description |
|---------|-------------|
| [`ruvyxa`](packages/ruvyxa) | CLI, runtime bridge, and public framework entrypoints |
| [`create-ruvyxa`](packages/create-ruvyxa) | Minimal app scaffolder |
| [`@ruvyxa/core`](packages/@ruvyxa/core) | Typed config, server APIs, cache helpers, responses, and adapter contracts |
| [`@ruvyxa/react`](packages/@ruvyxa/react) | React integration package |
| [`@ruvyxa/adapter-node`](packages/@ruvyxa/adapter-node) | Node deployment adapter |
| [`@ruvyxa/adapter-vercel`](packages/@ruvyxa/adapter-vercel) | Vercel serverless adapter |
| [`@ruvyxa/adapter-cloudflare`](packages/@ruvyxa/adapter-cloudflare) | Cloudflare edge adapter |
| [`@ruvyxa/adapter-netlify`](packages/@ruvyxa/adapter-netlify) | Netlify functions adapter |
| [`@ruvyxa/adapter-bun`](packages/@ruvyxa/adapter-bun) | Bun runtime adapter |
| [`@ruvyxa/adapter-static`](packages/@ruvyxa/adapter-static) | Static output adapter |

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

See [CONTRIBUTING.md](CONTRIBUTING.md) for local setup, verification commands, and release rules.

---

## License

[MIT](LICENSE)
