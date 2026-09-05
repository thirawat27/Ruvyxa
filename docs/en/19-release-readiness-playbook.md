# Release-readiness playbook

> **Tutorial goal:** rehearse one release path and catch missing configuration before deployment.
> **Start from:** a production plan from [Deploy, run, and operate](15-deploy-run-and-operate.md).
> **Checkpoint:** complete every release gate for the delivery model you selected.

Use this page as the final path from a working local application to a release candidate. It only
uses commands and framework behavior present in this repository; platform-specific upload, secrets,
health-check, and rollback controls remain owned by your chosen host.

## 1. Choose one supported delivery model

| Delivery model               | Build command                                                                           | Before you choose it                                                                                                                                                                                                  |
| ---------------------------- | --------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Long-lived Node/Bun process  | `npm run build -- --adapter node` or `npm run build -- --adapter bun`                   | Use for SSR and native realtime. The process is started from the built application with `npm run start`.                                                                                                              |
| Self-hosted Deno process     | `npm run build -- --adapter deno`                                                       | Supports SSR, SSG, CSR, ISR, PPR, and API routes. Copy `deploy/deno/` and start its standalone server; native realtime is unavailable.                                                                                |
| Static host                  | `npm run build -- --target static`                                                      | Every route required at runtime must be prerenderable. Static output cannot satisfy arbitrary SSR requests.                                                                                                           |
| First-party platform adapter | `npm run build -- --adapter vercel` (or netlify/cloudflare/railway/render/firebase/aws) | Inspect that adapter's output contract and configure the provider outside Ruvyxa. Native realtime is rejected for the serverless/static adapters listed in [Integrations](09-integrations-auth-data-and-realtime.md). |

Do not select an adapter only because its name matches your account. Run
`npm run doctor -- --adapter <name>` first; it is the application script intended to inspect adapter
compatibility without materializing artifacts.

## 2. Make configuration release-safe

Use the production configuration pattern in [Configuration](07-configuration.md). Before building:

- Set `site.url` or `RUVYXA_SITE_URL` to the real HTTPS origin; do not publish a preview URL as
  canonical.
- Supply every private value required by `requireEnv([...])` in the build environment.
- Keep `build.map: false` unless your release policy explicitly allows published source maps.
- Set `trustedProxyIps` only to the IPs/CIDRs of proxies that actually sit in front of the app.
- Replace development-only auth memory stores with `redisAuthStore`/`redisRateLimitStore` or another
  durable store before multi-instance deployment.

## 3. Run the release gate

Run these commands from the application root, in this order. Each command must finish successfully
before moving on.

```bash
npm run routes
npm run check
npm run build
npm run test:parity
```

`routes` confirms the discovered public surface. `check` is the application readiness gate. `build`
creates the target artifact. `test:parity` compares dev/prod routes and smoke-renders page routes;
it catches framework-route drift but does not replace unit, integration, accessibility, or load
tests for your application.

## 4. Deploy and prove the release

Deploy the artifact produced by the selected adapter/target through the platform's normal mechanism.
For a self-hosted long-lived process, run the same configured project with:

```bash
npm run start
```

For the Deno adapter, deploy the copied `<outDir>/deploy/deno/` directory and start the standalone
server from that directory instead:

```bash
deno run -A --no-prompt server/index.mjs
```

See the
[Platform adapter guide](20-platform-adapter-guide.md#node-bun-and-deno-copy-a-standalone-app) for
the artifact layout and environment settings.

Then make explicit probes that your app implements: request `/`, one dynamic page, one protected
route, a write action/API route using safe test data, and a health API route if you created one.
Check the response status, expected body, security headers, and structured logs. The framework does
not provide a universal `/health` endpoint, so a probe must target an application route you own.

## 5. Operate and roll back

Record the application version, adapter/target, release time, canonical origin, and a link to the
build logs. Alert on process availability and your own health route, not merely a successful build.
If the release fails, use the host's immutable-artifact rollback to restore the last known-good
version, then compare `npm run routes`, the generated build output, configuration, and logs between
releases. Do not clear a shared cache or change database state as a reflexive rollback action: those
actions have application-specific data consequences not managed by Ruvyxa.

## Sign-off checklist

- [ ] Target/adapter is compatible with every route and enabled plugin.
- [ ] Secret values are private and supplied at build/runtime as required.
- [ ] `routes`, `check`, `build`, and `test:parity` pass from the release commit.
- [ ] The deployed origin, API, auth path, and static assets have been probed.
- [ ] Logs/alerts and a platform rollback owner are in place.
- [ ] The team has tested the failure path for its application data store.

**Previous:** [Documentation scope and sources](18-documentation-scope-and-sources.md) · **Next:**
[Platform adapter guide](20-platform-adapter-guide.md)
