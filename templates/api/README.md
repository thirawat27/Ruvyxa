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
├── layout.tsx                 the HTML shell wrapping every page
├── page.tsx                   the home route, "/"
├── api/health/route.ts        "/api/health" — a liveness endpoint
├── api/items/route.ts         "/api/items" — collection GET and POST
├── api/items/[id]/route.ts    "/api/items/:id" — one item by id
├── api/items/store.ts         the data this starter reads and writes
└── globals.css                the default stylesheet
public/                        files served as-is at the site root
ruvyxa.config.ts               server, build, cache, security, and middleware settings
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

## How an API route works

Export an uppercase function per HTTP method — `GET`, `POST`, `DELETE` — from a `route.ts`. Each
receives the Web `Request` and returns a Web `Response`; there is no framework-specific request
object to learn. Validate anything that came from the client before you use it, and keep payload
limits under `security` in `ruvyxa.config.ts`.

Build API-only output with `npm run build -- --server-only` when the project ships no pages.

## Going further

- Configuration, rendering strategies, deployment, and the full API live in the
  [Ruvyxa documentation](https://github.com/thirawat27/Ruvyxa#documentation).
- `AGENTS.md` and `CLAUDE.md` in this folder describe the same project to a coding agent. Keep them
  accurate as the app grows.
