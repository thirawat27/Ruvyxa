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

Production builds convert every valid PNG/JPEG in `public/` into one WebP file. For example,
`public/hero.jpg` becomes `.ruvyxa/assets/hero.webp`; the copied JPEG is removed only after the WebP
has been written successfully. This keeps static output small and request-time serving free of image
transforms and content negotiation.

```ts
export default defineConfig({
  images: {
    optimize: true,
    quality: 82,
    lossless: false,
    parallelism: 0,
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

`width` and `height` are required to prevent cumulative layout shift unless `fill` is used. `fill`
uses an absolutely positioned image that fills its positioned parent, without adding a framework
wrapper. Non-priority images use `loading="lazy"` and `decoding="async"`; priority images use eager
loading and high fetch priority. The component rewrites only local PNG/JPEG URLs to `.webp`. Remote
URLs are left unchanged; use `unoptimized` to also preserve a local source URL. Development resolves
the WebP URL back to the untouched source, so the same component works before and after a build.

Use browser-native `srcSet` and `sizes` when you publish deliberate image sizes yourself. Local
PNG/JPEG URLs inside `srcSet` are rewritten to their WebP counterparts; Ruvyxa does not generate
width variants automatically, keeping builds and static output bounded.

For art direction, `Picture` renders native `<picture>` and `<source>` elements. Each local source
is rewritten to its one WebP build output:

```tsx
import { Picture } from '@ruvyxa/react'

;<Picture
  src="/hero-desktop.jpg"
  alt="Ruvyxa dashboard"
  width={1600}
  height={900}
  sources={[
    { media: '(max-width: 768px)', srcSet: '/hero-mobile.png' },
    { media: '(min-width: 769px)', srcSet: '/hero-desktop.jpg' },
  ]}
/>
```

For an external CDN, pass a `loader`. It produces the final URL at render time but does not make a
request through the Ruvyxa server or enable runtime image transformation:

```tsx
;<Image
  src="https://images.example.com/hero.jpg"
  alt="Ruvyxa dashboard"
  width={1600}
  height={900}
  quality={75}
  loader={({ src, width, quality }) =>
    `https://cdn.example.com/image?src=${encodeURIComponent(src)}&w=${width}&q=${quality}`
  }
/>
```

Plain HTML, CSS, and Markdown image references are not component-transformed. Point them at the
output name directly (for example, `/hero.webp`); the development server maps that URL to
`public/hero.jpg` or `public/hero.png` when there is exactly one matching source.

`quality` accepts 1–100 for lossy output. Set `lossless: true` for pixel-exact output and set
`parallelism` to a positive worker limit, or leave it at `0` to use available CPUs. Encoded bytes
are cached by source content and settings under the configured build cache, making unchanged
rebuilds copy/link-only. The build writes `.ruvyxa/assets/.ruvyxa-images.json` with source/output
URLs, dimensions, byte sizes, and cache-hit status. Invalid image files remain unchanged. Two source
files with the same stem, such as `hero.png` and `hero.jpg`, fail with a rename hint instead of
overwriting one another.

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
