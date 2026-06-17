# Ruvyxa

Ruvyxa is a Rust-powered full-stack TypeScript framework prototype with file routing, a smart dev server, production output, and typed public APIs.


## What Works Today

- Rust CLI commands: `dev`, `build`, `start`, `routes`, `analyze`, `doctor`, `test:parity`
- File route discovery from `app/`
- Route manifest generation
- React-compatible SSR for `page.tsx` through the Ruvyxa SSR renderer
- TS/TSX transformation through esbuild in the Node renderer bridge
- Dynamic route params passed into SSR page components
- CSS injection from app CSS files
- Tailwind CSS v4 support through `@import "tailwindcss"` in app CSS
- Single-file PNG brand asset copied from `public/ruvyxa.png` into `.ruvyxa/assets`
- API route execution for `route.ts` handlers such as `GET`
- Full-page reload events over WebSocket when files change under `app/`, `components/`, `server/`, or `public/`
- Browser hydration bundle generation for page routes
- Server action endpoint for `action.ts` route modules
- Runtime trace endpoint for route/module debugging
- `.env` and `.env.local` loading for SSR, API routes, and actions
- First-party Node adapter contract through `@ruvyxa/adapter-node`
- Client bundle boundary diagnostics for `server-only`, `server/` imports, and private `process.env.*`
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
cargo run -p ruvyxa_cli -- analyze --root examples/basic-app
cargo run -p ruvyxa_cli -- doctor --root examples/basic-app
cargo run -p ruvyxa_cli -- trace /todos --root examples/basic-app
cargo run -p ruvyxa_cli -- test:parity --root examples/basic-app
cargo test --workspace
pnpm -r build
```

`doctor` reports app structure, package manager, Node/Bun/Deno availability, React version alignment, duplicate dependency versions, route diagnostics, env schema presence, and native binary status.

Call the example server action:

```bash
curl -X POST "http://localhost:3000/__ruvyxa/action?path=/todos&name=createTodo" \
  -H "content-type: application/json" \
  -d "{\"title\":\"Ship Ruvyxa\"}"
```

## Styling

New Ruvyxa apps include Tailwind CSS v4 by default:

```css
@import "tailwindcss";

@source "../app";
@source "../components";
```

Ruvyxa detects CSS files that import `tailwindcss`, runs the local `@tailwindcss/cli`, and injects the compiled CSS during dev and production serving. The minimal template includes both `tailwindcss` and `@tailwindcss/cli`.

## Environment

Server-side renderers load `.env` and `.env.local` from the app root. Document required keys in `.env.example`. Browser-reachable code may only use `RUVYXA_PUBLIC_*` variables.

## Build Output

```txt
.ruvyxa/
├─ server/   # production route source used by ruvyxa start
├─ client/   # client manifest and hydration metadata
├─ assets/   # copied public assets
├─ manifest.json
└─ build.json
```

## Current Limitations

The runtime now uses React SSR, browser hydration bundles, API route execution, server action invocation, runtime tracing, Tailwind CSS compilation, a Node adapter contract, and esbuild for TS/TSX transformation. It still does not implement component-level HMR, optimized chunking, tree shaking, or managed-host deploy adapters.
