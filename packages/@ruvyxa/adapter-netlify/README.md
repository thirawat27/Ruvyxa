# @ruvyxa/adapter-netlify

Netlify functions deployment adapter metadata for Ruvyxa production builds.

## Install

```bash
npm install @ruvyxa/adapter-netlify
```

## Usage

```ts
import { defineConfig } from "ruvyxa/config"
import { netlifyAdapter } from "@ruvyxa/adapter-netlify"

export default defineConfig({
  adapter: netlifyAdapter(),
})
```

## Output Metadata

```json
{
  "name": "netlify",
  "target": "serverless",
  "platform": "netlify",
  "entry": ".ruvyxa/server/app",
  "assetsDir": ".ruvyxa/assets",
  "functionsDir": ".ruvyxa/netlify/functions",
  "configFiles": ["netlify.toml"]
}
```

Use this adapter when preparing function and static asset output for Netlify.
