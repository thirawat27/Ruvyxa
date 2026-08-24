<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-bun</h1>

<p align="center">
  Bun runtime adapter for Ruvyxa production builds.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-bun"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-bun?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-bun"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-bun?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

```bash
npm install @ruvyxa/adapter-bun
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { bun } from '@ruvyxa/adapter-bun'

export default config({
  adapter: bun(),
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

`ruvyxa build` creates a self-contained server. Run it without the Ruvyxa CLI:

```bash
bun .ruvyxa/deploy/bun/server/index.mjs
```

The generated server streams SSR/API response bodies and honors `PORT` and `HOST`.
