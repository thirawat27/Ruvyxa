# Data Loading & Cache

## Loaders

Use `loader` to create a server-side data-fetching function. Call it from a server page or
server-only module:

```ts
// app/products/server.ts
import { cache, loader } from 'ruvyxa/server'

export const getProducts = loader(async () => {
  return cache('products:list')
    .ttl('5m')
    .swr('1m')
    .get(() => database.products.findMany())
})
```

Consume in a page:

```tsx
// app/products/page.tsx
import { getProducts } from './server'

export default async function ProductsPage() {
  const products = await getProducts()
  return <pre>{JSON.stringify(products, null, 2)}</pre>
}
```

## Cache API

`cache(key)` creates an in-memory cache entry with TTL:

```ts
import { cache } from 'ruvyxa/server'

// Basic TTL cache
const data = await cache('my-key')
  .ttl('30s')
  .get(() => fetchData())

// With stale-while-revalidate
const data = await cache('my-key')
  .ttl('5m')
  .swr('1m')
  .get(() => fetchData())
```

### TTL Duration Format

| Value | Meaning    |
| ----- | ---------- |
| `30s` | 30 seconds |
| `5m`  | 5 minutes  |
| `1h`  | 1 hour     |
| `1d`  | 1 day      |

### Cache Keys

Keys should identify the resource and its scope:

```text
product:123
products:category:books
user:456:sessions
```

## Cache Invalidation

After a mutation call `invalidateCache(key)` or use the action context's `invalidate(key)`:

```ts
import { invalidateCache } from 'ruvyxa/server'

// Invalidate a specific key
invalidateCache('products:list')

// Invalidate all
invalidateCache()
```

From an action handler:

```ts
.handler(async ({ input, invalidate }) => {
  invalidate('todos')
  invalidate('user:123')
  return result
})
```

## Stale-While-Revalidate (SWR)

SWR improves response times for moderately stale data:

- When TTL expires but SWR hasn't → serve stale data, refresh in background.
- When SWR expires → fetch fresh data and cache.

```ts
const data = await cache('weather:current')
  .ttl('10m') // keep for 10 minutes
  .swr('1h') // serve stale up to 1 hour while revalidating
  .get(() => fetchWeather())
```

## Best Practices

1. Place loaders in `server.ts` files next to the routes that use them.
2. Set TTL based on data volatility — fast-changing data needs short TTLs.
3. Use `swr()` for data that tolerates brief staleness.
4. Always invalidate the cache after a mutation.
5. Use descriptive cache keys such as `user:email`, not `key1`.

See [Server Actions](server-actions.md) for mutations with cache invalidation.
