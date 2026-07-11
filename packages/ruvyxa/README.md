# ruvyxa

CLI, runtime bridge, and public framework entrypoints for Ruvyxa apps.

## Install

```bash
npm install ruvyxa react react-dom
```

Published installs include the TypeScript runtime files, a persistent Node worker pool, and a native
CLI binary for the current platform. Rust and Cargo are only required when developing Ruvyxa from
source.

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
import { action, cache, cacheStats, invalidateCache, json, loader, notFound, redirect } from 'ruvyxa/server'
import type { Adapter, BuildContext, PluginContext, RuvyxaConfig, RuvyxaPlugin, TransformResult } from 'ruvyxa'
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
    jsx: 'classic',
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
    actionLimit: 65536,
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
        methods: ['GET', 'POST', 'PUT', 'DELETE'],
        credentials: true,
      },
    },
    plugins: [
      {
        name: 'auth-guard',
        path: 'plugins/auth.wasm',
        phase: 'request',
        hot: true,
        routes: ['/api/*'],
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

## Runtime Architecture

The `ruvyxa` package includes a persistent Node render worker pool (`runtime/worker-pool.mjs`) that
keeps Node processes alive between requests, plus a persistent build-plugin worker
(`runtime/plugin-runner.mjs`) that reuses loaded config hooks across modules. This removes repeated
process startup and config compilation overhead. Plugin transform source maps are forwarded into
generated client maps.

The runtime files included in this package:

| File                          | Purpose                                            |
| ----------------------------- | -------------------------------------------------- |
| `runtime/worker-pool.mjs`     | Persistent IPC worker for all rendering            |
| `runtime/ssr-renderer.mjs`    | Server-side React rendering                        |
| `runtime/client-renderer.mjs` | Client hydration bundle generation                 |
| `runtime/compiler.mjs`        | Ruvyxa runtime compiler used by all Node renderers |
| `runtime/api-renderer.mjs`    | API route execution                                |
| `runtime/action-renderer.mjs` | Server action execution                            |
| `runtime/config-renderer.mjs` | Config file loading                                |
| `runtime/plugin-runner.mjs`   | Persistent config-plugin hook worker               |
| `runtime/ssg-renderer.mjs`    | Build-time SSG/ISR/PPR pre-rendering               |

## Native CLI

The `ruvyxa` npm package resolves the native CLI binary automatically for the current platform.
Resolution order:

1. **Source checkout** — `target/debug/ruvyxa` or `target/release/ruvyxa` when working in the
   monorepo
2. **Bundled binary** — `native-bin/<platform>-<arch>/ruvyxa(.exe)` shipped with the npm package
3. **Optional platform package** — `@ruvyxa/cli-<platform>-<arch>` as a fallback (e.g.,
   `@ruvyxa/cli-win32-arm64`)

Application users only need to install `ruvyxa`. No Rust toolchain required.
