# Ruvyxa

Ruvyxa is a Rust-powered full-stack TypeScript framework prototype with file routing, a smart dev server, production output, and typed public APIs.

This repository implements the first MVP slice from `ruvyxa_framework_spec.md`.

## What Works Today

- Rust CLI commands: `dev`, `build`, `start`, `routes`, `doctor`
- File route discovery from `app/`
- Route manifest generation
- React-compatible SSR for `page.tsx` through the Ruvyxa SSR renderer
- TS/TSX transformation through esbuild in the Node renderer bridge
- Dynamic route params passed into SSR page components
- CSS injection from app CSS files
- Standard PNG brand assets copied from `public/` during build
- API route execution for `route.ts` handlers such as `GET`
- Full-page reload events over WebSocket when files change under `app/`, `components/`, `server/`, or `public/`
- TypeScript public API package stubs

## Quick Start

```bash
pnpm install
cargo run -p ruvyxa_cli -- dev --root examples/basic-app
```

Open `http://localhost:3000`.

Build and serve production output:

```bash
cargo run -p ruvyxa_cli -- build --root examples/basic-app
cargo run -p ruvyxa_cli -- start --root examples/basic-app
```

## Commands

```bash
cargo run -p ruvyxa_cli -- routes --root examples/basic-app
cargo run -p ruvyxa_cli -- doctor --root examples/basic-app
cargo test --workspace
pnpm -r build
```

## Current Limitations

The runtime now uses React SSR and esbuild for TS/TSX transformation, but it still does not implement client hydration, component-level HMR, server actions over HTTP, optimized chunking, tree shaking, or deploy adapters.
