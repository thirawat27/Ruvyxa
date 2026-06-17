# Production Readiness

Ruvyxa 1.0 is release-ready when the CI and release workflow pass for the target tag.

## Required Gates

- Rust formatting, tests, and clippy pass.
- TypeScript build, type check, and tests pass.
- npm package metadata validates for every public package.
- npm pack smoke passes and verifies tarball contents.
- Native CLI binaries are packed for supported platforms.
- The example app passes dev/prod route parity.

## Runtime Guarantees

- `ruvyxa dev`, `ruvyxa build`, and `ruvyxa start` share route semantics.
- Production builds emit `.ruvyxa/server`, `.ruvyxa/assets`, `.ruvyxa/client`, `manifest.json`, and `build.json`.
- Client bundles are route-level, minified, tree-shaken by esbuild, and BLAKE3-hashed.
- Server actions are route-local and guarded by origin, Fetch Metadata, content type, body size, and rate-limit checks.
- Runtime responses include conservative default security headers.

## Native CLI Distribution

The `ruvyxa` npm package uses prebuilt native binaries:

- bundled `native-bin/<platform>-<arch>/ruvyxa(.exe)` for the package build platform
- optional packages such as `@ruvyxa/cli-win32-x64`
- source checkout binaries only as a developer fallback

End users installing from npm do not need Rust or Cargo.

## Release Command Summary

```bash
pnpm install
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
pnpm release:validate
pnpm pack:smoke
cargo run -p ruvyxa_cli -- test:parity --root examples/basic-app
```

Use the GitHub release workflow for actual npm publishing so npm provenance and platform binaries are produced consistently.
