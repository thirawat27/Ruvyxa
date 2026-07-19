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
    prerenderCache: true,
  },
  css: {
    entries: ['styles/theme.css'],
  },
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
    plugins: [
      {
        name: 'auth-guard',
        path: 'plugins/auth.wasm',
        phase: 'request',
        routes: ['/api/*'],
        config: {},
        allow: { env: ['AUTH_SECRET'], timeout: 5000, memory: 67108864 },
      },
    ],
  },
  adapter: undefined,
  adapterOptions: {},
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
| `split`          | `"route"`     | `"single"`, `"route"`, `"manual"`                                                                                                                         |
| `jsx`            | `"automatic"` | `"classic"`, `"automatic"`                                                                                                                                |
| `target`         | `"es2022"`    | `es2018`–`esnext`                                                                                                                                         |
| `workers`        | CPU count     | จำนวน threads สำหรับ route preparation/final emission และ prerender                                                                                       |
| `manifest`       | `false`       | เปิดใช้ chunk manifest                                                                                                                                    |
| `warm`           | `true`        | Pre-bundle dev dependencies                                                                                                                               |
| `prerenderCache` | `true`        | ใช้ HTML จาก SSG/ISR/PPR ซ้ำเมื่อ config, environment, assets, styles และ source fingerprints ตรงกัน; ปิดได้สำหรับหน้าที่ตั้งใจให้ผลลัพธ์เปลี่ยนทุก build |

### plugins

ปลั๊กอิน JavaScript จะทำงานผ่าน Node worker แบบ persistent โดยค่าเริ่มต้น hook จะถูก serialize
เพื่อรักษา state เดิมของปลั๊กอิน หาก hook ทุกตัวที่ใช้ `resolveId` หรือ `transform` เป็น
deterministic และไม่พึ่ง mutable state ใน process ให้กำหนด `parallel: true` เพื่อเปิด pool สูงสุด 8
workers (ยังถูกจำกัดด้วย `build.workers`) แต่ละ worker เป็น process แยกจากกัน

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

| Field             | Default                    | คำอธิบาย                                                         |
| ----------------- | -------------------------- | ---------------------------------------------------------------- |
| `actionLimit`     | 1 MiB (1,048,576 bytes)    | ขนาดสูงสุดของ action request body                                |
| `apiLimit`        | 10 MiB (10,485,760 bytes)  | ขนาดสูงสุดของ API request body                                   |
| `pluginLimit`     | 32 MiB                     | ขนาดสูงสุดของ response-phase Wasm plugin buffer (สูงสุด 256 MiB) |
| `actionRateLimit` | `{ max: 600, window: 60 }` | อัตรา request สูงสุด / client-action / วินาที                    |
| `sameOrigin`      | `true`                     | บังคับ Same-Origin check สำหรับ actions                          |
| `fetchMeta`       | `true`                     | บังคับ Fetch Metadata guard สำหรับ actions                       |
| `trustedProxyIps` | `[]`                       | IP ของ reverse proxy ที่อนุญาตให้ส่ง forwarded headers           |
| `headers`         | `true`                     | เปิดใช้ security headers ทั้งหมด                                 |

### middleware

ใช้ Tower-based middleware ผ่าน config:

- `builtin`: เปิด `timing`, `log`, `cors`, `rate`, `headers` ตามต้องการ
- `plugins`: array ของ Wasm plugins ที่มี `name`, `path`, `phase`, `routes`, `config`, `allow`

### adapter

| Adapter                      | เป้าหมาย           |
| ---------------------------- | ------------------ |
| `@ruvyxa/adapter-node`       | Node.js server     |
| `@ruvyxa/adapter-vercel`     | Vercel serverless  |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers |
| `@ruvyxa/adapter-netlify`    | Netlify Functions  |
| `@ruvyxa/adapter-bun`        | Bun runtime        |
| `@ruvyxa/adapter-static`     | Static hosting     |

`adapterOptions` ใช้ส่งข้อมูลเพิ่มเติมไปยัง adapter โดยทั้ง adapter output และ options จะถูกเขียนลง
`.ruvyxa/build.json`
