# Plugins

Ruvyxa plugins are ordinary application modules written in TypeScript.

Create a starter:

```bash
npx ruvyxa plugin new auth
```

The command creates `auth/` (named after the plugin — no `--dir` flag needed) with `src/index.ts`,
`package.json`, `tsconfig.json`, and `README.md`. Add `--dir <path>` only if you want a different
location. Plugins run on both Node.js and Bun (`--runtime bun` or `RUVYXA_RUNTIME=bun`):

```ts
import { plugin } from 'ruvyxa/config'

export default plugin('auth', {
  routes: ['/*'],
  onRequest(request) {
    const headers = new Headers(request.headers)
    headers.set('x-auth', 'true')
    return new Request(request, { headers })
  },
})
```

Import it from `ruvyxa.config.ts`:

```ts
import auth from './plugins/auth'
import { config } from 'ruvyxa/config'

export default config({ plugins: [auth] })
```

Use `plugin(name, middleware)` for request/response middleware. It accepts either a middleware
object (with optional `routes`, `onRequest`, `onResponse`) or just a request handler function.
Middleware uses standard Fetch `Request` and `Response`.

For `resolveId`, `transform`, or `onBuildComplete`, use the advanced `definePlugin({ name, setup })`
form. All hooks run in the persistent Node/Bun runtime; there is no separate compiler, debug
command, or custom middleware ABI.

## Built-in plugins

`ruvyxa/plugins` ships first-party plugins built on the same public hooks:

```ts
import { config } from 'ruvyxa/config'
import {
  cacheRules,
  feed,
  observability,
  openApi,
  pwa,
  searchIndex,
  securityHeaders,
} from 'ruvyxa/plugins'

export default config({
  plugins: [
    observability({ routes: ['/api/*'] }),
    securityHeaders({
      contentSecurityPolicy: {
        'default-src': ["'self'"],
        'object-src': ["'none'"],
      },
    }),
    cacheRules([
      { source: '/api/*', browser: 'no-store' },
      { source: '/blog/*', browser: 'public, max-age=60', cdn: 'max-age=300' },
    ]),
    pwa({ name: 'Example', offlineFallback: '/offline' }),
    feed({
      siteUrl: 'https://example.com',
      title: 'Example',
      description: 'Latest articles',
      items: [{ title: 'Launch', url: '/blog/launch' }],
    }),
    searchIndex({
      locale: 'en',
      documents: [{ id: 'home', title: 'Home', url: '/', text: 'Welcome to Example' }],
    }),
    openApi({
      info: { title: 'Example API', version: '1.0.0' },
      operations: [{ method: 'get', path: '/api/health', summary: 'Health check' }],
    }),
  ],
})
```

- `redirects(rules)` — declarative redirects served before rendering. Exact paths or trailing-`*`
  prefixes; a `*`-suffixed destination receives the matched remainder. `permanent: true` responds
  308 instead of 307.
- `headers(rules)` — response headers per route. Rules without `source` apply everywhere.
- `observability({ routes, requestIdHeader, traceContext, serverTiming, log, logger })` — propagates
  a validated request ID and W3C `traceparent`, measures across middleware workers, appends a
  `Server-Timing` metric, and logs method/path/status without query strings. Set `log: false` or
  provide `logger(entry)` when the application already has a log pipeline. A failing custom log sink
  is reported but never fails the application response.
- `securityHeaders(options)` — adds HSTS by default and optional CSP, permissions, referrer,
  cross-origin, frame, and custom headers. Ruvyxa's native defaults fill only missing headers, so
  explicit plugin policies win. CSP is opt-in because one universal policy would break valid apps.
- `cacheRules(rules)` — sets browser `Cache-Control`, shared `CDN-Cache-Control`, and merged `Vary`
  values per route. Later matching rules override earlier cache policies.
- `sitemap({ siteUrl, exclude, robots })` — writes `sitemap.xml` (and optionally `robots.txt`) into
  the served asset directory after each production build, from the route manifest. Dynamic patterns
  and API routes are skipped.
- `robots({ rules, sitemap })` — standalone `robots.txt` generation.
- `pwa(options)` — generates and serves a web manifest, service worker, and registration module;
  injects their tags into matching HTML responses; and patches matching prerendered HTML. Provide
  `precache` and `offlineFallback` explicitly so the service worker never guesses application data.
  Cache namespaces are isolated by service-worker scope, including when several apps share an
  origin.
- `feed({ siteUrl, title, description, items, path })` — generates RSS 2.0 from an item array or an
  async build-time loader. The default output is `/rss.xml`.
- `searchIndex({ documents, locale, stopWords, minTermLength, path })` — generates a deterministic
  JSON inverted index. `Intl.Segmenter` provides word boundaries for languages including Thai; the
  default output is `/search-index.json`.
- `openApi({ info, operations, servers, tags, components, path })` — validates operation uniqueness,
  serves OpenAPI 3.1 JSON during development, and writes `/openapi.json` for production.
- `alias(map)` — resolves exact import specifiers to project files before the native resolver.
- `bundleBudget({ maxChunkKb, maxTotalKb })` — fails the production build when emitted client
  JavaScript exceeds the budget, so bundle regressions surface in CI.
- `requireEnv(names)` — fails the production build when required environment variables are missing
  or empty.

Build-generated public files run before adapter materialization. Therefore sitemap, PWA, feed,
search, and OpenAPI files are included in static and hybrid deployment artifacts rather than only
the local `.ruvyxa` directory. Static adapters preserve the same URLs as the production server:
public files stay at `/...` and client bundles stay under `/__ruvyxa/client/...`. Generated files
use atomic replacement, and configurable artifact paths reject cross-origin, traversal, directory,
and colliding PWA endpoint values during configuration.

`observability`, `securityHeaders`, and `cacheRules` are runtime response plugins. On a serverless
or long-running adapter they run normally; a fully static host has no middleware runtime, so set
equivalent security/cache headers in that host or adapter configuration.

Middleware `routes` are also reported to the native server, which skips the plugin round-trip
entirely for requests no middleware can match — keep middleware route-scoped where possible. Route
patterns must be `*`, an exact path beginning with `/`, or a prefix ending in `*`; invalid patterns
fail during plugin startup instead of silently never matching.

## Middleware worker pool

Plugin middleware runs on one persistent runtime process by default. When stateless middleware on
hot routes becomes a throughput bottleneck, `middleware.workers` (1–8) starts a pool of identical
runtime processes dispatched round-robin:

```ts
export default config({
  middleware: {
    workers: 2,
    timeoutMs: 15_000,
  },
})
```

Workers do not share module-level plugin state — counters, caches, or sessions kept in plugin module
scope become per-process. Keep the default of one worker unless plugin middleware is stateless. The
pool prefers an idle worker before queueing behind a busy one. `timeoutMs` bounds each middleware
hook (default 30,000; range 1–300,000 ms). A crashed worker is restarted and the in-flight hook is
retried once. Timed-out hooks and malformed protocol responses replace the worker without retrying,
because the hook may already have produced side effects.
