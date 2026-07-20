# Deployment

## Adapter Deployment Artifacts

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

Select an adapter in `ruvyxa.config.ts`, or pass one on the command line without touching config:

```bash
ruvyxa build --adapter vercel
```

`--adapter` accepts `node`, `bun`, `static`, `vercel`, `netlify`, or `cloudflare`, resolves the
matching `@ruvyxa/adapter-*` package from your project, and overrides `config.adapter` for that
build. If the package is not installed the build fails with `RUV2203` and the exact install command.

An adapter's post-build lifecycle runs while the build is still in the staging directory, so a
failed adapter cannot replace a previously successful `.ruvyxa/` build. Generated deploy output
lands in `.ruvyxa/deploy/<platform>/`.

Static outputs for Vercel, Netlify, and Cloudflare ship immutable cache headers
(`Cache-Control: public, max-age=31536000, immutable`) for the content-hashed `/client/*` bundles
via `config.json` routes, `netlify.toml` headers, and an `_headers` file respectively.

### Vercel static output

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { vercelAdapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: vercelAdapter(),
})
```

The adapter emits Vercel's Build Output API layout (`.vercel/output/static` and
`.vercel/output/config.json`) **at the project root** during `ruvyxa build`. On Vercel, pick the
“Other” framework preset and set the build command to your build script (for example
`npm run build`) — Vercel detects `.vercel/output/` automatically, so no output-directory
configuration is needed. Add `.vercel/` to `.gitignore`; it is generated on every build.

Pass `vercelAdapter({ projectOutput: false })` to keep the previous behavior of writing only under
`.ruvyxa/deploy/vercel/` (deploy that directory manually with the “Other” preset).

### Netlify zero-config

```ts
import { netlifyAdapter } from '@ruvyxa/adapter-netlify'

export default config({
  adapter: netlifyAdapter(),
})
```

The first `ruvyxa build` writes a `netlify.toml` at the project root with the build command and
publish directory (`.ruvyxa/deploy/netlify/publish`) preconfigured — commit it and connect the
repository to Netlify with no dashboard configuration. An existing `netlify.toml` is **never
overwritten**; your own file always wins. Pass `netlifyAdapter({ projectConfig: false })` to skip
generating it.

### Cloudflare zero-config

```ts
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default config({
  adapter: cloudflareAdapter(),
})
```

`ruvyxa build` also writes a `wrangler.jsonc` at the project root pointing `assets.directory` at the
generated static assets, so `wrangler deploy` works immediately with no dashboard configuration. An
existing project `wrangler.jsonc` is **never overwritten**. Pass
`cloudflareAdapter({ projectConfig: false })` to skip generating it.

Vercel, Netlify, and Cloudflare adapters currently emit deployable **static** output only. They
accept SSG and CSR page routes; API, SSR, ISR, and PPR routes fail with `RUV2202` rather than
shipping a deployment without a request handler.

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

After `npm run build`, the normal runtime output remains in `.ruvyxa/` and an adapter may add a
deployment directory:

```text
.ruvyxa/
├── server/         # Server-side source
├── client/         # Client bundles with manifest
├── assets/         # Static assets + WebP images
├── prerender/      # Pre-rendered HTML pages
├── manifest.json   # Route manifest
├── build.json      # Build metadata
└── deploy/         # Adapter-specific artifacts, when configured
```

For a static adapter, use its generated publish directory instead of deploying all of `.ruvyxa/`.

---

## Adapters

### Available

| Adapter                      | Target                                         |
| ---------------------------- | ---------------------------------------------- |
| `@ruvyxa/adapter-node`       | Node launcher: `.ruvyxa/deploy/node/start.mjs` |
| `@ruvyxa/adapter-bun`        | Bun launcher: `.ruvyxa/deploy/bun/start.mjs`   |
| `@ruvyxa/adapter-static`     | Static files: `.ruvyxa/static/`                |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Pages: `.ruvyxa/deploy/cloudflare/` |
| `@ruvyxa/adapter-netlify`    | Netlify static: `.ruvyxa/deploy/netlify/`      |
| `@ruvyxa/adapter-vercel`     | Vercel static: `.ruvyxa/deploy/vercel/`        |

### Usage

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { nodeAdapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: nodeAdapter(),
})
```

### Important

- An adapter's `build()` function runs both during configuration loading and during the post-build
  artifact step.
- The post-build step may create only files inside `.ruvyxa/`; its result is recorded as
  `adapterArtifacts` in `.ruvyxa/build.json`.
- Static adapters deliberately reject dynamic request handling until a platform request handler
  exists. This is a safety boundary, not a fallback.

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
import { nodeAdapter } from '@ruvyxa/adapter-node'

export default config({
  adapter: nodeAdapter(),
})
```

## Static Hosting

```bash
npm install @ruvyxa/adapter-static
# configure staticAdapter(), then:
npm run build
# deploy .ruvyxa/static/ to your static host
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
- [ ] Reverse proxy — forward `X-Forwarded-Proto` and add its exact non-loopback IP to
      `security.trustedProxyIps` when behind an HTTPS proxy

## Learn from the Demo

`examples/demo/` is an integration app containing static, dynamic, and catch-all routes; API routes;
server actions; MDX; public environment variables; external CSS; and SSR, SSG, ISR, CSR, and PPR
examples. Read its [README](../../examples/demo/README.md), run the diagnostic commands, and copy a
proven pattern before adding a new feature to your own app.
