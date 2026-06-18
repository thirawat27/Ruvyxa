# @ruvyxa/core

Typed primitives shared by the Ruvyxa runtime, CLI package, and first-party adapters.

## Install

Most apps import these APIs through `ruvyxa`. Install this package directly only when writing adapters or low-level integrations.

```bash
npm install @ruvyxa/core
```

## Exports

```ts
import { defineConfig } from "@ruvyxa/core/config"
import { action, cache, invalidateCache, json, loader, notFound, redirect } from "@ruvyxa/core/server"
import type { Adapter, AdapterOutput, BuildContext, RuvyxaConfig } from "@ruvyxa/core"
```

## Server APIs

### Loader with caching

```ts
import { loader } from "@ruvyxa/core/server"

export const getPosts = loader(async ({ cache }) => {
  return cache("posts").ttl("5m").get(async () => {
    return await db.posts.findMany()
  })
})
```

### Action with validation

```ts
import { action } from "@ruvyxa/core/server"

export const createPost = action
  .input({ parse: (v) => ({ title: String(v.title) }) })
  .handler(async ({ input, invalidate }) => {
    invalidate("posts")
    return await db.posts.create(input)
  })
```

### Cache utility

The `cache()` function provides real in-memory TTL caching:

```ts
import { cache, invalidateCache } from "@ruvyxa/core/server"

// Cache with TTL (supports "30s", "5m", "1h", "1d")
const data = await cache("key").ttl("10m").get(async () => fetchExpensiveData())

// Invalidate by key or prefix
invalidateCache("key")       // exact match
invalidateCache("posts")     // also clears "posts:123"
invalidateCache()            // clear all
```

## Config Shape

```ts
import { defineConfig } from "@ruvyxa/core/config"

export default defineConfig({
  appDir: "app",
  outDir: ".ruvyxa",
  server: {
    host: "localhost",
    port: 3000,
  },
  build: {
    minify: true,
    sourcemap: false,
    splitStrategy: "route",
    parallelism: 4,
  },
  cache: {
    routeManifest: true,
    css: true,
  },
})
```

## Adapter Contract

Adapters return metadata describing how a platform should consume `.ruvyxa/` output:

```ts
import type { Adapter } from "@ruvyxa/core"

export function customAdapter(): Adapter {
  return {
    name: "custom",
    target: "node",
    build(ctx) {
      return {
        name: "custom",
        target: "node",
        platform: "node",
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
      }
    },
  }
}
```

This package is published as ESM with generated TypeScript declarations.
