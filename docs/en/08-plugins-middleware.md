# Plugins and middleware

> **Tutorial goal:** add cross-cutting behavior once, then apply it to the routes that need it.
> **Start from:** a configured application in [Configuration](07-configuration.md). **Checkpoint:**
> verify one matching and one non-matching route after enabling a plugin or middleware rule.

Plugins are values returned by `definePlugin()` from `ruvyxa/plugin` (also re-exported by `ruvyxa`).
Add them to `plugins` in `ruvyxa.config.ts`. A plugin needs a non-empty name and either declarative
behavior or `register(api)`; invalid definitions fail with `RUV2102`.

## Declarative plugin

```ts
// plugins/request-id.ts
import { definePlugin } from 'ruvyxa/plugin'

export const requestId = definePlugin({
  name: 'example:request-id',
  http: {
    match: ['/api/*'],
    onResponse({ response }) {
      const headers = new Headers(response.headers)
      headers.set('x-example', 'enabled')
      return new Response(response.body, { status: response.status, headers })
    },
  },
})
```

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { requestId } from './plugins/request-id'
export default config({ plugins: [requestId] })
```

`http.match` uses exact paths or trailing-`*` prefixes. A request hook may return `Request`,
`Response`, or nothing; a response hook may return `Response` or nothing. `http.routes` declares
exact plugin-owned routes and accepts one method, multiple methods, or every method if omitted. The
advanced `register` API exposes `http`, `build`, `dev`, `diagnostics`, `native`, and `head` sockets.

## Build and dev lifecycle

Build hooks are `onStart`, `onResolve`, `onLoad`, `onTransform`, and `onComplete`.
Resolve/load/transform hooks receive an environment of `client`, `server`, `edge`, `worker`, or
`shared`; transformations return code, `{ code, map }`, null, or nothing. Dev exposes an
`onFileChange` registration. Plugins can report diagnostics and contribute document-head entries. Do
not rely on module-level middleware state across workers: config explicitly states workers do not
share it.

### `onResolve` answers with a path, not with a virtual id

A resolve hook returns a **file path**. The file itself may be virtual — an `onLoad` hook supplies
its contents, and nothing has to exist on disk — but the value still has to name a location, because
everything downstream treats it as one. The two spellings the rest of the ecosystem uses for a
virtual module are not paths and are refused by name:

```ts
build.onResolve(({ id, root }) =>
  id === 'stress-virtual' ? `${root}/virtual-stress-virtual.ts` : undefined,
)
```

`'�stress-virtual'` and `'virtual:stress-virtual'` were each joined onto the project root and handed
to the filesystem, so they surfaced as a raw OS error —
`strings passed to WinAPI cannot contain NULs` and `The system cannot find the file specified` —
naming no plugin. Both now fail with a diagnostic that names the plugin, the specifier, and the path
shape to return instead.

### `onTransform` rewrites the browser bundle, not the server render

A build compiles each module twice, and only one of the two runs your hook. The browser bundle is
built by the Rust bundler, which calls `onTransform`; the server render — `dev`, `start`,
pre-rendering, and every deployed function — reads the same file through the JavaScript compiler,
which does not. In practice `environment` is therefore always `client` inside a transform.

That is fine for anything the browser alone observes, and wrong for anything that ends up in markup:

```ts
// The rewritten value is rendered by both halves, from different sources.
build.onTransform(({ code, id }) =>
  id.endsWith('/marker.ts') ? code.replace("'original'", "'rewritten'") : undefined,
)
```

```tsx
// app/page.tsx — server-rendered and hydrated
export default () => <p>{MARKER}</p> // server writes "original", browser expects "rewritten"
```

React discards the whole server tree and re-renders when they disagree (#418). Nothing fails: the
page ends up correct, after a flash of the wrong content, and in production there is no warning at
all. `ruvyxa build` reports the pairing when it sees it — a transformed module reached by a route
that both renders on the server and hydrates.

Two ways to keep it honest. Put the value behind a `'use client'` route, which has no server
document to disagree with — this is what `examples/demo` does. Or compute it at runtime, through an
environment variable or a module the server reads too, instead of rewriting source text.

## First-party plugins

`ruvyxa/plugins` implements: `redirects`, `headers`, `observability`, `securityHeaders`,
`cacheRules`, `sitemap`, `robots`, `alias`, and additional file-backed helpers in that public entry
point. Use its validation rather than reconstructing the behavior. For example, redirects permit
`*`, exact paths, or trailing-prefix patterns and only accept absolute HTTP(S) URLs or safe absolute
paths as destinations.

```ts
import { redirects, securityHeaders } from 'ruvyxa/plugins'
export default config({
  plugins: [
    redirects([{ source: '/old/*', destination: '/new/*', permanent: true }]),
    securityHeaders({ contentSecurityPolicy: { 'default-src': ["'self'"] } }),
  ],
})
```

`permanent: true` makes `redirects` send 308; otherwise it sends 307. `securityHeaders` supplies
HSTS by default but cannot choose a safe CSP for your application—set one deliberately and test
third-party resources.

## First-party plugin catalog

| Plugin                                | Output or runtime behavior                                                                          |
| ------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `redirects`, `headers`, `cacheRules`  | Route-scoped redirects, response headers, and browser/CDN cache directives.                         |
| `observability`, `securityHeaders`    | Request ID/timing/structured logs and response security policy.                                     |
| `pwa`                                 | Manifest, service worker, registration script, optional precache/offline fallback, and HTML wiring. |
| `sitemap`, `robots`, `feed`           | Build-time `sitemap.xml`, `robots.txt`, and RSS output from explicit metadata.                      |
| `searchIndex`, `contentEngine`        | Build-time search index and content-derived answer/search artifacts.                                |
| `openApi`                             | OpenAPI 3.1 JSON served in development and written into production output.                          |
| `alias`, `bundleBudget`, `requireEnv` | Build-time import aliasing, client JavaScript size limits, and required environment validation.     |
| `fonts`                               | Build-time self-hosting for supplied Google Fonts stylesheet URLs.                                  |
| `originGuard`                         | Blocks cross-site mutation requests to route handlers, opt-in per route scope.                      |
| `healthCheck`                         | Liveness endpoint answered by the request host, ahead of route rendering.                           |
| `webVitals`                           | Core Web Vitals collected in the browser and reported server-side.                                  |
| `llmsTxt`                             | Build-time `llms.txt` site index from curated sections and discovered routes.                       |
| `wellKnown`                           | Files under `/.well-known/`, including RFC 9116 `security.txt`.                                     |
| `headScriptHashes`                    | CSP source hashes for inline scripts and styles that plugins contribute.                            |

Use explicit data with build-time plugins: they do not discover your business content or API
semantics automatically. For example, this is a complete PWA declaration with the required `name`:

```ts
import { pwa, openApi } from 'ruvyxa/plugins'

export default config({
  plugins: [
    pwa({
      name: 'Example app',
      icons: [{ src: '/icon-192.png', sizes: '192x192', type: 'image/png' }],
    }),
    openApi({
      info: { title: 'Example API', version: '1.0.0' },
      operations: [
        { method: 'GET', path: '/api/health', responses: { '200': { description: 'Healthy' } } },
      ],
    }),
  ],
})
```

The PWA plugin defaults to `/manifest.webmanifest`, `/sw.js`, and `/pwa-register.js`; all three
paths must differ. `openApi` defaults to `/openapi.json`, requires a non-empty title/version, and
rejects duplicate method/path and `operationId` entries. Run a production build and inspect the
generated output whenever adding a build plugin.

## Build artifacts during development

`robots`, `feed`, `searchIndex`, `openApi`, `pwa`, `wellKnown`, and `webVitals` also answer requests
for the file they generate, so `ruvyxa dev` serves the same bytes the build writes and the output
can be checked without a production build.

`feed` and `searchIndex` do this only when their content is a static array. Given a loader, they
stay build-time only: a plugin cannot tell development from production at request time, so running a
loader per request would put a file read or a database query on the production response path.
`sitemap` and `llmsTxt` are build-time only for the same class of reason — their entries come from
the route manifest, which does not exist while the development server is running.

## Guarding route handlers

Server actions reject cross-site requests in both hosts. A handler under `app/api/` does not: it is
reachable from any origin, and a session cookie defaults to `SameSite=Lax`, which a cross-site form
POST still carries. `originGuard` closes that for the routes it is given.

```ts
import { healthCheck, originGuard, webVitals, wellKnown } from 'ruvyxa/plugins'

export default config({
  plugins: [
    originGuard({ routes: ['/api/*'] }),
    healthCheck({ path: '/health', check: () => ({ status: 'up' }) }),
    webVitals({ sampleRate: 0.1 }),
    wellKnown({
      securityTxt: {
        contact: 'mailto:security@example.com',
        expires: '2027-01-01T00:00:00.000Z',
      },
    }),
  ],
})
```

It is opt-in rather than a default because an API meant to be called from another origin is a
legitimate design; that case is governed by CORS instead. Unsafe methods are checked by comparing
`Origin` against `Host`, falling back to `Sec-Fetch-Site: same-origin` when the origin was stripped,
and failing closed when neither is present. `webVitals` publishes its client script as a build asset
and loads it with `src`, so it does not force `'unsafe-inline'` into a `script-src` policy.

## Content-Security-Policy

A page carries no executable inline script of Ruvyxa's own. Route parameters and the request path
travel to the client in a `<script type="application/json">` data block, which the browser does not
execute and `script-src` does not apply to, so a strict policy needs no nonce for it:

```ts
securityHeaders({ contentSecurityPolicy: { 'default-src': ["'self'"], 'script-src': ["'self'"] } })
```

Two things still need covering. A plugin that contributes an inline `<script>` through `head` is
identical on every request, so it is covered by a hash rather than a nonce:

```ts
import { headScriptHashes, securityHeaders } from 'ruvyxa/plugins'

const plugins = [analytics()]
export default config({
  plugins: [
    ...plugins,
    securityHeaders({
      contentSecurityPolicy: { 'script-src': ["'self'", ...headScriptHashes(plugins)] },
    }),
  ],
})
```

`headScriptHashes` returns nothing for a plugin that loads its script with `src`; the first-party
`webVitals` is built that way for exactly this reason. Pass `{ tag: 'style' }` for the matching
`style-src` hashes.

The other is React itself. A route that streams Suspense content carries React's own inline runtime
— the script that swaps a resolved boundary into place. It is not Ruvyxa's to move into a data
block, and it is written into a build artifact every request reuses, so a per-request nonce would be
baked in and therefore public. Its bytes are fixed once the artifact is written, so a hash is what
fits — but they name the boundary ids they complete, which makes them per-document and impossible to
maintain by hand.

`inlineScriptHashes` has the build record them:

```ts
securityHeaders({
  contentSecurityPolicy: { 'default-src': ["'self'"], 'script-src': ["'self'"] },
  inlineScriptHashes: true,
})
```

The build writes `csp-inline-hashes.json` into the output directory, and each response picks up the
hashes for the document it is serving — a route with no inline script gets its policy unchanged.
Pass `{ outDir }` when the project's build output is not `.ruvyxa`. It requires `script-src` to
already be in the policy: a policy that deliberately falls back to `default-src` is left alone,
because narrowing it to exactly these hashes would block the application's own bundles. Under
`ruvyxa dev` nothing has been built, so no hashes are added.

**Previous:** [Configuration and environment](07-configuration.md) · **Next:**
[Integrations](09-integrations-auth-data-and-realtime.md)
