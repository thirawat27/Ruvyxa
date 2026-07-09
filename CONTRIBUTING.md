# Contributing to Ruvyxa

Thanks for your interest in contributing. This guide covers development setup, conventions, and how
to submit changes.

---

## Development Setup

### Prerequisites

- [Rust](https://rustup.rs/) (1.96+)
- [Node.js](https://nodejs.org/) (22+)
- [pnpm](https://pnpm.io/) (11+)

### Clone and Install

```bash
git clone https://github.com/thirawat27/ruvyxa.git
cd ruvyxa
pnpm install
```

### Verify Everything Works

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace -- -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
```

### Run the Example App

```bash
cargo run -p ruvyxa_cli -- dev --root examples/kitchen-sink
```

Open [http://localhost:3000](http://localhost:3000).

---

## Project Structure

```
ruvyxa/
├── crates/                    # Rust crates
│   ├── ruvyxa_bundler/        # TS/JSX bundler: compiler, resolver, linker, minifier, source maps
│   ├── ruvyxa_cli/            # CLI binary (dev, build, check, start, routes, analyze, etc.)
│   ├── ruvyxa_dev_server/     # Dev + production HTTP server, HMR, render cache, worker pool
│   ├── ruvyxa_graph/          # Route discovery, validation, rendering strategy detection
│   ├── ruvyxa_middleware/     # Tower middleware (CORS, timing, logging, rate limit) + Wasm plugins
│   └── ruvyxa_diagnostics/    # Structured error types (RUV#### codes)
├── packages/                  # TypeScript packages (npm)
│   ├── ruvyxa/                # Main package (CLI wrapper + runtime Node scripts)
│   ├── create-ruvyxa/         # Project scaffolding
│   └── @ruvyxa/               # Scoped packages (core, react, adapters, cli-*)
├── examples/kitchen-sink/     # Full-featured demo app with all features
├── templates/minimal/         # Template for new user projects
├── tests/                     # Node package tests (organized by package)
└── docs/                      # User-facing documentation
```

---

## Conventions

### Rust

- Use `cargo fmt` formatting. No exceptions.
- All warnings are errors (`-D warnings` in CI).
- Use structured diagnostics with `RUV####` codes for user-facing errors.
- Add tests for behavior changes to route discovery, validation, or bundling.
- Keep errors explicit — do not silently ignore invalid state.

### TypeScript

- Public APIs must be typed. Export types alongside values.
- Keep package entry points small and focused.
- Avoid adding runtime dependencies unless they serve user-facing functionality.
- Use Node built-in test runner (`node --test`) for tests.

### General

- Keep dev and production behavior aligned. Shared logic goes in shared paths.
- Build validation must catch boundary leaks before output is emitted.
- Update `templates/minimal/` when a feature affects new projects.
- Update `examples/kitchen-sink/` when a feature needs demonstration.

---

## Making Changes

### 1. Create a branch

```bash
git checkout -b feature/my-change
```

### 2. Make your changes

- Read existing code before writing new code. Match the patterns.
- Keep changes focused. One concern per PR.
- Add or update tests for new behavior.

### 3. Run the checks

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace -- -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
```

### 4. Smoke test both modes

For runtime changes:

```bash
cargo run -p ruvyxa_cli -- dev --root examples/kitchen-sink --port 3001
cargo run -p ruvyxa_cli -- build --root examples/kitchen-sink
cargo run -p ruvyxa_cli -- start --root examples/kitchen-sink --port 3002
```

### 5. Run parity check

```bash
cargo run -p ruvyxa_cli -- test:parity --root examples/kitchen-sink
```

### 6. Submit a PR

- Write a clear title (< 70 characters).
- Describe what changed, why, and what you tested.
- Link related issues.

---

## Commit Messages

Use clear, imperative-mood commit messages:

```
feat: add optional dynamic route segments [[name]]
fix: prevent duplicate route detection for group routes
docs: update routing documentation for catch-all routes
test: add boundary validation tests for server/ imports
```

---

## Adding a Diagnostic Code

When adding a new error that users will see:

1. Choose the next available `RUV####` code in the relevant range.
2. Create a `Diagnostic` with `code`, `title`, `explanation`, and `suggested_fix`.
3. Add the file location with `.at_file()`.
4. Document it in `docs/debugging.md`.

```rust
Diagnostic::new("RUV1011", "Your error title")
    .explain("Why this happened.")
    .at_file(&file_path)
    .suggest("How to fix it.")
```

---

## Adding an Adapter

1. Create `packages/@ruvyxa/adapter-<name>/`.
2. Implement the `Adapter` interface from `@ruvyxa/core`.
3. Add a `package.json` with `@ruvyxa/core` as a dependency.
4. Document it in `docs/deployment.md`.
5. Add it to the publish procedure in `docs/publishing.md`.

---

## License

By contributing, you agree that your contributions will be licensed under the
[Apache 2.0 License](LICENSE).