<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
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

## Client identity behind a proxy

The generated server believes `X-Forwarded-For` and `X-Real-IP` only from a connection whose peer is
loopback or listed in `security.trustedProxyIps`. From any other peer both headers are dropped
before the request is routed, because nothing in front of the server overwrote them and one caller
rotating a value would otherwise collect a fresh bucket per request from the built-in `rate`
middleware, the server-action rate limiter, and the action replay quota.

Behind a reverse proxy that is not on the same host — nginx or Traefik in another container, an
ingress controller, a service mesh — list its address or range, or every visitor is counted as one
client:

```ts
export default config({
  security: { trustedProxyIps: ['10.0.0.0/8'] },
})
```
