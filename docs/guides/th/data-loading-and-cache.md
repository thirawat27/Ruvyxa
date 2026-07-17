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

## โหลดข้อมูลฝั่ง Client ด้วย `useRuvyxaLoader`

ใช้ `useRuvyxaLoader` จาก `@ruvyxa/react` เมื่อข้อมูลต้องโหลดในเบราว์เซอร์ เช่น ข้อมูลที่ขึ้นอยู่กับ
ค่าจากฝั่ง client หรือข้อมูลที่ต้อง refresh โดยไม่เปลี่ยนหน้าเต็ม:

```tsx
'use client'

import { useRuvyxaLoader } from '@ruvyxa/react'

export function UserProfile({ userId }: { userId: string }) {
  const { data, loading, error, refetch } = useRuvyxaLoader(
    () => fetch(`/api/users/${userId}`).then((response) => response.json()),
    { deps: [userId] },
  )

  if (loading) return <p>กำลังโหลด...</p>
  if (error) return <p>โหลดข้อมูลไม่สำเร็จ: {error.message}</p>

  return (
    <section>
      <pre>{JSON.stringify(data, null, 2)}</pre>
      <button type="button" onClick={refetch}>
        โหลดใหม่
      </button>
    </section>
  )
}
```

การทำงานหลัก:

- โหลดข้อมูลอัตโนมัติเมื่อ component เริ่มทำงาน
- โหลดใหม่เมื่อค่าที่อยู่ใน `deps` เปลี่ยน
- ใช้ `refetch()` เพื่อโหลดใหม่ด้วยตนเอง
- ใช้ `{ enabled: false }` เพื่อปิดการโหลดอัตโนมัติ
- คืนค่า `data`, `loading`, `error` และ `refetch`
- ป้องกัน request เก่าทับข้อมูลใหม่ และไม่อัปเดต state หลัง component ถูกถอดออก

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
