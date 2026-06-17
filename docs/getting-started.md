# Getting Started

## Start in 60 Seconds

```bash
pnpm install
cargo run -p ruvyxa_cli -- dev --root examples/basic-app
```

Open `http://localhost:3000`.

## Your First Page

Create `app/page.tsx`:

```tsx
export default function Home() {
  return (
    <main>
      <h1>Hello Ruvyxa</h1>
      <p>Full-stack TypeScript, powered by Rust.</p>
    </main>
  )
}
```

## Build

```bash
cargo run -p ruvyxa_cli -- analyze --root examples/basic-app
cargo run -p ruvyxa_cli -- build --root examples/basic-app
cargo run -p ruvyxa_cli -- start --root examples/basic-app
```

`analyze` validates route exports, client/server boundaries, and private env usage before production builds. The production server reads `.ruvyxa/server/app`, serves assets from `.ruvyxa/assets`, and uses the same route matching path as the dev server.

## Styling With Tailwind

Ruvyxa apps include Tailwind CSS v4 support in `app/global.css`:

```css
@import "tailwindcss";

@source "../app";
@source "../components";
```

Install dependencies once with `pnpm install`. Ruvyxa runs the local Tailwind CLI when a CSS file imports `tailwindcss`, then injects the compiled CSS into rendered pages.

## Environment Variables

Put documented keys in `.env.example` and local values in `.env` or `.env.local`.

```env
RUVYXA_PUBLIC_APP_NAME=Ruvyxa
DATABASE_URL=postgres://user:password@localhost:5432/ruvyxa
```

Ruvyxa loads `.env` and then `.env.local` into server-side renderers for SSR, API routes, and actions. Only `RUVYXA_PUBLIC_*` variables are allowed in client-reachable code.

## React SSR

Ruvyxa renders `page.tsx` through ReactDOMServer. Dynamic route params are passed to page components:

```tsx
export default function BlogPost({ params }: { params: { slug: string } }) {
  return <h1>{params.slug}</h1>
}
```

API routes are executed from `route.ts`:

```ts
export function GET() {
  return Response.json({ ok: true })
}
```

## Add an Action

Create `app/todos/action.ts`:

```ts
import { action } from "ruvyxa/server"

export const createTodo = action
  .input({ parse: (value: any) => ({ title: String(value.title).trim() }) })
  .handler(async ({ input, invalidate }) => {
    invalidate("todos")
    return { title: input.title, completed: false }
  })
```

Then post to it from the matching route:

```tsx
<form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
  <input name="title" />
  <button type="submit">Create todo</button>
</form>
```
