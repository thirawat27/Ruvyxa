# Claude Instructions

Read `AGENTS.md` first and follow it as the source of truth.

This is a Ruvyxa app with file-based routing under `app/`, React 19, TypeScript, and Node.js 22 or
newer.

This project supports multiple package managers. The examples below use `npm`; use the equivalent
command for `pnpm`, `yarn`, or `bun` when appropriate.

Available scripts in `package.json`:

- `npm run dev` — development server with HMR
- `npm run build` — production build to `.ruvyxa/`
- `npm run start` — production server
- `npm run check` — app-level readiness checks (typecheck + parity + smoke)
- `npm run typecheck` — TypeScript type check only (`tsc --noEmit`)

Before handing off changes that affect routes, imports, config, environment usage, or production
behavior, run:

```bash
npm run check
```

For changes that affect production output, styling, or config, also run:

```bash
npm run build
```
