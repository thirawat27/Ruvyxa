# Plan: Ruvyxa 1.0.15 Styling, SSG, and Starters

> Created by Squirrel on 2026-07-18

## Goal

Complete the Ruvyxa 1.0.15 developer experience by adding automatic SCSS and CSS Modules support,
making dynamic SSG parameters easier to declare and safely cache, exposing three production-ready
starter choices, and documenting the shipped behavior in English and Thai.

## Architecture Focus

- **Pass:** Focus — the change crosses the bundler/runtime style contract, route graph and
  build-time SSG worker, scaffolding/package output, and user documentation.
- **Confirmed flow:** Rust produces browser bundles, the Node runtime compiler produces SSR/SSG
  bundles, and the Rust dev server/CLI collects CSS for HTML injection.
- **Invariant:** `.module.css` and `.module.scss` must produce the same deterministic class mapping
  in browser and server bundles while the collected CSS uses those exact generated names.
- **Preserve:** Existing global CSS imports, `getStaticParams`, default minimal scaffolding,
  CLI/config behavior, server/client boundary checks, and package-manager-neutral templates.
- **Dependency decision:** Use a Rust Sass compiler for native style collection and Dart Sass in the
  packaged Node runtime; both compile before the shared deterministic CSS Modules naming rule.

## Task Breakdown

### Layer 1 — Styling contract

- [x] 🔴 Add deterministic CSS Modules parsing/scoping to `ruvyxa_bundler`, resolve `.module.css`
      and `.module.scss` as graph dependencies, and emit default class-map exports. Done when Rust
      bundle tests prove stable unique names and usable React imports.
- [x] 🔴 Update `ruvyxa_dev_server/src/style.rs` to compile `.scss`/`.sass`, scope CSS Modules with
      the same contract, follow Sass dependencies, and keep global CSS behavior. Done when style
      tests cover global SCSS, module SCSS, nested selectors, imports, and diagnostics.
- [x] 🔴 Align `packages/ruvyxa/runtime/compiler.mjs` with the same module mapping and Sass
      behavior. Done when SSR/client runtime compiler tests import both `.module.css` and
      `.module.scss` and include style inputs in dependency fingerprints.
- [x] 🟢 Expand framework-owned CSS ambient declarations for CSS/SCSS modules.

### Layer 2 — SSG parameters and cache

- [x] 🟡 Add typed `staticParams` shorthand/result contracts while preserving `getStaticParams`.
      Done when single-segment string arrays and object arrays normalize to the existing route
      params.
- [x] 🟡 Pass the real route manifest and route segment metadata into static-param discovery. Done
      when `StaticParamsContext.routes` matches its public contract and graph detection covers both
      APIs.
- [x] 🔴 Add dependency-aware, opt-in persistent parameter caching with bounded duration parsing and
      atomic cache writes. Done when worker tests prove cache reuse, expiry, and dependency
      invalidation.

### Layer 3 — Starter templates

- [x] 🟡 Promote source-of-truth `templates/blog`, `templates/crud`, and `templates/api-backend`,
      using the existing draft content where sound. Done when every starter has a valid
      package-neutral app.
- [x] 🟡 Expose `--template minimal|blog|crud|api-backend` in `create-ruvyxa`, preserve minimal as
      the default, copy all templates during prepack, and validate unknown choices. Done when Node
      tests and tarball smoke assertions cover every template.

### Layer 4 — Documentation and release surface

- [x] 🟢 Update README, create-ruvyxa README, EN/TH guides, guide index, and 1.0.15 release notes
      for SCSS/CSS Modules, SSG shorthand/cache, and starter selection.

### Layer 5 — Verification

- [x] 🟡 Run focused Rust and Node suites after each contract group.
- [x] 🔴 Run formatting, workspace checks/tests, demo check/parity, release validation, and pack
      smoke.

## Risks & Mitigations

| Risk                                       | Likelihood | Impact | Mitigation                                                                        |
| ------------------------------------------ | ---------- | ------ | --------------------------------------------------------------------------------- |
| Rust and Node class names diverge          | Medium     | High   | Golden test vectors shared by both implementations and an integration fixture     |
| Sass imports are absent from invalidation  | Medium     | High   | Track loaded style files in style collection and runtime bundle inputs            |
| SSG cache serves stale remote data         | Medium     | High   | Cache only when explicitly requested and invalidate on dependency fingerprint/TTL |
| New templates disappear during npm prepack | High       | High   | Copy all source templates and inspect packed tarball entries                      |
| Existing CLI consumers break               | Low        | High   | Keep current function call/default template additive and test it                  |

## Open Questions

None identified. The user explicitly requested the three starter categories and additive framework
support; naming is normalized to `blog`, `crud`, and `api-backend`.

## Progress

| Phase    | Status   | Signal                                                         |
| -------- | -------- | -------------------------------------------------------------- |
| Discover | Complete | Baseline Rust/Node suites green; no known pnpm vulnerabilities |
| Plan     | Complete | Architecture and verification boundaries locked                |
| Build    | Complete | Styling, SSG, and starters implemented                         |
| Test     | Complete | Focused, workspace, demo, and tarball suites passed            |
| Bug Hunt | Complete | Pack smoke corrected an invalid API starter contract           |
| Polish   | Complete | Format, lint, audit, and diff checks passed                    |
| Document | Complete | EN/TH guides, READMEs, and 1.0.15 notes updated                |
| Ship     | Complete | Release validation, demo parity, and pack smoke passed         |
