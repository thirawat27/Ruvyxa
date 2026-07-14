# Routing

Ruvyxa สร้าง route จากโฟลเดอร์ใน `app/` โดยตรง จึงไม่มี syntax ของ URL อีกชุดที่ต้องจำ:

```text
app/blog/[slug]/page.tsx
app/docs/[...path]/page.tsx
app/shop/[[...path]]/page.tsx
```

## Dynamic Segments

- `[name]` ได้ค่า `string`
- `[...path]` ได้ค่า `string[]` ที่มีอย่างน้อยหนึ่ง segment
- `[[...path]]` ได้ `string[] | undefined` และเข้าถึง parent route ได้ด้วย

```tsx
import type { PageProps } from 'ruvyxa/config'

export default function Docs({ params }: PageProps<{ path: string[] }>) {
  return <h1>{params.path.join('/')}</h1>
}
```

```tsx
export default function Shop({ params }: PageProps<{ path?: string[] }>) {
  return <h1>{params.path?.join('/') ?? 'All products'}</h1>
}
```

`params` ของ Ruvyxa เป็น synchronous ตาม contract ของ renderer ปัจจุบัน จึงไม่ได้อ้างว่า รองรับ
Promise params ของ Next.js ซึ่งต้องใช้ RSC transport.

## Route Groups

ใช้ `(...)` จัดโครงสร้างไฟล์โดยไม่เพิ่ม URL segment เช่น `app/(marketing)/pricing/page.tsx`

โฟลเดอร์ที่ขึ้นต้นด้วย `_` หรือ `@` จะถูก ignore และ Ruvyxa จะปฏิเสธ route ที่ ambiguous เช่น `[id]`
กับ `[slug]` ในตำแหน่งเดียวกัน

```bash
npx ruvyxa analyze
npx ruvyxa routes
```

ดูเพิ่มเติม: [API Routes](api-routes.md)
