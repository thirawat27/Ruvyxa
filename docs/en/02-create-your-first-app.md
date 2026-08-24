# Create your first Ruvyxa app

> **Tutorial goal:** create, run, and verify a Ruvyxa app with a page and a health API route.
> **Start from:** [Introduction](01-introduction.md). **Checkpoint:** the readiness,
> production-build, and route-parity checks succeed for your app.

## Create an application

The workspace publishes `create-ruvyxa`, and its source contains `minimal`, `blog`, `crud`, and
`api` templates. Use the generator for a complete, package-manager-neutral starter.

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run dev
```

The generated project scripts invoke the installed `ruvyxa` binary. `dev` discovers routes and
starts hot reload; its default root is the current directory. Visit the URL printed by the command
(the default server configuration is `localhost:3000` when no override is supplied).

## Install into an existing React project

On npm the framework is one package. `react` and `react-dom` are peer dependencies that npm installs
for you, and `@ruvyxa/react` is a dependency of `ruvyxa`.

```bash
npm install ruvyxa
npm install -D typescript @types/react @types/react-dom
```

pnpm and Yarn install neither peer dependencies nor a transitive package at the project root, so
name the set the templates declare. Keep compatible React versions together.

```bash
pnpm add ruvyxa @ruvyxa/react react react-dom
pnpm add -D typescript @types/react @types/react-dom
```

Create `ruvyxa.config.ts`:

```ts
import { config } from 'ruvyxa/config'

export default config({
  appDir: 'app',
  outDir: '.ruvyxa',
  server: { host: 'localhost', port: 3000 },
})
```

Then add the files in [Project structure](03-project-structure.md). Do not put an application secret
in a `RUVYXA_PUBLIC_` variable: that prefix is deliberately exposed to browser code.

## Build one working vertical slice

Create these files after installing the dependencies. This is deliberately small: it proves page
routing, a layout, and an API route before you introduce database, auth, or plugins.

```text
app/
├── layout.tsx
├── page.tsx
└── api/
    └── health/
        └── route.ts
```

```tsx
// app/layout.tsx
import type { ReactNode } from 'react'

export const meta = { title: 'My Ruvyxa app', description: 'First Ruvyxa app' }

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
```

```tsx
// app/page.tsx
export default function Home() {
  return (
    <main>
      <h1>Ruvyxa is running</h1>
      <p>Edit app/page.tsx and save.</p>
    </main>
  )
}
```

```ts
// app/api/health/route.ts
export function GET() {
  return Response.json({ status: 'ok' })
}
```

Run `npm run dev`, open `/`, then open `/api/health`. The first request renders the page; the health
route returns JSON with `status: "ok"`. Save a change to `app/page.tsx` to confirm hot reload, then
verify discovery and production behavior:

```bash
npm run routes
npm run check
npm run build
npm run test:parity
```

If any command fails, stop at that command and use [Troubleshooting](16-troubleshooting-upgrades.md)
before deploying. `test:parity` compares dev/prod routes and smoke-renders page routes; it is not a
replacement for application tests.

## Scripts

```bash
npm run dev
npm run build
npm run start
npm run preview
npm run typecheck
npm run check
npm run routes
npm run routes:json
npm run analyze
npm run analyze:html
npm run adds -- form
npm run doctor
npm run clean
npm run trace -- /
npm run bench
npm run test:parity
npm run plugin -- create my-plugin
```

These are the user-facing scripts provided by every starter. `start` and `preview` operate on an
existing production build; run `build` first. `check` is the application-level readiness command.
See [CLI reference](10-cli.md) for when each script is useful.

**Previous:** [Introduction](01-introduction.md) · **Next:**
[Project structure](03-project-structure.md)
