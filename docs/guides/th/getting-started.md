# เริ่มต้นใช้งาน

## ความต้องการ

- **Node.js** 22 หรือใหม่กว่า
- **Package manager**: npm, pnpm, Yarn, หรือ Bun
- ไม่จำเป็นต้องมี Rust toolchain สำหรับการใช้งานโปรเจค

## สร้างโปรเจคใหม่

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run dev
```

เปิด `http://localhost:3000` โปรเจคเริ่มต้น:

```text
my-app/
├── app/
│   ├── globals.css
│   ├── layout.tsx
│   └── page.tsx
├── public/
├── .gitignore
├── package.json
├── ruvyxa.config.ts
└── tsconfig.json
```

ค่าเริ่มต้นคือ starter แบบ `minimal` และในเวอร์ชัน 1.0.15 สามารถเลือกแบบอื่นได้ดังนี้:

```bash
npm create ruvyxa@latest my-blog -- --template blog
npm create ruvyxa@latest my-admin -- --template crud
npm create ruvyxa@latest my-api -- --template api-backend
```

| Starter       | สิ่งที่มีให้                                                      |
| ------------- | ----------------------------------------------------------------- |
| `minimal`     | หน้าแรก, root layout, global stylesheet และ config                |
| `blog`        | รายการบทความ, dynamic post route และ SSG parameters แบบตรงไปตรงมา |
| `crud`        | Task API ในหน่วยความจำ, loader, cache และ validated action        |
| `api-backend` | REST endpoints สำหรับ health/items พร้อม validation และ errors    |

## โครงสร้างพื้นฐาน

| ไฟล์/โฟลเดอร์           | หน้าที่                                                  |
| ----------------------- | -------------------------------------------------------- |
| `app/layout.tsx`        | ครอบทุกหน้า (root layout)                                |
| `app/page.tsx`          | หน้า `/`                                                 |
| `app/<folder>/page.tsx` | Nested route                                             |
| `public/`               | Static files, serve จาก `/`                              |
| `ruvyxa.config.ts`      | ตั้งค่า server, build, rendering, security, cache, style |

## Page แรก

```tsx
// app/products/page.tsx → /products
export default function ProductsPage() {
  return (
    <main>
      <h1>Products</h1>
    </main>
  )
}
```

## Layout

```tsx
// app/layout.tsx
import './globals.css'

export const meta = {
  title: 'My Ruvyxa App',
  description: 'A production-ready application.',
}

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  )
}
```

## ขั้นตอนต่อไป

- [Routing](routing.md) — file-system routes, dynamic segments
- [Server & Client Components](server-client-components.md) — `'use client'`, `server-only`
- [Configuration](configuration.md) — `ruvyxa.config.ts` ฉบับเต็ม
- [Styling](styling.md) — global CSS, SCSS/Sass และ CSS Modules
