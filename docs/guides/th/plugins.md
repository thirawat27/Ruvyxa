# Plugins

> 🔴 **ขั้นสูง** · ⏱️ อ่าน ~10 นาที
>
> **จะได้เรียนรู้:** ต่อขยาย build และ request pipeline ด้วย TypeScript plugin — transform,
> middleware, build-complete hook — แอปส่วนใหญ่ไม่จำเป็นต้องเขียน plugin เอง

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

Ruvyxa มี official package สำหรับ state ของแอปเพิ่มอีก 3 ตัว:

- `@ruvyxa/database` — facade CRUD/transaction แบบ typed โดย `prismaAdapter()` ใช้กับ PostgreSQL,
  MySQL, SQLite และ MongoDB ส่วน `dynamoAdapter()` รับ AWS transport แบบ explicit
- `@ruvyxa/auth` — credentials, OAuth PKCE (มี helper Google/GitHub), magic link, WebAuthn ผ่าน
  verification adapter, secure session, atomic token store และ rate limit
- `@ruvyxa/realtime` — WebSocket แบบ native ที่ publish event จาก server action สำหรับ Node/Bun แบบ
  self-host

```ts
// ruvyxa.config.ts
import { databasePlugin } from '@ruvyxa/database'
import { realtime } from '@ruvyxa/realtime'
import { config } from 'ruvyxa/config'

export default config({
  plugins: [databasePlugin({ requiredEnv: ['DATABASE_URL'] }), realtime()],
})
```

ให้สร้าง database client และ auth runtime ใน server-only module ของแอป ห้ามใช้ module-global state
ใน config เป็น shared store เพราะ config plugin, middleware worker และ render/action worker อยู่คนละ
process ฝั่ง browser ของ Auth ต้อง import จาก `@ruvyxa/auth/client` และ Realtime จาก
`@ruvyxa/realtime/client`; root import ของ `@ruvyxa/auth` และ `@ruvyxa/database` ใน client graph
จะถูก ปฏิเสธด้วย `RUV1007`

Native Realtime รองรับ `ruvyxa dev` และ Node/Bun self-host ผ่าน `ruvyxa start` เท่านั้น ส่วน Static,
Vercel, Netlify, Cloudflare และ Edge จะ fail ตอน build ด้วย `RUV3201` เพราะ adapter เหล่านี้ไม่มี
persistent WebSocket process กลาง สำหรับ Auth ให้ใช้ `auth.plugin` บน self-host middleware หรือ
เรียก `auth.handle(request)` จาก API route ของ serverless อ่าน flow, endpoint, security invariant
และ compatibility matrix ฉบับเต็มได้ที่
[Official Data, Auth, and Realtime Packages](../../architecture/official-plugins.md)

`ruvyxa/plugins` ยังเป็นชุด zero-install built-in plugin ที่ใช้ public hooks ชุดเดียวกัน:

`ruvyxa/plugins` มี plugin สำเร็จรูปที่สร้างบน public hooks ชุดเดียวกัน:

```ts
import { config } from 'ruvyxa/config'
import {
  cacheRules,
  contentEngine,
  feed,
  observability,
  openApi,
  pwa,
  robots,
  searchIndex,
  securityHeaders,
} from 'ruvyxa/plugins'

export default config({
  plugins: [
    observability({ routes: ['/api/*'] }),
    securityHeaders({
      contentSecurityPolicy: {
        'default-src': ["'self'"],
        'object-src': ["'none'"],
      },
    }),
    cacheRules([
      { source: '/api/*', browser: 'no-store' },
      { source: '/blog/*', browser: 'public, max-age=60', cdn: 'max-age=300' },
    ]),
    pwa({ name: 'Example', offlineFallback: '/offline' }),
    robots({
      sitemap: 'https://example.com/sitemap.xml',
      openAi: { search: true, training: false },
    }),
    contentEngine({
      siteUrl: 'https://example.com',
      title: 'Example',
      description: 'บทความล่าสุด',
      locale: 'th',
    }),
    openApi({
      info: { title: 'Example API', version: '1.0.0' },
      operations: [{ method: 'get', path: '/api/health', summary: 'ตรวจสุขภาพระบบ' }],
    }),
  ],
})
```

- `redirects(rules)` — redirect แบบ declarative ก่อนถึงขั้น render ใช้ path ตรงตัวหรือ prefix
  ที่ลงท้าย ด้วย `*` ได้ ถ้า destination ลงท้ายด้วย `*` ส่วนที่เหลือของ path จะถูกต่อท้ายให้ และ
  `permanent: true` ตอบ 308 แทน 307
- `headers(rules)` — กำหนด response header ต่อ route กติกาที่ไม่ระบุ `source` จะมีผลทุกหน้า
- `observability({ routes, requestIdHeader, traceContext, serverTiming, log, logger })` — ตรวจและส่ง
  request ID กับ W3C `traceparent` ต่อ วัดเวลาข้าม middleware worker เพิ่ม `Server-Timing` และ log
  เฉพาะ method/path/status โดยไม่เก็บ query string ถ้ามี log pipeline อยู่แล้วให้ใช้ `log: false`
  หรือส่ง `logger(entry)` ถ้า custom logger ขัดข้อง ระบบจะแจ้งข้อผิดพลาดแต่จะไม่ทำให้ response
  ของแอปล้ม
- `securityHeaders(options)` — เพิ่ม HSTS เป็นค่าเริ่มต้น และรองรับ CSP, permissions, referrer,
  cross-origin, frame และ header อื่น ค่า explicit จาก plugin จะชนะ native default ส่วน CSP ต้องเปิด
  เองเพราะ policy เดียวไม่สามารถใช้ได้ปลอดภัยกับทุกแอป
- `cacheRules(rules)` — ตั้ง `Cache-Control` สำหรับ browser, `CDN-Cache-Control` สำหรับ shared cache
  และรวมค่า `Vary` ตาม route ถ้ามีหลายกฎตรงกัน กฎหลังสุดจะชนะสำหรับ cache policy
- `sitemap({ siteUrl, exclude, robots })` — เขียน `sitemap.xml` (และ `robots.txt` ถ้าเปิด) ลง
  โฟลเดอร์ asset ที่เสิร์ฟจริงหลังจบ production build โดยอ่านจาก route manifest ข้าม dynamic route
  และ API route ให้อัตโนมัติ
- `robots({ rules, sitemap, openAi })` — สร้าง `robots.txt` แยกเดี่ยว โดย preset `openAi` แยกควบคุม
  OAI-SearchBot (`search`) ออกจาก GPTBot (`training`) และจะปฏิเสธ rule ของ agent เดียวกันที่กำหนดซ้ำ
  เพื่อไม่ให้เกิด policy กำกวม
- `pwa(options)` — สร้างและเสิร์ฟ web manifest, service worker และ registration module พร้อม inject
  tag ให้ HTML response และ prerendered HTML ที่ตรง route ควรกำหนด `precache` กับ `offlineFallback`
  เองเพื่อไม่ให้ service worker เดาข้อมูลที่ต้อง cache โดย cache namespace จะแยกตาม service-worker
  scope จึงไม่ลบ cache ข้ามกันแม้หลายแอปใช้ origin เดียวกัน
- `contentEngine({ siteUrl, title, description, ... })` — สแกน route แบบ native ที่
  `app/**/page.md(x)` เพียงครั้งเดียว แล้วสร้าง `/content.json`, `/search-index.json`, `/rss.xml`,
  `/sitemap.xml` และ link/answer index แบบ experimental ที่ `/llms.txt` จาก frontmatter กับเนื้อหา
  ชุดเดียว ระหว่างพัฒนาไฟล์จะอัปเดตสด และตอน production จะเขียนผลลัพธ์ที่ตรงกันทุก byte ระบบตัด
  route group, draft และโฟลเดอร์ private ออก พร้อมข้าม dynamic route ที่ยังไม่มี canonical path
  รองรับ metadata `title`, `description`/`summary`, `tags`, `publishedAt`/`date`, `updatedAt`,
  `author`, `answers` และ `draft` โดย citation URL จะถูก normalize เป็น HTTP(S) สาธารณะ ส่วน custom
  frontmatter ที่เป็น JSON-compatible จะคงอยู่ใน content manifest ใช้ `llmsPath: false` เพื่อปิด
  ไฟล์ experimental หรือกำหนด public path ใหม่
- `feed({ siteUrl, title, description, items, path })` — สร้าง RSS 2.0 จาก array หรือ async loader
  ตอน build โดยค่า output เริ่มต้นคือ `/rss.xml`
- `searchIndex({ documents, locale, stopWords, minTermLength, path })` — สร้าง inverted index แบบ
  deterministic เป็น JSON และใช้ `Intl.Segmenter` แบ่งคำภาษาไทยได้ ค่าเริ่มต้นคือ
  `/search-index.json`
- `openApi({ info, operations, servers, tags, components, path })` — ตรวจ method/path และ
  `operationId` ไม่ให้ซ้ำ เสิร์ฟ OpenAPI 3.1 ระหว่าง dev และเขียน `/openapi.json` ตอน build
- `alias(map)` — จับคู่ import specifier แบบตรงตัวไปยังไฟล์ในโปรเจกต์ก่อนถึง native resolver
- `bundleBudget({ maxChunkKb, maxTotalKb })` — ทำให้ production build ล้มเหลวเมื่อ client JavaScript
  เกินงบที่ตั้งไว้ ช่วยจับ bundle regression ได้ตั้งแต่ใน CI
- `requireEnv(names)` — ทำให้ production build ล้มเหลวเมื่อ environment variable ที่จำเป็นหายไป
  หรือว่างเปล่า

ถ้า RSS, search และ sitemap มาจากชุด Markdown/MDX เดียวกัน ให้ใช้ `contentEngine()` แทน `feed()`,
`searchIndex()` และ `sitemap()` แบบแยกตัว หากแอปจำเป็นต้องใช้ร่วมกัน ต้องตั้ง output path ให้ต่างกัน
เพื่อไม่ให้ plugin สองตัวเขียนทับ artifact เดียวกัน

`answers` ต้องเป็น `question` และ `answer` ที่ผู้เขียนระบุเอง และใส่ `sources: [{ name, url }]`
เพิ่มได้ ให้นำข้อมูลชุดเดียวกันไป render ให้ผู้ใช้เห็นด้วย `Answer` จาก `@ruvyxa/react`; Content
Engine จะไม่เดาคำตอบหรือสร้าง FAQ/QAPage markup ให้เอง ส่วน `llms.txt` เป็นเพียง discovery aid แบบ
experimental ไม่ได้แทน HTML ที่ index ได้, structured data ที่ตรงจริง, canonical URL
หรือความสดใหม่ของ sitemap

ไฟล์ public ที่ plugin สร้างจะเสร็จก่อน adapter materialize output ดังนั้น Content Engine, sitemap,
PWA, feed, search index และ OpenAPI จะติดไปกับ static/hybrid deployment artifact ด้วย
ไม่ได้อยู่แค่ใน `.ruvyxa` ฝั่ง local ส่วน static adapter จะรักษา URL ให้ตรง production server คือ
public file อยู่ที่ `/...` และ client bundle อยู่ใต้ `/__ruvyxa/client/...`
ไฟล์ที่สร้างจะถูกแทนที่แบบ atomic และ path ของ artifact จะถูกตรวจไม่ให้เป็น cross-origin, traversal,
directory หรือ endpoint ของ PWA ที่ชนกันตั้งแต่ตอนอ่าน config

`observability`, `securityHeaders` และ `cacheRules` เป็น runtime response plugin จึงทำงานตามปกติบน
serverless หรือ long-running adapter แต่ static host ล้วนไม่มี middleware runtime ต้องตั้ง security/
cache header ที่เทียบเท่ากันใน host หรือ adapter นั้นเพิ่มเอง

`routes` ของ middleware จะถูกส่งให้ native server ด้วย ทำให้ request ที่ไม่มีทาง match ข้ามการ
round-trip ไปยัง plugin runtime ทั้งหมด — จึงควรระบุ route ให้ middleware เสมอเมื่อทำได้ pattern
ต้องเป็น `*`, exact path ที่ขึ้นต้นด้วย `/` หรือ prefix ที่ลงท้ายด้วย `*` เท่านั้น pattern ที่ผิดจะ
fail ตั้งแต่เริ่ม plugin แทนการถูกข้ามแบบเงียบ ๆ

## Middleware worker pool

โดยปกติ plugin middleware ทำงานบน runtime process เดียวแบบ persistent ถ้า middleware แบบ stateless
บน route ที่ traffic สูงกลายเป็นคอขวด ใช้ `middleware.workers` (1–8) เพื่อเปิด pool ของ runtime
process ที่เหมือนกันแบบ round-robin:

```ts
export default config({
  middleware: {
    workers: 2,
    timeoutMs: 15_000,
  },
})
```

Worker แต่ละตัวไม่แชร์ state ระดับ module ของ plugin — ตัวนับ, cache หรือ session ที่เก็บใน module
scope จะแยกต่อ process ดังนั้นคงค่า default หนึ่ง worker ไว้เว้นแต่ middleware เป็น stateless จริง ๆ
pool จะเลือก worker ที่ว่างก่อนต่อคิวหลัง worker ที่กำลังทำงาน `timeoutMs` จำกัดเวลาของ middleware
hook แต่ละครั้ง (ค่าเริ่มต้น 30,000; ช่วง 1–300,000 ms) Worker ที่ crash จะถูก restart และ retry
hook เดิมหนึ่งครั้ง ส่วน hook ที่ timeout หรือส่ง protocol ผิดจะเปลี่ยน worker โดยไม่ retry เพราะ
hook อาจทำ side effect ไปแล้ว
