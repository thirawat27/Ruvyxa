# Publishing to npm

This repo publishes the public npm packages from `packages/`.

## Packages

- `ruvyxa`
- `create-ruvyxa`
- `@ruvyxa/core`
- `@ruvyxa/react`
- `@ruvyxa/adapter-node`
- `@ruvyxa/adapter-vercel`
- `@ruvyxa/adapter-cloudflare`
- `@ruvyxa/adapter-netlify`
- `@ruvyxa/adapter-bun`
- `@ruvyxa/adapter-static`
- `@ruvyxa/cli-win32-x64`
- `@ruvyxa/cli-linux-x64`
- `@ruvyxa/cli-linux-arm64`
- `@ruvyxa/cli-darwin-x64`
- `@ruvyxa/cli-darwin-arm64`

All packages use the GitHub repository `https://github.com/thirawat27/ruvyxa`.

## Prerequisites

1. Create or verify the npm account and npm organization/scope for `@ruvyxa`.
2. Run `npm login`.
3. Verify access:

```bash
npm whoami
npm org ls ruvyxa
```

4. Ensure Node 20+, pnpm 10+, Rust, and Cargo are installed on release/build machines. End users do not need Rust or Cargo when native binary packages are published.

## Release Checks

Run these from the repo root before publishing:

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace -- -D warnings
pnpm install
pnpm -r build
pnpm -r check
pnpm -r test
cargo run -p ruvyxa_cli -- test:parity --root examples/basic-app
```

## Dry Run

Use npm pack before publishing:

```bash
pnpm --filter @ruvyxa/core pack --pack-destination .npm-pack
pnpm --filter @ruvyxa/cli-win32-x64 pack --pack-destination .npm-pack
pnpm --filter ruvyxa pack --pack-destination .npm-pack
pnpm --filter create-ruvyxa pack --pack-destination .npm-pack
```

Inspect the tarballs:

```bash
tar -tf .npm-pack/ruvyxa-0.1.0.tgz
tar -tf .npm-pack/ruvyxa-cli-win32-x64-0.1.0.tgz
tar -tf .npm-pack/create-ruvyxa-0.1.0.tgz
```

Confirm:

- `ruvyxa` includes `dist/`, `runtime/`, `bin/`, and `native-bin/`.
- `create-ruvyxa` includes `dist/`, `bin/`, and `template/`.
- Platform CLI packages include `bin/ruvyxa` or `bin/ruvyxa.exe`.
- Scoped packages include `dist/` and `README.md`.
- No `workspace:*` dependencies are present in packed `package.json` files.

## Native CLI Packages

The public `ruvyxa` package resolves the native binary in this order:

1. `ruvyxa/native-bin/<platform>-<arch>/ruvyxa(.exe)` bundled in the package.
2. The matching optional package such as `@ruvyxa/cli-win32-x64`.
3. A source checkout binary under `target/debug` or `target/release`.

Publish platform packages from matching runners:

| Package | Runner |
|---|---|
| `@ruvyxa/cli-win32-x64` | Windows x64 |
| `@ruvyxa/cli-linux-x64` | Linux x64 |
| `@ruvyxa/cli-linux-arm64` | Linux arm64 |
| `@ruvyxa/cli-darwin-x64` | macOS x64 |
| `@ruvyxa/cli-darwin-arm64` | macOS arm64 |

## Publish Order

Publish internal foundations first, then packages that depend on them:

```bash
pnpm --filter @ruvyxa/core publish --access public
pnpm --filter @ruvyxa/react publish --access public
pnpm --filter @ruvyxa/adapter-node publish --access public
pnpm --filter @ruvyxa/adapter-vercel publish --access public
pnpm --filter @ruvyxa/adapter-cloudflare publish --access public
pnpm --filter @ruvyxa/adapter-netlify publish --access public
pnpm --filter @ruvyxa/adapter-bun publish --access public
pnpm --filter @ruvyxa/adapter-static publish --access public
pnpm --filter @ruvyxa/cli-win32-x64 publish --access public
pnpm --filter @ruvyxa/cli-linux-x64 publish --access public
pnpm --filter @ruvyxa/cli-linux-arm64 publish --access public
pnpm --filter @ruvyxa/cli-darwin-x64 publish --access public
pnpm --filter @ruvyxa/cli-darwin-arm64 publish --access public
pnpm --filter ruvyxa publish --access public
pnpm --filter create-ruvyxa publish --access public
```

For prereleases, publish with a tag:

```bash
pnpm --filter ruvyxa publish --access public --tag next
```

## After Publishing

Verify installs in a clean directory:

```bash
npm create ruvyxa@latest my-ruvyxa-app
cd my-ruvyxa-app
npm install
npx ruvyxa doctor --root .
npx ruvyxa build --root .
```

The published CLI uses prebuilt native binaries. If a platform package is missing, `npx ruvyxa` prints the missing platform key so the release can be completed with the matching `@ruvyxa/cli-*` package.
