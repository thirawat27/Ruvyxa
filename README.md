<p align="center">
  <img src="https://i.postimg.cc/0yGQbz2h/ruvyxa.png" alt="Ruvyxa" width="240" height="240" />
</p>

<h1 align="center">Ruvyxa</h1>

<p align="center">
  Ruvyxa is a production-minded web framework built around clarity, speed, and control.<br/>
  It keeps routing, server logic, validation, builds, and runtime output in one predictable workflow.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="License" />
  <img src="https://img.shields.io/badge/node-%3E%3D22-blue?style=flat-square" alt="Node 22+" />
  <img src="https://img.shields.io/badge/rust-1.96%2B-orange?style=flat-square" alt="Rust 1.96+" />
  <img src="https://img.shields.io/badge/pnpm-10%2B-yellow?style=flat-square" alt="pnpm 10+" />
  <img src="https://img.shields.io/badge/TypeScript-6.0-blue?style=flat-square" alt="TypeScript 6.0" />
</p>

---

## Why Ruvyxa

- **Clean starter** — new apps start with the same small surface you expect from Next.js-style app routers.
- **Fast core** — Rust handles routing, validation, production builds, and the dev server. A persistent Node worker pool eliminates per-request subprocess overhead.
- **Radix-tree routing** — O(path-depth) route resolution regardless of the number of registered routes.
- **SSR** — React pages render on the server via a persistent Node worker pool with layout nesting and route-level hydration bundles.
- **FIFO render cache** — pages and client bundles are cached in-memory (capacity 1024 dev / 512 prod, TTL 5 min dev / 30 min prod), invalidated automatically on file change.
- **Native Rust bundler** — TypeScript/JSX compilation, module resolution, tree-shaking, minification, and source map generation — all in a zero-dependency Rust binary.
- **Gzip + Brotli compression** — all responses compressed automatically via tower-http middleware.
- **Tower-based middleware** — composable CORS, timing, logging, and custom headers via `ruvyxa.config.ts`.
- **Wasm plugin runtime** — sandboxed WebAssembly plugins powered by Wasmtime with hot-reload, configurable permissions, and execution limits.
- **Parallel production bundling** — page client bundles are emitted concurrently and written back in deterministic route order.
- **Honest checks** — `ruvyxa check` runs type checking, build validation, dev/prod parity, and page smoke rendering before deploy.
- **SSR-first React** — pages render on the server, with route-level client bundles for hydration.
- **Secure server actions** — validation hooks, origin checks, Fetch Metadata guards, a 64 KB body limit, and rate limiting are built in.
- **Dev/prod parity** — `dev` and `start` share routing, rendering, static asset, and security-header semantics.
- **ETag / 304 support** — static assets include blake3-based ETags for efficient browser caching.
- **Async I/O** — file serving uses `tokio::fs` to avoid blocking the async runtime under concurrent load.

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
│   ├── global.css
│   ├── layout.tsx
│   └── page.tsx
├── public/
│   └── ruvyxa.png
├── package.json
├── ruvyxa.config.ts
└── tsconfig.json
```

For a fuller integration app with dynamic routes, API routes, loaders, and server actions, see [examples/kitchen-sink](examples/kitchen-sink).

---

## From Source

```bash
pnpm install
cargo run -p ruvyxa_cli -- dev --root examples/kitchen-sink
```

Build all packages:

```bash
cargo test --workspace
pnpm -r build
pnpm -r test
```

Standalone JavaScript and TypeScript tests live under `tests/` and are routed by each package's `test` script. See [Testing](docs/testing.md) for the layout.

---

## App Router

Routes are discovered from `app/`:

| File | Route |
|------|-------|
| `app/page.tsx` | `/` |
| `app/about/page.tsx` | `/about` |
| `app/blog/[slug]/page.tsx` | `/blog/:slug` |
| `app/docs/[...path]/page.tsx` | `/docs/*path` |
| `app/shop/[[...path]]/page.tsx` | `/shop/*path?` |
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

The `cache()` utility provides real in-memory TTL caching with human-readable durations (`"30s"`, `"5m"`, `"1h"`, `"1d"`).

---

## Middleware

Ruvyxa ships a tower-based middleware system configurable via `ruvyxa.config.ts`:

```ts
import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  middleware: {
    builtin: {
      timing: true,           // X-Response-Time header
      logging: true,          // structured request logging
      cors: {
        origins: ["https://myapp.com"],
        methods: ["GET", "POST", "PUT", "DELETE"],
        credentials: true,
        maxAge: 86400,
      },
      headers: {
        "X-Powered-By": "Ruvyxa",
      },
    },
  },
})
```

All middleware is applied as standard Tower layers, compatible with any axum/tower ecosystem middleware.
Server actions additionally include built-in rate limiting, origin checks, and Fetch Metadata guards.

---

## Wasm Plugins

Sandboxed WebAssembly plugins run in isolated Wasmtime instances with configurable security:

```ts
export default defineConfig({
  middleware: {
    plugins: [
      {
        name: "auth-guard",
        path: "plugins/auth-guard.wasm",
        phase: "request",        // "request" or "response"
        hotReload: true,         // reload on file change
        routes: ["/api/*"],      // route filter
        permissions: {
          env: ["AUTH_SECRET"],   // allowed env vars
          fsRead: [],            // no filesystem access
          net: [],               // no network access
          timeoutMs: 5000,       // execution limit
          maxMemoryBytes: 67108864, // 64MB memory limit
        },
      },
    ],
  },
})
```

**Security model:**
- Each plugin runs in its own Wasmtime `Store` with fuel-based execution limits
- No filesystem, network, or environment access unless explicitly granted
- Memory-bounded execution prevents resource exhaustion
- Hot-reload on `.wasm` file change without server restart

---

## Configuration

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
    treeShaking: true,
    splitStrategy: "route",
    jsxRuntime: "classic",
    esTarget: "es2022",
    parallelism: 4,
    emitChunkManifest: false,
  },
  cache: {
    routeManifest: true,
    css: true,
  },
  security: {
    actionBodyLimitBytes: 65536,
    sameOriginActions: true,
    fetchMetadataActions: true,
    securityHeaders: true,
  },
  middleware: {
    builtin: {
      timing: true,
      logging: true,
      cors: {
        origins: ["http://localhost:5173"],
        methods: ["GET", "POST", "PUT", "DELETE"],
        credentials: true,
      },
    },
    plugins: [
      {
        name: "auth-guard",
        path: "plugins/auth.wasm",
        phase: "request",
        hotReload: true,
        routes: ["/api/*"],
        permissions: {
          env: ["AUTH_SECRET"],
          timeoutMs: 5000,
          maxMemoryBytes: 67108864,
        },
      },
    ],
  },
})
```

---

## CLI

| Command | Purpose |
|---------|---------|
| `ruvyxa dev` | Start the development server with HMR |
| `ruvyxa build` | Validate and emit `.ruvyxa/` production output (`--target node|edge|static`) |
| `ruvyxa check` | Run app-level production readiness checks |
| `ruvyxa start` | Serve production output with the same runtime semantics as dev |
| `ruvyxa preview` | Alias-style production preview command |
| `ruvyxa routes` | Show discovered page and API routes |
| `ruvyxa analyze` | Print structured validation JSON for boundary and route diagnostics |
| `ruvyxa doctor` | Check dependencies, route counts, environment docs, and native CLI status |
| `ruvyxa trace <path>` | Print route matching details for a URL |
| `ruvyxa bench` | Benchmark discovery, validation, and builds |
| `ruvyxa test:parity` | Compare dev/prod routes and smoke-render page routes |
| `ruvyxa clean` | Remove `.ruvyxa/` |

---

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│                     ruvyxa (npm package)                     │
│  CLI launcher → native Rust binary (ruvyxa_cli)             │
│  Runtime: worker-pool.mjs (persistent Node IPC)             │
└─────────────────┬───────────────────────────────────────────┘
                  │
┌─────────────────┴───────────────────────────────────────────┐
│                   Rust Workspace (crates/)                   │
├─────────────────────────────────────────────────────────────┤
│ ruvyxa_bundler      │ native TS/JSX bundler: compiler,       │
│                     │ minifier, linker, resolver, source maps│
│ ruvyxa_cli          │ CLI commands, build orchestration      │
│ ruvyxa_dev_server   │ axum server, worker pool, radix router │
│ ruvyxa_middleware   │ tower layers, wasmtime wasm plugins    │
│ ruvyxa_graph        │ route discovery, import graph, validation │
│ ruvyxa_middleware    │ tower layers, wasmtime wasm plugins     │
│ ruvyxa_diagnostics  │ structured errors with RUV#### codes    │
└─────────────────────────────────────────────────────────────┘
```

**Performance features:**
- Persistent Node worker pool (eliminates 100-500ms/request subprocess overhead)
- Radix-trie route matching (O(depth) instead of O(n))
- FIFO render cache with TTL (sub-ms repeated page loads)
- Async file I/O via tokio::fs (no thread starvation)
- SSR via `renderToString` with layout nesting
- Gzip + Brotli compression (tower-http)
- ETag / 304 Not Modified (blake3 hashing)
- RwLock-based runtime cache (concurrent readers)

---

## Build Output

```text
.ruvyxa/
├── server/        # Production route source (copied from app/, components/, server/)
├── client/        # BLAKE3-hashed client bundles + manifest.json
├── assets/        # Copied public assets
├── manifest.json  # Route manifest with paths, layouts, module references
├── build.json     # Build metadata, security defaults, and config snapshot
└── cache/         # Bundler compile cache (preserved across builds)
```

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
- [Bundler Comparison](docs/bundler-comparison.md)
- [Dev/Prod Parity](docs/parity.md)
- [Production Readiness](docs/production-readiness.md)
- [Publishing](docs/publishing.md)

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for local setup, verification commands, and release rules.

---

## License

[MIT](LICENSE) Copyright (c) 2026 Thirawat Sinlapasomsak
