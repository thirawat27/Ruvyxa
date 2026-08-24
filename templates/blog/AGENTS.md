# Ruvyxa App Agent Guide

You are working in a Ruvyxa blog. Keep this starter small, explicit, and close to the file-system
route shape:

- `app/layout.tsx` wraps all pages.
- `app/page.tsx` is the home route.
- `app/globals.css` is the default global stylesheet.
- `public/` contains static assets.
- `ruvyxa.config.ts` configures server, build, cache, security, and middleware.

## How a post works here

- Every post lives in the `posts` array in `app/blog/posts.ts`, newest first. Adding a post is one
  entry in that array — nothing else has to be registered.
- `app/blog/page.tsx` lists them and `app/blog/[slug]/page.tsx` renders one. The `[slug]` folder is
  a dynamic segment; the page's `getStaticParams` export tells the build which slugs to pre-render,
  so each post is a file on disk before anyone asks for it. **Add a post to the array and the route
  appears — do not maintain a second list.**
- Both pages read the array through `findPost()` and `formatDate()`. Keep them there rather than
  duplicating a lookup or a date format in a page.
- `formatDate` passes an explicit `timeZone`, and that is load-bearing: a date formatted in the
  machine's own zone renders one day on the server and another in the browser. Do not drop it.
- Navigate with `<Link>` from `@ruvyxa/react`, not a bare `<a>`. `typedRoutes` is on, so a mistyped
  path is a compile error; a template literal built from a literal pattern — `` `/blog/${slug}` `` —
  type-checks on its own.
- `robots.txt` and `sitemap.xml` come from the route manifest at build time. Set `site.url` in
  `ruvyxa.config.ts` before deploying, or the build publishes `robots.txt` alone.

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
