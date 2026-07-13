# Data Loading & Cache

## Loader

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

## Cache API

```ts
import { cache } from 'ruvyxa/server'

const data = await cache('my-key')
  .ttl('5m')
  .swr('1m')
  .get(() => fetchData())
```

### TTL Format

| Value | ความหมาย  |
| ----- | --------- |
| `30s` | 30 วินาที |
| `5m`  | 5 นาที    |
| `1h`  | 1 ชั่วโมง |
| `1d`  | 1 วัน     |

## Invalidate

```ts
import { invalidateCache } from 'ruvyxa/server'

invalidateCache('products:list')
invalidateCache() // clear all
```

หรือผ่าน action:

```ts
.handler(async ({ input, invalidate }) => {
  invalidate('todos')
})
```

ดูเพิ่มเติม: [Server Actions](server-actions.md)
