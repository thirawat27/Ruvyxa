# Debugging & Diagnostics

Ruvyxa provides structured diagnostics with unique error codes, explanations, file locations, and suggested fixes. Every diagnostic answers five questions:

1. What happened?
2. Where did it happen?
3. Why did it happen?
4. How do you fix it?
5. Which routes are affected?

---

## Diagnostic Format

```
RUV1007: Server-only module imported into client graph
File: app/blog/[slug]/page.tsx

Why:
  This module is reachable from a hydrated page but declares `server-only`.

Fix:
  Move server-only work behind a route handler/server module and pass serializable data to the client.

Affected routes:
  /blog/:slug
```

---

## Error Codes Reference

### Route Discovery

| Code | Title | Cause |
|------|-------|-------|
| `RUV1001` | App directory not found | Missing `app/` folder |
| `RUV1002` | Invalid dynamic route segment | Malformed bracket syntax in folder name |
| `RUV1003` | Duplicate route path | Two folders resolve to the same URL |
| `RUV1004` | Missing default export | `page.tsx` without `export default` |

### Client/Server Boundary

| Code | Title | Cause |
|------|-------|-------|
| `RUV1007` | Server-only module in client graph | Browser code imports a `"server-only"` module |
| `RUV1008` | Private env in client graph | Browser code reads `process.env.SECRET` (non-public) |
| `RUV1009` | Client-only module in server graph | Server code imports a `"client-only"` module |
| `RUV1010` | Server directory in client graph | Browser code imports from `server/` folder |

### Tailwind CSS

| Code | Title | Cause |
|------|-------|-------|
| `RUV1400` | Tailwind compilation failed | Tailwind CLI returned an error |
| `RUV1401` | Tailwind CLI not found | `@tailwindcss/cli` missing from `node_modules` |

### Server Actions

| Code | Title | Cause |
|------|-------|-------|
| `RUV1500` | Action runtime error | Validation failure or handler exception |
| `RUV1501` | Action module not found | Route has no `action.ts` file |
| `RUV1502` | Action renderer not found | Internal renderer script missing |
| `RUV1503` | Renderer args missing | Internal invocation error |

### SSR & Rendering

| Code | Title | Cause |
|------|-------|-------|
| `RUV1100` | SSR render failed | ReactDOMServer error during page render |
| `RUV1102` | SSR renderer not found | Internal renderer worker-pool.mjs missing |

### API Routes

| Code | Title | Cause |
|------|-------|-------|
| `RUV1200` | API route execution failed | Handler threw an unhandled exception |
| `RUV1201` | No available server port | All configured ports are in use |
| `RUV1202` | API renderer not found | Internal renderer script missing |

### Client Bundles

| Code | Title | Cause |
|------|-------|-------|
| `RUV1300` | Client bundle failed | Ruvyxa compiler error during hydration bundle |
| `RUV1301` | Module compile error | Compiler failed on a specific module |
| `RUV1302` | Client renderer not found | Internal renderer script missing |
| `RUV1303` | Client route not found | Route has no matching client page |
| `RUV1304` | Non-page client bundle | Client bundle requested for API-only route |

### Server Actions

| Code | Title | Cause |
|------|-------|-------|
| `RUV1500` | Action runtime error | Validation failure or handler exception |
| `RUV1501` | Action module not found | Route has no `action.ts` file |
| `RUV1502` | Action renderer not found | Internal renderer script missing |
| `RUV1503` | Renderer args missing | Internal invocation error |

### Config & CLI

| Code | Title | Cause |
|------|-------|-------|
| `RUV1600` | Config error | Invalid configuration value |
| `RUV1601` | Empty config field | Required config field is empty or not relative |
| `RUV1702` | Worker pool script not found | `worker-pool.mjs` missing from node_modules |

---

## Tools

### `ruvyxa analyze`

Validates the import graph and route conventions:

```bash
ruvyxa analyze
```

Reports all boundary violations, missing exports, and invalid routes as structured JSON with diagnostics. Use `ruvyxa check` as the normal before-deploy command; use `analyze` when you need the raw diagnostic payload.

### `ruvyxa check`

Runs the recommended app-level production readiness checks:

```bash
ruvyxa check
```

Use this before deploys. It combines TypeScript type checking, production build validation, dev/prod parity, and runtime page smoke rendering.

### `ruvyxa doctor`

Checks project health and environment:

```bash
ruvyxa doctor
```

Reports:
- App directory status
- Package manager detection
- Node/Bun/Deno availability
- React and ReactDOM versions
- Duplicate dependencies
- Route count and diagnostics
- `.env.example` presence
- Native CLI binary status

### `ruvyxa trace <path>`

Inspect route matching for a specific URL:

```bash
ruvyxa trace /blog/hello
```

Or via the HTTP endpoint while the server is running:

```bash
curl "http://localhost:3000/__ruvyxa/trace?path=/blog/hello"
```

Returns:
- Matched route and params
- Layout chain
- Server modules
- Client modules
- Runtime mode (dev or production)
- Asset directories

### `ruvyxa test:parity`

Compares dev and production route graphs and smoke-renders every page route in both modes:

```bash
ruvyxa test:parity
```

See [Dev/Prod Parity](parity.md) for details.

---

## Client Boundary Errors

The most common build-time errors involve the server/client boundary.

### `RUV1007` — Server-only in client

```ts
// lib/db.ts
import "server-only"  // marks this module as server-only
export const db = createClient(process.env.DATABASE_URL)
```

If `page.tsx` (which is hydrated in the browser) imports `lib/db.ts` either directly or transitively, Ruvyxa reports `RUV1007`.

**Fix:** Move the database call into `server.ts` and pass data as props to the page.

### `RUV1008` — Private env in client

```tsx
// page.tsx
const apiKey = process.env.API_SECRET  // RUV1008
```

**Fix:** Read the variable in a loader (`server.ts`) and pass only the result to the page.

### `RUV1010` — Server directory in client

Files under `server/` are reserved for server-only code. If a page imports from `server/utils.ts`, Ruvyxa reports `RUV1010`.

**Fix:** Move browser-safe utilities outside the `server/` directory.

---

## Tailwind Errors

### `RUV1401` — CLI not installed

```bash
npm install @tailwindcss/cli
```

### `RUV1400` — Compilation failed

Check the diagnostic output for the Tailwind stderr message. Common causes:
- Invalid `@source` paths in your CSS
- Syntax errors in custom CSS
- Missing content files

---

## Dev Error Overlay

In development mode (`RUVYXA_DEV=1`), Ruvyxa displays errors in a Next.js-style overlay instead of a plain text page:

- **Dark theme** with red error badge and title
- **Code frame** — source context lines surrounding the error location, with `>` marker and `← error` indicator
- **Suggested fix** — green-highlighted hint when a diagnostic includes one
- **Collapsible stack trace** — hidden by default, expand on click
- **Footer** — `Ruvyxa Dev Server — fix the error and save to hot-reload`

The overlay is triggered automatically for any `RuvyxaError::Diagnostic` returned during SSR rendering, API execution, or client bundle generation. Non-diagnostic errors fall back to a plain overlay with the error message.

Production mode (`ruvyxa start`) always returns the plain text error page. No overlay is injected into production responses.

---

## Security Headers

All runtime responses include these headers by default:

| Header | Value |
|--------|-------|
| `X-Content-Type-Options` | `nosniff` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `Permissions-Policy` | `camera=(), microphone=(), geolocation=()` |
| `Cross-Origin-Opener-Policy` | `same-origin` |

These provide a secure baseline. For custom CSP or additional headers, configure them at the deployment layer until Ruvyxa exposes a typed security config.

---

## Benchmarks

Measure framework performance locally:

```bash
ruvyxa bench --samples 3
ruvyxa bench --samples 5 --json
```

See [Performance](performance.md) for details.

---

## Related

- [Data Loading](data.md) — server/client boundary rules
- [Server Actions](actions.md) — action error codes
- [Dev/Prod Parity](parity.md) — consistency checks
- [Performance](performance.md) — benchmarking
