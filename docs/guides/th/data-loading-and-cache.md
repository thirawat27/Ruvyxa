# Data Loading & Cache

> 🟡 **ระดับกลาง** · ⏱️ อ่าน ~8 นาที
>
> **จะได้เรียนรู้:** ดึงข้อมูลฝั่ง server ด้วย `loader`, cache ผลลัพธ์ที่แพง และ refresh แบบ SWR

บทนี้ว่าด้วยการ**อ่าน**ข้อมูลฝั่ง server — ดึงจากฐานข้อมูลหรือ API ก่อนหน้า render และ cache
ผลลัพธ์ให้ request ซ้ำเร็วขึ้น ส่วนการ**เขียน**ข้อมูล (submit form, mutation) ดูที่
[Server Actions](server-actions.md)

## Loaders

ใช้ `loader` เพื่อสร้างฟังก์ชันดึงข้อมูลฝั่ง server เรียกใช้จาก server page หรือ server-only module:

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

ใช้ใน page:

```tsx
// app/products/page.tsx
import { getProducts } from './server'

export default async function ProductsPage() {
  const products = await getProducts()
  return <pre>{JSON.stringify(products, null, 2)}</pre>
}
```

## Client-Side Data Loading

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

Hook ทำงานอัตโนมัติเมื่อ mount กำหนด `deps` เป็นค่าที่ควร trigger request ใหม่ และเรียก `refetch()`
เพื่อโหลดซ้ำด้วยตนเอง ใช้ `enabled: false` เพื่อปิดการโหลด:

```tsx
const result = useRuvyxaLoader(loadPreview, { enabled: false })
```

ผลลัพธ์ประกอบด้วย:

- `data`: ค่าสำเร็จล่าสุด หรือ `undefined` หากยังไม่มี
- `loading`: กำลังมี request ทำงานอยู่หรือไม่
- `error`: error จาก loader หาก request ล้มเหลว
- `refetch`: เริ่ม request ใหม่เมื่อ hook enabled

`useRuvyxaLoader` ยังละเลย request เก่าเมื่อ dependencies เปลี่ยน และหลีกเลี่ยงการอัปเดต state หลัง
component unmount

## Cache API

`cache(key)` สร้าง cache entry ในหน่วยความจำแบบมี TTL:

```ts
import { cache } from 'ruvyxa/server'

// Basic TTL cache
const data = await cache('my-key')
  .ttl('30s')
  .get(() => fetchData())

// พร้อม stale-while-revalidate
const data = await cache('my-key')
  .ttl('5m')
  .swr('1m')
  .get(() => fetchData())
```

### TTL Duration Format

| Value | ความหมาย  |
| ----- | --------- |
| `30s` | 30 วินาที |
| `5m`  | 5 นาที    |
| `1h`  | 1 ชั่วโมง |
| `1d`  | 1 วัน     |

### Cache Keys

Keys ควรระบุ resource และ scope:

```text
product:123
products:category:books
user:456:sessions
```

## Cache Invalidation

หลัง mutation เรียก `invalidateCache(key)` หรือใช้ `invalidate(key)` จาก action context:

```ts
import { invalidateCache } from 'ruvyxa/server'

// Invalidate เฉพาะ key
invalidateCache('products:list')

// Invalidate ทั้งหมด
invalidateCache()
```

จาก action handler:

```ts
.handler(async ({ input, invalidate }) => {
  invalidate('todos')
  invalidate('user:123')
  return result
})
```

## Stale-While-Revalidate (SWR)

SWR เพิ่มความเร็วสำหรับข้อมูลที่อาจเก่าเล็กน้อย:

- เมื่อ TTL หมดอายุแต่ SWR ยังไม่หมด → serve ข้อมูลเก่า, refresh ใน background
- เมื่อ SWR หมดอายุ → ดึงข้อมูลใหม่และ cache

```ts
const data = await cache('weather:current')
  .ttl('10m') // เก็บไว้ 10 นาที
  .swr('1h') // serve stale ได้นาน 1 ชั่วโมงระหว่าง revalidate
  .get(() => fetchWeather())
```

## Best Practices

1. วาง loaders ในไฟล์ `server.ts` ข้าง routes ที่ใช้
2. ตั้ง TTL ตามความถี่ที่ข้อมูลเปลี่ยน — ข้อมูลที่เปลี่ยนเร็วต้องการ TTL สั้น
3. ใช้ `swr()` สำหรับข้อมูลที่ทน stale ได้บ้าง
4. invalidate cache ทุกครั้งหลัง mutation
5. ใช้ cache key ที่สื่อความหมาย เช่น `user:email` ไม่ใช่ `key1`

ดูเพิ่มเติม: [Server Actions](server-actions.md) สำหรับ mutations พร้อม cache invalidation
