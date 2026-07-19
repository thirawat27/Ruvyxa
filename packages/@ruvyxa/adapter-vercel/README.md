# @ruvyxa/adapter-vercel

Vercel static deployment adapter for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-vercel
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { vercelAdapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: vercelAdapter(),
})
```

## Deployment Artifact

```json
{
  "name": "vercel",
  "target": "serverless",
  "platform": "vercel",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json",
  "functionsDir": ".ruvyxa/functions",
  "configFiles": ["vercel.json"]
}
```

`ruvyxa build` creates `.ruvyxa/deploy/vercel/.vercel/output/`, using Vercel's static Build Output
layout. Deploy `.ruvyxa/deploy/vercel/`.

This adapter safely supports static SSG/CSR output only. It rejects API, SSR, ISR, and PPR routes
with `RUV2202`; no Vercel serverless function handler is generated.
