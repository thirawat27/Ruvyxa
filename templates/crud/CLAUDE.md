# Claude Instructions

Read `AGENTS.md` first and follow it as the source of truth.

This is a Ruvyxa app with file-based routing under `app/`, React, and TypeScript. The Node.js floor
and every dependency version live in `package.json`; read them there rather than assuming a number.

This project supports multiple package managers. The examples below use `npm`; use the equivalent
command for `pnpm`, `yarn`, or `bun` when appropriate.

Available scripts in `package.json`:

- `npm run dev` — development server with HMR
- `npm run build` — production build to `.ruvyxa/`
- `npm run start` — production server
- `npm run check` — app-level readiness checks (typecheck + parity + smoke)
- `npm run typecheck` — TypeScript type check only (`tsc --noEmit`)
- `npm run preview` — serve the production output locally
- `npm run routes` — print discovered routes
- `npm run analyze` — inspect route, import, and boundary diagnostics
- `npm run adds -- form` — scaffold a supported feature
- `npm run doctor` — diagnose project and environment issues
- `npm run clean` — remove generated build output
- `npm run trace -- /` — inspect one route-manifest entry
- `npm run bench` — benchmark discovery, analysis, and build
- `npm run test:parity` — compare dev and production routes
- `npm run plugin -- create my-plugin` — scaffold a plugin package

Pass every argument for a framework command after `--`, for example
`npm run analyze -- --format sarif --output reports/ruvyxa.sarif`.

Before handing off changes that affect routes, imports, config, environment usage, or production
behavior, run:

```bash
npm run check
```

For changes that affect production output, styling, or config, also run:

```bash
npm run build
```
