# Routing

Routes ใน Ruvyxa มาจากชื่อไฟล์และโฟลเดอร์

| File                               | URL            |
| ---------------------------------- | -------------- |
| `app/page.tsx`                     | `/`            |
| `app/about/page.tsx`               | `/about`       |
| `app/blog/[slug]/page.tsx`         | `/blog/:slug`  |
| `app/docs/[...path]/page.tsx`      | `/docs/*path`  |
| `app/shop/[[...path]]/page.tsx`    | `/shop/*path?` |
| `app/(marketing)/pricing/page.tsx` | `/pricing`     |
| `app/api/health/route.ts`          | `/api/health`  |
| `app/guide/page.md` or `page.mdx`  | `/guide`       |

## Dynamic Segments

- `[name]` — required parameter
- `[...path]` — catch-all (1+ segments)
- `[[...path]]` — optional catch-all (0+ segments)

```tsx
// app/blog/[slug]/page.tsx
import type { PageProps } from 'ruvyxa/config'

export default function BlogPost({ params }: PageProps<{ slug: string }>) {
  return <h1>Post: {params.slug}</h1>
}
```

## Route Groups

ใช้วงเล็บ `(...)` จัดกลุ่มโดยไม่กระทบ URL: `app/(marketing)/about/page.tsx` → `/about`

## กฎการตั้งชื่อ

- โฟลเดอร์ที่ขึ้นต้นด้วย `_` หรือ `@` ถูก ignore
- Ruvyxa **ปฏิเสธ** โครงสร้างที่ ambiguous
- รัน `npx ruvyxa analyze` หลังจากเปลี่ยน routes

ดูเพิ่มเติม: [API Routes](api-routes.md)
