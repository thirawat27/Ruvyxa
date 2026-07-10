# @ruvyxa/adapter-static

Static output adapter metadata for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-static
```

## Usage

```ts
import { defineConfig } from 'ruvyxa/config'
import { staticAdapter } from '@ruvyxa/adapter-static'

export default defineConfig({
  adapter: staticAdapter(),
})
```

## Output Metadata

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

Use this adapter for static-only sites. Runtime APIs such as API routes and server actions require a
server target.
