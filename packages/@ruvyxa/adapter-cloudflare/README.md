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

This adapter supports SSR, API, SSG, and CSR routes via the Edge runtime (`--target edge`). ISR and
PPR are rejected because the assets binding is read-only and the adapter does not configure a
persistent KV or Durable Object cache. Static assets are served through the assets binding; dynamic
routes use compiled edge modules loaded from a static registry in the Worker bundle.
