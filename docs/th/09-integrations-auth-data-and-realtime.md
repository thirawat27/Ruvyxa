# การเชื่อมต่อ: authentication, data, realtime, adapter และ testing

> **เป้าหมายของ tutorial:** เชื่อมต่อความสามารถของแอปเข้ากับ framework โดยไม่สมมติ infrastructure
> ที่ไม่มี **เริ่มจาก:** นโยบาย route ใน [Plugin และ middleware](08-plugins-middleware.md)
> **Checkpoint:** เลือก integration หนึ่งอย่าง สร้าง flow ที่เล็กที่สุด และทดสอบ failure path ด้วย

## Authentication

`@ruvyxa/auth` export `createAuth`, provider helper `google` และ `github`, memory store, type และ
`AuthError` package export `@ruvyxa/auth/client` และ `@ruvyxa/auth/plugin` แยกกัน provider contract
ที่รองรับมี credentials, OAuth, magic link และ WebAuthn memory store เป็น process-local; เลือก
durable shared store ก่อน scale authentication แบบหลาย instance

```ts
import { createAuth, memoryAuthStore, memoryRateLimitStore } from '@ruvyxa/auth'

const auth = createAuth({
  secret: process.env.RUVYXA_AUTH_SECRET!,
  origin: 'https://example.test',
  store: memoryAuthStore({ development: true }),
  rateLimitStore: memoryRateLimitStore({ development: true }),
  providers: {},
})
```

`AuthOptions` contract ที่แน่นอนถูก export โดย package อย่าใช้ placeholder ในตัวอย่างนี้เป็น secret
จริง register plugin ที่ auth runtime คืนมา แล้วใช้ browser entry point แยกเฉพาะใน client code:

```ts
// ruvyxa.config.ts
export default config({ plugins: [auth.plugin] })

// a client module
import { createAuthClient } from '@ruvyxa/auth/client'
const authClient = createAuthClient()
```

auth path ปริยายคือ `/__ruvyxa/auth` client มี `login`, `logout`, `session` และ `oauth`;
`createAuth` มี `handle`, `login`, `getSession` และ `logout` สำหรับ server-side integration memory
store ต้องการ `{ development: true }` และตั้งใจให้ production build ล้มเหลวด้วย `RUV3105`; ให้ใช้
durable implementation ของ `AuthStore` และ `AuthRateLimitStore` แทน `createAuthPlugin(bridge)`
ใช้ได้เมื่อ ต้องมี custom bridge

## Database

`@ruvyxa/database` เป็น typed normalized-operation layer ไม่ใช่ ORM migration system
`createDatabase<TSchema>(adapter)` สร้าง model delegate สำหรับ `findMany`, `findFirst`,
`findUnique`, `create`, `createMany`, `update`, `updateMany`, `delete`, `deleteMany` และ `count`
มันมี `prismaAdapter`, `dynamoAdapter` และ `defineDatabaseAdapter`; adapter error ใช้
`RUV3001`–`RUV3003`

```ts
import { createDatabase, defineDatabaseAdapter } from '@ruvyxa/database'
const adapter = defineDatabaseAdapter({
  name: 'example',
  execute: async (operation) => {
    throw new Error(`implement ${operation.kind}`)
  },
})
const db = createDatabase<{ todo: { id: string; title: string } }>(adapter)
```

framework ไม่มี database server, migration engine หรือ backup service
ส่วนเหล่านี้เป็นความรับผิดชอบของ application/infrastructure

## Realtime และ adapter

> **ตัดสินใจเรื่อง hosting ก่อนจะสร้างงานบนสิ่งนี้** ปลั๊กอิน realtime
> ทั้งสองตัวต้องการโปรเซสที่อยู่ยาว เพื่อถือ WebSocket ไว้ จึงถูกให้บริการโดย `ruvyxa dev` และ
> `ruvyxa start` เท่านั้น และไม่มี serverless build ตัวไหนให้บริการได้เลย `ruvyxa dev`
> จะพิมพ์บรรทัดระบุ capability กับ path ของมัน, `ruvyxa build` ปฏิเสธ adapter ที่ไม่รองรับด้วย
> `RUV3201` และ `ruvyxa test:parity` รายงานช่องว่างนี้ — แต่การเปลี่ยน transport
> ทีหลังคือการเขียนแอปใหม่ ไม่ใช่การแก้ config

`@ruvyxa/realtime/plugin` export `realtime()` ซึ่ง claim native realtime capability มันปฏิเสธ build
ที่ไม่ใช่ long-lived Node/Bun output และปฏิเสธ adapter deno, aws, cloudflare, firebase, netlify,
static และ vercel ด้วย `RUV3201` `@ruvyxa/realtime/client` export `createRealtimeClient`; จำกัด
active channel ที่ 16 และ reconnect ด้วย bounded exponential backoff

## Real-time collaboration

`@ruvyxa/realtime/plugin` export `collab()` ด้วย ซึ่ง claim native capability `presence@1` และ serve
collaboration room แบบสองทางที่ `/__ruvyxa/collab` มันมีข้อจำกัดการ deploy เหมือน `realtime()` และ
ปฏิเสธ build ชุดเดียวกันด้วย `RUV3201`

```ts
import { config } from 'ruvyxa/config'
import { collab } from '@ruvyxa/realtime'

export default config({ plugins: [collab()] })
```

room หนึ่งมี state สองแบบ ซึ่งตั้งใจให้ทำงานต่างกัน:

| State        | เก็บไว้นานแค่ไหน    | ความหมาย                                         |
| ------------ | ------------------- | ------------------------------------------------ |
| Presence     | เท่าอายุ connection | แทนที่ทั้งก้อน และถูกทิ้งเมื่อ peer ออกจากห้อง   |
| Shared state | เท่าอายุ room       | last-writer-wins ต่อ key โดย server เป็นผู้ลำดับ |

server เป็นผู้ลำดับเพียงผู้เดียว ดังนั้น "last writer" หมายถึง "frame สุดท้ายที่มาถึง process"
ไม่มีนาฬิกาฝั่ง client เข้ามาเกี่ยว และ peer สองตัวที่เขียน key เดียวกันจะได้ผู้ชนะตัวเดียวกัน
**shared state ไม่ใช่ CRDT** การเขียน key เดียวกันพร้อมกันจะไม่ merge; ตัวที่มาทีหลังทับตัวก่อนหน้า
ถ้าต้องการให้การแก้ไขพร้อมกันอยู่รอดทั้งคู่ ให้แตกเอกสารออกเป็นหลาย key

`@ruvyxa/realtime/react` export `CollabProvider`, `usePresence`, `useSharedState`, `useCollabRoom`
และ `useCollabClient` หนึ่ง provider เป็นเจ้าของหนึ่ง socket; hook อ่านผ่าน `useSyncExternalStore`

```tsx
import { CollabProvider, usePresence, useSharedState } from '@ruvyxa/realtime/react'

function Editor() {
  const others = usePresence({ cursor: [x, y], name: 'Ada' })
  const [title, setTitle] = useSharedState('title', 'Untitled')
  return (
    <>
      <input value={title} onChange={(event) => setTitle(event.target.value)} />
      {others.map((peer) => (
        <Cursor key={peer.id} state={peer.state} />
      ))}
    </>
  )
}

export default function Page() {
  return (
    <CollabProvider room="doc:1">
      <Editor />
    </CollabProvider>
  )
}
```

`@ruvyxa/realtime/collab` export `createCollabClient` สำหรับใช้งานโดยไม่มี React

room เป็น process-local และ ephemeral: ไม่มี storage และจะถูกทิ้งเมื่อ peer สุดท้ายออก server
สองตัวหลัง load balancer จะเป็นเจ้าของ room คนละชุดที่ไม่รู้จักกัน ดังนั้น deployment ที่ใช้
collaboration ต้อง pin peer ของ room เดียวกันไว้ที่ process เดียว ข้อมูลที่ต้องอยู่รอดหลัง peer
สุดท้ายออกให้บันทึกผ่าน loader หรือ Server Action

ขีดจำกัดที่ server บังคับ: 64 peer และ 256 shared-state key ต่อ room, 1024 room ต่อ process, 32 key
ต่อการเขียนหนึ่งครั้ง, 32 KiB ต่อ frame และ 120 frame ต่อวินาทีต่อ connection connection ที่เกิน
frame budget จะถูกปิด ส่วน peer ที่ตามหลัง broadcast buffer ของ room จะได้รับ `resync` แล้ว
reconnect เพื่อรับ snapshot ใหม่

มี first-party adapter package สำหรับ Node, Bun, Deno, static, Vercel, Netlify, Cloudflare, Railway,
Render, Firebase และ AWS เลือก build ด้วย `npm run build -- --adapter <name>` หรือ config `adapter`;
ดู [Deploy, run และ operate](15-deploy-run-and-operate.md) `@ruvyxa/testing` export `mockLoader`,
`mockAction` และ `mockCache` สำหรับ unit test

**ก่อนหน้า:** [Plugin และ middleware](08-plugins-middleware.md) · **ถัดไป:**
[CLI reference](10-cli.md)
