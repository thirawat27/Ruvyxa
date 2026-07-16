# API Routes

สร้าง `route.ts` และ export HTTP method handlers:

```ts
// app/api/health/route.ts
export function GET() {
  return Response.json({ ok: true })
}

export async function POST({ request }: { request: Request }) {
  const body = await request.json()
  return Response.json({ received: body }, { status: 201 })
}
```

Handler ได้รับ `{ request, params }`:

- `request` — Web API `Request` object
- `params` — dynamic route parameters: `[id]` เป็น `string`, `[...slug]` เป็น `string[]`, และ
  `[[...slug]]` ที่ไม่มี segment เป็น `undefined`

## Response Types

```ts
export function GET() {
  return new Response('Hello', { headers: { 'Content-Type': 'text/plain' } })
}

export function GET() {
  return Response.json({ data: [1, 2, 3] })
}

export function GET() {
  return Response.redirect('/dashboard', 302)
}
```

## Body Size Limits

| Limit    | Default | Config              |
| -------- | ------- | ------------------- |
| API body | 10 MiB  | `security.apiLimit` |

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

Worker IPC ใช้ frame ขนาดไม่เกิน 64 KiB และจำกัด queue แยกต่อ response หาก producer หรือ consumer
ค้าง ระบบจะยุติเฉพาะ stream นั้นแทนการปล่อยให้หน่วยความจำที่รอส่งโตไม่จำกัด ทั้งหมดนี้ทำงานอัตโนมัติ
โดย handler ยังใช้ Web API `Response` ตามปกติ

## Unsupported Methods → 405

Handler ที่ไม่รองรับ method จะตอบ 405 โดยอัตโนมัติ

Ruvyxa ส่ง query string, bytes ของ request body และ header ที่ซ้ำกันไปยัง `Request` โดยไม่แปลง
ข้อมูล binary เป็นข้อความ ส่วน response จะคง header ซ้ำ เช่น `Set-Cookie` หลายค่าไว้ครบถ้วน และส่ง
binary response body แบบ streaming

ดูเพิ่มเติม: [Configuration](configuration.md)
