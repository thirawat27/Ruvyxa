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

*Note: This project supports multiple package managers. You can use `npm`, `pnpm`, `yarn`, or `bun` interchangeably.*

```bash
npm run dev         # ruvyxa dev
npm run build       # ruvyxa build
npm run start       # ruvyxa start
npm run check       # ruvyxa check (typecheck + parity + smoke render)
npm run typecheck   # tsc --noEmit
```

## Checks

Run the narrowest useful check while iterating:

```bash
npm run typecheck
```

Before handing off app-sensitive changes, run:

```bash
npm run check
```
