# Bundler Comparison

Ruvyxa ships its own Rust bundler because the framework needs tight control
over route discovery, SSR/client boundaries, deterministic output, and
app-level production checks. It should still learn from the strongest ideas in
the wider JavaScript toolchain.

## Current Ruvyxa Position

| Area | Ruvyxa today | Strength | Tradeoff |
|------|--------------|----------|----------|
| Runtime | Native Rust CLI and bundler | Fast startup and one binary for users | Smaller ecosystem than JS bundlers |
| Route builds | One hydration bundle per page route plus shared route chunk metadata | Predictable cache keys and simple deploy output | Runtime still loads the route script as the compatibility entry |
| Compilation | AST-backed module facts drive TypeScript stripping and JSX transform | Resolver/compiler share one structured view of imports, exports, JSX, decorators, and TS-only syntax | Still intentionally smaller than a full SWC-compatible parser |
| Resolution | Relative imports, `tsconfig` paths/baseUrl, package `exports` | Covers common app imports | Advanced conditional exports and loader pipelines are limited |
| Optimization | Tree-shaking, minification, BLAKE3 content hashing | Deterministic production bundles | Tree-shaking is conservative compared with Rollup/Parcel |
| Caching | In-process and disk compile cache | Rebuilds avoid repeated transforms | No remote/cache-server story yet |
| Extensibility | Native Rust plugin pipeline plus `ruvyxa.config.ts` JavaScript `resolveId`/`transform` bridge | Users can extend bundling without forking the compiler | JS plugin hooks currently start a Node runner per hook; a persistent worker is future optimization |
| Safety | Server/client boundary diagnostics | Framework-specific correctness checks | General-purpose plugin ecosystem is smaller |

## What Ruvyxa Borrows

| Source | Useful idea | Applied in Ruvyxa |
|--------|-------------|-------------------|
| Vite | Keep dev and build behavior aligned and avoid unnecessary full-app work during local development | Ruvyxa keeps route discovery and SSR semantics shared between `dev`, `build`, and `start`; bundler caches avoid repeated compilation |
| Rollup | Prefer ESM-aware dead-code elimination and deterministic chunks | Ruvyxa keeps tree-shaking enabled by default, exposes `build.treeShaking`, and records shared/dynamic chunk metadata |
| webpack | Rich build stats, long-term caching, and configurable optimization | Build manifests now report module count, output bytes, gzip estimate, cache hits, tree-shaken exports, and compile cache size |
| Turbopack | Rust-based incremental thinking and target-aware framework builds | Ruvyxa keeps bundling native and framework-aware, with route target metadata and persistent cache directories preserved across staged builds |
| esbuild | Fast native transforms with simple defaults | Ruvyxa keeps a zero-extra-bundler path and defaults to minified production output |
| Rspack | Rust performance while respecting familiar bundler expectations | Ruvyxa keeps typed config and manifest fields instead of hidden flags |
| Parcel | Content hashing, automatic production optimization, and useful manifests | Ruvyxa emits BLAKE3-hashed client bundles, shared route chunk metadata, and `client/chunk-manifest.json` via `build.emitChunkManifest` |
| SWC | Rust compiler platform and explicit transform configuration | Ruvyxa exposes `jsxRuntime` and `esTarget`, with AST-backed module facts feeding resolver and compiler passes |

## Recommended Defaults

```ts
import { defineConfig } from "ruvyxa/config"

export default defineConfig({
  build: {
    minify: true,
    sourcemap: false,
    treeShaking: true,
    splitStrategy: "route",
    parallelism: 4,
    jsxRuntime: "classic",
    esTarget: "es2022",
    emitChunkManifest: false,
  },
})
```

Use `treeShaking: false` only when debugging generated output or isolating a
suspected optimizer bug. Keep `emitChunkManifest: true` for CI, deployment
adapters, or performance tooling that needs per-route bundle metadata.

## Roadmap Candidates

These are intentionally scoped to the next high-value gaps after the native
bundler upgrades already landed.

| Candidate | Borrowed from | Why it matters |
|-----------|---------------|----------------|
| Runtime loading for extracted shared chunks | Rollup, webpack, Parcel | Let production HTML preload and execute shared chunks before route scripts |
| Persistent JavaScript plugin worker | Vite, Rollup | Reduce overhead when many modules run config plugin hooks |
| Plugin source-map forwarding | Vite, Rollup | Preserve transformed line mappings across plugin output |
| Full parser compatibility suite | SWC, esbuild, Rollup | Expand the AST-backed facts into broader TypeScript/JSX grammar coverage |
| Dependency pre-bundling | Vite, esbuild | Faster dev startup for dependency-heavy apps |
| Persistent cache invalidation by config dependency graph | webpack, Rspack, Parcel, Turbopack | Safer cache reuse when config, package metadata, or tsconfig changes |

## References

- Vite: <https://vite.dev/guide/why>
- Rollup: <https://rollupjs.org/>
- webpack: <https://webpack.js.org/concepts/>
- Turbopack: <https://nextjs.org/docs/app/api-reference/turbopack>
- esbuild: <https://esbuild.github.io/>
- Rspack: <https://rspack.dev/>
- Parcel: <https://parceljs.org/>
- SWC: <https://swc.rs/>
