# Deployment

Ruvyxa builds your app into a self-contained `.ruvyxa/` directory. Deploy it anywhere Node runs, or
use a platform adapter for managed hosting.

---

## Build for Production

```bash
ruvyxa build
```

This produces:

```
.ruvyxa/
├── server/       # Server-side route source (app/, components/, server/)
├── client/       # BLAKE3-hashed hydration bundles
│   └── manifest.json
├── assets/       # Static files from public/
├── prerender/    # Pre-rendered SSG/ISR/PPR/CSR HTML + manifest.json
├── manifest.json # Route manifest for production server
└── build.json    # Build metadata and config snapshot
```

---

## Self-Hosted (Node)

```bash
ruvyxa build
ruvyxa start --port 3000
```

The production server serves from `.ruvyxa/server/app`, with static assets from `.ruvyxa/assets`,
pre-rendered HTML from `.ruvyxa/prerender`, and client bundles from `.ruvyxa/client`. All responses
include default security headers and blake3-based ETags.

---

## Adapters

Adapters describe the build output in a format each platform expects:

```ts
import { defineConfig } from 'ruvyxa/config'
import { nodeAdapter } from '@ruvyxa/adapter-node'

export default defineConfig({
  adapter: nodeAdapter(),
})
```

### Available Adapters

| Package                      | Platform                           |
| ---------------------------- | ---------------------------------- |
| `@ruvyxa/adapter-node`       | Node.js (self-hosted, Docker, PM2) |
| `@ruvyxa/adapter-vercel`     | Vercel Functions                   |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers / Pages         |
| `@ruvyxa/adapter-netlify`    | Netlify Functions                  |
| `@ruvyxa/adapter-bun`        | Bun runtime                        |
| `@ruvyxa/adapter-static`     | Static site export (no server)     |

### Install an adapter

```bash
npm install @ruvyxa/adapter-vercel
```

---

## Node Adapter

The default adapter for any Node.js hosting environment:

```ts
import { nodeAdapter } from '@ruvyxa/adapter-node'

export default defineConfig({
  adapter: nodeAdapter({
    entry: '.ruvyxa/server/app', // optional, defaults to this
  }),
})
```

Output metadata:

```json
{
  "name": "node",
  "target": "node",
  "platform": "node",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets"
}
```

### Docker example

```dockerfile
FROM node:22-alpine AS builder
WORKDIR /app
COPY . .
RUN npm install && npx ruvyxa build

FROM node:22-alpine
WORKDIR /app
COPY --from=builder /app/.ruvyxa .ruvyxa
COPY --from=builder /app/node_modules node_modules
COPY --from=builder /app/package.json package.json
EXPOSE 3000
CMD ["npx", "ruvyxa", "start", "--port", "3000"]
```

---

## Vercel

```ts
import { vercelAdapter } from '@ruvyxa/adapter-vercel'

export default defineConfig({
  adapter: vercelAdapter({
    functionsDir: '.ruvyxa/functions', // optional
  }),
})
```

Deploys via the Vercel CLI or Git integration. Produces serverless function bundles and static
assets. Output includes `vercel.json`.

---

## Cloudflare Workers

```ts
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default defineConfig({
  adapter: cloudflareAdapter({
    workerEntry: '.ruvyxa/server/app', // optional
  }),
})
```

Deploys with `wrangler deploy`. Targets the Workers runtime. Output includes `wrangler.toml`.

---

## Netlify

```ts
import { netlifyAdapter } from '@ruvyxa/adapter-netlify'

export default defineConfig({
  adapter: netlifyAdapter({
    functionsDir: '.ruvyxa/netlify/functions', // optional
  }),
})
```

Deploys with the Netlify CLI or Git integration. Output includes `netlify.toml`.

---

## Bun

```ts
import { bunAdapter } from '@ruvyxa/adapter-bun'

export default defineConfig({
  adapter: bunAdapter(),
})
```

Run with `bun run .ruvyxa/server/app`.

---

## Static Export

For sites that don't need server-side rendering at request time:

```ts
import { staticAdapter } from '@ruvyxa/adapter-static'

export default defineConfig({
  adapter: staticAdapter({
    outputDir: '.ruvyxa/static', // optional
  }),
})
```

Pre-renders all pages at build time and outputs static HTML + JS + CSS. Deploy to any static host
(GitHub Pages, S3, Cloudflare Pages static, etc.).

> Note: Dynamic routes with runtime params, API routes, and server actions are not available in
> static mode.

---

## Environment Variables in Production

Set environment variables using your platform's standard method. Ruvyxa loads `.env` and
`.env.local` at server startup.

- `RUVYXA_PUBLIC_*` — available in both server and client code
- All other variables — server-only (SSR, loaders, actions, API routes)

---

## Build Metadata

`build.json` records build information:

```json
{
  "framework": "Ruvyxa",
  "version": "1.0.5",
  "target": "node",
  "profile": "production",
  "routes": 5,
  "serverDir": "server",
  "clientDir": "client",
  "assetsDir": "assets",
  "hashAlgorithm": "blake3-128",
  "createdAtUnix": 1712345678,
  "security": {
    "actionBodyLimitBytes": 65536,
    "sameOriginActions": true,
    "fetchMetadataActions": true,
    "securityHeaders": true
  },
  "build": {
    "minify": true,
    "sourcemap": false,
    "treeShaking": true,
    "splitStrategy": "route",
    "parallelism": 4
  },
  "rendering": {
    "prerendered": 3,
    "routes": [
      { "path": "/static-page", "strategy": "ssg", "revalidate": null },
      { "path": "/isr-page", "strategy": "isr", "revalidate": 60 }
    ]
  }
}
```

`.ruvyxa/client/manifest.json` contains per-route bundle metrics (module count, output bytes,
estimated gzip bytes, cache hits, tree-shaken modules). When `build.emitChunkManifest` is enabled,
Ruvyxa also writes `.ruvyxa/client/chunk-manifest.json` with dynamic import chunk info.

---

## Related

- [Getting Started](getting-started.md) — initial project setup
- [Production Readiness](production-readiness.md) — release checklist
- [Performance](performance.md) — build benchmarks and optimization
