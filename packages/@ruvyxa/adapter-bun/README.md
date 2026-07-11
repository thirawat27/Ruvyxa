# @ruvyxa/adapter-bun

Bun runtime adapter metadata for Ruvyxa production builds.

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

## Output Metadata

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

Use this adapter for Bun-compatible hosting targets that run the production server output.
