# Ruvyxa System Overview

**Design philosophy**: Rust handles everything before the render step (route discovery, bundling,
resolution, minification, serving). Node.js or Bun handles rendering (React SSR, API execution,
config evaluation). The hybrid architecture gives Rust's speed + type safety with JavaScript
ecosystem access.

## High-Level Architecture

```
┌──────────────────────────────────────────────────────────┐
│                     ruvyxa_cli                           │
│  (clap command dispatch, config loading, build orchestr) │
├─────────┬──────────┬──────────────┬───────────┬─────────┤
│ruvyxa_   │ruvyxa_   │ruvyxa_dev_   │ruvyxa_    │ruvyxa_  │
│graph     │bundler   │server        │middleware │diag-    │
│(route    │(TS/JSX   │(Axum + HMR + │(Tower     │nostics  │
│disc+val) │comp+link)│router+cache) │+TS host)  │(RUV####)│
└─────────┴──────────┴──────────────┴───────────┴─────────┘
       │         │              │           │
       └─────────┴──────────────┴───────────┘
                       │
             ┌─────────▼─────────┐
             │ Node/Bun Workers  │
             │  (SSR, SSG, API,   │
             │   Action, Config)  │
             └───────────────────┘
```

## Crate Dependency Graph

```
ruvyxa_diagnostics     (foundation: serde + thiserror only)
    ↑
    ├── ruvyxa_graph   (depends: diagnostics)
    ├── ruvyxa_bundler (depends: diagnostics, oxc, grass, dashmap, rayon, memmap2, blake3)
    ├── ruvyxa_middleware (depends: diagnostics, axum, tower, Node/Bun bridge)
    └── ruvyxa_dev_server (depends: diagnostics, bundler, graph, middleware, axum, notify, tokio)
         │
         └── ruvyxa_cli (depends: ALL crates, binary entry via clap)
```

## Key Design Decisions

| Decision                               | Rationale                                                                                                                                                                                                              |
| -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Rust core, Node/Bun renderers**      | Rust: fast startup, compile-time safety, single binary. When runtime is omitted, Node is preferred and Bun is selected automatically if Node is unavailable. Workers eliminate per-request process spawn (~100-500ms). |
| **Oxc for TS/JSX** (not Babel/SWC/TSC) | 10-100x faster. Single binary. No Node dep for bundling.                                                                                                                                                               |
| **Persistent JavaScript worker pool**  | Node or Bun pool: 2-8 workers (default CPU count). NDJSON over stdin/stdout.                                                                                                                                           |
| **Radix trie router**                  | O(path_depth) vs O(n) linear scan. Recompiled on manifest change.                                                                                                                                                      |
| **Content-hashed assets**              | Blake3 fingerprints → immutable caching (max-age=31536000).                                                                                                                                                            |
| **Staging + atomic commit**            | Build writes to staging dir → atomic rename. No corrupt output.                                                                                                                                                        |
| **Deterministic CSS scoping**          | fnv1a_64(project_relative_path + class_name) — reproducible builds.                                                                                                                                                    |
| **Strict config**                      | `deny_unknown_fields` — typos fail fast instead of silent defaults.                                                                                                                                                    |

## Rendering Strategies

| Strategy | Trigger                               | Behavior                                           |
| -------- | ------------------------------------- | -------------------------------------------------- |
| **CSR**  | `"use client"` directive              | Minimal HTML shell, client-side hydrate            |
| **PPR**  | `export const ppr = true`             | Static shell + streaming dynamic slots             |
| **ISR**  | `export const revalidate = <n>`       | Cache + stale-while-revalidate, background refresh |
| **SSG**  | `getStaticParams` or static candidate | Pre-rendered at build, cached indefinitely         |
| **SSR**  | Default                               | Server render per request, cached briefly          |

## Server/Client Boundary

Two enforcement levels: graph-level (source scan in `ruvyxa_graph::validate_app`) and bundle-level
(compiled output in `ruvyxa_bundler::boundary`).

| Rule                                              | Code    | Severity   |
| ------------------------------------------------- | ------- | ---------- |
| `"server-only"` in client bundle                  | RUV1007 | Error      |
| Private `process.env` in client                   | RUV1008 | Error      |
| `"client-only"` in SSR bundle                     | RUV1009 | Warning    |
| `server/` dir in client graph                     | RUV1010 | Error      |
| Only `RUVYXA_PUBLIC_*` env vars allowed in client | —       | Convention |

## Source File Conventions

| Pattern                         | Type               | URL                         |
| ------------------------------- | ------------------ | --------------------------- |
| `app/page.tsx`                  | Page               | `/`                         |
| `app/about/page.tsx`            | Page               | `/about`                    |
| `app/blog/[slug]/page.tsx`      | Dynamic            | `/blog/:slug`               |
| `app/docs/[...rest]/page.tsx`   | Catch-all          | `/docs/*`                   |
| `app/shop/[[...cats]]/page.tsx` | Optional catch-all | `/shop` or `/shop/a/b`      |
| `app/api/route.ts`              | API                | `/api`                      |
| `app/layout.tsx`                | Layout             | wraps children              |
| `app/(group)/page.tsx`          | Route group        | `/` (parenthesized ignored) |
| `app/@modal/page.tsx`           | Parallel slot      | _Ignored_                   |
| `app/_private/page.tsx`         | Private dir        | _Ignored_                   |
| `app/action.ts`                 | Server action      | sibling to page             |
| `app/server.ts`                 | Server module      | sibling to page             |
| `app/client.tsx`                | Client module      | sibling to page             |
| `app/page.md` / `.mdx`          | Content page       | `/`                         |

## NPM Packages

| Package             | Role                                                               |
| ------------------- | ------------------------------------------------------------------ |
| `ruvyxa`            | CLI launcher + runtime bridge                                      |
| `create-ruvyxa`     | Project scaffold                                                   |
| `@ruvyxa/core`      | Core runtime utilities                                             |
| `@ruvyxa/react`     | React integration                                                  |
| `@ruvyxa/adapter-*` | Platform adapters (bun, cloudflare, netlify, node, static, vercel) |
| `@ruvyxa/cli-*`     | Native binaries per platform                                       |

## Detailed Architecture Docs

- [Route Discovery & Validation](graph.md) — `ruvyxa_graph` internals
- [Compilation Pipeline](bundler.md) — `ruvyxa_bundler` resolver, compiler, linker, minifier
- [Dev Server](dev-server.md) — `ruvyxa_dev_server` Axum server, router, render cache, HMR, styles
- [CLI & Build Pipeline](cli.md) — `ruvyxa_cli` commands, config, build orchestration
- [Middleware](middleware.md) — built-in Tower stack and plugin bridge
- [Plugins](plugins.md) — unified setup registry and lifecycle
- [Worker Pool](worker-pool.md) — Node/Bun worker pool protocol, streaming, failure recovery
- [Diagnostic Codes](diagnostics.md) — RUV#### error catalog
- [Concurrency Model](concurrency.md) — locks, parallelism, performance characteristics
- [Wire Protocols](protocols.md) — NDJSON, WebSocket HMR, and Fetch payloads
- [Security Model](security.md) — env isolation, rate limiting, and plugin boundaries
