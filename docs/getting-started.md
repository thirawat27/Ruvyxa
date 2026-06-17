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
cargo run -p ruvyxa_cli -- build --root examples/basic-app
cargo run -p ruvyxa_cli -- start --root examples/basic-app
```

The production server reads `.ruvyxa/app` and uses the same route matching path as the dev server.

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
