# Environment Variables

## Public vs Private

| ประเภท  | Prefix            | เข้าถึงจาก                                                          |
| ------- | ----------------- | ------------------------------------------------------------------- |
| Public  | `RUVYXA_PUBLIC_*` | Client bundle + Server                                              |
| Private | ทุกอย่างอื่น      | Server เท่านั้น (server-only modules, loaders, actions, API routes) |

## .env File

```dotenv
# .env
RUVYXA_PUBLIC_APP_NAME=Storefront
RUVYXA_PUBLIC_API_URL=https://api.example.com
DATABASE_URL=postgres://private-connection-string
```

## การใช้ Public Variables

```tsx
const appName = import.meta.env.RUVYXA_PUBLIC_APP_NAME
```

## TypeScript Declarations

เพิ่ม declaration ใน `app/ruvyxa-env.d.ts` ให้ TypeScript รู้จัก public variables:

```ts
interface ImportMetaEnv {
  RUVYXA_PUBLIC_APP_NAME: string
  RUVYXA_PUBLIC_API_URL: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
```

## Private Variables

Private variables เข้าถึงได้เฉพาะใน:

- Server-only modules (`import 'server-only'`)
- ฟังก์ชัน `loader`
- `action` handlers
- API routes (`route.ts`)
- Modules ภายใต้ไดเรกทอรี `server/`

```ts
// server/database.ts
import 'server-only'

export const databaseUrl = process.env.DATABASE_URL
```

## Security Rules

### ห้ามทำ

- เปลี่ยนชื่อ private variable เป็น `RUVYXA_PUBLIC_` เพื่อเลี่ยง validation
- import private variable เข้า client component
- ส่งค่า private env ผ่าน props ไปยัง client component
- เปิดเผย secret ผ่าน API route โดยไม่ได้ตั้งใจ

### จำไว้

`RUVYXA_PUBLIC_` คือการตัดสินใจที่จะส่งค่านั้นไปยัง browser bundle
ใช้เฉพาะกับค่าที่ปลอดภัยต่อการเปิดเผย

## Validation

Ruvyxa ตรวจสอบการใช้ environment variables ระหว่าง analysis:

```bash
npx ruvyxa analyze   # ตรวจจับ private env ใน client code
npx ruvyxa check     # ตรวจสอบเต็มรูปแบบรวมถึง env validation
```

## .env.example

สำหรับโปรเจคที่แชร์กับผู้อื่น สร้าง `.env.example` ที่ระบุ**เฉพาะชื่อตัวแปร ไม่ใช่ค่าจริง**:

```dotenv
# .env.example
RUVYXA_PUBLIC_APP_NAME=
RUVYXA_PUBLIC_API_URL=
# DATABASE_URL=   (private — ห้ามใส่ใน example)
```

ดูเพิ่มเติม: [Server & Client Components](server-client-components.md)
