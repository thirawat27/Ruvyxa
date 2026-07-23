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

`--adapter` รับค่าได้ 2 แบบ และ override `config.adapter` เฉพาะ build ครั้งนั้น:

**1. ชื่อ built-in** — `node`, `bun`, `static`, `vercel`, `netlify`, `cloudflare`

Adapter ทางการทั้ง 6 ตัวถูก bundle มาเป็น dependency ของแพ็กเกจ `ruvyxa` อยู่แล้ว ติดตั้ง `ruvyxa`
ตัวเดียวก็ใช้ชื่อเหล่านี้ได้ทันที ไม่ต้อง `pnpm add @ruvyxa/adapter-*` เพิ่ม:

```bash
ruvyxa build --adapter node      # ได้ standalone server ทันที
ruvyxa build --adapter netlify   # ได้ .netlify/v1/ + deploy dir ทันที
```

ติดตั้งแพ็กเกจ `@ruvyxa/adapter-*` แยกเฉพาะเมื่อต้องส่ง option ผ่าน `ruvyxa.config.ts` เช่น
`netlifyAdapter({ projectConfig: true })` — ผ่าน `--adapter` จะได้ค่า default ของ adapter เสมอ

**2. ชื่อแพ็กเกจ adapter ใดก็ได้** — เปิดให้ ecosystem เขียน adapter เองสำหรับ platform ที่ไม่มี
adapter ทางการ (Deno Deploy, Fastly, AWS Lambda ฯลฯ):

```bash
ruvyxa build --adapter @acme/ruvyxa-adapter-deno   # ชื่อแบบมี scope ใช้ตรง ๆ
ruvyxa build --adapter fastly                       # ชื่อสั้นจะลองตาม convention
```

ลำดับการ resolve:

1. ชื่อที่มี scope (`@scope/name`) หรือมี `/` — resolve เป็นชื่อแพ็กเกจนั้นตรง ๆ
2. ชื่อสั้น — ลองตามลำดับ `@ruvyxa/adapter-<name>`, `ruvyxa-adapter-<name>` แล้วจึง `<name>` เปล่า ๆ
3. แต่ละชื่อ resolve จาก `node_modules` ของโปรเจกต์ก่อน แล้วจึง fallback เป็นชุดที่ bundle มากับ
   `ruvyxa` — **เวอร์ชันที่โปรเจกต์ติดตั้งเองชนะเสมอ** จึง pin เวอร์ชัน adapter เฉพาะโปรเจกต์ได้

หากไม่พบทุกชื่อ build จะจบด้วย `RUV2203` พร้อมรายชื่อแพ็กเกจทั้งหมดที่ลอง resolve
เพื่อให้รู้ทันทีว่า ต้องติดตั้งแพ็กเกจชื่ออะไร

แพ็กเกจ adapter ที่ใช้กับระบบนี้มีเงื่อนไขเดียว: ต้อง export ฟังก์ชัน factory เป็น default export
ที่คืน object ตาม interface `Adapter` ของ `@ruvyxa/core` (`name`, `target`, `supports?`,
`build(ctx)`) — เหมือนที่ adapter ทางการทุกตัวทำ:

```ts
// ruvyxa-adapter-fastly/src/index.ts
import type { Adapter, BuildContext } from '@ruvyxa/core'

export default function fastlyAdapter(): Adapter {
  return {
    name: 'fastly',
    target: 'edge',
    supports: ['ssr', 'ssg', 'csr', 'api'],
    build(ctx: BuildContext) {
      return {
        name: 'fastly',
        target: 'edge',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        artifacts: [/* ... */],
      }
    },
  }
}
```

### Zero-config platform detection

เมื่อไม่ได้ตั้ง `config.adapter` และไม่ได้ส่ง `--adapter` ระบบจะ detect hosting platform จาก build
environment แล้วเลือก adapter ให้อัตโนมัติ:

| Environment variable | Adapter      |
| -------------------- | ------------ |
| `VERCEL`             | `vercel`     |
| `NETLIFY`            | `netlify`    |
| `CF_PAGES`           | `cloudflare` |

ตั้ง `RUVYXA_ADAPTER=<name>` เพื่อ override การ detect หรือใช้เลือก adapter บน CI อื่น ๆ — adapter
ที่ตั้งใน config หรือ flag ชนะการ detect เสมอ

Adapter จะสร้าง artifact หลัง build แต่ก่อน commit output จึงหาก adapter ล้มเหลว `.ruvyxa/` ชุดเดิม
จะไม่ถูกแทนที่ ผลลัพธ์ deploy อยู่ที่ `.ruvyxa/deploy/<platform>/`

Static output ของ Vercel, Netlify และ Cloudflare ใส่ cache header แบบ immutable
(`Cache-Control: public, max-age=31536000, immutable`) ให้ `/__ruvyxa/client/*` ที่ hash
ชื่อไฟล์แล้ว ผ่าน `config.json`, `.netlify/v1/config.json` และไฟล์ `_headers` ตามลำดับ

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

### Netlify

```ts
import { netlifyAdapter } from '@ruvyxa/adapter-netlify'

export default config({
  adapter: netlifyAdapter(),
})
```

ไม่มีไฟล์ถูกเขียนที่ project root — `ruvyxa build` emit directory ตาม Netlify Frameworks API
(`.netlify/v1/` เป็น build artifact ที่ gitignore) ประกอบด้วย SSR/API function และ immutable cache
header ซึ่ง Netlify หยิบไปใช้เองอัตโนมัติตอน deploy การตั้งค่าครั้งเดียวใน Netlify dashboard:
**Build command** = `npm run build` และ **Publish directory** = `.ruvyxa/deploy/netlify/publish`

ถ้าต้องการไฟล์ config แบบ commit แทน ใส่ `netlifyAdapter({ projectConfig: true })` เพื่อ generate
`netlify.toml` ที่ project root (path เป็นแบบ relative กับโปรเจกต์) — ถ้ามี `netlify.toml`
อยู่แล้วจะ**ไม่ถูกเขียนทับ** ใส่ `frameworksApi: false` ถ้าไม่ต้องการ output `.netlify/v1/`

### Cloudflare

```ts
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default config({
  adapter: cloudflareAdapter(),
})
```

ไม่มีไฟล์ถูกเขียนที่ project root — deploy directory มี config ครบในตัว deploy ได้ตรง ๆ:

```bash
npx wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc
```

ถ้าต้องการ config ที่ root แบบ commit ใส่ `cloudflareAdapter({ projectConfig: true })` เพื่อ
generate `wrangler.jsonc` (path เป็นแบบ relative กับโปรเจกต์) — ถ้ามี `wrangler.jsonc`
อยู่แล้วจะ**ไม่ถูกเขียนทับ**

Adapter ของ Vercel, Netlify และ Cloudflare รองรับ **server rendering เต็มรูปแบบ** แล้ว:

| Strategy | Vercel | Netlify | Cloudflare |
| -------- | ------ | ------- | ---------- |
| SSG      | ✓      | ✓       | ✓          |
| CSR      | ✓      | ✓       | ✓          |
| SSR      | ✓      | ✓       | ✓          |
| API      | ✓      | ✓       | ✓          |
| ISR      | ✓      | ✓       | ✗*         |
| PPR      | ✓      | ✓       | ✗*         |

\* Cloudflare Workers ไม่มี persistent storage สำหรับ ISR cache — route ที่ใช้ ISR/PPR จะถูก reject
ด้วย `RUV2210` บน Cloudflare ใช้ KV หรือ Durable Objects binding เองถ้าต้องการ

Deploy แบบ static-only (SSG/CSR ล้วนไม่มี API/SSR) ยังทำงานเหมือนเดิมทุกประการ Adapter จะ emit ทั้ง
static assets และ serverless function; platform จะเสิร์ฟ static file ตรงๆ แล้ว forward request
ที่ไม่ match ไปยัง function handler

ภายใน function output จะเป็น static route registry bundle แบบ `.mjs` ที่ compile แล้ว ไม่ใช่ไฟล์
TypeScript/TSX ดิบ จึงรัน artifact ได้โดยตรงและ Wrangler มองเห็น edge module ครบ สำหรับ
Vercel/Netlify ระบบจะเทียบอายุ ISR cache กับค่า `revalidate` และ regenerate เฉพาะรายการที่ stale
พร้อมรวม request ซ้ำของ path เดียวกันภายใน function instance ที่ยัง warm ให้เหลืองานเดียว

### Permission Denied Error

```
node_modules/.bin/ruvyxa: Permission denied
```

→ อัปเกรดเป็น Ruvyxa release ที่มี executable launcher

### GLIBC Version Error

```
ruvyxa: /lib64/libc.so.6: version `GLIBC_2.39' not found
```

Release ก่อน 1.0.19 ship Linux binary แบบ dynamic link ซึ่งผูกกับ glibc ของเครื่อง build ทำให้พังบน
host ที่ glibc เก่ากว่า (เช่น build image ของ Vercel ที่เป็น Amazon Linux) ตั้งแต่ 1.0.19 binary
Linux เป็น static musl ทั้งหมด รันได้บน Linux ทุกแบบ — อัปเกรดแพ็กเกจ `ruvyxa` เพื่อแก้ error นี้

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

| Adapter                      | เป้าหมาย                                                  |
| ---------------------------- | --------------------------------------------------------- |
| `@ruvyxa/adapter-node`       | Standalone server: `.ruvyxa/deploy/node/server/index.mjs` |
| `@ruvyxa/adapter-bun`        | Bun launcher: `.ruvyxa/deploy/bun/start.mjs`              |
| `@ruvyxa/adapter-static`     | Static files: `.ruvyxa/static/`                           |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers: `.ruvyxa/deploy/cloudflare/`          |
| `@ruvyxa/adapter-netlify`    | Netlify functions + static: `.netlify/v1/` + deploy dir   |
| `@ruvyxa/adapter-vercel`     | Vercel Build Output API: `.vercel/output/`                |

Adapter ทางการทั้งหมด bundle มากับแพ็กเกจ `ruvyxa` — `--adapter <name>` และ platform auto-detection
ใช้ได้โดยไม่ต้องติดตั้งอะไรเพิ่ม ติดตั้งแพ็กเกจ `@ruvyxa/adapter-*` แยกเฉพาะเมื่อ ต้องส่ง option ใน
`ruvyxa.config.ts` แพ็กเกจ adapter จากภายนอก (แพ็กเกจใดก็ตามที่ export adapter factory เป็น default
export) ใช้ได้แบบเดียวกันผ่าน `--adapter <package-name>` หรือ `config.adapter`

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

หรือ build standalone server ที่รันได้โดยไม่ต้องมี ruvyxa CLI ตอน runtime:

```bash
ruvyxa build --adapter node
node .ruvyxa/deploy/node/server/index.mjs
```

directory `deploy/node/` ครบจบในตัว (server + assets ใน `public/`) — copy ไปใส่ Docker image, VPS,
PM2, systemd หรือ PaaS ใดก็ได้ (Render, Railway, Fly.io, Heroku) แล้วรันคำสั่งเดิม ไม่ต้องมี
`node_modules` และไม่ต้องมี native binary ตอน runtime รองรับ `PORT` (default 3000), `HOST` (default
0.0.0.0) และ SSR, API, ISR, PPR, SSG, CSR ครบ

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
