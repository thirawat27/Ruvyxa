# @ruvyxa/adapter-vercel

Vercel deployment adapter for Ruvyxa production builds.

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

`ruvyxa build` creates `.ruvyxa/deploy/vercel/.vercel/output/`, using Vercel's Build Output API
layout. Deploy `.ruvyxa/deploy/vercel/`.

This adapter supports SSR, API, ISR, PPR, SSG, and CSR routes via the serverless runtime. Static
assets and pre-rendered pages are served through Vercel's static output; dynamic routes are handled
by serverless functions.
