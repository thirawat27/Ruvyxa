# Ruvyxa App Agent Guide

You are working in a Ruvyxa API. Keep this starter small, explicit, and close to the file-system
route shape:

- `app/layout.tsx` wraps all pages.
- `app/page.tsx` is the endpoint documentation, and the only page.
- `app/globals.css` is the default global stylesheet.
- `public/` contains static assets.
- `ruvyxa.config.ts` configures server, build, cache, security, and middleware.

## How the endpoints work

- A folder under `app/api/` with a `route.ts` is an endpoint; the exported `GET`, `POST`, `PATCH`,
  and `DELETE` functions are its methods. A handler receives `{ request, params }`, and a dynamic
  segment is `string | string[]` until it has been narrowed.
- `app/api/http.ts` holds everything the handlers share: JSON body reading, field validation, and
  the error shape. **Add a new error there, not inline** — the repeated four-line error literal is
  what that module replaced.
- Errors answer with the RFC 9457 shape (`title`, `status`, `detail`) as `application/problem+json`,
  so a client can tell an error body from a result without inspecting its fields. Success bodies
  stay plain `application/json`.
- Status codes carry meaning: `201` with a `Location` header for a create, `204` with no body for a
  delete, `PATCH` for a partial update because `PUT` means replacement. Keep them that way.
- `app/api/items/store.ts` is in-process, so it resets on restart and each worker holds its own copy
  — two requests can legitimately see different data. It is a placeholder for a database client, and
  it is only ever imported by `route.ts` files, which never reach the browser.
- `security.apiLimit` in `ruvyxa.config.ts` bounds the request body these endpoints will read. Raise
  it deliberately if an endpoint starts accepting uploads.

## Rules

- Use Node.js 24.19 or newer.
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
