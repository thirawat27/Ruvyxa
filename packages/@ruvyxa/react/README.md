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

## SEO metadata

```tsx
import { Seo } from '@ruvyxa/react'

;<Seo
  title="Product"
  description="Product description"
  canonical="https://example.com/product"
  image="https://example.com/product-card.png"
/>
```

`Seo` emits React 19 document metadata for canonical URLs, robots, Open Graph, Twitter Cards, and
optional escaped JSON-LD.

## When to Use Directly

Use this package for React-specific integration work, framework experiments, or future
adapter/runtime composition. For ordinary apps, import public APIs from `ruvyxa/config` and
`ruvyxa/server`.
