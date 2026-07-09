# Claude Instructions

Read `AGENTS.md` first. It is the source of truth for working in this Ruvyxa monorepo.

Important local checks (run these before submitting changes):

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
cargo run -p ruvyxa_cli -- check --root examples/kitchen-sink
```

Commands for working with the example app:

```bash
cargo run -p ruvyxa_cli -- dev --root examples/kitchen-sink
cargo run -p ruvyxa_cli -- build --root examples/kitchen-sink
cargo run -p ruvyxa_cli -- start --root examples/kitchen-sink --port 3000
```

## Project Structure

This is a Rust + TypeScript monorepo. The native CLI is the Rust crate `ruvyxa_cli`. JS/TS packages
live in `packages/`. The bundler (`ruvyxa_bundler`) handles all TypeScript/JSX compilation,
tree-shaking, minification, and source maps natively.

- **crates/** — Rust workspace: CLI, bundler, dev server, graph, middleware, diagnostics
- **packages/** — npm packages: CLI wrapper, runtime, adapters, core primitives
- **tests/** — Node package tests (organized by package)
- **examples/kitchen-sink/** — full-featured demo app
- **templates/minimal/** — template for `create-ruvyxa`
- **docs/** — user-facing documentation
