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

`--adapter` accepts two kinds of value, and overrides `config.adapter` for that build only:

**1. Built-in names** — `node`, `bun`, `static`, `vercel`, `netlify`, `cloudflare`

All six official adapters ship as dependencies of the `ruvyxa` package itself, so these names work
with `ruvyxa` alone installed — no extra `pnpm add @ruvyxa/adapter-*` needed:

```bash
ruvyxa build --adapter node      # standalone server, ready to run
ruvyxa build --adapter netlify   # .netlify/v1/ + deploy dir, ready to deploy
```

Install an individual `@ruvyxa/adapter-*` package only when you need to pass options through
`ruvyxa.config.ts`, such as `netlifyAdapter({ projectConfig: true })` — the `--adapter` flag always
uses the adapter's defaults.

**2. Any adapter package name** — opens the ecosystem to platforms without an official adapter (Deno
Deploy, Fastly, AWS Lambda, and so on):

```bash
ruvyxa build --adapter @acme/ruvyxa-adapter-deno   # scoped names are used verbatim
ruvyxa build --adapter fastly                       # short names try the conventions
```

Resolution order:

1. A scoped name (`@scope/name`) or one containing `/` resolves as that exact package.
2. A short name tries `@ruvyxa/adapter-<name>`, then `ruvyxa-adapter-<name>`, then `<name>`.
3. Each candidate resolves from your project's `node_modules` first, then falls back to the copies
   bundled with `ruvyxa` — **a project-installed version always wins**, so you can pin an adapter
   version per project.

When no candidate resolves, the build fails with `RUV2203` listing every package name that was
tried, so the missing install is obvious.

An adapter package has a single contract: its default export must be a factory function returning an
object matching the `Adapter` interface from `@ruvyxa/core` (`name`, `target`, `supports?`,
`build(ctx)`) — exactly what every official adapter does:

```ts
// ruvyxa-adapter-fastly/src/index.ts
import type { Adapter, BuildContext } from '@ruvyxa/core'

export default function fastlyAdapter(): Adapter {
  return {
    name: 'fastly',
    target: 'edge',
    supports: ['ssr', 'ssg', 'csr', 'api'],
    build(ctx: BuildContext) {
      return {
        name: 'fastly',
        target: 'edge',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        artifacts: [/* ... */],
      }
    },
  }
}
```

### Zero-config platform detection

When neither `config.adapter` nor `--adapter` selects an adapter, `ruvyxa build` detects the hosting
platform from its build environment and picks the matching adapter automatically:

| Environment variable | Adapter      |
| -------------------- | ------------ |
| `VERCEL`             | `vercel`     |
| `NETLIFY`            | `netlify`    |
| `CF_PAGES`           | `cloudflare` |

Set `RUVYXA_ADAPTER=<name>` to override detection, or set it to a specific adapter on any other CI.
A configured adapter always wins over detection.

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

### Netlify

```ts
import { netlifyAdapter } from '@ruvyxa/adapter-netlify'

export default config({
  adapter: netlifyAdapter(),
})
```

No file is written at your project root. `ruvyxa build` emits Netlify's Frameworks API directory
(`.netlify/v1/`, a gitignored build artifact) containing the SSR/API function and the immutable
cache headers — Netlify picks it up automatically on deploy. One-time setup in the Netlify
dashboard: set **Build command** to `npm run build` and **Publish directory** to
`.ruvyxa/deploy/netlify/publish`.

Prefer a committed config file instead? Pass `netlifyAdapter({ projectConfig: true })` to generate a
project-root `netlify.toml` (with project-relative paths) on the next build; an existing
`netlify.toml` is **never overwritten**. Pass `frameworksApi: false` to skip the `.netlify/v1/`
output.

### Cloudflare

```ts
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default config({
  adapter: cloudflareAdapter(),
})
```

No file is written at your project root. The deploy directory is self-sufficient — deploy it
directly:

```bash
npx wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc
```

Prefer a committed root config? Pass `cloudflareAdapter({ projectConfig: true })` to generate a
project-root `wrangler.jsonc` (with project-relative paths); an existing `wrangler.jsonc` is **never
overwritten**.

Vercel, Netlify, and Cloudflare adapters now support **full server rendering**:

| Strategy | Vercel | Netlify | Cloudflare |
| -------- | ------ | ------- | ---------- |
| SSG      | Yes    | Yes     | Yes        |
| CSR      | Yes    | Yes     | Yes        |
| SSR      | Yes    | Yes     | Yes        |
| API      | Yes    | Yes     | Yes        |
| ISR      | Yes    | Yes     | No*        |
| PPR      | Yes    | Yes     | No*        |

\* Cloudflare Workers lack persistent server-side storage for ISR cache. ISR and PPR routes are
rejected with `RUV2210` on Cloudflare. Use KV or Durable Objects bindings manually if needed.

Static-only deployments (SSG/CSR pages without API or SSR routes) continue to work exactly as
before. The adapters emit both static assets and a serverless function; platforms serve static files
directly and forward unmatched requests to the function handler.

Function output contains a compiled `.mjs` static route registry bundle, not raw TypeScript/TSX.
This makes the emitted artifact executable as-is and lets Wrangler discover edge modules during
bundling. On Vercel and Netlify, ISR cache age is checked against `revalidate`; only stale entries
regenerate, and concurrent stale hits are coalesced within a warm function instance.

### Permission Denied Error

```
node_modules/.bin/ruvyxa: Permission denied
```

This means the installed Ruvyxa launcher was published without executable permission. Upgrade to a
Ruvyxa release that includes the executable launcher.

### GLIBC Version Error

```
ruvyxa: /lib64/libc.so.6: version `GLIBC_2.39' not found
```

Ruvyxa releases before 1.0.19 shipped dynamically linked Linux binaries that required the build
machine's glibc, which broke on hosts with an older glibc (for example Vercel's Amazon Linux build
image). Since 1.0.19 the Linux CLI binaries are fully static musl builds and run on any Linux —
upgrade the `ruvyxa` package to fix this error.

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

| Adapter                      | Target                                                    |
| ---------------------------- | --------------------------------------------------------- |
| `@ruvyxa/adapter-node`       | Standalone server: `.ruvyxa/deploy/node/server/index.mjs` |
| `@ruvyxa/adapter-bun`        | Bun launcher: `.ruvyxa/deploy/bun/start.mjs`              |
| `@ruvyxa/adapter-static`     | Static files: `.ruvyxa/static/`                           |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers: `.ruvyxa/deploy/cloudflare/`          |
| `@ruvyxa/adapter-netlify`    | Netlify functions + static: `.netlify/v1/` + deploy dir   |
| `@ruvyxa/adapter-vercel`     | Vercel Build Output API: `.vercel/output/`                |

All official adapters are bundled with the `ruvyxa` package — `--adapter <name>` and platform
auto-detection work without installing anything. Install the individual `@ruvyxa/adapter-*` package
only when you need to pass adapter options in `ruvyxa.config.ts`. Third-party adapter packages (any
package exporting an adapter factory as its default export) work the same way via
`--adapter <package-name>` or `config.adapter`.

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

Or build a standalone server that runs without the ruvyxa CLI at runtime:

```bash
ruvyxa build --adapter node
node .ruvyxa/deploy/node/server/index.mjs
```

The `deploy/node/` directory is self-contained (server + `public/` assets). Copy it into a Docker
image, a VPS, PM2, systemd, or any PaaS (Render, Railway, Fly.io, Heroku) and run the same command —
no `node_modules` and no native binary needed at runtime. The server honors `PORT` (default 3000)
and `HOST` (default 0.0.0.0), and supports SSR, API, ISR, PPR, SSG, and CSR.

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
