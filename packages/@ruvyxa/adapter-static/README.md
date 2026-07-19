# @ruvyxa/adapter-static

Static output adapter for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-static
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { staticAdapter } from '@ruvyxa/adapter-static'

export default config({
  adapter: staticAdapter(),
})
```

## Deployment Artifact

```json
{
  "name": "static",
  "target": "static",
  "platform": "static",
  "entry": ".ruvyxa/static",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json"
}
```

`ruvyxa build` copies publishable files to `.ruvyxa/static/`. Configure `outputDir` to choose a
different directory **inside** `.ruvyxa`, for example `staticAdapter({ outputDir: 'public' })`.

Only SSG and CSR page routes are supported. API routes and server-rendered, ISR, or PPR routes fail
the build with `RUV2202`, rather than producing an incomplete deployment.
