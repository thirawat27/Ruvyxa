<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-cloudflare</h1>

<p align="center">
  Cloudflare Workers deployment adapter for Ruvyxa production builds.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-cloudflare"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-cloudflare?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-cloudflare"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-cloudflare?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

```bash
npm install @ruvyxa/adapter-cloudflare
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { cloudflare } from '@ruvyxa/adapter-cloudflare'

export default config({
  adapter: cloudflare(),
})
```

## Deployment Artifact

```json
{
  "name": "cloudflare",
  "target": "edge",
  "platform": "cloudflare",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json",
  "configFiles": ["wrangler.jsonc"]
}
```

`ruvyxa build` creates `.ruvyxa/deploy/cloudflare/` with a Workers handler, `assets/`, and
`wrangler.jsonc`. Deploy that directory with Wrangler/Cloudflare Workers.

This adapter supports SSR, API, SSG, and CSR routes via the Edge runtime (`--target edge`). ISR and
PPR are rejected because the assets binding is read-only and the adapter does not configure a
persistent KV or Durable Object cache. Static assets are served through the assets binding; dynamic
routes use compiled edge modules loaded from a static registry in the Worker bundle.

Validated built-in middleware is embedded into the Worker and runs with Fetch/Web APIs—CORS, rate
limiting, timing, logging, and custom response headers therefore match the native server without
Node.js polyfills. When `image.onDemand` is enabled, the Worker delegates same-origin image
transforms to Cloudflare's `fetch(..., { cf: { image } })`; the Cloudflare account must have image
transformations enabled.
