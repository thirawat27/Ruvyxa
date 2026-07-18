# Getting Started

## Requirements

- **Node.js** 22 or later
- **Package manager**: npm, pnpm, Yarn, or Bun
- A published Ruvyxa application does **not** require a Rust toolchain

## Create a New Project

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run dev
```

Open `http://localhost:3000`. The generated project intentionally starts small:

```text
my-app/
├── app/
│   ├── globals.css
│   ├── layout.tsx
│   └── page.tsx
├── public/
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

## Next Steps

- [Routing](routing.md) — file-system routes, dynamic segments, catch-all, route groups
- [Server & Client Components](server-client-components.md) — `'use client'`, `server-only`,
  boundary checks
- [Configuration](configuration.md) — `ruvyxa.config.ts` full reference
- [Styling](styling.md) — global CSS, SCSS/Sass, and CSS Modules
