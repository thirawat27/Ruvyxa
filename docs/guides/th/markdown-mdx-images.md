# Markdown, MDX, Images & Metadata

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

- **Frontmatter** — เข้าถึงผ่าน `frontmatter` object
- **Markdown** — GFM (GitHub Flavored Markdown)
- **JSX** — ฝัง React components (`.mdx` เท่านั้น)
- **Expressions** — `{variable}` และ `{expression}`
- **Heading exports** — headings ถูก export สำหรับสร้าง table-of-contents
- **SSG** — pre-rendered ณ build time

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

| Config           | Default | คำอธิบาย                            |
| ---------------- | ------- | ----------------------------------- |
| `image.optimize` | `true`  | เปิด / ปิด image optimization       |
| `image.quality`  | `82`    | คุณภาพ WebP (1–100)                 |
| `image.lossless` | `false` | โหมด lossless                       |
| `image.workers`  | `0`     | จำนวน thread (0 = auto = CPU count) |

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
      robots="index, follow"
      ogImage="/og-image.png"
      ogType="website"
      twitterCard="summary_large_image"
      jsonLd={{
        '@context': 'https://schema.org',
        '@type': 'WebSite',
        name: 'My App',
      }}
    />
  )
}
```

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
