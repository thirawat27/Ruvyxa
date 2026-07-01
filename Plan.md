# Ruvyxa Improvement Plan

## Audit Report

- **Mode:** Mature monorepo, targeted compatibility and stability improvement.
- **Stack:** Rust CLI/dev server/graph/bundler plus typed TypeScript APIs and a Node renderer bridge.
- **Existing foundation:** Structured `RUV####` diagnostics, radix routing, dev/build/start commands, Rust and package tests, formatting, linting, and CI workflows.
- **Baseline:** `cargo test --workspace` passes 139 tests. The first package-test attempt was blocked by pnpm's non-interactive modules purge and will be repeated with `CI=true`.
- **Protected user work:** Existing edits in `crates/ruvyxa_cli/src/main.rs` and `docs/production-readiness.md` are outside this task's edit scope.
- **Primary gap:** File-route discovery and the two runtime matchers disagree with documented Next.js-style App Router semantics.

## Task Breakdown

1. **Next.js-compatible route grammar (hard)**
   - Reuse the existing route manifest and structured diagnostics.
   - Support `[name]`, `[...name]`, and `[[...name]]`.
   - Reject unsupported or malformed dynamic segments before output is emitted.
   - Treat private folders and parallel-route slots as non-routable.
   - Done when graph tests prove each convention and invalid forms return `RUV1002`.

2. **Deterministic conflict validation (medium)**
   - Extend existing discovery validation to reject routes with the same match shape even when parameter names differ.
   - Reject `page` and `route` files that resolve to the same URL.
   - Done when conflicts fail discovery with `RUV1003` and identify the affected files.

3. **Shared runtime semantics (hard)**
   - Update both the radix router and synchronous fallback matcher for required and optional catch-all behavior.
   - Preserve static-over-dynamic precedence.
   - Done when both matcher test suites cover zero-, one-, and multi-segment cases.

4. **Documentation and templates (easy)**
   - Correct routing documentation and README route examples where applicable.
   - No template change is needed unless the starter exposes a changed convention.

5. **Verification (hard)**
   - Run formatting, Rust tests and Clippy, package build/check/test, analyze, then dev/build/start smoke tests.
   - Report any environment-only blocker separately from code failures.

## Risks and Compatibility

- Applications using the non-Next `[[name]]` convention will now receive a build-time diagnostic and must migrate to `[[...name]]`.
- Catch-all parameters remain serialized through Ruvyxa's current `Record<string, string>` public contract as slash-joined values; changing them to Next.js arrays requires a separate public API migration.
- This milestone aligns file-route discovery and matching only. It does not claim feature parity with every Next.js subsystem.

## Completion

- Route discovery now implements the Next.js dynamic, catch-all, and optional catch-all folder grammar.
- Private folders and parallel-route slot trees no longer become accidental standalone routes.
- Match-shape conflicts fail with structured `RUV1003` diagnostics and affected route IDs.
- The synchronous fallback and live server use the same radix matcher.
- Rust formatting, 142 Rust tests, Clippy, all package builds/checks/tests, app analysis, app-level parity checks, and dev/production HTTP smoke tests pass.
