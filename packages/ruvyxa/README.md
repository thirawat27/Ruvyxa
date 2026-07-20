# ruvyxa

CLI, runtime bridge, and public framework entrypoints for Ruvyxa apps.

## Install

Node.js 22.12 or later is required by the native Oxc transformer.

```bash
npm install ruvyxa react react-dom
```

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
npx ruvyxa dev --root .          # Development server with HMR
npx ruvyxa build --root .        # Production build (--target node|edge|static)
npx ruvyxa start --root .        # Serve production build
npx ruvyxa preview --root .      # Alias for start
npx ruvyxa check --root .        # App-level production readiness checks
npx ruvyxa routes --root .       # Show discovered routes
npx ruvyxa analyze --root .      # Structured validation JSON
npx ruvyxa doctor --root .       # Check project health and environment
npx ruvyxa trace <path>          # Inspect route matching
npx ruvyxa bench --root .        # Benchmark discovery, validation, builds
npx ruvyxa test:parity --root .  # Dev/prod route parity check
npx ruvyxa clean --root .        # Remove .ruvyxa/ output
```

Human-facing commands print the same compact TUI style used by the native server: headings, aligned
fields, status labels, and color only on real terminals. Use `check` as the app-level production
readiness gate. Structured commands such as `analyze`, `trace`, and `bench --json` remain
machine-readable.

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
  PluginSetupContext,
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

Register application middleware in the same config with `plugin`:

```ts
import { config, plugin } from 'ruvyxa/config'

export default config({
  plugins: [
    plugin('auth-guard', {
      routes: ['/api/*'],
      onRequest(request) {
        return request.headers.get('authorization')
          ? undefined
          : new Response('Unauthorized', { status: 401 })
      },
    }),
  ],
})
```

## Runtime Architecture

The `ruvyxa` package includes a persistent Node/Bun render worker pool (`runtime/worker-pool.mjs`)
and one persistent plugin runtime (`runtime/plugin-runtime.mjs`). The runtime loads
`ruvyxa.config.ts` once, registers middleware and build hooks, and serves validated NDJSON calls
from the Rust server and bundler. Plugin transform source maps are forwarded into generated client
maps.

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
