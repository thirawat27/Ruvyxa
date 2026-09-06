# ruvyxa_middleware

Ruvyxa's Tower-based middleware stack, the shared route-rule evaluator, and the bridge to the
project worker.

Built-in middleware remains native Rust and can be configured through `config.middleware.builtin`.
`headers()`, `redirects()`, `rewrites()`, and `proxy.matcher` from `ruvyxa.config.ts` are compiled
and evaluated here (`route_rules`), replaying `tests/fixtures/route-rules-conformance.json` with the
JavaScript copy every deployed build runs.

`proxy.handler` is a function, so it runs in the persistent JavaScript project worker
(`packages/ruvyxa/runtime/project-worker.mjs`); `worker_host` owns that process, frames the
request and response over stdio, and validates what comes back. The bridge is deliberately small:
the handler stays in JavaScript, while Rust owns the matcher, ordering, limits, process lifetime,
and conversion to Axum responses.

Worker failures are reported as normal Ruvyxa diagnostics (`RUV1700`, `RUV1701`).
