<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-aws</h1>

<p align="center">
  Full-stack AWS adapter targeting the official Amplify Hosting deployment specification. Amplify<br/>
  builds auto-select it through <code>AWS_APP_ID</code> and receive<br/>
  <code>.amplify-hosting/static</code>, <code>.amplify-hosting/compute/default</code>, and<br/>
  <code>deploy-manifest.json</code>.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-aws"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-aws?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-aws"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-aws?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```ts
import { aws } from '@ruvyxa/adapter-aws'
import { config } from 'ruvyxa/config'

export default config({ adapter: aws() })
```

The compute server listens on Amplify's required port 3000, stores runtime ISR refreshes under
`/tmp`, and supports SSR, SSG, CSR, ISR, PPR, and API routes. AWS credentials and Amplify app
creation remain AWS responsibilities.
