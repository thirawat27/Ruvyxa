# Plugins

ระบบ plugin ของ Ruvyxa เป็นโมดูลแอปพลิเคชันที่เขียนด้วย TypeScript

สร้าง starter:

```bash
npx ruvyxa plugin new auth
```

คำสั่งจะสร้างแพ็กเกจ `auth/` ตรงๆ (ชื่อโฟลเดอร์ = ชื่อ plugin ไม่ต้องใช้ `--dir`) พร้อม
`package.json`, `tsconfig.json`, `README.md` และ `src/index.ts` ใส่ `--dir <path>` เฉพาะถ้าต้องการ
ตำแหน่งอื่น plugin รันได้ทั้ง Node.js และ Bun (`--runtime bun` หรือ `RUVYXA_RUNTIME=bun`):

```ts
import { plugin } from 'ruvyxa/config'

export default plugin('auth', {
  routes: ['/*'],
  onRequest(request) {
    return request.headers.has('authorization')
      ? undefined
      : new Response('Unauthorized', { status: 401 })
  },
})
```

นำเข้า package ใน `ruvyxa.config.ts`:

```ts
import auth from './plugins/auth'
import { config } from 'ruvyxa/config'

export default config({ plugins: [auth] })
```

รัน `pnpm install` และ `pnpm build` ภายในโฟลเดอร์ plugin เพื่อสร้าง `dist/` แล้วใช้ `pnpm publish`
เพื่อเผยแพร่เป็น npm library ได้

ใช้ `plugin(name, middleware)` สำหรับ request/response middleware ซึ่งรับได้ทั้ง middleware object
หรือ request handler function โดย Middleware ใช้ Fetch `Request` และ `Response` มาตรฐาน

หากต้องใช้ `resolveId`, `transform` หรือ `onBuildComplete` ให้ใช้รูปแบบขั้นสูง
`definePlugin({ name, setup })` ทุก hook ทำงานใน Node/Bun runtime แบบ persistent ไม่มี ABI แยกหรือ
คำสั่ง debug แบบเดิม
