<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-render</h1>

<p align="center">
  Full-stack Render adapter for Ruvyxa. Render builds auto-select it through<br/>
  <code>RENDER=true</code>.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-render"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-render?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-render"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-render?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```ts
import { render } from '@ruvyxa/adapter-render'
import { config } from 'ruvyxa/config'

export default config({ adapter: render() })
```

The build emits `.ruvyxa/deploy/render/server/index.mjs` plus a Render Blueprint. Existing
`render.yaml` files are never overwritten. The server honors `PORT`, binds to `0.0.0.0`, and
supports SSR, SSG, CSR, ISR, PPR, API routes, and native realtime.
