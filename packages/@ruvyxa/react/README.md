<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/react</h1>

<p align="center">
  React integration package for Ruvyxa apps.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/react"><img src="https://img.shields.io/npm/v/@ruvyxa/react?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/react"><img src="https://img.shields.io/node/v/@ruvyxa/react?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

```bash
npm install @ruvyxa/react react react-dom
```

Most apps never run that command: `ruvyxa` depends on this package, so installing the framework
brings it along. Install it directly when the package manager does not hoist a transitive dependency
to the project root, which is pnpm's and Yarn's default.

React and ReactDOM are peer dependencies here and on `ruvyxa`, declared at the same range in both so
one app cannot end up satisfying two.

## Optimized images

```tsx
import { Image } from '@ruvyxa/react'

;<Image src="/hero.png" alt="Product overview" width={1600} height={900} priority />
```

`Image` rewrites local PNG/JPEG URLs to Ruvyxa's build-time WebP output, requires intrinsic
dimensions unless `fill` is used, and applies sensible loading defaults. With
`image.onDemand: true`, add `dynamic` to request bounded same-origin runtime resizing through
`/__ruvyxa/image`; remote URLs are deliberately unchanged to prevent the endpoint becoming an open
proxy. Use `Picture` with `sources` for browser-native art direction, or a per-image `loader` for an
external CDN.

## View transitions

```tsx
<Link href="/products" viewTransition>
  Products
</Link>
```

`Link` and `router.push()` support `viewTransition: true`. Ruvyxa uses the stable browser View
Transitions API when present, respects `prefers-reduced-motion`, and falls back to ordinary client
navigation. It does not require React Canary's experimental `<ViewTransition>` component.

Ruvyxa targets React 19 and exercises the stable `useActionState` and `useFormStatus` APIs in its
compatibility suite.

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
