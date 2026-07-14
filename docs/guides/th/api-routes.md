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

## Unsupported Methods → 405

Handler ที่ไม่รองรับ method จะตอบ 405 โดยอัตโนมัติ

Ruvyxa ส่ง query string, bytes ของ request body และ header ที่ซ้ำกันไปยัง `Request` โดยไม่แปลง
ข้อมูล binary เป็นข้อความ และ response จะคง header ซ้ำ เช่น `Set-Cookie` หลายค่าไว้ครบถ้วน

ดูเพิ่มเติม: [Configuration](configuration.md)
