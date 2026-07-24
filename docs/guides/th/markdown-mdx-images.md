# Markdown, MDX, Images & Metadata

> 🟢 **เหมาะกับมือใหม่** · ⏱️ อ่าน ~7 นาที
>
> **จะได้เรียนรู้:** เขียนหน้าเว็บด้วย Markdown/MDX, รูปถูก optimize เป็น WebP ให้อัตโนมัติ และตั้ง
> SEO metadata รายหน้า

## Markdown และ MDX Pages

`page.md` และ `page.mdx` เป็น first-class route files รองรับ frontmatter, Markdown, MDX/JSX และ
dev/prod pipeline เดียวกับ TSX pages:

```mdx
---
title: Welcome
description: A page written in MDX.
---

# {frontmatter.title}

This page can contain **Markdown** and <strong>JSX</strong>.
```

ความสามารถที่รองรับ:

- **YAML frontmatter** — รองรับ nested objects, arrays, quoted values และ block scalars ผ่าน
  `frontmatter` object; `meta` จะอ้างถึง object เดียวกันหากไฟล์ไม่ได้ export ค่าเอง
- **GFM** — tables พร้อม alignment, task lists, strikethrough, autolink literals, references และ
  footnotes ใช้ได้ทั้ง Markdown และ MDX
- **JSX** — ฝัง React components, member components เช่น `<Card.Header>`, fragments และ prop spreads
  ได้ใน `.mdx`
- **Expressions และ ESM** — `{variable}`, `{expression}`, multiline `import` และ multiline `export`
  ถูก parse เป็น JavaScript/TypeScript โดยตรง ไม่ตัดสินจากบรรทัดแบบเดิม
- **Heading exports** — headings ถูก export สำหรับสร้าง table-of-contents; heading ที่ชื่อซ้ำจะได้
  suffix `-1`, `-2` ต่อเนื่องและตรงกับ ID ที่ render
- **Component overrides** — generated MDX page รับ `components` prop เพื่อแทน element อย่าง `h1`,
  `a`, `table` และ `code`
- **SSG** — pre-rendered ณ build time

```mdx
---
title: Content guide
author:
  name: Ada
tags: [mdx, gfm]
summary: |
  Nested YAML และ multiline values จะถูกเก็บครบ
---

import { Callout } from './Callout'

export const status = {
  stable: true,
}

## {frontmatter.title}

<Callout {...status}>Ready</Callout>
```

YAML ที่ผิดรูป, frontmatter ที่ไม่ปิด, MDX ESM ที่ไม่ถูกต้อง และ generated JavaScript ที่ compile
ไม่ได้จะหยุด build พร้อม diagnostic ส่วน Markdown รองรับ raw HTML จึงควรใช้กับเนื้อหาที่ผู้เขียน
ควบคุมได้; หากรับเนื้อหาจากภายนอกควร sanitize ก่อนเข้าสู่ build

## Images

วาง static assets ใน `public/` และอ้างอิงจาก `/`:

```tsx
import { Image, Seo } from '@ruvyxa/react'

export default function Home() {
  return (
    <>
      <Seo title="Home" description="Welcome" canonical="https://example.com" />
      <Image src="/hero.png" alt="Product overview" width={1600} height={900} priority />
    </>
  )
}
```

### Image Optimization

`Image` แปลงไฟล์ PNG/JPEG ในเครื่องเป็น WebP ระหว่าง production build เมื่อเปิด image optimization:

| Config                | Default          | คำอธิบาย                                               |
| --------------------- | ---------------- | ------------------------------------------------------ |
| `image.optimize`      | `true`           | เปิด / ปิด image optimization                          |
| `image.quality`       | `82`             | คุณภาพ WebP (1–100)                                    |
| `image.lossless`      | `false`          | โหมด lossless                                          |
| `image.variantWidths` | `[640, …, 3840]` | breakpoint สำหรับ responsive; `[]` ปิดการสร้าง variant |
| `image.workers`       | `0`              | จำนวน thread (0 = auto = CPU count)                    |

```ts
// ruvyxa.config.ts
export default config({
  image: {
    optimize: true,
    quality: 85,
    lossless: false,
    workers: 4,
  },
})
```

Remote URLs **จะไม่ถูก**แปลง — เฉพาะ local assets ภายใต้ `public/` เท่านั้น

### รูปภาพแบบ responsive

ใส่ `sizes` ให้ `<Image>` แล้วมันจะสร้าง `srcset` ให้ เบราว์เซอร์จะโหลดรูปขนาดที่เหมาะกับอุปกรณ์
ไม่ใช่ไฟล์ต้นฉบับความละเอียดเต็ม:

```tsx
<Image src="/hero.png" alt="" width={1600} height={900} sizes="100vw" />
```

ตอน build จะเขียนไฟล์ `hero-<w>w.webp` ที่แต่ละ breakpoint ใน `image.variantWidths`
ที่แคบกว่าต้นฉบับ และ `<Image>` อ้างถึงไฟล์เหล่านั้นพอดี — จำกัดที่ `width` จริงของรูป
เบราว์เซอร์จึงไม่ร้องขอ variant ที่ไม่ได้ถูกสร้าง หากใช้ `loader` เอง, `unoptimized` หรือ source
ที่เป็น remote/SVG จะปล่อย markup ไว้ตามเดิม

### Image Best Practices

- ระบุ `width` และ `height` จริงเพื่อป้องกัน layout shift (CLS)
- ใช้ `fill` prop เมื่อรูปต้องเติม container
- ใช้ `priority` สำหรับ LCP (Largest Contentful Paint) images
- ใช้ `<Image>` component แทน `<img>` เพื่อรับ optimization อัตโนมัติ

## SEO & Metadata

### `<Seo>` Component

```tsx
import { Seo } from '@ruvyxa/react'

export default function HomePage() {
  return (
    <Seo
      title="My Page"
      description="A concise description for search results"
      canonical="https://example.com/page"
      image="https://example.com/og-image.png"
      type="article"
      twitterCard="summary_large_image"
      article={{
        type: 'BlogPosting',
        publishedAt: '2026-07-22',
        updatedAt: '2026-07-23T10:30:00+07:00',
        authors: [{ name: 'Ada', url: 'https://example.com/authors/ada' }],
        tags: ['Ruvyxa', 'SSR'],
      }}
      breadcrumbs={[
        { name: 'Home', url: 'https://example.com/' },
        { name: 'My Page', url: 'https://example.com/page' },
      ]}
    />
  )
}
```

`article` และ `breadcrumbs` จะสร้าง Article กับ BreadcrumbList JSON-LD ที่ escape อย่างปลอดภัยจาก
ข้อเท็จจริงของหน้า ใช้ `jsonLd` สำหรับ schema ชนิดอื่นที่ตรงกับหน้านั้น และห้ามระบุข้อมูลที่ผู้อ่าน
มองไม่เห็นในหน้า

### เนื้อหาที่พร้อมสำหรับ Answer Engine

ใช้ `Answer` เพื่อแสดงคำตอบแบบสั้น ชัดเจน เข้าถึงได้ และมีแหล่งอ้างอิง:

```tsx
import { Answer } from '@ruvyxa/react'

export default function RenderingAnswer() {
  return (
    <Answer
      question="Ruvyxa render ที่ server หรือไม่?"
      answer="ใช่ หน้าเว็บจะ render ที่ server เป็นค่าเริ่มต้น"
      sources={[{ name: 'คู่มือ Rendering', url: '/docs/rendering' }]}
      sourcesLabel="แหล่งอ้างอิง"
    />
  )
}
```

`Answer` ครอบข้อความเดียวกับที่ผู้อ่านเห็นด้วย Schema.org Question/Answer microdata โดยไม่สร้าง
`FAQPage` หรือ `QAPage` อัตโนมัติ เพราะ schema สองแบบนี้มีเงื่อนไขเฉพาะและควรใช้เมื่อทั้งหน้าตรงตาม
ประเภทนั้นจริงเท่านั้น

สำหรับชุด Markdown/MDX ให้ใช้ร่วมกับ `contentEngine()` โดย `answers` frontmatter ที่ระบุเองจะอยู่ใน
`/content.json` และ discovery index แบบ experimental ที่ `/llms.txt`:

```mdx
---
title: คู่มือ Rendering
description: วิธีที่ Ruvyxa render หน้าเว็บ
answers:
  - question: Ruvyxa render ที่ server หรือไม่?
    answer: ใช่ หน้าเว็บจะ render ที่ server เป็นค่าเริ่มต้น
    sources:
      - name: คู่มือ Rendering
        url: /docs/rendering
---

import { Answer } from '@ruvyxa/react'

# {frontmatter.title}

<Answer {...frontmatter.answers[0]} sourcesLabel="แหล่งอ้างอิง" />
```

รูปแบบนี้ทำให้คำตอบที่มองเห็นและ content graph ใช้ข้อมูลชุดเดียวที่ผู้เขียนควบคุม `llms.txt` เป็น
เพียงความสะดวกแบบ experimental ไม่ใช่ ranking signal และไม่แทน crawlable HTML, canonical URL,
sitemap ที่มีวันที่ถูกต้อง หรือ structured data ที่ตรงกับเนื้อหาจริง

### Layout Metadata

สำหรับ metadata ที่ใช้ร่วมกันทุกหน้า:

```tsx
// app/layout.tsx
export const meta = {
  title: 'My Ruvyxa App',
  description: 'A production-ready application.',
}
```

## CSS & Styling

### Global CSS

Import ใน layout หรือ page files:

```tsx
import './globals.css'
```

### CSS Entries (สำหรับไฟล์ที่ไม่ได้ถูก import โดย application code)

```ts
// ruvyxa.config.ts
export default config({
  css: {
    entries: ['styles/theme.css'],
  },
})
```

### CSS-in-JS

React `style` objects และ `<style>` elements ทำงานได้ตามปกติ:

```tsx
<div style={{ color: 'red', fontSize: '1.2rem' }}>Styled text</div>
```

```tsx
<style>{`
  .custom { color: blue; }
`}</style>
<div className="custom">Blue text</div>
```

Libraries ที่ต้องการ compile-time transforms ควรเชื่อมต่อผ่าน transform plugin
