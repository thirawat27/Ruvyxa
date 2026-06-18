# Ruvyxa App Agent Guide

You are working in a Ruvyxa application. Keep this starter small and close to the app-router shape:

- `app/layout.tsx` wraps all pages.
- `app/page.tsx` is the home route.
- `app/globals.css` contains global styles.
- `public/` contains static assets.
- `ruvyxa.config.ts` controls app paths, server defaults, build output, cache behavior, and diagnostics.

## Rules

- Use Node.js 22 or newer.
- Keep browser-safe environment variables prefixed with `RUVYXA_PUBLIC_`.
- Keep private environment variables in server-only modules, API routes, loaders, and actions.
- Prefer typed public APIs from `ruvyxa`, `ruvyxa/server`, and `ruvyxa/config`.
- Do not commit `.env`, `.ruvyxa/`, `dist/`, `node_modules/`, or other generated output.

## Checks

Run the narrowest useful check while iterating:

```bash
pnpm typecheck
```

Before handing off app-sensitive changes, run:

```bash
pnpm check
```

