# Project Structure

Ruvyxa is organized as a Rust + TypeScript monorepo. Keep public package paths
stable; prefer internal module refactors over moving root folders.

## Root Folders

| Path | Owner | Purpose |
| --- | --- | --- |
| `crates/` | Rust runtime team | Native CLI, bundler, route graph, diagnostics, middleware, and dev server. |
| `packages/` | TypeScript package team | Published npm packages, runtime Node helpers, adapters, and create app tooling. |
| `tests/` | Cross-package maintainers | Node package tests and adapter behavior tests. |
| `examples/` | Framework QA | End-to-end fixture apps used by full-flow tests and docs examples. |
| `templates/` | Create-app maintainers | Starter templates copied by `create-ruvyxa`. |
| `scripts/` | Release/QA maintainers | Repository-level validation, packaging, and full-flow smoke scripts. |
| `docs/` | Documentation owners | User docs, architecture docs, production guidance, and roadmap notes. |

## Rust Crate Boundaries

| Crate | Responsibility |
| --- | --- |
| `ruvyxa_cli` | CLI commands, config loading, build orchestration, and production output layout. |
| `ruvyxa_bundler` | Native module graph, transform, boundary checks, linking, chunk manifests, and plugin pipeline. |
| `ruvyxa_graph` | File-system route discovery and route validation. |
| `ruvyxa_dev_server` | Dev/prod HTTP server, rendering, HMR, static/client asset serving. |
| `ruvyxa_diagnostics` | Structured diagnostics shared across crates. |
| `ruvyxa_middleware` | Built-in middleware and Wasm middleware plugin runtime. |

`ruvyxa_bundler` is intentionally split by pipeline stage:

- `types.rs` - public contracts and serializable manifest types.
- `context.rs` - shared caches and plugin pipeline state.
- `ast.rs` - lightweight AST facts shared by resolver/compiler.
- `resolver.rs` - specifier resolution and graph walking.
- `compiler.rs` - TypeScript/JSX transform.
- `boundary.rs` - server/client safety checks.
- `linker.rs` - module ordering and import/export rewrites.
- `chunking.rs` - dynamic import and chunk manifest helpers.
- `plugin.rs` - native resolve/transform hook contracts.
- `output.rs` - virtual entries and target wrappers.

## Package Boundaries

| Package | Responsibility |
| --- | --- |
| `packages/ruvyxa` | User-facing CLI package, runtime Node scripts, and public `ruvyxa/*` exports. |
| `packages/@ruvyxa/core` | Shared config, server primitives, adapter contracts, and plugin types. |
| `packages/@ruvyxa/adapter-*` | Deployment adapters. They consume `.ruvyxa/` output, not internal build state. |
| `packages/create-ruvyxa` | Starter scaffolding. It should only depend on templates and package metadata. |
| `packages/@ruvyxa/cli-*` | Platform-specific native binary packages. |

## Change Rules

- Do not move root folders (`crates`, `packages`, `tests`, `examples`, `templates`, `scripts`, `docs`) without a compatibility plan.
- Keep public npm exports stable unless a breaking release explicitly documents migration.
- Put Rust pipeline growth into a focused module, not `lib.rs`.
- Put Node runtime helpers under `packages/ruvyxa/runtime` and include them in `packages/ruvyxa/package.json#files`.
- Add tests at the same layer as the behavior: Rust crate tests for native logic, Node tests for runtime helpers, full-flow for deploy/build compatibility.
