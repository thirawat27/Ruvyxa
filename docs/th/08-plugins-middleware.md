# Plugin และ middleware

> **เป้าหมายของ tutorial:** เพิ่มพฤติกรรมที่ใช้ร่วมกันครั้งเดียว แล้วใช้กับ route ที่ต้องการ
> **เริ่มจาก:** แอปที่กำหนดค่าแล้วใน [Configuration](07-configuration.md) **Checkpoint:** ตรวจ route
> ที่ตรงและไม่ตรงอย่างละหนึ่งรายการหลังเปิดใช้ plugin หรือ middleware rule

plugin คือ value ที่คืนจาก `definePlugin()` ใน `ruvyxa/plugin` (ถูก re-export โดย `ruvyxa` ด้วย)
เพิ่มมันใน `plugins` ของ `ruvyxa.config.ts` plugin ต้องมีชื่อที่ไม่ว่าง และต้องมี declarative
behavior หรือ `register(api)` definition ที่ไม่ถูกต้องล้มเหลวด้วย `RUV2102`

## Declarative plugin

```ts
// plugins/request-id.ts
import { definePlugin } from 'ruvyxa/plugin'

export const requestId = definePlugin({
  name: 'example:request-id',
  http: {
    match: ['/api/*'],
    onResponse({ response }) {
      const headers = new Headers(response.headers)
      headers.set('x-example', 'enabled')
      return new Response(response.body, { status: response.status, headers })
    },
  },
})
```

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { requestId } from './plugins/request-id'
export default config({ plugins: [requestId] })
```

`http.match` ใช้ path แบบ exact หรือ prefix ที่ลงท้าย `*` request hook คืน `Request`, `Response`
หรือไม่คืนค่าได้ response hook คืน `Response` หรือไม่คืนค่าได้ `http.routes` ประกาศ plugin-owned
route แบบ exact และรับ method เดียว หลาย method หรือทุก method เมื่อไม่ระบุ advanced `register` API
เปิด socket `http`, `build`, `dev`, `diagnostics`, `native` และ `head`

## Build และ dev lifecycle

build hook คือ `onStart`, `onResolve`, `onLoad`, `onTransform` และ `onComplete`
resolve/load/transform hook รับ environment เป็น `client`, `server`, `edge`, `worker` หรือ `shared`;
transformation คืน code, `{ code, map }`, null หรือไม่คืนค่า dev เปิด `onFileChange` registration
plugin report diagnostic และเพิ่ม document-head entry ได้ อย่าพึ่งพา module-level middleware state
ข้าม worker: config ระบุชัดว่า worker ไม่ share state นี้

### `onResolve` ต้องคืน path ไม่ใช่ virtual id

resolve hook คืนค่าเป็น **file path** ตัวไฟล์จะเป็น virtual ก็ได้ — `onLoad` hook เป็นคนให้เนื้อหา
และไม่ต้องมีไฟล์จริงบนดิสก์ — แต่ค่าที่คืนยังต้องระบุตำแหน่ง
เพราะทุกอย่างถัดจากนั้นปฏิบัติกับมันเป็น path สองรูปแบบที่ ecosystem อื่นใช้เรียก virtual module
ไม่ใช่ path และจะถูกปฏิเสธพร้อมระบุชื่อ:

```ts
build.onResolve(({ id, root }) =>
  id === 'stress-virtual' ? `${root}/virtual-stress-virtual.ts` : undefined,
)
```

`'�stress-virtual'` และ `'virtual:stress-virtual'` เคยถูก join เข้ากับ project root แล้วส่งให้
filesystem จึงโผล่มาเป็น OS error ดิบ ๆ — `strings passed to WinAPI cannot contain NULs` และ
`The system cannot find the file specified` — โดยไม่บอกว่าปลั๊กอินตัวไหน ตอนนี้ทั้งคู่ล้มเหลวพร้อม
diagnostic ที่ระบุชื่อปลั๊กอิน specifier และรูปแบบ path ที่ควรคืนแทน

### `onTransform` แก้บันเดิลฝั่งเบราว์เซอร์ ไม่ได้แก้การเรนเดอร์ฝั่งเซิร์ฟเวอร์

build จะคอมไพล์แต่ละโมดูลสองครั้ง และมีแค่ครั้งเดียวที่เรียก hook ของคุณ
บันเดิลฝั่งเบราว์เซอร์สร้างโดย Rust bundler ซึ่งเรียก `onTransform` ส่วนการเรนเดอร์ฝั่งเซิร์ฟเวอร์ —
`dev`, `start`, การ prerender และทุกฟังก์ชันที่ deploy — อ่านไฟล์เดียวกันผ่านคอมไพเลอร์ฝั่ง
JavaScript ซึ่งไม่เรียก ในทางปฏิบัติ `environment` ที่อยู่ใน transform จึงเป็น `client` เสมอ

แบบนี้ไม่มีปัญหากับอะไรที่มีแต่เบราว์เซอร์เห็น แต่ผิดทันทีถ้าค่านั้นไปโผล่ใน markup:

```ts
// ค่าที่ถูกเขียนทับถูกเรนเดอร์โดยทั้งสองฝั่ง จากซอร์สคนละชุด
build.onTransform(({ code, id }) =>
  id.endsWith('/marker.ts') ? code.replace("'original'", "'rewritten'") : undefined,
)
```

```tsx
// app/page.tsx — เรนเดอร์ฝั่งเซิร์ฟเวอร์แล้ว hydrate
export default () => <p>{MARKER}</p> // เซิร์ฟเวอร์เขียน "original" เบราว์เซอร์คาดหวัง "rewritten"
```

เมื่อสองฝั่งไม่ตรงกัน React จะทิ้ง tree ทั้งหมดที่เซิร์ฟเวอร์ส่งมาแล้วเรนเดอร์ใหม่ (#418)
ไม่มีอะไรพัง — หน้าเว็บได้ผลลัพธ์ถูกต้องหลังจากกะพริบให้เห็นค่าที่ผิดหนึ่งครั้ง
และในโปรดักชันไม่มีคำเตือนใด ๆ เลย `ruvyxa build` จะรายงานคู่ที่เสี่ยงให้ — โมดูลที่ถูก transform
ซึ่งมี route ที่ทั้งเรนเดอร์ฝั่งเซิร์ฟเวอร์และ hydrate เข้าถึงได้

มีสองทางที่ทำให้มันตรงกัน: ย้ายค่าไปอยู่หลัง route ที่เป็น `'use client'`
ซึ่งไม่มีเอกสารฝั่งเซิร์ฟเวอร์ให้ขัดกัน — นี่คือสิ่งที่ `examples/demo` ทำ —
หรือคำนวณค่าตอนรันไทม์ผ่าน environment variable หรือโมดูลที่เซิร์ฟเวอร์ อ่านด้วย
แทนการเขียนทับข้อความในซอร์ส

## First-party plugin

`ruvyxa/plugins` มี implementation ของ `redirects`, `headers`, `observability`, `securityHeaders`,
`cacheRules`, `sitemap`, `robots`, `alias` และ file-backed helper อื่นใน public entry point นั้น ใช้
validation ของมันแทนการเขียน behavior ซ้ำ ตัวอย่างเช่น redirect รับ `*`, path exact หรือ
trailing-prefix pattern และรับ destination เฉพาะ HTTP(S) URL แบบ absolute หรือ absolute path
ที่ปลอดภัย

```ts
import { redirects, securityHeaders } from 'ruvyxa/plugins'
export default config({
  plugins: [
    redirects([{ source: '/old/*', destination: '/new/*', permanent: true }]),
    securityHeaders({ contentSecurityPolicy: { 'default-src': ["'self'"] } }),
  ],
})
```

`permanent: true` ทำให้ `redirects` ส่ง 308; มิฉะนั้นส่ง 307 `securityHeaders` ให้ HSTS โดยปริยาย
แต่ไม่สามารถเลือก CSP ที่ปลอดภัยสำหรับ application ของคุณได้—ให้กำหนดอย่างตั้งใจและทดสอบ third-party
resource

## แค็ตตาล็อก first-party plugin

| Plugin                                | Output หรือ runtime behavior                                                                      |
| ------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `redirects`, `headers`, `cacheRules`  | route-scoped redirect, response header และ browser/CDN cache directive                            |
| `observability`, `securityHeaders`    | request ID/timing/structured log และ response security policy                                     |
| `pwa`                                 | manifest, service worker, registration script, optional precache/offline fallback และ HTML wiring |
| `sitemap`, `robots`, `feed`           | `sitemap.xml`, `robots.txt` และ RSS output ตอน build จาก metadata ที่ระบุ                         |
| `searchIndex`, `contentEngine`        | search index ตอน build และ content-derived answer/search artifact                                 |
| `openApi`                             | OpenAPI 3.1 JSON ที่ serve ตอน development และเขียนเข้า production output                         |
| `alias`, `bundleBudget`, `requireEnv` | import aliasing ตอน build, client JavaScript size limit และ required environment validation       |
| `fonts`                               | self-host Google Fonts stylesheet URL ที่ส่งให้ตอน build                                          |
| `originGuard`                         | บล็อก mutation request ข้าม origin ที่ยิงเข้า route handler เปิดใช้เองตาม route scope             |
| `healthCheck`                         | liveness endpoint ที่ตอบจาก request host ก่อนเข้า route rendering                                 |
| `webVitals`                           | เก็บ Core Web Vitals จาก browser แล้วรายงานฝั่ง server                                            |
| `llmsTxt`                             | `llms.txt` ตอน build จาก section ที่กำหนดเองและ route ที่ค้นพบ                                    |
| `wellKnown`                           | ไฟล์ใต้ `/.well-known/` รวมถึง `security.txt` ตาม RFC 9116                                        |
| `headScriptHashes`                    | CSP source hash สำหรับ inline script/style ที่ plugin ใส่เข้ามา                                   |

ใช้ข้อมูลแบบ explicit กับ build-time plugin: มันไม่ค้นหา business content หรือ API semantic
ของคุณให้เอง ตัวอย่างนี้เป็น PWA declaration ที่สมบูรณ์พร้อม `name` ที่จำเป็น:

```ts
import { pwa, openApi } from 'ruvyxa/plugins'

export default config({
  plugins: [
    pwa({
      name: 'Example app',
      icons: [{ src: '/icon-192.png', sizes: '192x192', type: 'image/png' }],
    }),
    openApi({
      info: { title: 'Example API', version: '1.0.0' },
      operations: [
        { method: 'GET', path: '/api/health', responses: { '200': { description: 'Healthy' } } },
      ],
    }),
  ],
})
```

PWA plugin ใช้ `/manifest.webmanifest`, `/sw.js` และ `/pwa-register.js` โดยปริยาย; path
ทั้งสามต้องต่างกัน `openApi` ใช้ `/openapi.json` โดยปริยาย, ต้องมี title/version ที่ไม่ว่าง
และปฏิเสธ method/path กับ `operationId` ที่ซ้ำ รัน production build และตรวจ generated output
ทุกครั้งที่เพิ่ม build plugin

## Build artifact ตอน development

`robots`, `feed`, `searchIndex`, `openApi`, `pwa`, `wellKnown` และ `webVitals` ตอบ request
สำหรับไฟล์ที่ตัวเองสร้างด้วย ดังนั้น `ruvyxa dev` จึง serve byte เดียวกับที่ build เขียนออกมา
และตรวจ output ได้โดยไม่ต้องรัน production build

`feed` กับ `searchIndex` ทำแบบนี้เฉพาะตอนที่ content เป็น static array ถ้าให้ loader มา
ทั้งคู่จะเป็น build-time อย่างเดียว: plugin แยกไม่ออกว่ากำลังอยู่ใน development หรือ production
ตอนรับ request การรัน loader ต่อ request จึงเท่ากับเอา file read หรือ database query ไปวางบน
response path ของ production `sitemap` และ `llmsTxt` เป็น build-time อย่างเดียว
ด้วยเหตุผลประเภทเดียวกัน — entry ของมันมาจาก route manifest ซึ่งยังไม่มีตอน development server ทำงาน

## การป้องกัน route handler

Server action ปฏิเสธ request ข้าม origin อยู่แล้วทั้งสอง host แต่ handler ใต้ `app/api/`
ไม่ได้ป้องกัน: มันเรียกได้จากทุก origin และ session cookie ใช้ `SameSite=Lax` โดยปริยาย ซึ่ง
cross-site form POST ยังพา cookie ไปด้วย `originGuard` ปิดช่องนั้นให้กับ route ที่ระบุ

```ts
import { healthCheck, originGuard, webVitals, wellKnown } from 'ruvyxa/plugins'

export default config({
  plugins: [
    originGuard({ routes: ['/api/*'] }),
    healthCheck({ path: '/health', check: () => ({ status: 'up' }) }),
    webVitals({ sampleRate: 0.1 }),
    wellKnown({
      securityTxt: {
        contact: 'mailto:security@example.com',
        expires: '2027-01-01T00:00:00.000Z',
      },
    }),
  ],
})
```

มันเป็น opt-in ไม่ใช่ค่าปริยาย เพราะ API ที่ตั้งใจให้เรียกจาก origin อื่นเป็นการออกแบบที่ถูกต้อง
กรณีนั้นให้ CORS เป็นตัวคุมแทน method ที่ไม่ปลอดภัยถูกตรวจโดยเทียบ `Origin` กับ `Host` ถ้า origin
ถูกถอดออกจะถอยไปใช้ `Sec-Fetch-Site: same-origin` และถ้าไม่มีทั้งสองอย่างจะ fail closed `webVitals`
publish client script เป็น build asset แล้วโหลดด้วย `src` จึงไม่บังคับให้ policy `script-src`
ต้องเปิด `'unsafe-inline'`

## Content-Security-Policy

หน้าเว็บไม่มี inline script ที่รันได้ของ Ruvyxa เองเลย route parameter และ request path เดินทางไปหา
client ผ่าน `<script type="application/json">` ซึ่ง browser ไม่ execute และ `script-src`
ไม่มีผลกับมัน policy แบบเข้มจึงไม่ต้องใช้ nonce:

```ts
securityHeaders({ contentSecurityPolicy: { 'default-src': ["'self'"], 'script-src': ["'self'"] } })
```

ยังเหลือสองอย่างที่ต้องครอบคลุม อย่างแรกคือ plugin ที่ใส่ inline `<script>` ผ่าน `head`
ซึ่งเหมือนกันทุก request จึงใช้ hash แทน nonce ได้:

```ts
import { headScriptHashes, securityHeaders } from 'ruvyxa/plugins'

const plugins = [analytics()]
export default config({
  plugins: [
    ...plugins,
    securityHeaders({
      contentSecurityPolicy: { 'script-src': ["'self'", ...headScriptHashes(plugins)] },
    }),
  ],
})
```

`headScriptHashes` จะไม่คืนอะไรให้ plugin ที่โหลด script ด้วย `src` — `webVitals` ตัว first-party
ถูกออกแบบมาแบบนั้นด้วยเหตุผลนี้ ใส่ `{ tag: 'style' }` เพื่อเอา hash สำหรับ `style-src`

อย่างที่สองคือ React เอง route ที่ stream เนื้อหา Suspense จะพา inline runtime ของ React มาด้วย —
script ที่ย้าย boundary ที่ resolve แล้วเข้าไปแทนที่ มันไม่ใช่ของ Ruvyxa จึงย้ายไปเป็น data block
ไม่ได้ และมันถูกเขียนลง build artifact ที่ทุก request ใช้ซ้ำ nonce ต่อ request จึงถูก bake
ติดไปและกลายเป็นค่าสาธารณะ ส่วน byte ของมันคงที่เมื่อ artifact ถูกเขียนแล้ว hash จึงเป็นกลไกที่เหมาะ
— แต่มันอ้างอิง boundary id ที่กำลังเติม จึงต่างกันไปในแต่ละหน้าและดูแลด้วยมือไม่ไหว

`inlineScriptHashes` ให้ build เป็นคนบันทึกให้:

```ts
securityHeaders({
  contentSecurityPolicy: { 'default-src': ["'self'"], 'script-src': ["'self'"] },
  inlineScriptHashes: true,
})
```

build จะเขียน `csp-inline-hashes.json` ลง output directory แล้วแต่ละ response จะหยิบ hash
ของหน้าที่ตัวเองเสิร์ฟไปใส่ — หน้าที่ไม่มี inline script ก็ได้ policy เดิมไม่เปลี่ยน ใส่
`{ outDir }` ถ้า build output ไม่ใช่ `.ruvyxa` และต้องมี `script-src` อยู่ใน policy อยู่แล้ว: policy
ที่ตั้งใจให้ fallback ไป `default-src` จะถูกปล่อยไว้เฉย ๆ เพราะการบีบให้เหลือแค่ hash
เหล่านี้จะบล็อก bundle ของแอปเอง ส่วนใน `ruvyxa dev` ยังไม่มีอะไรถูก build จึงไม่มี hash ถูกเพิ่ม

**ก่อนหน้า:** [Configuration และ environment](07-configuration.md) · **ถัดไป:**
[การเชื่อมต่อ](09-integrations-auth-data-and-realtime.md)
