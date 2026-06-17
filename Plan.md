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
   - Done when the dev server serves React SSR page routes, real API route execution, CSS/Tailwind injection, and full-page reload over WebSocket.
5. Implement client boundary validation
   - Done when browser bundles fail on `server-only`, `server/` imports, and private environment variables with clear diagnostics.
5. Implement production output
   - Done when `ruvyxa build` writes `.ruvyxa/server`, `.ruvyxa/client`, `.ruvyxa/assets`, and `.ruvyxa/manifest.json`, and `ruvyxa start` serves them using the same route discovery/rendering path.
6. Add TypeScript public API
   - Done when `defineConfig`, `loader`, `action`, `cache`, `redirect`, `notFound`, and `json` are exported.
7. Add docs and examples
   - Done when README, getting started docs, templates, and `examples/basic-app` exist.
8. Add framework app handoff files
   - Done when root, template, and example apps include `AGENTS.md` and `CLAUDE.md`.
9. Add server actions MVP
   - Done when `action.ts` route modules can be invoked through the Ruvyxa runtime action endpoint with validation and tests.
10. Add runtime trace API
   - Done when the server exposes matched route, params, modules, and runtime mode through `/__ruvyxa/trace`.
11. Add dev/prod parity checks
   - Done when `ruvyxa test:parity` builds production output and compares dev/prod route graphs.
12. Expand doctor checks
   - Done when `ruvyxa doctor` reports package manager, Node/Bun/Deno availability, React compatibility, duplicate dependency versions, env schema presence, and route diagnostics.
13. Add env file loading
   - Done when SSR, API routes, and server actions receive `.env`/`.env.local` values and templates include `.env.example`.
14. Add first deployment adapter
   - Done when `@ruvyxa/adapter-node` exposes the first-party adapter contract and docs describe Node deployment output.
15. Add optimized route client bundles
   - Done when production build emits hashed per-route client scripts, a client manifest, minification, and tree-shaking metadata consumed by `ruvyxa start`.
16. Add component-aware HMR events
   - Done when the watcher distinguishes CSS updates, component updates, and full reload fallbacks, and the browser runtime handles each event type.
17. Add managed-host adapters
   - Done when Vercel, Cloudflare, Netlify, Bun, and static adapter packages expose typed first-party contracts.
18. Add benchmark suite
   - Done when `ruvyxa bench` reports route discovery, analyze validation, and production build timings in table or JSON form.
19. Harden server actions
   - Done when action requests enforce body size, supported content types, same-origin checks, and route-local action resolution.
20. Add production hardening defaults
   - Done when builds use deterministic content hashes, action payload limits are enforced before body extraction, runtime responses include default security headers, and static asset paths reject traversal.

## Scope Notes

This MVP now implements React SSR, client hydration, route-level optimized client bundles with deterministic BLAKE3 hashes, API route execution, hardened server action invocation, default security headers, runtime tracing, dev/prod route parity checks, benchmark reporting, expanded doctor checks, env file loading, first-party adapter contracts, Tailwind CSS v4 compilation, TS/TSX transformation through esbuild-powered Node renderer bridges, component-aware HMR events, and basic client boundary validation. Advanced Fast Refresh state preservation, source-map-rich optimizer reports, and host-specific deployment file emitters remain future production-hardening work.
