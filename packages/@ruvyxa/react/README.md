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

`Image` (also exported as `Picture`) emits AVIF/WebP sources backed by Ruvyxa build sidecars, keeps
the original as a fallback, requires intrinsic dimensions, and applies sensible loading defaults.

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
