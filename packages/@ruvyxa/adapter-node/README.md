# @ruvyxa/adapter-node

Node.js deployment adapter metadata for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-node
```

## Usage

```ts
import { defineConfig } from "ruvyxa/config"
import { nodeAdapter } from "@ruvyxa/adapter-node"

export default defineConfig({
  adapter: nodeAdapter(),
})
```

## Output Metadata

```json
{
  "name": "node",
  "target": "node",
  "platform": "node",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets"
}
```

Use this adapter for self-hosted Node, Docker, PM2, and other Node-compatible runtimes.
