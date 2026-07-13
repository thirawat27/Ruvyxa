# Deployment

## Vercel

### Setup

Use the standard npm scripts:

```json
{
  "scripts": {
    "dev": "ruvyxa dev",
    "build": "ruvyxa build",
    "start": "ruvyxa start",
    "check": "ruvyxa check"
  }
}
```

Configure Vercel:

- **Build Command**: `npm run build`
- **Output Directory**: `.ruvyxa`
- **Framework Preset**: _None_ — Ruvyxa handles everything through `npm run build`

### Adapter

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { adapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: adapter(),
  adapterOptions: {
    regions: ['iad1'],
  },
})
```

Adapters write metadata to `.ruvyxa/build.json` for deployment tooling.

### Permission Denied Error

```
node_modules/.bin/ruvyxa: Permission denied
```

This means the installed Ruvyxa launcher was published without executable permission. Upgrade to a
Ruvyxa release that includes the executable launcher.

### Node Version

Pin Node 22 for reproducible CI builds:

```json
{
  "engines": {
    "node": "22.x"
  }
}
```

---

## CI/CD

### Recommended Pipeline

```yaml
# .github/workflows/deploy.yml
- run: npm ci
- run: npx ruvyxa analyze
- run: npm run typecheck
- run: npm run check
- run: npm run build
```

### Build Artifacts

After `npm run build`, deploy the entire `.ruvyxa/` directory:

```text
.ruvyxa/
├── server/         # Server-side source
├── client/         # Client bundles with manifest
├── assets/         # Static assets + WebP images
├── prerender/      # Pre-rendered HTML pages
├── manifest.json   # Route manifest
└── build.json      # Build metadata
```

---

## Adapters

### Available

| Adapter                      | Target             |
| ---------------------------- | ------------------ |
| `@ruvyxa/adapter-node`       | Node.js server     |
| `@ruvyxa/adapter-vercel`     | Vercel serverless  |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers |
| `@ruvyxa/adapter-netlify`    | Netlify Functions  |
| `@ruvyxa/adapter-bun`        | Bun runtime        |
| `@ruvyxa/adapter-static`     | Static hosting     |

### Usage

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { adapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: adapter(),
})
```

### Important

- An adapter's `build()` function is executed while Ruvyxa loads configuration.
- Serializable `AdapterOutput` and `adapterOptions` are written to `.ruvyxa/build.json`.
- An adapter declaration alone does **not** create or publish platform functions.
- Always verify platform output, routing, and the serving model for your deployment.

---

## Self-Hosted (Node.js)

```bash
npm run build
npm run start          # serve from .ruvyxa/
```

Or use the Node adapter:

```bash
npm install @ruvyxa/adapter-node
```

```ts
import { adapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: adapter(),
})
```

## Static Hosting

```bash
npm run build -- --target static
# or set runtime: 'static' in config
# deploy .ruvyxa/ to your static host (S3, Cloudflare Pages, Netlify, etc.)
```

---

## Production Checklist

Before deploying:

- [ ] `npx ruvyxa analyze` — no errors
- [ ] `npm run typecheck` — type-safe
- [ ] `npm run check` — readiness checks pass
- [ ] `.env.example` — lists required variable names without real values
- [ ] Security headers — `security.headers: true`
- [ ] CORS origins — explicit, not wildcard
- [ ] Body limits — `security.apiLimit` and `security.actionLimit` are appropriate
- [ ] Reverse proxy — forward `X-Forwarded-Proto` when behind an HTTPS proxy

## Learn from the Demo

`examples/demo/` is an integration app containing static, dynamic, and catch-all routes; API routes;
server actions; MDX; public environment variables; external CSS; and SSR, SSG, ISR, CSR, and PPR
examples. Read its [README](../../examples/demo/README.md), run the diagnostic commands, and copy a
proven pattern before adding a new feature to your own app.
