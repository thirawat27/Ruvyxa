# ruvyxa

CLI, runtime bridge, and public framework entrypoints for Ruvyxa apps.

## Install

```bash
npm install ruvyxa react react-dom
```

Published installs include the TypeScript runtime files, a persistent Node worker pool, and a native CLI binary for the current platform. Rust and Cargo are only required when developing Ruvyxa from source.

## CLI

```bash
npx ruvyxa dev --root .
npx ruvyxa build --root .
npx ruvyxa start --root .
npx ruvyxa doctor --root .
```

Human-facing commands print the same compact TUI style used by the native server: headings, aligned fields, status labels, and color only on real terminals. Structured commands such as `analyze`, `trace`, and `bench --json` remain machine-readable.

Production builds emit route-level client bundles concurrently and keep manifest output deterministic.

## Imports

```ts
import { defineConfig } from "ruvyxa/config"
import { action, cache, invalidateCache, json, loader, notFound, redirect } from "ruvyxa/server"
import type { Adapter, RuvyxaConfig } from "ruvyxa"
```

## Configuration with Middleware

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

## Runtime Architecture

The `ruvyxa` package includes a persistent Node worker pool (`runtime/worker-pool.mjs`) that keeps Node processes alive between requests. This eliminates the ~100-500ms overhead of spawning Node + esbuild per request.

The runtime files included in this package:

| File | Purpose |
|------|---------|
| `runtime/ssr-renderer.mjs` | Server-side React rendering (streaming) |
| `runtime/client-renderer.mjs` | Client hydration bundle generation |
| `runtime/api-renderer.mjs` | API route execution |
| `runtime/action-renderer.mjs` | Server action execution |
| `runtime/config-renderer.mjs` | Config file loading |
| `runtime/worker-pool.mjs` | Persistent IPC worker for all rendering |

## Publish Notes

`prepack` builds the package, copies runtime files, and prepares the native binary path used by the npm CLI shim. Platform-specific `@ruvyxa/cli-*` packages may also provide the binary as optional dependencies.
