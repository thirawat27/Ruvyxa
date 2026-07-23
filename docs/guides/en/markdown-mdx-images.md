# Markdown, MDX, Images & Metadata

> 🟢 **Beginner friendly** · ⏱️ ~7 min read
>
> **You'll learn:** write pages in Markdown/MDX, get automatic WebP image optimization, and set SEO
> metadata per page.

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

- **YAML frontmatter** — nested objects, arrays, quoted values, and block scalars are available via
  the `frontmatter` object; `meta` aliases the same object unless the file exports its own value
- **GFM** — tables with alignment, task lists, strikethrough, autolink literals, references, and
  footnotes work in both Markdown and MDX
- **JSX** — embed React components, member components such as `<Card.Header>`, fragments, and prop
  spreads (`.mdx` only)
- **Expressions and ESM** — `{variable}`, `{expression}`, multiline `import`, and multiline `export`
  blocks are parsed as JavaScript/TypeScript rather than line-based text
- **Heading exports** — headings are exported for table-of-contents generation; duplicate headings
  receive stable `-1`, `-2`, and later suffixes that match their rendered IDs
- **Component overrides** — the generated MDX page accepts a `components` prop for replacing
  Markdown elements such as `h1`, `a`, `table`, and `code`
- **SSG** — pre-rendered at build time

```mdx
---
title: Content guide
author:
  name: Ada
tags: [mdx, gfm]
summary: |
  Nested YAML and multiline values are preserved.
---

import { Callout } from './Callout'

export const status = {
  stable: true,
}

## {frontmatter.title}

<Callout {...status}>Ready</Callout>
```

Malformed YAML, unclosed frontmatter, invalid MDX ESM, and invalid generated JavaScript stop the
build with a content/compiler diagnostic. Markdown files can contain raw HTML and therefore should
be author-controlled; sanitize untrusted external content before it enters the build.

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

`article` and `breadcrumbs` generate escaped Article and BreadcrumbList JSON-LD from explicit page
facts. Use `jsonLd` for other applicable schema types. Do not describe content that a reader cannot
see on the page.

### Answer-ready content

Use `Answer` for a concise answer that remains visible, accessible, and citeable:

```tsx
import { Answer } from '@ruvyxa/react'

export default function RenderingAnswer() {
  return (
    <Answer
      question="Does Ruvyxa render on the server?"
      answer="Yes. Pages render on the server by default."
      sources={[{ name: 'Rendering guide', url: '/docs/rendering' }]}
    />
  )
}
```

`Answer` emits Schema.org Question/Answer microdata around the same text readers see. It does not
generate `FAQPage` or `QAPage`: those formats have narrower eligibility rules and must be selected
only when the whole page genuinely matches them.

For Markdown/MDX collections, pair this with `contentEngine()`. Explicit `answers` frontmatter is
included in `/content.json` and the experimental `/llms.txt` discovery index:

```mdx
---
title: Rendering guide
description: How Ruvyxa renders pages.
answers:
  - question: Does Ruvyxa render on the server?
    answer: Yes. Pages render on the server by default.
    sources:
      - name: Rendering guide
        url: /docs/rendering
---

import { Answer } from '@ruvyxa/react'

# {frontmatter.title}

<Answer {...frontmatter.answers[0]} />
```

This keeps the visible answer and machine-readable content graph on one author-controlled source.
`llms.txt` is an experimental convenience, not a ranking signal or replacement for crawlable HTML,
canonical URLs, sitemap freshness, and accurate structured data.

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
