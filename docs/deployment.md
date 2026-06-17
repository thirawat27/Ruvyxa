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
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets"
}
```

For now, `ruvyxa start` is the supported Node runtime. Additional adapters for serverless, edge, static, and managed hosts can use the same `Adapter` interface.
