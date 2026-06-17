# Contributing

Ruvyxa is currently an MVP framework prototype. Keep changes small, explicit, and covered by tests.

## Development

```bash
pnpm install
cargo test --workspace
pnpm -r build
```

## Rust

- Use explicit error types in framework crates.
- Add tests for route and manifest behavior before changing route semantics.
- Do not silently ignore invalid routes.

## TypeScript

- Public APIs should be typed and documented.
- Keep package entrypoints small.
- Avoid adding runtime dependencies unless they are needed by user-facing APIs.
