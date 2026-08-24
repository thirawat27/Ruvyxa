<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-vercel</h1>

<p align="center">
  Vercel deployment adapter for Ruvyxa production builds.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-vercel"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-vercel?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-vercel"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-vercel?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

## Install

```bash
npm install @ruvyxa/adapter-vercel
```

## Usage

```ts
import { config } from 'ruvyxa/config'
import { vercel } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: vercel(),
})
```

## Deployment Artifact

```json
{
  "name": "vercel",
  "target": "serverless",
  "platform": "vercel",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "clientDir": ".ruvyxa/client",
  "chunkManifest": ".ruvyxa/client/chunk-manifest.json",
  "functionsDir": ".ruvyxa/functions",
  "configFiles": ["vercel.json"]
}
```

`ruvyxa build` creates `.ruvyxa/deploy/vercel/.vercel/output/`, using Vercel's Build Output API
layout. Deploy `.ruvyxa/deploy/vercel/`.

This adapter supports SSR, API, ISR, PPR, SSG, and CSR routes via the serverless runtime. Static
assets and pre-rendered pages are served through Vercel's static output; dynamic routes are handled
by serverless functions. Function output contains a compiled `.mjs` static route registry. ISR
checks file age against `revalidate` and coalesces concurrent stale refreshes within a warm function
instance.

Use `vercel({ edge: true })` to emit a Web-standard Edge Function instead. Edge mode supports SSR,
SSG, CSR, and API routes and embeds the validated built-in middleware policy without Node.js
imports. ISR and PPR are rejected in Edge mode because this adapter's ISR cache requires the
writable Node.js temporary directory. Do not combine `edge: true` with `runtime` or `maxDuration`.

When `image.onDemand` is enabled, the adapter emits the Build Output API `images` allowlist and
forwards Ruvyxa's validated same-origin image request to Vercel's native `/_vercel/image` service.
Configured responsive widths are bounded by `image.onDemand.maxWidth`; remote domains remain
disabled.
