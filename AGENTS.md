# Ruvyxa Monorepo Agent Guide

You are working on Ruvyxa, a Rust-powered full-stack TypeScript framework.

## Architecture

- `crates/` contains Rust CLI, dev server, graph, and diagnostics code.
- `packages/` contains public TypeScript APIs and Node renderer bridges.
- `examples/basic-app/` is the runnable integration app.
- `templates/minimal/` is copied into new user apps.
- `docs/` contains user-facing framework documentation.

## Engineering Rules

- Keep dev and production behavior aligned. If `dev` and `start` share semantics, put that logic in shared runtime paths rather than command-specific branches.
- Prefer structured diagnostics with `RUV####` codes over generic errors.
- Build validation must catch server/client boundary leaks before production output is emitted.
- Do not introduce a custom JS runtime. Use Node for the current renderer bridge and Rust for CLI/dev/build core.
- Keep public TypeScript APIs typed and small.
- Update templates when a feature affects newly created apps.

## Verification

Run these before handing off framework changes:

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
cargo run -p ruvyxa_cli -- analyze --root examples/basic-app
```

For runtime changes, smoke test both modes:

```bash
cargo run -p ruvyxa_cli -- dev --root examples/basic-app --port 3001
cargo run -p ruvyxa_cli -- build --root examples/basic-app
cargo run -p ruvyxa_cli -- start --root examples/basic-app --port 3002
```
