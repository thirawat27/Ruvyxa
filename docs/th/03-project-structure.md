# โครงสร้างโปรเจกต์

> **เป้าหมายของ tutorial:** วาง route file ในตำแหน่งที่ Ruvyxa ค้นพบได้ และเพิ่มหน้าที่สมบูรณ์หนึ่งหน้า **เริ่มจาก:**
> แอปที่ทำงานได้จาก [สร้าง app แรก](02-create-your-first-app.md) **Checkpoint:** รายการ route
> แสดงหน้าที่คุณเพิ่ม

Ruvyxa ค้นหา route จาก `appDir` ที่ตั้งค่าไว้ (`app` เมื่อกำหนดแบบ template) ชื่อไฟล์บอกพฤติกรรม route ส่วน
JavaScript export บอกพฤติกรรม rendering

```text
app/
├── layout.tsx                 # shared shell
├── page.tsx                   # GET /
├── about/
│   └── page.tsx               # GET /about
├── blog/
│   ├── page.tsx               # GET /blog
│   └── [slug]/page.tsx        # GET /blog/:slug
├── api/
│   └── health/route.ts        # API handler at /api/health
└── showcase/
    ├── error.tsx              # nearest render-error boundary
    ├── loading.tsx            # loading boundary
    └── not-found.tsx          # nearest not-found UI
```

## Route file

| ไฟล์             | วัตถุประสงค์ที่มี implementation                     |
| --------------- | ---------------------------------------------- |
| `page.tsx`      | page route component                           |
| `layout.tsx`    | layout ที่ประกอบตาม route path                   |
| `route.ts`      | API route module ที่ export HTTP method function |
| `loading.tsx`   | loading component ที่ค้นหาพร้อม route boundary     |
| `error.tsx`     | error component ที่รับ `{ error, reset }`         |
| `not-found.tsx` | UI ที่ใกล้ที่สุดสำหรับ framework not-found signal     |

dynamic folder ใช้ `[name]`, catch-all ใช้ `[...name]`, และ optional catch-all ใช้ `[[...name]]` demo
มีตัวอย่าง `[slug]` และ `[...slug]` วาง server code เฉพาะ route ไว้ข้าง route ได้เมื่อมันไม่ถูก import จาก
client module; validation บังคับ server/client boundary

## หน้าขนาดเล็กที่สมบูรณ์

```tsx
// app/about/page.tsx
import type { PageProps } from 'ruvyxa'

export default function About({ requestPath }: PageProps) {
  return (
    <main>
      <h1>About</h1>
      <p>Rendered for {requestPath}</p>
    </main>
  )
}
```

`PageProps.params` มีค่าของ dynamic segment; `requestPath` คือ concrete request path ใช้ `layout.tsx`
สำหรับ composition ระดับเอกสาร แทนที่จะทำ markup ซ้ำในทุก page

**ก่อนหน้า:** [สร้าง app แรก](02-create-your-first-app.md) · **ถัดไป:**
[Routing และ rendering](04-routing-rendering.md)
