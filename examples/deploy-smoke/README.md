# Deployment smoke fixture

The smallest application that is _deployable by every self-hosted adapter_, used by CI to build a
`deploy/<runtime>/` tree and then run it on the real runtime.

It is deliberately not `examples/demo`. The demo is the broad integration fixture and includes a
dynamic server-components route, which no adapter can deploy — every adapter serves pages through a
generated module built by the ordinary SSR entry, so such a route is refused at build time with
`RUV2213` (see `docs/en/04-routing-rendering.md`). Building the demo with an adapter therefore
cannot succeed, and the jobs that tried were testing the demo's feature list rather than the emitted
server.

What is here is chosen for what the emitted server has to decide:

| Route         | Proves                                                       |
| ------------- | ------------------------------------------------------------ |
| `/`           | a pre-rendered page served from the publish directory        |
| `/cached`     | ISR — the server reads the pre-rendered file and writes back |
| `/api/health` | the generated route registry, reached through the handler    |
| `/smoke.svg`  | a public asset and its cache headers                         |

```bash
pnpm --filter deploy-smoke deploy:bun
node scripts/smoke-runtime-adapter.mjs bun examples/deploy-smoke/.ruvyxa/deploy/bun 4391
```
