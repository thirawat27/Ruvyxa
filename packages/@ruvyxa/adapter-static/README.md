<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-static</h1>

<p align="center">
  Static output adapter for Ruvyxa production builds.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-static"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-static?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-static"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-static?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

```bash
npm install @ruvyxa/adapter-static
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { static as staticOutput } from '@ruvyxa/adapter-static'

export default config({
  adapter: staticOutput(),
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
different directory **inside** `.ruvyxa`, for example `staticOutput({ outputDir: 'public' })`.

Only SSG and CSR page routes are supported. API routes and server-rendered, ISR, or PPR routes fail
the build with `RUV2202`, rather than producing an incomplete deployment.
