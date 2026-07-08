# Ruvyxa Build and Bundler Upgrades Design

## Status

Approved direction from user: implement all four upgrades, starting with the plugin pipeline.

## Context

Ruvyxa already has a native Rust build pipeline with route discovery, validation, client bundle emission, source maps, chunk manifests, parallel bundling, and deterministic `.ruvyxa/` output. The public TypeScript API already exposes `RuvyxaPlugin`, `PluginContext`, and `TransformResult`, but the native production build does not yet execute those hooks. The resolver also ignores CSS-style imports, `splitStrategy: "manual"` currently behaves like `single`, and `ruvyxa analyze` reports route validation rather than bundle composition.

The goal is to bring the useful parts of Vite, Webpack, and Rollup into Ruvyxa without replacing Ruvyxa's native Rust bundler.

## Goals

- Make configured `RuvyxaPlugin.resolveId` and `RuvyxaPlugin.transform` hooks execute during production builds.
- Track CSS and asset imports as first-class build artifacts with hashed output and route metadata.
- Add bundle analysis data for route bundles, modules, assets, plugin transforms, and shared code.
- Make `splitStrategy: "manual"` produce real shared/manual chunk metadata and output.
- Preserve existing `.ruvyxa/server`, `.ruvyxa/client`, `.ruvyxa/assets`, `manifest.json`, and `build.json` compatibility by adding fields rather than removing current fields.

## Non-Goals

- Do not replace the Rust bundler with Vite, Webpack, Rollup, esbuild, or another JavaScript bundler.
- Do not implement the full Vite/Rollup plugin API surface in one pass.
- Do not add unsafe plugin execution inside Rust. User-authored JavaScript/TypeScript plugin code should run through the existing Node-based config/runtime path.
- Do not change adapter package contracts except for additive metadata fields.

## Approach

Use a hybrid plugin bridge. The Rust CLI remains responsible for route discovery, build orchestration, native bundling, file output, and diagnostics. The Node config/runtime layer loads `ruvyxa.config.*`, sanitizes serializable build config for Rust, and exposes a small hook bridge for user-authored plugins.

The first implementation should keep the plugin bridge intentionally small:

- `resolveId(id)` can rewrite or resolve an import specifier.
- `transform(code, id, ctx)` can return replacement code and an optional source map.
- `enforce: "pre" | "post"` controls hook ordering around default compilation.
- `PluginContext.environment` is derived from `BundleTarget`: `client` for hydration bundles and `server` for SSR/server-oriented work.

Plugin failures become structured Ruvyxa diagnostics with file/plugin context. A failing plugin should fail the affected build unless the error is explicitly recoverable in a later design.

## Milestone 1: Plugin Pipeline

### Public Contract

`RuvyxaPlugin` remains the public API, but the build now honors it. The config renderer should preserve enough plugin metadata for Rust and provide a Node bridge command that can execute hooks by plugin name/order. Non-serializable plugin functions must not be directly embedded in Rust config; Rust should call the bridge with a JSON request.

### Execution Flow

1. CLI loads config through the existing config renderer.
2. CLI detects configured plugins and initializes a plugin bridge descriptor.
3. Resolver asks the bridge for `resolveId` results before default resolution.
4. Compiler asks the bridge for `transform` results in `pre`, normal, and `post` phases.
5. Bundle stats record plugin transform counts and timing.
6. Cache keys include a plugin fingerprint so changed plugin config does not reuse stale compiled output.

### Diagnostics

Plugin diagnostics should include:

- Plugin name.
- Hook name.
- Module id or specifier.
- Original error message.
- Suggested recovery: fix the plugin or remove it from `ruvyxa.config.ts`.

## Milestone 2: Asset and CSS Bundling

### Public Contract

CSS and asset imports from app modules are no longer silently ignored. The build records them per route and emits copied or generated artifacts into the build output. Existing behavior for projects without CSS/assets stays unchanged.

### Asset Classes

- CSS-like imports: `.css`, `.scss`, `.sass`, `.less`
- Static assets: images, fonts, media, and text-like files imported from project code

The first pass should track and emit these files without implementing a full CSS preprocessor. Unsupported stylesheet dialects should be copied and reported as external stylesheet assets unless a plugin transforms them.

### Output

Add route asset metadata to `.ruvyxa/client/manifest.json` and build-level summaries to `.ruvyxa/build.json`. File names should use BLAKE3 content hashes to preserve immutable caching.

## Milestone 3: Bundle Analyzer

### Public Contract

`ruvyxa analyze` should keep its current validation behavior by default. Add a bundle-aware mode or output field that reports build composition without requiring users to inspect generated files manually.

Recommended CLI surface:

```bash
ruvyxa analyze --bundle --root .
```

The analyzer should report:

- Route path.
- Client bundle file.
- Output bytes and estimated gzip bytes.
- Module count.
- Asset count and asset bytes.
- Shared module candidates.
- Plugin transform count and duration.
- Chunk strategy.

The output should be JSON-first for CI and tooling.

## Milestone 4: Manual and Shared Chunks

### Public Contract

`splitStrategy: "manual"` becomes meaningful. The conservative first pass should support config-driven chunk grouping without changing route rendering semantics.

Proposed build config extension:

```ts
build: {
  splitStrategy: "manual",
  manualChunks: {
    vendor: ["react", "react-dom"],
    ui: ["./components"]
  }
}
```

Manual chunks should emit deterministic chunk files and metadata. Route bundles should reference required chunks through the client manifest so preload/script injection can happen predictably.

## Testing Strategy

- Rust unit tests for resolver hook integration, transform hook integration, asset classification, analyzer metadata, and manual chunk grouping.
- Node tests for config/plugin bridge behavior and error serialization.
- Existing package tests continue to run unchanged.
- At least one integration-style test should build a small app with a plugin, a CSS import, an asset import, analyzer output, and manual chunk config.

## Documentation Strategy

Update docs only where behavior changes:

- `README.md` feature list and config example.
- `docs/deployment.md` for emitted assets/chunks if output changes.
- `docs/debugging.md` for plugin diagnostics and analyzer usage.
- `packages/@ruvyxa/core/README.md` for plugin API behavior.

## Risks and Mitigations

- **Plugin determinism:** Include plugin fingerprints in cache keys and build metadata.
- **Cross-process overhead:** Batch hook calls where practical, but prefer correctness first.
- **Source map drift:** Keep plugin map support optional in the first pass and preserve current identity maps when maps are absent.
- **Manual chunk runtime breakage:** Start with metadata and preload-safe shared chunks; keep route bundle execution compatible.
- **Asset semantics:** Copy unsupported dialects instead of pretending to compile them.

## Self-Review

- No placeholder requirements remain.
- The design keeps the Rust bundler as the core architecture.
- The four requested upgrades are decomposed into ordered milestones.
- Plugin pipeline is first and provides the foundation for the other three upgrades.
- Existing output compatibility is preserved through additive metadata.
