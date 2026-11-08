# Single Production Host — Design (Phase 0 + Phase 1)

Status: approved design, not yet implemented. Date: 2026-10-12.

## Why

Ruvyxa serves a production request through two hosts that must agree: the Axum server behind
`ruvyxa start` ([`crates/ruvyxa_cli/src/main.rs`](../../../crates/ruvyxa_cli/src/main.rs), the
`Command::Start` arm calls `serve(production_server_config(..))`), and `createHandler` in
[`packages/ruvyxa/runtime/serverless-handler.mjs`](../../../packages/ruvyxa/runtime/serverless-handler.mjs),
which every adapter — including the self-hosted `node`, `bun`, and `deno` adapters — runs. The same
split exists for compilation (`ruvyxa_bundler` and `runtime/compiler.mjs`), scanning (`ast.rs` and
`runtime/scanner.mjs`), and worker pools (`worker_pool.rs` and `runtime/worker-pool.mjs`).

The repository holds 46 cross-language tables in `tests/fixtures/`; each exists because one rule is
implemented twice. The defects found in the August–September audits are overwhelmingly of one shape:
correct on one host, wrong on the other (`/__ruvyxa/action` returning 404 on every deployed build is
the example
[`framework-endpoint-conformance.json`](../../../tests/fixtures/framework-endpoint-conformance.json)
records). The cost is not the number of features; it is that every feature is paid for twice.

## Programme

The owner chose staged convergence: **one runtime, one compiler**. Rust becomes the compiler, the
build orchestrator, and the development transport; `createHandler` becomes the only request decision
core in every mode.

| Phase | Outcome                                                                     | Spec      |
| ----- | --------------------------------------------------------------------------- | --------- |
| 0     | Battery baseline green; integrations moved to an experimental tier          | this file |
| 1     | `ruvyxa start` runs the standalone JS server; one production host           | this file |
| 2     | `ruvyxa dev` routes requests through the same handler; Axum keeps watch/HMR | separate  |
| 3     | Rust bundler emits server modules; `compiler.mjs` resolution deleted        | separate  |

Each phase ships on its own with the full battery green.

### Decisions already made

- The production promise is **self-hosted** (Node, Bun, Deno, Docker).
- The single production host is the **JS handler** (`createHandler` inside the standalone server).
- **Tier 1** (guaranteed): SSR, SSG, CSR, API routes, server actions, the static adapter, RSC, ISR,
  PPR, the serverless adapters, the content engine, MDX, i18n routing, and image optimization.
- **Experimental**: `@ruvyxa/auth`, `@ruvyxa/database`, `@ruvyxa/realtime`, `@ruvyxa/testing`.
- `--workers` is in scope for Phase 1.

### Out of scope here

The development request path (Phase 2), the compiler (Phase 3), and splitting the large Rust files —
`crates/ruvyxa_dev_server/src/lib.rs` and `render_pipeline.rs` lose most of their content in Phase
2, so refactoring them now would be done twice.

## Phase 0 — Baseline

1. Run the eleven checks listed in `CLAUDE.md` on `main` and record the result in a "Baseline"
   section appended to this file. Every failure is fixed at its root cause, test first, before Phase
   1 begins.
2. Mark the four integration packages experimental:
   - `package.json` gains `"ruvyxa": { "tier": "experimental" }`.
   - Each README opens with a warning that the package carries no production guarantee.
   - A new check in `release:validate` fails when an experimental package lacks the field or the
     warning, and when a Tier 1 package depends on an experimental one.

## Phase 1 — One production host

### Components

1. **Local standalone stage** (`crates/ruvyxa_cli/src/build.rs`). Today a build with no adapter
   skips the adapter stage entirely. After this change every build also runs the built-in adapter
   named after the configured `runtime` (`node`, `bun`, or `deno`) when the selected adapter is not
   already that one, so `<outDir>/deploy/<runtime>/` always holds the standalone server — the same
   directory and the same generator a self-hosted deployment uses. The stage writes
   `<outDir>/deploy/<runtime>/.ruvyxa-build-id` holding `manifest.json`'s `deploy.buildId`.
2. **`start` launcher** (Rust). `ruvyxa start` becomes a process supervisor: resolve the runtime
   binary, spawn `<runtime> <outDir>/deploy/<runtime>/server/index.mjs`, pass `PORT`, `HOST`, and
   the environment through, forward `SIGTERM`/`SIGINT`, forward stdout/stderr untouched, and exit
   with the child's code. It is not on the request path.
3. **Rollback flag.** `ruvyxa start --legacy-host` runs today's Axum production path unchanged and
   prints a deprecation warning on every run. It is kept for one minor release and removed in
   Phase 2.
4. **`--workers N`** in the standalone server (below).

### Request flow

```text
client ─► standalone server (node:http | Bun.serve | Deno.serve)
          ├─ /__ruvyxa/health, /__ruvyxa/metrics   answered by the standalone server
          ├─ public/ and /__ruvyxa/client/*        static, existing ETag and gzip
          └─ createHandler(request)                action, rsc, flight, image, routes
```

### `--workers N`

- Set by `ruvyxa start --workers N` or `RUVYXA_WORKERS`, so `node server/index.mjs` deployed
  directly behaves the same. `auto` means `availableParallelism()`. Default `1`.
- **Node**: `node:cluster`. The primary forks N workers and serves nothing itself.
- **Bun**: N processes listening with `reusePort: true` on Linux. Elsewhere `N > 1` is refused with
  a diagnostic.
- **Deno**: `N > 1` is refused with a diagnostic. No emulation.
- **Drain**: the primary forwards `SIGTERM` to every worker; each drains with the existing logic;
  the primary exits when the last worker exits or the grace period ends.
- **Crash**: a dead worker is respawned with backoff. More than 5 crashes in 60 seconds makes the
  primary exit non-zero so the orchestrator sees it; no silent crash loop.
- **State**: the ISR store, rate limiters, and in-memory caches are per worker. The docs say so and
  point to `cache.handler` for state shared across workers. Per-process rate limiting is accepted
  and documented as such.

The standalone server is emitted as generated source, so every new line in it is subject to the
generated-source trap: a backtick in a comment breaks the emitted file. The emitted output is
compiled in a test (below).

### `image.onDemand` under `start`

The standalone server has no image pipeline, and a second one in JavaScript would be exactly the
duplication this programme removes. `ruvyxa start` serves the existing Rust pipeline
(`dynamic_image::optimize`) on `127.0.0.1:<ephemeral>` and passes `RUVYXA_IMAGE_SERVICE` and
`RUVYXA_IMAGE_SERVICE_TOKEN` to the child. The standalone server, when `image.onDemand` is on and
those are set, hands `createHandler` an `optimizeImage` that forwards to the sidecar — the shape the
Vercel and Cloudflare adapters already use for their platforms' optimizers. A deployment started
with `node server/index.mjs` directly has no sidecar and answers 404 exactly as it does today;
`tests/fixtures/adapter-contract.json` keeps `onDemandImages: false` for node/bun/deno because it
describes the deployed artifact, not `start`.

### Error handling for `start`

| Condition                                       | Result                                                     |
| ----------------------------------------------- | ---------------------------------------------------------- |
| No build output, or standalone from older build | RUV-coded error naming `ruvyxa build`; build id, not mtime |
| Runtime binary not on `PATH`                    | Error naming the runtime it looked for                     |
| Config enables realtime or collab               | Diagnostic: both need `--legacy-host` for now              |
| Child exits                                     | `start` exits with the same code                           |

| Code      | Meaning                                                                  |
| --------- | ------------------------------------------------------------------------ |
| `RUV1022` | Standalone server missing or older than the build output                 |
| `RUV1023` | The JavaScript runtime `start` needs was not found                       |
| `RUV1024` | Realtime or collab is configured, and both are served by `--legacy-host` |
| `RUV1025` | `RUVYXA_WORKERS` is invalid, or unsupported on this runtime/OS           |

New RUV codes go through the diagnostic code registry and its gate.

## Testing

1. **Launcher (Rust unit tests)**: runtime resolution, env and signal forwarding, exit-code
   propagation, and each error row above.
2. **Standalone staging**: a build with no adapter produces `<outDir>/deploy/<runtime>/`, and the
   generated source is compiled in the test.
3. **Cluster (Node tests)**: with N=2 both workers answer; a killed worker is respawned; `SIGTERM`
   drains every worker and `/__ruvyxa/health` reports `draining` before the socket closes; a crash
   loop exits the primary non-zero; Bun and Deno refuse N > 1 where specified.
4. **End to end**: a `start` lane in `scripts/smoke-runtime-adapter.mjs` builds
   `examples/deploy-smoke` with no adapter, runs `ruvyxa start` on real Node, Bun, and Deno, and
   replays the probes the adapter lanes use, proving `start` and the node adapter answer alike.
5. **Legacy**: `--legacy-host` passes the existing smoke and prints the warning.

## Fixtures

Phase 1 retires none: Axum still serves `dev` and `--legacy-host`. The tables that exist only to
hold Axum and `createHandler` together are listed below, so Phase 2 starts from a list. The fixtures
themselves are not edited — several of their readers assert an exact key set.

### Phase 2 — retire with the Axum request host

A fixture is listed when it has a Rust reader in `crates/ruvyxa_dev_server` or
`crates/ruvyxa_middleware` and a JavaScript reader that exercises `serverless-handler.mjs`,
`@ruvyxa/core`, or the standalone server. 18 fixtures.

| Fixture                               | Rust reader                                                     | JS reader                                                                 |
| ------------------------------------- | --------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `action-contract.json`                | `ruvyxa_dev_server/src/action_security.rs`                      | `serverless-handler.test.mjs`                                             |
| `byte-range-conformance.json`         | `ruvyxa_dev_server/src/static_assets.rs`                        | `byte-range-contract.test.mjs`, `core/standalone-server.test.ts`          |
| `client-ip-conformance.json`          | `ruvyxa_dev_server/src/action_security.rs`, `ruvyxa_middleware` | `client-ip-contract.test.mjs` (reads `serverless-handler.mjs`)            |
| `cors-conformance.json`               | `ruvyxa_middleware/src/config.rs`                               | `cors-contract.test.mjs` (`createHandler`)                                |
| `document-head-conformance.json`      | `ruvyxa_dev_server/src/static_assets.rs`                        | `document-head-parity.test.mjs` (`serverless-handler.mjs`)                |
| `dynamic-image-conformance.json`      | `ruvyxa_dev_server/src/dynamic_image.rs`                        | `serverless-shared-tables.test.mjs`, `@ruvyxa/core/src/utils.ts`          |
| `framework-endpoint-conformance.json` | `ruvyxa_dev_server/src/framework_endpoints.rs`, `lib.rs`        | `framework-endpoints.test.mjs`, `@ruvyxa/core/src/standalone-server.ts`   |
| `i18n-routing-conformance.json`       | `ruvyxa_dev_server/src/i18n.rs`                                 | `serverless-handler.test.mjs`                                             |
| `origin-policy-conformance.json`      | `ruvyxa_dev_server/src/action_security.rs`                      | `core/origin-policy-contract.test.ts`                                     |
| `prerender-path-conformance.json`     | `ruvyxa_dev_server/src/render_pipeline.rs`, `static_assets.rs`  | `prerender-path.test.mjs` (`serverless-handler.mjs`)                      |
| `rate-limit-conformance.json`         | `ruvyxa_middleware/src/builtin.rs`                              | `serverless-handler.test.mjs`                                             |
| `request-body-limit-conformance.json` | `ruvyxa_dev_server/src/lib.rs`                                  | `serverless-shared-tables.test.mjs`                                       |
| `revalidation-conformance.json`       | `ruvyxa_dev_server/src/render_cache.rs`, `render_pipeline.rs`   | `serverless-handler.test.mjs`, `request-context.test.mjs`                 |
| `route-match-conformance.json`        | `ruvyxa_dev_server/src/router.rs`                               | `@ruvyxa/core/src/route-match.ts`, `serverless-handler.mjs`               |
| `route-rules-conformance.json`        | `ruvyxa_middleware/src/route_rules.rs`                          | `core/route-rules.test.ts`, `route-rules-host.test.mjs`                   |
| `security-headers-conformance.json`   | `ruvyxa_dev_server/src/response.rs`                             | `core/security-headers.test.ts`, `serverless-shared-tables.test.mjs`      |
| `static-asset-conformance.json`       | `ruvyxa_dev_server/src/static_assets.rs`                        | `core/static-asset-contract.test.ts`, `serverless-shared-tables.test.mjs` |
| `stored-document-conformance.json`    | `ruvyxa_dev_server/src/render_pipeline.rs`                      | `serverless-handler.test.mjs`, `worker-pool.test.mjs`                     |

Test paths without a directory are under `tests/packages/ruvyxa/`. Dev-only fixtures
(`hmr-contract.json`, `instrumentation-files-conformance.json`) stay: `dev` keeps its watcher.

### Phase 3 — held between the two compilers

These pair `ruvyxa_bundler` / `ruvyxa_graph` / `ruvyxa_cli` with `runtime/compiler.mjs`,
`scanner.mjs`, or another runtime module, and have no request-host reader. 23 fixtures: the bundler
and scanner set (`content`, `css-module-class`, `entry-composition`, `client-bootstrap`,
`env-policy`, `es-target`, `glob`, `import-case`, `mdx-components`, `module-kind`, `module-lane`,
`module-resolution`, `module-syntax`, `ordering`, `path-alias`, `source-scanner`), the graph set
(`route-chain`, `route-pattern`, `intercepting-route`, `edge-runtime`, `deploy-output`), and
`adapter-contract`, `flight-conformance`. The remaining fixtures (`build-bench-contract`,
`cache-budget-contract`, `config-surface-conformance`) are single-language contracts with a gate,
not host pairs.

## Documentation

- `docs/en` and `docs/th`, deploy and CLI chapters: `start` runs the standalone server, `--workers`
  and its per-worker state, the experimental tier, the `--legacy-host` deprecation. No pinned
  version numbers and no other framework named.
- `ARCHITECTURE.md` and `AGENTS.md`: the "Two request hosts" passages describe the new state —
  production has one host, development still has two until Phase 2.

## Definition of done

All eleven checks in `CLAUDE.md` pass, and the `start` lane passes on Node, Bun, and Deno. Work is
reported, not committed; the owner commits.

## Risks

- **Throughput on many cores** without `--workers`: default is 1, so a deployment that relied on
  Axum's worker pool must set `--workers`. Called out in the release notes.
- **Realtime and collab users** lose `start` support until they pass `--legacy-host`; both are
  experimental, and `RUV1024` makes the change loud for either.
- **Removing `--legacy-host` is gated on Phase 2.** The realtime (`/__ruvyxa/realtime`) and collab
  (`/__ruvyxa/collab`) sockets are served only by the Axum host today. The flag cannot go until
  Phase 2 moves both sockets to the standalone server, or those projects have no production host at
  all; the "removed in the next minor release" wording follows that gate, not the calendar.
- **Bun `reusePort`** behaviour differs by OS; the refusal path keeps it honest rather than
  approximate. No CI lane runs Bun with more than one worker on Linux yet — the only OS where it is
  allowed — so that path is proven by unit tests over the emitted source alone. A Bun multi-worker
  Linux lane belongs in Phase 2.
- **The local standalone stage** adds a node pass to every non-self-hosted build. A failure in it is
  a warning, never a failed deployment build; `start` then reports `RUV1022`.

## Baseline

Recorded 2026-10-12 on `main` at `be8d7a7e`, before any Phase 1 change. Windows 11 host.

| Check                                              | Result |
| -------------------------------------------------- | ------ |
| `cargo fmt --all -- --check`                       | pass   |
| `cargo clippy --workspace --locked -- -D warnings` | pass   |
| `cargo test --workspace --locked`                  | pass   |
| `pnpm -r build`                                    | pass   |
| `pnpm -r check`                                    | pass   |
| `pnpm -r test`                                     | pass   |
| `pnpm lint`                                        | pass   |
| `pnpm format:check`                                | pass   |
| `pnpm check:unused`                                | pass   |
| `pnpm release:validate`                            | pass   |
| `pnpm pack:smoke`                                  | pass   |

No failure to fix: Phase 1 starts from a green tree.
