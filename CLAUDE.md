# Claude Instructions

Read `AGENTS.md` first. It is the source of truth for working in this Ruvyxa monorepo.

Important local checks (run these before submitting changes):

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --locked -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
pnpm pack:smoke
cargo run -p ruvyxa_cli -- check --root examples/demo
```

Commands for working with the example app:

```bash
cargo run -p ruvyxa_cli -- dev --root examples/demo
cargo run -p ruvyxa_cli -- build --root examples/demo
cargo run -p ruvyxa_cli -- start --root examples/demo --port 3000
```

## Project Structure

This is a Rust + TypeScript monorepo. The native CLI is the Rust crate `ruvyxa_cli`. JS/TS packages
live in `packages/`. The bundler (`ruvyxa_bundler`) handles all TypeScript/JSX compilation,
tree-shaking, minification, and source maps natively.

- **crates/** — Rust workspace: CLI, bundler, dev server, graph, middleware, diagnostics
- **packages/** — npm packages: CLI wrapper, runtime, adapters, core primitives
- **tests/** — Node package tests (organized by package)
- **examples/demo/** — full-featured demo app
- **templates/** — templates for `create-ruvyxa` (minimal, blog, crud, api-backend)
- **docs/** — user-facing documentation
