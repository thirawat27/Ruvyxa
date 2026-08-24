<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">ruvyxa</h1>

<p align="center">
  CLI, runtime bridge, and public framework entrypoints for Ruvyxa apps.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/ruvyxa"><img src="https://img.shields.io/npm/v/ruvyxa?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/ruvyxa"><img src="https://img.shields.io/node/v/ruvyxa?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

Ruvyxa requires the Node.js release declared by `engines.node` in this package. Run `ruvyxa doctor`
in a project to see what resolved on your machine and whether it is new enough.

```bash
npm install ruvyxa
```

One package is enough on npm. `react` and `react-dom` are peer dependencies, which npm installs on
your behalf, and `@ruvyxa/react` is a dependency of this package. Package managers that leave peer
dependencies to you — pnpm and Yarn — need the full set named instead:

```bash
pnpm add ruvyxa @ruvyxa/react react react-dom
```

Either way `npm create ruvyxa@latest` writes all four into the generated `package.json`, so a
scaffolded app never depends on which package manager installs peers.

Published installs include the TypeScript runtime files, a persistent JavaScript worker pool, and a
native CLI binary for the current platform. Rust and Cargo are only required when developing Ruvyxa
from source.

The package also provides ambient contracts for CSS, SCSS, Sass, and their `.module.*` variants. CSS
Module imports expose a typed readonly class map; projects created with `create-ruvyxa` do not need
a local `css.d.ts` file.

```tsx
import styles from './card.module.scss'

export function Card() {
  return <article className={styles.card}>Scoped card</article>
}
```

## CLI

```bash
npm run dev                       # Development server with HMR
npm run build                     # Production build (--target node|edge|static)
npm run start                     # Serve production build
npm run preview                   # Alias for start
npm run check                     # App-level production readiness checks
npm run routes                    # Show discovered routes
npm run routes:json                # Machine-readable route tree
npm run analyze:html               # Interactive self-contained bundle report
npm run adds -- form              # Scaffold form + validated Server Action
npm run adds -- data-table        # Scaffold a typed client data table
npm run adds -- auth              # Scaffold an @ruvyxa/auth flow
npm run doctor                    # Check project health and environment
npm run trace -- <path>           # Inspect route matching
npm run bench                     # Benchmark discovery, validation, builds
npm run bench -- --baseline       # Isolated build, route, and edit-class baseline
npm run test:parity               # Dev/prod route parity check
npm run clean                     # Remove .ruvyxa/ output
```

Human-facing commands print the same compact TUI style used by the native server: headings, aligned
fields, status labels, and color only on real terminals. Use `check` as the app-level production
readiness gate. Structured commands such as `analyze`, `trace`, and `bench --json` remain
machine-readable.

`bench --baseline --json` emits the stable `ruvyxa.build-bench` contract with `schemaVersion: 1`. It
clones project inputs into a disposable workspace per sample and measures cold/warm builds, first
route rendering, CSS/client/server/leaf edits, peak resident memory, and HMR reload fallbacks. It
verifies cold/warm semantic artifact equivalence before reporting; the application's real source and
cache are untouched.

During `npm run dev`, open `/__ruvyxa/devtools` for the registered route tree, render-cache state,
Server Action timings, bundle metrics, and server uptime. The endpoint is development-only and its
data endpoint enforces the dev server's origin policy.

Production builds emit route-level client bundles concurrently and keep manifest output
deterministic.

## Imports

```ts
import { config } from 'ruvyxa/config'
import {
  action,
  cache,
  cacheStats,
  invalidateCache,
  json,
  loader,
  notFound,
  redirect,
} from 'ruvyxa/server'
import type {
  Adapter,
  BuildContext,
  PluginRegistrationApi,
  RuvyxaConfig,
  RuvyxaPlugin,
  TransformResult,
} from 'ruvyxa'
```

## Configuration with Middleware

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
  cache: {
    routes: true,
    css: true,
    dir: '.ruvyxa/cache/bundler',
  },
  security: {
    actionLimit: 1024 * 1024,
    apiLimit: 10 * 1024 * 1024,
    pluginLimit: 32 * 1024 * 1024,
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
        methods: ['GET', 'POST', 'PUT', 'DELETE'],
        credentials: true,
      },
    },
  },
})
```

Register application middleware with the concise `http` section. Use `register()` for build, dev,
diagnostics, native, or advanced composition:

```ts
import { config } from 'ruvyxa/config'
import { definePlugin } from 'ruvyxa/plugin'

export default config({
  plugins: [
    definePlugin({
      name: 'auth-guard',
      http: {
        match: ['/api/*'],
        onRequest({ request }) {
          return request.headers.get('authorization')
            ? undefined
            : new Response('Unauthorized', { status: 401 })
        },
      },
    }),
  ],
})
```

## Built-in Plugins

For production stateful features, Ruvyxa also ships `@ruvyxa/database`, `@ruvyxa/auth`, and
`@ruvyxa/realtime`. Database and auth use explicit durable adapters rather than process-global
state. Native realtime is action-driven and supported on self-hosted Node/Bun; unsupported static,
edge, and serverless targets fail during build instead of deploying a dead socket.

`ruvyxa/plugins` provides typed first-party plugins without extra packages:

- Runtime: `observability()`, `securityHeaders()`, and `cacheRules()`
- Content and app delivery: `contentEngine()`, `pwa()`, `feed()`, `searchIndex()`, and `openApi()`
- Routing/build utilities: `redirects()`, `headers()`, `sitemap()`, `robots()`, `alias()`,
  `bundleBudget()`, and `requireEnv()`

```ts
import { config } from 'ruvyxa/config'
import { cacheRules, observability, securityHeaders } from 'ruvyxa/plugins'

export default config({
  site: {
    url: 'https://example.com',
    title: 'Example',
    description: 'Latest articles',
    language: 'en',
  },
  content: true,
  plugins: [
    observability({ routes: ['/api/*'] }),
    securityHeaders({ contentSecurityPolicy: { 'default-src': ["'self'"] } }),
    cacheRules([{ source: '/api/*', browser: 'no-store' }]),
  ],
})
```

`content: true` automatically enables Content Engine without duplicate plugin wiring. The explicit
`contentEngine(options)` plugin remains available for programmatic composition. Content Engine also
publishes explicit answer metadata and an `/llms.txt` agent discovery index from the same
Markdown/MDX graph. Build-generated files are written before adapters materialize deployment
artifacts, so PWA, RSS, search-index, OpenAPI, sitemap, robots, and `llms.txt` outputs ship with
static and hybrid adapters. See the English and Thai plugin guides for complete options, including
independent OpenAI search/training crawler policy.

## Runtime Architecture

The `ruvyxa` package includes a persistent Node/Bun render worker pool (`runtime/worker-pool.mjs`)
and the plugin runtime (`runtime/plugin-runtime.mjs`). Each plugin host loads `ruvyxa.config.ts`
once and serves validated NDJSON calls; dev HTTP hooks can use 1–8 processes, while one build-owned
host serves the complete start, resolve/load/transform, and complete lifecycle of each production
build. Module state is shared only inside one process. Dev middleware calls default to a 30-second
timeout, and repeated HTTP headers survive the native bridge. Plugin transform source maps are
forwarded into generated client maps.

The runtime files included in this package:

| File                          | Purpose                                                                          |
| ----------------------------- | -------------------------------------------------------------------------------- |
| `runtime/worker-pool.mjs`     | Persistent IPC worker for all rendering (SSR, SSG/ISR/PPR, API, actions, client) |
| `runtime/ssr-renderer.mjs`    | Standalone SSR fallback used when the worker pool is unavailable                 |
| `runtime/compiler.mjs`        | Oxc-backed runtime compiler used by all Node/Bun renderers                       |
| `runtime/api-renderer.mjs`    | Standalone API route fallback used when the worker pool is unavailable           |
| `runtime/config-renderer.mjs` | Config file loading                                                              |
| `runtime/plugin-runtime.mjs`  | Persistent plugin registry and hook worker                                       |

## Ruvyxa CLI

The `ruvyxa` npm package resolves the Ruvyxa CLI binary automatically for the current platform.
Resolution order:

1. **Source checkout** — `target/debug/ruvyxa` or `target/release/ruvyxa` when working in the
   monorepo
2. **Bundled binary** — `native-bin/<platform>-<arch>/ruvyxa(.exe)` shipped with the npm package
3. **Optional platform package** — `@ruvyxa/cli-<platform>-<arch>` as a fallback (e.g.,
   `@ruvyxa/cli-win32-arm64`)

Application users only need to install `ruvyxa`. No Rust toolchain required.
