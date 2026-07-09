# Ruvyxa App Agent Guide

You are working in a Ruvyxa application. Keep this starter small and close to the app-router shape:

- `app/layout.tsx` wraps all pages.
- `app/page.tsx` is the home route.
- `app/globals.css` contains global styles.
- `public/` contains static assets.
- `ruvyxa.config.ts` configures server, build, cache, security, and middleware.

## Rules

- Use Node.js 22 or newer.
- Keep browser-safe env vars prefixed with `RUVYXA_PUBLIC_`.
- Keep private env vars in server-only modules, API routes, loaders, and actions.
- Prefer typed public APIs from `ruvyxa`, `ruvyxa/server`, and `ruvyxa/config`.
- Do not commit `.env`, `.ruvyxa/`, `dist/`, `node_modules/`, or other generated output.

## Commands

```bash
pnpm dev         # ruvyxa dev
pnpm build       # ruvyxa build
pnpm start       # ruvyxa start
pnpm check       # ruvyxa check (typecheck + parity + smoke render)
pnpm typecheck   # tsc --noEmit
```

## Checks

Run the narrowest useful check while iterating:

```bash
pnpm typecheck
```

Before handing off app-sensitive changes, run:

```bash
pnpm check
```

