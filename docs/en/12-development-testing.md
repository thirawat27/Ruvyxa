# Development and testing

> **Tutorial goal:** set up a contributor loop and choose the smallest test that proves a change.
> **Start from:** the boundary map in [Architecture](11-architecture.md). **Checkpoint:** run the
> narrowest relevant check before choosing a broader repository gate.

## Framework contributor setup

This is a Rust workspace plus pnpm workspace. Install the declared Node version and pnpm, then use a
Rust toolchain compatible with the locked workspace.

```bash
pnpm install
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --locked -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
pnpm lint
pnpm format:check
pnpm check:unused
pnpm release:validate
pnpm pack:smoke
```

`pnpm release:validate` is the umbrella gate: it runs `lint` and then the individual repository
checks — package metadata, the release publish plan, `oxc` lockstep, Markdown links, repository
paths named in source comments, doc-comment attachment, silent defaults, cross-language constants,
template mirrors, adapter sync, and Knip. Run the narrower one directly while iterating; the
per-check triggers are listed in the repository guide [`AGENTS.md`](../../AGENTS.md).

For the broad fixture, use the exact commands established by the repository guide:

```bash
cargo run -p ruvyxa_cli -- check --root examples/demo
cargo run -p ruvyxa_cli -- test:parity --root examples/demo
```

## Test layers

Rust tests live with the relevant crate and cover CLI/graph/bundler/server behavior. Package tests
run through Node's built-in test runner; package manifests point at `tests/packages/**` or
package-local tests. `@ruvyxa/react` has tests for the client router. `@ruvyxa/testing` lets a unit
test create a loader/action/cache double and inspect its calls and invalidations.

```ts
import test from 'node:test'
import assert from 'node:assert/strict'
import { mockAction } from '@ruvyxa/testing'

test('records invalidation', async () => {
  const save = mockAction(({ input, invalidate }) => {
    invalidate('todos')
    return input
  })
  await save({ title: 'Write docs' })
  assert.deepEqual(save.invalidations, ['todos'])
})
```

The current repository has CI workflows at `.github/workflows/ci.yml` and
`.github/workflows/release.yml`. Do not claim an individual job's exact command without reading the
workflow at the revision you are changing; workflows can evolve independently of package scripts.

## Worker-pool change matrix

| Change                                    | Primary owner                                    | Focused proof                                                                               |
| ----------------------------------------- | ------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| Admission, fairness, queue bounds, close  | `runtime/worker-admission.mjs`                   | `node --test tests/packages/ruvyxa/worker-admission.test.mjs`                               |
| Node dispatch, rendering, cache, protocol | `runtime/worker-pool.mjs`                        | `node --test tests/packages/ruvyxa/worker-pool.test.mjs`                                    |
| Process selection, replacement, transport | `crates/ruvyxa_dev_server/src/worker_pool.rs`    | `cargo test -p ruvyxa_dev_server worker_pool --locked`                                      |
| Runtime files or imports                  | package manifest, pack smoke, CLI artifact cache | `node --test tests/packages/ruvyxa/worker-runtime-contract.test.mjs` then `pnpm pack:smoke` |

Run the row that owns the changed behavior first, then `pnpm --filter ruvyxa test` and the relevant
Rust crate tests. A new local import from `worker-pool.mjs` must be published by
`packages/ruvyxa/package.json` and fingerprinted by `crates/ruvyxa_cli/src/artifact_cache.rs` so an
installed CLI can load it and prerendering cannot reuse stale output. Keep protocol changes additive
when possible and update both Rust serde types and Node tests when a field changes.

## Definition of done

For a public framework change, update the Rust/TypeScript contract, tests, templates where
applicable, and both language editions in `docs/`. Run the narrowest relevant test during iteration,
then the broader checks above before handoff. Do not commit generated `.ruvyxa/`, `dist/`,
`target/`, `node_modules/`, or package smoke directories.

**Previous:** [Architecture](11-architecture.md) · **Next:** [Security](13-security.md)
