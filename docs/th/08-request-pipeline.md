# Request pipeline: headers, redirects, rewrites, proxy และ route handler

> **เป้าหมายบทเรียน:** เพิ่มพฤติกรรม cross-cutting ของ request ครั้งเดียวใน `ruvyxa.config.ts`
> แล้วให้ทำงานเหมือนกันทั้งใน `ruvyxa dev`, `ruvyxa start` และทุก deployed build **เริ่มจาก:**
> แอปที่ตั้งค่าแล้วใน [Configuration](07-configuration.md) **จุดตรวจ:** ตรวจ path ที่ match หนึ่ง
> เส้นและไม่ match หนึ่งเส้นหลังเพิ่ม rule หรือ proxy

Ruvyxa ไม่มีระบบ plugin ทุกอย่างที่เคยเป็น plugin กลายเป็นหนึ่งในสามอย่าง: key บน config object
(`headers()`, `redirects()`, `rewrites()`, `proxy`, `realtime`, `collab`, `content`), file
convention (`instrumentation.ts`) หรือ route handler ใต้ `app/` (`route.ts`)
แต่ละอย่างประกาศครั้งเดียวและถูกประเมินโดย request host ทั้งสอง — Axum server เบื้องหลัง
`dev`/`start` และ handler ที่ทุก adapter deploy — จากตารางร่วมตารางเดียว rule จึงไม่มีทางทำงานบน
host หนึ่งแต่ไม่ทำงานบนอีก host

## ลำดับการประเมิน

ทุก request ทำตามลำดับนี้:

1. `headers()` — ตัดสินจาก request ตามที่มาถึง แล้วตั้งบน response ที่ตอบ
2. `redirects()` — ตอบด้วย `308` (`permanent: true`) หรือ `307`
3. `proxy.handler` — สำหรับ path ที่ `proxy.matcher` ระบุ
4. `rewrites().beforeFiles` — ก่อน static file และ page
5. Static file และ page/API route
6. `rewrites().afterFiles` — หลัง file ก่อน dynamic route
7. Dynamic route
8. `rewrites().fallback` — หลังทุกอย่าง ก่อน 404

Framework endpoint ใต้ `/__ruvyxa/` ถูกตัดสินก่อนทั้งหมดบนทั้งสอง host ไม่มี rule ใด redirect
`/__ruvyxa/action` ได้ และ `proxy.handler` ไม่เห็นมันเลย

## Source pattern

`source` ในทุก rule และแต่ละ entry ของ `proxy.matcher` เป็น pattern แบบ path-to-regexp ที่ match กับ
canonical request path — decode แล้ว ไม่มี trailing slash `/` สำหรับ root — ทั้งเส้นและไม่
สนตัวพิมพ์เล็กใหญ่:

| Pattern               | Match                                       |
| --------------------- | ------------------------------------------- |
| `/about`              | `/about`, `/About`                          |
| `/blog/:slug`         | หนึ่ง segment; `slug` เป็น parameter        |
| `/blog/:slug*`        | ศูนย์หรือมากกว่า segment ต่อกันเป็น `a/b/c` |
| `/docs/:path+`        | หนึ่งหรือมากกว่า segment                    |
| `/shop/:category?`    | ศูนย์หรือหนึ่ง segment                      |
| `/post/:id(\\d{1,})`  | segment ที่ match regex ในวงเล็บ            |
| `/((?!api\|_next).*)` | unnamed group ได้ parameter `0`             |
| `/:path*`             | ทุกอย่าง รวมถึง `/`                         |

Parameter แทนค่าเข้าไปใน `destination` และใน header key/value เป็น `:name` rule ยังมีเงื่อนไข `has`
และ `missing` บน `header`, `cookie`, `query` หรือ `host` ได้ `value` เป็น regex ที่ต้อง match
ทั้งค่า และ named capture ในนั้นกลายเป็น parameter ด้วย pattern ที่ compile ไม่ผ่านทำให้ config
ล้มด้วย `RUV1602` แทนที่จะพังตอน request แรกที่ match ทั้งสอง host replay
`tests/fixtures/route-rules-conformance.json` ตารางข้างบนจึงเป็น contract ไม่ใช่แค่คำอธิบาย

## `headers()`

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'

export default config({
  headers: [
    { source: '/api/:path*', headers: [{ key: 'cache-control', value: 'no-store' }] },
    {
      source: '/:path*',
      has: [{ type: 'host', value: 'admin.example.com' }],
      headers: [{ key: 'x-frame-options', value: 'DENY' }],
    },
  ],
})
```

Rule ทำงานตามลำดับ key ของ rule หลังทับ rule ก่อน list เป็นฟังก์ชันได้ทั้ง sync และ async security
header ปริยายของ Ruvyxa ยังถูกตั้งบนทุก response แต่ตั้งเฉพาะที่แอปยัง ไม่ได้ตั้ง header เดียวกัน
rule ใน `headers()` จึงชนะ

## `redirects()`

```ts
export default config({
  redirects: async () => [
    { source: '/old-blog/:path*', destination: '/blog/:path*', permanent: true },
    { source: '/docs/:path*', destination: 'https://docs.example.com/:path*', permanent: false },
    {
      source: '/legacy',
      destination: '/',
      statusCode: 302,
      missing: [{ type: 'header', key: 'x-do-not-redirect' }],
    },
  ],
})
```

`statusCode` ชนะ `permanent` ไม่เช่นนั้น `permanent: true` คือ `308` และอย่างอื่นคือ `307`
destination ที่ไม่มี query สืบทอด query string ของ request destination ถูก validate ตอน config: URL
แบบ absolute ต้องเป็น `http(s)` และแบบ relative ต้องเป็น absolute application path

## `rewrites()`

```ts
export default config({
  rewrites: {
    beforeFiles: [{ source: '/alias', destination: '/' }],
    afterFiles: [{ source: '/blog/:slug', destination: '/posts/:slug' }],
    fallback: [{ source: '/:path*', destination: '/not-found-page' }],
  },
})
```

list เปล่าคือ `afterFiles` parameter ที่ destination ไม่ได้ใช้จะถูกต่อเข้า query เว้นแต่ destination
ใช้ parameter ใดก็ตาม query ของ request ถูก merge เข้าด้วย deployed build จะ fetch destination แบบ
`https://` ส่วน native host เสิร์ฟ destination ภายในเท่านั้นและตอบ `502` สำหรับ destination ภายนอก

## `proxy`

โค้ดที่รันก่อนทุก route ที่ match เก็บไว้ในไฟล์ config เพื่อให้โปรเจกต์มีที่อ่านที่เดียว:

```ts
export default config({
  proxy: {
    matcher: [
      '/admin/:path*',
      { source: '/api/:path*', has: [{ type: 'header', key: 'x-block' }] },
    ],
    handler(request) {
      const url = new URL(request.url)
      if (!request.headers.has('authorization')) {
        return new Response('Unauthorized', { status: 401 })
      }
      if (url.pathname === '/admin') {
        return new Request(new URL('/admin/dashboard', url), request)
      }
      const headers = new Headers(request.headers)
      headers.set('x-request-start', String(Date.now()))
      return new Request(request, { headers })
    },
  },
})
```

`handler` รับ `Request` มาตรฐาน และคืน `Response` เพื่อตอบเลย คืน `Request` เพื่อทำต่อด้วย request
นั้น — path ต่างคือ rewrite, header ต่างจะถูกส่งต่อ — หรือไม่คืนอะไรเพื่อทำต่อโดยไม่ เปลี่ยน
`matcher` เป็น string, array ของ string หรือ entry ที่มี `source`, `has`, `missing` ถ้าไม่ ระบุ
handler จะรันทุก request matcher ถูกประเมินแบบ native บนทั้งสอง host บน Axum host request ที่
matcher ไม่ระบุจึงไม่ข้ามไปยัง JavaScript process ที่ถือ handler เลย

`handler` เป็นฟังก์ชัน จึงเป็นส่วนเดียวของ config ที่ยังเป็นโค้ด: native host รันมันใน project
worker ที่คงอยู่ (`middleware.workers` process, `middleware.timeoutMs` ต่อการเรียก) และทุก deployed
build compile มันเข้า function bundle แล้วรันในโปรเซสเดียวกัน มันรันเป็น trusted application code
ด้วยสิทธิ์เต็มของโปรเซส ให้ถือว่าสิ่งที่มัน import เป็นส่วนหนึ่งของแอป static adapter
รันมันไม่ได้และปฏิเสธ build ด้วย `RUV2204`

## การป้องกัน route handler

Server action ปฏิเสธ cross-site request บนทั้งสอง host แต่ handler ใต้ `app/api/` ไม่ทำเอง: มัน
เข้าถึงได้จากทุก origin และ session cookie แบบ `SameSite=Lax` ยังไปกับ cross-site form `POST` ปิด
ช่องนี้ใน `proxy.handler` สำหรับ route ที่เปลี่ยนสถานะ:

```ts
export default config({
  proxy: {
    matcher: '/api/:path*',
    handler(request) {
      if (['GET', 'HEAD', 'OPTIONS'].includes(request.method)) return undefined
      const origin = request.headers.get('origin')
      const host = request.headers.get('host')
      const sameOrigin = origin ? new URL(origin).host === host : false
      const fetchSite = request.headers.get('sec-fetch-site')
      if (sameOrigin || fetchSite === 'same-origin') return undefined
      return new Response('Forbidden', { status: 403 })
    },
  },
})
```

เป็นแบบต่อ route ไม่ใช่ค่าปริยาย เพราะ API ที่ตั้งใจให้เรียกจาก origin อื่นเป็น design ที่ถูก ต้อง
กรณีนั้นใช้ `middleware.builtin.cors` แทน

## ไฟล์แทน hook

สิ่งที่เคย generate ไฟล์ตอน build กลายเป็น route handler ที่ตอบ path เดียวกัน หรือ config key ที่
build เข้าใจอยู่แล้ว:

| ความต้องการ                                    | อยู่ที่ไหนตอนนี้                                                                                   |
| ---------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `sitemap.xml`, `robots.txt`                    | `site.sitemap` และ `site.robots` ใน config; `ruvyxa build` เขียนให้                                |
| `/content.json`, search index, RSS, `llms.txt` | `content: true` — ดู [Configuration](07-configuration.md#content-artifact)                         |
| `security.txt`, feed, manifest, OpenAPI        | `app/.well-known/security.txt/route.ts`, `app/feed.xml/route.ts` และอื่น ๆ                         |
| Health endpoint                                | `/__ruvyxa/health` บน Axum host และ standalone server หรือ `route.ts` ของคุณเอง                    |
| Environment ที่ต้องมีตอนเริ่ม                  | `register()` ใน `instrumentation.ts`; `@ruvyxa/database` มี `requireDatabaseEnv()`                 |
| Import alias                                   | `paths` ใน `tsconfig.json` ซึ่งทั้งสอง compiler เคารพ                                              |
| Realtime และ collaboration socket              | `realtime: true` และ `collab: true` — ดู [การเชื่อมต่อ](09-integrations-auth-data-and-realtime.md) |

```ts
// app/.well-known/security.txt/route.ts
export function GET() {
  return new Response('Contact: mailto:security@example.com\nExpires: 2027-01-01T00:00:00.000Z\n', {
    headers: { 'content-type': 'text/plain; charset=utf-8' },
  })
}
```

## Content-Security-Policy

หน้าเว็บไม่มี inline script ที่รันได้ของ Ruvyxa เอง route parameter และ request path เดินทางไป
client ใน `<script type="application/json">` data block ซึ่งเบราว์เซอร์ไม่รันและ `script-src`
ไม่บังคับ policy ที่เข้มจึงไม่ต้องใช้ nonce:

```ts
export default config({
  headers: [
    {
      source: '/:path*',
      headers: [{ key: 'content-security-policy', value: "default-src 'self'; script-src 'self'" }],
    },
  ],
})
```

Route ที่ stream เนื้อหา Suspense มี inline runtime ของ React เอง — script ที่สลับ boundary ที่
resolve แล้วเข้าที่ byte ของมันคงที่เมื่อเขียน artifact แล้ว hash จึงใช้ได้ แต่มันระบุ boundary id
ที่มันเติมจึงต่างกันตามเอกสาร policy ที่ต้องครอบคลุมควรจำกัด `script-src 'unsafe-inline'` เฉพาะ
route เหล่านั้นด้วย `source` ที่แคบกว่า แทนที่จะผ่อนทั้งเว็บ

**ก่อนหน้า:** [Configuration และ environment](07-configuration.md) · **ถัดไป:**
[การเชื่อมต่อ](09-integrations-auth-data-and-realtime.md)
