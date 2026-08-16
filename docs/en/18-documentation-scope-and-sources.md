# Documentation scope and sources

> **Tutorial goal:** know which claims are implemented, tested, and safe to rely on in your app.
> **Start from:** any chapter whose capability you need to verify. **Checkpoint:** distinguish a
> supported framework contract from a provider-owned implementation detail.

This page maps each durable topic to its responsible source tree and the chapter that documents its
user-relevant behavior. The implementation paths are the source of truth; this is not a claim that
undocumented private implementation is public API.

| Source area            | Responsible implementation                                                                   | Documentation                                                                                                                             |
| ---------------------- | -------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| CLI/config/build       | `crates/ruvyxa_cli/src/*` and `packages/ruvyxa/runtime/{config-renderer,adapter-runner}.mjs` | [CLI](10-cli.md), [Configuration](07-configuration.md), [Deploy and operate](15-deploy-run-and-operate.md)                                |
| Route graph            | `crates/ruvyxa_graph/src/lib.rs`                                                             | [Project structure](03-project-structure.md), [Routing](04-routing-rendering.md)                                                          |
| Bundler/boundaries     | `crates/ruvyxa_bundler/src/*`                                                                | [Architecture](11-architecture.md), [Security](13-security.md)                                                                            |
| Dev server             | `crates/ruvyxa_dev_server/src/*`                                                             | [Architecture](11-architecture.md), [Performance](14-observability-performance.md)                                                        |
| Middleware/diagnostics | `crates/ruvyxa_middleware/src/*`, `crates/ruvyxa_diagnostics/src/lib.rs`                     | [Plugins](08-plugins-middleware.md), [Security](13-security.md)                                                                           |
| Terminal presentation  | `crates/ruvyxa_tui/src/*`                                                                    | [CLI](10-cli.md), [Architecture](11-architecture.md)                                                                                      |
| Core surface           | `packages/@ruvyxa/core/src/{index,types,server,config,plugin}.ts`                            | [Data](05-data-actions-api.md), [Configuration](07-configuration.md), [API reference](17-public-api-reference.md)                         |
| React surface          | `packages/@ruvyxa/react/src/*`                                                               | [UI and assets](06-ui-navigation-metadata-and-assets.md), [Routing](04-routing-rendering.md), [API reference](17-public-api-reference.md) |
| First-party plugins    | `packages/ruvyxa/src/plugins.ts`                                                             | [Plugins](08-plugins-middleware.md), [Observability](14-observability-performance.md)                                                     |
| Runtime/adapters       | `packages/ruvyxa/runtime/*`, `packages/@ruvyxa/adapter-*/src/index.ts`                       | [Architecture](11-architecture.md), [Deploy and operate](15-deploy-run-and-operate.md)                                                    |
| Auth/database/realtime | `packages/@ruvyxa/{auth,database,realtime}/src/*`                                            | [Integrations](09-integrations-auth-data-and-realtime.md), [Security](13-security.md)                                                     |
| Creation/testing       | `packages/create-ruvyxa/src/*`, templates, `packages/@ruvyxa/testing/src/index.ts`, tests    | [First app](02-create-your-first-app.md), [Development and testing](12-development-testing.md)                                            |
| Demo examples          | `examples/demo/app/*`, `examples/demo/plugins/*`, `examples/demo/ruvyxa.config.ts`           | Chapters 03–09                                                                                                                            |

## Verified command inventory

| Scope                 | Commands/scripts                                                                                                                                                                                                |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ruvyxa CLI            | `dev`, `build`, `check`, `start`, `preview`, `routes`, `analyze`, `adds`, `doctor`, `clean`, `trace`, `bench`, `test:parity`, `plugin create`                                                                   |
| Generated application | `dev`, `build`, `start`, `preview`, `typecheck`, `check`, `routes`, `routes:json`, `analyze`, `analyze:html`, `adds`, `doctor`, `clean`, `trace`, `bench`, `test:parity`, `plugin`                              |
| Repository root       | `build`, `check`, `test`, `prepare`, `check:cargo-lock`, `check:oxc-lockstep`, `format`, `format:check`, `format:staged`, `release:validate`, `release:bump`, `pack:smoke`, `test:full-flow`, `publish:dry-run` |

## Explicitly unverified / not implemented as framework features

The codebase has no public generic dependency-injection API, generic queue, scheduler, framework
event bus, database migration service, managed metrics backend, alert manager, backup/recovery
implementation, container/orchestrator manifests, or universal readiness endpoint. This
documentation names those absences instead of inventing APIs or deployment procedures. Platform
behavior outside first-party adapter contracts must be verified in the selected platform's
configuration.

## Documentation verification procedure

After edits, validate that both language trees contain the same filenames and that Markdown links
resolve. Then run the application and repository checks relevant to changed behavior.
Documentation-only work should at minimum verify internal links and paired-tree parity; code changes
require the checks in [Development and testing](12-development-testing.md).

**Previous:** [Public API reference](17-public-api-reference.md) · **Next:**
[Release-readiness playbook](19-release-readiness-playbook.md)
