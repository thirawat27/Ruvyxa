# Changelog

## v1.0.9 (2026-07-10)

### Client Bundling and Boundaries

- Bundled browser React and React DOM dependencies, including CommonJS package dependencies, so
  client hydration no longer leaves unresolved bare `react` module specifiers
- Preserved valid third-party JavaScript, including regular-expression literals, when the native
  text minifier cannot safely parse the dependency source
- Made server/client boundary diagnostics syntax-aware so ordinary content containing `server-only`
  is not treated as a module marker
- Ignored type-only imports during runtime dependency resolution

### Build Reliability

- Capped default and configured static pre-render concurrency at two workers to prevent memory
  exhaustion on content-heavy sites
- Added Windows rename retries for transient file locks while committing build output
- Fixed file-watcher cache invalidation on threads without a Tokio runtime

### Starter and Documentation

- Added the CSS module declaration required by the minimal TypeScript starter
- Synchronized all npm packages, Rust crates, lockfiles, and template dependencies to 1.0.9
- Added regression coverage for client dependency bundling, boundary markers, Windows-safe build
  commits, pre-render limits, watcher invalidation, and starter generation

## v1.0.8 (2026-07-10)

### Performance and Build

- Parallelized build-time prerendering for CSR, SSG, ISR, and PPR routes while preserving manifest
  order
- Reused the configured build parallelism for prerender work to reduce production build time
- Kept client bundling parallelism capped to available work to avoid oversubscription
- Reduced the demo production build benchmark from about 2.3s to about 1.1s

### Styling

- Collected CSS through the application dependency graph, including styles imported from outside
  `app/` and nested local CSS `@import` dependencies
- Added project-relative `css.entries` for unimported global style files and directories
- Preserved runtime CSS-in-JS style objects and `<style>` elements, with external style HMR and
  production-copy coverage
- Added actionable diagnostics for unresolved styles, unsafe entries, and preprocessors without a
  transform plugin

## v1.0.7 (2026-07-10)

### Performance and Bundling

- Reused one persistent Node worker for JavaScript config plugin hooks during each build
- Forwarded plugin transform Source Map v3 data into generated client bundle maps
- Added route-scoped shared chunk metadata and `modulepreload` hints to runtime and pre-rendered
  HTML
- Ensured pre-rendered SSG, ISR, PPR, and CSR output loads hashed hydration assets from the client
  manifest
- Added fixture-driven advanced TypeScript/JSX parser coverage and fixed multiline enums,
  `implements`, and namespaced JSX tags
- Invalidated native compile artifacts when imported config/plugin dependencies change
- Added shared build-cache directories via `cache.buildDir` or `RUVYXA_BUILD_CACHE_DIR`
- Pre-bundled dev route dependencies in background across every persistent Node worker
- Added consistent client directory and chunk-manifest references to every deployment adapter

## v1.0.6 (2026-07-09)

### Highlights

- SSG, ISR, and PPR pre-rendering support added to the rendering pipeline
- New runtime SSG renderer for server-side page pre-rendering at build time
- CSR minimal shell HTML generation for client-side rendered pages
- Revalidation metadata tracking for ISR routes
- Dev server and build output updated with prerendered routes manifest
- Demo examples demonstrating SSG, ISR, PPR, and CSR rendering strategies
- Codebase-wide formatting standardization with Prettier configuration
- `render_api` refactored to use structured request object for improved maintainability
- Documentation overhaul across all guides (rendering strategies, cache, security, middleware)
- pnpm requirement upgraded from 10+ to 11+

### Rust Crates

- **ruvyxa_cli**:
  - SSG/ISR/PPR pre-rendering at build time with dynamic route support
  - `getStaticParams` resolution for dynamic routes during build
  - Build output includes prerendered routes manifest and prerender stats
  - Code formatting improvements
- **ruvyxa_dev_server**:
  - Prerender directory support in dev server and production configs
  - `render_api` refactored to accept `RenderApiRequest` struct instead of multiple params
  - Reduced parameter passing complexity and improved type safety
  - Worker pool and router enhancements
- **ruvyxa_graph**:
  - Route manifest generation updates for prerendering
  - Enhanced route discovery

### npm Packages

- All packages updated with version bumps
- **@ruvyxa/core**: Added `RenderStrategy` enum and rendering configuration to types
- **ruvyxa/runtime**:
  - New `ssg-renderer.mjs` for server-side page rendering
  - `worker-pool.mjs` modernized with improved concurrent request handling
  - All runtime modules formatted to new Prettier standards
- All adapter packages updated with `tsconfig.check.json` and formatting
- All CLI binary packages updated

### Examples

- **demo**:
  - New SSG blog with `[slug]` dynamic routes (`app/ssg-blog/`)
  - New ISR page with revalidation (`app/isr-page/`)
  - New PPR page with partial pre-rendering (`app/ppr-page/`)
  - New CSR page with client-side rendering (`app/static-page/`)
  - Static page example
  - Updated layout, routing, and configurations

### Documentation

- Updated README with rendering strategies, pnpm 11+ requirement, expanded crate descriptions
- Updated CONTRIBUTING with correct Rust verification flags and adapter guidelines
- Enhanced `docs/architecture/project-structure.md` with crate capabilities and features
- Updated `docs/routing.md`, `docs/data.md`, `docs/actions.md` with rendering strategy details
- Revamped `docs/debugging.md`, `docs/deployment.md`, `docs/performance.md`
- Expanded `docs/production-readiness.md` with cache configuration and security
- Improved `docs/publishing.md` and `docs/parity.md`
- `docs/testing.md` updated with API renderer test documentation

### Infrastructure

- Added `.prettierrc` and `.prettierignore` for consistent code formatting
- pnpm requirement changed from `^10.32.1` to `^11.7.0`
- Package metadata validation uses dynamic license from root `package.json`
- All `package.json` files updated with version and dependency sync
- TypeScript config check files added to adapter packages
- GitHub Actions workflows updated for formatting consistency

### Testing

- New `api-renderer.test.mjs` test suite for API rendering
- Updated `compiler.test.mjs`, `client-renderer.test.mjs`, `action-renderer.test.mjs`
- Updated adapter tests for all 6 deployment targets
- Updated core config and server tests
- `test-full-flow.ps1` updated with expanded coverage

## v1.0.5 (2026-07-09)

### Highlights

- Full native bundler pipeline with AST parsing, plugin system, chunking, and tree-shaking
- New `demo` example app replacing `basic-app`
- Comprehensive end-to-end test script (`test-full-flow.ps1`)
- Project structure and engineering backlog documentation
- README logo switched to local asset for reliability

### Rust Crates

- **ruvyxa_bundler**: Major feature expansion
  - AST module (`ast.rs`) for structured parsing of imports, exports, JSX, decorators, TypeScript
  - Plugin system (`plugin.rs`) for custom transformations in the bundler pipeline
  - Chunking module (`chunking.rs`) for dynamic import split points and output chunk generation
  - Context module (`context.rs`) for shared bundler execution state across parallel workers
  - Types module (`types.rs`) with core bundler type definitions
  - Tree-shaking as separate step before minification (`treeShaking` build option)
  - Cache hit tracking via `cache_hit` field on `CompiledModule`
  - Plugin-runner module for runtime plugin execution
  - Resolver enhancements: CommonJS `require()`, dynamic `import()`, improved caching
  - Source map improvements
  - Parallel cache reuse across bundle jobs
- **ruvyxa_cli**: Integrated new bundler components, expanded CLI commands
- **ruvyxa_dev_server**: Render cache improvements, HMR tracker updates, worker pool enhancements
- **ruvyxa_middleware**: WASM plugin system improvements
- **ruvyxa_graph**: Graph module updates
- **ruvyxa_diagnostics**: Diagnostic enhancements

### npm Packages

- All packages bumped to v1.0.5
- **@ruvyxa/core**: Added `utils.ts`, `PluginContext` and `TransformResult` exports, config updates
- **@ruvyxa/react**: Package updates
- **ruvyxa/runtime**: Added `plugin-runner.mjs`, `config-renderer.mjs` enhancements, `compiler.mjs`
  updates
- **adapters**: All 6 adapter packages updated with platform info and README improvements
- **CLI platform binaries**: All 5 platform packages updated
- **create-ruvyxa**: Updates

### Examples

- Replaced `basic-app` with comprehensive `demo` example
  - Multiple route patterns (static, dynamic `[slug]`, catchall `[...slug]`)
  - Todos with server actions and in-memory DB
  - Blog routes, environment variables page
  - Full TypeScript + Tailwind CSS setup
  - AGENTS.md and CLAUDE.md for AI-assisted development

### Documentation

- Added `docs/architecture/project-structure.md`
- Added `docs/roadmap/engineering-backlog.md`
- Updated debugging, deployment, parity, performance, production-readiness docs
- Bundler comparison documentation (`bundler-comparison.md`)
- README refreshed with new logo, features, and bundler comparison link

### Testing

- New `scripts/test-full-flow.ps1` end-to-end test script
- Expanded compiler test coverage
- Integration tests for all adapter packages
- AST parsing tests across import forms

### Infrastructure

- Removed obsolete `basic-app` example
- Cleaned up old design spec documents
- CLAUDE.md and AGENTS.md updated

---

## v1.0.4 (2026-07-01)

### Highlights

- `ruvyxa check` command for pre-deploy verification
- Type checking, build validation, dev/prod parity, and page smoke rendering
- Plugin contract documentation and type exports
- Simplified template structure (removed `.env.example`, consolidated CSS)

### Rust Crates

- **ruvyxa_cli**: Added `CheckArgs` and `ruvyxa check` command
  - Runs type checking, build validation, dev/prod parity, smoke rendering
- **ruvyxa_dev_server**: Exported `render_request` for programmatic rendering in `check` command
- **ruvyxa_graph**: Route discovery and manifest generation updates
- All crates bumped to v1.0.4

### npm Packages

- All packages bumped to v1.0.4
- **@ruvyxa/core**:
  - Exported `PluginContext` and `TransformResult` types
  - Added comprehensive Plugin Contract section to README
  - New `types.ts` for shared type definitions
  - `config.ts` improvements
- **ruvyxa**: Type exports synced, removed unused CSS module declarations
- **create-ruvyxa**: Package manager detection utility
- All adapters and CLI binaries updated

### Examples & Templates

- **basic-app**: Removed `.env.example`, simplified
- **minimal template**:
  - Removed todos example and about page
  - Consolidated `global.css` → `globals.css`
  - Removed legacy API examples
  - Updated TypeScript configuration

### Documentation

- Plugin Contract documentation with example implementation
- `check` positioned as primary verification command before `build`
- SKILL.md and app-guide.md updated to recommend `pnpm check`
- `test:parity` command description clarified
- Updated routing, data, debugging, deployment, getting-started docs
- Removed outdated `Plan.md`

### Testing

- `core/config.test.ts` test suite for configuration validation
- Expanded compiler test coverage
- create-ruvyxa test suite enhancements
- Verification guidance: use `check` for routine changes, `parity`/`analyze` for drill-down

---

## v1.0.3 (2026-07-08)

### Highlights

- Incremental caching and HMR tracking for bundler and dev server
- Centralized test directory structure (`tests/`)
- First-class adapter packages with dedicated tests
- React utilities: error boundary, hydration, `useLoader` hook

### Rust Crates

- **ruvyxa_bundler**:
  - Incremental graph cache with `incremental.rs` (file fingerprinting via blake3)
  - `CachedModuleEntry` and `GraphManifest` for persistent dependency storage
  - Fast-reject optimization (mtime/size check before hashing)
  - Cache hit tracking improvements
  - Compiler enhancements: TypeScript annotation stripping fixes, JSX child detection
  - All crates bumped to v1.0.3
- **ruvyxa_cli**: Path validation in `ProjectConfig`, command additions
- **ruvyxa_dev_server**:
  - HMR tracker module (`hmr_tracker.rs`)
  - Module invalidation tracking and dependency relationship mapping
  - Render cache expiry and validation improvements
  - Worker pool task scheduling and error handling enhancements
- **ruvyxa_middleware**: Updates
- **ruvyxa_graph**: Updates
- **ruvyxa_diagnostics**: Updates

### npm Packages

- All packages bumped to v1.0.3
- **@ruvyxa/react**:
  - Error boundary component
  - Hydration utilities for React client initialization
  - `useLoader` hook for data loading patterns
- **@ruvyxa/core**: Server implementation improvements
- **ruvyxa/runtime**:
  - Full runtime compiler (`compiler.mjs`)
  - SSR renderer, action renderer, client renderer, API renderer improvements
  - Worker pool enhancements
- **create-ruvyxa**: Package manager detection utility
- All adapters and CLI binaries updated

### Testing

- Reorganized tests from package-local to centralized `tests/` directory
- Dedicated test files for each adapter: Bun, Cloudflare, Netlify, Node, Static, Vercel
- `tests/packages/core/server.test.ts`
- `tests/packages/ruvyxa/compiler.test.mjs`
- `tests/packages/ruvyxa/action-renderer.test.mjs`
- `tests/packages/ruvyxa/client-renderer.test.mjs`
- vitest configuration
- Adapter test coverage for all deployment targets

### Documentation

- `docs/testing.md` with testing layout guidance
- Updated debugging, performance, production-readiness, publishing docs
- README logo enlarged
- CLI platform binary READMEs

### Infrastructure

- Dashmap, memmap2, parking_lot dependencies for concurrent caching
- Clap bumped to 4.6
- Dependency updates

---

## v1.0.2 (2026-06-18)

### Highlights

- First release of `ruvyxa_bundler` — native Rust bundler
- `ruvyxa_middleware` crate with WASM plugin support
- Compression, caching, and worker pool in dev server
- Upgraded toolchain: Node.js 22, Rust 1.96, pnpm 10

### Rust Crates

- **ruvyxa_bundler** (new crate):
  - Native Rust TypeScript/JSX compiler pipeline
  - Boundary checker for server/client module isolation
  - Caching layer with blake3 hashing for incremental builds
  - AST transformation and code generation
  - Linker for module resolution and bundle generation
  - Minifier for production bundle optimization
  - Source map generation
  - Module path resolution and dependency tracking
  - Output formatter with bundle metadata
- **ruvyxa_middleware** (new crate):
  - Builtin middleware implementations
  - Config-driven middleware stack
  - WASM plugin system for custom middleware
- **ruvyxa_dev_server**:
  - HTTP compression (gzip + brotli) via tower-http
  - Render cache system with blake3 hashing
  - Radix router for efficient route matching
  - Node.js worker pool for concurrent request handling
- **ruvyxa_cli**: Integrated bundler, middleware, expanded CLI
- All crates bumped to v1.0.2

### npm Packages

- All packages bumped to v1.0.2
- **@ruvyxa/core**: Server refactoring, config improvements, type safety
- **@ruvyxa/react**: Package initialized with exports
- **ruvyxa/runtime**:
  - `config-renderer.mjs` for runtime configuration
  - `worker-pool.mjs` for Node.js worker management
- **CLI platform binaries**: All 5 platform packages with native binaries
- **create-ruvyxa**: Enhanced CLI with platform detection
- **@ruvyxa/adapter-***: All 6 adapters with README and teamplate support

### CI/CD

- GitHub Actions CI and release workflows
- Cargo caching (Swatinem/rust-cache@v2)
- Smart publishing via `publish-if-new.mjs`
- Version resolution and tag management
- Cross-platform npm pack smoke testing
- Exponential backoff retry for cargo fetch and pnpm install
- `bump-version.mjs` script
- `--locked` flag for reproducible builds

### Toolchain

- Node.js upgraded from 20 to 22
- Rust toolchain upgraded from 1.80+ to 1.96+
- pnpm upgraded to v10
- TypeScript 6.0
- pnpm action v5

### Templates

- **minimal template**: Complete Ruvyxa starter with routing, API, todos, Tailwind CSS
- AGENTS.md and CLAUDE.md for AI-assisted development
- `config-renderer.mjs` integration
- TypeScript configuration with path aliases

### Documentation

- Comprehensive README with hero section, features, CLI commands
- Full CONTRIBUTING.md with development setup, conventions, workflows
- Getting started, routing, data, actions, debugging, deployment, performance docs
- Production readiness and publishing guides
- Parity testing documentation
- Security documentation in SECURITY.md
- Skill framework documentation (SKILL.md, app-guide.md)

---

## v1.0.1 (2026-06-17)

### Highlights

- Cross-platform npm pack smoke testing
- HMR WebSocket payload optimization
- `create-ruvyxa` CLI UX improvements
- CI/CD reliability and version management

### Rust Crates

- All crates bumped to v1.0.1 (ruvyxa_cli, ruvyxa_dev_server, ruvyxa_diagnostics, ruvyxa_graph)
- **ruvyxa_dev_server**: Simplified HMR WebSocket handling using pre-serialized messages from
  channel

### npm Packages

- All packages bumped to v1.0.1
- **create-ruvyxa**:
  - Try-catch error handling with graceful error display
  - Formatted next steps after app creation (cd, pnpm install, pnpm dev)
  - Target directory validation (exists + empty check)
  - Clear error messages for non-empty directories
- **ruvyxa**: Release packaging scripts
- **@ruvyxa/adapter-***: All adapters updated
- **CLI platform binaries**: All platform packages updated

### CI/CD

- `resolve-version` job for version extraction and git tag validation
- Auto tag creation on `workflow_dispatch`
- Git tag existence check via `git ls-remote`
- Release summary in GitHub step summary
- `release:bump` script for syncing workspace versions
- Cross-platform npm pack smoke detection (dynamic tarball resolution)
- macOS 13 build target removal
- HMR error handling simplification

### Documentation

- `docs/publishing.md` with npm publishing guidelines
- Updated deployment docs
- Version reference updates across docs
- README version badges

### Infrastructure

- `scripts/validate-package-metadata.mjs`
- `scripts/pack-smoke.mjs` with dynamic tarball detection
- Platform-specific native binary preparation scripts

---

## v1.0.0 (2026-06-17)

### Highlights

- Initial production release of Ruvyxa
- Native Rust CLI with full-stack React framework
- Built-in development server and production server
- Route discovery and manifest generation
- Diagnostic system with error codes

### Rust Crates

- **ruvyxa_cli**:
  - CLI entry point with commands: `dev`, `build`, `start`, `preview`, `routes`, `analyze`,
    `doctor`, `clean`, `trace`, `bench`, `test:parity`
  - Project configuration and build pipeline
  - PID file management
- **ruvyxa_dev_server**:
  - Development server with HMR and WebSocket support
  - Production server with static file serving
  - Node.js runtime integration
- **ruvyxa_diagnostics**:
  - Diagnostic type system: warnings, errors, hints, tips
  - Structured diagnostic output
- **ruvyxa_graph**:
  - Route discovery from file system
  - Route manifest generation
  - Layout and page tree construction

### npm Packages

- **ruvyxa**: Main CLI wrapper package with native binary resolution
- **@ruvyxa/core**: Core framework with server, config, types, request/response handling
- **@ruvyxa/react**: React integration with SSR support
- **create-ruvyxa**: Project scaffolding CLI
- **@ruvyxa/adapter-bun**: Bun deployment adapter
- **@ruvyxa/adapter-cloudflare**: Cloudflare Workers deployment adapter
- **@ruvyxa/adapter-netlify**: Netlify deployment adapter
- **@ruvyxa/adapter-node**: Node.js deployment adapter
- **@ruvyxa/adapter-static**: Static site generation adapter
- **@ruvyxa/adapter-vercel**: Vercel deployment adapter
- **@ruvyxa/cli-darwin-arm64**: macOS ARM64 native binary
- **@ruvyxa/cli-darwin-x64**: macOS x64 native binary
- **@ruvyxa/cli-linux-arm64**: Linux ARM64 native binary
- **@ruvyxa/cli-linux-x64**: Linux x64 native binary
- **@ruvyxa/cli-win32-x64**: Windows x64 native binary

### Runtime

- `ssr-renderer.mjs` — Server-side rendering
- `client-renderer.mjs` — Client hydration and rendering
- `action-renderer.mjs` — Server action handling
- `api-renderer.mjs` — API route handling
- `config-renderer.mjs` — Runtime configuration
- `worker-pool.mjs` — Worker pool management

### Examples

- **basic-app**: Starter application with:
  - Layout and page routing
  - About page
  - Blog with dynamic `[slug]` routes
  - Todos with server actions
  - Tailwind CSS styling
  - TypeScript configuration

### Templates

- **minimal template**: Minimal Ruvyxa starter
  - Single page with layout
  - Basic route structure
  - TypeScript + Tailwind CSS

### Documentation

- README.md with feature overview, getting started, examples
- CLI command documentation
- Architecture overview

### Infrastructure

- Rust workspace with 5 crates
- pnpm monorepo with 18 packages
- GitHub repository setup
- Prebuilt native CLI binaries for 5 platforms (Windows x64, macOS x64/ARM64, Linux x64/ARM64)
- npm publishing configuration
- TypeScript base configuration

---

## Pre-release History (unversioned)

The following commits occurred before the v1.0.0 tag and represent the initial project bootstrap:

| Date       | Description                                                                   |
| ---------- | ----------------------------------------------------------------------------- |
| 2026-06-17 | Initial project scaffold (`first commit`)                                     |
|            | Application structure with Tailwind CSS, todos page, navigation               |
|            | Security headers, performance benchmarks, deployment docs                     |
|            | Repository references updated, npm publishing documentation                   |
|            | Prebuilt native CLI binaries for multiple platforms                           |
|            | Adapter packages initialized (Bun, Cloudflare, Netlify, Node, Static, Vercel) |
|            | Foundational documentation and contributing guide                             |

---

## Release Tags

| Tag      | Date       | Type       |
| -------- | ---------- | ---------- |
| `v1.0.0` | 2026-06-17 | Production |
| `v1.0.1` | 2026-06-17 | Patch      |
| `v1.0.2` | 2026-06-18 | Minor      |
| `v1.0.3` | 2026-07-08 | Minor      |
| `v1.0.4` | 2026-07-01 | Minor      |
| `v1.0.5` | 2026-07-09 | Minor      |
| `v1.0.6` | 2026-07-09 | Patch      |
| `v1.0.7` | 2026-07-10 | Minor      |
| `v1.0.8` | 2026-07-10 | Minor      |
| `v1.0.9` | 2026-07-10 | Patch      |
