# Kitchen Sink — Ruvyxa Example App

Comprehensive example demonstrating all Ruvyxa framework features:

## Routes

| Path            | Type            | File                               |
| --------------- | --------------- | ---------------------------------- |
| `/`             | Static page     | `app/page.tsx`                     |
| `/about`        | Static nested   | `app/about/page.tsx`               |
| `/blog`         | Dynamic listing | `app/blog/page.tsx`                |
| `/blog/:slug`   | Dynamic segment | `app/blog/[slug]/page.tsx`         |
| `/catchall/...` | Catch-all       | `app/catchall/[...slug]/page.tsx`  |
| `/ssg-blog/:slug` | SSG           | `app/ssg-blog/[slug]/page.tsx`     |
| `/isr-page`     | ISR             | `app/isr-page/page.tsx`            |
| `/ppr-page`     | PPR             | `app/ppr-page/page.tsx`            |
| `/static-page`  | CSR             | `app/static-page/page.tsx`         |
| `/csr-page`     | CSR             | `app/csr-page/page.tsx`            |
| `/todos`        | Server action   | `app/todos/page.tsx` + `action.ts` |
| `/env`          | Public env demo | `app/env/page.tsx`                 |
| `/api/health`   | GET API         | `app/api/health/route.ts`          |
| `/api/echo`     | POST API        | `app/api/echo/route.ts`            |

## Architecture

- `app/layout.tsx` — root layout with nav
- `app/globals.css` — global styles
- `lib/utils.ts` — shared utilities (safe for client)
- `lib/db.ts` — server-only module (imports "server-only")

## Verification

```bash
cargo run -p ruvyxa_cli -- analyze --root .
cargo run -p ruvyxa_cli -- build --root .
cargo run -p ruvyxa_cli -- start --root . --port 3002
cargo run -p ruvyxa_cli -- test:parity --root .
cargo run -p ruvyxa_cli -- dev --root . --port 3001
```
