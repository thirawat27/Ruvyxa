# Server Actions

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

## HTML Form

```tsx
<form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
  <input name="title" required />
  <button type="submit">Create</button>
</form>
```

## Security (Default)

| Protection     | Default                       |
| -------------- | ----------------------------- |
| Body limit     | 1 MiB                         |
| Same-origin    | Enabled                       |
| Fetch Metadata | Enabled                       |
| Rate limiting  | 600 req / client-action / 60s |

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

ดูเพิ่มเติม: [Configuration](configuration.md), [Data Loading](data-loading-and-cache.md)
