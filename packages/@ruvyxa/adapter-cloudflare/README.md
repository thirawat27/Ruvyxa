# @ruvyxa/adapter-cloudflare

Cloudflare Workers deployment adapter for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-cloudflare
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default config({
  adapter: cloudflareAdapter(),
})
```

## Deployment Artifact

```json
{
  "name": "cloudflare",
  "target": "edge",
  "platform": "cloudflare",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json",
  "configFiles": ["wrangler.jsonc"]
}
```

`ruvyxa build` creates `.ruvyxa/deploy/cloudflare/` with a Workers handler, `assets/`, and
`wrangler.jsonc`. Deploy that directory with Wrangler/Cloudflare Workers.

This adapter supports SSR, API, ISR, PPR, SSG, and CSR routes via the Edge runtime
(`--target edge`). Static assets (client bundles, pre-rendered pages) are served through
Cloudflare's assets binding; dynamic routes are handled by the Worker.
