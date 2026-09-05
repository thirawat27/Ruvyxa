# Configuration and environment

> **Tutorial goal:** turn a development app into a deliberately configured, secret-safe application.
> **Start from:** the UI and assets in
> [UI, navigation, metadata, and assets](06-ui-navigation-metadata-and-assets.md). **Checkpoint:**
> commit a harmless environment example, keep secrets server-only, and run the app check.

`ruvyxa.config.ts` is evaluated by the configuration renderer then validated. Use `config()` from
`ruvyxa/config` for typed authoring. Configuration names below come from `RuvyxaConfig` and its
nested source types.

## Primary options

| Key                                                                                             | Type / default                                          | Effect                                                                                |
| ----------------------------------------------------------------------------------------------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `appDir`, `outDir`                                                                              | strings                                                 | App source and generated output locations.                                            |
| `runtime`                                                                                       | `node \| bun \| deno \| edge \| static`, default `node` | Runtime/target policy.                                                                |
| `typedRoutes`                                                                                   | boolean, default `false`                                | Generate `.ruvyxa/types/routes.d.ts` so `<Link href>` is checked against real routes. |
| `server.host`, `server.port`                                                                    | string, number                                          | Listening address. See [Listening address](#listening-address).                       |
| `build.minify`, `map`, `treeShake`, `manifest`, `warm`, `prerenderCache`                        | booleans; cache defaults true                           | Compiler/build artifact behavior.                                                     |
| `build.split`                                                                                   | `single \| route \| manual`                             | Bundle splitting policy.                                                              |
| `build.workers`                                                                                 | number                                                  | Build parallelism. See note below.                                                    |
| `render.strategy`, `render.revalidate`                                                          | strategy, seconds                                       | Default page rendering policy.                                                        |
| `cache.routes`, `cache.css`, `cache.dir`, `cache.handler`, `cache.maxEntries`, `cache.maxBytes` | booleans/string                                         | Route/CSS/cache-directory settings.                                                   |

## Complete option map

| Group         | Keys                                                                                                                             | Operational decision                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Root          | `appDir`, `outDir`, `runtime`, `typedRoutes`, `reactCompiler`                                                                    | Keep defaults unless the source/output layout or target requires a change. `runtime` is `node`, `bun`, `deno`, `edge`, or `static`; the CLI target can override it. `typedRoutes` also requires `.ruvyxa/types/**/*.d.ts` in the tsconfig `include`. `reactCompiler` is off by default — see [The React Compiler](#the-react-compiler). `react` and `typescript` are accepted by the validator but read by nothing — set strictness in your own `tsconfig.json`.                                                                           |
| CSS and debug | `css.entries`, `debug.overlay`, `debug.traces`                                                                                   | `entries` is for project-relative global styles not imported by a module. Debug flags change development diagnostics, not production access control.                                                                                                                                                                                                                                                                                                                                                                                       |
| Build         | `minify`, `map`, `treeShake`, `split`, `workers`, `jsx`, `target`, `manifest`, `warm`, `prerenderCache`                          | `split` is `single`, `route`, or `manual`; `jsx` is `classic` or `automatic`; `target` is `es2015` through `es2026` or `esnext` (default), and both compilers apply it. A target below the syntax your code uses may need runtime helper functions — Ruvyxa ships no helper runtime, so a module that would need one fails the build by name instead of emitting an import nothing can resolve. Ordinary application code compiles helper-free at `es2022` and above. Use source maps deliberately because they can expose source content. |
| Rendering     | `render.strategy`, `render.revalidate`                                                                                           | Strategy is `ssr`, `ssg`, `isr`, `csr`, or `ppr`. The default strategy is SSR and default revalidation is 60 seconds.                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Image         | `optimize`, `quality`, `lossless`, `keepOriginal`, `variantWidths`, `workers`, `effort`, `onDemand.enabled`, `onDemand.maxWidth` | Defaults are optimize true, quality 82, lossless false, keep-original false, no prebuilt variants, workers 0 (available CPU count), and effort 4. This produces one WebP per source. Object-form on-demand mode defaults enabled with max width 3840.                                                                                                                                                                                                                                                                                      |
| i18n          | `locales`, `defaultLocale`, `localeParam`, `detectLocale`, `cookie`                                                              | `locales` and `defaultLocale` are required when i18n is set. Default param is `lang`, detection true, cookie `RUVYXA_LOCALE`.                                                                                                                                                                                                                                                                                                                                                                                                              |
| Site          | `site.url`, `site.sitemap`, `site.robots`                                                                                        | Sitemap may set `exclude`, `additionalPaths`, `defaults`, and enriched `entries`; robots may set rules, sitemap URLs, and host.                                                                                                                                                                                                                                                                                                                                                                                                            |
| Middleware    | `builtin.cors`, `builtin.timing`, `builtin.log`, `builtin.rate`, `builtin.headers`, `workers`, `timeoutMs`                       | CORS has origins/methods/headers/credentials/maxAge. Built-in rate needs `max`, `window`, optional `key`. Plugin workers are 1–8; timeout is 30,000 ms by default and at most 300,000.                                                                                                                                                                                                                                                                                                                                                     |
| Integration   | `adapter`, `adapterOptions`, `plugins`                                                                                           | `adapter` holds a constructed adapter; `adapterOptions` configures one selected by name instead (see [Configuring an adapter selected by name](#configuring-an-adapter-selected-by-name)). Setting both is an error. `plugins` is an array of `RuvyxaPlugin` values.                                                                                                                                                                                                                                                                       |

## Runtime selection

For JavaScript processes, use `node`, `bun`, or `deno`. `--runtime` has the highest precedence, then
`RUVYXA_RUNTIME`, then `runtime` in `ruvyxa.config.ts`. If the project has no explicit runtime,
launching through `bun run` or `deno task` is a hint; otherwise detection prefers Node, then Bun,
then Deno. `edge` and `static` are build targets, not JavaScript worker hosts.

Deno executes trusted local project configuration and plugins with the permissions they require
(`deno run -A --no-prompt --node-modules-dir=manual`). Do not select it for untrusted project code.

## Listening address

`--host` and `--port` have the highest precedence, then the `HOST` and `PORT` environment variables,
then `server.host` and `server.port` in `ruvyxa.config.ts`, then the command's own default.

The environment beats the config file deliberately: a managed platform injects `PORT` and expects
the process to use it, and a `ruvyxa.config.ts` committed to the repository cannot know the number.
A `PORT` that is not a number between 0 and 65535 fails the command rather than falling back, so a
misconfigured deployment reports the cause instead of a failing health check.

| Command                          | Default host | Default port |
| -------------------------------- | ------------ | ------------ |
| `ruvyxa dev`                     | `localhost`  | `3000`       |
| `ruvyxa start`, `ruvyxa preview` | `0.0.0.0`    | `3000`       |

`start` and `preview` bind every interface because a container routes to the container's address
rather than to its loopback: a production server bound to `localhost` answers nothing from outside.
This matches the standalone server that `ruvyxa build` generates, which has always read `PORT` and
`HOST` the same way. Pass `--host localhost` to keep a local production run off the network.

## Configuring an adapter selected by name

An adapter can reach a build in two ways, and each takes its options differently.

Constructed in the config — the options go to the factory:

```ts
import { config } from 'ruvyxa/config'
import { render } from '@ruvyxa/adapter-render'

export default config({ adapter: render({ serviceName: 'checkout-api' }) })
```

Selected by name — `ruvyxa build --adapter render`, `RUVYXA_ADAPTER=render`, or platform detection
from the hosting environment. There is no factory call to pass options to, so `adapterOptions` is
that call's argument:

```ts
import { config } from 'ruvyxa/config'

export default config({ adapterOptions: { serviceName: 'checkout-api' } })
```

The second form is what keeps a zero-config deploy configurable: the project names no adapter, the
platform selects one, and the options still apply. The adapter validates them itself, so a value it
refuses fails the build with that adapter's own diagnostic.

Setting `adapter` and `adapterOptions` together is an error rather than a precedence rule. The
constructed adapter already holds its options and nothing would read the second set.

## Production configuration example

Start from this narrow configuration, then add only the features your application has tested. The
values are all supported option names; replace the example origin before release.

```ts
import { config } from 'ruvyxa/config'
import { requireEnv, securityHeaders } from 'ruvyxa/plugins'

export default config({
  site: {
    url: 'https://app.example.com',
    title: 'Example',
    description: 'Product notes and guides',
    language: 'en',
    sitemap: true,
    robots: true,
  },
  content: true,
  build: { minify: true, map: false, treeShake: true, split: 'route', prerenderCache: true },
  security: { actionLimit: 1_048_576, apiLimit: 10_485_760, sameOrigin: true, fetchMeta: true },
  plugins: [
    requireEnv(['DATABASE_URL', 'RUVYXA_AUTH_SECRET']),
    securityHeaders({ contentSecurityPolicy: { 'default-src': ["'self'"] } }),
  ],
})
```

`requireEnv` validates names at the end of the production build, so configure its required values in
the same build environment. It does not read a secret into browser code. A CSP commonly needs extra
sources for analytics, images, fonts, or APIs; test every route after tightening it.

```ts
import { config } from 'ruvyxa/config'

export default config({
  build: { minify: true, map: false, treeShake: true, split: 'route', prerenderCache: true },
  render: { strategy: 'ssr', revalidate: 60 },
  image: {
    optimize: true,
    quality: 82,
    variantWidths: [640, 1200],
    onDemand: { enabled: true, maxWidth: 1920 },
  },
  i18n: { locales: ['en', 'th'], defaultLocale: 'en' },
})
```

## The React Compiler

`reactCompiler: true` runs the stable React Compiler over your components before Ruvyxa's own Oxc
transform, so memoization is inferred instead of hand-written with `useMemo` and `useCallback`.

```ts
export default config({ reactCompiler: true })
```

It is **off by default** and deliberately has no options of its own. Two consequences are worth
knowing before you turn it on:

- It runs in the upstream default **inference mode**, targeting React 19 — the peer version Ruvyxa
  already requires. There is no per-file opt-in or opt-out setting here; the compiler's own
  directives are what a component uses to escape it.
- Babel configuration files are **ignored** (`babelrc: false`, `configFile: false`). That is
  intentional: a `.babelrc` in the project could otherwise make the server lane and the client lane
  compile the same component differently, which is the kind of divergence that only shows up as a
  hydration mismatch in production.

Compiled output is content-keyed like every other transform, so the setting participates in build
caching rather than defeating it. Turn it on, run `ruvyxa build`, and compare — it changes the
emitted JavaScript, not the semantics of correct code.

## Markdown and MDX compiler

`markdown` configures the shared `@mdx-js/mdx` pipeline used by development, SSR/SSG, adapters, and
native production client bundles. `gfm` defaults to `true`; `remarkPlugins`, `rehypePlugins`, and
`recmaPlugins` accept unified plugins or `[plugin, options]` tuples; `remarkRehypeOptions` forwards
localized footnote and bridge settings. See
[Routing and rendering](04-routing-rendering.md#markdown-mdx-and-shared-components) for a complete
example and the frontmatter/heading contracts.

## Security, middleware, site, and plugins

`security.actionLimit` defaults to 1,048,576 bytes; `security.apiLimit` defaults to 10,485,760
bytes; `security.pluginLimit` defaults to 33,554,432 and is capped at 268,435,456.
`security.actionRateLimit` defaults to 600 requests in 60 seconds. `trustedProxyIps` accepts exact
IPv4/IPv6 addresses or CIDR ranges; only configured non-loopback proxies may supply forwarded
client/protocol headers.

`middleware` contains built-ins (`cors`, `timing`, `log`, `rate`, `headers`) and TypeScript plugin
`build.workers` is best left unset. Unset, route bundling is sized to the machine: the smaller of
its core count (honouring `RAYON_NUM_THREADS`) and what free memory can hold. A pinned number caps a
large machine — a value of 4 uses four workers on a 16-core host — and the starter templates no
longer ship one. Setting it lowers the CPU budget only; the memory bound still applies, so a value
copied from another project cannot make a memory-limited CI container ask for more than it has.

`workers` (1–8) and `timeoutMs` (default 30,000, maximum 300,000). `site` configures build-time
`sitemap.xml` and `robots.txt`; an exact app route or same-named `public/` file suppresses the core
generator. `plugins` is the array of `RuvyxaPlugin` objects.

## Sitemap and robots entries

`site.sitemap: true` publishes every discovered route. The object form enriches that table rather
than replacing it, so a URL route discovery already found keeps its place and gains whatever the
entry adds.

| Key                            | Type                                                           | Purpose                                                                                      |
| ------------------------------ | -------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `sitemap.exclude`              | `string[]`                                                     | Exact paths or trailing-`*` prefixes to omit.                                                |
| `sitemap.additionalPaths`      | `string[]`                                                     | Concrete dynamic URLs discovery and prerendering cannot infer.                               |
| `sitemap.defaults`             | `{ lastModified?, changeFrequency?, priority? }`               | Inherited by every discovered and explicit entry.                                            |
| `sitemap.entries[].url`        | `string`                                                       | Required. A root-relative URL resolves against `site.url`.                                   |
| `sitemap.entries[]`            | `lastModified`, `changeFrequency`, `priority`                  | Per-entry overrides of the same three defaults.                                              |
| `sitemap.entries[].alternates` | `{ languages: Record<string, string> }`                        | `hreflang` alternates for one URL.                                                           |
| `sitemap.entries[].images`     | `string[]`                                                     | Image URLs for the image sitemap extension.                                                  |
| `sitemap.entries[].videos`     | `SiteSitemapVideo[]`                                           | Video sitemap entries; `title`, `thumbnail_loc`, and `description` are required on each one. |
| `robots.rules`                 | `SiteRobotsRule` or `SiteRobotsRule[]`                         | One group or several. Defaults to allowing every crawler.                                    |
| `robots.rules[]`               | `userAgent` (default `"*"`), `allow`, `disallow`, `crawlDelay` | String fields accept a value or an array.                                                    |
| `robots.sitemap`               | `string \| string[]`                                           | Absolute sitemap URLs. Defaults to the generated root sitemap.                               |
| `robots.host`                  | `string`                                                       | Preferred absolute origin, written as a `Host:` record.                                      |

`changeFrequency` is one of `always`, `hourly`, `daily`, `weekly`, `monthly`, `yearly`, or `never`;
`priority` is `0`–`1`. A video entry also accepts `content_loc`, `player_loc`, `duration`,
`view_count`, `rating`, `expiration_date`, `publication_date`, `family_friendly`,
`requires_subscription`, `live`, `restriction`, `platform`, `uploader`, and `tag`; `restriction` and
`platform` are `{ relationship: 'allow' | 'deny', content }` and `uploader` is `{ content, info? }`.
The exported types — `SiteSitemapConfig`, `SiteSitemapEntry`, `SiteSitemapVideo`,
`SiteRobotsConfig`, `SiteRobotsRule` — are the checked spelling of this table; see the
[Public API reference](17-public-api-reference.md).

## Content artifacts without plugin wiring

Markdown and MDX routes work without `content`. Enable `content: true` only when the site also needs
`/content.json`, `/search-index.json`, `/rss.xml`, `/sitemap.xml`, and `/llms.txt`. The content
engine reuses `site.url`, `site.title`, `site.description`, and `site.language`, so it does not
require a second plugin import or duplicate site identity.

```ts
export default config({
  site: {
    url: 'https://example.com',
    title: 'Example Docs',
    description: 'Guides for Example',
    language: 'en',
  },
  content: {
    engine: {
      exclude: ['/drafts/*'],
      minTermLength: 3,
      llmsPath: false,
    },
  },
})
```

| `content.engine` key | Default                | Purpose                                                                 |
| -------------------- | ---------------------- | ----------------------------------------------------------------------- |
| `exclude`            | none                   | Exact route paths or trailing-`*` patterns omitted from every artifact. |
| `locale`             | `site.language`        | BCP 47 locale used for search tokenization.                             |
| `stopWords`          | built-in list          | Terms the search index ignores.                                         |
| `minTermLength`      | `2`                    | Ignore shorter search terms.                                            |
| `manifestPath`       | `"/content.json"`      | Where the content manifest is published.                                |
| `searchPath`         | `"/search-index.json"` | Where the search index is published.                                    |
| `feedPath`           | `"/rss.xml"`           | Where the feed is published.                                            |
| `sitemapPath`        | `"/sitemap.xml"`       | Where the content sitemap is published.                                 |
| `llmsPath`           | `"/llms.txt"`          | Agent discovery index. `false` disables it.                             |
| `language`           | `site.language`        | Feed language, when it differs from the search locale.                  |

The existing `contentEngine(options)` plugin remains supported for advanced or programmatic plugin
composition. Do not configure both forms in the same application.

## Environment variables

| Variable                                                                                                                                                                 | Evidence-backed purpose                                                                                                                     |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `RUVYXA_SITE_URL`                                                                                                                                                        | Fallback canonical origin for site discovery.                                                                                               |
| `RUVYXA_RUNTIME`                                                                                                                                                         | CLI/runtime override (`node`, `bun`, or `deno`) used by dev/build paths.                                                                    |
| `RUVYXA_ADAPTER`                                                                                                                                                         | Build adapter selection override.                                                                                                           |
| `RUVYXA_BUILD_CACHE_DIR`                                                                                                                                                 | Shared build cache directory override.                                                                                                      |
| `RUVYXA_RENDER_CACHE_SIZE`                                                                                                                                               | Render-cache capacity.                                                                                                                      |
| `RUVYXA_WORKER_POOL_SIZE`, `RUVYXA_WORKER_TIMEOUT_MS`, `RUVYXA_WORKER_MAX_CONCURRENCY`, `RUVYXA_WORKER_MAX_QUEUE`, `RUVYXA_MEMORY_LIMIT_MB`, `RUVYXA_WORKER_SHUTDOWN_MS` | Worker-pool operational controls.                                                                                                           |
| `RUVYXA_MAX_CONCURRENCY`, `RUVYXA_MAX_QUEUE`                                                                                                                             | How many renders `ruvyxa start` runs at once and how many may wait; `0` concurrency turns admission off. Off by default under `ruvyxa dev`. |
| `RUVYXA_DRAIN_DELAY`, `RUVYXA_SHUTDOWN_GRACE`                                                                                                                            | Milliseconds a shutdown keeps accepting so a readiness probe can read `503`, and how long in-flight work then has to finish.                |
| `RUVYXA_PRERENDER_RECYCLE_AFTER`                                                                                                                                         | Isolated renders one build worker performs before it is replaced (default 32); `0` disables recycling.                                      |
| `RUVYXA_STREAM_ASSET_THRESHOLD_BYTES`                                                                                                                                    | Public-asset size above which the response is streamed rather than buffered (default 8 MiB).                                                |
| `RUVYXA_WORKER_MAX_LINE_BYTES`                                                                                                                                           | Largest single worker protocol line accepted before the worker is replaced (default 64 MiB).                                                |
| `RUVYXA_PUBLIC_*`                                                                                                                                                        | Browser-safe values injected for client use.                                                                                                |
| `RUVYXA_FUN`                                                                                                                                                             | Set to `0`/`false`/`off` to stop CLI spinners and the running mascot; colour is unaffected.                                                 |
| `RUVYXA_ASCII`                                                                                                                                                           | Set to `1` to draw progress and status with ASCII glyphs only.                                                                              |
| `FORCE_COLOR`, `CLICOLOR_FORCE`                                                                                                                                          | Colour redirected output, and optionally pin its depth: `1` = 16 colours, `2` = 256, `3` = 24-bit.                                          |

CLI output also honours the two conventional terminal opt-outs: `NO_COLOR` removes colour, and
`TERM=dumb` removes colour, animation, and non-ASCII glyphs. Output that is piped or redirected is
never animated.

`FORCE_COLOR` is for the case those two get wrong: a CI log that renders ANSI. It outranks both
`NO_COLOR` and `TERM=dumb`, because it is the one of the three set deliberately for a single run
rather than inherited from a shell profile or a build image. `FORCE_COLOR=0` is how the same
variable says no. Forcing colour never forces animation — a spinner repaints its line, and a log
file has nowhere to repaint to.

When the terminal reports 24-bit colour, decorative parts of the output — the wordmark, the rules
under a header and a section title, the trail behind the progress mascot, the magnitude bars in
`bench` — are drawn as gradients. Nothing that carries meaning is: every status, count, path, and
classification stays inside the sixteen colours every terminal renders identically, so a smaller
palette loses the decoration and never a distinction.

Internal variables beginning or ending in double underscores are runtime transport details, not
application configuration. Never set them manually. Values such as `RUVYXA_AUTH_SECRET` occur in the
auth scaffolder; use a private environment source and never expose one with the public prefix.

`RUVYXA_WORKER_MAX_QUEUE` defaults to four times `RUVYXA_WORKER_MAX_CONCURRENCY`. It bounds waiting
render work and returns `RUV1705` when full; use load-test evidence before increasing it because a
larger queue retains more request data and increases wait time.

### Type public variables and keep private ones server-only

Declare the public variables that your client code reads. This turns misspellings into TypeScript
errors and makes the browser-visible contract reviewable without exposing private values.

```ts
// app/ruvyxa-env.d.ts
interface ImportMetaEnv {
  readonly RUVYXA_PUBLIC_APP_NAME: string
  readonly RUVYXA_PUBLIC_API_URL: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
```

```dotenv
# .env.example — commit names and harmless placeholders, never real secrets
RUVYXA_PUBLIC_APP_NAME=Example
RUVYXA_PUBLIC_API_URL=https://api.example.test
DATABASE_URL=replace-me-at-deploy
RUVYXA_AUTH_SECRET=replace-me-at-deploy
```

```tsx
// A client component: only public values belong here.
'use client'
export function AppName() {
  return <span>{import.meta.env.RUVYXA_PUBLIC_APP_NAME}</span>
}
```

Do not add `DATABASE_URL` or other private names to `ImportMetaEnv`, and do not read them in a
client component. Read private values only from server-only code such as a loader, action, or API
route. The framework's boundary validation is an additional guard, not a reason to place secrets in
shared modules. Pair the committed `.env.example` with `requireEnv([...])` for names that must be
present at release time.

### `cache.handler` — which store keeps a revalidated document

An ISR or PPR route renders once and is served from a store until its window expires. Which store is
normally the platform's answer: a Cloudflare Worker gets KV, a serverless function gets the one
writable directory it has, and `ruvyxa start` gets its own build output.

That directory is per-instance and per-deployment. For a single container it is the right answer.
For an application running several instances behind one domain, it is not: each instance revalidates
separately, and a visitor is served whichever copy the load balancer happened to pick. Only the
application knows what store they should share, so this is where it says so.

**`ruvyxa start` and `ruvyxa preview` honour it, and `ruvyxa dev` does not.** A deployed build
compiles the handler into the route registry every adapter's wrapper imports; `start` and `preview`
load the same module into their render workers instead, so `cache()`, `revalidateTag()`,
`invalidateCache()`, and stored ISR documents all reach it there too.

`ruvyxa dev` is excluded deliberately. Every key a shared store holds is namespaced by the build id,
and a development server has no build — it would either write unprefixed keys into whatever store
the project configured, which is usually production's, or invent an identity meaning nothing to the
deployment. Reading and writing production cache entries from a working copy is not a smaller
version of this feature, so `dev` says on startup that it is skipping the handler and keeps its
per-process cache.

One difference from a deployed build is worth knowing. There, the handler replaces the platform's
document store outright. Here it is consulted _first_ and the build's own `prerender` directory is
the fallback, because on this host those are two different things: a path your store has never held
is still answered by the document this build produced, rather than re-rendered once per instance
against a cold store. A document the store reports as `stale` is served and, on an ISR route,
refreshed behind the response — the same stale-while-revalidate a deployed build runs — and the
refreshed document is written back through the handler so every other instance sees it. A bare
string from `read` counts as stale on both hosts.

```ts
// ruvyxa.config.ts
export default {
  cache: { handler: './cache-handler.mjs' },
}
```

```js
// cache-handler.mjs — Redis, S3, a database, whatever the deployment already runs.
export async function read(pathname, revalidate) {
  const entry = await store.get(pathname)
  if (!entry) return null // not cached
  return { html: entry.html, stale: Date.now() - entry.storedAt >= revalidate * 1000 }
}

export async function write(pathname, html, revalidate, forced) {
  await store.set(pathname, { html, storedAt: Date.now() })
}
```

The path is project-relative and the module is compiled into the deployed bundle, so it may import
anything the application can. It is not loaded at build time.

Both exports are optional: supply `read` alone and the platform still writes where it would have.
Declare nothing and every host keeps the behaviour it has.

This is the seam Next.js exposes as `cacheHandler` in `next.config.js`, and it exists for the same
reason — the framework cannot pick an application's shared store for it.

`revalidateTag()` clears this process's own `cache()` entries immediately, as it always has. When a
handler exports `revalidateTag`, the tags a request queued are additionally handed to it after the
response — which is what makes the invalidation reach every instance rather than only the one that
served the mutation:

```js
export async function revalidateTag(tags) {
  await store.dropEverythingLabelled(tags)
}
```

It is optional, and there is no platform fallback: a tag labels whatever the application decided to
label with it, and a platform cache keyed by URL has nothing to look one up by. A project that
declares no handler keeps the behaviour it has, which is that `revalidateTag()` clears one process.

`invalidateCache()` has the same seam, and needs it more. Without one it is undone by its own next
read: the key is gone from this process, the shared store still holds it, and the following miss
reads it back and re-commits it under a full TTL — an invalidation that looks like it worked.

```js
export async function deleteData(keys) {
  for (const { key, prefix } of keys) {
    if (key !== undefined) await store.delete(key)
    await store.deleteByPrefix(prefix)
  }
}
```

Each entry carries an exact key and a prefix because they are not derivable from one another:
`invalidateCache('products')` clears `products` and everything under `products:`, and not
`productsXYZ`. `key` is absent for `invalidateCache()` with no argument, which names none — that
call clears the deployment's whole namespace, which the prefix alone says. Both values already carry
this deployment's build id, so apply them as they arrive.

The same module can back `cache()` itself, which is the other half of what Next.js puts behind
`cacheHandler`:

```js
export async function readData(key) {
  const row = await store.get(key)
  // `populatedAt` is when the value was produced. The freshness window is
  // recomputed from the `ttl` the calling code asked for, so an entry another
  // instance wrote with a longer one cannot extend it here.
  return row ? { value: row.value, populatedAt: row.populatedAt } : null
}

export async function writeData(key, entry) {
  await store.set(key, entry)
}
```

The process's own in-memory store still answers first — it is the fast tier, and it is where
Next.js's `cacheMaxMemorySize` sits too. Only a local miss consults the shared one, and only a miss
in both runs the producer. Writes are published without the request waiting on them.

A store that throws is a slower cache, not a failed request: the error is reported and the producer
runs. Declaring none costs nothing at all — the no-handler path does not even allocate a promise.

### `cache.maxEntries` — how much of `cache()` stays in this process

```ts
export default {
  cache: { handler: './cache-handler.mjs', maxEntries: 0 },
}
```

The in-memory tier holds 1024 entries by default and evicts least-recently-used beyond that. `0`
turns it off entirely, so every read reaches the shared store.

That is the setting to reach for once a deployment runs several instances behind one shared store: a
per-instance copy in front of a shared one is the thing that makes two instances answer the same key
differently, and turning it off trades one round trip for one answer.

Next.js spells this decision `cacheMaxMemorySize`, and `0` means the same there. The unit differs on
purpose — this store counts entries and has no size accounting to answer a byte budget with, and a
budget that estimated would be one nobody could rely on.

A value that is not a whole number of entries is reported and ignored: an unusable bound must not
quietly become "no cache" or "unbounded", which are the two directions that hurt and both look like
working code.

### `cache.maxBytes` — the memory bound the entry count cannot express

```ts
export default {
  cache: { maxEntries: 1024, maxBytes: 52_428_800 },
}
```

`maxEntries` bounds how many values are held; it says nothing about how large they are. A thousand
entries of ten megabytes is ten gigabytes. `maxBytes` defaults to fifty megabytes — the same budget
Next.js defaults `cacheMaxMemorySize` to — and evicts least-recently-used until the total fits.

Each value is measured by its serialized length. That is an approximation, and it is the one
available: every cached value has already been proved serializable, so it is always measurable this
way, and a measurement within a small factor beats a bound that does not exist. `0` disables the
byte budget and leaves `maxEntries` in sole charge.

A value larger than the whole budget is still stored once and evicted by the next write. A write
that reported success must not leave nothing behind.

### What the shared store is not asked to guarantee

Three things are worth knowing before pointing `cache.handler` at a network store, because each is a
deliberate trade rather than an oversight:

- **Invalidation waits; population does not.** `revalidateTag()` is awaited before the response,
  because a mutation that answered `200` has told the caller the old value is gone — losing that
  write is a correctness failure. Cache _writes_ are not awaited, because holding a response to
  populate a cache is the opposite of what the cache is for.
- **Unawaited writes are bounded.** At most 256 are in flight; beyond that they are dropped and
  counted, and the first drop is reported. A store that has gone slow under load would otherwise
  accumulate one promise per produced value, and the cache that exists to protect the origin becomes
  what exhausts the process. `cacheStats()` reports `pendingSharedWrites` and `droppedSharedWrites`.
- **Keys carry this deployment's build id.** Two deployments pointed at one managed store would
  otherwise both write `cache('user:1')` and read each other's answer. Your handler receives the
  prefixed key.

**Previous:** [UI, navigation, metadata, and assets](06-ui-navigation-metadata-and-assets.md) ·
**Next:** [Plugins and middleware](08-plugins-middleware.md)
