# Markdown, MDX, Images & Metadata

## Markdown and MDX Pages

`page.md` and `page.mdx` are first-class route files. They support frontmatter, Markdown, MDX/JSX,
and the same dev/prod pipeline as TSX pages:

```mdx
---
title: Welcome
description: A page written in MDX.
---

# {frontmatter.title}

This page can contain **Markdown** and <strong>JSX</strong>.
```

Supported features:

- **Frontmatter** — access via the `frontmatter` object
- **Markdown** — GFM (GitHub Flavored Markdown)
- **JSX** — embed React components (`.mdx` only)
- **Expressions** — `{variable}` and `{expression}`
- **Heading exports** — headings are exported for table-of-contents generation
- **SSG** — pre-rendered at build time

## Images

Put static assets in `public/` and reference them from `/`:

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

`Image` converts local PNG/JPEG assets to WebP during a production build when image optimisation is
enabled.

| Config           | Default | Description                         |
| ---------------- | ------- | ----------------------------------- |
| `image.optimize` | `true`  | Enable / disable image optimization |
| `image.quality`  | `82`    | WebP quality (1–100)                |
| `image.lossless` | `false` | Lossless mode                       |
| `image.workers`  | `0`     | Thread count (0 = auto = CPU count) |

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

Remote URLs are **not** transformed — only local assets under `public/`.

### Image Best Practices

- Supply intrinsic `width` and `height` to prevent layout shift (CLS).
- Use the `fill` prop when the image must fill its container.
- Use `priority` for LCP (Largest Contentful Paint) images.
- Use the `<Image>` component instead of `<img>` for automatic optimization.

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

For metadata shared across all pages:

```tsx
// app/layout.tsx
export const meta = {
  title: 'My Ruvyxa App',
  description: 'A production-ready application.',
}
```

## CSS & Styling

### Global CSS

Import in layout or page files:

```tsx
import './globals.css'
```

### CSS Entries (for files not imported by application code)

```ts
// ruvyxa.config.ts
export default config({
  css: {
    entries: ['styles/theme.css'],
  },
})
```

### CSS-in-JS

React `style` objects and `<style>` elements work as expected:

```tsx
<div style={{ color: 'red', fontSize: '1.2rem' }}>Styled text</div>
```

```tsx
<style>{`
  .custom { color: blue; }
`}</style>
<div className="custom">Blue text</div>
```

Libraries that require compile-time transforms should be wired through a transform plugin.
