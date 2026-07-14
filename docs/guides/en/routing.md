# Routing

Routes come directly from folders under `app/`. There is no second route pattern to write or
remember: a folder's bracket name is the route contract.

```text
app/blog/[slug]/page.tsx
app/docs/[...path]/page.tsx
app/shop/[[...path]]/page.tsx
```

## Dynamic Segments

`[name]` captures one required segment as a `string`.

```tsx
import type { PageProps } from 'ruvyxa/config'

export default function BlogPost({ params }: PageProps<{ slug: string }>) {
  return <h1>Post: {params.slug}</h1>
}
```

`[...path]` captures one or more remaining segments as `string[]`.

```tsx
export default function Docs({ params }: PageProps<{ path: string[] }>) {
  return <h1>{params.path.join('/')}</h1>
}
```

`[[...path]]` also matches its parent route. Its value is `undefined` at the parent and `string[]`
when segments are present.

```tsx
export default function Shop({ params }: PageProps<{ path?: string[] }>) {
  return <h1>{params.path?.join('/') ?? 'All products'}</h1>
}
```

Ruvyxa keeps `params` synchronous. This matches the framework's current server/client renderer
contract; it does not claim Next.js's RSC-based Promise params API.

## Route Groups

Use `(...)` to organize files without adding a URL segment:

```text
app/(marketing)/pricing/page.tsx
```

Folders beginning with `_` or `@` are ignored. Ruvyxa rejects ambiguous route shapes, including
dynamic siblings such as `[id]` and `[slug]`.

```bash
npx ruvyxa analyze
npx ruvyxa routes
```

See [API Routes](api-routes.md) for `route.ts` handlers, which receive the same parameter shapes.
