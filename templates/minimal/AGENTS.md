# Ruvyxa App Agent Guide

You are working in a Ruvyxa application. Keep this starter small, explicit, and close to the
file-system app-router shape:

- `app/layout.tsx` wraps all pages.
- `app/page.tsx` is the home route.
- `app/globals.css` is the default global stylesheet.
- `public/` contains static assets.
- `ruvyxa.config.ts` configures server, build, cache, security, and middleware.

## Rules

- Use Node.js 22 or newer.
- Keep route files under `app/`. Pages use `page.tsx`; API routes use `route.ts`.
- Server-rendered pages are the default. Add `'use client'` only when browser-only interactivity is
  required.
- Keep browser-safe env vars prefixed with `RUVYXA_PUBLIC_`.
- Keep private env vars in server-only modules, API routes, loaders, and actions.
- Prefer typed public APIs from `ruvyxa`, `ruvyxa/server`, and `ruvyxa/config`.
- Keep external CSS project-relative. Imported CSS can live outside `app/`; use `css.entries` in
  `ruvyxa.config.ts` for global CSS files or directories that are not imported by application code.
- Runtime CSS-in-JS through React `style` objects and `<style>` elements is supported. Libraries
  that require compile-time transforms should be wired through a transform plugin.
- Do not commit `.env`, `.ruvyxa/`, `dist/`, `node_modules/`, or other generated output.

## Commands

This project supports multiple package managers. The scripts below are shown with `npm`; use the
equivalent command for `pnpm`, `yarn`, or `bun` if that is the package manager in use.

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

Use `npm run build` as the final local production build signal when changing routing, rendering,
styling, config, or environment behavior.
