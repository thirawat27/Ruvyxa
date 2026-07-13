<p align="center">
  <img src="./assets/branding/ruvyxa.png" alt="Ruvyxa" width="240" height="240" />
</p>

<h1 align="center">Ruvyxa</h1>

<p align="center">
  <strong>R</strong>obust <strong>U</strong>niversal <strong>V</strong>alidation & <strong>Y</strong>ielding e<strong>X</strong>perience <strong>A</strong>pplication
</p>

<p align="center">
  Ruvyxa is a production-minded web framework built around clarity, speed, and control.<br/>
  It keeps routing, server logic, validation, builds, and runtime output in one predictable workflow.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
  <img src="https://img.shields.io/badge/node-%3E%3D22-blue?style=flat-square" alt="Node 22+" />
  <img src="https://img.shields.io/badge/rust-1.96%2B-orange?style=flat-square" alt="Rust 1.96+" />
  <img src="https://img.shields.io/badge/pnpm-11%2B-yellow?style=flat-square" alt="pnpm 11+" />
  <img src="https://img.shields.io/badge/TypeScript-7.0.2-blue?style=flat-square" alt="TypeScript 7.0.2" />
</p>

---

## Why Ruvyxa

- **Clean starter** — new apps start with the same small surface you expect from file-system app
  routers.
- **Fast core** — Rust handles routing, validation, production builds, and the dev server. A
  persistent Node worker pool eliminates per-request subprocess overhead.
- **Radix-tree routing** — O(path-depth) route resolution regardless of the number of registered
  routes.
- **SSR** — React pages render on the server via a persistent Node worker pool with layout nesting
  and route-level hydration bundles.
- **FIFO render cache** — SSR pages and client bundles are cached in-memory (capacity 1024 dev / 512
  prod, TTL 5 min dev / 30 min prod), invalidated automatically on file change. Configurable via
  `RUVYXA_RENDER_CACHE_SIZE`.
- **Native Rust bundler** — TypeScript/JSX/Markdown/MDX compilation, module resolution,
  tree-shaking, minification, and source map generation in one self-contained binary.
- **Built-in content routes** — `page.md` and `page.mdx` support frontmatter, heading exports, GFM,
  JSX components, expressions, SSG, and the same dev/prod pipeline as TSX.
- **Fast WebP image pipeline** — production builds replace copied PNG/JPEG assets with one cached,
  parallel-encoded WebP output plus React image primitives for low CLS.
- **SEO primitives** — typed canonical, robots, Open Graph, Twitter Card, and safe JSON-LD metadata.
- **Gzip + Brotli compression** — all responses compressed automatically via tower-http middleware.
- **Tower-based middleware** — composable CORS, timing, logging, rate limiting, and custom headers
  via `ruvyxa.config.ts`.
- **Wasm plugin runtime** — sandboxed WebAssembly request/response plugins powered by Wasmtime with
  explicit environment access, execution timeouts, and memory limits.
- **Parallel production bundling** — page client bundles are emitted concurrently via scoped threads
  and written back in deterministic route order.
- **Honest checks** — `ruvyxa check` runs type checking, build validation, dev/prod parity, and page
  smoke rendering before deploy.
- **Multiple rendering strategies** — SSR (default), SSG, ISR, CSR, and PPR — configurable per-route
  via `ruvyxa.config.ts` or inline exports.
- **SSR-first React** — pages render on the server, with route-level client bundles for hydration.
- **Secure server actions** — validation hooks, origin checks, Fetch Metadata guards, a 1 MB body
  limit, a 10 MB API body limit (`security.apiLimit`), and per-client/action rate limiting (600
  req/min default via `security.actionRateLimit`) are built in.
- **Dev/prod parity** — `dev` and `start` share routing, rendering, static asset, and
  security-header semantics.
- **ETag / 304 support** — static assets include BLAKE3-256-based ETags for efficient browser
  caching.
- **Async I/O** — file serving uses `tokio::fs` to avoid blocking the async runtime under concurrent
  load.

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
│   ├── globals.css
│   ├── layout.tsx
│   └── page.tsx
├── public/
│   └── ruvyxa.png
├── package.json
├── ruvyxa.config.ts
└── tsconfig.json
```

For a fuller integration app with dynamic routes, API routes, server actions, and all rendering
strategies, see [examples/demo](examples/demo).

---

## Documentation

| Guide                                                               | Description                                       |
| ------------------------------------------------------------------- | ------------------------------------------------- |
| [User Guide](docs/guides/index.md)                                  | Build and deploy a Ruvyxa app — EN & TH           |
| [Developer Guide](docs/developer-guide.md)                          | Develop, test, and package the framework          |
| [Bundler Modernization](docs/architecture/bundler-modernization.md) | Oxc integration boundary and pipeline design      |
| [Production Readiness](docs/architecture/production-readiness.md)   | Framework assessment, caching, and security audit |

---

## From Source

```bash
pnpm install
cargo run -p ruvyxa_cli -- dev --root examples/demo
```

Build and test all packages:

```bash
cargo test --workspace
pnpm -r build
pnpm -r check
pnpm -r test
```

Standalone JavaScript and TypeScript tests live under `tests/` and are routed by each package's
`test` script. See the [Developer Guide](docs/developer-guide.md) for the verification layout.

---

## App Router

Routes are discovered from `app/`. Every `page.tsx` must export a default component; every
`route.ts` exports named HTTP-method handlers.

| File                               | Route          |
| ---------------------------------- | -------------- |
| `app/page.tsx`                     | `/`            |
| `app/docs/page.md`                 | `/docs`        |
| `app/guide/page.mdx`               | `/guide`       |
| `app/about/page.tsx`               | `/about`       |
| `app/blog/[slug]/page.tsx`         | `/blog/:slug`  |
| `app/docs/[...path]/page.tsx`      | `/docs/*path`  |
| `app/shop/[[...path]]/page.tsx`    | `/shop/*path?` |
| `app/(marketing)/pricing/page.tsx` | `/pricing`     |
| `app/api/health/route.ts`          | `/api/health`  |

Route groups (parentheses), dynamic segments (brackets), and catch-all segments (`[...param]` /
`[[...param]]`) are all supported. Directories starting with `_` or `@` are ignored.

```tsx
export default function Home() {
  return <main>Hello Ruvyxa</main>
}
```

---

## Data Loading

Co-locate server-only data fetching beside routes via `server.ts`:

```ts
import { loader, cache } from 'ruvyxa/server'

export const getPost = loader(async ({ params, cache, request }) => {
  return cache(`post:${params.slug}`)
    .ttl('5m')
    .get(async () => {
      return db.posts.findBySlug(params.slug)
    })
})
```

The `cache()` utility provides real in-memory TTL caching with human-readable durations (`"30s"`,
`"5m"`, `"1h"`, `"1d"`). Call `invalidateCache(key)` or `invalidateCache()` (clear all) from server
actions.

---

## Server Actions

Co-locate validated mutations beside routes via `action.ts`:

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

**Supported content types:** `application/json`, `application/x-www-form-urlencoded`.

**Security defaults:** body size limit (1 MB), API body limit (10 MB), same-origin check, Fetch
Metadata guards, per-client/action rate limiting (600 req/min), module isolation.

---

## Middleware

Ruvyxa ships a tower-based middleware system configurable via `ruvyxa.config.ts`:

```ts
import { config } from 'ruvyxa/config'

export default config({
  middleware: {
    builtin: {
      timing: true,
      log: true,
      cors: {
        origins: ['https://myapp.com'],
        methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'],
        credentials: true,
        maxAge: 86400,
      },
      rate: {
        max: 100,
        window: 60,
        key: 'ip',
      },
      headers: {
        'X-Powered-By': 'Ruvyxa',
      },
    },
    plugins: [
      {
        name: 'auth-guard',
        path: 'plugins/auth-guard.wasm',
        phase: 'request',
        routes: ['/api/*'],
        config: { apiKeyHeader: 'X-Api-Key' },
        allow: {
          env: ['AUTH_SECRET'],
          timeout: 5000,
          memory: 67108864,
        },
      },
    ],
  },
})
```

All middleware is applied as standard Tower layers, compatible with any axum/tower ecosystem
middleware. The `wasm-plugins` feature is optional (requires `wasmtime` / `wasmtime-wasi`).

**Security model for Wasm plugins:**

- Each plugin runs in its own Wasmtime `Store` with fuel-based execution limits
- Environment access is limited to the explicit `allow.env` list
- Filesystem and network permissions are not available yet; non-empty `allow.read` or `allow.net`
  are rejected at startup rather than silently ignored
- Memory-bounded execution prevents resource exhaustion
- Request and response phase plugins run in the server request lifecycle

---

## Configuration

CSS imports are dependency-driven, so application modules may import `.css` from anywhere inside the
project. Use `css.entries` below for global files or directories that are not imported; React style
objects and `<style>` elements continue to work for runtime CSS-in-JS.

```ts
import { config } from 'ruvyxa/config'

export default config({
  appDir: 'app',
  outDir: '.ruvyxa',
  css: {
    entries: ['styles/theme.css'],
  },
  server: {
    host: 'localhost',
    port: 3000,
  },
  build: {
    minify: true,
    map: false,
    treeShake: true,
    split: 'route',
    jsx: 'classic',
    target: 'es2022',
    workers: 4,
    manifest: false,
    warm: true,
  },
  render: {
    strategy: 'ssr',
    revalidate: 60,
  },
  cache: {
    routes: true,
    css: true,
    dir: '.ruvyxa/cache/bundler',
  },
  debug: {
    overlay: true,
    traces: true,
  },
  image: {
    optimize: true,
    quality: 82,
    lossless: false,
    workers: 0,
  },
  security: {
    actionLimit: 1024 * 1024,
    apiLimit: 10 * 1024 * 1024,
    actionRateLimit: { max: 600, window: 60 },
    sameOrigin: true,
    fetchMeta: true,
    headers: true,
  },
  middleware: {
    builtin: {
      timing: true,
      log: true,
      cors: {
        origins: ['http://localhost:5173'],
        methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'],
        credentials: true,
      },
    },
    plugins: [
      {
        name: 'auth-guard',
        path: 'plugins/auth.wasm',
        phase: 'request',
        routes: ['/api/*'],
        config: {},
        allow: {
          env: ['AUTH_SECRET'],
          timeout: 5000,
          memory: 67108864,
        },
      },
    ],
  },
})
```

---

## Rendering Strategies

| Strategy | Export                    | Behavior                                     |
| -------- | ------------------------- | -------------------------------------------- |
| SSR      | default                   | Rendered per request (default)               |
| SSG      | (via config or route)     | Pre-rendered at build time, served as HTML   |
| ISR      | `export const revalidate` | Stale-while-revalidate with configurable TTL |
| CSR      | `'use client'` directive  | Minimal shell, full render in browser        |
| PPR      | `export const ppr = true` | Static shell + streamed dynamic slots        |

Routes with `getStaticParams` export generate static paths at build time.

---

## CLI

| Command               | Purpose                                                                            |
| --------------------- | ---------------------------------------------------------------------------------- |
| `ruvyxa dev`          | Start the development server with HMR and file watching                            |
| `ruvyxa build`        | Build production output to `.ruvyxa/` (supports `--target node`, `edge`, `static`) |
| `ruvyxa check`        | Run app-level production readiness checks (typecheck, build, parity, smoke)        |
| `ruvyxa start`        | Serve production output with the same runtime semantics as dev                     |
| `ruvyxa preview`      | Alias for `ruvyxa start` (preview production build locally)                        |
| `ruvyxa routes`       | Print the discovered route table                                                   |
| `ruvyxa analyze`      | Validate routes, imports, and server/client boundaries (structured JSON)           |
| `ruvyxa doctor`       | Check project health, dependencies, environment, and native CLI status             |
| `ruvyxa trace <path>` | Print route matching details for a URL                                             |
| `ruvyxa bench`        | Benchmark route discovery, analysis, validation, and production builds             |
| `ruvyxa test:parity`  | Compare dev/prod routes and smoke-render page routes                               |
| `ruvyxa clean`        | Remove `.ruvyxa/` build output                                                     |

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
│ ruvyxa_cli          │ CLI commands, config loading, build    │
│                     │ orchestration, production output       │
│ ruvyxa_dev_server   │ axum server, websocket HMR, worker     │
│                     │ pool, radix router, render cache, HMR  │
│ ruvyxa_middleware    │ tower layers, wasmtime wasm plugins   │
│ ruvyxa_graph        │ route discovery, import graph, render  │
│                     │ strategy detection, validation          │
│ ruvyxa_diagnostics  │ structured errors with RUV#### codes   │
└─────────────────────────────────────────────────────────────┘
```

**Performance features:**

- Persistent Node worker pool (eliminates 100-500ms/request subprocess overhead)
- Radix-trie route matching (O(depth) instead of O(n))
- FIFO render cache with TTL (sub-ms repeated page loads)
- Async file I/O via tokio::fs (no thread starvation)
- SSR via `renderToString` with layout nesting
- Gzip + Brotli compression (tower-http)
- ETag / 304 Not Modified (BLAKE3-256 hashing)
- RwLock-based runtime cache (concurrent readers)
- Route-level client bundle splitting with tree-shaking

---

## Build Output

```text
.ruvyxa/
├── server/        # Production route source (copied from app/, components/, server/)
├── client/        # BLAKE3-hashed client bundles + manifest.json
├── assets/        # Public assets + converted WebP images and image manifest
├── prerender/     # Pre-rendered SSG/ISR/PPR/CSR HTML files + manifest.json
├── manifest.json  # Route manifest with paths, layouts, module references
└── build.json     # Build metadata, security defaults, build settings, render summary
```

---

## Packages

| Package                                                             | Description                                                                |
| ------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| [`ruvyxa`](packages/ruvyxa)                                         | CLI, runtime bridge, and public framework entrypoints                      |
| [`create-ruvyxa`](packages/create-ruvyxa)                           | Minimal app scaffolder                                                     |
| [`@ruvyxa/core`](packages/@ruvyxa/core)                             | Typed config, server APIs, cache helpers, responses, and adapter contracts |
| [`@ruvyxa/react`](packages/@ruvyxa/react)                           | React integration package (error boundary, hydration, useLoader)           |
| [`@ruvyxa/adapter-node`](packages/@ruvyxa/adapter-node)             | Node deployment adapter                                                    |
| [`@ruvyxa/adapter-vercel`](packages/@ruvyxa/adapter-vercel)         | Vercel serverless adapter                                                  |
| [`@ruvyxa/adapter-cloudflare`](packages/@ruvyxa/adapter-cloudflare) | Cloudflare edge adapter                                                    |
| [`@ruvyxa/adapter-netlify`](packages/@ruvyxa/adapter-netlify)       | Netlify functions adapter                                                  |
| [`@ruvyxa/adapter-bun`](packages/@ruvyxa/adapter-bun)               | Bun runtime adapter                                                        |
| [`@ruvyxa/adapter-static`](packages/@ruvyxa/adapter-static)         | Static output adapter                                                      |

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for local setup, verification commands, and release rules.

---

## License

[Apache 2.0](LICENSE) Copyright (c) 2026 Thirawat27
