# เริ่มต้นใช้งาน

> 🟢 **เหมาะกับมือใหม่** · ⏱️ อ่าน ~5 นาที
>
> **จะได้เรียนรู้:** ติดตั้ง Ruvyxa, สร้างโปรเจกต์, ทำหน้าแรก ๆ ของตัวเอง และรู้ว่าต้องไปทางไหน
> เมื่อมีอะไรพัง — ไม่ต้องเคยใช้ framework มาก่อน รู้ React พื้นฐานก็พอ

Ruvyxa เป็น React framework ที่ **โฟลเดอร์คือ route, หน้าคือ component และ toolchain เป็น native
binary ตัวเดียว** ถ้ารู้ React พื้นฐานอยู่แล้ว คุณรู้เกือบทุกอย่างที่ต้องใช้ —
หน้านี้พาจากศูนย์ถึงแอปที่รันได้ในราวห้านาที

## ความต้องการ

- **Node.js** 22 หรือใหม่กว่า (เช็คด้วย `node --version`)
- **Package manager**: npm, pnpm, Yarn, หรือ Bun
- ไม่จำเป็นต้องมี Rust toolchain — CLI native มาเป็น binary สำเร็จรูปตามแพลตฟอร์ม

ไม่แน่ใจเรื่อง environment? ให้ Ruvyxa ตรวจให้หลังติดตั้ง: `npx ruvyxa doctor`

## สร้างโปรเจคใหม่

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run dev
```

เปิด `http://localhost:3000` — จะเห็นหน้า starter ลองแก้ `app/page.tsx` แล้วบันทึก browser
อัปเดตทันทีโดยไม่ reload ทั้งหน้า (นั่นคือ HMR) โปรเจคเริ่มต้น:

```text
my-app/
├── app/
│   ├── globals.css
│   ├── layout.tsx
│   └── page.tsx
├── public/
│   └── ruvyxa.png
├── .gitignore
├── package.json
├── ruvyxa.config.ts
└── tsconfig.json
```

ค่าเริ่มต้นคือ starter แบบ `minimal` สามารถเลือกแบบอื่นได้ดังนี้:

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

### Git Ignore

Starter จะ ignore `node_modules/`, `.ruvyxa/`, `dist/`, log files และ `.env` files:

- **ห้าม commit secrets** หรือค่า environment จริง
- ใช้ `.env.example` เพื่อระบุชื่อตัวแปรที่จำเป็น โดยไม่มีค่าจริง

## โครงสร้างพื้นฐาน

Ruvyxa ค้นพบ routes ภายใต้ `app/`:

| ไฟล์/โฟลเดอร์           | หน้าที่                                                  |
| ----------------------- | -------------------------------------------------------- |
| `app/layout.tsx`        | ครอบทุกหน้า (root layout)                                |
| `app/page.tsx`          | หน้า `/`                                                 |
| `app/<folder>/page.tsx` | Nested route                                             |
| `public/`               | Static files, serve จาก `/`                              |
| `ruvyxa.config.ts`      | ตั้งค่า server, build, rendering, security, cache, style |

## Page แรก

ทุก page file ต้อง default-export React component:

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

เก็บ layout concerns ใน `app/layout.tsx`:

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

## Standard Scripts

```json
{
  "scripts": {
    "dev": "ruvyxa dev",
    "build": "ruvyxa build",
    "start": "ruvyxa start",
    "typecheck": "tsc --noEmit",
    "check": "ruvyxa check"
  }
}
```

## 10 นาทีแรกของคุณ

เส้นทางแนะนำเมื่อ `npm run dev` รันแล้ว:

1. **แก้หน้าแรก** — แก้ `app/page.tsx` ดู HMR อัปเดต browser
2. **เพิ่มหน้าที่สอง** — สร้าง `app/about/page.tsx` ที่ default-export component แล้วเปิด `/about`
   ไม่ต้องลงทะเบียนที่ไหน โฟลเดอร์*คือ* route
3. **เพิ่มหน้า dynamic** — สร้าง `app/hello/[name]/page.tsx` อ่าน `params.name` แล้วเปิด
   `/hello/world`
4. **ดูสิ่งที่ framework เห็น** — รัน `npx ruvyxa routes` พิมพ์ตาราง route ที่ค้นพบ
5. **Ship** — `npm run build` แล้ว `npm run start` รัน production server จริงบนเครื่อง

## เมื่อมีอะไรพัง

| อาการ                          | ลองอันนี้ก่อน                                                                  |
| ------------------------------ | ------------------------------------------------------------------------------ |
| `npm run dev` ไม่ start        | `npx ruvyxa doctor` — เช็ค Node version, port, config                          |
| Port 3000 ไม่ว่าง              | Ruvyxa สแกน 100 port ถัดไปอัตโนมัติพร้อมบอกว่าใครใช้อยู่ หรือใส่ `--port 4000` |
| URL ขึ้น 404 ทั้งที่ไม่ควร     | `npx ruvyxa routes` — route อยู่ในตารางไหม?                                    |
| Build fail พร้อมรหัส `RUV____` | ข้อความบอกไฟล์ + คำแนะนำ; รหัสมีเอกสารใน diagnostics reference                 |
| Output ค้างหลังแก้เยอะ         | `npx ruvyxa clean` ล้าง cache `.ruvyxa/` อย่างปลอดภัย                          |

ทุก error ของ Ruvyxa มีรหัส `RUV` คงที่ ไฟล์ต้นเหตุ และคำแนะนำแก้ — อ่านข้อความก่อนไปหาในเว็บ
คำตอบมักอยู่ในนั้นแล้ว

## ศัพท์ที่จะเจอในเอกสารชุดนี้

อ่านผ่าน ๆ รอบเดียวพอ — ทุกบทใช้คำเหล่านี้

| คำ                 | ความหมายแบบบ้าน ๆ                                                                     |
| ------------------ | ------------------------------------------------------------------------------------- |
| **Route**          | URL ที่แอปตอบ สร้างโดยทำโฟลเดอร์ที่มี `page.tsx` ใต้ `app/`                           |
| **Layout**         | Component ที่ห่อทุกหน้าที่อยู่ใต้มัน (แถบเมนู, footer)                                |
| **HMR**            | Hot Module Replacement — เซฟไฟล์แล้ว browser อัปเดตทันทีไม่ต้อง reload                |
| **SSR**            | HTML ถูก render บน server ทุก request — เหมาะกับข้อมูลสด                              |
| **SSG**            | HTML ถูก render ครั้งเดียวตอน build — เร็วสุด เหมาะกับหน้าคงที่                       |
| **ISR**            | SSG ที่ render ตัวเองใหม่เมื่อครบเวลา — เหมาะกับหน้า "เกือบ static"                   |
| **CSR**            | หน้า render ใน browser ด้วย JavaScript — สำหรับ UI ที่ interactive หนัก               |
| **PPR**            | Shell แบบ static เสิร์ฟทันที ส่วนที่ช้า stream ตามมา — ได้ข้อดีสองทาง                 |
| **API route**      | ไฟล์ `route.ts` ที่ export `GET`/`POST`/… — endpoint หลังบ้าน ไม่มีหน้าเว็บ           |
| **Server action**  | ฟังก์ชันฝั่ง server ที่เรียกจาก form/component พร้อม validation — mutation แบบปลอดภัย |
| **Adapter**        | ตัวแปลง build output ให้อยู่ในรูปที่ hosting แต่ละเจ้าต้องการ                         |
| **`.ruvyxa/`**     | โฟลเดอร์ผลลัพธ์ build — ถูก generate เสมอ ห้ามแก้ ห้าม commit                         |
| **รหัส `RUV____`** | ทุก error ของ Ruvyxa มีรหัสคงที่ ไฟล์ต้นเหตุ และคำแนะนำแก้                            |

รายละเอียด rendering เต็ม ๆ อยู่ที่ [Rendering Strategies](rendering-strategies.md) — มีตาราง
"ควรใช้ตัวไหน?" ให้เลือกง่าย ๆ ด้วย

## ขั้นตอนต่อไป

- [Routing](routing.md) — file-system routes, dynamic segments, catch-all, route groups
- [Server & Client Components](server-client-components.md) — `'use client'`, `server-only`,
  boundary checks
- [Configuration](configuration.md) — `ruvyxa.config.ts` ฉบับเต็ม
- [Styling](styling.md) — global CSS, SCSS/Sass และ CSS Modules
- [แพ็กเกจทางการ](official-packages.md) — เพิ่มฐานข้อมูล ระบบ login และ realtime เมื่อพร้อม
