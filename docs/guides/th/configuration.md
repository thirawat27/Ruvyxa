# Configuration

> 🔴 **Reference** · ⏱️ เปิดหาเฉพาะที่ต้องใช้
>
> **ในบทนี้มี:** ทุก option ของ `ruvyxa.config.ts` — server, build, rendering, security, cache,
> styles — พร้อม default และ type

**มือใหม่ข้ามบทนี้ไปก่อนได้เลย** — โปรเจกต์ใหม่ทำงานได้โดยไม่ต้องแตะ `ruvyxa.config.ts` ที่ generate
มา ทุก option มี default ที่เหมาะสมอยู่แล้ว กลับมาอ่านเมื่อต้องเปลี่ยน port, ใส่ adapter, จูน cache
หรือเข้มงวด security และใช้ autocomplete ใน editor สำรวจได้เลย (`config()` มี type ครบ)

`ruvyxa.config.ts` — ใช้ `config()` เพื่อ TypeScript validation:

```ts
import { config } from 'ruvyxa/config'

export default config({
  appDir: 'app',
  outDir: '.ruvyxa',
  server: { host: 'localhost', port: 3000 },
  build: {
    minify: true,
    map: false,
    treeShake: true,
    split: 'route',
    target: 'es2022',
    workers: 4,
    jsx: 'automatic',
    manifest: false,
    warm: true,
    prerenderCache: true,
  },
  css: {
    entries: ['styles/theme.css'],
  },
  plugins: [],
  render: { strategy: 'ssr', revalidate: 60 },
  cache: { routes: true, css: true, dir: '.ruvyxa/cache/bundler' },
  debug: { overlay: true, traces: false },
  image: { optimize: true, quality: 82, lossless: false, keepOriginal: true, workers: 0 },
  security: {
    actionLimit: 1024 * 1024,
    apiLimit: 10 * 1024 * 1024,
    pluginLimit: 32 * 1024 * 1024,
    actionRateLimit: { max: 600, window: 60 },
    sameOrigin: true,
    fetchMeta: true,
    trustedProxyIps: [],
    headers: true,
  },
  middleware: {
    builtin: {
      timing: true,
      log: true,
      cors: {
        origins: ['https://myapp.com'],
        methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'],
        credentials: true,
        maxAge: 86400,
      },
      rate: { max: 100, window: 60, key: 'ip' },
      headers: { 'X-Powered-By': 'Ruvyxa' },
    },
  },
})
```

Key ที่ไม่รู้จักจะทำให้ config ล้มเหลวโดยเจตนา — ป้องกัน typo ไม่ให้เปลี่ยนพฤติกรรม deployment
โดยเงียบ

## รายละเอียดแต่ละ Section

### appDir / outDir

| Field    | Default     | คำอธิบาย                  |
| -------- | ----------- | ------------------------- |
| `appDir` | `"app"`     | ไดเรกทอรีต้นทางของ routes |
| `outDir` | `".ruvyxa"` | ไดเรกทอรีผลลัพธ์ build    |

### server

| Field  | Default       | คำอธิบาย     |
| ------ | ------------- | ------------ |
| `host` | `"localhost"` | Bind address |
| `port` | `3000`        | Port         |

### build

| Field            | Default       | Options                                                                                                                                                   |
| ---------------- | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `minify`         | `true`        | Oxc minification                                                                                                                                          |
| `map`            | `false`       | Source maps                                                                                                                                               |
| `treeShake`      | `true`        | Tree-shaking                                                                                                                                              |
| `split`          | `"route"`     | `"single"`, `"route"` (`"manual"` เป็น alias ของ `"single"`)                                                                                              |
| `jsx`            | `"automatic"` | `"classic"`, `"automatic"`                                                                                                                                |
| `target`         | `"es2022"`    | `es2018`–`esnext`                                                                                                                                         |
| `workers`        | CPU count     | จำนวน threads สำหรับ route preparation/final emission และ prerender                                                                                       |
| `manifest`       | `false`       | เปิดใช้ chunk manifest                                                                                                                                    |
| `warm`           | `true`        | Pre-bundle dev dependencies                                                                                                                               |
| `prerenderCache` | `true`        | ใช้ HTML จาก SSG/ISR/PPR ซ้ำเมื่อ config, environment, assets, styles และ source fingerprints ตรงกัน; ปิดได้สำหรับหน้าที่ตั้งใจให้ผลลัพธ์เปลี่ยนทุก build |

### runtime

`runtime` เลือก JavaScript runtime ที่ใช้รัน config, SSR, static rendering, API routes, actions และ
plugins ของ Ruvyxa โดยไม่เปลี่ยน Rust HTTP server ค่าเริ่มต้นคือ Node

```ts
export default config({
  runtime: 'bun', // 'node' หรือ 'bun'; ไม่ระบุจะใช้ Node ก่อน แล้ว fallback เป็น Bun หากไม่มี Node
})
```

เมื่อไม่ระบุ runtime ระบบจะใช้ Node หากมี และสลับไป Bun อัตโนมัติหากไม่มี Node หากต้องการบังคับให้
Bun ถูกใช้ตั้งแต่การโหลด config ครั้งแรก ให้ตั้งค่า bootstrap override ในคำสั่งของแอป:
`RUVYXA_RUNTIME=bun bunx ruvyxa dev` ค่า override นี้เหมาะกับ CI และมีลำดับความสำคัญเหนือ `runtime`

เพื่อ backward compatibility ค่า `runtime: 'edge'` และ `runtime: 'static'` ยังทำงานเป็น build target
alias และจะใช้ Node รัน JavaScript สำหรับงาน deploy ใหม่ ให้ใช้ `ruvyxa build --target edge` หรือ
`ruvyxa build --target static` แทน

### plugins

ใช้ `plugin(name, middleware)` สำหรับ request/response middleware และใช้
`definePlugin({ name, setup })` เมื่อต้องลงทะเบียน `resolveId`, `transform` หรือ `onBuildComplete`
ทุก hook ทำงานตามลำดับที่ลงทะเบียนใน persistent plugin runtime และ middleware ใช้ Fetch
`Request`/`Response`

### css

| Field     | Default | คำอธิบาย                                      |
| --------- | ------- | --------------------------------------------- |
| `entries` | `[]`    | ไฟล์/ไดเรกทอรี global CSS ที่ไม่ได้ถูก import |

### render

| Field        | Default | คำอธิบาย                   |
| ------------ | ------- | -------------------------- |
| `strategy`   | `"ssr"` | Default rendering strategy |
| `revalidate` | —       | ISR interval (seconds)     |

### cache

| Field    | Default                   | คำอธิบาย                                     |
| -------- | ------------------------- | -------------------------------------------- |
| `routes` | `true`                    | เปิด/ปิด route render cache                  |
| `css`    | `true`                    | เปิด/ปิด CSS cache                           |
| `dir`    | `".ruvyxa/cache/bundler"` | ไดเรกทอรี build cache ที่แชร์ข้ามการ restart |

### debug

| Field     | Default | คำอธิบาย                    |
| --------- | ------- | --------------------------- |
| `overlay` | `true`  | Error overlay ในเบราว์เซอร์ |
| `traces`  | `false` | Debug trace logging         |

### image

| Field          | Default | คำอธิบาย                                |
| -------------- | ------- | --------------------------------------- |
| `optimize`     | `true`  | เปิด / ปิด image optimization           |
| `quality`      | `82`    | คุณภาพ WebP (1–100)                     |
| `lossless`     | `false` | โหมด lossless                           |
| `keepOriginal` | `true`  | เก็บไฟล์ PNG/JPEG ต้นฉบับไว้คู่กับ WebP |
| `workers`      | `0`     | จำนวน thread (0 = auto = CPU count)     |

`keepOriginal` มีไว้เพราะ `public/` คือสัญญาเรื่อง URL: ไฟล์ที่วางไว้ต้องถูกเสิร์ฟที่ path เดียวกัน
`ruvyxa dev` และ `ruvyxa start` แปลง `/logo.png` ไปหา `logo.webp` ให้เองได้ แต่ CDN (Vercel,
Netlify, Cloudflare, S3) ไม่มี fallback แบบนั้น ดังนั้น `<img src="/logo.png">` ธรรมดา จะ 404
เฉพาะตอน deploy เท่านั้น การเก็บต้นฉบับไว้ทำให้ทั้งสอง URL ใช้ได้ทุก host ถ้าต้องการให้
เสิร์ฟไฟล์เล็กจริง ให้ใช้ `<Image>` จาก `@ruvyxa/react` ซึ่งชี้ไปที่ WebP ให้แล้ว และปิด
`keepOriginal` ได้ก็ต่อเมื่อทุกจุดอ้างอิงรูปผ่าน `<Image>` ทั้งหมด

### security

| Field             | Default                    | คำอธิบาย                                                   |
| ----------------- | -------------------------- | ---------------------------------------------------------- |
| `actionLimit`     | 1 MiB (1,048,576 bytes)    | ขนาดสูงสุดของ action request body                          |
| `apiLimit`        | 10 MiB (10,485,760 bytes)  | ขนาดสูงสุดของ API request body                             |
| `pluginLimit`     | 32 MiB                     | ขนาดสูงสุดของ response middleware buffer (สูงสุด 256 MiB)  |
| `actionRateLimit` | `{ max: 600, window: 60 }` | อัตรา request สูงสุด / client-action / วินาที              |
| `sameOrigin`      | `true`                     | บังคับ Same-Origin check สำหรับ actions                    |
| `fetchMeta`       | `true`                     | บังคับ Fetch Metadata guard สำหรับ actions                 |
| `trustedProxyIps` | `[]`                       | IP ของ reverse proxy ที่อนุญาตให้ส่ง forwarded headers     |
| `headers`         | `true`                     | เติม security header ค่าเริ่มต้นเมื่อ response ยังไม่มีค่า |

### middleware

ใช้ Tower-based middleware ผ่าน config:

- `builtin`: เปิด `timing`, `log`, `cors`, `rate`, `headers` ตามต้องการ
- `workers`: จำนวน TypeScript middleware processes 1–8 (ค่าเริ่มต้น 1 และ state แยกต่อ process)
- `timeoutMs`: เวลาสูงสุดต่อ hook 1–300,000 ms (ค่าเริ่มต้น 30,000 ms) เมื่อ timeout ระบบจะเปลี่ยน
  worker โดยไม่ retry เพื่อป้องกัน side effect ซ้ำ
- `addMiddleware` รับ `onRequest` และ `onResponse` callbacks ที่ใช้ Fetch `Request` และ `Response`
  objects `resolveId`, `transform` และ `onBuildComplete` ใช้คู่กับ middleware ได้ ทุก hook
  ทำงานตามลำดับการลงทะเบียนผ่าน persistent plugin runtime

### adapter

| Adapter                      | เป้าหมาย                  | Route ที่ deploy ได้ |
| ---------------------------- | ------------------------- | -------------------- |
| `@ruvyxa/adapter-node`       | Node launcher             | ทั้งหมด              |
| `@ruvyxa/adapter-bun`        | Bun launcher              | ทั้งหมด              |
| `@ruvyxa/adapter-vercel`     | Vercel static + function  | ทั้งหมด              |
| `@ruvyxa/adapter-netlify`    | Netlify static + function | ทั้งหมด              |
| `@ruvyxa/adapter-cloudflare` | Worker + asset binding    | SSR, SSG, CSR, API   |
| `@ruvyxa/adapter-static`     | Static files              | SSG, CSR             |

Adapter จะ materialize deployment artifact ภายใน `.ruvyxa/` หลัง build และบันทึกผลไว้ใน
`adapterArtifacts` ของ `.ruvyxa/build.json` Node/Bun สร้าง launcher ส่วน Cloudflare/Netlify/Vercel
เป็นแบบ hybrid คือสร้าง static publish directory สำหรับหน้า pre-render และ client asset ควบคู่กับ
serverless function ที่ให้บริการ route แบบ SSR และ API

function artifact จะบรรจุ `route-modules.mjs` ซึ่งเป็น static registry bundle ที่ compile แล้ว
สำหรับ platform handler โดย handler จะไม่รันไฟล์ `.ts`/`.tsx` ที่คัดลอกมาโดยตรง

Adapter แต่ละตัวประกาศชนิด route และ render strategy ที่ deploy ได้ ถ้ามี route อยู่นอกชุดนั้น build
จะถูกปฏิเสธด้วย `RUV2202` พร้อมระบุชื่อ route ที่ไม่รองรับ ก่อนที่ `build()` ของ adapter จะทำงาน

Cloudflare ไม่รองรับ ISR และ PPR เพราะ asset binding ของ Worker เป็นแบบอ่านอย่างเดียว
จึงไม่มีที่ให้เขียนหน้าที่ revalidate แล้ว ส่วน static adapter ไม่มี server เลย
