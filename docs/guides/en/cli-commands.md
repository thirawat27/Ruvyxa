# CLI Commands

## Quick Reference

| Command                         | Purpose                                                            |
| ------------------------------- | ------------------------------------------------------------------ |
| `npm run dev`                   | Development server with hot reload and route watching              |
| `npm run build`                 | Build the application for production output                        |
| `npm run start`                 | Serve an existing production build                                 |
| `npm run typecheck`             | Run `tsc --noEmit`                                                 |
| `npm run check`                 | Run app-level production readiness checks                          |
| `npx ruvyxa preview`            | Preview an existing production build locally                       |
| `npx ruvyxa routes`             | Print routes and their discovered render strategy                  |
| `npx ruvyxa analyze`            | Validate routes, imports, and server/client boundaries             |
| `npx ruvyxa doctor`             | Inspect project setup, tools, dependencies, and diagnostics        |
| `npx ruvyxa trace /blog/[slug]` | Inspect one route manifest entry                                   |
| `npx ruvyxa bench`              | Benchmark route discovery, analysis, and production build          |
| `npx ruvyxa test:parity`        | Compare development and production routes, then smoke render pages |
| `npx ruvyxa parity`             | Alias for `test:parity`                                            |
| `npx ruvyxa clean`              | Remove generated `.ruvyxa/` output                                 |
| `npx ruvyxa plugin new <name>`  | Create a plugin starter                                            |

## Common Options

| Option      | Commands                  | Description                                   |
| ----------- | ------------------------- | --------------------------------------------- |
| `--root`    | All                       | Project root directory (default: `.`)         |
| `--host`    | `dev`, `start`, `preview` | Bind host (overrides config)                  |
| `--port`    | `dev`, `start`, `preview` | Bind port (overrides config)                  |
| `--target`  | `build`                   | Build target: `node`, `bun`, `edge`, `static` |
| `--samples` | `bench`                   | Number of samples (default: 3)                |
| `--json`    | `bench`                   | Output in JSON format                         |

---

## Command Details

### `dev`

```bash
npx ruvyxa dev
npx ruvyxa dev --root ./my-app
npx ruvyxa dev --host 0.0.0.0 --port 8080
```

Starts the development server with:

- Hot Module Replacement (HMR) via WebSocket
- Automatic file watching and reload
- In-memory render cache (capacity 1024, TTL 5 min)
- Error overlay (`debug.overlay`)

### `build`

```bash
npx ruvyxa build
npx ruvyxa build --target node     # default
npx ruvyxa build --target bun      # bun runtime
npx ruvyxa build --target static   # static output
npx ruvyxa build --target edge     # edge runtime
npx ruvyxa build --adapter vercel  # run a deploy adapter without config changes
npx ruvyxa build --runtime bun     # execute build workers with Bun
```

`--runtime <node|bun>` is accepted by `dev`, `start`, `preview`, `build`, `check`, `routes`,
`analyze`, `doctor`, `clean`, and `test:parity`. It overrides the `RUVYXA_RUNTIME` environment
variable and `config.runtime`, so switching the JavaScript runtime never requires editing config.

Pipeline:

1. Discover routes
2. Validate routes, imports, server/client boundaries
3. Collect CSS styles
4. Optimize images (PNG/JPEG → WebP)
5. Bundle client code (minify, tree-shake, split)
6. Pre-render SSG / ISR / PPR / CSR routes
7. Write output to `.ruvyxa/`

**Output structure:**

```text
.ruvyxa/
├── server/
│   ├── app/         # Production route source (copied from app/)
│   ├── components/  # Copied from project components/
│   └── server/      # Copied from project server/
├── client/         # BLAKE3-hashed client bundles + manifest.json
├── assets/         # Public assets + optimized WebP images
├── prerender/      # Pre-rendered HTML pages + manifest.json
├── manifest.json   # Route manifest (paths, layouts, module references)
└── build.json      # Build metadata, config snapshot, render summary
```

`build.json.timing` records route discovery, validation, preparation, client bundling, prerendering,
and total build durations in milliseconds. Use it with `ruvyxa bench` to identify the stage to
investigate before changing build settings.

The client manifest's `budget` lists the ten largest first-load routes against a 250 KiB observation
budget without failing the build. Each route also exposes `artifactCacheHit` when its
graph-fingerprinted client artifact was reused.

### `check`

```bash
npx ruvyxa check
```

Runs in sequence:

1. TypeScript type checking (`tsc --noEmit`; skipped if no `tsconfig.json`)
2. Parity check: builds production output, compares dev/prod routes, smoke-renders every page

Use this as a deploy readiness signal.

### `start` / `preview`

```bash
npx ruvyxa start
npx ruvyxa preview      # alias
```

Serves the production build with the same runtime semantics as `dev`. Use after `npm run build` to
inspect production output locally.

### `routes`

```bash
npx ruvyxa routes
```

Prints the discovered route table with detected rendering strategies:

```text
Route                    Strategy
/                        ssg
/about                   ssg
/blog/[slug]             ssr
/api/health              api
```

### `analyze`

```bash
npx ruvyxa analyze
```

Validates per-route (page + imports + layouts):

- Missing default export (page.tsx only)
- `"server-only"` imports in client-reachable code
- Private `process.env.*` access in client graph
- `server/` dir imports in client graph
- `"client-only"` imports in server code

Route conflict detection runs during route discovery (before `analyze`). Config validation runs
during config loading (before route discovery).

Run after any change to routes, imports, configuration, or environment usage.

### `doctor`

```bash
npx ruvyxa doctor
```

Reports:

- Ruvyxa CLI version
- Node.js, Rust (`rustc`, `cargo`), Bun, Deno versions
- Package manager detection
- Project structure (package.json, tsconfig.json, config, app dir)
- Dependency status
- Configuration validity
- Route count (total, page, API)

### `trace`

```bash
npx ruvyxa trace /blog/hello-world
npx ruvyxa trace /blog/[slug]
```

Inspects one route manifest entry: source file, matching pattern, rendering strategy, layout
nesting, and module dependencies.

### `bench`

```bash
npx ruvyxa bench
npx ruvyxa bench --samples 5
npx ruvyxa bench --json
```

Benchmarks the full pipeline: route discovery, analysis, validation, and production build.

### `test:parity` / `parity`

```bash
npx ruvyxa test:parity
npx ruvyxa parity           # alias
```

Compares development and production routes, then smoke-renders every page.

### `clean`

```bash
npx ruvyxa clean
```

Removes the `.ruvyxa/` output directory entirely.

### `plugin new`

```bash
npx ruvyxa plugin new request-logger
```

`plugin new` creates a publishable package under `plugins/request-logger/` with `src/index.ts`,
`package.json`, `tsconfig.json`, and `README.md`. Build it with `pnpm build`, then publish it with
`pnpm publish`. Import the package entry from `ruvyxa.config.ts` and register it in
`config({ plugins: [...] })`. See [Plugins](plugins.md) for the complete workflow.

---

## Recommended Diagnostic Flow

When making risky changes:

1. `npx ruvyxa analyze` — validate routes, imports, boundaries
2. `npm run typecheck` — TypeScript verification
3. `npm run check` — full readiness signal (before deploy)
4. `npm run build && npm run start` — inspect production output locally

## Relevant Environment Variables

| Variable                   | Purpose                                                                           | Default                 |
| -------------------------- | --------------------------------------------------------------------------------- | ----------------------- |
| `RUVYXA_RENDER_CACHE_SIZE` | Render-cache capacity; `0` disables it and values above 16,384 clamp.             | 1024 (dev), 512 (prod)  |
| `RUVYXA_BUILD_CACHE_DIR`   | Override build cache directory                                                    | `.ruvyxa/cache/bundler` |
| `RUVYXA_WORKER_TIMEOUT_MS` | Rust/Node worker request and API stream-idle timeout; accepts 1–2,147,483,647 ms. | 30,000 ms               |
| `RUVYXA_MEMORY_LIMIT_MB`   | Node-worker cache-pressure threshold; invalid or zero values use default.         | 512 MiB                 |
| `RUVYXA_CACHE_MAX_ENTRIES` | Per-worker compiled-bundle and module-cache capacity.                             | 256                     |
