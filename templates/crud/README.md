# Your Ruvyxa app

Scaffolded with `create-ruvyxa`. The compiler and server are a prebuilt native binary; everything
you edit is ordinary React and TypeScript.

## Run it

```bash
npm install
npm run dev
```

Open the URL the command prints. Edit a file under `app/` and save — the page reloads without losing
state.

## What is where

```text
app/
├── layout.tsx         the HTML shell wrapping every page
├── page.tsx           the home route, "/"
├── about/page.tsx     a static nested route, "/about"
├── tasks/page.tsx     the list UI, "/tasks"
├── tasks/server.ts    a loader — reads data on the server
├── tasks/action.ts    a server action — validated writes
└── globals.css        the default stylesheet, importing Tailwind CSS
public/                files served as-is at the site root
postcss.config.ts      the PostCSS chain Ruvyxa runs over every global stylesheet
ruvyxa.config.ts       server, build, cache, security, and middleware settings
```

Routing is the folder layout: a `page.tsx` becomes a page at its folder's path, a `route.ts` becomes
an API endpoint, and `layout.tsx` wraps everything below it. Nothing registers a route by hand.
`npm run routes` prints the table Ruvyxa discovered, which is the fastest way to check that a file
landed where you meant.

## The five commands worth learning first

| Command          | Use it when                                                                |
| ---------------- | -------------------------------------------------------------------------- |
| `npm run dev`    | Writing code. Hot reload, route watching, error overlay.                   |
| `npm run routes` | A page you added does not appear. Shows every discovered route.            |
| `npm run check`  | Before you hand work off. Typecheck, build, dev/prod parity, smoke render. |
| `npm run build`  | Producing the production output in `.ruvyxa/`.                             |
| `npm run doctor` | Something is off about the environment — versions, dependencies, adapter.  |

`npm run start` and `npm run preview` serve an existing build, so run `build` first. The full
command list is in `package.json`; pass framework flags after `--`, as in
`npm run analyze -- --format json`.

## Rules that will save you time

- **Server by default.** Pages render on the server. Add `'use client'` at the top of a file only
  when it needs browser-only interactivity — state, effects, event handlers.
- **Secrets stay server-side.** Only variables prefixed `RUVYXA_PUBLIC_` reach the browser, and that
  prefix is a deliberate, one-way door. Everything else belongs in loaders, actions, API routes, or
  server-only modules.
- **`ruvyxa.config.ts` is typed.** Autocomplete lists the real options; `npm run check` rejects the
  rest.

## The read/write split

`server.ts` holds a `loader` — it runs on the server and hands data to the page. `action.ts` holds
an `action` — it validates input with a schema before the handler ever sees it, then calls
`invalidate()` so the next read is fresh. Swap the in-memory store for your database and the page
does not change.

## Going further

- Configuration, rendering strategies, deployment, and the full API live in the
  [Ruvyxa documentation](https://github.com/thirawat27/Ruvyxa#documentation).
- `AGENTS.md` and `CLAUDE.md` in this folder describe the same project to a coding agent. Keep them
  accurate as the app grows.
