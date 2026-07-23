# Deployment

> 🟢 **Quick Deploy is beginner friendly** · ⏱️ ~8 min read (2 min for Quick Deploy alone)
>
> **You'll learn:** put your app online in 2-3 steps on any platform, what an adapter does, and — if
> you want it — the advanced adapter system at the very end.

New to deploying? Start with **Quick Deploy** below — most apps ship in two or three steps. The
deeper sections explain how it works, and the **Advanced** section at the end covers the adapter
system for power users and adapter authors.

## Quick Deploy

Pick your platform. Every path assumes your `package.json` has the standard scripts
(`"build": "ruvyxa build"` — the starter templates already do).

| Platform                       | Steps                                                                                                         |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------- |
| **Vercel**                     | Push your repo → import it on Vercel → done. Ruvyxa detects Vercel and emits the right output.                |
| **Netlify**                    | Push your repo → import it on Netlify → set **Publish directory** to `.ruvyxa/deploy/netlify/publish` → done. |
| **Cloudflare**                 | `ruvyxa build --adapter cloudflare` → `npx wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc`       |
| **Your own server / Docker**   | `ruvyxa build --adapter node` → `node .ruvyxa/deploy/node/server/index.mjs`                                   |
| **Static host (GitHub Pages)** | `ruvyxa build --adapter static` → upload `.ruvyxa/static/`                                                    |

That's it for most projects. No config file is written to your project root, and on Vercel and
Netlify you don't even choose an adapter — the build detects the platform automatically.

## How It Works (in one minute)

`ruvyxa build` compiles your app into `.ruvyxa/`. An **adapter** then repackages that output into
the exact shape a hosting platform expects — a serverless function for Netlify, a Build Output
directory for Vercel, a standalone server for a VPS. You choose an adapter one of three ways:

1. **Automatically** — building on Vercel/Netlify/Cloudflare Pages CI selects the right adapter from
   the platform's environment. Zero configuration.
2. **Command line** — `ruvyxa build --adapter node` (no config changes, uses adapter defaults).
3. **Config** — set `adapter` in `ruvyxa.config.ts` when you need adapter options.

All six official adapters (`node`, `bun`, `static`, `vercel`, `netlify`, `cloudflare`) ship with the
`ruvyxa` package — nothing extra to install.

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
(`Cache-Control: public, max-age=31536000, immutable`) for the content-hashed `/__ruvyxa/client/*`
bundles via `config.json` routes, `.netlify/v1/config.json` headers, and an `_headers` file
respectively.

## Platform Guides

### Vercel

Connect the repository and deploy — nothing else to configure. During the build, the adapter emits
Vercel's Build Output API layout (`.vercel/output/static` and `.vercel/output/config.json`) at the
project root, which Vercel picks up automatically. `.vercel/` is a generated build artifact; the
starter templates already gitignore it.

To choose the adapter explicitly in config:

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { vercelAdapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: vercelAdapter(),
})
```

Pass `vercelAdapter({ projectOutput: false })` to write only under `.ruvyxa/deploy/vercel/` and
deploy that directory manually with the “Other” preset.

### Netlify

Connect the repository, then set two fields once in the Netlify dashboard:

- **Build command**: `npm run build`
- **Publish directory**: `.ruvyxa/deploy/netlify/publish`

No file is written at your project root. The build emits Netlify's Frameworks API directory
(`.netlify/v1/`, a gitignored build artifact) containing the SSR/API function and the immutable
cache headers — Netlify picks it up automatically on deploy.

To choose the adapter explicitly in config:

```ts
import { netlifyAdapter } from '@ruvyxa/adapter-netlify'

export default config({
  adapter: netlifyAdapter(),
})
```

Prefer a committed config file instead of dashboard fields? Pass
`netlifyAdapter({ projectConfig: true })` to generate a project-root `netlify.toml` (with
project-relative paths) on the next build; an existing `netlify.toml` is **never overwritten**. Pass
`frameworksApi: false` to skip the `.netlify/v1/` output.

### Cloudflare

No file is written at your project root. The deploy directory is self-sufficient — deploy it
directly:

```bash
ruvyxa build --adapter cloudflare
npx wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc
```

To choose the adapter explicitly in config:

```ts
import { cloudflareAdapter } from '@ruvyxa/adapter-cloudflare'

export default config({
  adapter: cloudflareAdapter(),
})
```

Prefer a committed root config? Pass `cloudflareAdapter({ projectConfig: true })` to generate a
project-root `wrangler.jsonc` (with project-relative paths); an existing `wrangler.jsonc` is **never
overwritten**.

### Self-Hosted (Node.js, Docker, VPS, PaaS)

```bash
npm run build
npm run start          # serve from .ruvyxa/ using the ruvyxa CLI
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

### Static Hosting

```bash
ruvyxa build --adapter static
# upload .ruvyxa/static/ to your static host
```

Static hosting works for apps whose pages are all SSG/CSR. Pages that need a server (SSR, ISR, PPR,
API routes) are rejected at build time with a clear per-route error — pick a serverless or Node
target for those.

### What Each Platform Supports

| Strategy | Vercel | Netlify | Cloudflare | Node (standalone) | Static |
| -------- | ------ | ------- | ---------- | ----------------- | ------ |
| SSG      | Yes    | Yes     | Yes        | Yes               | Yes    |
| CSR      | Yes    | Yes     | Yes        | Yes               | Yes    |
| SSR      | Yes    | Yes     | Yes        | Yes               | No     |
| API      | Yes    | Yes     | Yes        | Yes               | No     |
| ISR      | Yes    | Yes     | No*        | Yes               | No     |
| PPR      | Yes    | Yes     | No*        | Yes               | No     |

\* Cloudflare Workers lack persistent server-side storage for ISR cache. ISR and PPR routes are
rejected with `RUV2210` on Cloudflare. Use KV or Durable Objects bindings manually if needed.

Static-only deployments (SSG/CSR pages without API or SSR routes) work everywhere. The serverless
adapters emit both static assets and a serverless function; platforms serve static files directly
and forward unmatched requests to the function handler.

## Troubleshooting

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

---

## Advanced: The Adapter System

Everything below is for power users and adapter authors — deploying an app never requires it.

<details>
<summary><strong>Expand the adapter system reference</strong> (resolution rules, writing your own adapter, lifecycle)</summary>

### Available Adapters

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
only when you need to pass adapter options in `ruvyxa.config.ts`.

### `--adapter` Resolution

`--adapter` accepts two kinds of value, and overrides `config.adapter` for that build only:

**1. Built-in names** — `node`, `bun`, `static`, `vercel`, `netlify`, `cloudflare`. These work with
`ruvyxa` alone installed and always use the adapter's defaults.

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

### Writing an Adapter

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

The framework does the heavy lifting: it compiles every route into an executable `.mjs` registry,
copies the shared serverless handler runtime (`serverless-handler.mjs` — SSR, API dispatch, ISR
revalidation, PPR), and materializes the artifacts an adapter declares (`file`, `static-site`,
`function`). The adapter only describes the platform's expected layout and wraps the handler in the
platform's function signature.

### Adapter Lifecycle Notes

- An adapter's `build()` function runs both during configuration loading and during the post-build
  artifact step.
- The post-build step may create only files inside `.ruvyxa/` (plus an allowlist of platform
  discovery paths at the project root, such as `.vercel/output` and `.netlify/v1`); its result is
  recorded as `adapterArtifacts` in `.ruvyxa/build.json`.
- Static adapters deliberately reject dynamic request handling until a platform request handler
  exists. This is a safety boundary, not a fallback.
- Function output contains a compiled `.mjs` static route registry bundle, not raw TypeScript/TSX.
  This makes the emitted artifact executable as-is and lets Wrangler discover edge modules during
  bundling. On Vercel and Netlify, ISR cache age is checked against `revalidate`; only stale entries
  regenerate, and concurrent stale hits are coalesced within a warm function instance.

</details>
