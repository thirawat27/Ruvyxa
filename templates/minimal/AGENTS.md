# Ruvyxa App Agent Guide

This is a Ruvyxa application.

## App Structure

- `app/` contains file-based routes.
- `app/page.tsx` is the root page.
- `app/layout.tsx` wraps nested pages.
- `app/api/**/route.ts` contains API route handlers.
- `public/` contains static assets copied into production output.
- `ruvyxa.config.ts` configures the app.

## Rules

- Keep server-only code out of page/client imports.
- Only expose client environment values with the `RUVYXA_PUBLIC_` prefix.
- Put browser-only code in page/client components, not route handlers.
- Run `ruvyxa analyze` before production builds when changing imports or routes.

## Commands

```bash
pnpm install
pnpm dev
pnpm build
pnpm start
pnpm routes
ruvyxa analyze
```
