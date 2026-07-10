# @ruvyxa/adapter-cloudflare

Cloudflare edge deployment adapter metadata for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-cloudflare
```

## Usage

```ts
import { defineConfig } from 'ruvyxa/config'
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default defineConfig({
  adapter: cloudflareAdapter(),
})
```

## Output Metadata

```json
{
  "name": "cloudflare",
  "target": "edge",
  "platform": "cloudflare",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json",
  "configFiles": ["wrangler.toml"]
}
```

Use this adapter for Cloudflare Workers and Pages-style edge deployments.
