# Debugging & Diagnostics

Ruvyxa provides structured diagnostics with unique error codes, explanations, file locations, and
suggested fixes. Every diagnostic answers five questions:

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

| Code      | Title                         | Cause                                   |
| --------- | ----------------------------- | --------------------------------------- |
| `RUV1001` | App directory not found       | Missing `app/` folder                   |
| `RUV1002` | Invalid dynamic route segment | Malformed bracket syntax in folder name |
| `RUV1003` | Duplicate route path          | Two folders resolve to the same URL     |
| `RUV1004` | Missing default export        | `page.tsx` without `export default`     |

### Client/Server Boundary

| Code      | Title                              | Cause                                                |
| --------- | ---------------------------------- | ---------------------------------------------------- |
| `RUV1007` | Server-only module in client graph | Browser code imports a `"server-only"` module        |
| `RUV1008` | Private env in client graph        | Browser code reads `process.env.SECRET` (non-public) |
| `RUV1009` | Client-only module in server graph | Server code imports a `"client-only"` module         |
| `RUV1010` | Server directory in client graph   | Browser code imports from `server/` folder           |

### SSR & Rendering

| Code      | Title                     | Cause                                                               |
| --------- | ------------------------- | ------------------------------------------------------------------- |
| `RUV1100` | SSR render failed         | ReactDOMServer error during page render                             |
| `RUV1101` | SSR renderer args missing | SSR renderer missing required projectRoot/appDir/pageFile arguments |
| `RUV1102` | SSR renderer not found    | Internal renderer `ssr-renderer.mjs` missing                        |

### API Routes

| Code      | Title                      | Cause                                |
| --------- | -------------------------- | ------------------------------------ |
| `RUV1200` | API route execution failed | Handler threw an unhandled exception |
| `RUV1201` | No available server port   | All configured ports are in use      |
| `RUV1202` | API renderer not found     | Internal renderer script missing     |

### Partial Prerendering (PPR)

| Code      | Title             | Cause                                   |
| --------- | ----------------- | --------------------------------------- |
| `RUV1550` | PPR render failed | PPR (Partial Prerendering) render error |

### Client Bundles

| Code      | Title                     | Cause                                         |
| --------- | ------------------------- | --------------------------------------------- |
| `RUV1300` | Client bundle failed      | Ruvyxa compiler error during hydration bundle |
| `RUV1301` | Module compile error      | Compiler failed on a specific module          |
| `RUV1302` | Client renderer not found | Internal renderer script missing              |
| `RUV1303` | Client route not found    | Route has no matching client page             |
| `RUV1304` | Non-page client bundle    | Client bundle requested for API-only route    |
| `RUV1310` | Markdown parse error      | A `.md` page contains invalid content syntax  |
| `RUV1311` | MDX parse error           | A `.mdx` page contains invalid JSX/expression |
| `RUV1312` | Frontmatter not closed    | Opening `---` has no closing delimiter        |

### Styles and Tailwind CSS

| Code      | Title                        | Cause                                               |
| --------- | ---------------------------- | --------------------------------------------------- |
| `RUV1400` | Tailwind compilation failed  | Tailwind CLI returned an error                      |
| `RUV1401` | Tailwind CLI not found       | `@tailwindcss/cli` missing from `node_modules`      |
| `RUV1402` | CSS preprocessor unavailable | Sass/Less import has no configured transform plugin |
| `RUV1403` | Stylesheet could not resolve | Imported or configured CSS path does not exist      |
| `RUV1404` | CSS entry outside project    | `css.entries` escapes the project root              |

### Server Actions

| Code      | Title                     | Cause                                           |
| --------- | ------------------------- | ----------------------------------------------- |
| `RUV1500` | Action runtime error      | Validation failure or handler exception         |
| `RUV1501` | Action module not found   | Route has no `action.ts` or `action.js`         |
| `RUV1502` | Action renderer not found | Internal renderer `action-renderer.mjs` missing |
| `RUV1503` | Renderer args missing     | Internal invocation error                       |

### Config & CLI

| Code      | Title              | Cause                                                          |
| --------- | ------------------ | -------------------------------------------------------------- |
| `RUV1600` | Config error       | Invalid configuration value                                    |
| `RUV1601` | Empty config field | Required config field is empty or not relative to project root |

### Build Plugins

| Code      | Title                        | Cause                                         |
| --------- | ---------------------------- | --------------------------------------------- |
| `RUV1700` | Config plugin error          | JS build plugin bridge returned an error      |
| `RUV1701` | JS plugin runner not found   | `plugin-runner.mjs` missing from node_modules |
| `RUV1702` | Worker pool script not found | `worker-pool.mjs` missing from node_modules   |
| `RUV1801` | Module resolution error      | Bundler could not resolve a module specifier  |

### Middleware

| Code      | Title                          | Cause                                |
| --------- | ------------------------------ | ------------------------------------ |
| `RUV2000` | Middleware configuration error | Invalid or missing middleware config |
| `RUV2001` | Middleware execution failed    | Middleware layer threw at runtime    |

### Wasm Plugins

| Code      | Title                        | Cause                                                   |
| --------- | ---------------------------- | ------------------------------------------------------- |
| `RUV2100` | Wasm plugin load error       | Plugin `.wasm` file could not be loaded or compiled     |
| `RUV2101` | Wasm plugin execution error  | Plugin hook threw or was killed by timeout/memory limit |
| `RUV2102` | Wasm plugin hot-reload error | Watcher failed to reload `.wasm` file on change         |

---

## Tools

### `ruvyxa analyze`

Validates the import graph and route conventions:

```bash
ruvyxa analyze
```

Reports all boundary violations, missing exports, and invalid routes as structured JSON with
diagnostics. Use `ruvyxa check` as the normal before-deploy command; use `analyze` when you need the
raw diagnostic payload.

### `ruvyxa check`

Runs the recommended app-level production readiness checks:

```bash
ruvyxa check
```

Combines TypeScript type checking (when `tsconfig.json` is present), production build validation,
dev/prod parity, and runtime page smoke rendering.

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

---

## Client Boundary Errors

The most common build-time errors involve the server/client boundary.

### `RUV1007` — Server-only in client

```ts
// lib/db.ts
import 'server-only'
export const db = createClient(process.env.DATABASE_URL)
```

If `page.tsx` (hydrated in the browser) imports `lib/db.ts` either directly or transitively, Ruvyxa
reports `RUV1007`.

**Fix:** Move the database call into `server.ts` and pass data as props to the page.

### `RUV1008` — Private env in client

```tsx
// page.tsx
const apiKey = process.env.API_SECRET // RUV1008
```

**Fix:** Read the variable in a loader (`server.ts`) and pass only the result to the page.

### `RUV1010` — Server directory in client

Files under `server/` are reserved for server-only code. A page importing from `server/utils.ts`
triggers `RUV1010`.

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

In development mode, Ruvyxa displays errors in a source-aware modal overlay:

- **Error navigation and close control** matching the browser runtime-error workflow
- **Diagnostic code, title, file, line, and column** at the top of the modal
- **Dark source frame** with surrounding lines and an exact error marker
- **Suggested fix, import chain, and affected routes** when available
- **Collapsible stack trace** for runtime failures
- **Responsive layout and escaped diagnostic content** for safe display on desktop and mobile

The overlay is triggered automatically for any `RuvyxaError::Diagnostic` returned during SSR
rendering, API execution, or client bundle generation. Production mode (`ruvyxa start`) always
returns a plain text error page.

---

## Security Headers

All runtime responses include:

| Header                       | Value                                      |
| ---------------------------- | ------------------------------------------ |
| `X-Content-Type-Options`     | `nosniff`                                  |
| `Referrer-Policy`            | `strict-origin-when-cross-origin`          |
| `Permissions-Policy`         | `camera=(), microphone=(), geolocation=()` |
| `Cross-Origin-Opener-Policy` | `same-origin`                              |

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
