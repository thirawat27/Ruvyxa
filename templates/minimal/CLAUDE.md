# Claude Instructions

Read `AGENTS.md` first and follow it as the source of truth.

This is a Ruvyxa app with file-based routing under `app/`, React 19, TypeScript, and Node.js 22
runtime expectations.

Available scripts in `package.json`:

- `pnpm dev` — development server with HMR
- `pnpm build` — production build to `.ruvyxa/`
- `pnpm start` — production server
- `pnpm check` — app-level readiness checks (typecheck + parity + smoke)
- `pnpm typecheck` — TypeScript type check only (`tsc --noEmit`)

Before handing off changes that affect routes, imports, config, environment usage, or production
behavior, run:

```bash
pnpm check
```
