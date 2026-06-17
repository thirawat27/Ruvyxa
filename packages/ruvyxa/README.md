# ruvyxa

Rust-powered full-stack TypeScript framework CLI and runtime.

## Install

```bash
npm install ruvyxa
```

The `ruvyxa` npm package includes the TypeScript runtime files and a native CLI binary for the current release platform. Users do not need Rust or Cargo to run the CLI after installing from npm.

## Usage

```bash
npx ruvyxa dev --root .
npx ruvyxa build --root .
npx ruvyxa start --root .
```

Framework APIs are re-exported from `@ruvyxa/core`:

```ts
import { defineConfig, action, loader } from "ruvyxa"
```

## Publish Notes

This package runs `prepack` before publishing. The script builds `dist/` and copies the release binary into `native-bin/` so the published CLI can run outside the monorepo. Platform-specific packages such as `@ruvyxa/cli-win32-x64` can also provide the binary as optional dependencies.
