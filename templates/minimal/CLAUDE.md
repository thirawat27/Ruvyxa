# Claude Instructions

Read `AGENTS.md` first and follow it as the source of truth. This is a Ruvyxa app with file-based routing under `app/`, Node.js 22 runtime expectations, React 19, and TypeScript.

Before handing off changes that affect routes, imports, config, middleware, environment usage, or production behavior, run:

```bash
pnpm check
pnpm analyze
pnpm build
```

Run `pnpm parity` for changes that could affect dev/prod route behavior. Keep `analyze` JSON output machine-readable and do not work around framework validation by moving unsafe imports into browser-reachable files.
