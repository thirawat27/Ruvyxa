# Ruvyxa App Agent Guide

You are working in a Ruvyxa API. Keep this starter small, explicit, and close to the file-system
route shape:

- `app/layout.tsx` wraps all pages.
- `app/page.tsx` is the endpoint documentation, and the only page.
- `app/globals.css` is the default global stylesheet.
- `public/` contains static assets.
- `ruvyxa.config.ts` configures server, build, cache, security, and middleware.

## How the endpoints work

- A folder under `app/api/` with a `route.ts` is an endpoint; the exported `GET`, `POST`, `PUT`, and
  `DELETE` functions are its methods. A handler receives `{ request, params }`, and a dynamic
  segment is `string | string[]` until it has been checked.
- **Keep each handler readable on its own.** Validation and error responses are written inline, not
  behind a shared helper module, so a reader never has to open a second file to know what an
  endpoint answers. A little repetition is the price and it is deliberate.
- One error shape everywhere: `Response.json({ error: '…' }, { status })`. Success bodies are plain
  JSON. `Response.json` already sets `Content-Type`, so do not set it by hand.
- `app/api/items/store.ts` is an in-memory array, so it resets on restart and each worker keeps its
  own copy — two requests can legitimately see different data. It is a placeholder for a database
  client, and it is only ever imported by `route.ts` files, which never reach the browser.

## Rules

- Use a Node.js release at or above the `engines.node` floor in `package.json`. `npm run doctor`
  reports the resolved version and says when it is too old.
- Keep route files under `app/`. Pages use `page.tsx`; API routes use `route.ts`.
- Server-rendered pages are the default. Add `'use client'` only when browser-only interactivity is
  required.
- Keep browser-safe env vars prefixed with `RUVYXA_PUBLIC_`.
- Keep private env vars in server-only modules, API routes, loaders, and actions.
- Prefer typed public APIs from `ruvyxa`, `ruvyxa/server`, and `ruvyxa/config`.
- Keep external CSS project-relative. Imported CSS can live outside `app/`; use `css.entries` in
  `ruvyxa.config.ts` for global CSS files or directories that are not imported by application code.
- Import `.scss` or `.sass` directly when Sass is useful. Use `.module.css`, `.module.scss`, or
  `.module.sass` when styles should expose a locally scoped class map to a component.
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
npm run preview     # ruvyxa preview
npm run routes      # ruvyxa routes
npm run analyze     # ruvyxa analyze
npm run adds -- form # ruvyxa adds form
npm run doctor      # ruvyxa doctor
npm run clean       # ruvyxa clean
npm run trace -- /  # ruvyxa trace /
npm run bench       # ruvyxa bench
npm run test:parity # ruvyxa test:parity
npm run plugin -- create my-plugin # ruvyxa plugin create my-plugin
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
