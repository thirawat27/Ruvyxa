# Publishing to npm

This document covers the npm publishing process for all Ruvyxa packages.

---

## Packages

### Core

| Package         | Description                                           |
| --------------- | ----------------------------------------------------- |
| `ruvyxa`        | CLI + runtime (the main user-facing package)          |
| `create-ruvyxa` | Project scaffolding (`npm create ruvyxa`)             |
| `@ruvyxa/core`  | Shared primitives: config, loaders, actions, adapters |
| `@ruvyxa/react` | React SSR and hydration integration                   |

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
3. Verified:

```bash
npm whoami
npm org ls ruvyxa
```

4. Node 22+, pnpm 10+, Rust toolchain installed on the build machine.

---

## Pre-Publish Checks

Run the full validation suite before publishing:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
pnpm install
pnpm -r build
pnpm -r check
pnpm -r test
cargo run -p ruvyxa_cli -- check --root examples/kitchen-sink
pnpm release:validate
pnpm pack:smoke
```

JavaScript and TypeScript tests are centralized under `tests/`; package scripts route to their own
subset. See [Testing](testing.md) before adding or moving test files.

---

## Dry Run

Pack packages locally and inspect their contents:

```bash
pnpm --filter ruvyxa pack --pack-destination .npm-pack
pnpm --filter create-ruvyxa pack --pack-destination .npm-pack
pnpm --filter @ruvyxa/core pack --pack-destination .npm-pack
```

Verify the tarballs:

```bash
tar -tf .npm-pack/ruvyxa-<VERSION>.tgz
tar -tf .npm-pack/create-ruvyxa-<VERSION>.tgz
```

Confirm:

- `ruvyxa` includes `dist/`, `runtime/`, `bin/`, and `native-bin/`.
- `create-ruvyxa` includes `dist/`, `bin/`, and `template/`.
- Platform CLI packages include `bin/ruvyxa` or `bin/ruvyxa.exe`.
- Scoped packages include `dist/` and `README.md`.
- No `workspace:*` dependencies remain in packed `package.json` files.

---

## Publish Order

Publish in dependency order — foundations first:

```bash
# 1. Core primitives
pnpm --filter @ruvyxa/core publish --access public

# 2. React integration
pnpm --filter @ruvyxa/react publish --access public

# 3. Adapters
pnpm --filter @ruvyxa/adapter-node publish --access public
pnpm --filter @ruvyxa/adapter-vercel publish --access public
pnpm --filter @ruvyxa/adapter-cloudflare publish --access public
pnpm --filter @ruvyxa/adapter-netlify publish --access public
pnpm --filter @ruvyxa/adapter-bun publish --access public
pnpm --filter @ruvyxa/adapter-static publish --access public

# 4. Native CLI binaries (from matching platform runners)
pnpm --filter @ruvyxa/cli-win32-x64 publish --access public
pnpm --filter @ruvyxa/cli-linux-x64 publish --access public
pnpm --filter @ruvyxa/cli-linux-arm64 publish --access public
pnpm --filter @ruvyxa/cli-darwin-x64 publish --access public
pnpm --filter @ruvyxa/cli-darwin-arm64 publish --access public

# 5. Main package (depends on core + optional CLI packages)
pnpm --filter ruvyxa publish --access public

# 6. Scaffolding tool (standalone)
pnpm --filter create-ruvyxa publish --access public
```

### Prereleases

Use a dist tag for pre-release versions:

```bash
pnpm --filter ruvyxa publish --access public --tag next
```

---

## Native Binary Publishing

Platform-specific CLI packages must be built and published from matching runners:

| Package                    | Runner      |
| -------------------------- | ----------- |
| `@ruvyxa/cli-win32-x64`    | Windows x64 |
| `@ruvyxa/cli-linux-x64`    | Linux x64   |
| `@ruvyxa/cli-linux-arm64`  | Linux arm64 |
| `@ruvyxa/cli-darwin-x64`   | macOS x64   |
| `@ruvyxa/cli-darwin-arm64` | macOS arm64 |

The GitHub release workflow handles this automatically via matrix builds.

---

## Post-Publish Verification

Verify the published packages work in a clean environment:

```bash
mkdir /tmp/verify && cd /tmp/verify
npm create ruvyxa@latest my-app
cd my-app
npm install
npx ruvyxa doctor
npx ruvyxa build
npx ruvyxa start --port 4000
```

Check:

- `create-ruvyxa` scaffolds without errors.
- `ruvyxa doctor` reports healthy state.
- `ruvyxa build` produces valid output.
- `ruvyxa start` serves the app.

If a platform binary is missing, `npx ruvyxa` will print the missing platform key — publish the
corresponding `@ruvyxa/cli-*` package to fix it.

---

## Automated Releases

Prefer the GitHub Actions release workflow (`.github/workflows/release.yml`) over manual publishing.
It:

- Runs all quality gates
- Builds binaries on each target platform
- Publishes with npm provenance
- Creates a GitHub release

Manual publishing is a fallback for emergency fixes only.

---

## Related

- [Production Readiness](production-readiness.md) — quality gates and release checklist
- [Deployment](deployment.md) — what users do after installing
