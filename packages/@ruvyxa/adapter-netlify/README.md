# @ruvyxa/adapter-netlify

Netlify deployment adapter for Ruvyxa production builds.

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

`ruvyxa build` creates `.ruvyxa/deploy/netlify/` with `publish/`, Netlify Functions handlers, and
`netlify.toml`. Deploy that directory on Netlify.

This adapter supports SSR, API, ISR, PPR, SSG, and CSR routes via the serverless runtime. Static
assets and pre-rendered pages are served through Netlify's publish directory; dynamic routes are
handled by Netlify Functions.
