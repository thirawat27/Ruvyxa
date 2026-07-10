# Engineering Backlog

This backlog tracks practical framework work that remains after the native bundler and JavaScript
plugin bridge upgrades. Items here should not be treated as deploy blockers unless marked as such.

## Completed

| Area                                          | Status                                                                            |
| --------------------------------------------- | --------------------------------------------------------------------------------- |
| Native Rust bundler                           | Full pipeline: resolver, compiler, linker, minifier, source maps, chunk manifests |
| Route-level code splitting                    | One hydration bundle per page route via `splitStrategy: "route"`                  |
| Shared route chunk metadata                   | Emitted in client manifest for modules shared across routes                       |
| Dynamic `import()` split points               | Implemented in native bundler manifests and emitted chunk files                   |
| AST-backed module facts                       | Implemented for resolver/compiler inputs                                          |
| Native bundler plugin pipeline                | `NativeBundlerPlugin` trait with `resolveId`/`transform` hooks                    |
| JavaScript config plugin bridge               | `resolveId` and `transform` hooks from `ruvyxa.config.ts`                         |
| Persistent JS plugin worker                   | One JSONL Node worker per build context serves all config plugin hooks            |
| Plugin source map forwarding                  | Plugin transform maps are merged into emitted client bundle source maps           |
| Runtime preload for shared chunks             | Route manifests drive `modulepreload` hints for runtime and pre-rendered HTML     |
| Tree-shaking                                  | Dead-code elimination per route bundle, enabled by default                        |
| Minification                                  | Whitespace removal, identifier shortening, dead-code elimination                  |
| BLAKE3 content hashing                        | Deterministic cache-busting filenames for client bundles                          |
| In-process + disk compile cache               | Incremental rebuilds avoid repeated transforms                                    |
| Incremental module graph cache                | Persistent cache across route builds in same session                              |
| Server/client boundary validation             | `RUV1007`, `RUV1008`, `RUV1009`, `RUV1010` diagnostics                            |
| Dev/prod parity check                         | `ruvyxa test:parity` compares routes and smoke-renders pages                      |
| FIFO render cache                             | Configurable capacity and TTL for SSR and client bundles                          |
| Node worker pool                              | Persistent IPC worker pool eliminates per-request subprocess overhead             |
| Radix-tree router                             | O(path-depth) route resolution                                                    |
| Security headers + action guards              | Default headers, origin checks, rate limiting, body limits                        |
| Wasmtime plugin runtime                       | Sandboxed WebAssembly plugins with fuel, memory, and permissions                  |
| Configurable CORS, timing, logging middleware | Tower-based composable middleware stack                                           |
| CLI platform packages                         | `@ruvyxa/cli-*` for win32, linux x64/arm64, darwin x64/arm64                      |
| Full parser compatibility suite               | Fixture coverage for multiline modules, decorators, `satisfies`, `using`, and JSX |
| Config dependency invalidation                | Imported config/plugin fingerprints namespace compile-cache artifacts             |
| Adapter chunk-manifest consumption            | Every adapter reports client output and optional chunk-manifest paths             |
| Dependency pre-bundling                       | Dev route graphs warm every persistent Node worker in the background              |
| Shared/remote build cache                     | Configurable local, network, or CI-restored compile-cache directory               |

## Remaining Work

No P2 or P3 engineering items remain in this backlog. New work should be added with an observable
proof before implementation begins.

## Release Gate

Before a release that claims production readiness, run:

```bash
cargo test --workspace
pnpm -r build
pnpm -r check
pnpm -r test
cargo run -p ruvyxa_cli -- check --root examples/demo
```

Or use the full CI workflow (`.github/workflows/`) which runs all quality gates, builds native CLI
binaries per platform, and publishes npm packages with provenance. Warnings for unsupported optional
platform packages on non-target OSes are expected during `pnpm workspace` commands.
