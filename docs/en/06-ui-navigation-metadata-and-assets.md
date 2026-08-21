# UI, navigation, metadata, and assets

> **Tutorial goal:** make your app navigable, accessible, and ready to present real content. **Start
> from:** a working page or API flow from [Data, actions, and API routes](05-data-actions-api.md).
> **Checkpoint:** navigate with a Link, inspect page metadata, and confirm one asset loads.

`@ruvyxa/react` exports framework-aware React helpers. They are optional; normal React components
continue to work.

## Navigation and route state

Use `Link` for application navigation and `useRouter()` for imperative navigation. `usePathname()`,
`useParams()`, `useSearchParams()`, `useSelectedRoute()`, and `useRouteContext()` expose the current
client route state.

```tsx
'use client'
import { Link, useRouter, useSearchParams } from '@ruvyxa/react'

export function SearchControls() {
  const router = useRouter()
  const query = useSearchParams().get('q') ?? ''
  return (
    <>
      <Link href="/about">About</Link>
      <button onClick={() => router.push(`/search?q=${query}`)}>Search</button>
    </>
  )
}
```

`useSearchParams()` returns an empty set during SSR when the query is unavailable; do not use it for
markup that must be identical in server HTML. `useRouter().pending` tracks a route-bundle
navigation.

### Choose prefetch deliberately

`Link` renders a normal anchor first, then enhances eligible same-window clicks. It preserves new
tab, modified-click, download, and non-`_self` link behavior. Its `prefetch` default is `'hover'`.
Choose the mode by the likelihood and cost of the next navigation rather than enabling eager
prefetching everywhere.

```tsx
import { Link } from '@ruvyxa/react'

export function ProductLinks() {
  return (
    <nav>
      {/* The default: warm only when a visitor shows intent. */}
      <Link href="/products/notebook">Notebook</Link>

      {/* Good for a prominent next step likely to enter the viewport. */}
      <Link href="/checkout" prefetch="viewport">
        Checkout
      </Link>

      {/* Avoid warming a large, low-probability destination. */}
      <Link href="/reports" prefetch="none">
        Reports
      </Link>

      {/* Replace a transient URL; keep scroll position if the view needs it. */}
      <Link href="/search?q=paper" replace scroll={false}>
        Apply filter
      </Link>

      {/* Keep external destinations as ordinary anchors. */}
      <a href="https://status.example.com" target="_blank" rel="noreferrer">
        Status
      </a>
    </nav>
  )
}
```

Use `prefetch="viewport"` sparingly on above-the-fold or clearly next-step links; it loads a route
when its link becomes visible. Use `'none'` (or `false`) for low-intent destinations. `replace`
replaces the current history entry, `scroll` defaults to `true`, and `viewTransition` opts into the
browser View Transitions API when available.

## Metadata and error UI

Use a route `meta` export for hierarchy-aware metadata ([Routing](04-routing-rendering.md)), or use
`<Seo>` inside a component for per-render tags. `<Seo>` can emit Open Graph, X card, Article
JSON-LD, breadcrumb JSON-LD, and custom JSON-LD. The X card shape is set with `card`; the former
`twitterCard` prop has been removed.

```tsx
import { Seo, RuvyxaErrorBoundary } from '@ruvyxa/react'

export default function Product() {
  return (
    <RuvyxaErrorBoundary
      fallback={({ error, resetError }) => (
        <button onClick={resetError}>Retry: {error.message}</button>
      )}
    >
      <Seo
        title="Product"
        description="A documented product"
        canonical="https://example.test/product"
      />
      <main>...</main>
    </RuvyxaErrorBoundary>
  )
}
```

`RuvyxaErrorBoundary` catches descendant React render errors, calls optional `onError`, and passes
`resetError` to its fallback. It does not replace route-level `error.tsx` boundaries.

## Images, CSS, and static files

`Image` accepts React image props plus Ruvyxa options. By default, a production build replaces each
local public PNG/JPEG with exactly one WebP and publishes neither the original nor responsive
variants. Use `<Image>` or reference the generated `.webp` URL directly. `image.keepOriginal: true`
retains source files for raw `<img>` compatibility, while `image.variantWidths` explicitly opts into
prebuilt variants that an author-provided `srcSet` can reference. `image.onDemand` enables automatic
responsive URLs through same-origin runtime transformations at `/__ruvyxa/image` and has a default
maximum width of 3840 when configured as an object.

<!-- prettier-ignore -->
```tsx
import { Image } from '@ruvyxa/react'
export function Hero() {
  return (
    <Image
      src="/hero.jpg"
      alt="Team at work"
      width={1200}
      height={630}
      priority
    />
  )
}
```

Files in `public/` are served with byte-range support, so `<video>` and `<audio>` elements seek
without re-downloading and interrupted downloads resume. `ruvyxa start` and a standalone/node
deployment answer ranges identically; a single `Range: bytes=…` returns `206` with `Content-Range`,
a range past the end of the file returns `416`, and a multi-range request is answered with the whole
file. Assets over 8 MiB stream from disk instead of being buffered, and a ranged request for one
reads only the bytes it asked for.

Imported project CSS may live outside `app/`. To include global styles not imported by a module,
list project-relative files/directories in `css.entries`. The runtime recognizes Sass as a package
dependency; use styles that your build can resolve and run `npm run check` after changing
boundaries.

### PostCSS and Tailwind CSS

If the project root has a PostCSS configuration, Ruvyxa runs your plugin chain over every collected
global stylesheet — in `ruvyxa dev` and `ruvyxa build` alike, on the same code path, so the two
produce the same CSS.

Recognized filenames, in this order: `postcss.config.mjs`, `postcss.config.js`,
`postcss.config.cjs`, `postcss.config.ts`, `postcss.config.mts`, `postcss.config.cts`,
`postcss.config.json`, `.postcssrc.mjs`, `.postcssrc.js`, `.postcssrc.cjs`, `.postcssrc.json`,
`.postcssrc`.

Ruvyxa names no plugin of its own. Whatever the config declares is what runs, resolved from your
`node_modules`. Tailwind CSS v4 needs nothing framework-specific:

```js
// postcss.config.mjs
export default { plugins: { '@tailwindcss/postcss': {} } }
```

```css
/* app/globals.css */
@import 'tailwindcss';
```

```bash
npm install -D postcss tailwindcss @tailwindcss/postcss
```

Details worth knowing:

- **Plugins run per stylesheet entry, after local `@import`s are inlined.** A partial pulled in with
  `@import "./theme.css"` reaches the plugin chain as part of its entry.
- **The config declares the shape you already know.** A plugin array, a `{ name: options }` map, or
  a function of `{ mode }` — `mode` is `production` during `ruvyxa build` and `development` during
  `ruvyxa dev`.
- **Files your plugins read become watch inputs.** Tailwind reports the templates it scanned for
  class names, so editing a component in dev regenerates the stylesheet.
- **A plugin failure fails the build.** Ruvyxa does not fall back to untransformed CSS: an
  unresolved `@import "tailwindcss"` reaching a browser renders the page with browser defaults,
  which looks like a styling bug rather than a build failure. See `RUV1405` and `RUV1406` in
  [Troubleshooting](16-troubleshooting-upgrades.md).
- **A project with no PostCSS config is unaffected.** The CSS pipeline behaves exactly as it did
  before. A stylesheet that imports `tailwindcss` without a PostCSS config still falls back to
  `@tailwindcss/cli` when that is installed.

Do not add a separate Tailwind CLI script alongside a PostCSS config. Two build pipelines over one
stylesheet disagree about live reload, asset manifests, and where errors are reported.

**Previous:** [Data, actions, and API routes](05-data-actions-api.md) · **Next:**
[Configuration and environment](07-configuration.md)

## Typed routes

With `typedRoutes: true` in `ruvyxa.config.ts`, Ruvyxa writes `.ruvyxa/types/routes.d.ts` from the
discovered route graph. `<Link href>`, `useRouter().push`, `useRouter().replace`, and
`useRouter().prefetch` are then checked against the routes that exist, and a mistyped path is a
compile error rather than a 404 someone finds later.

The file is rewritten by `ruvyxa dev` whenever route discovery re-runs, and once by `ruvyxa build`
and `ruvyxa check`. It is generated output: do not edit it, and do not commit it.

TypeScript only reads the file if your `tsconfig.json` includes it:

```json
{
  "include": ["app", "ruvyxa.config.ts", ".ruvyxa/types/**/*.d.ts"]
}
```

Projects scaffolded by `create-ruvyxa` have both the config flag and this `include` already.
`ruvyxa check` reports `RUV1502` if the flag is on and the `include` is missing, because a generated
file nothing reads looks exactly like a working feature.

Without the flag — and in every project that predates it — `href` stays `string` and nothing about
type-checking changes.

### What is and is not caught

A dynamic segment expands to `${string}`, which is as precise as a TypeScript template literal type
can be: there is no way to say "any string without a slash". So:

```tsx
<Link href="/blog/hello">Post</Link>        // ok
<Link href="/blog/hello?draft=1">Post</Link> // ok — query and hash are allowed
<Link href="https://example.com">Docs</Link> // ok — external URLs stay valid
<Link href="/abuot">About</Link>             // error: no such route
<Link href="/blogs/hello">Post</Link>        // error: the static prefix is wrong
<Link href="/blog/a/b">Post</Link>           // accepted, though `[slug]` is one segment
```

The last line is the known limitation, and it is the same one Next.js has. What the check reliably
catches is the common mistake: a wrong static part of a path.

### URLs built at runtime

A path assembled from data is a `string`, and `string` is not assignable to a union of literals.
Wrap it in `route()`:

```tsx
import { Link, route } from '@ruvyxa/react'

;<Link href={route(record.canonicalUrl)}>Open</Link>
```

`route()` asserts rather than validates. Prefer a template built from a literal pattern —
`` `/blog/${slug}` `` type-checks on its own — and keep `route()` for values you genuinely cannot
know at compile time.

## Third-party scripts

`<Script>` loads an external or inline script without putting it on the critical path, and fetches
it once per document however many routes render it.

```tsx
import { Script } from '@ruvyxa/react'

<Script src="https://plausible.io/js/script.js" strategy="lazyOnload" />
<Script id="consent" strategy="beforeInteractive">{`window.__consent = true`}</Script>
```

| `strategy`                   | When it runs                                         | Use for                                  |
| ---------------------------- | ---------------------------------------------------- | ---------------------------------------- |
| `beforeInteractive`          | Rendered into the server HTML, runs before hydration | Consent gating, A/B bucketing, polyfills |
| `afterInteractive` (default) | Appended to `<body>` after hydration                 | Analytics, tag managers                  |
| `lazyOnload`                 | Appended when the browser is idle after `load`       | Chat widgets, support popovers           |

Deduplication is keyed by `id`, falling back to `src`. That key survives client-side navigation, so
navigating away from a route and back does not run its analytics tag a second time. A script that
fails to load releases its key, so a later render can retry it.

`beforeInteractive` is the only strategy that works on a page with `export const hydrate = false`:
the others are appended by an effect, and such a page ships no client runtime for an effect to run
in. Inline scripts need an `id` — they have no `src` to be identified by.
