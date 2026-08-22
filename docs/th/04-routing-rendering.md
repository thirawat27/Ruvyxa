# Routing และ rendering

> **เป้าหมายของ tutorial:** เพิ่ม dynamic route และเลือกวิธี render อย่างตั้งใจ **เริ่มจาก:** กติกา
> route ใน [โครงสร้างโปรเจกต์](03-project-structure.md) **Checkpoint:** เปิด dynamic URL
> จริงหนึ่งรายการในโหมด development แล้วรัน application check

route discovery แปลง file tree เป็น manifest รัน `npm run routes` ระหว่างพัฒนาเพื่อดู manifest
และใช้ `npm run routes:json` เมื่อ script ต้องการข้อมูลที่เครื่องอ่านได้ strategy ของหน้าถูกเลือกจาก
export ของหน้าและ configuration `render`

| Strategy | การเลือกที่ยืนยันจาก source                | เวลาสร้าง HTML                                          |
| -------- | ------------------------------------------ | ------------------------------------------------------- |
| SSR      | ค่าเริ่มต้น หรือ `render.strategy: 'ssr'`  | ทุก request                                             |
| SSG      | static route/static parameter discovery    | build time                                              |
| ISR      | `export const revalidate = 60`             | build time แล้ว revalidate หลัง TTL                     |
| CSR      | หน้า `'use client'`                        | browser หลัง minimal shell                              |
| PPR      | `export const ppr = true` พร้อม `Suspense` | static shell ตอน build; dynamic slot stream ตอน request |

strategy เป็นตัวกำหนดว่า HTML ถูกสร้าง _เมื่อไร_ ส่วน `export const serverComponents = true`
กำหนดว่า _graph ไหน_ เป็นคนสร้าง และใช้ร่วมกับทุก strategy ด้านบนได้ยกเว้น PPR — ดู
[React Server Components](#react-server-components)

## Markdown, MDX และ component ที่ใช้ร่วมกัน

สร้าง `page.md` สำหรับ Markdown หรือ `page.mdx` เมื่อต้องใช้ JSX, expression และ import ทั้งสองแบบ
ไม่ต้องตั้ง compiler เพิ่ม หากต้องการตกแต่ง element มาตรฐานของ MDX หรือให้ component ร่วมกัน
ให้เพิ่ม `mdx-components.tsx` ที่ใกล้ page ที่สุด (รองรับ `.ts`, `.jsx`, `.js`, `.mts` และ `.mjs`
ด้วย) ไว้ใน directory ของ page หรือ ancestor:

```tsx
// app/mdx-components.tsx
export function useMDXComponents(components = {}) {
  return {
    ...components,
    h1: (props) => <h1 className="docs-title" {...props} />,
  }
}
```

provider ที่ใกล้ที่สุดจะชนะ ดังนั้น `app/docs/mdx-components.tsx` จะปรับเฉพาะ route ใต้ `app/docs/`
ก็ได้ `components` ที่ส่งให้ page โดยตรงจะถูกส่งต่อเข้า `useMDXComponents`; ให้ merge มันใน object
ที่คืนมาหากยังต้องการใช้ provider เป็น module ใน client graph ปกติ ดังนั้น `ruvyxa check` จะปฏิเสธ
server-only import และ private environment variable

Ruvyxa รวม `@mdx-js/mdx` และ GFM มาให้แล้ว หากต้องการใช้ plugin ของ unified ให้ติดตั้ง remark,
rehype หรือ recma plugin เป็น dependency ของ application จากนั้น import ใน `ruvyxa.config.ts`
และตั้งครั้งเดียวเพื่อใช้กับทั้ง `.md` และ `.mdx`:

```ts
import rehypeAutolinkHeadings from 'rehype-autolink-headings'
import rehypeSlug from 'rehype-slug'
import remarkToc from 'remark-toc'
import { config } from 'ruvyxa/config'

export default config({
  markdown: {
    remarkPlugins: [[remarkToc, { heading: 'contents', maxDepth: 3 }]],
    rehypePlugins: [rehypeSlug, [rehypeAutolinkHeadings, { behavior: 'append' }]],
  },
})
```

ระบบรักษาลำดับ plugin ตาม config และเก็บ `headings` หลัง application rehype plugin ทำงานแล้ว ดังนั้น
`id` ที่ plugin กำหนดให้ heading จะเป็น slug ที่ export ด้วย remark หรือ rehype plugin แก้
frontmatter ผ่าน `file.data.ruvyxa.frontmatter` ได้ แต่ค่าหลังแก้ต้องยังเป็น object ที่แปลงเป็น JSON
ได้ GFM เปิดเป็นค่าเริ่มต้นและปิดได้ด้วย `markdown.gfm: false` ส่วน raw HTML ใน `.md` จะยัง ถูก
escape; ให้ใช้ `.mdx` เฉพาะเมื่อเจตนาให้ JSX ทำงานจริง

## Dynamic SSG

สำหรับ dynamic SSG/ISR page ให้ export `getStaticParams` มันรับ route ทั้งหมดที่ค้นพบและรายละเอียด
route ปัจจุบัน แล้วคืน object (หรือ string/number shorthand สำหรับ route ที่มี dynamic segment
เดียว) ผลลัพธ์ห่อด้วย `{ params, cache }` ได้ โดย `cache` รับวินาทีหรือข้อความอย่าง `"10m"`

```tsx
// app/blog/[slug]/page.tsx
import type { GetStaticParams, PageProps } from 'ruvyxa'

export const getStaticParams: GetStaticParams<{ slug: string }> = () => [
  { slug: 'first-post' },
  { slug: 'release-notes' },
]

export default function Post({ params }: PageProps<{ slug: string }>) {
  return (
    <article>
      <h1>{params.slug}</h1>
    </article>
  )
}
```

`generateStaticParams` และ `staticParams` ถูกยอมรับเป็นชื่อของ export เดียวกัน page ที่ย้ายมาจาก
Next.js จึงประกาศ parameter ได้โดยไม่ต้องเปลี่ยนชื่อ

## การกำหนด rendering strategy เอง

Ruvyxa เลือก strategy ให้อัตโนมัติ และ `export const dynamic` ใช้ override ได้ — เป็น route segment
config ตัวเดียวกับที่ Next.js ใช้ และลำดับความสำคัญเหมือนกัน `'force-dynamic'` จะพา route
ออกจากเส้นทาง pre-render แม้จะ export `revalidate` ด้วยก็ตาม ส่วน `'force-static'` และ `'error'`
จะพาเข้าไป และ `'auto'` คือค่าเริ่มต้น ใช้ `export const revalidate = <วินาที>` เพื่อเลือก ISR และ
`export const ppr = true` เพื่อเลือก partial pre-rendering

`export const metadata` **ไม่ถูกอ่าน** เพราะ metadata object ของ Next เป็นโครงซ้อนชั้น ขณะที่ `meta`
ของ Ruvyxa เป็นโครงแบน ทั้งสองจึงใช้แทนกันไม่ได้ ให้ใช้ `export const meta` ด้านล่างแทน

## React Server Components

`export const serverComponents = true` ทำให้ route หนึ่งเรนเดอร์ผ่านไปป์ไลน์ server components ของ
React ตัว page และ layout ของมันจะรันใน module graph ที่ resolve ด้วย condition `react-server` ของ
React และมีเฉพาะ module ที่ทำเครื่องหมาย `'use client'` เท่านั้นที่ไปถึงเบราว์เซอร์

```tsx
// app/dashboard/page.tsx
import { readFile } from 'node:fs/promises'
import Chart from './chart'

export const serverComponents = true

export default async function Dashboard() {
  const rows = JSON.parse(await readFile('./data/metrics.json', 'utf8'))
  return <Chart rows={rows} />
}
```

```tsx
// app/dashboard/chart.tsx
'use client'
import { useState } from 'react'

export default function Chart({ rows }: { rows: Row[] }) {
  const [range, setRange] = useState('30d')
  // ...
}
```

`page.tsx` ด้านบนไม่เคยถูก bundle ไปเบราว์เซอร์ ส่วน `chart.tsx` ถูก และเป็น module เดียวจาก route
นี้ ที่ถูก page จะถูกแปลงเป็น payload — element tree ที่ serialise ไว้ ซึ่ง `Chart` ปรากฏเป็น
reference id แทนที่จะเป็นโค้ด — เซิร์ฟเวอร์เรนเดอร์ payload นั้นเป็น HTML และเบราว์เซอร์เล่นซ้ำเพื่อ
hydrate ทั้งสองฝั่งอ่าน payload เดียวกัน สิ่งที่ hydrate จึงเป็นสิ่งเดียวกับที่เรนเดอร์ไว้

payload เดินทางมาใน data block `<script type="application/json">` เช่นเดียวกับ route bootstrap:
`Content-Security-Policy` ที่ไม่มี `'unsafe-inline'` จึงไม่บล็อกมัน และไม่ต้องใช้ nonce

### การติดตั้ง runtime

Server components ต้องใช้ `react-server-dom-webpack` เวอร์ชันเดียวกับ React:

```bash
npm install react-server-dom-webpack@19.2.8
```

แพ็กเกจนี้เป็น optional: แอปที่ไม่เคยเขียน export นี้ก็ไม่ต้องมี และ route ที่เขียนจะได้ `RUV1863`
ที่บอกชื่อแพ็กเกจตรง ๆ แพ็กเกจนี้ประกาศ `webpack` เป็น peer สำหรับไฟล์เดียว — bundler plugin ของมัน
ซึ่ง Ruvyxa ไม่เคยโหลด — จึงควรบอก package manager ให้ข้าม สำหรับ pnpm:

```yaml
# pnpm-workspace.yaml
peerDependencyRules:
  ignoreMissing:
    - webpack
```

API ของ Node เป็นโค้ดธรรมดาภายใน server component TypeScript ต้องรู้ว่ามันมีอยู่ ซึ่งหมายถึง
`@types/node` และ `"types": ["node"]` ใน `tsconfig.json` — โดยมีข้อแม้ว่าสิ่งนี้ทำให้ global ของ
Node มองเห็นได้จากไฟล์ `'use client'` ด้วย ซึ่งตรงนั้นสิ่งที่กันไม่ให้ใช้คือ boundary check ไม่ใช่
type checker

### สิ่งที่ server component ทำไม่ได้

React build แบบ `react-server` ไม่มี `useState` ไม่มี `useEffect` และไม่มี `createContext` ดังนั้น
server component จึงถือ state, รัน effect, หรือให้ context ไม่ได้ นี่คือเส้นแบ่ง
ไม่ใช่ข้อจำกัดที่ต้อง หาทางเลี่ยง: ย้ายส่วนเหล่านั้นไปไว้ใน module `'use client'`
แล้วส่งข้อมูลลงไปเป็น props ส่วน `Suspense` ใช้ได้ทั้งสอง graph ดังนั้น `loading.tsx` จึงทำงานเหมือน
route อื่น ๆ

`error.tsx` และ `not-found.tsx` เป็น boundary ที่สร้างจาก class ซึ่ง server graph รันไม่ได้ บน route
ที่ใช้ server components ทั้งสองไฟล์ต้องเป็น module `'use client'` — เป็นกฎเดียวกับที่ React
กำหนดเอง

`@ruvyxa/react` import จาก server component ได้อย่างปลอดภัย เพราะ `Link`, routing hook ต่าง ๆ,
`Script`, `RuvyxaErrorBoundary` และ `useRuvyxaLoader` ประกาศ `'use client'` ไว้ในตัวเองแล้ว ดังนั้น
root layout จึงเรนเดอร์ nav ที่เป็น `<Link>` บน route ที่ใช้ server components
ได้โดยไม่ต้องแก้อะไรเลย — server graph ได้ reference ส่วนเบราว์เซอร์เป็นคน resolve ให้ ส่วน `Image`,
`Seo` และ `notFound()` ไม่มีฝั่งเบราว์เซอร์ จึงถูกเรนเดอร์โดย server component เอง

แพ็กเกจอื่นที่คุณติดตั้งก็ใช้กฎเดียวกัน: component ที่ใช้ hook ต้องประกาศ `'use client'`
ไว้ในไฟล์ที่เผยแพร่ ไม่เช่นนั้น server graph จะคอมไพล์มันกับ React build ที่ไม่มี hook อยู่เลย

### Server function

ฟังก์ชันที่อยู่หลัง `'use server'` จะรันบนเซิร์ฟเวอร์ และเรียกจากเบราว์เซอร์ได้ Ruvyxa
รองรับทั้งสองรูปแบบที่ React มี

แบบทั้งโมดูล ซึ่งเป็นรูปแบบที่ component `'use client'` import เข้าไปใช้:

```ts
// app/dashboard/actions.ts
'use server'

export async function rename(id: string, name: string) {
  await db.rename(id, name)
  return db.get(id)
}
```

```tsx
// app/dashboard/row.tsx
'use client'
import { rename } from './actions'

export function Row({ id }: { id: string }) {
  return <button onClick={() => rename(id, 'new')}>Rename</button>
}
```

โค้ดใน `actions.ts` ไม่มีอยู่ใน browser bundle เลย `rename` ที่นั่นคือ _reference_:
การเรียกมันคือการ POST อาร์กิวเมนต์ไปที่เซิร์ฟเวอร์ รันฟังก์ชันจริง แล้วได้ค่าที่มัน return กลับมา —
รวมถึง element tree ด้วย เพราะคำตอบเป็น Flight payload ไม่ใช่ JSON

หรือแบบฟังก์ชันเดียวภายใน server component ที่ใช้มัน ซึ่งไม่ต้องมีไฟล์ที่สอง:

```tsx
// app/dashboard/page.tsx
export const serverComponents = true

export async function markAllRead(userId: string) {
  'use server'
  await db.markAllRead(userId)
}

export default async function Dashboard() {
  return <Toolbar onClear={markAllRead} />
}
```

ฟังก์ชันถูกส่งให้ component `'use client'` เป็น prop ธรรมดา และไปถึงที่นั่นในรูป reference
เหมือนกับแบบที่ import เข้ามาทุกประการ

**server function แบบ inline ต้องอยู่ที่ระดับบนสุดของโมดูล**
ฟังก์ชันที่ประกาศไว้ข้างในฟังก์ชันอื่นจะ closure ตัวแปรของการเรียกครั้งนั้นไว้
และการเรียกที่มาถึงทีหลัง — จาก request อื่น ใน process อื่น —
ไม่มีทางสร้างค่าเหล่านั้นขึ้นมาใหม่ได้ Ruvyxa จึงปฏิเสธด้วย `RUV1867` พร้อมบอกบรรทัด
แทนที่จะคอมไพล์เป็น ฟังก์ชันที่อ่านค่าจากการเรนเดอร์ที่จบไปแล้ว ให้ย้ายไปไว้ระดับบนสุด
หรือย้ายไปโมดูลที่ขึ้นต้นด้วย directive

การเรียกจะไปที่ `POST /__ruvyxa/rsc` ซึ่งเป็น endpoint เดียวกับที่ให้ payload ของ route โดยแนบ
cookie ของผู้ใช้ไปด้วย และมี header แบบ same-origin ที่หน้าเว็บ cross-origin ตั้งไม่ได้ server
function หนึ่งตัวจะเรียกได้จาก route ที่ page หรือ client component ของมัน import ฟังก์ชันนั้น

`<form action={fn}>` ทำงาน **ตั้งแต่ก่อน JavaScript ของหน้าจะโหลดเสร็จ และทำงานได้แม้ไม่มี
JavaScript เลย** React เขียน reference ของฟังก์ชันลงใน hidden field ตอนเรนเดอร์ฟอร์ม
เบราว์เซอร์ที่ยังไม่ได้รัน bundle ของหน้านั้นเลยจึง submit ได้ตามปกติ — โพสต์ไปที่ URL ของหน้าเอง
Ruvyxa อ่าน field เหล่านั้น เรียกฟังก์ชัน
แล้วตอบกลับเป็นเอกสารที่เรนเดอร์ใหม่ซึ่งมีผลลัพธ์อยู่ในนั้นแล้ว ตัวที่พาผลลัพธ์เข้ามาใน markup คือ
`useActionState` — ค่าที่ action คืนจะถูกเล่นซ้ำเข้าไปใน hook ดังนั้น component
เดียวกันจะเรนเดอร์คำตอบเดียวกัน ไม่ว่าจะมาทาง `fetch` หรือมาทางการ submit ฟอร์ม

```tsx
// app/search/form.tsx
'use client'

import { useActionState } from 'react'

import { lookup } from './actions'

export default function Search() {
  const [answer, submit] = useActionState(lookup, null)
  return (
    <form action={submit}>
      <input name="q" />
      <output>{answer ?? 'nothing looked up yet'}</output>
    </form>
  )
}
```

เมื่อ bundle โหลดเสร็จแล้ว React จะดักการ submit แทน เรียกฟังก์ชันเดิมผ่าน `fetch`
แล้วอัปเดตเฉพาะส่วนที่เปลี่ยน ไม่มีอะไรในฟอร์มที่ต้องเขียนสองแบบ

ฟอร์มที่ถูก submit จะถูกตอบด้วยการเรนเดอร์ของ route เอง ไม่ใช่ตามกลยุทธ์การเรนเดอร์ของมัน
เพราะเอกสารที่ prerender หรือ cache ไว้ถูกสร้างก่อนที่ action จะรัน response จึงถูกเรนเดอร์ใหม่และมี
`Cache-Control: no-store` ส่วนอะไรก็ตามที่ action ส่งให้ `revalidatePath()` จะถูกนำไปใช้ก่อนที่
response จะถูกส่งกลับ server function ต้องมี graph แบบ `react-server` ไว้ให้ resolve reference
ดังนั้นทั้งหมดนี้ใช้กับ route ที่เป็น server components ส่วน `POST`
ไปหน้าอื่นยังเรนเดอร์เหมือนเดิมทุกอย่าง

ส่วน server action ของ Ruvyxa ยังเหมือนเดิม และยังเรียกได้จาก component `'use client'` บน route
ที่ใช้ server components — ดู [Data, action และ API route](05-data-actions-api.md)

### การสตรีมเอกสาร

route ที่ใช้ server components และสร้างเอกสารใหม่ทุก request จะถูก **สตรีม** React ส่ง shell
ออกไปทันทีที่มี และส่งแต่ละ `Suspense` boundary ตามมาเมื่อเซิร์ฟเวอร์ทำเสร็จ ดังนั้น server
component ที่ช้าจะหน่วงเฉพาะส่วนที่รอมันอยู่ ไม่ใช่ทั้งหน้า

```tsx
// app/dashboard/page.tsx
import { Suspense } from 'react'

export const serverComponents = true
export const dynamic = 'force-dynamic'

async function Revenue() {
  return <Chart rows={await db.revenue()} />
}

export default function Dashboard() {
  return (
    <main>
      <h1>Dashboard</h1>
      <Suspense fallback={<Skeleton />}>
        <Revenue />
      </Suspense>
    </main>
  )
}
```

ถ้าไม่มี `Suspense` ทั้งเอกสารก็ยังต้องรอ `db.revenue()` อยู่ดี — boundary
คือสิ่งที่ทำให้เซิร์ฟเวอร์มีอะไรส่งออกไปก่อน

**เฉพาะเอกสารที่สร้างใหม่ทุก request เท่านั้นที่สตรีม** `export const dynamic = 'force-dynamic'`
หรืออะไรก็ตามที่ทำให้ route เป็น dynamic คือสิ่งที่เลือกโหมดนี้ ส่วน route ที่ prerender, ใช้
`revalidate` หรือเรนเดอร์แบบ static ต้องกลายเป็นสตริงเพื่อเขียนลงดิสก์หรือเก็บใน cache
ซึ่งสตรีมไม่ใช่รูปแบบที่ทำแบบนั้นได้ — route เหล่านั้นจึงยังถูกส่งทั้งก้อน และ route ที่ไม่ได้ใช้
server components ก็ไม่สตรีมเช่นกัน เพราะการเรนเดอร์ของมันจบในขั้นตอนเดียว ไม่มีอะไรให้ส่งก่อน

response ที่สตรีมจะมี `Cache-Control: no-store` ไม่มี `Content-Length` และไม่ถูกเก็บใน render cache
ทั้งหมดนี้มาจากข้อเท็จจริงเดียวกัน: เอกสารไม่เคยมีตัวตนเป็นสตริงที่เซิร์ฟเวอร์จะเก็บได้

Flight payload ยังถูกเขียนลงในเอกสารเหมือนเดิม โดยอยู่ท้ายสุด ใน data block
`<script type="application/json">` ก้อนเดิม เพราะมันสมบูรณ์ก็ต่อเมื่อการเรนเดอร์เสร็จ
และเบราว์เซอร์ต้องใช้มันตอน hydrate ไม่ใช่ตอนวาดหน้า — hydration เริ่มไม่ได้จนกว่าเอกสารจะถูก parse
จบ ซึ่งก็คือหลังไบต์สุดท้ายอยู่แล้ว

### ชุดที่ Ruvyxa ปฏิเสธ

แต่ละกรณีด้านล่างจะ build ผ่านแล้วไม่ทำอะไรเลย การค้นหา route จึงล้มเหลวแทน (`RUV1011`):

| ชุด                                              | เหตุผล                                                                         |
| ------------------------------------------------ | ------------------------------------------------------------------------------ |
| page ที่เป็น `'use client'` + `serverComponents` | page ที่รันในเบราว์เซอร์ทั้งหมดไม่มีฝั่งเซิร์ฟเวอร์ให้เรนเดอร์                 |
| `export const ppr = true` + `serverComponents`   | partial pre-rendering สตรีม shell ผ่าน entry ที่ไปป์ไลน์นี้ไม่ได้สร้าง         |
| intercepting route + `serverComponents`          | การ intercept ถูกจับคู่จาก client route registry ที่ไปป์ไลน์นี้ไม่ได้ประกาศไว้ |

การนำทางฝั่งไคลเอนต์ใช้ได้ทั้งสองทิศทาง การเข้า route ที่ใช้ server components จะดึง payload จาก
`/__ruvyxa/rsc` แล้วเรนเดอร์แทนที่ทันที ไม่มีการโหลดเอกสารใหม่ และหน้าเดิมถูกแทนที่แบบเดียวกับการ
เปลี่ยน route ปกติ endpoint นั้นคือการ _เรนเดอร์_: มันพา cookie ของผู้ใช้ไปเหมือนคำขอเต็ม ๆ ดังนั้น
response จึงแคชไม่ได้ และต้องมี header แบบ same-origin ที่หน้าเว็บข้าม origin ตั้งไม่ได้ถ้าไม่ผ่าน
preflight

### การ deploy

route ที่ใช้ server components และถูก pre-render แล้ว deploy ได้ทุกที่: payload อยู่ในไฟล์ HTML ที่
adapter คัดลอกไปอยู่แล้ว ส่วน route ที่ยังต้องใช้เซิร์ฟเวอร์ตอนมี request — `ssr`, `isr`, หรือ
`export const dynamic = 'force-dynamic'` — จะถูกปฏิเสธตอน build ด้วย `RUV2213` เพราะ adapter ทุกตัว
เสิร์ฟหน้าเว็บผ่าน module ที่สร้างจาก entry แบบ SSR ปกติ ให้เสิร์ฟ route เหล่านั้นด้วย
`ruvyxa start` หรือปล่อยให้มัน pre-render

ฟังก์ชันที่ deploy แล้วจะตอบ `/__ruvyxa/rsc` ด้วย 501 ด้วยเหตุผลเดียวกัน ดังนั้นบนเป้าหมายเหล่านั้น
การนำทางเข้า route ที่ใช้ server components จะย้อนไปโหลดเอกสารแทน ซึ่งสำหรับ route ที่ pre-render
ไว้แล้วก็คือไฟล์สแตติกที่ CDN ถืออยู่แล้ว

## Route metadata และ boundary

`export const meta` รับ `Meta` object หรือ `MetaFactory` metadata จาก layout merge จาก root ไป leaf;
ค่าที่เฉพาะที่สุดชนะ title ระดับล่างจะถูก format โดย `titleTemplate` ของ ancestor ที่ใกล้ที่สุด

```tsx
// app/layout.tsx
import type { Meta } from '@ruvyxa/react'
export const meta: Meta = { titleTemplate: '%s — Example', siteName: 'Example' }

// app/blog/[slug]/page.tsx
export const meta = ({ params }: { params: Record<string, string> }) => ({
  title: params.slug,
  canonical: `https://example.test/blog/${params.slug}`,
})
```

`error.tsx` รับ `{ error, reset, retry }`; `loading.tsx` และ `not-found.tsx` เป็น component ปกติ
หากต้องการเลือก `not-found.tsx` ที่ใกล้ที่สุด ให้ import `notFound` จาก `@ruvyxa/react` แล้วเรียกมัน
(มัน throw tagged signal) อย่าสับสนกับ `notFound` จาก `ruvyxa/server` ซึ่งสร้าง HTTP `Response`
สถานะ 404

### `template.tsx`

`template.tsx` ห่อ children ของระดับตัวเองแบบเดียวกับ layout
ต่างกันตรงจุดเดียวซึ่งเป็นเหตุผลที่มีไฟล์นี้: มันถูกให้ key จาก request path ดังนั้นการ navigate
ภายใน layout เดิมจะ **remount** มัน — state รีเซ็ต และ effect ทำงานใหม่ — ขณะที่ layout ด้านบนยังคง
mount อยู่ ใช้กับ enter animation, `useEffect` ต่อการ navigate หนึ่งครั้ง หรือ state
ที่ไม่ควรอยู่รอดข้าม sibling route

```tsx
// app/dashboard/template.tsx
'use client'
import { useEffect } from 'react'

export default function DashboardTemplate({ children }: { children: React.ReactNode }) {
  useEffect(() => {
    // ทำงานใหม่ทุกครั้งที่ navigate ภายใน app/dashboard/ ต่างจาก layout
  }, [])
  return <section className="fade-in">{children}</section>
}
```

layout กับ template ซ้อนกันเป็น `layout > template > children` ในแต่ละระดับ
ระดับหนึ่งจึงมีไฟล์ใดไฟล์หนึ่ง มีทั้งคู่ หรือไม่มีเลยก็ได้ template ไม่ประกาศ metadata —
`export const meta` เป็นของ layout หรือ page

### Parallel routes

โฟลเดอร์ `@name` ที่วางข้าง `layout.tsx` ประกาศ slot ที่ layout นั้นจะได้รับเป็น prop ควบคู่ไปกับ
page ที่มันรับเป็น `children` อยู่แล้ว ใช้เมื่อหนึ่งหน้าจอประกอบด้วยหลาย panel ที่เป็นอิสระต่อกัน
ไม่ใช่ page เดียวที่ render ทุกอย่าง

```text
app/dashboard/
├── @activity/
│   ├── default.tsx
│   └── page.tsx
├── @team/
│   ├── page.tsx
│   └── reports/page.tsx
├── layout.tsx
├── page.tsx
└── reports/page.tsx
```

```tsx
// app/dashboard/layout.tsx
export default function DashboardLayout({
  children,
  team,
  activity,
}: {
  children: React.ReactNode
  team?: React.ReactNode
  activity?: React.ReactNode
}) {
  return (
    <div className="grid">
      <aside>{team}</aside>
      <aside>{activity}</aside>
      <main>{children}</main>
    </div>
  )
}
```

แต่ละ slot จับคู่กับ URL อย่างอิสระจาก page ที่ `/dashboard/reports` page มาจาก `reports/page.tsx`
ส่วน `team` มาจาก `@team/reports/page.tsx`; `activity` ไม่มีอะไรสำหรับ URL นั้น จึง render
`default.tsx` ของตัวเอง slot ที่ไม่มีทั้ง page ที่ตรงและ `default.tsx` จะถูกตัดออก — layout
จะไม่ได้รับ prop นั้นเลย และโฟลเดอร์ `@name` ไม่เคยกลายเป็น route ของตัวเอง

ข้อจำกัดสองข้อที่ควรรู้: `layout.tsx` หรือ `loading.tsx` ที่อยู่ภายใน slot จะไม่ถูกประกอบเข้า
subtree ของ slot และ slot ที่ไม่ตรงจะกลับไปใช้ `default.tsx` ทุกครั้งที่ navigate แทนที่จะคงสิ่งที่
render ไว้ล่าสุด

### Intercepting route

`(.)`, `(..)`, `(..)(..)` และ `(...)` ทำเครื่องหมายให้โฟลเดอร์เป็น **overlay** ทับ route
ที่มีอยู่แล้ว การ navigate แบบ soft ไปยัง URL ที่มันระบุจะ render โฟลเดอร์นั้นลงใน parallel-route
slot โดยที่หน้า ด้านล่างยังคง mount อยู่ ส่วนการโหลดแบบ hard ที่ URL เดียวกันจะ render หน้าปกติ

```text
app/gallery/
├── @modal/
│   ├── (.)photo/page.tsx   ← แสดงทับ /gallery เมื่อ router ไปที่ /gallery/photo
│   └── default.tsx         ← แสดงในเวลาอื่น
├── layout.tsx              ← รับ `modal` มาพร้อมกับ `children`
├── page.tsx
└── photo/page.tsx          ← สิ่งที่ /gallery/photo render ด้วยตัวเอง
```

marker บอกว่า segment ที่ตามหลังอยู่ที่ระดับใด นับเป็นระดับของ **URL** — route group และโฟลเดอร์
slot ไม่นับเป็นระดับ สำหรับ `app/gallery/@modal/(.)photo` นั้น `(.)` หมายถึงระดับ `app/gallery`
เป้าหมายจึงเป็น `/gallery/photo` ส่วน `(..)` ขึ้นไปหนึ่งระดับ `(..)(..)` สองระดับ และ `(...)`
เริ่มจาก root ของ app

มีกฎสามข้อที่ทำให้พฤติกรรมนี้คาดเดาได้แทนที่จะเป็นเวทมนตร์:

- **route จริงต้องมีอยู่** เพราะ interception คือ overlay การ reload, การแชร์ลิงก์
  หรือการเปิดแท็บใหม่ ก็ยังต้อง render อะไรสักอย่าง marker ที่เป้าหมายไม่มีหน้าใดตอบจะทำให้ build
  ล้มเหลวด้วย **RUV1006**
- **โฟลเดอร์ต้องอยู่ภายใน slot `@name`** เพราะนั่นคือสิ่งที่ overlay เข้าไปแทนที่ ถ้าอยู่นอก slot
  ก็ไม่มี ที่ให้วาง และ build จะล้มเหลวด้วย **RUV1005**
- **เฉพาะ soft navigation เท่านั้นที่ intercept** overlay ถูก ship อยู่ใน bundle
  ของหน้าที่คุณยืนอยู่ นั่นคือเหตุผลที่มันเปิดได้โดยไม่ต้องยิง request เลย
  และเป็นเหตุผลเดียวกันที่การเข้ามาจากที่อื่นจะเห็นหน้าจริง

`router.back()` ปิด overlay ได้: interception push history entry ไว้หนึ่งรายการ การ pop มันจึงพา URL
กลับไปยังหน้าที่ยัง mount อยู่ด้านล่าง

ขณะที่ overlay เปิดอยู่ `usePathname()` **ภายใน route tree** ยังรายงานหน้าที่ mount อยู่
เพราะหน้านั้น คือสิ่งที่ mount จริง และ `template.tsx` ใช้ค่านั้นเป็น key การรายงาน URL ของ overlay
จึงจะทำให้หน้าที่ overlay ทับอยู่ถูก remount ส่วนคอมโพเนนต์ overlay จะได้รับ URL และ parameter
ที่ถูก intercept ผ่าน prop `requestPath` และ `params` ของตัวเอง และ router snapshot
(สิ่งที่คอมโพเนนต์นอก tree เห็น) จะตามแถบที่อยู่

### boundary ของสถานะ route แบบครบชุด### boundary ของสถานะ route แบบครบชุด

วาง special file ทั้งสามไว้ข้าง segment ที่ต้องการครอบคลุม ระบบจะเลือกไฟล์ที่อยู่ใกล้ที่สุด ดังนั้น
โครงสร้างนี้ทำให้ทุกหน้า product มี loading UI, ปุ่มลองใหม่เมื่อ error และ 404 เฉพาะส่วน โดยไม่ต้อง
แก้ทุก page

```text
app/products/
├── [slug]/
│   └── page.tsx
├── error.tsx
├── loading.tsx
└── not-found.tsx
```

```tsx
// app/products/[slug]/page.tsx
import { notFound } from '@ruvyxa/react'

const products = { notebook: 'A plain notebook' }

export default function Product({ params }: { params: { slug: string } }) {
  const product = products[params.slug as keyof typeof products]
  if (!product) notFound()
  return (
    <main>
      <h1>{product}</h1>
    </main>
  )
}
```

```tsx
// app/products/loading.tsx
export default function Loading() {
  return <main aria-busy="true">กำลังโหลดสินค้า…</main>
}

// app/products/not-found.tsx
export default function ProductNotFound() {
  return (
    <main>
      <h1>ไม่พบสินค้า</h1>
    </main>
  )
}
```

```tsx
// app/products/error.tsx
'use client'
import type { RouteErrorProps } from '@ruvyxa/react'

export default function ProductError({ error, reset }: RouteErrorProps) {
  return (
    <main>
      <h1>ไม่สามารถโหลดสินค้านี้ได้</h1>
      <p>{error.message}</p>
      <button type="button" onClick={reset}>
        ลองใหม่
      </button>
    </main>
  )
}
```

`loading.tsx` ทำสองหน้าที่ ฝั่ง server มันคือ Suspense fallback ของ route ส่วนฝั่งเบราว์เซอร์มันคือ
**loading shell** ของ route ด้วย: เมื่อ navigate ไปยัง route ที่มีไฟล์นี้ Ruvyxa จะวาด layout กับ
`loading.tsx` ของปลายทางทันทีที่ bundle ของ route พร้อม โดยไม่รอข้อมูลจาก server ของหน้านั้น
เนื้อหาจริงจะเข้ามาแทน fallback เมื่อ payload มาถึง

นี่คือสิ่งที่ทำให้การ navigate ไป route ที่ช้ารู้สึกว่า "ตอบสนองทันที"
แทนที่จะเหมือนกดแล้วไม่มีอะไรเกิดขึ้น และไม่มีค่า request เพิ่มเลย เพราะ layout กับ component
loading อยู่ใน bundle ที่ `<Link prefetch>` อุ่นไว้อยู่แล้ว ส่วน route ที่ไม่มี `loading.tsx`
ถือว่าไม่ได้ประกาศสถานะ loading ไว้ หน้าเดิมจึงค้างอยู่ จนกว่าหน้าใหม่จะพร้อม เหมือนเดิมทุกประการ

`error.tsx` ได้เส้นทางการกู้คืนทั้งสองแบบ: `reset()` ล้าง boundary แล้ว render ใหม่จากข้อมูลที่
client มีอยู่แล้ว เหมาะกับกรณีที่พังเพราะตัว render เอง ส่วน `retry()` จะขอข้อมูล route จาก server
ใหม่ก่อน ซึ่งเป็นสิ่งที่ต้องการเมื่อสิ่งที่พังคือ _ข้อมูล_ — คืนค่าเป็น promise ที่ resolve เมื่อ
boundary ถูก reset แล้ว ถ้าไม่มี router ทำงานอยู่ `retry()` จะถอยไปทำงานเหมือน `reset()`

ส่วน `not-found.tsx` render ฝั่ง server ได้ แต่ `reset()` จะ render ใหม่หลัง hydration ดังนั้น
`error.tsx` ที่มีปุ่มลองใหม่ต้องเป็น client component ควรแสดงข้อความ error ที่ปลอดภัยสำหรับผู้ใช้
และบันทึกรายละเอียดวินิจฉัยใน server หรือ integration ด้าน observability แทนการ render secret หรือ
stack trace

## นโยบาย i18n route

`i18n.locales` และ `i18n.defaultLocale` เป็น configuration field locale routing เป็นแบบ file-system
(เช่น `app/[lang]/about/page.tsx`); ชื่อ parameter ปริยายคือ `lang` เมื่อเปิด locale detection
server จะพิจารณา cookie ที่ตั้งค่าไว้ (ปริยาย `RUVYXA_LOCALE`) และ `Accept-Language`

**ก่อนหน้า:** [โครงสร้างโปรเจกต์](03-project-structure.md) · **ถัดไป:**
[ข้อมูล, action และ API route](05-data-actions-api.md)
