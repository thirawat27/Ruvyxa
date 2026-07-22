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
  <img src="https://img.shields.io/badge/TypeScript-7%2B-blue?style=flat-square" alt="TypeScript 7+" />
</p>

---

## Why Ruvyxa

### Rust core

- **Ruvyxa Bundler** — TypeScript/JSX/Markdown/MDX compilation, module resolution, tree-shaking,
  Oxc-backed minification, and source map generation in one self-contained binary.
- **Radix-trie routing** — O(path-depth) route resolution regardless of the number of registered
  routes. Duplicate and ambiguous routes are rejected at graph validation time.
- **Persistent JavaScript worker pool** — eliminates 100–500 ms per-request subprocess overhead for
  SSR. Shared across requests with layout nesting and route-level hydration bundles.
- **LRU render cache** — SSR pages and client bundles cached in-memory (capacity 1024 dev / 512
  prod, TTL 5 min dev / 30 min prod), invalidated automatically on file change. Configurable via
  `RUVYXA_RENDER_CACHE_SIZE` (`0` disables it; environment values are capped at 16,384). Backed by
  `RwLock` for concurrent readers.
- **Parallel production bundling** — route graphs are prepared once with bounded concurrency, then
  emitted once with shared-route modules in deterministic route order. Lightweight route plans and
  final/shared artifacts are content-validated, with each shared dependency fingerprinted once per
  build. Plugin-free cold shared output reuses prepared modules; warm builds reuse the validated
  registry.
- **Bounded build reuse** — Node transforms, plugin-free native dependency closures, and native
  Markdown/MDX output reuse content-keyed results. Prerendering loads its asset index once and
  shares immutable CSS across the bounded worker pool.
- **Async I/O** — file serving uses `tokio::fs` to avoid blocking the async runtime under concurrent
  load.
- **Incremental bundler cache** — blake3+mtime fingerprinting recompiles only changed modules across
  dev restarts. Shared compile cache at `.ruvyxa/cache/bundler/` survives clean builds.
- **plugin pipeline** — one `definePlugin({ setup })` registry provides `resolveId`, `transform`,
  middleware, and build-complete hooks through a persistent Node/Bun subprocess. AST-based
  import/export extraction and CommonJS detection for npm dependencies.
- **Gzip + Brotli compression** — all responses compressed automatically via tower-http middleware.
- **ETag / 304 support** — static assets include BLAKE3-256-based ETags for efficient browser
  caching. Bundle names are BLAKE3-content-addressed for deterministic cache busting.

### Dev server & HMR

- **Hot Module Replacement** — style and component updates streamed over WebSocket without full-page
  reloads. CSS collection, minification, and HMR are handled natively by the dev server.
- **Debug overlay** — in-browser error overlay during development with source-mapped stack traces.
- **Dev/prod parity** — `dev` and `start` share routing, rendering, static asset, security-header,
  and compression semantics.
- **Port conflict detection** — auto-scans 100 subsequent ports with process-owner identification
  (Windows `netstat`/`tasklist`, Unix `lsof`).

### Rendering strategies

- **SSR-first React** — pages render on the server with layout nesting, route-level client bundles
  for hydration, and the persistent worker pool.
- **Five rendering strategies** — SSR (default), SSG, ISR, CSR, and PPR. Configurable per-route via
  `ruvyxa.config.ts` or inline exports (`revalidate`, `ppr`, `getStaticParams`, `'use client'`).
- **Partial Pre-rendering (PPR)** — static shell with streamed dynamic slots via React `<Suspense>`
  boundaries and `onShellReady` streaming.
- **Incremental Static Regeneration (ISR)** — stale-while-revalidate with configurable TTL.
- **`getStaticParams`** — generate static paths at build time for dynamic SSG routes.
- **Simple SSG parameters** — export `staticParams` directly for known values, or return scalar
  values from `getStaticParams` when a route has one dynamic segment. Parameter discovery supports
  opt-in, dependency-aware persistent caching.
- **CDN-ready code splitting** — route-level, shared, or vendor chunk splitting via `build.split`
  with tree-shaking applied per-split.

### File-system routing

- **App directory router** — `app/` discovers `page.tsx`, `page.md`, `page.mdx`, `route.ts`,
  `layout.tsx`, `server.ts`, and `action.ts` automatically.
- **Dynamic segments** — `[param]`, `[...catchAll]`, and `[[...optionalCatchAll]]` with full param
  access injected into loaders and page components.
- **Route groups** — `(group)` directories for logical organization without affecting the URL.
- **Parallel route slots** — `@slot/` directories for parallel-rendered route segments.
- **API routes** — named HTTP-method exports (`GET`, `POST`, `PUT`, `DELETE`, `PATCH`) in `route.ts`
  files, with binary-safe response streaming through bounded worker IPC.
- **Duplicate & ambiguous route rejection** — the graph validator catches conflicts before they
  reach production.

### Content & images

- **Built-in content routes** — `page.md` and `page.mdx` support nested YAML frontmatter, stable
  heading exports, GFM tables/tasks/references/footnotes, multiline ESM, JSX member components,
  expressions, component overrides, and SSG. Same dev/prod pipeline as TSX routes.
- **Fast WebP image pipeline** — production builds replace copied PNG/JPEG assets with cached,
  parallel-encoded WebP output for low CLS.

### CSS pipeline

- **Dependency-driven CSS imports** — application modules import `.css` from anywhere; no separate
  import manifest required.
- **CSS entries for globals** — unimported global stylesheets via `css.entries` in config.
- **SCSS/Sass built in** — import `.scss` and `.sass` files directly, including partials referenced
  with Sass `@use`, `@forward`, or `@import`.
- **CSS Modules** — `.module.css`, `.module.scss`, and `.module.sass` imports expose deterministic,
  project-scoped class maps to React components while emitting the matching collected CSS.
- **CSS-in-JS compatible** — React style objects and `<style>` elements work natively.
- **CSS caching & minification** — production builds minify collected styles with cached results.
- **Tailwind CSS auto-detection** — detects `@import "tailwindcss"` in stylesheets and invokes
  `@tailwindcss/cli` with `--minify` in production. LESS imports produce a clear diagnostic.

### Data loading & cache

- **Co-located data fetching** — server-only `server.ts` files beside routes with `loader()` and
  `cache()` utilities.
- **Real TTL caching** — human-readable durations (`"30s"`, `"5m"`, `"1h"`, `"1d"`) with
  `invalidateCache(key)` or `invalidateCache()` (clear all) from server actions. Stale-while-
  revalidate keeps responses fast during background refresh. `cacheStats()` provides runtime
  observability (`{ size, maxEntries }`).

### Server actions

- **Type-safe server actions** — `action.input()` with validation parser and `.handler()` with typed
  input and cache invalidation callback.
- **Content type support** — `application/json` and `application/x-www-form-urlencoded`.
- **Module isolation** — actions run in isolated contexts with bounded resource usage.

### React primitives

- **Error boundary** — `<RuvyxaErrorBoundary>` with typed `fallback({ error, resetError })` for
  per-route error isolation. `resetError()` clears state for retry without full-page reload.
- **Hydration** — `hydrate()` attaches React to server-rendered DOM with automatic error reporting
  and fallback rendering when hydration fails.
- **Image components** — `<Image>` (responsive, `fill`, `priority`, `loader`, `unoptimized`,
  `fetchPriority`) and `<Picture>` for art-direction with multi-source support.
- **SEO, GEO, and AEO primitives** — `<Seo>` with typed canonical, robots, Open Graph, Twitter Card,
  Article/Breadcrumb JSON-LD, plus a visible `<Answer>` primitive with citations and Question/Answer
  microdata.
- **Client loader hook** — `useRuvyxaLoader` loads client-side data and returns
  `{ data, loading, error, refetch }` with built-in race-condition handling and mount-safety checks.
  See the [client loader guide](docs/guides/en/data-loading-and-cache.md#client-side-data-loading).

### Security

- **Server/client boundary enforcement** — `server-only`, `client-only`, and `server/` imports are
  validated at build time. Private environment variables never leak to client bundles; only
  `RUVYXA_PUBLIC_`-prefixed variables are accessible on the client.
- **Server action guards** — same-origin checks, Fetch Metadata guards, 1 MB body limit
  (`security.actionLimit`), 10 MB API body limit (`security.apiLimit`), and per-client/action rate
  limiting (600 req/min default via `security.actionRateLimit`).
- **Security headers** — configurable response headers with sensible production defaults.
- **Config safety** — unknown configuration keys fail intentionally; typos never silently change
  deployment behavior.

### Middleware & plugins

- **Tower-based middleware** — composable CORS, timing, logging, rate limiting, and custom headers
  via `ruvyxa.config.ts`. Route-scoped middleware targets specific path patterns.
- **Plugin middleware** — application modules register route-scoped Fetch `Request`/`Response` hooks
  alongside build transforms and completion callbacks.
- **First-party plugin kit** — `ruvyxa/plugins` includes a Content Engine that turns native
  Markdown/MDX routes into a live content API, locale-aware search, RSS, sitemap, explicit answer
  data, and an experimental `llms.txt` index from one metadata source. AI search/training crawlers
  can be controlled independently, alongside observability, security policy, cache rules, PWA,
  OpenAPI, redirects, bundle budgets, and environment validation. Build-generated files are included
  in adapter artifacts.
- **Official state packages** — `@ruvyxa/database` provides a typed adapter facade, `@ruvyxa/auth`
  provides secure provider-driven sessions, and `@ruvyxa/realtime` connects opted-in server actions
  to the native self-hosted WebSocket transport with explicit deployment guards.

### CLI & diagnostics

- **12 verified commands** — `dev`, `build`, `check`, `start`, `preview`, `routes`, `analyze`,
  `doctor`, `clean`, `trace`, `bench`, and `test:parity`.
- **`build`** — production output supports `--target node`, `edge`, or `static`. Pre-renders SSG,
  ISR, PPR, and CSR pages at build time via parallel worker pool (`MAX_PRERENDER_PARALLELISM: 2`).
- **`check`** — type checking, production build, dev/prod route parity, and page smoke rendering in
  one command.
- **`analyze`** — structured JSON validation of routes, imports, and server/client boundaries.
- **`doctor`** — project health check covering dependencies, environment, and Ruvyxa CLI status.
- **`bench`** — benchmark route discovery, analysis, validation, and production builds.
- **`test:parity`** — compare dev/prod routes and smoke-render page routes.
- **Structured diagnostics** — `RUV####` error codes with file locations and suggested fixes. Never
  a generic build error when the framework can pinpoint the source.

### Scaffold & adapters

- **Four starters** — `npm create ruvyxa@latest` defaults to the focused `minimal` app, with `blog`,
  `crud`, and `api-backend` available through `--template`.
- **Six deployment adapters** — `@ruvyxa/adapter-node`, `adapter-vercel`, `adapter-cloudflare`,
  `adapter-netlify`, `adapter-bun`, and `adapter-static` for typed, serializable output metadata.

---

## Quick Start

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npx ruvyxa dev
```

Choose a focused starter when you want more than the minimal route:

```bash
npm create ruvyxa@latest my-blog -- --template blog
npm create ruvyxa@latest my-admin -- --template crud
npm create ruvyxa@latest my-api -- --template api-backend
```

Open [http://localhost:3000](http://localhost:3000).

`pnpm`, `yarn`, and `bun` work too. When no runtime is configured, Ruvyxa uses Node when it is
available and falls back to Bun automatically. Set `runtime: 'bun'` and run with
`RUVYXA_RUNTIME=bun` when Bun should execute SSR, API routes, actions, and build plugins. The
generated app keeps the first screen focused:

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

## Benchmarks

Measured head-to-head against the **Next.js** and **Astro** minimal starters that were the latest
releases on the measurement date (2026-07-22) — real runs, not synthetic claims. Each number is the
**median of 5 fully cold runs** (all framework caches deleted between runs). Benchmark numbers are
only valid for the exact versions measured; newer releases of any framework may differ — re-run the
harness below to refresh them.

| Metric (lower is better)             | **Ruvyxa 1.0.18** | Next.js 16.2.11 | Astro 7.1.3 |
| ------------------------------------ | ----------------: | --------------: | ----------: |
| Production build (cold)              |         **2.0 s** |          10.9 s |       5.2 s |
| Dev server → first rendered response |         **1.5 s** |           6.3 s |       9.0 s |
| Prod server start → first response   |         **1.3 s** |           2.6 s |       3.6 s |
| Client JS shipped (minimal page)     |            184 KB |          627 KB |      0 KB ¹ |

| Throughput (higher is better)        | Ruvyxa 1.0.18 |  Next.js 16.2.11 |   Astro 7.1.3 |
| ------------------------------------ | ------------: | ---------------: | ------------: |
| Requests/second (`/`, prod server) ² |         1,293 |        **2,441** |           803 |
| Latency p50 / p99                    | 19 ms / 27 ms | **9 ms / 21 ms** | 30 ms / 45 ms |

Ruvyxa's Rust-native pipeline builds **5.4× faster than Next.js** and reaches a working dev server
**4–6× sooner** than either framework, while shipping **3.4× less JavaScript** than Next.js for the
same React-hydrated page.

¹ Astro's minimal starter is a zero-JS static page by design (no React hydration), so it has no
client bundle and its `preview` server serves static files only. ² Both Ruvyxa and Next.js serve a
prerendered static route here; Next.js currently wins raw throughput on this page while Ruvyxa ships
it with a smaller client bundle and full dev/prod parity checks. Numbers are honest — we publish the
losses along with the wins.

**Methodology** — measured 2026-07-22 on Windows 11, AMD Ryzen 7 8845HS, 32 GB RAM, Node.js 22.23.1,
npm-installed release artifacts of each framework's default minimal starter (`npm create ruvyxa` /
`create-next-app` / `create astro -- --template minimal`). Build = `build` script wall time.
Dev/prod readiness = time from process spawn to first HTTP 200 on `/`. Throughput =
`autocannon -d 10 -c 25` against the production server. Cold = `.ruvyxa`/`.next`/`dist`/`.astro`/
Vite caches removed before every run. Re-run it yourself against current framework versions:
[`scripts/bench-frameworks.mjs`](scripts/bench-frameworks.mjs) — scaffold the three starters
(instructions in the file header) and run `node bench-frameworks.mjs`.

---

## Documentation

| Guide                                                  | Description                              |
| ------------------------------------------------------ | ---------------------------------------- |
| [User Guide](docs/guides/index.md)                     | Build and deploy a Ruvyxa app — EN & TH  |
| [Developer Guide](docs/developer-guide.md)             | Develop, test, and package the framework |
| [Architecture Overview](docs/architecture/overview.md) | System architecture and module reference |
| [Getting Started](docs/guides/en/getting-started.md)   | Create your first Ruvyxa app             |

---

## From Source

```bash
./setup.sh
cargo run -p ruvyxa_cli -- dev --root examples/demo
```

On Windows, use `setup.bat` instead of `./setup.sh`. The setup script installs locked dependencies,
builds workspace packages, and compiles the Ruvyxa CLI.

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

## App Directory

Routes are discovered from `app/`. Every `page.tsx` must export a default component; every
`route.ts` exports named HTTP-method handlers.

The folder name is the complete route contract: use `[slug]` for one segment, `[...path]` for a
required `string[]`, and `[[...path]]` for an optional `string[]`. Route groups (`(...)`) organize
files without adding a segment. There is no separate `:param` or `*param` route syntax to configure.
Directories starting with `_` or `@` are ignored.

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
import { config, definePlugin } from 'ruvyxa/config'

export default config({
  middleware: { builtin: { timing: true, log: true } },
  plugins: [
    definePlugin({
      name: 'auth-guard',
      setup({ addMiddleware }) {
        addMiddleware({
          routes: ['/api/*'],
          onRequest(request) {
            return request.headers.has('authorization')
              ? undefined
              : new Response('Unauthorized', { status: 401 })
          },
        })
      },
    }),
  ],
})
```

Built-in middleware stays native Tower code. Plugin middleware uses Fetch primitives in the
persistent plugin runtime; Rust validates the bridge and enforces `security.pluginLimit` for
response buffering.

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
    jsx: 'automatic',
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
    trustedProxyIps: [],
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
  },
})
```

---

## Rendering Strategies

| Strategy | Export                    | Behavior                                     |
| -------- | ------------------------- | -------------------------------------------- |
| SSR      | default                   | Rendered per request (default)               |
| SSG      | `staticParams` / config   | Pre-rendered at build time, served as HTML   |
| ISR      | `export const revalidate` | Stale-while-revalidate with configurable TTL |
| CSR      | `'use client'` directive  | Minimal shell, full render in browser        |
| PPR      | `export const ppr = true` | Static shell + streamed dynamic slots        |

Dynamic routes can export a `staticParams` array for known values or use `getStaticParams(context)`
for asynchronous discovery. `getStaticParams` may return `{ params, cache: '10m' }` to persist the
result until its TTL expires; changes to the route or imported dependencies invalidate it early. See
the [rendering guide](docs/guides/en/rendering-strategies.md) for scalar shorthand, context, and
cache examples.

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
| `ruvyxa doctor`       | Check project health, dependencies, environment, and Ruvyxa CLI status             |
| `ruvyxa trace <path>` | Print route matching details for a URL                                             |
| `ruvyxa bench`        | Benchmark route discovery, analysis, validation, and production builds             |
| `ruvyxa test:parity`  | Compare dev/prod routes and smoke-render page routes                               |
| `ruvyxa clean`        | Remove `.ruvyxa/` build output                                                     |

---

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│                     ruvyxa (npm package)                     │
│  CLI launcher → Ruvyxa CLI Rust binary (ruvyxa_cli)         │
│  Runtime: worker-pool.mjs (persistent Node IPC)             │
└─────────────────┬───────────────────────────────────────────┘
                  │
┌─────────────────┴───────────────────────────────────────────┐
│                   Rust Workspace (crates/)                   │
├─────────────────────────────────────────────────────────────┤
│ ruvyxa_bundler      │ Ruvyxa Bundler: compiler,              │
│                     │ minifier, linker, resolver, source maps│
│ ruvyxa_cli          │ CLI commands, config loading, build    │
│                     │ orchestration, production output       │
│ ruvyxa_dev_server   │ axum server, websocket HMR, worker     │
│                     │ pool, radix router, render cache, HMR  │
│ ruvyxa_middleware    │ Tower layers + plugin bridge│
│ ruvyxa_graph        │ route discovery, import graph, render  │
│                     │ strategy detection, validation          │
│ ruvyxa_diagnostics  │ structured errors with RUV#### codes   │
└─────────────────────────────────────────────────────────────┘
```

**Performance features:**

- Persistent JavaScript worker pool (eliminates 100-500ms/request subprocess overhead)
- Radix-trie route matching (O(depth) instead of O(n))
- LRU render cache with TTL (sub-ms repeated page loads)
- Bounded, binary-safe API response streaming across worker IPC
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

Hybrid adapters add platform deployment directories containing a compiled `.mjs` static route
registry. Serverless and edge handlers execute that bundle directly; raw TS/TSX source is not used
as a deployment entrypoint.

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
