# ruvyxa

Rust-powered full-stack TypeScript framework CLI and runtime.

## Install

```bash
npm install ruvyxa
```

The `ruvyxa` npm package includes the TypeScript runtime files and a source-built Rust CLI wrapper. The v0.1 package expects Rust/Cargo to be available on the machine that runs the CLI.

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

This package runs `prepack` before publishing. The script builds `dist/` and copies the Rust CLI crates into `native-src/` so the published CLI can run outside the monorepo.
