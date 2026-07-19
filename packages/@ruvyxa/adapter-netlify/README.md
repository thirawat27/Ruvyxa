# @ruvyxa/adapter-netlify

Netlify static deployment adapter for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-netlify
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { netlifyAdapter } from '@ruvyxa/adapter-netlify'

export default config({
  adapter: netlifyAdapter(),
})
```

## Deployment Artifact

```json
{
  "name": "netlify",
  "target": "serverless",
  "platform": "netlify",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json",
  "functionsDir": ".ruvyxa/netlify/functions",
  "configFiles": ["netlify.toml"]
}
```

`ruvyxa build` creates `.ruvyxa/deploy/netlify/` with `publish/` and `netlify.toml`. Deploy that
directory on Netlify.

This adapter safely supports static SSG/CSR output only. It rejects API, SSR, ISR, and PPR routes
with `RUV2202`; no Netlify Function handler is generated.
