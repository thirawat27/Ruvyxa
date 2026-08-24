<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-railway</h1>

<p align="center">
  Full-stack Railway adapter for Ruvyxa. Railway builds auto-select it through<br/>
  <code>RAILWAY_PROJECT_ID</code>; no <code>ruvyxa.config.ts</code> change or separate install is<br/>
  required.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-railway"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-railway?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-railway"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-railway?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```ts
import { railway } from '@ruvyxa/adapter-railway'
import { config } from 'ruvyxa/config'

export default config({ adapter: railway() })
```

The build emits a standalone server at `.ruvyxa/deploy/railway/server/index.mjs` and a safe
`railway.json`. Existing project configuration is never overwritten. The server honors Railway's
`PORT` and supports SSR, SSG, CSR, ISR, PPR, API routes, and native realtime.
