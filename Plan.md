# Ruvyxa MVP Plan

## Audit Report

**Mode:** Greenfield.
**Stack:** Rust workspace for CLI/dev server/route graph/diagnostics, pnpm workspace for TypeScript public packages.
**Existing files:** `ruvyxa_framework_spec.md` only.
**Tests:** None yet.
**Tooling:** None yet.

## Task Breakdown

1. Bootstrap monorepo
   - Done when Cargo and pnpm workspaces exist with the crate/package layout from the spec.
2. Implement Rust CLI
   - Done when `ruvyxa dev`, `build`, `start`, `routes`, and `doctor` are wired.
3. Implement route discovery
   - Done when `app/page.tsx`, nested routes, dynamic routes, catch-all routes, and duplicate conflicts are tested.
4. Implement minimal runtime
   - Done when the dev server serves React SSR page routes, real API route execution, CSS injection, and full-page reload over WebSocket.
5. Implement production output
   - Done when `ruvyxa build` writes `.ruvyxa/manifest.json` and copied app sources, and `ruvyxa start` serves them using the same route discovery/rendering path.
6. Add TypeScript public API
   - Done when `defineConfig`, `loader`, `action`, `cache`, `redirect`, `notFound`, and `json` are exported.
7. Add docs and examples
   - Done when README, getting started docs, templates, and `examples/basic-app` exist.

## Scope Notes

This MVP now implements React SSR and TS/TSX transformation through an esbuild-powered Node renderer bridge. It still does not implement client hydration, optimized bundling, component-level HMR, deploy adapters, or the full server action protocol.
