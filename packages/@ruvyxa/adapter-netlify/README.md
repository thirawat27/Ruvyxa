<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-netlify</h1>

<p align="center">
  Netlify deployment adapter for Ruvyxa production builds.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-netlify"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-netlify?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-netlify"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-netlify?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

```bash
npm install @ruvyxa/adapter-netlify
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { netlify } from '@ruvyxa/adapter-netlify'

export default config({
  adapter: netlify(),
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
handled by Netlify Functions. Function output contains a compiled `.mjs` static route registry. ISR
checks file age against `revalidate` and coalesces concurrent stale refreshes within a warm function
instance.
