# Environment Variables

| ประเภท  | Prefix            | เข้าถึงจาก      |
| ------- | ----------------- | --------------- |
| Public  | `RUVYXA_PUBLIC_*` | Client + Server |
| Private | ทุกอย่างอื่น      | Server เท่านั้น |

```dotenv
# .env
RUVYXA_PUBLIC_APP_NAME=Storefront
DATABASE_URL=postgres://...
```

```tsx
const appName = import.meta.env.RUVYXA_PUBLIC_APP_NAME
```

## Type Declarations

```ts
// app/ruvyxa-env.d.ts
interface ImportMetaEnv {
  RUVYXA_PUBLIC_APP_NAME: string
}
interface ImportMeta {
  readonly env: ImportMetaEnv
}
```

ห้ามเปลี่ยนชื่อ private variable เป็น `RUVYXA_PUBLIC_` เพื่อเลี่ยง validation

ดูเพิ่มเติม: [Server & Client Components](server-client-components.md)
