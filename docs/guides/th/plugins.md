# Plugins

ระบบ plugin ของ Ruvyxa เป็นโมดูลแอปพลิเคชันที่เขียนด้วย TypeScript

สร้าง starter:

```bash
npx ruvyxa plugin new auth
```

ตัวอย่าง `plugins/auth.ts`:

```ts
import { definePlugin } from 'ruvyxa/config'

export default definePlugin({
  name: 'auth',
  setup({ addMiddleware }) {
    addMiddleware({
      routes: ['/api/*'],
      onRequest(request) {
        return request.headers.has('authorization')
          ? undefined
          : new Response('Unauthorized', { status: 401 })
      },
    })
  },
})
```

นำเข้าใน `ruvyxa.config.ts` แล้วใช้ `resolveId`, `transform` หรือ `onBuildComplete` ใน `setup`
เดียวกันได้ Middleware ใช้ Fetch `Request` และ `Response` มาตรฐาน และทำงานใน Node/Bun runtime แบบ
persistent ไม่มี ABI แยกหรือคำสั่ง debug แบบเดิม
