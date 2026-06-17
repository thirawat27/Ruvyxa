# ruvyxa

CLI, runtime bridge, and public framework entrypoints for Ruvyxa apps.

## Install

```bash
npm install ruvyxa react react-dom
```

Published installs include the TypeScript runtime files and a native CLI binary for the current platform. Rust and Cargo are only required when developing Ruvyxa from source.

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
import { action, cache, json, loader, notFound, redirect } from "ruvyxa/server"
import type { Adapter, RuvyxaConfig } from "ruvyxa"
```

## Minimal Config

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

The native CLI consumes these settings for app paths, output paths, server defaults, minification, client bundle parallelism, and runtime cache behavior. `runtime: "node"` and `react: true` are defaults and do not need to be written in new apps.

## Publish Notes

`prepack` builds the package, copies runtime files, and prepares the native binary path used by the npm CLI shim. Platform-specific `@ruvyxa/cli-*` packages may also provide the binary as optional dependencies.
