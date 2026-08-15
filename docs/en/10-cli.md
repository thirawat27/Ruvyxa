# CLI and application scripts

> **Tutorial goal:** use the CLI as a feedback loop—from local development through release checks.
> **Start from:** an app with at least one route;
> [Create your first app](02-create-your-first-app.md) provides one. **Checkpoint:** inspect the
> route list, application check, and analyzer output for your app.

The root [README](../../README.md) is the authoritative project overview. In a generated Ruvyxa
application, use the npm scripts below. They are the stable, copy-pasteable interface provided by
every starter; in particular, use `routes:json` and `analyze:html` rather than teaching readers to
reconstruct the flags behind those scripts.

| Application command                                                                                                                   | Runs                                  | Purpose                                                                                |
| ------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- | -------------------------------------------------------------------------------------- |
| `npm run dev`                                                                                                                         | `ruvyxa dev`                          | Route watching and hot reload.                                                         |
| `npm run build`                                                                                                                       | `ruvyxa build`                        | Production output.                                                                     |
| `npm run check`                                                                                                                       | `ruvyxa check`                        | Application readiness checks.                                                          |
| `npm run start` / `npm run preview`                                                                                                   | `ruvyxa start` / `preview`            | Serve or locally preview an existing build.                                            |
| `npm run routes`                                                                                                                      | `ruvyxa routes`                       | Human-readable route table.                                                            |
| `npm run routes:json`                                                                                                                 | Starter-defined route JSON command    | Machine-readable route output.                                                         |
| `npm run analyze`                                                                                                                     | `ruvyxa analyze`                      | Validate routes, imports, and server/client boundaries.                                |
| `npm run analyze:html`                                                                                                                | Starter-defined HTML analysis command | Interactive local analysis page.                                                       |
| `npm run adds -- form`                                                                                                                | `ruvyxa adds form`                    | Scaffold a supported application flow.                                                 |
| `npm run doctor`, `npm run clean`, `npm run trace -- /`, `npm run bench`, `npm run test:parity`, `npm run plugin -- create my-plugin` | Matching `ruvyxa` command             | Diagnose, clean output, inspect a route, benchmark, verify parity, or create a plugin. |

## Select a JavaScript runtime

Project commands that execute JavaScript accept `--runtime node|bun|deno`; for example,
`npm run build -- --runtime deno`. The flag overrides `RUVYXA_RUNTIME` and `runtime` in
`ruvyxa.config.ts`. See [Configuration](07-configuration.md#runtime-selection) for the fallback
order and Deno permission model.

## Scaffold a starter feature with `adds`

`adds` accepts one or more of exactly `form`, `data-table`, and `auth`. It writes below the
configured `appDir` (normally `app/`), not beside `package.json`. Use the generated `adds` npm
script: its plural name distinguishes this scaffold from package-install commands such as `npm add`.

```bash
npm run adds -- form
npm run adds -- data-table
npm run adds -- auth

# Add independent examples in one operation.
npm run adds -- form data-table auth
```

| Scaffold     | Created files                                                                         | What it demonstrates                                                                                           | What you must supply before production                                                                                               |
| ------------ | ------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `form`       | `app/form-example/page.tsx`, `app/form-example/action.ts`                             | A native POST form, server-side email/message validation, an action handler, and `invalidate('contacts')`.     | Replace the example action with your persistence, authorization, anti-abuse controls, and success/error UX.                          |
| `data-table` | `app/_components/ruvyxa/data-table.tsx`                                               | A generic client component with text filtering, click-to-sort columns, a row key, and optional cell renderers. | Provide real rows and columns; add pagination, server filtering, authorization, and mutations if your app needs them.                |
| `auth`       | `app/_server/auth.ts`, `app/__ruvyxa/auth/[...path]/route.ts`, `app/sign-in/page.tsx` | Credentials sign-in UI, GET/POST auth route, and a development-only in-memory auth/rate-limit store.           | Install `@ruvyxa/auth`, register `auth.plugin`, set the required environment values, and replace demo credentials and memory stores. |

### Form: what the generated action accepts

The form posts to `submitContact`. Its server parser lowercases and validates `email`, requires a
`message` of 10–2,000 characters, then invalidates the `contacts` cache key. Browser attributes such
as `required`, `minLength`, and `maxLength` improve the immediate experience, but the action parser
is the authoritative validation because requests can bypass HTML controls.

```tsx
// app/form-example/action.ts — replace the demonstration handler's body
.handler(async ({ input, invalidate }) => {
  await contacts.insert(input) // your server-side persistence and authorization
  invalidate('contacts')
  return { accepted: true, email: input.email }
})
```

### Data table: use the generated generic component

The scaffold creates a component only; it does not create a route or fetch data. Import it from a
page and pass typed rows and columns. Sorting is client-side and compares displayed values, so use a
server query rather than this component alone for a large dataset.

```tsx
// app/users/page.tsx
import { DataTable, type DataColumn } from '../_components/ruvyxa/data-table'

type User = { id: string; name: string; role: 'admin' | 'member' }
const columns: readonly DataColumn<User>[] = [
  { key: 'name', label: 'Name' },
  { key: 'role', label: 'Role', render: (role) => <strong>{role}</strong> },
]

export default function UsersPage() {
  const rows: User[] = [{ id: 'u1', name: 'Ari', role: 'admin' }]
  return <DataTable rows={rows} columns={columns} rowKey="id" />
}
```

### Auth: make the scaffold safe to run

After adding auth, install its package and register the generated runtime with your configuration.
The scaffold itself does **not** install a package or edit `ruvyxa.config.ts`.

```bash
npm install @ruvyxa/auth
```

```ts
// ruvyxa.config.ts
import { config } from 'ruvyxa/config'
import { auth } from './app/_server/auth'

export default config({ plugins: [auth.plugin] })
```

```dotenv
# .env — never commit these values
RUVYXA_AUTH_SECRET=replace-with-a-secret-of-at-least-32-characters
RUVYXA_AUTH_ORIGIN=https://app.example.com
RUVYXA_DEMO_USER=demo@example.com
RUVYXA_DEMO_PASSWORD=replace-this-demo-password
```

The generated credentials provider accepts only the email/password values above. It is a runnable
demonstration, not a user database or password-hashing system. Before a production build, replace
the development-only memory auth and rate-limit stores with durable atomic implementations;
otherwise the auth package fails closed with `RUV3105`.

### Conflicts and `--force`

Before writing, the command checks every target file. If any exists, it stops with `RUV2401` and
does not write the scaffold set. Review the listed paths, preserve user-owned changes, and use force
only for files you intentionally want to regenerate:

```bash
npm run adds -- form --force
```

## API-only builds with `build --server-only`

`ruvyxa build --server-only` produces an API-only artifact. It runs configuration loading, route
discovery, validation, plugin build hooks, server staging, and the deploy adapter exactly as a
normal build does, and it skips the work that only a rendered HTML page consumes:

| Produced                                          | Skipped                                          |
| ------------------------------------------------- | ------------------------------------------------ |
| `server/` (app, components, and server sources)   | `client/` — route bundles, `route-manifest.json` |
| `assets/` — every file from `public/`, unmodified | WebP conversion and responsive image variants    |
| `manifest.json`, `build.json`                     | `prerender/` — SSG, ISR, and PPR output          |
| `deploy/` from the selected adapter               | `robots.txt` and `sitemap.xml`                   |
|                                                   | page CSS collection                              |

Security headers, API and action body limits, action rate limiting, middleware, and diagnostics are
unchanged: they belong to the server, which this mode still builds.

Two rules are enforced before any output is staged, so a rejected build leaves the previous `dist/`
untouched:

- **`RUV1211`** — the mode supports the `node` and `bun` targets only. `static` has no server, and
  the edge adapters have no server-only output contract yet.
- **`RUV1210`** — a project containing any page route fails, naming the first offending path. A page
  route in a server-only artifact would deploy successfully and then return 404, so this is a
  build-time error rather than a silent omission.

```bash
ruvyxa build --server-only
ruvyxa build --server-only --target bun --adapter node
```

`build.json` records `"serverOnly": true` and sets `"clientDir": null`. Rebuilding an existing
full-build output with `--server-only` removes the now-stale `client/`, `prerender/`, and discovery
files, because the atomic commit replaces the complete set of named build outputs.

The flag is opt-in. `ruvyxa build` without it is unchanged.

## Reproducible production-build baseline

Use the baseline mode before and after changing compiler, cache, chunking, HMR, or adapter behavior:

```bash
npm run bench -- --baseline --samples 3 --json
```

Every sample runs in a disposable project copy under `.ruvyxa/bench/` with its own cache. It
measures seven scenarios in dependency order: cold build, warm build, first production route, CSS
edit, client-boundary edit, server-route edit, and a leaf-route edit. All mutations are syntax-safe
and remain inside the copy. The real project source and build cache are never edited, deleted, or
warmed by this mode. Temporary workspaces are removed after each sample.

The report uses the stable `ruvyxa.build-bench` contract with `schemaVersion: 1` and includes
per-scenario cache observations. It is written only after the cold and warm outputs pass a semantic
artifact-equivalence check. Build timestamps, cache counters, and timing fields are telemetry and
are normalized for that check; deployed code, assets, and manifests are not. The report also records
`peakResidentBytes`, edit files, cache observations, and HMR `reloadFallbacks`; fixture-owned
budgets reject misleading or unsafe results. Existing consumers of the ordinary `bench --json`
result keep receiving the original array shape because baseline mode is opt-in.

## Recommended application loop

Run this from the root of a generated application, not from this framework monorepo:

```bash
npm run dev
npm run routes
npm run check
npm run build
npm run test:parity
```

Use `npm run routes:json` only when another tool needs structured route data; open the report from
`npm run analyze:html` when investigating bundle, route, import, or boundary findings. `clean`
removes generated Ruvyxa build output, so do not run it against a path containing manually
maintained artifacts.

## Running the framework CLI from this monorepo

This repository root deliberately has workspace scripts such as `pnpm build`, `pnpm check`, and
`pnpm test`, but it does **not** define application scripts such as `npm run dev` or
`npm run routes`. To exercise the broad fixture from the repository root, invoke the CLI through
Cargo and name the fixture explicitly:

```bash
cargo run -p ruvyxa_cli -- dev --root examples/demo
cargo run -p ruvyxa_cli -- routes --root examples/demo
cargo run -p ruvyxa_cli -- check --root examples/demo
```

Run `cargo run -p ruvyxa_cli -- <command> --help` when maintaining the framework itself. The checked
CLI exposes `dev`, `build`, `check`, `start`, `preview`, `routes`, `analyze`, `adds`, `doctor`,
`clean`, `trace`, `bench`, `test:parity`, and `plugin create`.

## Repository scripts

The root `package.json` defines `build`, `check`, `test`, `prepare`, `check:cargo-lock`,
`check:oxc-lockstep`, `check:unused`, `check:template-mirrors`, `format`, `format:check`,
`format:staged`, `release:validate`, `release:bump`, `pack:smoke`, `test:full-flow`, and
`publish:dry-run`. `check:unused` runs [Knip](https://knip.dev) across the JavaScript/TypeScript
workspaces and fails on unused files, exports, types, and dependencies; `release:validate` runs it
too. Ruvyxa loads a lot of code by convention — `app/` routes, `plugins/`, `ruvyxa.config.ts`,
runtime files the native CLI resolves by path — so `knip.json` declares those as entry points rather
than treating every one as unused. Published TypeScript packages consistently define `build`,
`check`, `test`, `format`, and `prepack`; consult the relevant package manifest for its test glob.

**Previous:** [Integrations](09-integrations-auth-data-and-realtime.md) · **Next:**
[Architecture](11-architecture.md)
