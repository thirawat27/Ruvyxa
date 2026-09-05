# Deployment smoke fixture

The smallest application that is _deployable by every self-hosted adapter_, used by CI to build a
`deploy/<runtime>/` tree and then run it on the real runtime.

It is deliberately not `examples/demo`. The demo is the broad integration fixture: it exercises
every feature this framework has, which makes a failure there a question about the feature rather
than about the emitted server. This fixture is small enough that a failing check names the decision
that broke.

What is here is chosen for what the emitted server has to decide:

| Route         | Proves                                                                                 |
| ------------- | -------------------------------------------------------------------------------------- |
| `/`           | a pre-rendered page served from the publish directory                                  |
| `/cached`     | ISR — the server reads the pre-rendered file and writes back                           |
| `/ppr`        | PPR — the server serves the stored shell instead of rendering it per request           |
| `/api/health` | the generated route registry, reached through the handler                              |
| `/smoke.svg`  | a public asset and its cache headers                                                   |
| `/rsc`        | a **dynamic** server-components route: the `react-server` graph, the SSR registry, and |
|               | the Flight payload, all rendered by the emitted function on every request              |

```bash
pnpm --filter deploy-smoke deploy:bun
node scripts/smoke-runtime-adapter.mjs bun examples/deploy-smoke/.ruvyxa/deploy/bun 4391
```
