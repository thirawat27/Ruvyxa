# Bundler Comparison

Ruvyxa ships its own Rust bundler because the framework needs tight control
over route discovery, SSR/client boundaries, deterministic output, and
app-level production checks. It should still learn from the strongest ideas in
the wider JavaScript toolchain.

## Current Ruvyxa Position

| Area | Ruvyxa today | Strength | Tradeoff |
|------|--------------|----------|----------|
| Runtime | Native Rust CLI and bundler | Fast startup and one binary for users | Smaller ecosystem than JS bundlers |
| Route builds | One hydration bundle per page route | Predictable cache keys and simple deploy output | Shared chunk extraction is not yet implemented |
| Compilation | TypeScript stripping and JSX transform | No external bundler dependency | Text-based transform is less complete than AST compilers |
| Resolution | Relative imports, `tsconfig` paths/baseUrl, package `exports` | Covers common app imports | Advanced conditional exports and loader pipelines are limited |
| Optimization | Tree-shaking, minification, BLAKE3 content hashing | Deterministic production bundles | Tree-shaking is conservative compared with Rollup/Parcel |
| Caching | In-process and disk compile cache | Rebuilds avoid repeated transforms | No remote/cache-server story yet |
| Safety | Server/client boundary diagnostics | Framework-specific correctness checks | General-purpose plugin ecosystem is smaller |

## What Ruvyxa Borrows

| Source | Useful idea | Applied in Ruvyxa |
|--------|-------------|-------------------|
| Vite | Keep dev and build behavior aligned and avoid unnecessary full-app work during local development | Ruvyxa keeps route discovery and SSR semantics shared between `dev`, `build`, and `start`; bundler caches avoid repeated compilation |
| Rollup | Prefer ESM-aware dead-code elimination and deterministic chunks | Ruvyxa keeps tree-shaking enabled by default and now exposes `build.treeShaking` for explicit control |
| webpack | Rich build stats, long-term caching, and configurable optimization | Build manifests now report module count, output bytes, gzip estimate, cache hits, tree-shaken exports, and compile cache size |
| Turbopack | Rust-based incremental thinking and target-aware framework builds | Ruvyxa keeps bundling native and framework-aware, with route target metadata and persistent cache directories preserved across staged builds |
| esbuild | Fast native transforms with simple defaults | Ruvyxa keeps a zero-extra-bundler path and defaults to minified production output |
| Rspack | Rust performance while respecting familiar bundler expectations | Ruvyxa keeps typed config and manifest fields instead of hidden flags |
| Parcel | Content hashing, automatic production optimization, and useful manifests | Ruvyxa emits BLAKE3-hashed client bundles and can emit `client/chunk-manifest.json` via `build.emitChunkManifest` |
| SWC | Rust compiler platform and explicit transform configuration | Ruvyxa exposes `jsxRuntime` and `esTarget` as build config, while keeping the heavier AST/compiler rewrite as future work |

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

These are intentionally not marked as complete until implemented and tested.

| Candidate | Borrowed from | Why it matters |
|-----------|---------------|----------------|
| Shared route chunks | Rollup, webpack, Parcel | Avoid duplicated shared components/layouts across route bundles |
| Dynamic `import()` split points | Rollup, webpack, Parcel | Load expensive modules on demand |
| AST-backed parser/transform | SWC, esbuild, Rollup | Better correctness for complex TypeScript/JSX syntax |
| Bundler plugin hooks wired into the native pipeline | Vite, Rollup, webpack, Rspack | Let users transform files without forking Ruvyxa |
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
