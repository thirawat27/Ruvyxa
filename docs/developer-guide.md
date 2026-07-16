# Ruvyxa Developer Guide

This guide is for framework contributors: people changing the Rust workspace, native CLI, npm
packages, adapters, templates, runtime, or integration fixtures. Application authors should begin
with the [User Guide](guides/index.md).

## 1. Local requirements and setup

Install Node.js 22 or later, pnpm 11 or later, and Rust 1.96 or later (Rust edition 2024). Then
install dependencies and inspect the integration fixture:

```bash
pnpm install
cargo run -p ruvyxa_cli -- doctor --root examples/demo
cargo run -p ruvyxa_cli -- routes --root examples/demo
```

Do not commit generated output: `target/`, `node_modules/`, `.ruvyxa/`, `dist/`, `.npm-pack/`, and
`.npm-smoke/`. Preserve the distinction between browser-safe `RUVYXA_PUBLIC_` variables and
server-only secrets at every layer.

## 2. Repository map

```text
npm package: ruvyxa
  └─ bin/ruvyxa.js -> platform-specific native CLI
       ├─ crates/ruvyxa_cli          commands, config loading, build orchestration
       ├─ crates/ruvyxa_graph        route discovery, render detection, validation
       ├─ crates/ruvyxa_bundler      TS/JSX/MDX compilation, resolution, linking, maps, Oxc-backed minification
       ├─ crates/ruvyxa_dev_server   Axum server, HMR, router, cache, Node worker pool, CSS minification
       ├─ crates/ruvyxa_middleware   Tower middleware and Wasm plugin support
       └─ crates/ruvyxa_diagnostics  structured RUV#### diagnostics

packages/
  ├─ ruvyxa                    CLI launcher, runtime bridge, public re-exports
  ├─ @ruvyxa/core              config and server APIs, types, adapter contracts
  ├─ @ruvyxa/react             Image, Seo, hydration, loaders, error boundaries
  ├─ @ruvyxa/adapter-*         deployment metadata packages
  ├─ @ruvyxa/cli-*             platform-specific native binaries (darwin-arm64, linux-arm64, linux-x64, win32-arm64, win32-x64)
  └─ create-ruvyxa             scaffold command and minimal template packaging
```

The [Bundler Modernization doc](architecture/bundler-modernization.md) describes the oxc integration
boundary and the reasoning behind keeping resolution, linking, and diagnostics in Ruvyxa while
delegating minification to Oxc.

Framework contracts often span Rust and TypeScript. A change to configuration, runtime files,
package exports, or starter behaviour must be checked in both places. Do not change a TypeScript
type and assume the native CLI will accept it: `ruvyxa_cli` deserializes a strict runtime
configuration independently.

## 3. Working loop and verification

Read the touched module, direct callers, tests, and the closest demo example before editing. Start
with the narrowest useful check and expand only when shared behaviour changes.

```bash
# Targeted Rust work
cargo test -p ruvyxa_graph --locked
cargo test -p ruvyxa_cli --locked

# Targeted JavaScript package work
pnpm --filter ruvyxa test
pnpm --filter ruvyxa check

# End-to-end application signal
cargo run -p ruvyxa_cli -- analyze --root examples/demo
cargo run -p ruvyxa_cli -- check --root examples/demo
cargo run -p ruvyxa_cli -- test:parity --root examples/demo
```

Before handing off framework, runtime, template, or packaging work, run the applicable broad checks:

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

If Windows reports that `target/debug/ruvyxa.exe` is locked, stop the development server or other
process holding that executable before retrying. Do not delete the whole `target/` directory merely
to hide a file-lock problem.

## 4. Change map

| Change                                                     | Primary surface                               | Minimum proof                         |
| ---------------------------------------------------------- | --------------------------------------------- | ------------------------------------- |
| CLI command, config parsing, build orchestration           | `crates/ruvyxa_cli/src/main.rs`               | relevant Rust test plus demo `check`  |
| route matching, validation, rendering detection            | `crates/ruvyxa_graph/src/lib.rs`              | graph test plus `routes`/`analyze`    |
| compilation, linking, source maps, Oxc-backed minification | `crates/ruvyxa_bundler`                       | bundler tests plus demo build         |
| CSS collection, minification, style HMR                    | `crates/ruvyxa_dev_server/src/style.rs`       | crate tests plus demo build           |
| API/action/HMR/server behaviour                            | `crates/ruvyxa_dev_server`                    | crate tests plus parity               |
| core config or server API                                  | `packages/@ruvyxa/core/src`                   | package test/check                    |
| npm launcher or runtime script                             | `packages/ruvyxa`                             | package test and `pnpm pack:smoke`    |
| generated starter                                          | `templates/minimal`, `packages/create-ruvyxa` | create-package test and pack smoke    |
| cross-cutting application behaviour                        | `examples/demo`                               | `analyze`, `check`, and `test:parity` |

Add a Rust test beside shared Rust behaviour. Add a Node test under `tests/packages/**` when
changing a public config, runtime, package, or template contract. Never weaken an existing test just
to make a change pass.

## 5. Public contracts that must remain aligned

### CLI

The supported command surface is:

```text
dev, build, check, start, preview, routes, analyze, doctor,
clean, trace, bench, test:parity (with parity alias)
```

Preserve command names, option names, output semantics, and the public build/root defaults unless
the change explicitly introduces a breaking release.

### Configuration

`ruvyxa.config.ts` is a strict contract. The core package defines TypeScript types, while the native
CLI validates and deserializes the runtime representation. When adding a field:

1. Add the type and documentation in `packages/@ruvyxa/core`.
2. Add the matching Rust config field with the correct camelCase mapping.
3. Validate unsafe or impossible values in Rust.
4. Wire the value to development and production server/build paths.
5. Add tests for accepted and rejected values.
6. Update the user guide if an app author can use the option.

Unknown configuration keys intentionally fail rather than being ignored. This prevents configuration
typos from silently changing deployment behaviour.

### Routes, rendering, and boundaries

- Reject duplicate and ambiguous routes rather than applying undocumented precedence.
- Preserve the rendering detection order: client directive, PPR, ISR, `getStaticParams`, static
  candidate, then SSR.
- Preserve server/client validation for `server-only`, `client-only`, `server/` imports, and private
  environment access.
- Keep private variables server-only; only `RUVYXA_PUBLIC_` values can enter client bundles.

### Packaging

Published tarballs must not contain tests or `workspace:` protocol dependencies. They must include
every runtime script, template file, platform binary, and launcher required by the public command.

## 6. Diagnostics

User-visible framework diagnostics use the `RUV####` format. A new diagnostic should include:

1. A code in the appropriate range.
2. A concise title.
3. An explanation of the violated contract.
4. The file location when known.
5. A concrete suggested fix.

Do not emit a generic build error when the framework can identify the source route, import,
configuration field, or boundary violation. Add tests for the new diagnostic and update the
appropriate English guide if users need to act on it.

## 7. Templates and scaffold packaging

The source starter is `templates/minimal/`. `packages/create-ruvyxa/scripts/prepare-template.mjs`
copies it into the ignored package template before packing. Keep both observable starter behaviour
and package tests aligned.

npm omits nested `.gitignore` files from package tarballs. The prepare script therefore renames the
packaged copy to `gitignore`; the scaffold restores it as `.gitignore` in the generated application.
This is intentional and is covered by `pnpm pack:smoke`. Do not replace it with an npm ignore rule
that removes the starter's ignore file again.

The starter uses the normal npm binary:

```json
"build": "ruvyxa build"
```

Keep `dev`, `build`, `start`, and `check` consistent with this standard pattern when changing the
starter. The package, rather than every consuming application, is responsible for publishing the
launcher with executable permission.

## 8. Vercel executable-bit regression

`ruvyxa` declares an npm binary at `packages/ruvyxa/bin/ruvyxa.js`. That file must be executable
(`100755`) in Git and in the published tarball. Otherwise environments such as Vercel can fail
before the framework starts:

```text
node_modules/.bin/ruvyxa: Permission denied
```

Verify both layers:

```bash
git ls-files --stage packages/ruvyxa/bin/ruvyxa.js
pnpm pack:smoke
```

The Git mode must begin with `100755`. Pack smoke checks the tar header, runs the extracted Ruvyxa
launcher through Node, verifies the packed create command, and confirms that its generated
application contains `.gitignore`.

When changing launcher behaviour, native binary discovery, optional platform packages, or package
`files` lists, always run `pnpm release:validate` and `pnpm pack:smoke`. Do not rely only on a
workspace symlink: published package contents and permissions are the deployment contract.

## 9. The demo as an integration system

`examples/demo` is more than a showcase. It exercises static, dynamic, catch-all, API, action, MDX,
environment, style, and rendering-strategy behaviour through the same paths users run. Use it to
localize flexibility and parity problems:

```bash
pnpm --dir examples/demo doctor
pnpm --dir examples/demo routes
pnpm --dir examples/demo analyze
pnpm --dir examples/demo typecheck
pnpm --dir examples/demo check
```

Use `analyze` first for route/import/boundary problems. Use `check` for type checking, production
build, development/production route comparison, and page smoke rendering. Use `trace` to inspect one
manifest entry. Do not hard-code framework versions or route counts in the demo health endpoint;
`doctor` and `routes` are the runtime sources of truth.

## 10. Known boundaries and honest documentation

Document only what source code and tests support.

- Rendering strategy selection is source scanning with a documented precedence order; recommend
  explicit route exports for important deployment behaviour.
- Configuration paths are restricted to the project root to prevent traversal. External styles or
  assets need a project-local import/copy strategy.
- Adapter packages return typed output metadata. The config renderer executes `adapter.build()` and
  the CLI persists serializable output plus `adapterOptions` in `build.json`; this does not itself
  create or publish platform deployment files.
- `check` is an application-readiness signal, not a browser E2E suite, load test, or security audit.
  Add verification in the layer changed by a feature.

The maintained documentation is intentionally split in two: `docs/user-guide.md` for application
authors and `docs/developer-guide.md` for framework contributors. Keep the root README,
create-package README, demo README, commands, defaults, security limits, and deployment statements
in agreement whenever a user journey changes.
