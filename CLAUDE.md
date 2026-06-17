# Claude Instructions

Read `AGENTS.md` first. It is the source of truth for working in this Ruvyxa monorepo.

Important local checks:

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
```
