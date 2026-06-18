# Ruvyxa App Guide

## Purpose

Use this reference when an agent needs to build or modify an application that uses Ruvyxa. This is only an application-side guide. It should steer the agent toward public APIs, app files, config, package scripts, and CLI verification.

## App Structure

- `app/layout.tsx`: shared layout wrapper.
- `app/page.tsx`: home page route.
- `app/**/page.tsx`: page routes.
- `app/api/**/route.ts`: API routes.
- `app/**/client.tsx`: route-level browser code.
- `app/**/server.ts`: route-local server-only helpers.
- `app/**/action.ts`: route-local server actions.
- `public/`: static assets.
- `ruvyxa.config.ts`: app paths, server defaults, build settings, cache, middleware, adapters, and debug settings.

## Routing

- Static page: `app/about/page.tsx` maps to `/about`.
- Dynamic page: `app/blog/[slug]/page.tsx` maps to `/blog/:slug`.
- Catch-all page: `app/docs/[...path]/page.tsx` maps to `/docs/*path`.
- Optional catch-all page: `app/docs/[[...path]]/page.tsx`.
- Route groups such as `app/(marketing)/pricing/page.tsx` do not add URL segments.
- API route: `app/api/health/route.ts` maps to `/api/health`.

Every page must export a default component.

## Server And Client Boundaries

- Browser-reachable files must not import server-only modules.
- Private env reads such as `process.env.SECRET` must stay in server-only modules.
- Browser-safe values must use `RUVYXA_PUBLIC_*`.
- Keep database, filesystem, secret, and privileged network calls in server modules, loaders, actions, or API routes.
- Run `pnpm check` after moving imports or adding env reads.

## Public APIs

Prefer imports from public package entrypoints:

```ts
import { defineConfig } from "ruvyxa/config"
import { action, loader } from "ruvyxa/server"
```

Use `loader()` for server-side data loading and `action()` for validated server mutations. Keep action input validation close to the handler.

## Config

Check `ruvyxa.config.ts` before changing runtime behavior. Common blocks:

- `appDir`: route source directory, usually `app`.
- `outDir`: production output directory, usually `.ruvyxa`.
- `server`: host and port defaults.
- `build`: minify, sourcemap, split strategy, and parallelism.
- `cache`: route manifest and CSS runtime cache toggles.
- `middleware`: timing, logging, CORS, rate limiting, custom headers, and plugins.
- `debug`: trace-oriented settings.

## Node 22 Policy

Ruvyxa apps should use Node.js 22:

- Keep `"engines": { "node": ">=22.0.0" }` in app `package.json`.
- Do not switch Ruvyxa execution to another JavaScript runtime.
- Use package manager scripts such as `pnpm dev`, `pnpm build`, and `pnpm start`.

## Verification Choices

- Component or type-only changes: run `pnpm check`.
- Route/import/env changes: run `pnpm check`.
- Dev/prod behavior failures: run `pnpm parity` to isolate route behavior.
- Raw diagnostics: run `pnpm analyze`.
- Environment problems: run `pnpm doctor`.
- Manual runtime check: run `pnpm dev` during development and `pnpm start` after `pnpm build`.

## Do Not Do

- Do not edit Ruvyxa framework source code, package internals, native implementation files, renderer scripts, or adapter package internals.
- If a task requires framework source changes, stop and ask for a framework-development guide instead of using this app-user skill.
- Do not commit `.ruvyxa/`, `dist/`, `node_modules/`, local `.env` files, or secrets.
- Do not silence diagnostics by moving unsafe code into browser-reachable files.
- Do not rewrite the app structure when a narrow route, config, or component change is enough.
