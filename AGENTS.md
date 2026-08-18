# Ruvyxa Monorepo Agent Guide

You are working in the Ruvyxa framework monorepo. Treat this file as the source of truth for agent
work in the repository.

## Repository Shape

- `crates/` contains the Rust workspace.
- `crates/ruvyxa_cli` owns the Ruvyxa CLI commands: `dev`, `build`, `check`, `start`, `preview`,
  `routes`, `analyze`, `doctor`, `clean`, `trace`, `bench`, and `test:parity`.
- `crates/ruvyxa_bundler` owns TypeScript/JSX compilation, module resolution, linking, minification,
  source maps, and server/client boundary checks.
- `crates/ruvyxa_dev_server` owns Axum serving, HMR, render cache, router, worker pool, style
  collection, and action/API/client endpoints.
- `crates/ruvyxa_graph` owns file-system route discovery, validation, rendering strategy detection,
  and route manifests.
- `packages/` contains npm packages: `ruvyxa`, `create-ruvyxa`, `@ruvyxa/core`, `@ruvyxa/react`,
  adapters, and platform CLI packages.
- `examples/demo/` is the broad integration fixture.
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
pnpm lint
pnpm format:check
pnpm check:unused
pnpm release:validate
pnpm pack:smoke
```

`pnpm lint` runs Oxlint over `packages/`, `templates/`, `examples/`, `scripts/`, and `tests/`.
`pnpm -r check` and `pnpm release:validate` both call it, so it gates a release. The rules it
enforces and, just as importantly, the ones it deliberately turns off are in `.oxlintrc.json`, each
with the reason beside it: sequential `await` in a loop is how the bundler applies backpressure,
`void` before a floating promise is the marker TypeScript itself recommends, and `__RUVYXA_*`
globals are a contract with the generated entry, not a naming slip. Add a rule to the off list only
with the reason it does not apply here, never to clear a finding.

It also caps structure directly: `complexity` at 30, `max-depth` at 4, `max-nested-callbacks` at 4,
and `max-params` at 8. `max-lines-per-function` is deliberately off — a long flat function is a
reading cost, a branchy one is a correctness risk, and only the second is worth failing a build
over. When a function trips `complexity`, split it along the seam it already has (a strategy per
branch, a section per validator) rather than hoisting fragments out to move the number.

The Rust side has the matching gate: `.clippy.toml` sets `cognitive-complexity-threshold` and
`[workspace.lints.clippy]` in `Cargo.toml` turns the lint on, so a function that grows past what one
screen holds fails `cargo clippy -- -D warnings`. The threshold sits well above Clippy's default
because a `match` over twenty token shapes is one decision to a reader; where the metric is inflated
by macro expansion rather than real branching, `#[allow]` it on that function with the reason.

`pnpm check:unused` runs Knip over the JavaScript/TypeScript workspaces and fails on unused files,
exports, types, and dependencies. `pnpm release:validate` runs it too, so it gates a release. Ruvyxa
loads a lot of code by convention rather than by import — `app/` routes, `plugins/`,
`ruvyxa.config.ts`, the `runtime/*.mjs` modules the Rust CLI resolves by path, and adapters resolved
from a `@ruvyxa/adapter-${name}` template string — so `knip.json` declares those as entry points or
ignored dependencies. When a new runtime module or dynamically loaded package reports as unused, add
it there rather than deleting it; check for a dynamic or path-based loader first. Knip must stay on
version 6 or newer: version 5 crashes against this repository's TypeScript 7.

For demo behavior changes, also run:

```bash
cargo run -p ruvyxa_cli -- check --root examples/demo
cargo run -p ruvyxa_cli -- test:parity --root examples/demo
```

## Change Guidance

- Route matching has one JavaScript implementation, `packages/@ruvyxa/core/src/route-match.ts`.
  `packages/ruvyxa/runtime/route-match.mjs` is a committed copy of its compiled output, because the
  serverless handler is copied into function bundles that resolve no bare specifiers. After changing
  the shared module run `pnpm --filter ruvyxa sync:route-match` and commit both; `ruvyxa`'s build
  fails on a stale copy. The Rust router cannot share the module, so both languages are held to
  `tests/fixtures/route-match-conformance.json` — add a case there before changing match behavior.
- Rust shared behavior needs Rust tests near the changed crate.
- Runtime/config/package behavior needs Node tests under `tests/packages/**`. TypeScript suites go
  through `scripts/test-package.mjs <suite>`, which compiles them with `tsc -p tsconfig.test.json`
  into `.test-build/packages/<suite>/` and runs that output, so they are type-checked and never rely
  on a runtime that strips types. A new suite needs a `tsconfig.test.json` in its package (extending
  `tsconfig.test-base.json`) and a `test` script calling that runner. Import the package under test
  through its built `dist/*.js` — a `src/*.ts` import would be compiled into `.test-build/` as a
  second copy, and any module that resolves its own `import.meta.url` (adapter `package.json` reads,
  the `create-ruvyxa` template lookup) would then resolve against the wrong directory. Use
  `repoPath()` from `tests/repo-root.ts` to reach a repository file rather than walking up from
  `import.meta.url`.
- Template changes should stay package-manager neutral and must match
  `templates/minimal/package.json`.
- `templates/minimal/app/components/ruvyxa-runner.tsx` and
  `examples/demo/app/components/ruvyxa-runner.tsx` are required to be byte-identical, so the demo
  fixture exercises exactly what a scaffolded project ships with. After editing either, run
  `node scripts/check-template-mirrors.mjs` to resync and commit both; `pnpm release:validate` fails
  on drift. Five commits edited both copies by hand before this existed, and a real defect (a
  projectile scoring against two targets in one frame) lived in both copies as a result.
- A cross-language table that cannot be shared as code — a Content-Type map, a security-header list,
  a route-matching rule — belongs in `tests/fixtures/*-conformance.json` with a test in each
  language that replays it, not in a comment promising the two stay in sync. `env_read_is_private`
  (private env var policy), `STATIC_CONTENT_TYPES` (static asset Content-Type), and
  `DEFAULT_SECURITY_HEADERS` all drifted silently before gaining this. `env_read_is_private` was
  listed here while `tests/fixtures/env-policy-conformance.json` did not yet exist and the rule was
  still held by a comment alone — when this list names a fixture, check that the file is there.
- Documentation changes should describe actual supported behavior, not intended future behavior.
- If a check was already failing before your work, report it as baseline and do not weaken tests to
  pass.
