# Markdown, MDX, Images & Metadata

## Markdown / MDX

`page.md` และ `page.mdx` เป็น first-class route files:

```mdx
---
title: Welcome
---

# {frontmatter.title}

Markdown และ <strong>JSX</strong> ในไฟล์เดียวกัน
```

## Image

```tsx
import { Image } from '@ruvyxa/react'

;<Image src="/hero.png" alt="Hero" width={1600} height={900} priority />
```

PNG/JPEG → WebP อัตโนมัติใน production build

## SEO

```tsx
import { Seo } from '@ruvyxa/react'

;<Seo title="Home" description="Welcome" canonical="https://example.com" />
```

ดูเพิ่มเติม: [Configuration](configuration.md) สำหรับ `css.entries` และ `image.*`
