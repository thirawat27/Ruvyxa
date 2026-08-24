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
  <img src="https://img.shields.io/badge/node-%3E%3D24.19-blue?style=flat-square" alt="Node 24.19+" />
  <img src="https://img.shields.io/badge/compiler-Rust-orange?style=flat-square" alt="Compiler written in Rust" />
  <img src="https://img.shields.io/badge/runtime-TypeScript-blue?style=flat-square" alt="Runtime written in TypeScript" />
  <a href="https://www.npmjs.com/package/ruvyxa"><img src="https://img.shields.io/npm/v/ruvyxa?style=flat-square" alt="npm version" /></a>
</p>

---

## Documentation

| Guide                                                                      | Description                                      |
| -------------------------------------------------------------------------- | ------------------------------------------------ |
| [Documentation Home](docs/README.md)                                       | Full manual — English and Thai editions          |
| [Introduction](docs/en/01-introduction.md)                                 | What Ruvyxa is and when to use it                |
| [Create Your First App](docs/en/02-create-your-first-app.md)               | Scaffold, run, and build a first vertical slice  |
| [Project Structure](docs/en/03-project-structure.md)                       | `app/`, `public/`, config, and generated output  |
| [Routing & Rendering](docs/en/04-routing-rendering.md)                     | File-system routes, layouts, SSR/SSG/ISR/CSR/PPR |
| [Data, Actions & API](docs/en/05-data-actions-api.md)                      | Loaders, `cache()`, server actions, API routes   |
| [UI, Navigation & Assets](docs/en/06-ui-navigation-metadata-and-assets.md) | Components, metadata, images, fonts              |
| [Configuration](docs/en/07-configuration.md)                               | `ruvyxa.config.ts` and environment variables     |
| [Plugins & Middleware](docs/en/08-plugins-middleware.md)                   | Plugin hooks and the middleware chain            |
| [Integrations](docs/en/09-integrations-auth-data-and-realtime.md)          | `@ruvyxa/auth`, `database`, `realtime`           |
| [CLI](docs/en/10-cli.md)                                                   | Scripts, scaffolding, and the build loop         |
| [Architecture](docs/en/11-architecture.md)                                 | Graph, bundler, dev server, diagnostics          |
| [Development & Testing](docs/en/12-development-testing.md)                 | Develop, test, and package the framework         |
| [Security](docs/en/13-security.md)                                         | Boundary enforcement and env safety              |
| [Observability & Performance](docs/en/14-observability-performance.md)     | Logging, timing, caching, budgets                |
| [Deploy, Run & Operate](docs/en/15-deploy-run-and-operate.md)              | Adapters and platform-specific configuration     |
| [Troubleshooting](docs/en/16-troubleshooting-upgrades.md)                  | Diagnostic catalog and upgrade notes             |
| [Public API Reference](docs/en/17-public-api-reference.md)                 | Complete `@ruvyxa/react` + `@ruvyxa/core` API    |
| [Documentation Scope](docs/en/18-documentation-scope-and-sources.md)       | What the manual covers and its sources           |
| [Release Readiness](docs/en/19-release-readiness-playbook.md)              | Pre-release verification playbook                |
| [Platform Adapter Guide](docs/en/20-platform-adapter-guide.md)             | Writing a custom deployment adapter              |
| [Practical Recipes](docs/en/21-practical-recipes.md)                       | Task-oriented end-to-end examples                |

A full Thai edition lives beside the English one under [`docs/th/`](docs/th). Repository-level
documents: [ARCHITECTURE.md](ARCHITECTURE.md), [CHANGELOG.md](CHANGELOG.md),
[CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md),
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md), and [AGENTS.md](AGENTS.md) for agent/contributor rules.

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
- **Persistent incremental module graph** — production builds persist content-verified dependency
  edges and reuse unchanged client resolution work on warm builds. The graph is namespaced by the
  evaluated config dependency hash; build hooks bypass edge reuse so plugin resolution stays
  correct. Compiled output remains content-addressed under the configured build cache.
- **Typed artifact task graph** — resolve, transform, analysis, chunk-plan, emit, source-map, and
  manifest records share explicit dependency edges and generation-scoped completion. Records are
  persisted atomically beside the build cache; corrupt or incompatible metadata rebuilds normally,
  and `RUVYXA_DISABLE_ARTIFACT_CACHE=1` provides a correctness-preserving temporary bypass.
- **Coordinated cache pressure** — compiler memory, resolver derivations, and artifact metadata use
  one soft/hard hysteresis policy (`RUVYXA_BUILD_CACHE_MEMORY_MB`, 256 MiB default). Worker caches
  apply the same policy under `RUVYXA_MEMORY_LIMIT_MB` (512 MiB default), skip pinned build keys,
  stop speculative warmups at the hard limit, and expose pressure/eviction counters.
- **plugin pipeline** — one `definePlugin({ name, register })` registry provides grouped HTTP,
  build, dev, diagnostic, and native sockets through a versioned Node/Bun/Deno subprocess. AST-based
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
- **On-demand revalidation** — `revalidatePath()` from `ruvyxa/server` queues one URL for a fresh
  render on its next request from any API route or server action. For SSR and CSR the cached
  document is dropped; for SSG, ISR, and PPR the next request also bypasses the per-rendered HTML.
  The invalidation arrives with the write that caused it, so a client following the action cannot
  outrun the cache clear. Works in `dev`, `start`, and serverless.
- **`getStaticParams`** — generate static paths at build time for dynamic SSG routes.
- **Deferred and zero-JS hydration** — route exports accept `hydrate = 'visible'`, `'idle'`, or
  `false`. Deferred routes do not preload their React bundle; one small shared loader imports it
  only when the route becomes visible or the browser is idle. `false` emits no client bundle at all.
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
  microdata. Production builds also emit standards-compliant, automatically sharded `sitemap.xml`
  and configurable `robots.txt`, while public files or exact routes can take ownership.
- **Client loader hook** — `useRuvyxaLoader` loads client-side data and returns
  `{ data, loading, error, refetch }` with built-in race-condition handling and mount-safety checks.
  See the [client loader guide](docs/en/05-data-actions-api.md).
- **Typed routes** — set `typedRoutes: true` and `dev`/`build`/`check` generate
  `.ruvyxa/types/routes.d.ts`; `<Link href>` and the imperative router are then narrowed to the
  routes the project actually has, with `route(url)` for URLs computed at runtime. Opt-in — until
  the file exists, the type collapses back to plain `string`.
- **`<Script>`** — third-party scripts with `beforeInteractive`, `afterInteractive`, and
  `lazyOnload` strategies; an external URL is fetched once per page.
- **Instrumentation** — an optional `instrumentation.ts` whose `register()` runs once per server
  process, for installing OpenTelemetry, error reporters, or metrics exporters.

### Security

- **Server/client boundary enforcement** — `server-only`, `client-only`, and `server/` imports are
  validated at build time. Private environment variables never leak to client bundles; only
  `RUVYXA_PUBLIC_`-prefixed variables are accessible on the client.
- **Server action guards** — same-origin checks, Fetch Metadata guards, 1 MB body limit
  (`security.actionLimit`), 10 MB API body limit (`security.apiLimit`), and per-client/action rate
  limiting (600 req/min default via `security.actionRateLimit`).
- **Security headers** — native, standalone, and serverless responses receive the same seven safe
  defaults while explicit application headers win. Static/Cloudflare `_headers` output carries the
  same policy; CSP and HSTS remain explicit because safe values are application-specific.
- **Config safety** — unknown configuration keys fail intentionally; typos never silently change
  deployment behavior.

### Middleware & plugins

- **Tower-based middleware** — composable CORS, timing, logging, rate limiting, and custom headers
  via `ruvyxa.config.ts`. Route-scoped middleware targets specific path patterns.
- **Plugin middleware** — application modules register route-scoped Fetch `Request`/`Response` hooks
  alongside build transforms and completion callbacks.
- **16 Built-in Plugins** — Drop-in solutions from `ruvyxa/plugins` for advanced functionality:
  Content Engine (Markdown/MDX to API + `llms.txt`), OpenAPI generator, Bundle Budget enforcer,
  PWA/Manifest, Sitemap, RSS Feeds, SEO Robots, Redirects, Security Headers, and more. All plugins
  operate within strict execution limits and are fully extensible. See the
  [Plugins & Middleware](docs/en/08-plugins-middleware.md).
- **Official state packages** — `@ruvyxa/database` provides a typed adapter facade, `@ruvyxa/auth`
  provides secure provider-driven sessions, and `@ruvyxa/realtime` connects opted-in server actions
  to the native self-hosted WebSocket transport with explicit deployment guards.

### CLI & diagnostics

- **14 verified commands** — `dev`, `build`, `check`, `start`, `preview`, `routes`, `analyze`,
  `adds`, `doctor`, `clean`, `trace`, `bench`, `test:parity`, and `plugin`.
- **`build`** — production output supports `--target node`, `bun`, `edge`, or `static`, plus
  `--adapter <name>` to run a deploy adapter without editing config and `--server-only` for API-only
  artifacts. Pre-renders SSG, ISR, PPR, and CSR pages at build time via parallel worker pool
  (`MAX_PRERENDER_PARALLELISM: 2`).
- **`check`** — type checking, production build, dev/prod route parity, and page smoke rendering in
  one command.
- **`analyze`** — human, JSON, SARIF 2.1.0, or a self-contained interactive HTML validation report
  for routes, imports, and server/client boundaries (`--format sarif --output reports/ruvyxa.sarif`;
  `--html`).
- **`adds`** — scaffold an additive, framework-native `form`, `data-table`, or `auth` flow.
- **`doctor`** — project health plus deploy-target inspection: adapter runtime, platform,
  capabilities, and every route unsupported by the selected adapter (`--json` for CI).
- **`bench`** — benchmark route discovery, analysis, validation, and production builds.
- **`test:parity`** — compare dev/prod routes and smoke-render page routes.
- **First-Class Diagnostics** — Over 60 `RUV####` error codes spanning Boundary Violations
  (RUV1000s), Server/Render Errors (RUV1100s), Build/Compilation (RUV1300s), and Official Packages
  (RUV3000s). Every diagnostic carries its own explanation and suggested fix at the point it is
  raised; see [Troubleshooting](docs/en/16-troubleshooting-upgrades.md) for the common symptoms and
  their evidence-backed fixes.

### Deploy Anywhere (11 Adapters)

- **Four starters** — `npm create ruvyxa@latest` defaults to the focused `minimal` app, with `blog`,
  `crud`, and `api` available through `--template`. Run it with no arguments on a real terminal and
  it prompts for a project name and offers an arrow-key template menu (`j`/`k` also work); the
  scaffold summary prints the actual generated file tree, colored by role.
- **11 Deployment Adapters** — Zero-config native output for Vercel, Netlify, Cloudflare, Node.js,
  Bun, Deno, Static, AWS Amplify, Firebase, Railway, and Render. See the
  [Deployment Guide](docs/en/15-deploy-run-and-operate.md) for platform-specific configurations.

---

## Requirements

Version floors are declared once, in the manifests, and this table points at them rather than
restating numbers that go stale:

| Use case                | Requirement                                                                           | Declared in                                     |
| ----------------------- | ------------------------------------------------------------------------------------- | ----------------------------------------------- |
| Building an app         | Node.js (Bun can run SSR instead). No Rust toolchain needed — the CLI ships prebuilt. | `engines.node` in each published `package.json` |
| Supported CLI platforms | `win32-x64`, `win32-arm64`, `linux-x64`, `linux-arm64`, `darwin-arm64`                | `optionalDependencies` of `ruvyxa`              |
| Building the framework  | Node.js, pnpm, and a Rust toolchain (edition 2024)                                    | `.nvmrc`, `packageManager`, `rust-version`      |

`ruvyxa doctor` prints the versions it actually resolved on your machine and says which ones are too
old, which is the answer that matters more than any number written here.

The `ruvyxa` npm package resolves one `@ruvyxa/cli-<platform>` optional dependency that carries the
prebuilt native binary, so `npm install` is all that is required to get a working `ruvyxa` command.

---

## Quick Start

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run dev
```

Choose a focused starter when you want more than the minimal route:

```bash
npm create ruvyxa@latest my-blog -- --template blog
npm create ruvyxa@latest my-admin -- --template crud
npm create ruvyxa@latest my-api -- --template api
```

Open [http://localhost:3000](http://localhost:3000).

`pnpm`, `yarn`, `bun`, and Deno tasks work too. When no runtime is configured, Ruvyxa prefers Node,
then falls back to Bun and Deno. Set `runtime: 'bun'` or `runtime: 'deno'`, or pass
`--runtime bun|deno`, when that runtime should execute config, SSR, API routes, actions, adapters,
and build plugins. Deno local tooling runs with the permissions required by trusted project config
and plugins (`-A --no-prompt`). A scaffold can be run with `deno install && deno task dev`. The
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

In practice you rarely type `ruvyxa` directly — every starter wires the CLI into `package.json`
scripts, so the day-to-day loop is plain npm/pnpm/yarn/bun scripts:

| Script                 | Runs               | When you use it                             |
| ---------------------- | ------------------ | ------------------------------------------- |
| `npm run dev`          | `ruvyxa dev`       | Local development with HMR                  |
| `npm run build`        | `ruvyxa build`     | Production build into `.ruvyxa/`            |
| `npm start`            | `ruvyxa start`     | Serve the production build                  |
| `npm run preview`      | `ruvyxa preview`   | Check the production build locally          |
| `npm run check`        | `ruvyxa check`     | Pre-deploy gate: typecheck + build + parity |
| `npm run routes`       | `ruvyxa routes`    | See what the router discovered              |
| `npm run analyze`      | `ruvyxa analyze`   | Boundary/import validation, CI-friendly     |
| `npm run doctor`       | `ruvyxa doctor`    | Environment and deploy-target health        |
| `npm run adds -- form` | `ruvyxa adds form` | Scaffold a framework-native flow            |
| `npm run trace -- /`   | `ruvyxa trace /`   | Inspect one route manifest entry            |
| `npm run clean`        | `ruvyxa clean`     | Remove `.ruvyxa/`                           |

Arguments after `npm run <script>` need the `--` separator (`npm run trace -- /blog/[slug]`); pnpm
and bun pass them through directly. `ruvyxa` has no `--version` flag — run `ruvyxa doctor`, which
prints the resolved version alongside the rest of the project report.

First time using Ruvyxa? Check out the
**[Tutorial: Build a Mini Blog](docs/en/02-create-your-first-app.md#build-one-working-vertical-slice)**
in the Getting Started guide for a step-by-step introduction to Routing, Server Components, and
Dynamic Routes.

For a fuller integration app with dynamic routes, API routes, server actions, and all rendering
strategies, see [examples/demo](examples/demo).

---

## Benchmarks

Measured on 2026-08-21 with the repository harness against minimal starters. These figures are valid
only for the exact versions, machine, and run conditions shown below; re-run the harness when
comparing newer releases.

| Metric (lower is better)             |  **Ruvyxa** | Next.js |   Astro |
| ------------------------------------ | ----------: | ------: | ------: |
| Production build (cold-cache median) | **1.131 s** | 7.667 s | 2.309 s |
| Dev server → first rendered response | **1.115 s** | 3.310 s | 5.855 s |
| Prod server start → first response   | **0.845 s** | 1.074 s | 2.813 s |
| Client JS shipped (minimal page)     |    235 KB ¹ |  568 KB |  0 KB ² |

| Throughput (higher is better)        |   **Ruvyxa** |   Next.js |     Astro |
| ------------------------------------ | -----------: | --------: | --------: |
| Requests/second (`/`, prod server) ³ |   **44,528** |     3,647 |     3,611 |
| Latency p50 / p99                    | **0 / 1 ms** | 6 / 12 ms | 6 / 10 ms |

In this run, Ruvyxa's median cold build completed **6.8× / 2.0×** faster than Next.js / Astro
respectively, dev-server readiness was **3.0× / 5.3×** faster, production-server readiness **1.3× /
3.3×** faster, and the measured production throughput was **12.2× / 12.3×** higher. These are local
results for this minimal-starter workload, not a universal performance ranking.

First-run warm-up is real and large enough to swamp a careless reading: Next.js's first cold build
measured 19.8 s against a 7.6 s median for its other two runs, and Astro's first spiked to 20.2 s
against a ~2.2 s median. The reported medians are unaffected because a median discards one outlier;
Ruvyxa's three cold builds were 1.175 s / 1.128 s / 1.131 s, which is the spread worth noting on its
own. Re-run the harness on an otherwise idle machine before reading anything into small differences,
and treat gaps under ~100 ms as noise.

¹ For the interactive React starter page. A content page can opt out entirely with
`export const hydrate = false` — its HTML then ships **0 KB** of JavaScript and no client bundle is
emitted. ² Astro's minimal starter is a zero-JS static page by design (no React hydration), so it
has no client bundle and its `preview` server serves static files only. ³ The harness runs
`autocannon` once for 10 seconds with 25 connections against the final production server of each
framework; throughput and latency are not medians.

**Methodology** — measured on Windows 11 Home, AMD Ryzen 7 8845HS, 31 GB RAM. Each framework used a
freshly scaffolded minimal starter in an isolated benchmark root with `RUNS=3` and the same
[`scripts/bench-frameworks.mjs`](scripts/bench-frameworks.mjs) harness. The harness reports timings
only, not framework versions; capture those from each starter's own lockfile at the moment you run
it, because that is the only record that will still be accurate later. Build = `build` script wall
time. Dev/prod readiness = time from process spawn to first HTTP 200 on `/`. Cold-cache runs remove
`.ruvyxa`/`.next`/`dist`/`.astro`/Vite caches before each run; dependencies remain installed during
the harness. The Ruvyxa column was packed from this repository with `pnpm pack:smoke` and installed
from local tarballs together with the matching Windows native CLI package, rather than installed
from the npm registry; the Ruvyxa and Next starters ran the same React. The machine was not
otherwise idle during this run, so the absolute readiness and build timings run slightly high; the
three frameworks were measured back to back under the same conditions, so the comparison between
them holds. To reproduce, prepare the three isolated starters, use local tarball overrides for an
unpublished Ruvyxa release candidate, then run
`BENCH_ROOT=<benchmark-root> RUNS=3 node scripts/bench-frameworks.mjs`.

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
`test` script. See [Development & Testing](docs/en/12-development-testing.md) for the verification
layout.

The full pre-submit gate used in this repository:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
pnpm -r build && pnpm -r check && pnpm -r test
pnpm format:check
pnpm release:validate
pnpm pack:smoke
cargo run -p ruvyxa_cli -- check --root examples/demo
```

### Repository layout

```text
ruvyxa/
├── crates/            # Rust workspace (7 crates)
│   ├── ruvyxa_cli/        # commands, config loading, build orchestration
│   ├── ruvyxa_bundler/    # TS/JSX compile, resolve, link, minify, source maps
│   ├── ruvyxa_dev_server/ # axum server, HMR, worker pool, router, caches
│   ├── ruvyxa_graph/      # route discovery, validation, render strategies
│   ├── ruvyxa_middleware/ # Tower layers and the plugin bridge
│   ├── ruvyxa_diagnostics/# RUV#### structured errors
│   └── ruvyxa_tui/        # terminal layout, progress, mascot, and theme primitives
├── packages/          # npm packages (see the table below)
├── templates/         # create-ruvyxa starters + plugin scaffold
├── examples/demo/     # 23-route integration fixture
├── tests/             # Node tests, organized by package
├── docs/{en,th}/      # user-facing manual, both editions
└── scripts/           # release validation, packing, benchmarks
```

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
import { config } from 'ruvyxa/config'
import { definePlugin } from 'ruvyxa/plugin'

export default config({
  middleware: { builtin: { timing: true, log: true } },
  plugins: [
    definePlugin({
      name: 'auth-guard',
      register({ http }) {
        http.onRequest({
          match: ['/api/*'],
          handler({ request }) {
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
    variantWidths: [640, 750, 828, 1080, 1200, 1920, 2048, 3840],
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
the [rendering guide](docs/en/04-routing-rendering.md) for scalar shorthand, context, and cache
examples.

---

## CLI

Fourteen commands. Project commands accept `--root <dir>` (default `.`) and
`--runtime node|bun|deno`.

| Command                       | Purpose                                                                         | Additional flags                                                       |
| ----------------------------- | ------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| `ruvyxa dev`                  | Development server with HMR and file watching                                   | `--host`, `--port`                                                     |
| `ruvyxa build`                | Build production output to `.ruvyxa/`                                           | `--target node\|bun\|deno\|edge\|static`, `--adapter`, `--server-only` |
| `ruvyxa check`                | Production readiness gate: typecheck, build, dev/prod parity, page smoke render | —                                                                      |
| `ruvyxa start`                | Serve production output with the same runtime semantics as dev                  | `--host`, `--port`                                                     |
| `ruvyxa preview`              | Preview an existing production build locally (same server as `start`)           | `--host`, `--port`                                                     |
| `ruvyxa routes`               | Print the discovered route table                                                | `--json`                                                               |
| `ruvyxa analyze`              | Validate routes, imports, and server/client boundaries                          | `--format auto\|human\|json\|sarif\|html`, `--output`, `--html`        |
| `ruvyxa adds <flow…>`         | Scaffold a `form`, `data-table`, or `auth` flow                                 | `--force`                                                              |
| `ruvyxa doctor`               | Project health plus deploy-target inspection                                    | `--target`, `--adapter`, `--json`                                      |
| `ruvyxa trace <route>`        | Inspect one route manifest entry                                                | —                                                                      |
| `ruvyxa bench`                | Benchmark route discovery, analysis, validation, and production builds          | `--runtime`, `--samples <n>` (default 3), `--json`, `--baseline`       |
| `ruvyxa test:parity`          | Compare dev/prod routes and smoke-render page routes                            | —                                                                      |
| `ruvyxa plugin create <name>` | Scaffold a publishable plugin package                                           | `--dir`                                                                |
| `ruvyxa clean`                | Remove `.ruvyxa/` build output                                                  | —                                                                      |

`trace` takes a **route manifest path, not a request URL**: use `ruvyxa trace "/blog/[slug]"`, not
`ruvyxa trace /blog/hello`. It prints the route id, file, layout chain, server/client modules,
runtime, and render strategy as JSON. On Windows, quote paths containing `[` `]`, and note that Git
Bash rewrites a leading `/` into a Windows path — use PowerShell, `cmd`, or `MSYS_NO_PATHCONV=1`.

Command and flag spellings are normalized before parsing, so `--root=x`, `--server_only`,
`test-parity`, and an em-dashed `—root` all resolve to their canonical form. Run
`ruvyxa help <command>` for the built-in reference. There is no `--version` flag; `ruvyxa doctor`
reports the resolved version.

Use `ruvyxa bench --baseline --json` for the production baseline. Each sample copies project inputs
into an ignored temporary workspace and measures cold/warm builds, first-route rendering, and
CSS/client/server/leaf edit classes. Results use the stable `ruvyxa.build-bench` contract with a
separate `schemaVersion: 1`, plus peak resident memory and HMR reload-fallback counts. The command
refuses to publish timings unless cold and warm builds have the same semantic artifact set;
cache/timing telemetry is excluded from that comparison, while deployed code, assets, and manifests
remain part of it. The ordinary `bench --json` array is unchanged for existing consumers.

### Verified end to end

Every command in the table above was run on 2026-08-24 against [`examples/demo`](examples/demo) — 30
routes: 27 pages and 3 API routes — with a `--release` binary built from this working tree.
Scaffolding commands ran against a throwaway copy of `templates/minimal` so the fixture stayed
clean. Timings are one machine's, not a specification; re-run them rather than quoting them.

| Command         | Observed result                                                                         |
| --------------- | --------------------------------------------------------------------------------------- |
| `routes`        | 30 routes discovered (27 pages, 3 API), strategy resolved per route                     |
| `doctor`        | 0 diagnostics, adapter `ruvyxa-native`, every route supported by the target, **1.17s**  |
| `analyze`       | 0 diagnostics; 38 client modules and 4 server modules reported                          |
| `trace`         | route id, file, layout chain, runtime, and render strategy as JSON                      |
| `build`         | cold build of all 30 routes into `.ruvyxa/` in **8.15s**, 247 kB shared by every page   |
| `start`         | ready in **425ms**; `GET /`, `GET /api/health`, and `GET /blog/hello-world` all → 200   |
| `preview`       | ready and serving the same production build → 200                                       |
| `dev`           | ready in **482ms** with HMR enabled; `GET /` → 200                                      |
| `check`         | typecheck + build + parity + smoke render over 30 routes, **16.01s**, exit 0            |
| `test:parity`   | parity passed for 30 routes in **14.40s**, exit 0                                       |
| `bench`         | 6 scenarios; cold build **8.27s** against warm **657ms** — 92% of it saved by the cache |
| `adds`          | `form` wrote `page.tsx` + `action.ts`; `data-table` wrote the generic client component  |
| `plugin create` | created a publishable `ruvyxa-plugin-<name>` package with a harness test                |
| `clean`         | removed `.ruvyxa/` in **141ms**                                                         |

---

## Architecture

```text
┌───────────────────────────────────────────────────────────────┐
│                      ruvyxa (npm package)                     │
│   CLI launcher → Ruvyxa CLI Rust binary (ruvyxa_cli)          │
│   Runtime: worker-pool.mjs (persistent Node/Bun/Deno IPC)     │
└─────────────────────┬─────────────────────────────────────────┘
                      │
┌─────────────────────┴─────────────────────────────────────────┐
│                    Rust Workspace (crates/)                   │
├─────────────────────┬─────────────────────────────────────────┤
│ ruvyxa_bundler      │ compiler, minifier, linker, resolver,   │
│                     │ source maps, boundary checks            │
│ ruvyxa_cli          │ CLI commands, config loading, build     │
│                     │ orchestration, production output        │
│ ruvyxa_dev_server   │ axum server, websocket HMR, worker      │
│                     │ pool, radix router, render cache        │
│ ruvyxa_middleware   │ Tower layers + plugin bridge            │
│ ruvyxa_graph        │ route discovery, import graph, render   │
│                     │ strategy detection, validation          │
│ ruvyxa_diagnostics  │ structured errors with RUV#### codes    │
└─────────────────────┴─────────────────────────────────────────┘
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
| [`create-ruvyxa`](packages/create-ruvyxa)                           | Minimal app scaffolder (4 starters)                                        |
| [`@ruvyxa/core`](packages/@ruvyxa/core)                             | Typed config, server APIs, cache helpers, responses, and adapter contracts |
| [`@ruvyxa/react`](packages/@ruvyxa/react)                           | React components, hooks, SEO, error boundary, hydration                    |
| [`@ruvyxa/auth`](packages/@ruvyxa/auth)                             | Provider-driven authentication (GitHub, Google, Discord, magic link)       |
| [`@ruvyxa/database`](packages/@ruvyxa/database)                     | Typed database adapter facade (Prisma, DynamoDB)                           |
| [`@ruvyxa/realtime`](packages/@ruvyxa/realtime)                     | WebSocket transport for opted-in server actions                            |
| [`@ruvyxa/testing`](packages/@ruvyxa/testing)                       | Dependency-free loader, action, and cache test doubles                     |
| [`@ruvyxa/adapter-node`](packages/@ruvyxa/adapter-node)             | Node deployment adapter                                                    |
| [`@ruvyxa/adapter-vercel`](packages/@ruvyxa/adapter-vercel)         | Vercel serverless adapter                                                  |
| [`@ruvyxa/adapter-cloudflare`](packages/@ruvyxa/adapter-cloudflare) | Cloudflare edge adapter                                                    |
| [`@ruvyxa/adapter-netlify`](packages/@ruvyxa/adapter-netlify)       | Netlify functions adapter                                                  |
| [`@ruvyxa/adapter-bun`](packages/@ruvyxa/adapter-bun)               | Bun runtime adapter                                                        |
| [`@ruvyxa/adapter-deno`](packages/@ruvyxa/adapter-deno)             | Deno self-hosted runtime adapter                                           |
| [`@ruvyxa/adapter-static`](packages/@ruvyxa/adapter-static)         | Static output adapter                                                      |
| [`@ruvyxa/adapter-railway`](packages/@ruvyxa/adapter-railway)       | Railway standalone service adapter                                         |
| [`@ruvyxa/adapter-render`](packages/@ruvyxa/adapter-render)         | Render Web Service and Blueprint adapter                                   |
| [`@ruvyxa/adapter-firebase`](packages/@ruvyxa/adapter-firebase)     | Firebase Hosting and Cloud Functions v2 adapter                            |
| [`@ruvyxa/adapter-aws`](packages/@ruvyxa/adapter-aws)               | AWS Amplify Hosting static and compute adapter                             |

`ruvyxa` also resolves one of five `@ruvyxa/cli-<platform>` packages as an optional dependency. They
carry the prebuilt native CLI for a single target and are not imported directly.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for local setup, verification commands, and release rules.

---

## License

[Apache 2.0](LICENSE) Copyright (c) 2026 Thirawat Sinlapasomsak
