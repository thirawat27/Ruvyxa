# Contributing to Ruvyxa

Thanks for your interest in contributing. This guide covers development setup, conventions, and how
to submit changes.

---

## Development Setup

### Prerequisites

- [Rust](https://rustup.rs/) (1.96+)
- [Node.js](https://nodejs.org/) (24.19+ LTS)
- [pnpm](https://pnpm.io/) (11+)

### Clone and Install

```bash
git clone https://github.com/thirawat27/ruvyxa.git
cd ruvyxa
```

On Windows, run:

```bat
setup.bat
```

On macOS or Linux, run:

```bash
./setup.sh
```

Both scripts install the locked dependencies, build workspace packages, and compile the Ruvyxa CLI.

`pnpm install`, which is run by both setup scripts, enables the repository pre-commit hook. It
automatically formats staged files that Prettier supports and re-stages only those files. When Rust
files are staged, the hook verifies their `cargo fmt` formatting before allowing the commit.

### Verify Everything Works

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --locked -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
pnpm check:unused
```

### Run the Example App

```bash
cargo run -p ruvyxa_cli -- dev --root examples/demo
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
│   ├── ruvyxa_middleware/     # Tower middleware plus the TypeScript plugin host bridge
│   └── ruvyxa_diagnostics/    # Structured error types (RUV#### codes)
├── packages/                  # TypeScript packages (npm)
│   ├── ruvyxa/                # Main package (CLI wrapper + runtime Node scripts)
│   ├── create-ruvyxa/         # Project scaffolding
│   └── @ruvyxa/               # Scoped packages (core, react, adapters, cli-*)
├── examples/demo/             # Full-featured demo app with all features
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
- Update `examples/demo/` when a feature needs demonstration.

### Worker pool changes

The worker boundary crosses Rust and Node. Before editing it, read the ownership map and invariants
in [Architecture](docs/en/11-architecture.md#worker-pool-boundary), then use the focused
verification matrix in
[Development and testing](docs/en/12-development-testing.md#worker-pool-change-matrix). If
`worker-pool.mjs` imports a new local runtime module, include it in the `ruvyxa` package and in the
prerender runtime fingerprint; the runtime contract test and `pnpm pack:smoke` enforce both.

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
cargo clippy --workspace --locked -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
pnpm check:unused
```

`check:unused` runs [Knip](https://knip.dev) and fails on unused files, exports, types, and
dependencies across the JavaScript/TypeScript workspaces; `release:validate` runs it too, so it
gates a release. If a new runtime module or dynamically loaded package reports as unused, check for
a dynamic or path-based loader before deleting it, then declare it in `knip.json` if it is genuinely
loaded by convention rather than by import.

### 4. Smoke test both modes

For runtime changes:

```bash
cargo run -p ruvyxa_cli -- dev --root examples/demo --port 3001
cargo run -p ruvyxa_cli -- build --root examples/demo
cargo run -p ruvyxa_cli -- start --root examples/demo --port 3002
```

### 5. Run parity check

```bash
cargo run -p ruvyxa_cli -- test:parity --root examples/demo
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
4. Add it to `docs/en/16-troubleshooting-upgrades.md` and its Thai counterpart when users need the
   symptom and fix, not just the message at the point it is raised.

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
4. Document its user-facing setup in `docs/en/15-deploy-run-and-operate.md` and its Thai
   counterpart, following `docs/en/20-platform-adapter-guide.md`.
5. Add its release checks to `docs/en/19-release-readiness-playbook.md`.

---

## License

By contributing, you agree that your contributions will be licensed under the
[Apache 2.0 License](LICENSE).
