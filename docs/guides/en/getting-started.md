# Getting Started

Ruvyxa is a React framework where **folders are routes, pages are components, and the toolchain is
one native binary**. If you know basic React, you already know most of what you need — this page
gets you from zero to a running app in about five minutes.

## Requirements

- **Node.js** 22 or later (`node --version` to check)
- **Package manager**: npm, pnpm, Yarn, or Bun
- A published Ruvyxa application does **not** require a Rust toolchain — the native CLI ships as a
  prebuilt binary for your platform

Unsure about your environment? Ruvyxa can check it for you after install: `npx ruvyxa doctor`.

## Create a New Project

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run dev
```

Open `http://localhost:3000` — you should see the starter page. Edit `app/page.tsx`, save, and the
browser updates instantly without a full reload (that's HMR). The generated project intentionally
starts small:

```text
my-app/
├── app/
│   ├── globals.css
│   ├── layout.tsx
│   └── page.tsx
├── public/
│   └── ruvyxa.png
├── .gitignore
├── package.json
├── ruvyxa.config.ts
└── tsconfig.json
```

The default starter is `minimal`. Ruvyxa also provides focused starters:

```bash
npm create ruvyxa@latest my-blog -- --template blog
npm create ruvyxa@latest my-admin -- --template crud
npm create ruvyxa@latest my-api -- --template api-backend
```

| Starter       | Includes                                                         |
| ------------- | ---------------------------------------------------------------- |
| `minimal`     | One page, root layout, global stylesheet, and framework config.  |
| `blog`        | Post listing, dynamic post pages, and direct SSG parameters.     |
| `crud`        | In-memory task API, loader, cache, and validated server action.  |
| `api-backend` | Health and item REST endpoints with validation and error shapes. |

### Git Ignore

The starter ignores `node_modules/`, `.ruvyxa/`, `dist/`, log files, and `.env` files:

- **Do not commit secrets** or real environment values.
- Use `.env.example` only to list required variable names without real values.

## Application Structure

Ruvyxa discovers routes under `app/`:

| File / Folder           | Purpose                                                     |
| ----------------------- | ----------------------------------------------------------- |
| `app/layout.tsx`        | Wraps every page rendered below it.                         |
| `app/page.tsx`          | Handles the root URL: `/`.                                  |
| `app/<folder>/page.tsx` | Creates a nested route.                                     |
| `public/`               | Static files served from `/`.                               |
| `ruvyxa.config.ts`      | Controls server, build, rendering, security, cache, styles. |

## Your First Page

Every page file must default-export a React component:

```tsx
// app/products/page.tsx → /products
export default function ProductsPage() {
  return (
    <main>
      <h1>Products</h1>
    </main>
  )
}
```

## Layout

Keep layout concerns in `app/layout.tsx`. A layout normally imports global CSS and returns the
document shell:

```tsx
// app/layout.tsx
import './globals.css'

export const meta = {
  title: 'My Ruvyxa App',
  description: 'A production-ready application.',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
```

## Standard Scripts

```json
{
  "scripts": {
    "dev": "ruvyxa dev",
    "build": "ruvyxa build",
    "start": "ruvyxa start",
    "typecheck": "tsc --noEmit",
    "check": "ruvyxa check"
  }
}
```

## Your First 10 Minutes

A suggested path once `npm run dev` is running:

1. **Change the home page** — edit `app/page.tsx`, watch HMR update the browser.
2. **Add a second page** — create `app/about/page.tsx` with a default-exported component, then visit
   `/about`. No registration step; the folder _is_ the route.
3. **Add a dynamic page** — create `app/hello/[name]/page.tsx` and read `params.name`. Visit
   `/hello/world`.
4. **See what the framework sees** — run `npx ruvyxa routes` to print the discovered route table.
5. **Ship it** — `npm run build` then `npm run start` runs the exact production server locally.

## When Something Goes Wrong

| Symptom                            | First thing to try                                                                                |
| ---------------------------------- | ------------------------------------------------------------------------------------------------- |
| `npm run dev` fails to start       | `npx ruvyxa doctor` — checks Node version, ports, config                                          |
| Port 3000 is busy                  | Ruvyxa auto-scans the next 100 ports and tells you who owns the conflict; or pass `--port 4000`   |
| A URL unexpectedly 404s            | `npx ruvyxa routes` — is the route in the table?                                                  |
| Build fails with an `RUV____` code | The message includes the file and a suggestion; codes are documented in the diagnostics reference |
| Stale output after big changes     | `npx ruvyxa clean` removes `.ruvyxa/` caches safely                                               |

Every Ruvyxa error has a stable `RUV` code, the offending file, and a suggested fix — read the
message before searching the web; it usually contains the answer.

## Next Steps

- [Routing](routing.md) — file-system routes, dynamic segments, catch-all, route groups
- [Server & Client Components](server-client-components.md) — `'use client'`, `server-only`,
  boundary checks
- [Configuration](configuration.md) — `ruvyxa.config.ts` full reference
- [Styling](styling.md) — global CSS, SCSS/Sass, and CSS Modules
- [Official Packages](official-packages.md) — add a database, login, and realtime updates when
  you're ready
