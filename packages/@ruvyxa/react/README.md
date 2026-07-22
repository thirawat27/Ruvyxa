# @ruvyxa/react

React integration package for Ruvyxa apps.

## Install

```bash
npm install @ruvyxa/react react react-dom
```

React and ReactDOM are peer dependencies. Most app users do not import this package directly; the
main `ruvyxa` runtime uses React SSR and route-level client bundling internally.

## Optimized images

```tsx
import { Image } from '@ruvyxa/react'

;<Image src="/hero.png" alt="Product overview" width={1600} height={900} priority />
```

`Image` rewrites local PNG/JPEG URLs to Ruvyxa's single build-time WebP output, requires intrinsic
dimensions unless `fill` is used, and applies sensible loading defaults. Remote URLs are unchanged.
Use `Picture` with `sources` for browser-native art direction, or a per-image `loader` to send an
image URL to an external CDN—neither option adds an image transformation endpoint to Ruvyxa.

## SEO, GEO, and AEO primitives

```tsx
import { Answer, Seo } from '@ruvyxa/react'

export default function Guide() {
  return (
    <>
      <Seo
        title="Rendering guide"
        description="How Ruvyxa renders pages."
        canonical="https://example.com/guides/rendering"
        image="https://example.com/rendering.png"
        type="article"
        article={{
          type: 'BlogPosting',
          publishedAt: '2026-07-22',
          updatedAt: '2026-07-23T10:30:00+07:00',
          authors: [{ name: 'Ada', url: 'https://example.com/authors/ada' }],
          tags: ['SSR', 'React'],
        }}
        breadcrumbs={[
          { name: 'Home', url: 'https://example.com/' },
          { name: 'Guides', url: 'https://example.com/guides' },
          { name: 'Rendering', url: 'https://example.com/guides/rendering' },
        ]}
      />
      <Answer
        question="Does Ruvyxa render on the server?"
        answer="Yes. Pages render on the server by default."
        sources={[{ name: 'Rendering guide', url: '/docs/rendering' }]}
      />
    </>
  )
}
```

`Seo` emits React 19 document metadata for canonical URLs, robots, Open Graph, Twitter Cards, and
optional escaped JSON-LD. Its typed `article` and `breadcrumbs` inputs derive Article and
BreadcrumbList JSON-LD from explicit page facts. `Answer` renders the answer and citations visibly
with Schema.org Question/Answer microdata; it does not claim FAQ or Q&A rich-result eligibility.

## When to Use Directly

Use this package for React-specific integration work, framework experiments, or future
adapter/runtime composition. For ordinary apps, import public APIs from `ruvyxa/config` and
`ruvyxa/server`.
