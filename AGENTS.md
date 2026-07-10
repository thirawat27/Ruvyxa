# Ruvyxa Monorepo Agent Guide

You are working in the Ruvyxa framework monorepo. Treat this file as the source of truth for agent
work in the repository.

## Repository Shape

- `crates/` contains the Rust workspace.
- `crates/ruvyxa_cli` owns the native CLI commands: `dev`, `build`, `check`, `start`, `preview`,
  `routes`, `analyze`, `doctor`, `clean`, `trace`, `bench`, and `test:parity`.
- `crates/ruvyxa_bundler` owns TypeScript/JSX compilation, module resolution, linking, minification,
  source maps, and server/client boundary checks.
- `crates/ruvyxa_dev_server` owns Axum serving, HMR, render cache, router, worker pool, style
  collection, and action/API/client endpoints.
- `crates/ruvyxa_graph` owns file-system route discovery, validation, rendering strategy detection,
  and route manifests.
- `packages/` contains npm packages: `ruvyxa`, `create-ruvyxa`, `@ruvyxa/core`, `@ruvyxa/react`,
  adapters, and platform CLI packages.
- `examples/kitchen-sink/` is the broad integration fixture.
- `templates/minimal/` is copied into new projects by `create-ruvyxa`.
- `docs/` is user-facing documentation.

## Operating Rules

- Preserve public CLI, config, package, and route behavior unless the task explicitly changes it.
- Read the existing module and its tests before editing shared framework behavior.
- Keep Rust and TypeScript contracts aligned when changing config, runtime files, package exports,
  or generated template behavior.
- Do not commit generated output such as `.ruvyxa/`, `dist/`, `.npm-pack/`, `.npm-smoke/`,
  `target/`, or `node_modules/`.
- Keep browser-safe env vars prefixed with `RUVYXA_PUBLIC_`; private env vars must stay server-only.
- Preserve the server/client boundary: `server-only`, `client-only`, `server/` imports, and private
  env access must continue to be caught by validation.
- For styling changes, keep dev, build, HMR, prerender, docs, and templates in agreement. Imported
  project CSS may live outside `app/`; unimported global styles should use `css.entries`.
- For npm packaging changes, verify that packed tarballs do not include tests or `workspace:`
  protocol dependencies and that runtime files needed by the CLI are included.

## Verification

Use the narrowest useful check while iterating. Before handing off broad framework, runtime,
template, or packaging changes, run the relevant subset of:

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --locked -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
pnpm format:check
pnpm release:validate
pnpm pack:smoke
```

For kitchen-sink behavior changes, also run:

```bash
cargo run -p ruvyxa_cli -- check --root examples/kitchen-sink
cargo run -p ruvyxa_cli -- test:parity --root examples/kitchen-sink
```

## Change Guidance

- Rust shared behavior needs Rust tests near the changed crate.
- Runtime/config/package behavior needs Node tests under `tests/packages/**`.
- Template changes should stay package-manager neutral and must match
  `templates/minimal/package.json`.
- Documentation changes should describe actual supported behavior, not intended future behavior.
- If a check was already failing before your work, report it as baseline and do not weaken tests to
  pass.
