# Ruvyxa App Agent Guide

You are working in a Ruvyxa application. Ruvyxa uses a Rust native CLI, a Node 22 renderer bridge, React 19, TypeScript, and app-router file routing under `app/`.

## Project Shape

- `app/layout.tsx` wraps all pages.
- `app/page.tsx` is the home route.
- `app/**/page.tsx` defines page routes.
- `app/api/**/route.ts` defines API routes.
- `app/**/client.tsx` contains route-level browser modules.
- `app/**/server.ts` and `app/**/action.ts` contain server-only logic for nearby routes.
- `public/` contains static assets.
- `ruvyxa.config.ts` controls paths, server defaults, build output, caching, middleware, and debug settings.

## Runtime Rules

- Use Node.js 22 for all app development and CI. Do not lower the `engines.node` requirement.
- Do not introduce another JavaScript runtime for framework execution. Ruvyxa's renderer bridge runs on Node.
- Keep server-only imports out of browser-reachable modules and client components.
- Prefix browser-safe environment variables with `RUVYXA_PUBLIC_`; keep private env reads in server-only modules.
- Keep `dev`, `build`, and `start` behavior aligned. If behavior must match across modes, put it behind shared config or framework paths rather than local command-specific workarounds.
- Preserve structured diagnostics. User-facing framework errors should keep actionable `RUV####` codes when they originate from Ruvyxa.

## Development Workflow

- Read `ruvyxa.config.ts` before changing routes, middleware, cache behavior, or runtime defaults.
- Update `app/` and colocated server/client modules together when route behavior changes.
- Prefer typed public APIs from `ruvyxa`, `ruvyxa/server`, and `ruvyxa/config`.
- Keep generated output out of source changes unless the task is explicitly about build artifacts.
- Do not commit secrets, local `.env` files, `.ruvyxa/`, `dist/`, or `node_modules/`.

## Verification

Run the narrowest useful check while iterating, then run the full handoff set before shipping app-sensitive changes:

```bash
pnpm check
pnpm analyze
pnpm build
pnpm parity
```

Use `pnpm doctor` when environment, dependency, or route discovery behavior is unclear. Use `pnpm routes` to inspect route IDs and file mapping.

## Commands

```bash
pnpm dev
pnpm build
pnpm start
pnpm preview
pnpm routes
pnpm analyze
pnpm doctor
pnpm clean
pnpm parity
pnpm check
```
