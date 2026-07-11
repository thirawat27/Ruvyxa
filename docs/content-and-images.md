# Markdown, MDX, Images, and SEO

Ruvyxa treats Markdown and MDX as first-class page modules. Content files use the same route graph,
boundary validation, compile cache, client bundling, SSG, and dev/prod runtime as TSX pages.

## Markdown and MDX routes

Name a route file `page.md` or `page.mdx`:

```text
app/
├── page.tsx
└── docs/
    └── page.mdx   # /docs
```

Markdown supports CommonMark plus GFM tables, task lists, strikethrough, autolinks, and footnotes.
MDX additionally supports ESM imports/exports, JSX components, and JavaScript expressions:

```mdx
---
title: Ruvyxa content
description: Markdown and components in one route
tags: [docs, framework]
---

import Callout from '../../components/Callout'

# {frontmatter.title}

<Callout tone="info">This is **MDX**.</Callout>
```

Every content module exports:

- `frontmatter` — flat YAML-style keys with strings, numbers, booleans, nulls, and inline arrays
- `meta` — an alias of `frontmatter`, unless the MDX file exports its own `meta`
- `headings` — `{ depth, slug, text }[]` for a table of contents
- `contentFormat` — `"md"` or `"mdx"`
- a default React page component

Relative `.md` and `.mdx` imports are also resolved by both native and Node compilers.

## Image optimization

Production builds keep every original PNG/JPEG and generate AVIF/WebP sidecars next to it. The
server negotiates the best format from the browser's `Accept` header and sends `Vary: Accept`.
Static hosts can use the sidecars through the React `Image`/`Picture` component.

```ts
export default defineConfig({
  images: {
    optimize: true,
    formats: ['avif', 'webp'],
    quality: 80,
  },
})
```

```tsx
import { Image } from '@ruvyxa/react'

;<Image
  src="/hero.jpg"
  alt="Ruvyxa dashboard"
  width={1600}
  height={900}
  sizes="(max-width: 768px) 100vw, 1200px"
  priority
/>
```

`width` and `height` are required to prevent cumulative layout shift. Non-priority images use
`loading="lazy"` and `decoding="async"`; priority images use eager loading and high fetch priority.
The build writes `.ruvyxa/assets/.ruvyxa-images.json` with source dimensions, variant URLs, byte
sizes, and counts. Invalid image files remain available as originals instead of failing the build.

## SEO metadata

React 19 hoists metadata rendered by `Seo` into the document head:

```tsx
import { Seo } from '@ruvyxa/react'

;<Seo
  title="Ruvyxa documentation"
  description="Build fast server-rendered React applications."
  canonical="https://example.com/docs"
  image="https://example.com/docs-card.png"
  imageAlt="Ruvyxa documentation"
  siteName="Ruvyxa"
  jsonLd={{
    '@context': 'https://schema.org',
    '@type': 'TechArticle',
    headline: 'Ruvyxa documentation',
  }}
/>
```

The component emits title, description, canonical, robots, Open Graph, Twitter Card, and escaped
JSON-LD tags. A Lighthouse SEO score still depends on the application's content, valid links, mobile
usability, status codes, crawl policy, and deployment—not only the framework—but these primitives
cover the framework-controlled metadata and image requirements.
