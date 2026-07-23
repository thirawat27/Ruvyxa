# API Routes

> 🟡 **ระดับกลาง** · ⏱️ อ่าน ~6 นาที
>
> **จะได้เรียนรู้:** สร้าง JSON endpoint ด้วย `route.ts`, จัดการแต่ละ HTTP method และ validate
> request body อย่างปลอดภัย

API route คือ endpoint หลังบ้านที่ไม่มีหน้าเว็บ — URL ที่ตอบ JSON (หรืออะไรก็ได้) แทน HTML ใช้เมื่อ
browser, mobile app หรือ service อื่นต้องเรียก server ของคุณ: webhook, health check, public API ฯลฯ
แต่ถ้าแค่ต้องการแก้ข้อมูลจากหน้าเว็บของตัวเอง [Server Actions](server-actions.md) มักง่ายกว่า

## การสร้าง API Routes

สร้าง `route.ts` และ export named HTTP method handlers แต่ละ handler รับ `Request` มาตรฐาน และคืน
`Response` มาตรฐาน:

```ts
// app/api/health/route.ts
export function GET() {
  return Response.json({ ok: true })
}

export async function POST({ request }: { request: Request }) {
  const body = await request.json()
  return Response.json({ received: body }, { status: 201 })
}

export function PUT() {
  return new Response('Method Not Allowed', { status: 405 })
}
```

Methods ที่รองรับ: `GET`, `POST`, `PUT`, `DELETE`, `PATCH`, `HEAD`, `OPTIONS`

แต่ละ handler ได้รับ `{ request, params }`:

- `request` — Web API `Request` object มาตรฐาน
- `params` — dynamic route parameters: `[id]` เป็น `string`, `[...slug]` เป็น `string[]`, และ
  `[[...slug]]` ที่ไม่มี segment เป็น `undefined`

## Response Types

Handler ต้องคืน `Response` object (หรือ Promise ที่ resolve เป็น Response):

```ts
// Plain text
export function GET() {
  return new Response('Hello', { headers: { 'Content-Type': 'text/plain' } })
}

// JSON
export function GET() {
  return Response.json({ data: [1, 2, 3] })
}

// Redirect
export function GET() {
  return Response.redirect('/dashboard', 302)
}

// Error
export function GET() {
  return new Response('Not Found', { status: 404 })
}
```

## Input Validation

ตรวจสอบ input ทั้งหมดไว้ใกล้กับ handler:

```ts
export async function POST({ request }: { request: Request }) {
  const body = await request.json()

  if (!body.name || typeof body.name !== 'string') {
    return Response.json({ error: 'name is required' }, { status: 400 })
  }

  return Response.json({ created: body.name }, { status: 201 })
}
```

## Body Size Limits

| Limit       | Default | Config Key             |
| ----------- | ------- | ---------------------- |
| API body    | 10 MiB  | `security.apiLimit`    |
| Action body | 1 MiB   | `security.actionLimit` |

เปลี่ยนค่าเฉพาะเมื่อ endpoint ต้องการ และกำหนด upper bound ที่สมเหตุสมผล:

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'

export default config({
  security: {
    apiLimit: 20 * 1024 * 1024,
  },
})
```

## Streaming Responses

API handler สามารถคืน `Response` ที่มี body เป็น `ReadableStream` ได้ Ruvyxa จะส่ง status และ
headers ก่อน แล้วทยอยส่ง chunk ของ body แบบ binary-safe ผ่าน persistent worker ไปยัง HTTP response
โดยไม่รวม response ทั้งหมดเป็นข้อความก้อนเดียว

```ts
export function GET() {
  const encoder = new TextEncoder()
  const body = new ReadableStream({
    start(controller) {
      controller.enqueue(encoder.encode('first\n'))
      controller.enqueue(encoder.encode('second\n'))
      controller.close()
    },
  })

  return new Response(body, {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  })
}
```

Worker IPC ใช้ frame ขนาดไม่เกิน 64 KiB และจำกัด queue แยกต่อ response หาก consumer ช้า ระบบจะใช้
backpressure หยุด producer ชั่วคราวแทนการตัด HTTP response ที่เริ่มส่งไปแล้ว ส่วน producer
ที่ค้างเกิน idle timeout จะยุติเฉพาะ stream นั้นแทนการปล่อยให้หน่วยความจำที่รอส่งโตไม่จำกัด
ระบบจะข้าม gzip และ Brotli อัตโนมัติสำหรับ live stream ที่ยังไม่ทราบขนาดสุดท้าย ส่วน response ที่
buffer ครบและทราบขนาดแล้วจะยังใช้ HTTP compression ตามปกติ ค่าเริ่มต้นสำหรับช่วงว่างระหว่าง worker
response event คือ 30 วินาที และปรับได้ด้วย `RUVYXA_WORKER_TIMEOUT_MS` โดย Rust และ Node จะใช้ค่าที่
normalize แล้วค่าเดียวกัน ทั้งหมดนี้ทำงานอัตโนมัติ โดย handler ยังใช้ Web API `Response` ตามปกติ

## Unsupported Methods

เมื่อ handler ไม่ได้ export method นั้น server จะตอบ `405 Method Not Allowed`:

```json
{
  "ok": true,
  "status": 405,
  "headers": { "content-type": "text/plain; charset=utf-8" },
  "body": "Method DELETE is not allowed"
}
```

## Middleware & Security Headers

API routes ได้รับ security headers, rate limiting และ middleware ที่ตั้งค่าใน `ruvyxa.config.ts`
โดยอัตโนมัติ

Ruvyxa ส่ง query string, bytes ของ request body และ header ที่ซ้ำกันไปยัง `Request` โดยไม่แปลง
ข้อมูล binary เป็นข้อความ ส่วน response จะคง header ซ้ำ เช่น `Set-Cookie` หลายค่าไว้ครบถ้วน และส่ง
binary response body แบบ streaming

ดูเพิ่มเติม: [Configuration](configuration.md) สำหรับการตั้งค่า security และ middleware
