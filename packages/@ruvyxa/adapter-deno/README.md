<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-deno</h1>

<p align="center">
  Self-contained Deno runtime adapter for Ruvyxa production builds.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-deno"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-deno?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-deno"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-deno?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```bash
npm install @ruvyxa/adapter-deno
```

```ts
import { config } from 'ruvyxa/config'
import { deno } from '@ruvyxa/adapter-deno'

export default config({ adapter: deno() })
```

Build with `ruvyxa build`, copy `.ruvyxa/deploy/deno/`, then run:

```bash
deno run -A --no-prompt server/index.mjs
```
