# Engineering Backlog

This backlog tracks practical framework work that remains after the native
bundler and JavaScript plugin bridge upgrades. Items here should not be treated
as deploy blockers unless marked as such.

## Completed In Current Build Track

| Area | Status |
| --- | --- |
| Shared route chunk metadata | Implemented in CLI client manifests. |
| Dynamic `import()` split points | Implemented in native bundler manifests and emitted chunk files. |
| AST-backed module facts | Implemented for resolver/compiler inputs. |
| Native bundler plugin pipeline | Implemented for Rust resolve/transform hooks. |
| JavaScript config plugin bridge | Implemented for `resolveId` and `transform` hooks from `ruvyxa.config.ts`. |
| Runtime config multi-line default exports | Fixed for `export default defineConfig({ ... })`. |
| Bundler crate structure | Split into stage modules with `lib.rs` as orchestration. |

## Remaining Non-Blocking Work

| Priority | Area | Why it matters | Suggested proof |
| --- | --- | --- | --- |
| P1 | Persistent JS plugin worker | Current bridge starts Node per hook; fine for correctness, slower for large apps. | Benchmark route builds before/after worker pooling. |
| P1 | Runtime preload for shared chunks | Shared chunks are emitted as metadata/files, but route scripts remain compatibility self-contained. | Browser integration test verifies preload tags and route hydration. |
| P1 | Plugin source maps | `transform` may return maps, but native bridge currently forwards only code. | Source map fixture with transformed line mapping. |
| P2 | Full parser compatibility suite | AST facts are lightweight; add cases for advanced TS/JSX grammar. | Parser fixture suite and native bundler tests. |
| P2 | Config dependency invalidation | Config/plugin changes should invalidate all affected compile caches explicitly. | Change plugin code and verify rebuilt output changes. |
| P2 | Adapter consumption of chunk manifest | Adapters should copy/use `chunk-manifest.json` consistently. | Adapter tests inspect produced deployment output. |
| P3 | Dependency pre-bundling | Can improve large dependency-heavy dev startup. | Benchmark cold dev startup with large dependency graph. |

## Release Gate

Before a release that claims production readiness, run:

```powershell
cargo test --workspace
pnpm -r build
pnpm -r test
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\test-full-flow.ps1
```

Warnings for unsupported optional platform packages on non-target OSes are
expected during pnpm workspace commands.
