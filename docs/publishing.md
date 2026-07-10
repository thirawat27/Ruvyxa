# Publishing to npm

This document covers the npm publishing process for all Ruvyxa packages. In general, use the GitHub
Actions release workflow rather than publishing manually.

---

## Packages

### Core

| Package         | Description                                              |
| --------------- | -------------------------------------------------------- |
| `ruvyxa`        | CLI + runtime bridge (the main user-facing package)      |
| `create-ruvyxa` | Project scaffolding (`npm create ruvyxa`)                |
| `@ruvyxa/core`  | Shared primitives: config, server APIs, cache helpers    |
| `@ruvyxa/react` | React integration (error boundary, hydration, useLoader) |

### Adapters

| Package                      | Platform           |
| ---------------------------- | ------------------ |
| `@ruvyxa/adapter-node`       | Node.js            |
| `@ruvyxa/adapter-vercel`     | Vercel             |
| `@ruvyxa/adapter-cloudflare` | Cloudflare Workers |
| `@ruvyxa/adapter-netlify`    | Netlify            |
| `@ruvyxa/adapter-bun`        | Bun                |
| `@ruvyxa/adapter-static`     | Static export      |

### Native CLI Binaries

| Package                    | Platform    |
| -------------------------- | ----------- |
| `@ruvyxa/cli-win32-x64`    | Windows x64 |
| `@ruvyxa/cli-linux-x64`    | Linux x64   |
| `@ruvyxa/cli-linux-arm64`  | Linux arm64 |
| `@ruvyxa/cli-darwin-x64`   | macOS x64   |
| `@ruvyxa/cli-darwin-arm64` | macOS arm64 |

---

## Prerequisites

1. npm account with access to the `@ruvyxa` scope.
2. Logged in: `npm login`
3. Verified: `npm whoami` and `npm org ls ruvyxa`
4. Node 22+, pnpm 11+, Rust toolchain on the build machine.

---

## Pre-Publish Checks

Run the full validation suite:

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --locked -- -D warnings
pnpm install --frozen-lockfile
pnpm -r build
pnpm -r check
pnpm -r test
cargo run -p ruvyxa_cli -- check --root examples/demo
pnpm release:validate
pnpm pack:smoke
```

JavaScript and TypeScript tests are centralized under `tests/`. See [Testing](testing.md) for the
layout.

---

## Dry Run

Pack packages locally and inspect their contents:

```bash
pnpm pack --filter ruvyxa --pack-destination .npm-pack
pnpm pack --filter create-ruvyxa --pack-destination .npm-pack
pnpm pack --filter @ruvyxa/core --pack-destination .npm-pack
```

Check contents:

```bash
tar -tf .npm-pack/ruvyxa-<VERSION>.tgz
```

Confirm:

- `ruvyxa` includes `dist/`, `runtime/`, `bin/`, and `native-bin/`.
- `create-ruvyxa` includes `dist/`, `bin/`, and `template/minimal/`.
- Platform CLI packages include `bin/ruvyxa` or `bin/ruvyxa.exe`.
- Scoped packages include `dist/` and `README.md`.
- No `workspace:*` dependencies remain in packed `package.json`.

---

## Publish Order

Publish in dependency order. The CI workflow does this automatically.

```bash
# 1. Native binaries (one-per-platform — build on each OS)
# CI handles this; for local testing:
pnpm --filter @ruvyxa/cli-win32-x64 publish --access public
pnpm --filter @ruvyxa/cli-linux-x64 publish --access public
pnpm --filter @ruvyxa/cli-linux-arm64 publish --access public
pnpm --filter @ruvyxa/cli-darwin-x64 publish --access public
pnpm --filter @ruvyxa/cli-darwin-arm64 publish --access public

# 2. Core primitives
pnpm --filter @ruvyxa/core publish --access public

# 3. React integration
pnpm --filter @ruvyxa/react publish --access public

# 4. Adapters (can be published together)
pnpm --filter @ruvyxa/adapter-bun publish --access public &
pnpm --filter @ruvyxa/adapter-cloudflare publish --access public &
pnpm --filter @ruvyxa/adapter-netlify publish --access public &
pnpm --filter @ruvyxa/adapter-node publish --access public &
pnpm --filter @ruvyxa/adapter-static publish --access public &
pnpm --filter @ruvyxa/adapter-vercel publish --access public &
wait

# 5. Main packages (depend on everything above)
pnpm --filter ruvyxa publish --access public
pnpm --filter create-ruvyxa publish --access public
```

---

## CI Publishing

The `.github/workflows/release.yml` workflow handles:

1. **Tag push** (`v*.*.*`) or manual `workflow_dispatch` triggers the workflow.
2. **Resolve version** from `package.json` and validate tag match.
3. **Publish native binaries** in parallel across platform runners.
4. **Publish JS packages** sequentially in dependency order:
   - `@ruvyxa/core` → `@ruvyxa/react` → adapters → `ruvyxa` → `create-ruvyxa`
5. Each package is published only if its current npm version differs (via
   `scripts/publish-if-new.mjs`).

---

## Release Checklist

```bash
# Full manual release
git tag v$(node -p "require('./package.json').version")
git push origin --tags
# Then trigger the CI release workflow, or run:
pnpm release:validate
pnpm pack:smoke
# Follow the publish order above
```

---

## Related

- [Production Readiness](production-readiness.md) — quality gates and CI
- [Deployment](deployment.md) — adapters and hosting
- [Testing](testing.md) — test layout and conventions
