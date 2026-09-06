<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/core</h1>

<p align="center">
  Typed primitives shared by the Ruvyxa runtime, CLI package, and first-party adapters.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/core"><img src="https://img.shields.io/npm/v/@ruvyxa/core?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/core"><img src="https://img.shields.io/node/v/@ruvyxa/core?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

Most apps import these APIs through `ruvyxa`. Install this package directly only when writing
adapters or low-level integrations.

```bash
npm install @ruvyxa/core
```

## Exports

```ts
import { config } from '@ruvyxa/core/config'
import {
  action,
  cache,
  cacheStats,
  invalidateCache,
  json,
  loader,
  notFound,
  redirect,
} from '@ruvyxa/core/server'
import type {
  Adapter,
  AdapterOutput,
  BuildContext,
  HeaderRule,
  ProxyConfig,
  RedirectRule,
  RewriteRule,
  RuvyxaConfig,
  TransformResult,
} from '@ruvyxa/core'
```

## Server APIs

### Loader with caching

```ts
import { loader } from '@ruvyxa/core/server'

export const getPosts = loader(async ({ cache }) => {
  return cache('posts')
    .ttl('5m')
    .get(async () => {
      return await db.posts.findMany()
    })
})
```

### Action with validation

```ts
import { action } from '@ruvyxa/core/server'

export const createPost = action
  .input({ parse: (v) => ({ title: String(v.title) }) })
  .handler(async ({ input, invalidate }) => {
    invalidate('posts')
    return await db.posts.create(input)
  })
```

### Cache utility

The `cache()` function provides real in-memory TTL caching with LRU eviction and
stale-while-revalidate:

```ts
import { cache, cacheStats, invalidateCache } from '@ruvyxa/core/server'

// Cache with TTL (supports "30s", "5m", "1h", "1d")
const data = await cache('key')
  .ttl('10m')
  .swr('1h') // serve stale while revalidating in background
  .get(async () => fetchExpensiveData())

// Invalidate by key or prefix
invalidateCache('key') // exact match
invalidateCache('posts') // also clears "posts:123"
invalidateCache() // clear all

// Monitor cache
const stats = cacheStats() // { size: number, maxEntries: number }
```

### Response helpers

```ts
import { json, notFound, redirect } from '@ruvyxa/core/server'

// JSON response
return json({ ok: true }, { status: 200 })

// Redirect (status must be 3xx)
return redirect('/login') // 302 by default
return redirect('/dashboard', 301)

// Not found
return notFound('User not found') // 404
```

## Config Shape

```ts
import { config } from '@ruvyxa/core/config'

export default config({
  appDir: 'app',
  outDir: '.ruvyxa',
  css: {
    entries: ['styles/theme.css'],
  },
  server: {
    host: 'localhost',
    port: 3000,
  },
  build: {
    minify: true,
    map: false,
    treeShake: true,
    split: 'route',
    jsx: 'automatic',
    target: 'es2022',
    workers: 4,
    manifest: false,
    warm: true,
  },
  cache: {
    routes: true,
    css: true,
    dir: '.ruvyxa/cache/bundler',
  },
})
```

## Adapter Contract

Adapters return metadata describing how a platform should consume `.ruvyxa/` output:

```ts
import type { Adapter, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

export function customAdapter(): Adapter {
  return {
    name: 'custom',
    target: 'node',
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'customAdapter')
      return {
        name: 'custom',
        target: 'node',
        platform: 'node',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
      }
    },
  }
}
```

## Route rules

`headers()`, `redirects()`, `rewrites()`, and `proxy` are keys on the config object. Their sources
are path-to-regexp patterns compiled by `route-rules.ts`, the same module every deployed build
evaluates, and held to `tests/fixtures/route-rules-conformance.json` together with the native
evaluator:

```ts
import { config } from '@ruvyxa/core/config'

export default config({
  headers: [{ source: '/api/:path*', headers: [{ key: 'cache-control', value: 'no-store' }] }],
  redirects: async () => [{ source: '/old/:path*', destination: '/new/:path*', permanent: true }],
  rewrites: { beforeFiles: [{ source: '/alias', destination: '/' }] },
  proxy: {
    matcher: ['/admin/:path*'],
    handler(request) {
      return request.headers.has('authorization')
        ? undefined
        : new Response('Unauthorized', { status: 401 })
    },
  },
})
```

`proxy.handler` returns `undefined` to continue, a `Request` to continue with (a different path is a
rewrite), or a `Response` to answer. Rules apply in a fixed order: headers, redirects, proxy,
`beforeFiles` rewrites, files, `afterFiles`, dynamic routes, `fallback`.

This package is published as ESM with generated TypeScript declarations.
