# Deployment

> 🟢 **Quick Deploy เหมาะกับมือใหม่** · ⏱️ อ่าน ~8 นาที (Quick Deploy อย่างเดียว 2 นาที)
>
> **จะได้เรียนรู้:** เอาแอปขึ้นเว็บใน 2-3 ขั้นทุก platform, adapter ทำอะไร และ (ถ้าอยากรู้) ระบบ
> adapter ขั้นสูงท้ายบท

เพิ่งเคย deploy ครั้งแรก? เริ่มที่ **Quick Deploy** ด้านล่าง — แอปส่วนใหญ่ขึ้นเว็บได้ใน 2-3 ขั้นตอน
ส่วนถัดไปอธิบายว่าระบบทำงานยังไง และหัวข้อ **Advanced** ท้ายสุดสำหรับ power user และคนที่อยากเขียน
adapter เอง

## Quick Deploy

เลือก platform ของคุณ (ทุกแบบใช้ script มาตรฐาน `"build": "ruvyxa build"` ซึ่ง starter template
ตั้งให้อยู่แล้ว)

| Platform                       | ขั้นตอน                                                                                                 |
| ------------------------------ | ------------------------------------------------------------------------------------------------------- |
| **Vercel**                     | push repo → import บน Vercel → จบ (Ruvyxa detect Vercel เองแล้ว emit output ให้ถูกแบบ)                  |
| **Netlify**                    | push repo → import บน Netlify → กรอก **Publish directory** = `.ruvyxa/deploy/netlify/publish` → จบ      |
| **Cloudflare**                 | `ruvyxa build --adapter cloudflare` → `npx wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc` |
| **Server ตัวเอง / Docker**     | `ruvyxa build --adapter node` → `node .ruvyxa/deploy/node/server/index.mjs`                             |
| **Static host (GitHub Pages)** | `ruvyxa build --adapter static` → อัปโหลด `.ruvyxa/static/`                                             |

โปรเจกต์ส่วนใหญ่จบแค่นี้ — ไม่มีไฟล์ config ถูกเขียนที่ project root และบน Vercel/Netlify
ไม่ต้องเลือก adapter เองด้วยซ้ำ ระบบ detect platform ให้อัตโนมัติ

## ระบบทำงานยังไง (อ่าน 1 นาที)

`ruvyxa build` compile แอปทั้งหมดลง `.ruvyxa/` จากนั้น **adapter** จะแปลง output ให้อยู่ในรูปที่
hosting แต่ละเจ้าต้องการ — serverless function สำหรับ Netlify, Build Output directory สำหรับ Vercel,
standalone server สำหรับ VPS การเลือก adapter มี 3 ทาง:

1. **อัตโนมัติ** — build บน CI ของ Vercel/Netlify/Cloudflare Pages ระบบเลือก adapter ที่ตรงจาก
   environment ของ platform เอง ไม่ต้องตั้งอะไรเลย
2. **Command line** — `ruvyxa build --adapter node` (ไม่ต้องแก้ config ได้ค่า default ของ adapter)
3. **Config** — ตั้ง `adapter` ใน `ruvyxa.config.ts` เมื่อต้องส่ง option ให้ adapter

Adapter ทางการทั้ง 6 ตัว (`node`, `bun`, `static`, `vercel`, `netlify`, `cloudflare`) มาพร้อมกับ
แพ็กเกจ `ruvyxa` แล้ว — ไม่ต้องติดตั้งอะไรเพิ่ม

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

adapter ทุกตัวใส่ cache policy เหมือนกัน 2 แบบ ผ่าน config ที่ host แต่ละเจ้าอ่าน (`config.json`
routes ของ Vercel, `.netlify/v1/config.json` + `netlify.toml` ของ Netlify, ไฟล์ `_headers` ของ
Cloudflare และ static adapter, response header ใน standalone Node server):

- bundle ที่ hash ชื่อไฟล์แล้วใต้ `/__ruvyxa/client/*` — `public, max-age=31536000, immutable`
- ไฟล์อื่นจาก `public/` — `public, max-age=3600, must-revalidate` ซึ่งเป็น header เดียวกับที่
  `ruvyxa dev` และ `ruvyxa start` ส่ง ถ้าไม่ตั้ง Vercel, Netlify และ Cloudflare จะ default เป็น
  `max-age=0, must-revalidate` ทำให้โหลดรูปและฟอนต์ใหม่ทุกครั้งที่เปลี่ยนหน้า

## คู่มือรายแพลตฟอร์ม

### Vercel

เชื่อม repository แล้ว deploy — ไม่ต้องตั้งอะไรเพิ่ม ระหว่าง build adapter จะ emit layout ตาม Build
Output API ของ Vercel (`.vercel/output/static` และ `.vercel/output/config.json`) ที่ project root
ซึ่ง Vercel หยิบไปใช้เองอัตโนมัติ — `.vercel/` เป็น build artifact ที่ starter template gitignore
ให้แล้ว

ถ้าต้องการระบุ adapter ใน config เอง:

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { vercelAdapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: vercelAdapter(),
})
```

ใส่ `vercelAdapter({ projectOutput: false })` ถ้าต้องการเขียนเฉพาะใน `.ruvyxa/deploy/vercel/` แล้ว
deploy เองด้วย preset แบบ Other

หน้า static ถูกเสิร์ฟจาก edge ทั่วโลกอยู่แล้ว แต่หน้า SSR, API route และการ revalidate ของ ISR
จะรันใน **function region** ซึ่งคือ `iad1` (US East) ถ้าไม่ได้ตั้งค่าไว้เป็นอย่างอื่น
ถ้าผู้ใช้อยู่ไกลจากภูมิภาคนั้น ให้ย้าย function เข้ามาใกล้:

```ts
export default config({
  adapter: vercelAdapter({ regions: ['sin1'] }), // สิงคโปร์
})
```

### Netlify

เชื่อม repository แล้วกรอก 2 ช่องใน Netlify dashboard ครั้งเดียว:

- **Build command**: `npm run build`
- **Publish directory**: `.ruvyxa/deploy/netlify/publish`

ไม่มีไฟล์ถูกเขียนที่ project root — build จะ emit directory ตาม Netlify Frameworks API
(`.netlify/v1/` เป็น build artifact ที่ gitignore) ประกอบด้วย SSR/API function และ immutable cache
header ซึ่ง Netlify หยิบไปใช้เองอัตโนมัติตอน deploy

ถ้าต้องการระบุ adapter ใน config เอง:

```ts
import { netlifyAdapter } from '@ruvyxa/adapter-netlify'

export default config({
  adapter: netlifyAdapter(),
})
```

ถ้าอยากได้ไฟล์ config แบบ commit แทนการกรอก dashboard ใส่ `netlifyAdapter({ projectConfig: true })`
เพื่อ generate `netlify.toml` ที่ project root (path เป็นแบบ relative กับโปรเจกต์) — ถ้ามี
`netlify.toml` อยู่แล้วจะ**ไม่ถูกเขียนทับ** ใส่ `frameworksApi: false` ถ้าไม่ต้องการ output
`.netlify/v1/`

### Cloudflare

ไม่มีไฟล์ถูกเขียนที่ project root — deploy directory มี config ครบในตัว deploy ได้ตรง ๆ:

```bash
ruvyxa build --adapter cloudflare
npx wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc
```

ถ้าต้องการระบุ adapter ใน config เอง:

```ts
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default config({
  adapter: cloudflareAdapter(),
})
```

ถ้าต้องการ config ที่ root แบบ commit ใส่ `cloudflareAdapter({ projectConfig: true })` เพื่อ
generate `wrangler.jsonc` (path เป็นแบบ relative กับโปรเจกต์) — ถ้ามี `wrangler.jsonc`
อยู่แล้วจะ**ไม่ถูกเขียนทับ**

### Self-Hosted (Node.js, Docker, VPS, PaaS)

```bash
npm run build
npm run start          # serve จาก .ruvyxa/ ด้วย ruvyxa CLI
```

หรือ build standalone server ที่รันได้โดยไม่ต้องมี ruvyxa CLI ตอน runtime:

```bash
ruvyxa build --adapter node
node .ruvyxa/deploy/node/server/index.mjs
```

บน Bun ให้ build ด้วย `--adapter bun` แล้วรัน `bun .ruvyxa/deploy/bun/server/index.mjs` — adapter
ทั้งสอง ตัว emit server ตัวเดียวกัน ลำดับการ route, static fallback และ cache header
จึงเหมือนกันทั้งสอง runtime

directory `deploy/node/` ครบจบในตัว (server + assets ใน `public/`) — copy ไปใส่ Docker image, VPS,
PM2, systemd หรือ PaaS ใดก็ได้ (Render, Railway, Fly.io, Heroku) แล้วรันคำสั่งเดิม ไม่ต้องมี
`node_modules` และไม่ต้องมี native binary ตอน runtime รองรับ `PORT` (default 3000), `HOST` (default
0.0.0.0) และ SSR, API, ISR, PPR, SSG, CSR ครบ

### Static Hosting

```bash
ruvyxa build --adapter static
# อัปโหลด .ruvyxa/static/ ไป static host
```

Static hosting ใช้ได้กับแอปที่ทุกหน้าเป็น SSG/CSR — หน้าที่ต้องมี server (SSR, ISR, PPR, API routes)
จะถูกปฏิเสธตอน build พร้อม error ชี้ route ชัด ๆ ให้เปลี่ยนไปใช้ target แบบ serverless หรือ Node แทน

### แต่ละ platform รองรับอะไรบ้าง

| Strategy | Vercel | Netlify | Cloudflare | Node (standalone) | Static |
| -------- | ------ | ------- | ---------- | ----------------- | ------ |
| SSG      | ✓      | ✓       | ✓          | ✓                 | ✓      |
| CSR      | ✓      | ✓       | ✓          | ✓                 | ✓      |
| SSR      | ✓      | ✓       | ✓          | ✓                 | ✗      |
| API      | ✓      | ✓       | ✓          | ✓                 | ✗      |
| ISR      | ✓      | ✓       | ✗*         | ✓                 | ✗      |
| PPR      | ✓      | ✓       | ✗*         | ✓                 | ✗      |

\* Cloudflare Workers ไม่มี persistent storage สำหรับ ISR cache — route ที่ใช้ ISR/PPR จะถูก reject
ด้วย `RUV2210` บน Cloudflare ใช้ KV หรือ Durable Objects binding เองถ้าต้องการ

Deploy แบบ static-only (SSG/CSR ล้วนไม่มี API/SSR) ทำงานได้ทุก platform — adapter แบบ serverless จะ
emit ทั้ง static assets และ serverless function; platform เสิร์ฟ static file ตรง ๆ แล้ว forward
request ที่ไม่ match ไปยัง function handler

### ลำดับการ route บน host

adapter แบบ serverless ทุกตัวใช้ลำดับเดียวกัน ซึ่งตรงกับที่ `ruvyxa dev` และ `ruvyxa start`
ทำในเครื่อง:

1. **client bundle ที่ hash แล้ว** ใต้ `/__ruvyxa/client/` — เสิร์ฟจาก CDN, cache แบบ immutable
2. **public asset** (ทุกไฟล์จาก `public/`) — เสิร์ฟจาก CDN ด้วย
   `public, max-age=3600, must-revalidate`
3. **หน้า SSG/CSR ที่ prerender แล้ว** — เสิร์ฟจาก CDN
4. **ที่เหลือทั้งหมด** — เข้า function handler

สองข้อที่ควรรู้:

- request ที่หา asset ไม่เจอ (`/logo.png`, `/favicon.ico`) จะได้ **404** ไม่ใช่หน้าเว็บที่ render
  ออกมา ถ้าไม่มีกฎนี้ dynamic route อย่าง `/[lang]` จะจับชื่อไฟล์นั้นไว้แล้วตอบ `200` พร้อม body
  เป็น HTML ซึ่ง browser แสดงเป็นรูปเสีย และเสีย function invocation ทุก request ส่วน route
  ที่ประกาศนามสกุลเอง (`/sitemap.xml`) ยังคง match ตามปกติ
- **หน้า ISR และ PPR จะไม่ถูก publish เป็นไฟล์ static โดยตั้งใจ** เพราะ host จะเสิร์ฟ snapshot ตอน
  build ก่อนถึง function ทำให้หน้าไม่มีวัน revalidate ตัว HTML จากตอน build ยังอยู่ใน function
  bundle และถูกใช้ เป็น cache entry แรก

## Troubleshooting

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
[README](../../../examples/demo/README.md), รันคำสั่ง diagnostic และคัดลอกรูปแบบที่
พิสูจน์แล้วก่อนเพิ่ม feature ใหม่ในแอปของคุณ

---

## Advanced: ระบบ Adapter

เนื้อหาต่อจากนี้สำหรับ power user และคนเขียน adapter — การ deploy แอปปกติไม่จำเป็นต้องอ่าน

<details>
<summary><strong>กดเปิดอ่านระบบ adapter ฉบับเต็ม</strong> (กติกา resolve, เขียน adapter เอง, lifecycle)</summary>

### Adapter ที่มีให้

| Adapter                      | เป้าหมาย                                                  |
| ---------------------------- | --------------------------------------------------------- |
| `@ruvyxa/adapter-node`       | Standalone server: `.ruvyxa/deploy/node/server/index.mjs` |
| `@ruvyxa/adapter-bun`        | Standalone server: `.ruvyxa/deploy/bun/server/index.mjs`  |
| `@ruvyxa/adapter-static`     | Static files: `.ruvyxa/static/`                           |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers: `.ruvyxa/deploy/cloudflare/`          |
| `@ruvyxa/adapter-netlify`    | Netlify functions + static: `.netlify/v1/` + deploy dir   |
| `@ruvyxa/adapter-vercel`     | Vercel Build Output API: `.vercel/output/`                |

Adapter ทางการทั้งหมด bundle มากับแพ็กเกจ `ruvyxa` — `--adapter <name>` และ platform auto-detection
ใช้ได้โดยไม่ต้องติดตั้งอะไรเพิ่ม ติดตั้งแพ็กเกจ `@ruvyxa/adapter-*` แยกเฉพาะเมื่อต้องส่ง option ใน
`ruvyxa.config.ts`

### การ resolve ของ `--adapter`

`--adapter` รับค่าได้ 2 แบบ และ override `config.adapter` เฉพาะ build ครั้งนั้น:

**1. ชื่อ built-in** — `node`, `bun`, `static`, `vercel`, `netlify`, `cloudflare` ใช้ได้ทันที
ด้วยแพ็กเกจ `ruvyxa` ตัวเดียว และได้ค่า default ของ adapter เสมอ

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
เพื่อให้รู้ทันทีว่าต้องติดตั้งแพ็กเกจชื่ออะไร

### เขียน Adapter เอง

แพ็กเกจ adapter มีเงื่อนไขเดียว: ต้อง export ฟังก์ชัน factory เป็น default export ที่คืน object ตาม
interface `Adapter` ของ `@ruvyxa/core` (`name`, `target`, `supports?`, `build(ctx)`) — เหมือนที่
adapter ทางการทุกตัวทำ:

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

Framework ทำงานหนักให้ทั้งหมด: compile ทุก route เป็น `.mjs` registry ที่รันได้จริง, copy serverless
handler runtime กลาง (`serverless-handler.mjs` — SSR, API dispatch, ISR revalidation, PPR) และ
materialize artifact ที่ adapter ประกาศ (`file`, `static-site`, `function`) — adapter
มีหน้าที่แค่บรรยาย layout ที่ platform ต้องการ และห่อ handler ด้วย function signature ของ platform
นั้น

### หมายเหตุ lifecycle ของ Adapter

- ฟังก์ชัน `build()` ของ adapter ทำงานตอนโหลด configuration และตอนสร้าง artifact หลัง build
- ขั้น post-build เขียนไฟล์ได้เฉพาะใน `.ruvyxa/` (บวก allowlist ของ path discovery ที่ project root
  เช่น `.vercel/output`, `.netlify/v1`) และผลถูกบันทึกใน `adapterArtifacts` ของ `build.json`
- static adapter ปฏิเสธ route ที่ต้องมี dynamic request handler โดยเจตนา — เป็น safety boundary
- function output เป็น static route registry bundle แบบ `.mjs` ที่ compile แล้ว ไม่ใช่ TypeScript
  ดิบ จึงรันได้ตรง ๆ และ Wrangler มองเห็น edge module ครบ; บน Vercel/Netlify ระบบเทียบอายุ ISR cache
  กับ `revalidate` แล้ว regenerate เฉพาะรายการ stale พร้อมรวม request ซ้ำภายใน function instance
  ที่ยัง warm

</details>
