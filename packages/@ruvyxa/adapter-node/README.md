<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-node</h1>

<p align="center">
  Node.js deployment adapter for Ruvyxa production builds.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-node"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-node?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-node"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-node?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

```bash
npm install @ruvyxa/adapter-node
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { node } from '@ruvyxa/adapter-node'

export default config({
  adapter: node(),
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
