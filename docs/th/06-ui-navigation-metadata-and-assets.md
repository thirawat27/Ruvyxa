# UI, navigation, metadata และ asset

> **เป้าหมายของ tutorial:** ทำให้แอปนำทางได้ เข้าถึงได้ และพร้อมนำเสนอเนื้อหาจริง **เริ่มจาก:** page
> หรือ API flow ที่ทำงานได้จาก [ข้อมูล, action และ API route](05-data-actions-api.md)
> **Checkpoint:** นำทางด้วย Link ตรวจ metadata ของหน้า และยืนยันว่า asset หนึ่งรายการโหลดได้

`@ruvyxa/react` export React helper ที่รู้จัก framework helper เหล่านี้เป็น optional; React
component ปกติยังทำงานได้

## Navigation และ route state

ใช้ `Link` สำหรับ application navigation และ `useRouter()` สำหรับ imperative navigation
`usePathname()`, `useParams()`, `useSearchParams()`, `useSelectedRoute()` และ `useRouteContext()`
เปิดเผย client route state ปัจจุบัน

```tsx
'use client'
import { Link, useRouter, useSearchParams } from '@ruvyxa/react'

export function SearchControls() {
  const router = useRouter()
  const query = useSearchParams().get('q') ?? ''
  return (
    <>
      <Link href="/about">About</Link>
      <button onClick={() => router.push(`/search?q=${query}`)}>Search</button>
    </>
  )
}
```

`useSearchParams()` คืน set ว่างระหว่าง SSR เมื่อ query ใช้ไม่ได้ จึงอย่าใช้มันสำหรับ markup
ที่ต้องเหมือนกันใน server HTML `useRouter().pending` ติดตาม route-bundle navigation

### เลือก prefetch อย่างมีเหตุผล

`Link` render เป็น anchor ปกติก่อน แล้วค่อยเพิ่มความสามารถให้ click ที่เป็น same-window
และเข้าเงื่อนไข จึงยังรักษาพฤติกรรมเปิดแท็บใหม่, modified-click, download และ link ที่ไม่ใช่ `_self`
ได้ ค่าเริ่มต้นของ `prefetch` คือ `'hover'` ให้เลือก mode ตามโอกาสที่ผู้ใช้จะไปหน้านั้นและต้นทุนของ
bundle แทนการเปิด prefetch แบบ eager ทุกลิงก์

```tsx
import { Link } from '@ruvyxa/react'

export function ProductLinks() {
  return (
    <nav>
      {/* ค่าเริ่มต้น: warm เมื่อผู้ใช้แสดงเจตนาจะไป */}
      <Link href="/products/notebook">สมุดโน้ต</Link>

      {/* เหมาะกับ next step ที่เด่นและน่าจะเข้ามาใน viewport */}
      <Link href="/checkout" prefetch="viewport">
        ชำระเงิน
      </Link>

      {/* ไม่ warm ปลายทางใหญ่ที่มีโอกาสไปต่ำ */}
      <Link href="/reports" prefetch="none">
        รายงาน
      </Link>

      {/* แทนที่ URL ชั่วคราว และคงตำแหน่ง scroll เมื่อต้องการ */}
      <Link href="/search?q=paper" replace scroll={false}>
        ใช้ตัวกรอง
      </Link>

      {/* ปลายทางภายนอกใช้ anchor ปกติ */}
      <a href="https://status.example.com" target="_blank" rel="noreferrer">
        สถานะระบบ
      </a>
    </nav>
  )
}
```

ใช้ `prefetch="viewport"` อย่างจำกัดกับลิงก์เหนือพับหรือ next step ที่ชัดเจน เพราะจะโหลด route เมื่อ
ลิงก์เข้ามาใน viewport ใช้ `'none'` (หรือ `false`) กับปลายทางที่ผู้ใช้อาจไม่ไป `replace` จะแทนที่
history entry ปัจจุบัน, `scroll` มีค่าเริ่มต้นเป็น `true` และ `viewTransition` จะใช้ Browser View
Transitions API เมื่อ browser รองรับ

## Metadata และ error UI

ใช้ route `meta` export สำหรับ metadata แบบ hierarchy-aware ([Routing](04-routing-rendering.md))
หรือใช้ `<Seo>` ใน component สำหรับ tag ต่อ render `<Seo>` สามารถสร้าง Open Graph, X card, Article
JSON-LD, breadcrumb JSON-LD และ custom JSON-LD กำหนดรูปแบบ X card ด้วย `card` ส่วน prop
`twitterCard` เดิมถูกถอดออกแล้ว

```tsx
import { Seo, RuvyxaErrorBoundary } from '@ruvyxa/react'

export default function Product() {
  return (
    <RuvyxaErrorBoundary
      fallback={({ error, resetError }) => (
        <button onClick={resetError}>Retry: {error.message}</button>
      )}
    >
      <Seo
        title="Product"
        description="A documented product"
        canonical="https://example.test/product"
      />
      <main>...</main>
    </RuvyxaErrorBoundary>
  )
}
```

`RuvyxaErrorBoundary` ดัก React render error ของ descendant, เรียก `onError` เมื่อมี และส่ง
`resetError` ให้ fallback มันไม่แทน route-level `error.tsx` boundary

## Image, CSS และ static file

`Image` รับ React image prop พร้อม Ruvyxa option โดย production build จะแทน PNG/JPEG แบบ local แต่ละ
ไฟล์ด้วย WebP เพียงไฟล์เดียวเป็นค่าเริ่มต้น และจะไม่ publish ทั้งไฟล์ต้นฉบับหรือ responsive variant
ให้ใช้ `<Image>` หรืออ้าง URL `.webp` ที่สร้างแล้วโดยตรง ตั้ง `image.keepOriginal: true`
เมื่อต้องรองรับ raw `<img>` และใช้ `image.variantWidths` เมื่อต้องการสร้าง variant แบบ opt-in สำหรับ
`srcSet` ที่กำหนดเอง ส่วน `image.onDemand` สร้าง responsive URL อัตโนมัติผ่าน same-origin runtime
transformation ที่ `/__ruvyxa/image` และมี maximum width ปริยาย 3840 เมื่อกำหนดเป็น object

> **`image.onDemand` ให้บริการโดย `ruvyxa dev` และ `ruvyxa start` เท่านั้น** การแปลงรูปทำผ่าน native
> image pipeline ซึ่งไม่มีอยู่ในสิ่งที่ build ปล่อยออกมา ทุก deployed artifact ตอบ `/__ruvyxa/image`
> ด้วย 404 และความล้มเหลวนี้เงียบโดยธรรมชาติ เพราะเบราว์เซอร์ที่โหลด `srcSet` ไม่ได้จะ fallback
> ไปที่ `src` หน้าจึงยังขึ้นปกติ ต้นทุนที่เสียคือมือถือโหลดรูปขนาดเต็ม `ruvyxa build`
> จะเตือนเมื่อเปิดตัวเลือกนี้ และ `ruvyxa test:parity` รายงานเป็น `images@1` ให้ใช้
> `image.variantWidths` สร้างขนาดที่ deployment ต้องใช้ไว้ล่วงหน้าแทน

<!-- prettier-ignore -->
```tsx
import { Image } from '@ruvyxa/react'
export function Hero() {
  return (
    <Image
      src="/hero.jpg"
      alt="Team at work"
      width={1200}
      height={630}
      priority
    />
  )
}
```

ไฟล์ใน `public/` เสิร์ฟพร้อมรองรับ byte range ทำให้ `<video>` และ `<audio>`
เลื่อนตำแหน่งได้โดยไม่ต้องดาวน์โหลดใหม่ และการดาวน์โหลดที่ขาดตอนสามารถทำต่อได้ `ruvyxa start` กับ
deployment แบบ standalone/node ตอบ range เหมือนกัน โดย range เดี่ยว `Range: bytes=…` คืน `206` พร้อม
`Content-Range` range ที่เลยท้ายไฟล์คืน `416` และ multi-range จะคืนไฟล์ทั้งไฟล์ asset ที่ใหญ่กว่า 8
MiB จะ stream จากดิสก์แทนการ buffer และ ranged request ของไฟล์นั้นจะอ่านเฉพาะไบต์ที่ขอเท่านั้น

imported project CSS อาจอยู่นอก `app/` ได้ หากต้อง include global style ที่ module ไม่ได้ import
ให้ใส่ file/directory แบบ project-relative ใน `css.entries` runtime รู้จัก Sass เป็น package
dependency ให้ใช้ style ที่ build resolve ได้ และรัน `npm run check` หลังเปลี่ยน boundary

### PostCSS และ Tailwind CSS

ถ้า project root มี PostCSS configuration Ruvyxa จะรัน plugin chain ของคุณกับ global stylesheet
ทุกไฟล์ที่เก็บรวบรวมได้ — ทั้งใน `ruvyxa dev` และ `ruvyxa build` ผ่าน code path เดียวกัน ผลลัพธ์ CSS
จึงตรงกัน

ชื่อไฟล์ config ที่รองรับ ตามลำดับนี้: `postcss.config.mjs`, `postcss.config.js`,
`postcss.config.cjs`, `postcss.config.ts`, `postcss.config.mts`, `postcss.config.cts`,
`postcss.config.json`, `.postcssrc.mjs`, `.postcssrc.js`, `.postcssrc.cjs`, `.postcssrc.json`,
`.postcssrc`

Ruvyxa ไม่ hard-code plugin ใดไว้เอง config ประกาศอะไร สิ่งนั้นคือสิ่งที่รัน โดย resolve จาก
`node_modules` ของโปรเจกต์ Tailwind CSS v4 จึงไม่ต้องใช้อะไรที่เฉพาะกับ framework:

```js
// postcss.config.mjs
export default { plugins: { '@tailwindcss/postcss': {} } }
```

```css
/* app/globals.css */
@import 'tailwindcss';
```

```bash
npm install -D postcss tailwindcss @tailwindcss/postcss
```

รายละเอียดที่ควรรู้:

- **plugin รันต่อหนึ่ง stylesheet entry หลังจาก `@import` ภายในโปรเจกต์ถูก inline แล้ว** partial ที่
  ดึงเข้ามาด้วย `@import "./theme.css"` จึงผ่าน plugin chain ไปพร้อมกับ entry ของมัน
- **config รองรับรูปแบบที่คุ้นเคยอยู่แล้ว** ทั้ง array ของ plugin, object แบบ `{ name: options }`
  หรือ function ที่รับ `{ mode }` โดย `mode` เป็น `production` ตอน `ruvyxa build` และ `development`
  ตอน `ruvyxa dev`
- **ไฟล์ที่ plugin อ่านจะกลายเป็น watch input** Tailwind รายงาน template ที่มันสแกนหา class name
  การแก้ component ตอน dev จึง regenerate stylesheet ให้
- **plugin ที่ล้มเหลวจะทำให้ build ล้มเหลว** Ruvyxa ไม่ fallback ไปใช้ CSS ที่ยังไม่ transform เพราะ
  `@import "tailwindcss"` ที่หลุดถึง browser จะทำให้หน้าแสดงด้วย browser default ซึ่งดูเหมือน bug
  ของ style มากกว่า build ที่ล้มเหลว ดู `RUV1405` และ `RUV1406` ใน
  [Troubleshooting](16-troubleshooting-upgrades.md)
- **โปรเจกต์ที่ไม่มี PostCSS config ไม่ได้รับผลกระทบ** CSS pipeline ทำงานเหมือนเดิมทุกประการ
  stylesheet ที่ import `tailwindcss` โดยไม่มี PostCSS config ยัง fallback ไปใช้ `@tailwindcss/cli`
  ได้เมื่อติดตั้งไว้

อย่าเพิ่ม script Tailwind CLI แยกควบคู่กับ PostCSS config การมี build pipeline สองชุดกับ stylesheet
เดียวจะทำให้ live reload, asset manifest และตำแหน่งที่รายงาน error ไม่สอดคล้องกัน

**ก่อนหน้า:** [ข้อมูล, action และ API route](05-data-actions-api.md) · **ถัดไป:**
[Configuration และ environment](07-configuration.md)

## Typed routes

เมื่อตั้ง `typedRoutes: true` ใน `ruvyxa.config.ts` Ruvyxa จะเขียน `.ruvyxa/types/routes.d.ts` จาก
route graph ที่ค้นพบ จากนั้น `<Link href>`, `useRouter().push`, `useRouter().replace` และ
`useRouter().prefetch` จะถูกตรวจกับ route ที่มีอยู่จริง path ที่พิมพ์ผิดจะกลายเป็น compile error
แทนที่จะเป็น 404 ที่มีคนไปเจอทีหลัง

ไฟล์นี้ถูกเขียนใหม่โดย `ruvyxa dev` ทุกครั้งที่ค้นหา route ใหม่ และเขียนหนึ่งครั้งโดย `ruvyxa build`
กับ `ruvyxa check` มันคือไฟล์ที่ถูก generate: อย่าแก้เอง และอย่า commit

TypeScript จะอ่านไฟล์นี้ก็ต่อเมื่อ `tsconfig.json` include มันไว้:

```json
{
  "include": ["app", "ruvyxa.config.ts", ".ruvyxa/types/**/*.d.ts"]
}
```

โปรเจกต์ที่สร้างด้วย `create-ruvyxa` มีทั้งค่าใน config และ `include` นี้มาให้แล้ว `ruvyxa check`
จะรายงาน `RUV1502` ถ้าเปิดค่านี้ไว้แต่ไม่มี `include` เพราะไฟล์ที่ถูก generate แล้วไม่มีใครอ่าน
หน้าตาเหมือนฟีเจอร์ที่ทำงานอยู่ทุกประการ

ถ้าไม่เปิดค่านี้ — และในทุกโปรเจกต์ที่มีมาก่อนฟีเจอร์นี้ — `href` ยังเป็น `string` เหมือนเดิม
และการตรวจ type ไม่เปลี่ยนแปลงอะไรเลย

### อะไรที่จับได้และจับไม่ได้

segment แบบ dynamic จะขยายเป็น `${string}` ซึ่งละเอียดที่สุดเท่าที่ template literal type ของ
TypeScript ทำได้: ไม่มีวิธีเขียนว่า "string ใด ๆ ที่ไม่มี slash" ดังนั้น:

```tsx
<Link href="/blog/hello">Post</Link>        // ผ่าน
<Link href="/blog/hello?draft=1">Post</Link> // ผ่าน — query และ hash ใช้ได้
<Link href="https://example.com">Docs</Link> // ผ่าน — URL ภายนอกยังใช้ได้
<Link href="/abuot">About</Link>             // error: ไม่มี route นี้
<Link href="/blogs/hello">Post</Link>        // error: ส่วน static ผิด
<Link href="/blog/a/b">Post</Link>           // ผ่าน ทั้งที่ `[slug]` คือหนึ่ง segment
```

บรรทัดสุดท้ายคือข้อจำกัดที่รู้อยู่ และเป็นข้อจำกัดเดียวกับที่ Next.js มี
สิ่งที่การตรวจนี้จับได้แน่นอน คือความผิดพลาดที่พบบ่อยกว่ามาก: ส่วน static ของ path ที่ผิด

### URL ที่สร้างตอน runtime

path ที่ประกอบจากข้อมูลมีชนิดเป็น `string` และ `string` ไม่สามารถ assign เข้า union ของ literal ได้
ให้ครอบด้วย `route()`:

```tsx
import { Link, route } from '@ruvyxa/react'

;<Link href={route(record.canonicalUrl)}>Open</Link>
```

`route()` เป็นการ assert ไม่ใช่การตรวจสอบ ควรใช้ template ที่สร้างจาก pattern ที่เป็น literal ก่อน —
`` `/blog/${slug}` `` ผ่านการตรวจ type ได้ด้วยตัวเอง — และเก็บ `route()`
ไว้สำหรับค่าที่รู้ไม่ได้จริง ๆ ตอน compile

## สคริปต์จากภายนอก

`<Script>` โหลดสคริปต์ภายนอกหรือสคริปต์ inline โดยไม่วางไว้บนเส้นทางวิกฤต และดาวน์โหลดครั้งเดียว
ต่อหนึ่งเอกสารไม่ว่าจะมีกี่ route ที่ render มัน

```tsx
import { Script } from '@ruvyxa/react'

<Script src="https://plausible.io/js/script.js" strategy="lazyOnload" />
<Script id="consent" strategy="beforeInteractive">{`window.__consent = true`}</Script>
```

| `strategy`                     | ทำงานเมื่อไร                                           | เหมาะกับ                                |
| ------------------------------ | ------------------------------------------------------ | --------------------------------------- |
| `beforeInteractive`            | ถูก render ลง HTML ฝั่งเซิร์ฟเวอร์ ทำงานก่อน hydration | consent gating, A/B bucketing, polyfill |
| `afterInteractive` (ค่าปริยาย) | ต่อท้าย `<body>` หลัง hydration                        | analytics, tag manager                  |
| `lazyOnload`                   | ต่อท้ายเมื่อเบราว์เซอร์ว่างหลัง `load`                 | chat widget, ป็อปอัปช่วยเหลือ           |

การกันซ้ำใช้ `id` เป็นกุญแจ ถ้าไม่มีจะใช้ `src` แทน กุญแจนี้อยู่รอดข้ามการนำทางฝั่ง client ดังนั้น
การออกจาก route แล้วกลับมาจะไม่รัน analytics tag ซ้ำอีกรอบ สคริปต์ที่โหลดล้มเหลวจะปล่อยกุญแจคืน
เพื่อให้การ render ครั้งถัดไปลองใหม่ได้

`beforeInteractive` เป็นกลยุทธ์เดียวที่ใช้ได้บนหน้าที่มี `export const hydrate = false`: กลยุทธ์อื่น
ถูกต่อท้ายด้วย effect และหน้าแบบนั้นไม่ส่ง client runtime ไปให้ effect ทำงาน สคริปต์ inline ต้องมี
`id` เพราะไม่มี `src` ให้ใช้ระบุตัวตน
