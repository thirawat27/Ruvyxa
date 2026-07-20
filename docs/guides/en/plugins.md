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
import { alias, headers, redirects, robots, sitemap } from 'ruvyxa/plugins'

export default config({
  plugins: [
    redirects([{ source: '/old-blog/*', destination: '/blog/*', permanent: true }]),
    headers([{ source: '/api/*', headers: { 'cache-control': 'no-store' } }]),
    sitemap({ siteUrl: 'https://example.com', robots: true }),
    alias({ '~content': 'content/index.ts' }),
  ],
})
```

- `redirects(rules)` — declarative redirects served before rendering. Exact paths or trailing-`*`
  prefixes; a `*`-suffixed destination receives the matched remainder. `permanent: true` responds
  308 instead of 307.
- `headers(rules)` — response headers per route. Rules without `source` apply everywhere.
- `sitemap({ siteUrl, exclude, robots })` — writes `sitemap.xml` (and optionally `robots.txt`) into
  the served asset directory after each production build, from the route manifest. Dynamic patterns
  and API routes are skipped.
- `robots({ rules, sitemap })` — standalone `robots.txt` generation.
- `alias(map)` — resolves exact import specifiers to project files before the native resolver.
- `bundleBudget({ maxChunkKb, maxTotalKb })` — fails the production build when emitted client
  JavaScript exceeds the budget, so bundle regressions surface in CI.
- `requireEnv(names)` — fails the production build when required environment variables are missing
  or empty.

Middleware `routes` are also reported to the native server, which skips the plugin round-trip
entirely for requests no middleware can match — keep middleware route-scoped where possible.

## Middleware worker pool

Plugin middleware runs on one persistent runtime process by default. When stateless middleware on
hot routes becomes a throughput bottleneck, `middleware.workers` (1–8) starts a pool of identical
runtime processes dispatched round-robin:

```ts
export default config({
  middleware: { workers: 2 },
})
```

Workers do not share module-level plugin state — counters, caches, or sessions kept in plugin module
scope become per-process. Keep the default of one worker unless plugin middleware is stateless. A
crashed worker is restarted automatically and the in-flight hook retried once.
