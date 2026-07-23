# Rendering Strategies

> 🟢 **เหมาะกับมือใหม่** · ⏱️ อ่าน ~8 นาที
>
> **จะได้เรียนรู้:** วิธี render หน้าเว็บทั้ง 5 แบบ, เลือกด้วยคำถามเดียว และประกาศแต่ละแบบยังไง —
> ค่า default ที่ปลอดภัยคือไม่ต้องประกาศอะไรเลย

## ควรใช้ตัวไหน?

เพิ่งเริ่มต้น? ตอบคำถามเดียว — _HTML ของหน้านี้ควรถูกสร้างตอนไหน?_ — แล้วเลือกจากตาราง
ไม่ต้องตั้งค่ารวมทั้งแอป แต่ละหน้าประกาศ (หรือถูก detect) ของตัวเอง

| หน้าของคุณเป็นแบบ…                                          | ใช้     | วิธี                                                |
| ----------------------------------------------------------- | ------- | --------------------------------------------------- |
| เหมือนกันทุกคน แทบไม่เปลี่ยน (about, เอกสาร)                | **SSG** | ไม่ต้องทำอะไร — หน้า static ถูก detect ให้อัตโนมัติ |
| ข้อมูลสดทุก request (dashboard, ผลค้นหา)                    | **SSR** | ไม่ต้องทำอะไร — หน้าที่ใช้ข้อมูล request เป็น SSR   |
| ส่วนใหญ่ static แต่อยาก refresh เป็นระยะ (หน้ารวม blog)     | **ISR** | `export const revalidate = 60`                      |
| Interactive หนัก ทำงานใน browser ล้วน (editor, canvas, เกม) | **CSR** | `'use client'` บรรทัดแรกของไฟล์                     |
| Shell แบบ static + บางส่วนช้า/dynamic                       | **PPR** | `export const ppr = true` + `<Suspense>`            |

ไม่แน่ใจ? ไม่ต้องทำอะไรเลย — Ruvyxa เลือก SSG ให้หน้า static และ SSR ให้หน้า dynamic
ซึ่งถูกต้องสำหรับหน้าส่วนใหญ่ อยากรู้ว่าแต่ละหน้าได้ strategy อะไร รัน `npx ruvyxa routes`
ได้ทุกเมื่อ

## Detection Order

Ruvyxa เลือก rendering strategy ให้แต่ละเพจ ลำดับการตรวจสอบมีความสำคัญ — **rule แรกที่ match
จะถูกใช้**

| ลำดับ | Declaration                           | Strategy | การใช้งานที่เหมาะสม                           |
| ----- | ------------------------------------- | -------- | --------------------------------------------- |
| 1     | `'use client'`                        | CSR      | หน้าแบบ browser-only หรือ interactive สูง     |
| 2     | `export const ppr = true`             | PPR      | Shell แบบ static + dynamic `Suspense` regions |
| 3     | `export const revalidate = 60`        | ISR      | เนื้อหาที่ refresh หลังจากช่วงเวลาที่กำหนด    |
| 4     | `getStaticParams` หรือ `staticParams` | SSG      | Dynamic paths ที่รู้ล่วงหน้า ณ build time     |
| 5     | Static route (ไม่มี dynamic markers)  | SSG      | หน้า stable และ content                       |
| 6     | Default                               | SSR      | ข้อมูล ณ เวลา request — default ที่ปลอดภัย    |

## SSR — Server-Side Rendering (Default)

Rendered ทุก request:

```tsx
export default async function ProductPage() {
  const products = await db.products.findMany()
  return <ProductList items={products} />
}
```

## SSG — Static Site Generation

### Static pages

Static routes ที่ไม่มี dynamic data markers และไม่มี `'use client'` จะถูก auto-detect เป็น SSG โดยจะ
pre-render ตอน build และ serve เป็น static HTML

### Direct parameters ด้วย `staticParams`

ถ้ารู้ค่าล่วงหน้า export โดยไม่ต้องใช้ function ได้ รองรับ scalar shorthand เมื่อ route มี dynamic
segment เดียว:

```tsx
// app/articles/[slug]/page.tsx
export const staticParams = ['getting-started', 'deployment']
```

Object สำหรับ routes ที่มีหลาย dynamic segments:

```tsx
export const staticParams = [
  { category: 'guides', slug: 'getting-started' },
  { category: 'news', slug: 'release-1-0-15' },
]
```

### Asynchronous parameters ด้วย `getStaticParams`

สำหรับ dynamic routes ที่รู้ path ณ build time:

```tsx
// app/articles/[slug]/page.tsx
import type { GetStaticParams, PageProps } from 'ruvyxa/config'

export const getStaticParams: GetStaticParams<{ slug: string }> = async ({ route, routes }) => {
  console.log(`Generating ${route.path}; ${routes.length} routes discovered`)
  return ['getting-started', 'deployment']
}

export default function Article({ params }: PageProps<{ slug: string }>) {
  return <article>{params.slug}</article>
}
```

context ประกอบด้วย path ปัจจุบัน, ข้อมูล dynamic segments และทุก `{ path, id }` route entries
ที่ค้นพบ ใช้ object entries เมื่อ route มีหลาย dynamic segments สำหรับ catch-all segment scalar
shorthand จะกลายเป็น string array หนึ่งสมาชิก

### Persistent parameter cache

การค้นหา parameters ที่มีต้นทุนสูงสามารถเปิด persistent TTL cache ได้:

```tsx
export const getStaticParams: GetStaticParams<{ slug: string }> = async () => {
  const posts = await fetchPosts()
  return {
    params: posts.map((post) => post.slug),
    cache: '10m',
  }
}
```

`cache` รับจำนวนวินาทีหรือ duration `s`, `m`, `h`, `d` ตั้งแต่ 1 วินาทีถึง 365 วัน รายการ parameters
ที่ถูก cache จะถูกใช้ซ้ำจนกว่า TTL จะหมดอายุ การเปลี่ยนแปลงที่ page, dependency ใดๆ, route metadata
หรือ route manifest จะ invalidate ก่อนเวลา หาก return array โดยตรงจะยังคงไม่ cache เช่นเดิม

#### Constraints

- Scalar entries ต้องมี dynamic segment เพียง segment เดียว มิฉะนั้นต้องเป็น object ที่มีค่าครบทุก
  required dynamic segment
- ค่าต้องไม่มี path traversal, query หรือ fragment characters (`..`, `/`, `\`, `?`, `#`)
- ผลลัพธ์ที่ generate จะอยู่ภายใน `.ruvyxa/prerender`

## ISR — Incremental Static Regeneration

สำหรับข้อมูลที่อาจล้าสมัยแต่ไม่ต้อง render ทุก request:

```tsx
export const revalidate = 60 // seconds

export default async function ProductPage() {
  return <main>Product data refreshed after at most 60 seconds.</main>
}
```

Cached output ยังใช้ได้ระหว่าง regenerate Ruvyxa จะเริ่มงาน background หลังจากครบช่วงเวลาที่กำหนด
และรวม request พร้อมกันของ route เดียวกันเป็นการ refresh ครั้งเดียว

## PPR — Partial Pre-rendering

Static shell + dynamic `Suspense` regions:

```tsx
export const ppr = true

export default function PPRPage() {
  return (
    <main>
      <h1>Static Shell</h1>
      <Suspense fallback={<p>Loading…</p>}>
        <DynamicContent />
      </Suspense>
    </main>
  )
}
```

เฉพาะ static shell เท่านั้นที่ pre-render; dynamic slots จะถูก stream ณ request time

## CSR — Client-Side Rendering

```tsx
'use client'

import { useState, useEffect } from 'react'

export default function InteractiveDashboard() {
  const [data, setData] = useState(null)
  useEffect(() => {
    fetch('/api/dashboard')
      .then((r) => r.json())
      .then(setData)
  }, [])
  // ...
}
```

ณ build time HTML shell แบบ minimal จะถูก emit สำหรับ CSR routes

## หน้า Zero-JS — `export const hydrate = false`

หน้า server-rendered ทุกแบบ (SSR, SSG, ISR, PPR) เลือกปิด client hydration ได้ทั้งหมด:

```tsx
// app/terms/page.tsx — ส่ง JavaScript ไป browser ศูนย์ไบต์
export const hydrate = false

export default function TermsPage() {
  return (
    <main>
      <h1>ข้อกำหนดการใช้งาน</h1>
      <p>เนื้อหาล้วน — ไม่มี React runtime ไม่มี hydration bundle</p>
    </main>
  )
}
```

สิ่งที่เปลี่ยนสำหรับหน้านั้น:

- HTML ที่ serve และ prerender **ไม่มี `<script>` เลย** (โหมด dev เหลือเฉพาะ HMR reload client)
- Production build **ข้ามการสร้าง client bundle** ของ route นั้น — ไม่ emit ไม่ ship
- หน้านั้น interactive ไม่ได้: island `'use client'` ข้างในจะ render HTML ฝั่ง server แต่ไม่ hydrate
  — event handler และ state ไม่ทำงาน

เหมาะกับเนื้อหาที่ไม่ต้องการ JavaScript — terms, privacy, changelog, blog post, docs
เป็นการตัดสินใจรายหน้า จึงผสมหน้า content แบบ zero-JS กับหน้า interactive เต็มรูปในแอปเดียวได้อิสระ
หน้า `'use client'` (CSR) จะไม่สน export นี้ — directive ชนะเสมอ

## Pre-render Output

SSG, ISR, PPR และ CSR routes จะถูก pre-render ณ build time:

```text
.ruvyxa/prerender/
├── manifest.json          # route list พร้อม strategy และ revalidate
├── index.html             # /
├── about/index.html       # /about
└── blog/
    └── hello-world/
        └── index.html     # /blog/hello-world
```

## Best Practices

1. ให้ SSR เป็น default — เลือกใช้ strategy อื่นเมื่อมีเหตุผลชัดเจนเท่านั้น
2. ใช้ explicit export (`ppr`, `revalidate`, `staticParams`, `getStaticParams`) สำหรับ routes ที่
   deployment behaviour สำคัญ
3. ตรวจสอบ strategy ที่ถูก detect ด้วย `npx ruvyxa routes`
4. ตรวจสอบโครงสร้าง route ด้วย `npx ruvyxa analyze`
5. Static parameters ควรอธิบาย paths ที่รู้แน่นอน ณ build time; cache เฉพาะงานค้นหาที่ผลลัพธ์
   ปลอดภัยที่จะคงเดิมในช่วง TTL ที่เลือก
