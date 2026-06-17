# Deployment

Ruvyxa builds your app into a self-contained `.ruvyxa/` directory. Deploy it anywhere Node runs, or use a platform adapter for managed hosting.

---

## Build for Production

```bash
ruvyxa build
```

This produces:

```
.ruvyxa/
├── server/       # Server-side route source
├── client/       # BLAKE3-hashed hydration bundles
├── assets/       # Static files from public/
├── manifest.json # Route manifest
└── build.json    # Build metadata
```

---

## Self-Hosted (Node)

The simplest deployment: build and run.

```bash
ruvyxa build
ruvyxa start --port 3000
```

`ruvyxa start` serves the production build using the same route matching, SSR, and security headers as the dev server. It reads from `.ruvyxa/server/app` and serves static assets from `.ruvyxa/assets`.

---

## Adapters

Adapters describe the build output in a format each platform expects. They all follow the same contract:

```ts
import { defineConfig } from "ruvyxa/config"
import { nodeAdapter } from "@ruvyxa/adapter-node"

export default defineConfig({
  adapter: nodeAdapter(),
})
```

### Available Adapters

| Package | Platform |
|---------|----------|
| `@ruvyxa/adapter-node` | Node.js (self-hosted, Docker, PM2) |
| `@ruvyxa/adapter-vercel` | Vercel Functions |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers / Pages |
| `@ruvyxa/adapter-netlify` | Netlify Functions |
| `@ruvyxa/adapter-bun` | Bun runtime |
| `@ruvyxa/adapter-static` | Static site export (no server) |

### Install an adapter

```bash
npm install @ruvyxa/adapter-vercel
```

---

## Node Adapter

The default adapter. Produces output ready for any Node.js hosting environment.

```ts
import { nodeAdapter } from "@ruvyxa/adapter-node"

const output = await nodeAdapter().build({
  root: ".",
  outDir: ".ruvyxa",
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
FROM node:20-alpine AS builder
WORKDIR /app
COPY . .
RUN npm install && npx ruvyxa build

FROM node:20-alpine
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
import { vercelAdapter } from "@ruvyxa/adapter-vercel"

export default defineConfig({
  adapter: vercelAdapter(),
})
```

Deploy with the Vercel CLI or Git integration. The adapter outputs Vercel-compatible serverless function bundles and static assets.

---

## Cloudflare Workers

```ts
import { cloudflareAdapter } from "@ruvyxa/adapter-cloudflare"

export default defineConfig({
  adapter: cloudflareAdapter(),
})
```

Deploy with `wrangler deploy`. The adapter targets the Workers runtime.

---

## Netlify

```ts
import { netlifyAdapter } from "@ruvyxa/adapter-netlify"

export default defineConfig({
  adapter: netlifyAdapter(),
})
```

Deploy with the Netlify CLI or Git integration.

---

## Bun

```ts
import { bunAdapter } from "@ruvyxa/adapter-bun"

export default defineConfig({
  adapter: bunAdapter(),
})
```

Run with `bun run .ruvyxa/server/app`.

---

## Static Export

For sites that don't need server-side rendering at request time:

```ts
import { staticAdapter } from "@ruvyxa/adapter-static"

export default defineConfig({
  adapter: staticAdapter(),
})
```

This pre-renders all pages at build time and outputs plain HTML + JS + CSS. Deploy to any static host (GitHub Pages, S3, Cloudflare Pages static, etc.).

> Note: Dynamic routes with runtime params, API routes, and server actions are not available in static mode.

---

## Environment Variables in Production

Set environment variables using your platform's standard method (`.env` file, platform dashboard, Docker env, etc.). Ruvyxa loads `.env` and `.env.local` at server startup.

Remember:
- `RUVYXA_PUBLIC_*` — available in both server and client code
- All other variables — server-only (SSR, loaders, actions, API routes)

---

## Build Metadata

`build.json` records useful information about the build:

```json
{
  "framework": "Ruvyxa",
  "version": "1.0.1",
  "target": "node",
  "profile": "production",
  "routes": 5,
  "hashAlgorithm": "blake3-128",
  "security": {
    "actionBodyLimitBytes": 65536,
    "sameOriginActions": true,
    "fetchMetadataActions": true,
    "securityHeaders": true
  }
}
```

---

## Related

- [Getting Started](getting-started.md) — initial project setup
- [Production Readiness](production-readiness.md) — release checklist
- [Performance](performance.md) — build benchmarks and optimization
