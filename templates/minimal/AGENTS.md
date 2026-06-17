# Ruvyxa App Agent Guide

This is a Ruvyxa application using app-router file routing under `app/`.

## Structure

- `app/page.tsx` is the home page.
- `app/layout.tsx` wraps all pages.
- `app/api/**/route.ts` contains API route handlers.
- `public/` contains static assets.
- `ruvyxa.config.ts` controls app paths, server defaults, build output, and runtime caching.

## Rules

- Keep server-only code out of browser-reachable imports.
- Prefix browser-safe environment variables with `RUVYXA_PUBLIC_`.
- Run `pnpm analyze` before production builds when changing routes, imports, or environment usage.

## Commands

```bash
pnpm dev
pnpm build
pnpm start
pnpm analyze
pnpm doctor
```
