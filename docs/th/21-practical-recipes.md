# Practical recipes

> **เป้าหมายของ tutorial:** ต่อขยาย starter app โดยดัดแปลง pattern ที่สมบูรณ์และมี source
> รองรับครั้งละหนึ่งแบบ **เริ่มจาก:** foundational chapter ที่ตรงกันซึ่งแต่ละ recipe ลิงก์ไว้
> **Checkpoint:** คัดลอก recipe หนึ่งรายการ แทนที่ placeholder data แล้วรัน application check

แต่ละ recipe ใช้ public API หรือ route convention ที่ repository นี้ implement อยู่จริง ให้ copy
ไฟล์ที่แสดงไปยัง app ที่ทำ [สร้าง app แรก](02-create-your-first-app.md) เสร็จแล้ว และรัน
`npm run check` ก่อน build

## 1. Static dynamic page

ใช้ `getStaticParams` สำหรับ dynamic path ทุก path ที่ต้องการให้สร้างระหว่าง build

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

รัน `npm run build`; concrete path จะเป็น prerender candidate ใช้ object result แบบ
`{ params, cache: '10m' }` เมื่อ parameter discovery เองควรถูก cache อย่าใช้ pattern
นี้สำหรับค่าที่รู้ได้หลัง user-specific request เท่านั้น

## 2. Validate API request และคืน status code ที่เป็นประโยชน์

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

เก็บ body limit ไว้ใน `security.apiLimit`; มันป้องกัน memory ส่วน handler นี้ป้องกันความหมายของ
input ทดสอบ valid JSON, invalid JSON, text ว่าง และค่าที่ยาวเกิน

## 3. Cache loader และ invalidate หลัง write

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

`invalidate('tasks')` คือ action invalidation metadata; `invalidateCache('tasks')` ล้าง
process-local cache key/prefix ใช้ shared data/cache design เมื่อต้องให้หลาย process
เห็นข้อมูลตรงกัน

## 4. Client data loading พร้อม UI ที่ retry ได้

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

เพิ่ม `{ deps: [conversationId] }` เมื่อค่าที่เปลี่ยนต้อง trigger fetch อย่าวาง server-rendered
content ที่ขึ้นกับ `useSearchParams` เพราะระหว่าง SSR ค่าของมันเป็น client-only

## 5. Accessible navigation, metadata และ image

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

อย่าตั้ง title เดียวกันทั้งใน `meta` และ `<Seo>` ถ้าไม่ได้ตั้งใจให้ metadata ซ้ำ `Link` ยังเป็น
anchor จริงก่อน hydration วาง `product.jpg` ใน `public/`; production build สร้าง WebP variant สำหรับ
local PNG/JPEG ได้

## 6. เพิ่ม route-scoped policy โดยไม่ทำ handler ซ้ำ

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

pattern เป็น exact หรือ trailing-star prefix ทดสอบ route ที่ match หนึ่ง route และไม่ match หนึ่ง
route; cache rule ต้องมีอย่างน้อย browser, CDN หรือ `vary`

## 7. Test server primitive โดยไม่ต้องรัน server

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

รัน package test script หรือ `node --test` ตาม setup ของ application mock ตรวจ action/loader
contract ของคุณ; มันไม่แทน HTTP integration test

## 8. เพิ่ม release control ที่ fail เร็ว

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

`requireEnv` ทำให้ production build ล้มเหลวเมื่อค่าที่ระบุหาย/ว่าง `bundleBudget` วัด final minified
client JavaScript รัน four release command ใน
[Release-readiness playbook](19-release-readiness-playbook.md) แล้วเลือก artifact จาก
[คู่มือ platform adapter](20-platform-adapter-guide.md)

## 9. ประกาศชุดโมดูลที่รู้จักด้วย `import.meta.glob`

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

pattern และ option `{ eager: true }` ต้องเป็น compile-time literal เท่านั้น — pattern ที่เป็นตัวแปร
หรือ option ที่คำนวณจะเป็น build diagnostic ไม่ใช่ runtime fallback key ที่ generate จาก lazy match
จะ map ไปที่ `() => import(...)` จึงไม่มีอะไรถูก evaluate จนกว่าจะมีการเรียก loader นั้น ส่วน eager
match จะกลายเป็น static import ที่ hoist ขึ้นมาและเข้าสู่ dependency graph, chunking และ
tree-shaking เดียวกับ `import` statement ปกติ key เป็น specifier แบบ project-relative,
slash-normalized ที่มีลำดับ แน่นอนและไม่ขึ้นกับ locale ของเครื่อง จึง source เดียวกันได้ key
เหมือนกันทุกเครื่อง pattern จะ resolve จากไฟล์ที่ import และ resolve ออกนอก project root ไม่ได้ —
`import.meta.glob('../../secret/*.ts')` เป็น build error ไม่ใช่ผลลัพธ์บางส่วน alias จาก
`tsconfig.json`/`jsconfig.json` `paths` ทำงานเหมือน กับ import ปกติ

**ก่อนหน้า:** [คู่มือ platform adapter](20-platform-adapter-guide.md) · **ถัดไป:**
[ดัชนีเอกสาร](README.md)
