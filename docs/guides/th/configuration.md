# Configuration

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
  image: { optimize: true, quality: 82, lossless: false, workers: 0 },
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

| Field      | Default | คำอธิบาย                            |
| ---------- | ------- | ----------------------------------- |
| `optimize` | `true`  | เปิด / ปิด image optimization       |
| `quality`  | `82`    | คุณภาพ WebP (1–100)                 |
| `lossless` | `false` | โหมด lossless                       |
| `workers`  | `0`     | จำนวน thread (0 = auto = CPU count) |

### security

| Field             | Default                    | คำอธิบาย                                                  |
| ----------------- | -------------------------- | --------------------------------------------------------- |
| `actionLimit`     | 1 MiB (1,048,576 bytes)    | ขนาดสูงสุดของ action request body                         |
| `apiLimit`        | 10 MiB (10,485,760 bytes)  | ขนาดสูงสุดของ API request body                            |
| `pluginLimit`     | 32 MiB                     | ขนาดสูงสุดของ response middleware buffer (สูงสุด 256 MiB) |
| `actionRateLimit` | `{ max: 600, window: 60 }` | อัตรา request สูงสุด / client-action / วินาที             |
| `sameOrigin`      | `true`                     | บังคับ Same-Origin check สำหรับ actions                   |
| `fetchMeta`       | `true`                     | บังคับ Fetch Metadata guard สำหรับ actions                |
| `trustedProxyIps` | `[]`                       | IP ของ reverse proxy ที่อนุญาตให้ส่ง forwarded headers    |
| `headers`         | `true`                     | เปิดใช้ security headers ทั้งหมด                          |

### middleware

ใช้ Tower-based middleware ผ่าน config:

- `builtin`: เปิด `timing`, `log`, `cors`, `rate`, `headers` ตามต้องการ
- `addMiddleware` รับ `onRequest` และ `onResponse` callbacks ที่ใช้ Fetch `Request` และ `Response`
  objects `resolveId`, `transform` และ `onBuildComplete` ใช้คู่กับ middleware ได้ ทุก hook
  ทำงานตามลำดับการลงทะเบียนผ่าน persistent plugin runtime

### adapter

| Adapter                      | เป้าหมาย          |
| ---------------------------- | ----------------- |
| `@ruvyxa/adapter-node`       | Node launcher     |
| `@ruvyxa/adapter-bun`        | Bun launcher      |
| `@ruvyxa/adapter-static`     | Static files      |
| `@ruvyxa/adapter-cloudflare` | Cloudflare static |
| `@ruvyxa/adapter-netlify`    | Netlify static    |
| `@ruvyxa/adapter-vercel`     | Vercel static     |

Adapter จะ materialize deployment artifact ภายใน `.ruvyxa/` หลัง build และบันทึกผลไว้ใน
`adapterArtifacts` ของ `.ruvyxa/build.json` Node/Bun สร้าง launcher;
static/Cloudflare/Netlify/Vercel สร้าง static publish directory และปฏิเสธ API, SSR, ISR และ PPR ด้วย
`RUV2202`
