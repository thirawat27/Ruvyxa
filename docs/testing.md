# Testing

Ruvyxa keeps standalone JavaScript and TypeScript tests in the repository-level `tests/` directory.
Package source folders stay focused on shipped code, while each package script still runs only its
own test subset.

The root package declares ESM module semantics so `.ts` tests under `tests/` can use native `import`
syntax with Node's built-in type stripping.

---

## Layout

| Directory                       | Scope                               |
| ------------------------------- | ----------------------------------- |
| `tests/packages/ruvyxa/`        | Runtime renderer and compiler tests |
| `tests/packages/core/`          | `@ruvyxa/core` public API tests     |
| `tests/packages/create-ruvyxa/` | Scaffold validation tests           |
| `tests/packages/adapter-*/`     | Adapter contract tests              |

Rust unit tests stay inline in their owning crates because they test private Rust modules directly.

---

## Test Files

| File                                             | Tests                              |
| ------------------------------------------------ | ---------------------------------- |
| `tests/packages/ruvyxa/action-renderer.test.mjs` | Action endpoint rendering          |
| `tests/packages/ruvyxa/api-renderer.test.mjs`    | API route forwarding               |
| `tests/packages/ruvyxa/client-renderer.test.mjs` | Client bundle boundary diagnostics |
| `tests/packages/ruvyxa/compiler.test.mjs`        | Runtime compiler, source maps,     |
|                                                  | incremental writes, JSX edge cases |
| `tests/packages/core/config.test.ts`             | Config API shape                   |
| `tests/packages/core/server.test.ts`             | Loader/action/cache API            |
| `tests/packages/create-ruvyxa/index.test.ts`     | Scaffold validation                |
| `tests/packages/adapter-*/index.test.ts`         | Adapter contract tests             |

---

## Commands

```bash
cargo test --workspace --locked
pnpm -r test
```

Package-level test scripts use Node's built-in test runner (`node --test`) and point back to
`tests/packages/...`:

```bash
pnpm --filter ruvyxa test       # tests/packages/ruvyxa/
pnpm --filter @ruvyxa/core test # tests/packages/core/
pnpm --filter create-ruvyxa test
```

The root-level `test` script combines both:

```bash
pnpm test   # runs cargo test + pnpm -r test
```

The test stack intentionally avoids external JavaScript bundlers and runners. Runtime compiler tests
exercise Ruvyxa's own compiler, source maps, incremental writes, dynamic imports, and TSX edge
cases. Rust unit tests remain inline in their owning crates.
