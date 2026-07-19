# @ruvyxa/adapter-bun

Bun runtime adapter for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-bun
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { bunAdapter } from '@ruvyxa/adapter-bun'

export default config({
  adapter: bunAdapter(),
})
```

## Deployment Artifact

```json
{
  "name": "bun",
  "target": "node",
  "platform": "bun",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json"
}
```

`ruvyxa build` creates `.ruvyxa/deploy/bun/start.mjs`. Start it from the project root with
`bun .ruvyxa/deploy/bun/start.mjs`; it launches `ruvyxa start` through Bun.
