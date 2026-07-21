# Plugins

ระบบ plugin ของ Ruvyxa เป็นโมดูลแอปพลิเคชันที่เขียนด้วย TypeScript

สร้าง starter:

```bash
npx ruvyxa plugin new auth
```

คำสั่งจะสร้างแพ็กเกจ `auth/` ตรงๆ (ชื่อโฟลเดอร์ = ชื่อ plugin ไม่ต้องใช้ `--dir`) พร้อม
`package.json`, `tsconfig.json`, `README.md` และ `src/index.ts` ใส่ `--dir <path>` เฉพาะถ้าต้องการ
ตำแหน่งอื่น plugin รันได้ทั้ง Node.js และ Bun (`--runtime bun` หรือ `RUVYXA_RUNTIME=bun`):

```ts
import { plugin } from 'ruvyxa/config'

export default plugin('auth', {
  routes: ['/*'],
  onRequest(request) {
    return request.headers.has('authorization')
      ? undefined
      : new Response('Unauthorized', { status: 401 })
  },
})
```

นำเข้า package ใน `ruvyxa.config.ts`:

```ts
import auth from './plugins/auth'
import { config } from 'ruvyxa/config'

export default config({ plugins: [auth] })
```

รัน `npm install` และ `npm run build` ภายในโฟลเดอร์ plugin เพื่อสร้าง `dist/` แล้วใช้ `npm publish`
เพื่อเผยแพร่เป็น npm library ได้

ใช้ `plugin(name, middleware)` สำหรับ request/response middleware ซึ่งรับได้ทั้ง middleware object
หรือ request handler function โดย Middleware ใช้ Fetch `Request` และ `Response` มาตรฐาน

หากต้องใช้ `resolveId`, `transform` หรือ `onBuildComplete` ให้ใช้รูปแบบขั้นสูง
`definePlugin({ name, setup })` ทุก hook ทำงานใน Node/Bun runtime แบบ persistent ไม่มี ABI แยกหรือ
คำสั่ง debug แบบเดิม

## Built-in plugins

`ruvyxa/plugins` มี plugin สำเร็จรูปที่สร้างบน public hooks ชุดเดียวกัน:

```ts
import { config } from 'ruvyxa/config'
import { alias, headers, redirects, robots, sitemap } from 'ruvyxa/plugins'

export default config({
  plugins: [
    redirects([{ source: '/old-blog/*', destination: '/blog/*', permanent: true }]),
    headers([{ source: '/api/*', headers: { 'cache-control': 'no-store' } }]),
    sitemap({ siteUrl: 'https://example.com', robots: true }),
    alias({ '~content': 'content/index.ts' }),
  ],
})
```

- `redirects(rules)` — redirect แบบ declarative ก่อนถึงขั้น render ใช้ path ตรงตัวหรือ prefix
  ที่ลงท้าย ด้วย `*` ได้ ถ้า destination ลงท้ายด้วย `*` ส่วนที่เหลือของ path จะถูกต่อท้ายให้ และ
  `permanent: true` ตอบ 308 แทน 307
- `headers(rules)` — กำหนด response header ต่อ route กติกาที่ไม่ระบุ `source` จะมีผลทุกหน้า
- `sitemap({ siteUrl, exclude, robots })` — เขียน `sitemap.xml` (และ `robots.txt` ถ้าเปิด) ลง
  โฟลเดอร์ asset ที่เสิร์ฟจริงหลังจบ production build โดยอ่านจาก route manifest ข้าม dynamic route
  และ API route ให้อัตโนมัติ
- `robots({ rules, sitemap })` — สร้าง `robots.txt` แยกเดี่ยว
- `alias(map)` — จับคู่ import specifier แบบตรงตัวไปยังไฟล์ในโปรเจกต์ก่อนถึง native resolver
- `bundleBudget({ maxChunkKb, maxTotalKb })` — ทำให้ production build ล้มเหลวเมื่อ client JavaScript
  เกินงบที่ตั้งไว้ ช่วยจับ bundle regression ได้ตั้งแต่ใน CI
- `requireEnv(names)` — ทำให้ production build ล้มเหลวเมื่อ environment variable ที่จำเป็นหายไป
  หรือว่างเปล่า

`routes` ของ middleware จะถูกส่งให้ native server ด้วย ทำให้ request ที่ไม่มีทาง match ข้ามการ
round-trip ไปยัง plugin runtime ทั้งหมด — จึงควรระบุ route ให้ middleware เสมอเมื่อทำได้

## Middleware worker pool

โดยปกติ plugin middleware ทำงานบน runtime process เดียวแบบ persistent ถ้า middleware แบบ stateless
บน route ที่ traffic สูงกลายเป็นคอขวด ใช้ `middleware.workers` (1–8) เพื่อเปิด pool ของ runtime
process ที่เหมือนกันแบบ round-robin:

```ts
export default config({
  middleware: { workers: 2 },
})
```

Worker แต่ละตัวไม่แชร์ state ระดับ module ของ plugin — ตัวนับ, cache หรือ session ที่เก็บใน module
scope จะแยกต่อ process ดังนั้นคงค่า default หนึ่ง worker ไว้เว้นแต่ middleware เป็น stateless จริง ๆ
Worker ที่ crash จะถูก restart อัตโนมัติและ retry hook ที่ค้างอยู่หนึ่งครั้ง
