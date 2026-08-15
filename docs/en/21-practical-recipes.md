# Practical recipes

> **Tutorial goal:** extend the starter app by adapting one complete, source-backed pattern at a
> time. **Start from:** the matching foundational chapter linked by each recipe. **Checkpoint:**
> copy one recipe, replace its placeholder data, and run the application check.

Each recipe uses a public API or route convention implemented by this repository. Copy the shown
file into an application that already completed
[Create your first app](02-create-your-first-app.md), then run `npm run check` before building.

## 1. Static dynamic pages

Use `getStaticParams` for each dynamic path you want produced during the build.

```tsx
// app/guides/[slug]/page.tsx
import type { GetStaticParams, PageProps } from 'ruvyxa'

export const getStaticParams: GetStaticParams<{ slug: string }> = () => [
  { slug: 'getting-started' },
  { slug: 'deployment' },
]

export default function Guide({ params }: PageProps<{ slug: string }>) {
  return (
    <main>
      <h1>Guide: {params.slug}</h1>
    </main>
  )
}
```

Run `npm run build`; the concrete paths are pre-render candidates. Use an object result with
`{ params, cache: '10m' }` when parameter discovery itself should be cached. Do not use this pattern
for values known only after a user-specific request.

## 2. Validate an API request and return useful status codes

```ts
// app/api/messages/route.ts
export async function POST({ request }: { request: Request }) {
  let body: unknown
  try {
    body = await request.json()
  } catch {
    return Response.json({ error: 'Invalid JSON' }, { status: 400 })
  }
  if (!body || typeof body !== 'object' || typeof (body as { text?: unknown }).text !== 'string') {
    return Response.json({ error: 'text must be a string' }, { status: 400 })
  }
  const text = (body as { text: string }).text.trim()
  if (!text || text.length > 500)
    return Response.json({ error: 'text must be 1–500 characters' }, { status: 422 })
  return Response.json({ id: crypto.randomUUID(), text }, { status: 201 })
}
```

Keep the body limit in `security.apiLimit`; it protects memory, while this handler protects the
meaning of the input. Test with valid JSON, invalid JSON, empty text, and an overlong value.

## 3. Cache a loader and invalidate after a write

```ts
// app/tasks/server.ts
import { action, cache, invalidateCache, loader } from 'ruvyxa/server'

export const listTasks = loader(({ cache }) =>
  cache('tasks:list')
    .ttl('30s')
    .swr('30s')
    .get(async () => [{ id: 'example', title: 'Write docs' }]),
)

export const createTask = action
  .input({
    parse(value: unknown) {
      if (
        !value ||
        typeof value !== 'object' ||
        typeof (value as { title?: unknown }).title !== 'string'
      )
        throw new Error('title is required')
      return { title: (value as { title: string }).title.trim() }
    },
  })
  .handler(({ input, invalidate }) => {
    if (!input.title) throw new Error('title is required')
    invalidate('tasks')
    invalidateCache('tasks')
    return { id: crypto.randomUUID(), ...input }
  })
```

`invalidate('tasks')` is action invalidation metadata; `invalidateCache('tasks')` clears the
process-local cache key/prefix. Use a shared data/cache design when multiple processes must agree.

## 4. Client data loading with recoverable UI

```tsx
// app/messages/page.tsx
'use client'
import { useRuvyxaLoader } from '@ruvyxa/react'

type Message = { id: string; text: string }
export default function Messages() {
  const { data, loading, error, refetch } = useRuvyxaLoader<Message[]>(async () => {
    const response = await fetch('/api/messages')
    if (!response.ok) throw new Error(`Request failed: ${response.status}`)
    return response.json() as Promise<Message[]>
  })
  if (loading) return <p>Loading messages…</p>
  if (error) return <button onClick={refetch}>Retry: {error.message}</button>
  return (
    <ul>
      {data?.map((message) => (
        <li key={message.id}>{message.text}</li>
      ))}
    </ul>
  )
}
```

Add `{ deps: [conversationId] }` when a changing value should trigger a fetch. Keep server-rendered
content out of `useSearchParams`-dependent markup because its values are client-only during SSR.

## 5. Accessible navigation, metadata, and images

```tsx
// app/products/page.tsx
import { Image, Link, Seo } from '@ruvyxa/react'

export const meta = { title: 'Products', description: 'Example product catalog' }
export default function Products() {
  return (
    <main>
      <Seo title="Products" canonical="https://app.example.com/products" />
      <Link href="/" prefetch="viewport">
        Back home
      </Link>
      <Image
        src="/product.jpg"
        alt="Example product"
        width={1200}
        height={800}
        sizes="(max-width: 768px) 100vw, 1200px"
        priority
      />
    </main>
  )
}
```

Do not set the same title in both `meta` and `<Seo>` unless duplicate metadata is intentional.
`Link` remains a real anchor before hydration. Put `product.jpg` under `public/`; production builds
can create WebP variants for local PNG/JPEG assets.

## 6. Add route-scoped policy without duplicating handlers

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { cacheRules, headers, securityHeaders } from 'ruvyxa/plugins'

export default config({
  plugins: [
    headers([{ source: '/api/*', headers: { 'x-content-type-options': 'nosniff' } }]),
    cacheRules([
      { source: '/assets/*', browser: 'public, max-age=3600', cdn: 'public, max-age=86400' },
    ]),
    securityHeaders({
      routes: ['/admin/*'],
      contentSecurityPolicy: { 'default-src': ["'self'"] },
      frameOptions: 'DENY',
    }),
  ],
})
```

Patterns are exact or trailing-star prefixes. Test one matching and one non-matching route; a cache
rule must set at least browser, CDN, or `vary`.

## 7. Test server primitives without a running server

```ts
import assert from 'node:assert/strict'
import test from 'node:test'
import { mockAction, mockCache } from '@ruvyxa/testing'

test('a write records its invalidation', async () => {
  const save = mockAction(({ input, invalidate }) => {
    invalidate('tasks')
    return input
  })
  await save({ title: 'Release' })
  assert.deepEqual(save.invalidations, ['tasks'])
})

test('a cache producer runs once for a hit', async () => {
  const cache = mockCache({ 'tasks:list': ['saved'] })
  const value = await cache('tasks:list')
    .ttl('30s')
    .get(() => ['new'])
  assert.deepEqual(value, ['saved'])
  assert.equal(cache.calls[0]?.hit, true)
})
```

Run the package's test script or `node --test` according to your application setup. Mocks verify
your action/loader contract; they do not replace an HTTP integration test.

## 8. Add release controls that fail early

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { bundleBudget, requireEnv } from 'ruvyxa/plugins'

export default config({
  build: { minify: true, map: false, split: 'route' },
  plugins: [
    requireEnv(['DATABASE_URL', 'RUVYXA_AUTH_SECRET']),
    bundleBudget({ maxChunkKb: 250, maxTotalKb: 800 }),
  ],
})
```

`requireEnv` fails a production build if a named value is absent/empty. `bundleBudget` measures
final minified client JavaScript. Run the four release commands in
[Release-readiness playbook](19-release-readiness-playbook.md), then choose the artifact from
[Platform adapter guide](20-platform-adapter-guide.md).

## 9. Declare a known set of modules with `import.meta.glob`

```tsx
// app/guides/page.tsx
const lazyGuides = import.meta.glob('./guides/*.mdx')
const eagerIcons = import.meta.glob('./icons/*.tsx', { eager: true })

export default async function GuidesIndex() {
  const slugs = Object.keys(lazyGuides).map((path) => path.split('/').pop()!.replace('.mdx', ''))
  return (
    <main>
      <ul>
        {slugs.map((slug) => (
          <li key={slug}>{slug}</li>
        ))}
      </ul>
    </main>
  )
}
```

The pattern and the `{ eager: true }` option must be compile-time literals — a variable pattern or a
computed option is a build diagnostic, not a runtime fallback. A lazy match's generated key maps to
`() => import(...)`, so nothing under it is evaluated until a caller invokes that loader; an eager
match becomes a hoisted static import and enters the same dependency graph, chunking, and
tree-shaking as an ordinary `import` statement. Keys are project-relative, slash-normalized
specifiers in a deterministic, locale-independent order, so the same source produces the same keys
on every machine. A pattern is resolved from the importing file and cannot resolve outside the
project root — `import.meta.glob('../../secret/*.ts')` is a build error, not a partial result.
Aliases from `tsconfig.json`/`jsconfig.json` `paths` work the same as they do in an ordinary import.

**Previous:** [Platform adapter guide](20-platform-adapter-guide.md) · **Next:**
[Documentation index](README.md)
