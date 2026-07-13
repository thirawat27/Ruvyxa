# Server & Client Components

## Default: Server Components

Pages เป็น server-rendered โดย default

## Client Components

ใช้ `'use client'` เฉพาะเมื่อต้องการ browser APIs หรือ state/effects:

```tsx
'use client'

import { useState } from 'react'

export default function Counter() {
  const [count, setCount] = useState(0)
  return <button onClick={() => setCount((v) => v + 1)}>{count}</button>
}
```

## Server-Only Code

```ts
// server/database.ts
import 'server-only'

export const databaseUrl = process.env.DATABASE_URL
```

## Boundary Validation

Ruvyxa ตรวจสอบ:

- `import 'server-only'` ใน client → **Rejected**
- `import 'client-only'` ใน server → **Rejected**
- Private env ใน client → **Rejected**

ห้ามเปลี่ยนชื่อ private variable เป็น `RUVYXA_PUBLIC_` เพื่อเลี่ยง validation

ดูเพิ่มเติม: [Environment Variables](environment-variables.md)
