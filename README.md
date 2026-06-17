# Ruvyxa

Ruvyxa is a Rust-powered full-stack TypeScript framework prototype with file routing, a smart dev server, production output, and typed public APIs.


## What Works Today

- Rust CLI commands: `dev`, `build`, `start`, `routes`, `analyze`, `doctor`, `bench`, `test:parity`
- File route discovery from `app/`
- Route manifest generation
- React-compatible SSR for `page.tsx` through the Ruvyxa SSR renderer
- TS/TSX transformation through esbuild in the Node renderer bridge
- Dynamic route params passed into SSR page components
- CSS injection from app CSS files
- Tailwind CSS v4 support through `@import "tailwindcss"` in app CSS
- Single-file PNG brand asset copied from `public/ruvyxa.png` into `.ruvyxa/assets`
- API route execution for `route.ts` handlers such as `GET`
- HMR WebSocket events for CSS updates, component updates, and full reload fallbacks
- Route-level browser hydration bundles with BLAKE3-hashed production output, minification, and esbuild tree shaking
- Server action endpoint for `action.ts` route modules with body-limit middleware, same-origin checks, Fetch Metadata checks, content-type guards, and rate limiting
- Default production security headers on HTML, JSON, static assets, client bundles, API responses, and action responses
- Runtime trace endpoint for route/module debugging
- `.env` and `.env.local` loading for SSR, API routes, and actions
- First-party adapter contracts for Node, Vercel, Cloudflare, Netlify, Bun, and static output
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
cargo run -p ruvyxa_cli -- bench --root examples/basic-app --samples 3
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
├─ client/   # hashed route-level hydration bundles and client manifest
├─ assets/   # copied public assets
├─ manifest.json
└─ build.json
```

`build.json` records the production profile, Unix build time, output directories, hash algorithm, and enabled security defaults.

## npm Publishing

Public npm package metadata is configured for `https://github.com/thirawat27/ruvyxa`. See [docs/publishing.md](docs/publishing.md) for dry-run checks, package contents, publish order, and post-publish verification.

## Production Readiness

Ruvyxa 1.0 is packaged for npm with native CLI binaries, release validation scripts, CI/release workflows, route-level optimized builds, hardened server actions, default security headers, deterministic BLAKE3 asset hashes, and dev/prod parity checks.

See [docs/production-readiness.md](docs/production-readiness.md) and [SECURITY.md](SECURITY.md).

## Roadmap

Post-1.0 work focuses on deeper Fast Refresh state preservation, richer source-map optimizer reports, and host-specific deployment emitters beyond the current typed adapter contracts.
