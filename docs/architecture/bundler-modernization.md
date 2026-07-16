# Bundler modernization boundary

## Decision

Ruvyxa owns framework-specific module resolution, plugins, route entry generation, server/client
boundary diagnostics, linking, chunk manifests, and source-map composition. Oxc 0.136.0 owns
TypeScript/JSX parsing and lowering plus final JavaScript parsing, semantic compression, name
mangling, and code generation.

This is intentionally not a Rolldown dependency. Rolldown is a bundler workspace with tightly
coupled scanner, module graph, linker, chunk graph, and render stages; borrowing its staged
architecture is lower risk than importing those internals into Ruvyxa's framework runtime.

## Current production pipeline

```text
virtual route entry
  -> Ruvyxa resolver/cache/plugins
  -> Ruvyxa MDX compiler -> Oxc TS/JSX parser/transformer/codegen
  -> Ruvyxa boundary checks and dynamic chunk plan
  -> static entry linker + lazily loaded dynamic chunk linkers
  -> Ruvyxa explicit export pruning
  -> Oxc parser -> semantic minifier/mangler -> code generator
  -> Ruvyxa output wrappers, chunks, manifests, source maps
```

`build.treeShaking` keeps its public meaning. When enabled, Ruvyxa performs its existing
linker-aware pruning and Oxc uses its normal compression profile. When it is disabled, Oxc uses
`CompressOptions::safest()` so otherwise-unused bindings are not removed. A parse diagnostic now
aborts the build instead of risking malformed minified JavaScript.

The old token compressor is test-only during this transition; it is no longer on a production bundle
path. `minify_parallel` remains as an API-compatible entry point, but delegates to one whole-program
Oxc pass because semantic mangling cannot safely be performed independently per linker segment.

## Evidence-based adoption map

| Area                     | Current Ruvyxa owner                       | Adopt now                                                  | Deferred boundary                                                     |
| ------------------------ | ------------------------------------------ | ---------------------------------------------------------- | --------------------------------------------------------------------- |
| JavaScript minification  | `minifier.rs`                              | Oxc parser, minifier, codegen                              | Oxc source-map integration after mapping-quality fixtures             |
| Module resolution        | `resolver.rs`                              | Keep current package exports, tsconfig paths, plugin hooks | Evaluate `oxc_resolver` only behind adapter conformance tests         |
| TS/JSX transform         | `compiler.rs`                              | Oxc parser, semantic model, transformer, codegen           | Oxc source-map integration after mapping-quality fixtures             |
| Scan/link/chunk render   | `ast.rs`, `linker.rs`, `chunking.rs`       | Keep Ruvyxa output contracts                               | Borrow Rolldown's explicit scan -> link -> render metadata boundaries |
| Caching/incremental work | `cache.rs`, `context.rs`, `incremental.rs` | Keep current shared context and cache keys                 | Add per-stage invalidation metrics before changing algorithms         |

## Next safe stages

1. Add fixture-based semantic and source-map tests for ESM, CommonJS, dynamic imports, decorators,
   JSX output, and malformed linked input.
2. Make scan, link, and render metadata explicit types (rather than replacing the whole linker),
   following Rolldown's staged ownership pattern.
3. Wire the persisted graph manifest into the production bundle context only after it has per-stage
   invalidation metrics and lifecycle coverage.
4. Evaluate `oxc_resolver` behind an internal adapter only after resolver fixtures prove Ruvyxa
   plugin and framework semantics are preserved.

## Constraints and risks

- Oxc is pinned exactly to `0.136.0`; upgrading it is a deliberate compatibility review, not a
  floating dependency update.
- Runtime renderers invoke the same native bundler through `runtime/ruvyxa-compiler.mjs`; the old
  JavaScript `runtime/compiler.mjs` transformer is not shipped or used.
- The current Ruvyxa source-map builder remains in place. Oxc reprints the final code, so mapping
  fidelity needs dedicated fixtures before replacing map handling.
- Rolldown/SWC are reference implementations, not runtime dependencies. Directly importing either
  bundler would bypass Ruvyxa's public configuration and plugin contracts without a proven migration
  path.
- Client dynamic chunks now follow an explicit scan → plan → link flow. The entry follows static
  edges only, each dynamic root receives a deterministic graph-versioned filename, and runtime
  `import()` resolves the chunk's original module namespace. The graph-level fingerprint
  deliberately invalidates dependent chunk names together, prioritizing cache correctness over
  premature fine-grained hashing.
