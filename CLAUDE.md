# Claude Instructions

**Read [`AGENTS.md`](AGENTS.md) first.** It is the source of truth for working in this repository:
the repository shape, the operating rules, what each check gates, and the change guidance written
from defects that actually shipped. This file does not repeat it — it orients a session and says
what is specific to working here as an agent.

## What this repository is

Ruvyxa is a web framework whose compiler and server are **Rust**, and whose runtime and public API
are **TypeScript**. That split is the single most important thing to hold in mind, because most of
the expensive mistakes here come from one half of a rule being changed without the other:

- `crates/` — the CLI (`ruvyxa_cli`), the bundler (`ruvyxa_bundler`: TypeScript/JSX compilation,
  resolution, linking, minification, source maps, boundary checks), the dev/production server
  (`ruvyxa_dev_server`), route discovery (`ruvyxa_graph`), middleware (`ruvyxa_middleware`),
  diagnostics, and the terminal UI.
- `packages/` — `ruvyxa` (the framework package, including the `runtime/*.mjs` modules the Rust CLI
  spawns or imports by path), `create-ruvyxa`, `@ruvyxa/core`, `@ruvyxa/react`, four optional
  integrations, eleven deploy adapters, and five prebuilt CLI binaries.
- `examples/demo` is the broad fixture and is deliberately **not** deployable;
  `examples/deploy-smoke` is the one every adapter can deploy, and CI runs it on real Node, Bun, and
  Deno.

There is no esbuild, no Vite, no Webpack. The bundler is ours.

## Working here

- **Never run `git commit`, `git push`, or `git add`.** The owner commits their own work. Stop at
  _verified and reported_: say what changed, what you ran, and what came back.
- Preserve changes you did not make. This working tree sometimes carries the owner's own edits from
  another terminal — check `git diff HEAD`, not just `git diff`, because staged changes do not
  appear in the latter.
- Run the narrowest check that can fail while iterating, and the relevant subset of the full battery
  before handing off. Both lists, and what each one gates, are in `AGENTS.md`.
- Prefer a fixture over a promise. When two implementations must agree and cannot share code, the
  answer is a file in `tests/fixtures/` replayed by a test in each language — not a comment saying
  they are kept in sync. `AGENTS.md` lists the rules that drifted before they had one.

## The checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm -r build
pnpm -r check
pnpm -r test
pnpm lint
pnpm format:check
pnpm check:unused
pnpm release:validate
pnpm pack:smoke
```

`pnpm -r build` first if a JavaScript suite fails on a missing export — that is almost always a
stale `dist/`, not a deleted file.

## The example app

```bash
cargo run -p ruvyxa_cli -- dev --root examples/demo
cargo run -p ruvyxa_cli -- build --root examples/demo
cargo run -p ruvyxa_cli -- start --root examples/demo --port 3000
cargo run -p ruvyxa_cli -- check --root examples/demo
cargo run -p ruvyxa_cli -- test:parity --root examples/demo
```

## The traps that cost the most

Each is explained in `AGENTS.md`; this is the index, so a session recognises the shape before
spending an hour on it.

| Shape                               | Why it bites                                                                                                                                                             |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Two module graphs**               | The Rust resolver and `runtime/compiler.mjs` both resolve imports. Teaching one a new specifier and not the other breaks SSG silently.                                   |
| **Two request hosts**               | Axum serves `dev`/`start`; `createHandler` serves every deployed build. A feature added to one only works where it was added.                                            |
| **One source scanner per language** | `ruvyxa_bundler::ast` in Rust, `runtime/scanner.mjs` in JavaScript. A hand-rolled second scanner has caused the same class of silent bug five times.                     |
| **Cache identity is derived**       | No hand-maintained `CACHE_VERSION`/`-v2` stamp decides reuse. A stamp is only correct while somebody remembers it, and forgetting is silent.                             |
| **Ordering is a contract**          | Sort order decides cache keys, fingerprints, and emitted bytes, so `localeCompare` is banned outright — it answers by the host's ICU locale.                             |
| **Windows paths**                   | `canonicalize` returns the `\\?\` prefix; anything used as a key goes through `normalized_canonical_path`.                                                               |
| **The linker is line-based**        | Both linkers rewrite ESM one line at a time; a multi-line export construct has broken it twice.                                                                          |
| **Registration lists**              | A new `runtime/*.mjs` must be added to `package.json` `files`, to `WORKER_RUNTIME_FILES`, and to the standalone-copy tests, or it is missing exactly where nobody looks. |
