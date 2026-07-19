# @ruvyxa/adapter-node

Node.js deployment adapter for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-node
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { nodeAdapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: nodeAdapter(),
})
```

## Deployment Artifact

```json
{
  "name": "node",
  "target": "node",
  "platform": "node",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json"
}
```

`ruvyxa build` creates `.ruvyxa/deploy/node/start.mjs`. Start it from the project root with
`node .ruvyxa/deploy/node/start.mjs`; it launches `ruvyxa start` using the installed project CLI.
Use this adapter for self-hosted Node, Docker, PM2, and other Node-compatible runtimes.
