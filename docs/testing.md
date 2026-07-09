# Testing

Ruvyxa keeps standalone JavaScript and TypeScript tests in the repository-level `tests/` directory. Package source folders stay focused on shipped code, while each package script still runs only its own test subset.

The root package declares ESM module semantics so `.ts` tests under `tests/` can use native `import` syntax with Node's built-in type stripping.

## Layout

| Directory | Scope |
|-----------|-------|
| `tests/packages/ruvyxa/` | Runtime renderer and compiler tests |
| `tests/packages/core/` | `@ruvyxa/core` public API tests |
| `tests/packages/create-ruvyxa/` | Scaffold validation tests |
| `tests/packages/adapter-*/` | Adapter contract tests |

Rust unit tests stay inline in their owning crates because they test private Rust modules directly.

## Commands

```bash
cargo test --workspace
pnpm -r test
```

Package-level test scripts use Node's built-in test runner (`node --test`) and point back to `tests/packages/...`, for example:

```bash
pnpm --filter ruvyxa test       # tests/packages/ruvyxa/
pnpm --filter @ruvyxa/core test # tests/packages/core/
pnpm --filter create-ruvyxa test
```

All listed packages run automatically via `pnpm -r test`. The root-level `test` script also includes `cargo test --workspace`:

```bash
pnpm test   # runs cargo test + pnpm -r test
```

The test stack intentionally avoids external JavaScript bundlers and runners. Runtime compiler tests exercise Ruvyxa's own compiler, source maps, incremental writes, dynamic imports, and TSX edge cases. Rust unit tests remain inline in their owning crates because they test private Rust modules directly.
