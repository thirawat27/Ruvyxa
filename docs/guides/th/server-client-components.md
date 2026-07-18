# Server & Client Components

## Default: Server Components

Pages เป็น server-rendered โดย default ทุกอย่างอยู่บน server — ไม่มีอะไรถูกส่งไป browser bundle
ยกเว้นระบุไว้ชัดเจน

## Client Components

ใช้ `'use client'` เฉพาะเมื่อ module ต้องการ:

- Browser APIs (`window`, `document`, `localStorage`, ฯลฯ)
- React state / effects (`useState`, `useEffect`, `useReducer`)
- Event handlers (`onClick`, `onChange`, ฯลฯ)

```tsx
'use client'

import { useState } from 'react'

export default function Counter() {
  const [count, setCount] = useState(0)
  return <button onClick={() => setCount((value) => value + 1)}>{count}</button>
}
```

## Server-Only Code

เก็บ private code ให้พ้นจาก client graph วาง database access และ secrets ใน server-only module
และกำกับไว้ชัดเจน:

```ts
// server/database.ts
import 'server-only'

export const databaseUrl = process.env.DATABASE_URL
```

## Boundary Validation

Ruvyxa ตรวจสอบ imports และการเข้าถึง environment ณ build time:

| Import / Access                  | Client Bundle | Server Bundle |
| -------------------------------- | ------------- | ------------- |
| `import 'server-only'`           | **Rejected**  | Allowed       |
| `import 'client-only'`           | Allowed       | **Rejected**  |
| Module ภายใต้ไดเรกทอรี `server/` | **Rejected**  | Allowed       |
| Private environment variables    | **Rejected**  | Allowed       |
| `RUVYXA_PUBLIC_*` variables      | Allowed       | Allowed       |

**ห้าม**เลี่ยง diagnostic เหล่านี้โดยการเปิดเผย secret ไปยัง browser ห้ามเปลี่ยนชื่อ private
variable เป็น `RUVYXA_PUBLIC_` เพื่อเลี่ยง validation — prefix
นั้นคือการตัดสินใจที่จะส่งค่านั้นไปยัง browser bundle

## Best Practices

- วาง components ที่ต้องการ browser API เป็น client components
- ส่งข้อมูล server ไปยัง client components ผ่าน props
- ใช้ prefix `server/` สำหรับ server-only utilities
- ใช้ `import 'server-only'` สำหรับไฟล์ที่ต้องอยู่บน server เท่านั้น

ดูเพิ่มเติม: [Environment Variables](environment-variables.md)
