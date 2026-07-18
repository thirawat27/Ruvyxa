# Deployment

## Vercel

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

ตั้งค่า Vercel:

- **Build Command**: `npm run build`
- **Output Directory**: `.ruvyxa`
- **Framework Preset**: _None_ — Ruvyxa จัดการทุกอย่างผ่าน `npm run build`

### Adapter

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { adapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: adapter(),
  adapterOptions: {
    regions: ['iad1'],
  },
})
```

Adapters เขียน metadata ลง `.ruvyxa/build.json` สำหรับ deployment tooling

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

หลังจาก `npm run build` ให้ deploy ทั้งไดเรกทอรี `.ruvyxa/`:

```text
.ruvyxa/
├── server/         # Server-side source
├── client/         # Client bundles + manifest
├── assets/         # Static assets + WebP images
├── prerender/      # Pre-rendered HTML pages
├── manifest.json   # Route manifest
└── build.json      # Build metadata
```

---

## Adapters

### ที่มีให้

| Adapter                      | เป้าหมาย           |
| ---------------------------- | ------------------ |
| `@ruvyxa/adapter-node`       | Node.js server     |
| `@ruvyxa/adapter-vercel`     | Vercel serverless  |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers |
| `@ruvyxa/adapter-netlify`    | Netlify Functions  |
| `@ruvyxa/adapter-bun`        | Bun runtime        |
| `@ruvyxa/adapter-static`     | Static hosting     |

### วิธีใช้

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { adapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: adapter(),
})
```

### ข้อสำคัญ

- ฟังก์ชัน `build()` ของ adapter ทำงานขณะ Ruvyxa โหลด configuration
- `AdapterOutput` และ `adapterOptions` ที่ serialize ได้จะถูกเขียนลง `.ruvyxa/build.json`
- การประกาศ adapter เพียงอย่างเดียว**ไม่ได้**สร้างหรือ publish platform functions
- ตรวจสอบ platform output, routing และ serving model ของ deployment เสมอ

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
import { adapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: adapter(),
})
```

## Static Hosting

```bash
npm run build -- --target static
# หรือตั้งค่า runtime: 'static' ใน config
# deploy .ruvyxa/ ไปยัง static host (S3, Cloudflare Pages, Netlify, ฯลฯ)
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
