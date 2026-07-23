# Server & Client Components

> 🟡 **ระดับกลาง** · ⏱️ อ่าน ~8 นาที
>
> **จะได้เรียนรู้:** โค้ดของคุณรันอยู่ "สองโลก" ไหนบ้าง, เมื่อไหร่ต้องเขียน `'use client'` และ
> Ruvyxa กัน secret ไม่ให้รั่วไป browser ยังไง
>
> ยังไม่ได้อ่าน [Routing](routing.md) แนะนำอ่านก่อน

## วิธีคิด

แอป Ruvyxa มี **สองโลก**:

- **โลก server** — รันบนเครื่อง/โฮสต์ของคุณ อ่านฐานข้อมูล ใช้ secret แตะไฟล์ได้ โค้ดในโลกนี้
  _ไม่มีวัน_ ถูกส่งไป browser
- **โลก client** — รันใน browser ของผู้ใช้ ใช้ `useState`, click handler, browser API ได้
  ทุกอย่างในโลกนี้ถูกส่งเป็น JavaScript ให้ผู้เข้าชมทุกคน

หน้าเริ่มต้นในโลก server คุณเลือกย้าย component ทีละตัวเข้าโลก client ด้วย directive เดียว — และ
framework **ตรวจเส้นแบ่งตอน build** secret จึงรั่วโดยบังเอิญไม่ได้

```text
โลก server  (default)                  โลก client  ('use client')
─────────────────────────              ─────────────────────────────
app/page.tsx                    →      app/_components/Counter.tsx
  อ่าน db, env, ไฟล์                     useState, onClick, window
  render HTML                            hydrate ใน browser
```

## Default: Server Components

หน้าเป็น server-rendered โดย default โค้ดทั้งหมดอยู่บน server — ไม่มีอะไรเข้า browser bundle
เว้นแต่ระบุชัดเจน หน้า Ruvyxa จึงอ่านข้อมูลตรงๆ ได้:

```tsx
// app/products/page.tsx — server component แตะข้อมูล server ได้ปลอดภัย
import { db } from '../../lib/db'

export default async function ProductsPage() {
  const products = await db.products.findMany({ take: 20 })
  return (
    <ul>
      {products.map((product) => (
        <li key={product.id}>{product.title}</li>
      ))}
    </ul>
  )
}
```

## Client Components

ใส่ directive `'use client'` **เฉพาะ** module ที่ต้องใช้:

- Browser API (`window`, `document`, `localStorage` ฯลฯ)
- React state / effect (`useState`, `useEffect`, `useReducer`)
- Event handler (`onClick`, `onChange` ฯลฯ)

```tsx
'use client'

import { useState } from 'react'

export default function Counter() {
  const [count, setCount] = useState(0)
  return <button onClick={() => setCount((value) => value + 1)}>{count}</button>
}
```

### ประกอบร่าง: หน้า server + เกาะ client

Pattern ที่ใช้ทุกวัน — หน้า server ดึงข้อมูลและ render HTML เกือบ static พร้อม client component
เล็กๆ แทรกเฉพาะจุด interactive:

```tsx
// app/products/page.tsx  (server — ไม่มี directive)
import { AddToCart } from './_components/AddToCart' // ข้างในมี 'use client'

export default async function ProductsPage() {
  const products = await getProducts() // fetch ฝั่ง server
  return (
    <main>
      {products.map((product) => (
        <article key={product.id}>
          <h2>{product.title}</h2>
          <AddToCart productId={product.id} /> {/* hydrate แค่ตัวนี้ */}
        </article>
      ))}
    </main>
  )
}
```

ส่งข้อมูล server **ลงผ่าน props** — props ต้อง serialize เป็น JSON ได้ function และ class instance
ข้ามเส้นแบ่งไม่ได้

### ต้องใช้โลกไหน?

| Component…                      | โลก    | เหตุผล                     |
| ------------------------------- | ------ | -------------------------- |
| แสดงข้อมูล ไม่มี interaction    | Server | ไม่ส่ง JS สักไบต์          |
| มีปุ่ม/input/form พร้อม handler | Client | ต้องการ event handler      |
| ใช้ `useState`/`useEffect`      | Client | React state อยู่ใน browser |
| อ่านฐานข้อมูลหรือ env ส่วนตัว   | Server | Secret อยู่บน server       |
| ใช้ `window`/`localStorage`     | Client | API เฉพาะ browser          |

ลังเลเมื่อไหร่: เริ่ม server ก่อนเสมอ แล้วเพิ่ม `'use client'` เมื่อ build หรือ editor บอกว่า
hook/handler ต้องการ รางวัลคือ client bundle ที่เล็กลง

## โค้ด Server-Only

กันโค้ดส่วนตัวออกจาก client graph — วางการเข้าถึงฐานข้อมูลและ secret ในโมดูล server-only และ mark
ให้ชัด:

```ts
// server/database.ts
import 'server-only'

export const databaseUrl = process.env.DATABASE_URL
```

ทุกอย่างใต้โฟลเดอร์ `server/` ถูก treat แบบเดียวกันอัตโนมัติ แพ็กเกจ state ทางการเป็น server-only
โดยนิยาม: import `@ruvyxa/auth` หรือ `@ruvyxa/database` จากโค้ด client ถูก reject — ฝั่ง browser ใช้
`@ruvyxa/auth/client` และ `@ruvyxa/realtime/client` แทน

## Boundary Validation

Ruvyxa ตรวจ import และการเข้าถึง environment ตอน build:

| Import / การเข้าถึง                | Client Bundle | Server Bundle | Diagnostic |
| ---------------------------------- | ------------- | ------------- | ---------- |
| `import 'server-only'`             | **Reject**    | ผ่าน          | `RUV1007`  |
| `import '@ruvyxa/auth'` (root)     | **Reject**    | ผ่าน          | `RUV1007`  |
| `import '@ruvyxa/database'` (root) | **Reject**    | ผ่าน          | `RUV1007`  |
| ตัวแปร environment ส่วนตัว         | **Reject**    | ผ่าน          | `RUV1008`  |
| `import 'client-only'`             | ผ่าน          | **Reject**    | `RUV1009`  |
| โมดูลใต้โฟลเดอร์ `server/`         | **Reject**    | ผ่าน          | `RUV1010`  |
| ตัวแปร `RUVYXA_PUBLIC_*`           | ผ่าน          | ผ่าน          | —          |

**ห้าม** แก้ diagnostic ด้วยการเปิดเผย secret ให้ browser — อย่าเปลี่ยนชื่อตัวแปรส่วนตัวเป็น
`RUVYXA_PUBLIC_` เพียงเพื่อปิด validation prefix นั้นคือการตัดสินใจชัดเจนว่า "ส่งค่านี้ไป browser
bundle"

## แก้ boundary error ที่เจอบ่อย

| Error                          | สาเหตุทั่วไป                                        | แก้                                                 |
| ------------------------------ | --------------------------------------------------- | --------------------------------------------------- |
| `RUV1007` ที่ไฟล์ component    | ไฟล์ `'use client'` import `lib/db.ts` หรือคล้ายกัน | Fetch ในหน้า server แล้วส่งผลลัพธ์ลงเป็น props      |
| `RUV1007` ที่ `@ruvyxa/auth`   | โค้ด browser import root package                    | Import `createAuthClient` จาก `@ruvyxa/auth/client` |
| `RUV1008` env ส่วนตัวใน client | `process.env.SECRET` ใน client component            | อ่านฝั่ง server แล้วส่งข้อมูลที่ไม่ใช่ secret ลงไป  |
| `RUV1009` client-only ใน SSR   | lib เฉพาะ browser ถูก import จากหน้า server         | ย้ายเข้า component `'use client'`                   |

ข้อความ error ระบุ chain การ import ชัดเจน — ไล่จากไฟล์ client ไปยังโมดูลต้นเหตุ แล้วตัด chain
ที่ข้อแรกที่ไม่จำเป็นต้องอยู่ฝั่ง client

## Best Practices

- ให้หน้าอยู่ฝั่ง server; ผลัก interactivity ไป client component ใบเล็กปลายทาง
- ส่งข้อมูล server ให้ client component ผ่าน props ที่ serialize ได้
- ใช้โฟลเดอร์ `server/` สำหรับ utility ฝั่ง server
- ใช้ `import 'server-only'` กับไฟล์ที่ต้องอยู่ server แม้อยู่นอก `server/`
- มอง `'use client'` ทุกครั้งเป็นต้นทุน: ตัวมันและทุกอย่างที่มัน import ถูกส่งไป browser

ดู [Environment Variables](environment-variables.md) สำหรับกติกาตัวแปร public/private และ
[แพ็กเกจทางการ](official-packages.md) สำหรับแพ็กเกจ state ฝั่ง server
