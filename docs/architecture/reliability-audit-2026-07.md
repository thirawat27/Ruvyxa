# Reliability Audit — 2026-07

## Follow-up update — 2026-07-16

This document retains the July 13 repair record below and adds the current follow-up evidence for
v1.0.14. The current production pipeline uses the Ruvyxa resolver/linker/cache layers with Oxc
0.139.0 for TypeScript/JSX transformation and minification; see
`docs/architecture/bundler-modernization.md` for the ownership boundary.

### Confirmed follow-up findings and repairs

| #   | Finding                                                                                                                                                                       | Evidence                                        | Impact                                                                                                              | Severity | Confidence | Applied correction                                                                                                                                                    |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- | -------- | ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | The persistent Node worker parsed `RUVYXA_WORKER_TIMEOUT_MS` and `RUVYXA_MEMORY_LIMIT_MB` directly. Invalid or zero values disabled the timeout or cache-pressure protection. | `packages/ruvyxa/runtime/worker-pool.mjs`       | A malformed deployment environment could leave hung work unbounded or prevent cache shedding under memory pressure. | High     | Direct     | Both settings now use the existing positive-integer parser and safely fall back to 30,000 ms and 512 MiB. A worker IPC regression test verifies the effective values. |
| 2   | `RUVYXA_RENDER_CACHE_SIZE` was used directly for `HashMap` and queue pre-allocation.                                                                                          | `crates/ruvyxa_dev_server/src/render_cache.rs`  | An accidentally extreme value could request an excessive allocation while the server starts.                        | High     | Direct     | Environment-derived capacity now clamps to 16,384. `0` remains the documented cache-disable setting, and invalid values retain the mode-specific default.             |
| 3   | API responses were converted with `response.text()` and returned to Rust as one NDJSON value.                                                                                 | Node worker protocol and Rust worker dispatcher | Large or binary responses required whole-body text materialization on both sides of the worker boundary.            | High     | Direct     | API responses now use opt-in start/chunk/end/error frames, binary-safe Base64 payloads, bounded per-response queues, idle timeouts, and legacy fallback.              |

### Completed API streaming boundary

Rust now requests streaming with the additive `streamResponse` capability. Supporting Node workers
send response metadata first and then binary-safe 64 KiB frames; Axum exposes the receiver as its
HTTP body instead of waiting for one complete text response. Each response has a 16-frame pending
queue, and worker exit, stream error, queue overflow, or idle timeout closes only the affected body.

Compatibility is bidirectional across the additive protocol: a new Rust runtime accepts the legacy
single-message response from an older worker, and a new Node worker keeps returning the legacy shape
when the caller does not request streaming. Base64 remains intentional here: it has native fast
paths, less expansion than Base58, and is safe inside JSON strings. A separate binary transport
would be the appropriate future optimization if encoding overhead becomes material.

### Follow-up validation

- `cargo test --workspace --locked` passed 301 Rust tests, including the render-cache and streamed
  worker-body regressions.
- `cargo clippy --workspace --locked -- -D warnings` and `pnpm format:check` passed.
- Focused Rust worker tests passed for binary reconstruction, queue overflow, idle timeout, stream
  error propagation, and legacy response fallback.
- The focused Node worker suite passed four tests, including a 150,000-byte multi-frame binary
  response and the worker-environment regression.
- `pnpm -r build`, `pnpm -r check`, and `pnpm -r test` passed. The `ruvyxa` package passed 43 tests,
  and the demo passed build plus 16-route parity and smoke rendering.
- `pnpm check:cargo-lock` passed after adding the explicit Base64 and stream-trait dependencies.
- `pnpm release:validate` and `pnpm pack:smoke` passed, confirming package metadata and tarball
  contents remain valid.

## Scope

- Project: Ruvyxa monorepo
- Inspection date: 2026-07-13
- Intake scope: Find and repair confirmed bugs, bottlenecks, and errors across the repository.
- Final documented scope: Rust CLI/server/bundler/graph/middleware crates, Node runtime and
  packages, demo integration, CI and package artifacts.
- Pass level: Full Mode
- Pass reason: The request spans six Rust crates, npm runtime/package contracts, and the demo
  integration flow.
- Inspection source: tracked-file inventory; workspace manifests; CI; README and existing
  architecture docs; static checks; Rust and Node test suites; demo check/parity; targeted
  source/caller review of runtime module resolution and worker/cache paths.
- Skipped areas: A line-by-line review of every large implementation file; non-Windows CI runners;
  generated, dependency, cache, binary, and secret material.

## Confirmed Architecture Facts

- `ruvyxa_cli` orchestrates route discovery, bundling, build output, and the dev server.
  - Evidence: `crates/ruvyxa_cli/Cargo.toml`, `crates/ruvyxa_cli/src/main.rs`.
  - Evidence strength: Direct.
- The Node runtime is shared by standalone renderers and the persistent worker pool.
  - Evidence: `packages/ruvyxa/runtime/{action,api,client,ssg,ssr,worker-pool,compiler}.mjs`.
  - Evidence strength: Direct.
- CI validates Rust formatting/tests/clippy, TypeScript checks, packages, demo parity, metadata, and
  package tarballs.
  - Evidence: `.github/workflows/ci.yml`.
  - Evidence strength: Direct.

## Component Map

| Component                 | Responsibility                                                     | Evidence                         |
| ------------------------- | ------------------------------------------------------------------ | -------------------------------- |
| `ruvyxa_cli`              | CLI orchestration and production build staging                     | `crates/ruvyxa_cli/src/main.rs`  |
| `ruvyxa_bundler`          | TS/JSX compilation, resolution, linking, minification, source maps | `crates/ruvyxa_bundler/src/`     |
| `ruvyxa_dev_server`       | Axum serving, HMR, render caching, worker-pool use                 | `crates/ruvyxa_dev_server/src/`  |
| `ruvyxa_graph`            | File-system routes, render strategy, boundary validation           | `crates/ruvyxa_graph/src/lib.rs` |
| `packages/ruvyxa/runtime` | Standalone renderers and persistent Node worker implementation     | `packages/ruvyxa/runtime/`       |
| `examples/demo`           | End-to-end parity fixture                                          | `examples/demo/`                 |

## Finding and Repair

| #   | Finding                                                                                                                                                                     | Dimension          | Evidence                                                                                                                                                                            | Impact                                                                                                                                                           | Severity | Confidence |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------- | ---------- |
| 1   | Runtime-directory resolution used URL `.pathname` in six renderer/worker entry points and the compiler default.                                                             | Flow conflict      | `packages/ruvyxa/runtime/*.mjs`; regression test in `tests/packages/ruvyxa/compiler.test.mjs`.                                                                                      | URL-encoded path segments, such as spaces, remained encoded and caused runtime alias paths to miss source or distribution files.                                 | High     | Direct     |
| 2   | The Action endpoint extracted Axum `ConnectInfo<SocketAddr>` while the server passed a plain router to `axum::serve`.                                                       | Flow conflict      | `crates/ruvyxa_dev_server/src/lib.rs`; Axum 0.8.9 `extract/connect_info.rs` documents this extractor fails without `into_make_service_with_connect_info`; live TCP regression test. | Requests to `POST /__ruvyxa/action` could be rejected before the handler because the required connection extension was absent.                                   | Critical | Direct     |
| 3   | Forwarded identity/protocol headers were trusted for every private-network peer, and the generic rate limiter fell back to `X-Forwarded-For` when peer metadata was absent. | Boundary violation | `crates/ruvyxa_dev_server/src/lib.rs`, `crates/ruvyxa_middleware/src/builtin.rs`; regression tests.                                                                                 | A client on the same private network could forge headers to partition or bypass action rate limits; the same trust ambiguity affected the proxied origin scheme. | High     | Direct     |

The approved correction is applied: all affected runtime files now use
`fileURLToPath(import.meta.url)`, and a regression test imports a copied compiler from a temporary
path containing spaces.

The later Full Mode pass also applies the following root-cause repairs:

- The server now always wraps its router with `into_make_service_with_connect_info::<SocketAddr>()`;
  a live TCP test proves the request handler receives the peer address.
- Loopback is trusted for local reverse proxies. Other proxy addresses must be explicitly listed in
  `security.trustedProxyIps` before Ruvyxa accepts forwarded identity or protocol headers.
- Built-in IP rate limiting uses the transport peer by default. A header-based key remains an
  explicit middleware configuration choice.

The completed follow-up repair pass also closes the remaining confirmed findings:

- API worker IPC now preserves query strings, binary request bodies, ordered duplicate headers, and
  repeated `Set-Cookie` response values while retaining legacy fields for installed runtimes.
- ISR uses the route's `revalidate` interval, serves stale content during a coalesced refresh, and
  refuses unsafe prerender paths before joining them to the prerender directory.
- Built-in middleware rate limiting avoids a full bucket sweep on each request; invalid rate
  limits/selectors fail validation. Wasm plugin output keeps its existing ABI, supports results
  above 4 KiB, and has a bounded 1 MiB decode limit.
- Bundling and graph analysis now preserve exactly-once evaluation for overlapping dynamic chunks,
  resolve `baseUrl` path targets correctly, inspect local imports/layouts before implicit SSG, and
  validate literal dynamic import/require edges without rewriting comments or strings.
- Core/runtime caches preserve full-cache refreshes, serve stale values consistently to concurrent
  readers, bound compiler derivation caches, invalidate them on worker changes, and reject invalid
  scalar configuration values instead of silently dropping them.

## Validation

- `cargo test --workspace --locked`: passed (279 Rust tests).
- `cargo fmt --all -- --check` and `cargo clippy --workspace --locked -- -D warnings`: passed.
- `pnpm -r build`, `pnpm -r check`, and `pnpm -r test`: passed; the demo's 16 routes passed dev/prod
  parity and smoke rendering.
- `pnpm format:check`, `pnpm check:cargo-lock`, `pnpm release:validate`, and `pnpm pack:smoke`:
  passed.
- Targeted post-repair checks:
  `cargo test -p ruvyxa_dev_server -p ruvyxa_cli -p ruvyxa_middleware --locked` (126 tests) and
  `pnpm --filter ruvyxa test -- tests/packages/ruvyxa/compiler.test.mjs` (34 tests) passed.
- Final post-repair checks: `cargo test --workspace --locked` (283 tests),
  `cargo clippy --workspace --locked -- -D warnings`, `pnpm -r check` (including 16-route demo
  parity), `pnpm -r test`, `pnpm format:check`,
  `cargo run -p ruvyxa_cli -- check --root examples/demo`, and
  `cargo run -p ruvyxa_cli -- test:parity --root examples/demo` passed.
- Follow-up repair validation: `cargo test --workspace --locked` (299 Rust tests),
  `cargo clippy --workspace --locked -- -D warnings`, `pnpm -r build`, `pnpm -r check`,
  `pnpm -r test` (including 38 `ruvyxa` tests), `pnpm format:check`, `pnpm release:validate`,
  `pnpm pack:smoke`, and the direct 16-route demo check/parity commands passed on Windows x64.

## Risks and Unknowns

- Cross-platform CI remains the final proof for macOS, Linux, Windows ARM, and native package
  execution; this audit ran on Windows x64.
- The explicit trusted-proxy policy changes non-loopback reverse-proxy setup: deployments must set
  `security.trustedProxyIps` to the exact proxy IPs before forwarded headers influence client
  identity or HTTPS origin checks.
- No additional confirmed critical defect survived the available static and targeted runtime checks.
  This is not a claim that every possible latent defect has been eliminated.

## Validation Gate

1. Claim traceability: All findings and architecture statements cite inspected files or commands.
2. Scope alignment: The completed work matches the requested audit-and-repair scope; no public
   contracts were intentionally changed.
3. Handoff readiness: The repaired path has regression coverage; cross-platform CI is the remaining
   external verification.
