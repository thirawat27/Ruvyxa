# Deployment

Build once, then run the production server:

```bash
ruvyxa build
ruvyxa start
```

The Node adapter describes the production output that a Node host should serve:

```ts
import { nodeAdapter } from "@ruvyxa/adapter-node"

const output = await nodeAdapter().build({
  root: ".",
  outDir: ".ruvyxa",
})
```

Output:

```json
{
  "name": "node",
  "target": "node",
  "platform": "node",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets"
}
```

First-party adapter packages expose the same contract:

```ts
import { vercelAdapter } from "@ruvyxa/adapter-vercel"
import { cloudflareAdapter } from "@ruvyxa/adapter-cloudflare"
import { netlifyAdapter } from "@ruvyxa/adapter-netlify"
import { bunAdapter } from "@ruvyxa/adapter-bun"
import { staticAdapter } from "@ruvyxa/adapter-static"
```

`ruvyxa build` emits `.ruvyxa/server`, `.ruvyxa/assets`, and BLAKE3-hashed route-level client bundles in `.ruvyxa/client`. `build.json` records the production profile, hash algorithm, output directories, and enabled runtime security defaults.

`ruvyxa start` is the supported Node runtime today; managed-host adapters describe target output for deployment integrations that consume the same build artifacts.
