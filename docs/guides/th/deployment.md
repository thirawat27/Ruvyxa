# Deployment

## Deployment Artifacts จาก Adapter

### Setup

ใช้ npm scripts มาตรฐาน:

```json
{
  "scripts": {
    "dev": "ruvyxa dev",
    "build": "ruvyxa build",
    "start": "ruvyxa start",
    "check": "ruvyxa check"
  }
}
```

เลือก adapter ใน `ruvyxa.config.ts` หรือระบุผ่าน command line โดยไม่ต้องแก้ config:

```bash
ruvyxa build --adapter vercel
```

`--adapter` รองรับ `node`, `bun`, `static`, `vercel`, `netlify`, `cloudflare` โดยจะ resolve แพ็กเกจ
`@ruvyxa/adapter-*` จากโปรเจกต์และ override `config.adapter` สำหรับ build ครั้งนั้น หากยังไม่ได้
ติดตั้งแพ็กเกจ build จะจบด้วย `RUV2203` พร้อมคำสั่งติดตั้งที่ถูกต้อง

Adapter จะสร้าง artifact หลัง build แต่ก่อน commit output จึงหาก adapter ล้มเหลว `.ruvyxa/` ชุดเดิม
จะไม่ถูกแทนที่ ผลลัพธ์ deploy อยู่ที่ `.ruvyxa/deploy/<platform>/`

Static output ของ Vercel, Netlify และ Cloudflare ใส่ cache header แบบ immutable
(`Cache-Control: public, max-age=31536000, immutable`) ให้ `/client/*` ที่ hash ชื่อไฟล์แล้ว ผ่าน
`config.json`, `netlify.toml` และไฟล์ `_headers` ตามลำดับ

### Vercel static output

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { vercelAdapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: vercelAdapter(),
})
```

Adapter สร้าง Build Output API layout (`.vercel/output/static` และ `.vercel/output/config.json`)
**ที่ project root** ตอน `ruvyxa build` — บน Vercel เลือก preset แบบ Other และตั้ง build command
เป็น build script ของโปรเจกต์ (เช่น `npm run build`) Vercel จะ detect `.vercel/output/` ให้เอง
ไม่ต้องตั้ง output directory เพิ่ม แนะนำใส่ `.vercel/` ใน `.gitignore` เพราะถูก generate ทุก build

ใส่ `vercelAdapter({ projectOutput: false })` ถ้าต้องการพฤติกรรมเดิม (เขียนเฉพาะใน
`.ruvyxa/deploy/vercel/` แล้ว deploy เอง)

### Netlify zero-config

```ts
import { netlifyAdapter } from '@ruvyxa/adapter-netlify'

export default config({
  adapter: netlifyAdapter(),
})
```

`ruvyxa build` ครั้งแรกจะสร้าง `netlify.toml` ที่ project root พร้อม build command และ publish
directory (`.ruvyxa/deploy/netlify/publish`) ให้เสร็จ — commit ไฟล์นี้แล้วเชื่อม repo กับ Netlify
ได้เลยโดยไม่ต้องตั้งค่าใน dashboard ถ้ามี `netlify.toml` อยู่แล้วจะ**ไม่ถูกเขียนทับ**
(ไฟล์ของผู้ใช้ชนะเสมอ) ใส่ `netlifyAdapter({ projectConfig: false })` ถ้าไม่ต้องการให้ generate

### Cloudflare zero-config

```ts
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default config({
  adapter: cloudflareAdapter(),
})
```

`ruvyxa build` จะสร้าง `wrangler.jsonc` ที่ project root ให้ด้วย โดย `assets.directory` ชี้ไปที่
static assets ที่ generate แล้ว ทำให้ `wrangler deploy` ใช้ได้ทันทีโดยไม่ต้องตั้งค่าใน dashboard
ถ้ามี `wrangler.jsonc` อยู่แล้วจะ**ไม่ถูกเขียนทับ** ใส่
`cloudflareAdapter({ projectConfig: false })` ถ้าไม่ต้องการให้ generate

Adapter ของ Vercel, Netlify และ Cloudflare รองรับ static SSG/CSR เท่านั้นในปัจจุบัน API, SSR, ISR
และ PPR จะทำให้ build จบด้วย `RUV2202` แทนที่จะ deploy output ที่ไม่มี request handler

### Permission Denied Error

```
node_modules/.bin/ruvyxa: Permission denied
```

→ อัปเกรดเป็น Ruvyxa release ที่มี executable launcher

### Node Version

ระบุ Node 22 เพื่อ reproducible CI builds:

```json
{
  "engines": {
    "node": "22.x"
  }
}
```

---

## CI/CD

### Pipeline แนะนำ

```yaml
# .github/workflows/deploy.yml
- run: npm ci
- run: npx ruvyxa analyze
- run: npm run typecheck
- run: npm run check
- run: npm run build
```

### Build Artifacts

หลัง `npm run build` จะได้ output หลักใน `.ruvyxa/` และ adapter อาจสร้างไดเรกทอรี deploy เพิ่ม:

```text
.ruvyxa/
├── server/         # Server-side source
├── client/         # Client bundles + manifest
├── assets/         # Static assets + WebP images
├── prerender/      # Pre-rendered HTML pages
├── manifest.json   # Route manifest
├── build.json      # Build metadata
└── deploy/         # Adapter-specific artifacts, เมื่อตั้งค่าไว้
```

---

## Adapters

### ที่มีให้

| Adapter                      | เป้าหมาย                                       |
| ---------------------------- | ---------------------------------------------- |
| `@ruvyxa/adapter-node`       | Node launcher: `.ruvyxa/deploy/node/start.mjs` |
| `@ruvyxa/adapter-bun`        | Bun launcher: `.ruvyxa/deploy/bun/start.mjs`   |
| `@ruvyxa/adapter-static`     | Static files: `.ruvyxa/static/`                |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Pages: `.ruvyxa/deploy/cloudflare/` |
| `@ruvyxa/adapter-netlify`    | Netlify static: `.ruvyxa/deploy/netlify/`      |
| `@ruvyxa/adapter-vercel`     | Vercel static: `.ruvyxa/deploy/vercel/`        |

### วิธีใช้

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { nodeAdapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: nodeAdapter(),
})
```

### ข้อสำคัญ

- ฟังก์ชัน `build()` ของ adapter ทำงานตอนโหลด configuration และตอนสร้าง artifact หลัง build
- artifact ต้องอยู่ภายใน `.ruvyxa/` และจะถูกบันทึกใน `adapterArtifacts` ของ `build.json`
- static adapter จะปฏิเสธ route ที่ต้องมี dynamic request handler โดยเจตนา

---

## Self-Hosted (Node.js)

```bash
npm run build
npm run start          # serve จาก .ruvyxa/
```

หรือใช้ Node adapter:

```bash
npm install @ruvyxa/adapter-node
```

```ts
import { nodeAdapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: nodeAdapter(),
})
```

## Static Hosting

```bash
npm install @ruvyxa/adapter-static
# ตั้งค่า staticAdapter() แล้วรัน:
npm run build
# deploy .ruvyxa/static/ ไป static host
```

---

## Production Checklist

ก่อน deploy:

- [ ] `npx ruvyxa analyze` — ไม่มี error
- [ ] `npm run typecheck` — type-safe
- [ ] `npm run check` — readiness checks ผ่าน
- [ ] `.env.example` — ระบุชื่อตัวแปรที่จำเป็น โดยไม่มีค่าจริง
- [ ] Security headers — `security.headers: true`
- [ ] CORS origins — explicit ไม่ใช่ wildcard
- [ ] Body limits — `security.apiLimit` และ `security.actionLimit` เหมาะสม
- [ ] Reverse proxy — ส่ง `X-Forwarded-Proto` และเพิ่ม IP จริงของ proxy ที่ไม่ใช่ loopback ใน
      `security.trustedProxyIps` เมื่ออยู่หลัง HTTPS proxy

## เรียนรู้จาก Demo

`examples/demo/` เป็น integration app ที่มี static, dynamic และ catch-all routes; API routes; server
actions; MDX; public environment variables; external CSS; และ SSR, SSG, ISR, CSR, PPR ตัวอย่าง อ่าน
[README](../../examples/demo/README.md), รันคำสั่ง diagnostic และคัดลอกรูปแบบที่
พิสูจน์แล้วก่อนเพิ่ม feature ใหม่ในแอปของคุณ
