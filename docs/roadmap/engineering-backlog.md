# Engineering Backlog

This backlog tracks practical framework work that remains after the native
bundler and JavaScript plugin bridge upgrades. Items here should not be treated
as deploy blockers unless marked as such.

## Completed

| Area | Status |
| --- | --- |
| Native Rust bundler | Full pipeline: resolver, compiler, linker, minifier, source maps, chunk manifests |
| Route-level code splitting | One hydration bundle per page route via `splitStrategy: "route"` |
| Shared route chunk metadata | Emitted in client manifest for modules shared across routes |
| Dynamic `import()` split points | Implemented in native bundler manifests and emitted chunk files |
| AST-backed module facts | Implemented for resolver/compiler inputs |
| Native bundler plugin pipeline | `NativeBundlerPlugin` trait with `resolveId`/`transform` hooks |
| JavaScript config plugin bridge | `resolveId` and `transform` hooks from `ruvyxa.config.ts` |
| Tree-shaking | Dead-code elimination per route bundle, enabled by default |
| Minification | Whitespace removal, identifier shortening, dead-code elimination |
| BLAKE3 content hashing | Deterministic cache-busting filenames for client bundles |
| In-process + disk compile cache | Incremental rebuilds avoid repeated transforms |
| Incremental module graph cache | Persistent cache across route builds in same session |
| Server/client boundary validation | `RUV1007`, `RUV1008`, `RUV1009`, `RUV1010` diagnostics |
| Dev/prod parity check | `ruvyxa test:parity` compares routes and smoke-renders pages |
| FIFO render cache | Configurable capacity and TTL for SSR and client bundles |
| Node worker pool | Persistent IPC worker pool eliminates per-request subprocess overhead |
| Radix-tree router | O(path-depth) route resolution |
| Security headers + action guards | Default headers, origin checks, rate limiting, body limits |
| Wasmtime plugin runtime | Sandboxed WebAssembly plugins with fuel, memory, and permissions |
| Configurable CORS, timing, logging middleware | Tower-based composable middleware stack |
| CLI platform packages | `@ruvyxa/cli-*` for win32, linux x64/arm64, darwin x64/arm64 |

## Remaining Work

| Priority | Area | Why it matters | Suggested proof |
| --- | --- | --- | --- |
| P1 | Persistent JS plugin worker | Current bridge spawns Node per hook; fine for correctness, slower for large apps with many plugin hooks | Benchmark route builds before/after worker pooling |
| P1 | Runtime preload for shared chunks | Shared chunks emitted as metadata/files, but route scripts remain self-contained; preload tags would improve hydration | Browser integration test verifies preload tags and route hydration timing |
| P1 | Plugin source map forwarding | `transform` may return maps, but native bridge currently forwards only code | Source map fixture with transformed line mapping verified in browser DevTools |
| P2 | Full parser compatibility suite | AST facts are lightweight; add cases for advanced TS/JSX grammar (decorators, `satisfies`, `using`, etc.) | Parser fixture suite in native bundler tests |
| P2 | Config dependency invalidation | Config/plugin changes should invalidate all affected compile caches explicitly | Change plugin code and verify rebuilt output changes without `ruvyxa clean` |
| P2 | Adapter consumption of chunk manifest | Adapters should copy/use `chunk-manifest.json` consistently | Adapter tests inspect produced deployment output for chunk references |
| P3 | Dependency pre-bundling | Can improve cold dev startup for dependency-heavy apps | Benchmark cold `ruvyxa dev` startup with large dependency graph |
| P3 | Remote/build-cache server | Share compile cache across CI and developer machines | Integration test with shared network cache directory |

## Release Gate

Before a release that claims production readiness, run:

```bash
cargo test --workspace
pnpm -r build
pnpm -r check
pnpm -r test
cargo run -p ruvyxa_cli -- check --root examples/kitchen-sink
```

Or use the full CI workflow (`.github/workflows/`) which runs all quality gates,
builds native CLI binaries per platform, and publishes npm packages with
provenance. Warnings for unsupported optional platform packages on non-target
OSes are expected during `pnpm workspace` commands.
