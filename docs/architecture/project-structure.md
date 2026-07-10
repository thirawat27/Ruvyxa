# Project Structure

Ruvyxa is organized as a Rust + TypeScript monorepo. Keep public package paths stable; prefer
internal module refactors over moving root folders.

## Root Folders

| Path         | Responsibility                                               |
| ------------ | ------------------------------------------------------------ |
| `crates/`    | Rust workspace — CLI, bundler, route graph, diagnostics,     |
|              | middleware, and dev server.                                  |
| `packages/`  | Published npm packages — CLI wrapper, runtime Node helpers,  |
|              | typed primitives, adapters, and scaffolding tool.            |
| `tests/`     | Node package tests organized by package under                |
|              | `tests/packages/`.                                           |
| `examples/`  | End-to-end fixture apps used by docs examples and CI smoke   |
|              | tests.                                                       |
| `templates/` | Starter templates copied by `create-ruvyxa` at prepack time. |
| `scripts/`   | Repository-level validation, packaging, and release scripts. |
| `docs/`      | User-facing documentation, architecture notes, and roadmap.  |

## Rust Crate Boundaries

| Crate                | Responsibility                                                                                |
| -------------------- | --------------------------------------------------------------------------------------------- |
| `ruvyxa_cli`         | CLI commands (`dev`, `build`, `check`, `start`, `preview`, `routes`, `analyze`, `doctor`,     |
|                      | `clean`, `trace`, `bench`, `test:parity`), config loading via Node config renderer, build     |
|                      | orchestration, production output layout.                                                      |
| `ruvyxa_bundler`     | Native module graph, TypeScript/JSX compilation, server/client boundary checks,               |
|                      | linker, minifier, source maps, chunk manifests, and plugin pipeline.                          |
| `ruvyxa_graph`       | File-system route discovery, rendering strategy detection, route validation,                  |
|                      | manifest writing.                                                                             |
| `ruvyxa_dev_server`  | Axum HTTP server, WebSocket HMR, radix-tree router, render cache (FIFO with TTL),             |
|                      | Node worker pool, dependency-driven style collection, action/API/client endpoints.            |
| `ruvyxa_diagnostics` | Structured `Diagnostic` struct, `RuvyxaError` enum with `Diagnostic`/`Io`/`Message` variants. |
| `ruvyxa_middleware`  | Built-in Tower middleware (CORS, timing, logging, rate limit, custom headers).                |
|                      | Wasm plugin runtime (optional, requires `wasm-plugins` feature).                              |

`ruvyxa_bundler` is organized into focused stage modules:

- `types.rs` — public contracts (`BundleInput`, `BundleOutput`, `BundleOptions`, `BundleStats`)
- `context.rs` — shared caches, incremental graph, and plugin pipeline state
- `ast.rs` — lightweight AST facts shared by resolver and compiler
- `resolver.rs` — specifier resolution, module graph walking, `tsconfig` paths
- `compiler.rs` — TypeScript/JSX transform, decorators, es-target lowering
- `boundary.rs` — server/client safety checks (`server-only`, `client-only`, private env)
- `linker.rs` — module ordering, import/export rewrites, and bundle assembly
- `chunking.rs` — dynamic `import()` split points and chunk manifest helpers
- `minifier.rs` — whitespace removal, identifier shortening, dead-code elimination
- `plugin.rs` — native resolve/transform hook contracts (`NativeBundlerPlugin`)
- `output.rs` — virtual entries, code emission, and source map generation
- `cache.rs` — in-process and disk compile cache with incremental rebuilds
- `incremental.rs` — persistent incremental module graph cache
- `sourcemap.rs` — source map generation
- `types.rs` — public contracts and serializable manifest types

## Package Boundaries

| Package                      | Responsibility                                                                |
| ---------------------------- | ----------------------------------------------------------------------------- |
| `packages/ruvyxa`            | User-facing npm package. Re-exports from `@ruvyxa/core`. Contains `bin/`,     |
|                              | `runtime/` (Node renderers, worker pool, compiler), and `native-bin/`.        |
| `packages/@ruvyxa/core`      | Shared typed primitives: `defineConfig`, `loader`, `action`, `cache`, `json`, |
|                              | `redirect`, `notFound`, adapter and plugin contracts.                         |
| `packages/@ruvyxa/react`     | React integration: `RuvyxaErrorBoundary`, `useRuvyxaLoader`, `hydrate`.       |
| `packages/@ruvyxa/adapter-*` | Deployment adapters consuming `.ruvyxa/` output.                              |
| `packages/create-ruvyxa`     | Project scaffolding. Copies `templates/minimal/` at prepack time.             |
| `packages/@ruvyxa/cli-*`     | Platform-specific native binary packages (win32-x64, linux-x64/arm64,         |
|                              | darwin-x64/arm64).                                                            |

## Runtime Scripts

The `packages/ruvyxa/runtime/` directory contains Node scripts that the Rust CLI spawns:

| Script                | Purpose                              |
| --------------------- | ------------------------------------ |
| `worker-pool.mjs`     | Persistent Node worker pool (IPC)    |
| `ssr-renderer.mjs`    | Server-side React rendering          |
| `client-renderer.mjs` | Client hydration bundle generation   |
| `action-renderer.mjs` | Server action execution              |
| `api-renderer.mjs`    | API route execution                  |
| `config-renderer.mjs` | Load and validate `ruvyxa.config.ts` |
| `plugin-runner.mjs`   | JavaScript build plugin hooks        |
| `compiler.mjs`        | Runtime module compilation           |
| `ssg-renderer.mjs`    | SSG/ISR/PPR build-time pre-rendering |

## Change Rules

- Do not move root folders without a compatibility plan.
- Keep public npm exports stable unless a breaking release documents migration.
- Put Rust pipeline growth into a focused module, not `lib.rs`.
- Put Node runtime helpers under `packages/ruvyxa/runtime` and list them in `package.json#files`.
- Add tests at the same layer as the behavior: Rust crate tests for native logic, Node tests for
  runtime helpers, full-flow for deploy/build compatibility.
