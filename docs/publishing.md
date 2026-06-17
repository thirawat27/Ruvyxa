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

All packages use the GitHub repository `https://github.com/thirawat27/ruvyxa`.

## Prerequisites

1. Create or verify the npm account and npm organization/scope for `@ruvyxa`.
2. Run `npm login`.
3. Verify access:

```bash
npm whoami
npm org ls ruvyxa
```

4. Ensure Node 20+, pnpm 10+, Rust, and Cargo are installed.

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
pnpm --filter ruvyxa pack --pack-destination .npm-pack
pnpm --filter create-ruvyxa pack --pack-destination .npm-pack
```

Inspect the tarballs:

```bash
tar -tf .npm-pack/ruvyxa-0.1.0.tgz
tar -tf .npm-pack/create-ruvyxa-0.1.0.tgz
```

Confirm:

- `ruvyxa` includes `dist/`, `runtime/`, `bin/`, and `native-src/`.
- `create-ruvyxa` includes `dist/`, `bin/`, and `template/`.
- Scoped packages include `dist/` and `README.md`.
- No `workspace:*` dependencies are present in packed `package.json` files.

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

The v0.1 CLI package is source-built and requires Rust/Cargo on the machine running `npx ruvyxa`. Future releases can replace this with platform-specific native binary packages.
