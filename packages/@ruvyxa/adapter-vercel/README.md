# @ruvyxa/adapter-vercel

Vercel serverless deployment adapter metadata for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-vercel
```

## Usage

```ts
import { defineConfig } from "ruvyxa/config"
import { vercelAdapter } from "@ruvyxa/adapter-vercel"

export default defineConfig({
  adapter: vercelAdapter(),
})
```

## Output Metadata

```json
{
  "name": "vercel",
  "target": "serverless",
  "platform": "vercel",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "functionsDir": ".ruvyxa/functions",
  "configFiles": ["vercel.json"]
}
```

Use this adapter when preparing Ruvyxa output for Vercel-style function and asset layouts.
