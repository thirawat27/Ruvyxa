# Server Actions

## การสร้าง Actions

วาง mutation ในไฟล์ `action.ts` ไว้ข้าง route ที่เป็นเจ้าของ ตรวจสอบและ validate
ค่าที่ไม่น่าไว้ใจก่อนลงมือ mutate:

```ts
// app/todos/action.ts
import { action } from 'ruvyxa/server'

export const createTodo = action
  .input({
    parse(value: unknown) {
      const title =
        typeof value === 'object' && value && 'title' in value ? String(value.title).trim() : ''

      if (!title) throw new Error('Title is required')
      return { title }
    },
  })
  .handler(async ({ input, invalidate }) => {
    const todo = await database.todos.create(input)
    invalidate('todos')
    return todo
  })
```

## HTML Form Integration

Actions รองรับ progressive enhancement — ส่งผ่าน form ธรรมดาได้:

```tsx
<form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
  <label>
    Title
    <input name="title" required />
  </label>
  <button type="submit">Create</button>
</form>
```

รูปแบบ action endpoint: `/__ruvyxa/action?path=<route>&name=<exportName>`

## Input Validation

`action.input({ parse })` ต้อง:

- รับ input เป็น `unknown` (ห้ามเชื่อ type จาก client)
- คืนค่าที่ parse แล้ว (type ตรงกับที่ handler คาดหวัง)
- throw error เมื่อ invalid (ส่งกลับไปที่ client เป็น error response)

```ts
export const updateProfile = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== 'object') throw new Error('Expected object')
      const obj = value as Record<string, unknown>

      const name = String(obj.name ?? '').trim()
      if (!name || name.length > 100) throw new Error('name: 1–100 chars required')

      const email = String(obj.email ?? '').trim()
      if (!email.includes('@')) throw new Error('email: must be valid')

      return { name, email }
    },
  })
  .handler(async ({ input }) => {
    return database.users.update(input)
  })
```

## Supported Content Types

| Content-Type                        | Format           |
| ----------------------------------- | ---------------- |
| `application/json`                  | JSON body        |
| `application/x-www-form-urlencoded` | URL-encoded form |

## Security

Actions มีระบบป้องกันเป็น default:

| Protection           | Default                             | Config Key                 |
| -------------------- | ----------------------------------- | -------------------------- |
| Body size limit      | 1 MiB                               | `security.actionLimit`     |
| Same-origin check    | Enabled                             | `security.sameOrigin`      |
| Fetch Metadata guard | Enabled                             | `security.fetchMeta`       |
| Rate limiting        | 600 requests / client-action / 60 s | `security.actionRateLimit` |

### การตั้งค่า Security

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'

export default config({
  security: {
    actionLimit: 2 * 1024 * 1024,
    actionRateLimit: { max: 300, window: 60 },
    sameOrigin: true,
    fetchMeta: true,
  },
})
```

### Reverse Proxy

Ruvyxa เชื่อถือ proxy ที่เป็น loopback โดยปริยายเท่านั้น หาก proxy อยู่คนละโฮสต์ ต้องระบุ IP
แบบเจาะจงก่อนจึงจะเชื่อถือ `X-Forwarded-For`, `X-Real-IP` และ `X-Forwarded-Proto` ได้:

```ts
export default config({
  security: {
    trustedProxyIps: ['10.0.0.2'],
  },
})
```

Proxy ต้องเขียนทับ forwarded headers ที่ผู้ใช้ส่งมา ไม่ใช่ส่งผ่านต่อโดยตรง

## Response Middleware Limits

Plugin response middleware ต้อง buffer response ทั้งหมด:

| Config        | Default | Max     |
| ------------- | ------- | ------- |
| `pluginLimit` | 32 MiB  | 256 MiB |

```ts
export default config({
  security: {
    pluginLimit: 64 * 1024 * 1024,
  },
})
```

เลือกค่าที่เล็กที่สุดที่ครอบคลุม response type ที่ plugin ต้องจัดการ ใช้ streaming route หรือ ข้าม
response-phase plugin สำหรับ file download และ payload ที่เกิน 256 MiB

## Cache Invalidation จาก Actions

```ts
.handler(async ({ input, invalidate }) => {
  await database.todos.create(input)
  invalidate('todos')        // invalidate เฉพาะ key
  invalidate()               // invalidate ทั้งหมด
  return { ok: true }
})
```

ดูเพิ่มเติม: [Data Loading & Cache](data-loading-and-cache.md) สำหรับรายละเอียด cache
