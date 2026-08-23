# Ruvyxa App Agent Guide

You are working in a Ruvyxa blog. Keep this starter small, explicit, and close to the file-system
route shape:

- `app/layout.tsx` wraps all pages.
- `app/page.tsx` is the home route.
- `app/globals.css` is the default global stylesheet.
- `public/` contains static assets.
- `ruvyxa.config.ts` configures server, build, cache, security, and middleware.

## How a post works here

- A post is a folder under `app/blog/` containing `page.mdx`. The folder name is the URL segment.
  Markdown and MDX routes need no configuration; `.md` keeps raw HTML inert, `.mdx` evaluates JSX.
- The frontmatter between the `---` fences is exported twice by the compiler: as `frontmatter`,
  which `app/blog/posts.ts` reads to build the index, and as `meta`, which the router renders as
  `<title>` and `<meta name="description">`. **Write it once; never restate a title in `posts.ts`.**
  `app/ruvyxa-env.d.ts` is the type contract those fields have to satisfy.
- Publishing a post is a new folder plus one line in `app/blog/posts.ts`. Its `href` is checked
  against the real routes because `typedRoutes` is on, so a renamed folder is a compile error rather
  than a dead link.
- Sort and compare dates as ISO strings and format them with an explicit `timeZone`. `posts.ts`
  explains both: `localeCompare` orders by the building machine's locale, and a date formatted in
  the machine's own zone renders one day on the server and another in the browser.
- `content: true` derives `rss.xml`, `content.json`, `search-index.json`, and `llms.txt` from those
  same posts, and the route manifest yields `sitemap.xml` and `robots.txt`. All of them embed
  `site.url`, so **change it in `ruvyxa.config.ts` before deploying** — it ships as a placeholder.

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
