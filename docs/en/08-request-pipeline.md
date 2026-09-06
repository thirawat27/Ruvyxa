# Request pipeline: headers, redirects, rewrites, proxy, and route handlers

> **Tutorial goal:** add cross-cutting request behavior once, in `ruvyxa.config.ts`, and have it
> apply identically under `ruvyxa dev`, `ruvyxa start`, and every deployed build. **Start from:** a
> configured application in [Configuration](07-configuration.md). **Checkpoint:** verify one
> matching and one non-matching path after adding a rule or a proxy.

Ruvyxa has no plugin system. Everything that used to be a plugin is one of three things: a key on
the config object (`headers()`, `redirects()`, `rewrites()`, `proxy`, `realtime`, `collab`,
`content`), a file convention (`instrumentation.ts`), or a route handler under `app/` (`route.ts`).
Each is declared once and evaluated by both request hosts — the Axum server behind `dev`/`start`,
and the handler every adapter deploys — from one shared table, so a rule cannot apply on one host
and not the other.

## Order of evaluation

For every request, in this order:

1. `headers()` — decided on the request as it arrived, set on whatever answers it.
2. `redirects()` — answers with `308` (`permanent: true`) or `307`.
3. `proxy.handler` — for the paths `proxy.matcher` names.
4. `rewrites().beforeFiles` — before static files and pages.
5. Static files and page/API routes.
6. `rewrites().afterFiles` — after files, before dynamic routes.
7. Dynamic routes.
8. `rewrites().fallback` — after everything, just before the 404.

Framework endpoints under `/__ruvyxa/` are decided ahead of all of it on both hosts; no rule can
redirect `/__ruvyxa/action`, and `proxy.handler` never sees it.

## Source patterns

`source` in every rule, and each entry of `proxy.matcher`, is a path-to-regexp pattern matched
against the canonical request path — decoded, no trailing slash, `/` for the root — whole and
case-insensitively:

| Pattern               | Matches                                  |
| --------------------- | ---------------------------------------- |
| `/about`              | `/about`, `/About`                       |
| `/blog/:slug`         | one segment; `slug` is a parameter       |
| `/blog/:slug*`        | zero or more segments, joined as `a/b/c` |
| `/docs/:path+`        | one or more segments                     |
| `/shop/:category?`    | zero or one segment                      |
| `/post/:id(\\d{1,})`  | a segment matching the inline regex      |
| `/((?!api\|_next).*)` | an unnamed group, parameter `0`          |
| `/:path*`             | everything, including `/`                |

Parameters substitute into `destination` and into header keys and values as `:name`. A rule may also
carry `has` and `missing` conditions on a `header`, `cookie`, `query`, or `host`; `value` is a
whole-value regex, and a named capture in it becomes a parameter too. A pattern that does not
compile fails the config with `RUV1602` rather than the first request it would have matched. Both
hosts replay `tests/fixtures/route-rules-conformance.json`, so the table above is a contract rather
than a description.

## `headers()`

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'

export default config({
  headers: [
    { source: '/api/:path*', headers: [{ key: 'cache-control', value: 'no-store' }] },
    {
      source: '/:path*',
      has: [{ type: 'host', value: 'admin.example.com' }],
      headers: [{ key: 'x-frame-options', value: 'DENY' }],
    },
  ],
})
```

Rules apply in order; a later rule's key overrides an earlier one. The list may be a function, sync
or async. Ruvyxa's default security headers are still applied to every response and set only where
the application has not set the same header, so a `headers()` rule wins.

## `redirects()`

```ts
export default config({
  redirects: async () => [
    { source: '/old-blog/:path*', destination: '/blog/:path*', permanent: true },
    { source: '/docs/:path*', destination: 'https://docs.example.com/:path*', permanent: false },
    {
      source: '/legacy',
      destination: '/',
      statusCode: 302,
      missing: [{ type: 'header', key: 'x-do-not-redirect' }],
    },
  ],
})
```

`statusCode` wins over `permanent`; otherwise `permanent: true` is `308` and anything else is `307`.
A destination with no query inherits the request's query string. Destinations are validated at
config time: an absolute URL must be `http(s)`, and a relative one must be an absolute application
path.

## `rewrites()`

```ts
export default config({
  rewrites: {
    beforeFiles: [{ source: '/alias', destination: '/' }],
    afterFiles: [{ source: '/blog/:slug', destination: '/posts/:slug' }],
    fallback: [{ source: '/:path*', destination: '/not-found-page' }],
  },
})
```

A bare list is `afterFiles`. Parameters the destination does not use are appended to its query,
unless the destination uses any parameter at all; the request's own query is merged. A deployed
build fetches an `https://` destination; the native host serves internal destinations only and
answers `502` for an external one.

## `proxy`

Code that runs ahead of every matching route, kept inside the config file so a project has one place
to read:

```ts
export default config({
  proxy: {
    matcher: [
      '/admin/:path*',
      { source: '/api/:path*', has: [{ type: 'header', key: 'x-block' }] },
    ],
    handler(request) {
      const url = new URL(request.url)
      if (!request.headers.has('authorization')) {
        return new Response('Unauthorized', { status: 401 })
      }
      if (url.pathname === '/admin') {
        return new Request(new URL('/admin/dashboard', url), request)
      }
      const headers = new Headers(request.headers)
      headers.set('x-request-start', String(Date.now()))
      return new Request(request, { headers })
    },
  },
})
```

`handler` receives the standard `Request` and may return a `Response` to answer it, a `Request` to
continue with — a different path is a rewrite, different headers are forwarded — or nothing to
continue unchanged. `matcher` is a string, an array of strings, or entries with `source`, `has`, and
`missing`; without one the handler runs on every request. The matcher is evaluated natively on both
hosts, so on the Axum host a request the matcher does not name never crosses to the JavaScript
process that holds the handler.

`handler` is a function, so it is the one part of the config that stays code: the native host runs
it in a persistent project worker (`middleware.workers` processes, `middleware.timeoutMs` per call),
and every deployed build compiles it into the function bundle and runs it in-process. It runs as
trusted application code with the process's full access; treat what it imports as part of the
application. A static adapter cannot run it and refuses the build with `RUV2204`.

## Guarding route handlers

Server actions reject cross-site requests on both hosts. A handler under `app/api/` does not by
itself: it is reachable from any origin, and a `SameSite=Lax` session cookie still travels with a
cross-site form `POST`. Close that in `proxy.handler` for the routes that mutate state:

```ts
export default config({
  proxy: {
    matcher: '/api/:path*',
    handler(request) {
      if (['GET', 'HEAD', 'OPTIONS'].includes(request.method)) return undefined
      const origin = request.headers.get('origin')
      const host = request.headers.get('host')
      const sameOrigin = origin ? new URL(origin).host === host : false
      const fetchSite = request.headers.get('sec-fetch-site')
      if (sameOrigin || fetchSite === 'same-origin') return undefined
      return new Response('Forbidden', { status: 403 })
    },
  },
})
```

It is per-route rather than a default because an API meant to be called from another origin is a
legitimate design; that case is governed by `middleware.builtin.cors` instead.

## Files instead of hooks

Anything that used to generate a file at build time is a route handler that answers the same path,
or a config key the build already understands:

| Need                                           | Where it lives now                                                                                  |
| ---------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `sitemap.xml`, `robots.txt`                    | `site.sitemap` and `site.robots` in the config; written by `ruvyxa build`                           |
| `/content.json`, search index, RSS, `llms.txt` | `content: true` — see [Configuration](07-configuration.md#content-artifacts)                        |
| `security.txt`, a feed, a manifest, OpenAPI    | `app/.well-known/security.txt/route.ts`, `app/feed.xml/route.ts`, and so on                         |
| A health endpoint                              | `/__ruvyxa/health` on the Axum host and the standalone server, or your own `route.ts`               |
| Required environment at startup                | `register()` in `instrumentation.ts`; `@ruvyxa/database` ships `requireDatabaseEnv()`               |
| Import aliases                                 | `paths` in `tsconfig.json`, honoured by both compilers                                              |
| Realtime and collaboration sockets             | `realtime: true` and `collab: true` — see [Integrations](09-integrations-auth-data-and-realtime.md) |

```ts
// app/.well-known/security.txt/route.ts
export function GET() {
  return new Response('Contact: mailto:security@example.com\nExpires: 2027-01-01T00:00:00.000Z\n', {
    headers: { 'content-type': 'text/plain; charset=utf-8' },
  })
}
```

## Content-Security-Policy

A page carries no executable inline script of Ruvyxa's own. Route parameters and the request path
travel to the client in a `<script type="application/json">` data block, which the browser does not
execute and `script-src` does not apply to, so a strict policy needs no nonce for it:

```ts
export default config({
  headers: [
    {
      source: '/:path*',
      headers: [{ key: 'content-security-policy', value: "default-src 'self'; script-src 'self'" }],
    },
  ],
})
```

A route that streams Suspense content carries React's own inline runtime — the script that swaps a
resolved boundary into place. Its bytes are fixed once the artifact is written, so a hash fits, but
it names the boundary ids it completes and so differs per document; a policy that must cover it
should scope `script-src 'unsafe-inline'` to those routes with a narrower `source` rather than relax
the whole site.

**Previous:** [Configuration and environment](07-configuration.md) · **Next:**
[Integrations](09-integrations-auth-data-and-realtime.md)
