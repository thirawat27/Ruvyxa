# Deployment

## Vercel

scripts มาตรฐาน:

```json
{
  "scripts": {
    "dev": "ruvyxa dev",
    "build": "ruvyxa build",
    "start": "ruvyxa start"
  }
}
```

- **Build Command**: `npm run build`
- **Output Directory**: `.ruvyxa`

### Adapter

```ts
import { config } from 'ruvyxa/config'
import { adapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: adapter(),
  adapterOptions: { regions: ['iad1'] },
})
```

### Permission Denied

```
node_modules/.bin/ruvyxa: Permission denied
```

→ อัปเกรดเป็น Ruvyxa release ที่มี executable launcher

## Adapters

| Adapter                      | เป้าหมาย           |
| ---------------------------- | ------------------ |
| `@ruvyxa/adapter-node`       | Node.js server     |
| `@ruvyxa/adapter-vercel`     | Vercel serverless  |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers |
| `@ruvyxa/adapter-netlify`    | Netlify Functions  |
| `@ruvyxa/adapter-bun`        | Bun runtime        |
| `@ruvyxa/adapter-static`     | Static hosting     |

## Production Checklist

- [ ] `npx ruvyxa analyze`
- [ ] `npm run typecheck`
- [ ] `npm run check`
- [ ] `.env.example` — มีชื่อตัวแปรที่จำเป็น
- [ ] Security headers เปิดใช้งาน
- [ ] CORS origins — explicit
- [ ] Reverse proxy forward `X-Forwarded-Proto` และระบุ IP ของ proxy ที่ไม่ใช่ loopback ใน
      `security.trustedProxyIps`

ดู [Demo App](../../examples/demo/README.md) สำหรับตัวอย่างเต็มรูปแบบ
