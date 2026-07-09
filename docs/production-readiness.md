# Production Readiness

Ruvyxa 1.0 is production-ready when the full CI pipeline passes. This document defines the quality
gates, runtime guarantees, and release checklist.

---

## Quality Gates

For application projects, use the single app-level gate first:

```bash
ruvyxa check
```

It runs TypeScript type checking (when `tsconfig.json` is present), builds production output,
compares dev/prod route behavior, and smoke-renders every page route.

All of the following must pass before a release:

| Gate                       | Command                                                               |
| -------------------------- | --------------------------------------------------------------------- |
| Rust formatting            | `cargo fmt --all -- --check`                                          |
| Rust tests                 | `cargo test --workspace --locked`                                     |
| Rust lints                 | `cargo clippy --workspace -- -- -D warnings`                          |
| TypeScript build           | `pnpm -r build`                                                       |
| TypeScript type check      | `pnpm -r check`                                                       |
| TypeScript tests           | `pnpm -r test`                                                        |
| Package metadata           | `pnpm release:validate`                                               |
| Pack smoke test            | `pnpm pack:smoke`                                                     |
| App deploy gate            | `cargo run -p ruvyxa_cli -- check --root examples/kitchen-sink`       |
| Dev/prod parity drill-down | `cargo run -p ruvyxa_cli -- test:parity --root examples/kitchen-sink` |

---

## Test Layout

Standalone JavaScript and TypeScript tests are centralized under `tests/`, grouped by package.
Package `test` scripts point to their own subset in `tests/packages/...`. Integration tests for
runtime renderers (`action-renderer.mjs`, `api-renderer.mjs`, `client-renderer.mjs`, `compiler.mjs`)
live under `tests/packages/ruvyxa/`. Rust unit tests remain inline in their crates. See
[Testing](testing.md) for details.

---

## Runtime Guarantees

### Route Semantics

- `ruvyxa dev`, `ruvyxa build`, and `ruvyxa start` share the same route discovery and matching
  algorithm.
- The parity check enforces this at CI time.

### Build Output

| Directory               | Contents                           |
| ----------------------- | ---------------------------------- |
| `.ruvyxa/server/`       | Production route source for SSR    |
| `.ruvyxa/client/`       | Route-level hydration bundles      |
| `.ruvyxa/assets/`       | Static files from `public/`        |
| `.ruvyxa/prerender/`    | Pre-rendered HTML files            |
| `.ruvyxa/manifest.json` | Full route manifest                |
| `.ruvyxa/build.json`    | Build metadata and config snapshot |

### Client Bundles

- Route-level splitting (one bundle per page)
- Minified and tree-shaken by default
- BLAKE3 content-addressed file names (immutable caching)
- Dynamic `import()` split points emitted as chunk files
- Per-route bundle metrics in `client/manifest.json`

### Server Actions

- Route-local (cannot invoke arbitrary modules)
- Origin validation (same-origin only)
- Fetch Metadata checks (`Sec-Fetch-Site`)
- Content-Type enforcement (JSON or form-encoded)
- Body size limit (64 KB)
- Per-client rate limiting (60 req/min)

### Security Headers

All responses include:

- `X-Content-Type-Options: nosniff`
- `Referrer-Policy: strict-origin-when-cross-origin`
- `Permissions-Policy: camera=(), microphone=(), geolocation=()`
- `Cross-Origin-Opener-Policy: same-origin`

---

## Native CLI Distribution

End users install from npm and receive a prebuilt native binary. No Rust toolchain required.

Resolution order:

1. Bundled binary at `ruvyxa/native-bin/<platform>-<arch>/ruvyxa(.exe)`
2. Optional platform package (e.g., `@ruvyxa/cli-win32-x64`)
3. Source checkout fallback (`target/debug` or `target/release`)

### Supported Platforms

| Package                    | OS      | Architecture          |
| -------------------------- | ------- | --------------------- |
| `@ruvyxa/cli-win32-x64`    | Windows | x64                   |
| `@ruvyxa/cli-linux-x64`    | Linux   | x64                   |
| `@ruvyxa/cli-linux-arm64`  | Linux   | arm64                 |
| `@ruvyxa/cli-darwin-x64`   | macOS   | x64                   |
| `@ruvyxa/cli-darwin-arm64` | macOS   | arm64 (Apple Silicon) |

---

## Release Checklist

```bash
# 1. Clean state
git status

# 2. Rust checks
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace -- -- -D warnings

# 3. TypeScript checks
pnpm install --frozen-lockfile
pnpm -r build
pnpm -r check
pnpm -r test

# 4. Integration checks
cargo run -p ruvyxa_cli -- check --root examples/kitchen-sink
pnpm release:validate
pnpm pack:smoke

# 5. Full smoke test
cargo run -p ruvyxa_cli -- dev --root examples/kitchen-sink --port 3001
cargo run -p ruvyxa_cli -- build --root examples/kitchen-sink
cargo run -p ruvyxa_cli -- start --root examples/kitchen-sink --port 3002
```

---

## CI/CD

The GitHub Actions workflow (`.github/workflows/ci.yml`) runs on every push and pull request:

1. Rust formatting, tests, clippy on ubuntu, macOS, and Windows
2. TypeScript build, check, and test
3. Package metadata validation and pack smoke test

The release workflow (`.github/workflows/release.yml`) runs on `v*.*.*` tags:

1. All quality gates
2. Build native CLI binaries per platform
3. Publish npm packages in dependency order with provenance

Never publish manually unless CI is unavailable and the full checklist passes locally.

---

## Related

- [Deployment](deployment.md) — adapters and hosting
- [Performance](performance.md) — build benchmarks
- [Publishing](publishing.md) — npm publish procedure
- [Dev/Prod Parity](parity.md) — consistency verification
