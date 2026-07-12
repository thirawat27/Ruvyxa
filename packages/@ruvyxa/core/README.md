# @ruvyxa/core

Typed primitives shared by the Ruvyxa runtime, CLI package, and first-party adapters.

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
  PluginContext,
  RuvyxaConfig,
  RuvyxaPlugin,
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
    jsx: 'classic',
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

## Plugin Contract

Custom build plugins use the exported `RuvyxaPlugin`, `PluginContext`, and `TransformResult` types.
During `ruvyxa build`, `resolveId` and `transform` hooks from `ruvyxa.config.ts` are bridged into
the native bundler pipeline:

```ts
import type { PluginContext, RuvyxaPlugin, TransformResult } from '@ruvyxa/core'

export function bannerPlugin(): RuvyxaPlugin {
  return {
    name: 'banner',
    transform(code: string, id: string, ctx: PluginContext): TransformResult | null {
      if (ctx.environment !== 'client' || !id.endsWith('.tsx')) {
        return null
      }

      return {
        code: `/* client bundle */\n${code}`,
      }
    },
  }
}
```

This package is published as ESM with generated TypeScript declarations.
