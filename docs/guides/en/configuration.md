# Configuration Reference

Use `config()` so TypeScript validates the public configuration shape:

```ts
import { config } from 'ruvyxa/config'

export default config({
  appDir: 'app',
  outDir: '.ruvyxa',
  css: { entries: ['styles/theme.css'] },
  server: { host: 'localhost', port: 3000 },
  build: {
    minify: true,
    map: false,
    treeShake: true,
    split: 'route',
    workers: 4,
    jsx: 'automatic',
    target: 'es2022',
    manifest: false,
    warm: true,
    prerenderCache: true,
  },
  plugins: [],
  middleware: {
    builtin: { log: true, rate: true, cors: false, headers: {} },
  },
  render: { strategy: 'ssr', revalidate: 60 },
  cache: { routes: true, css: true, dir: '.ruvyxa/cache/bundler' },
  debug: { overlay: true, traces: false },
  image: { optimize: true, quality: 82, lossless: false, workers: 0 },
  security: {
    actionLimit: 1024 * 1024,
    apiLimit: 10 * 1024 * 1024,
    pluginLimit: 32 * 1024 * 1024,
    actionRateLimit: { max: 600, window: 60 },
    sameOrigin: true,
    fetchMeta: true,
    trustedProxyIps: ['10.0.0.2'],
    headers: true,
  },
})
```

Unknown configuration keys intentionally fail rather than being ignored — this prevents typos from
silently changing deployment behaviour.

---

## Reference by Section

### `appDir`

| Property       | Value                                                                            |
| -------------- | -------------------------------------------------------------------------------- |
| **Type**       | `string`                                                                         |
| **Default**    | `"app"`                                                                          |
| **Constraint** | Must be a project-relative path. Absolute paths and `..` traversal are rejected. |

### `outDir`

| Property       | Value             |
| -------------- | ----------------- |
| **Type**       | `string`          |
| **Default**    | `".ruvyxa"`       |
| **Constraint** | Same as `appDir`. |

### `css`

| Field     | Type       | Default | Description                                     |
| --------- | ---------- | ------- | ----------------------------------------------- |
| `entries` | `string[]` | `[]`    | Global CSS files/dirs not imported by app code. |

### `server`

| Field  | Type     | Default       |
| ------ | -------- | ------------- |
| `host` | `string` | `"localhost"` |
| `port` | `number` | `3000`        |

### `build`

| Field            | Type      | Default          | Options                                                                                                                                                                                        |
| ---------------- | --------- | ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `minify`         | `boolean` | `true`           | Oxc-powered JavaScript minification                                                                                                                                                            |
| `map`            | `boolean` | `false`          | Emit source maps                                                                                                                                                                               |
| `treeShake`      | `boolean` | `true`           | Linker-aware tree shaking                                                                                                                                                                      |
| `split`          | `string`  | `"route"`        | `"single"`, `"route"` (`"manual"` is an alias for `"single"`)                                                                                                                                  |
| `workers`        | `number`  | CPU count (auto) | Bounded concurrency for route preparation/final emission plus prerendering. Example `workers: 4` is an explicit override; prerendering remains capped to avoid excessive JavaScript processes. |
| `jsx`            | `string`  | `"automatic"`    | JSX runtime mode; use `"classic"` only for code that provides a React global/import                                                                                                            |
| `target`         | `string`  | `"es2022"`       | `"es2018"`, `"es2019"`, `"es2020"`, `"es2022"`, `"esnext"`                                                                                                                                     |
| `manifest`       | `boolean` | `false`          | Emit chunk manifest                                                                                                                                                                            |
| `warm`           | `boolean` | `true`           | Pre-bundle dependencies in dev server (no effect during production build)                                                                                                                      |
| `prerenderCache` | `boolean` | `true`           | Reuse final SSG/ISR/PPR HTML only when config, environment, assets, styles, and every source fingerprint match; disable for intentionally non-deterministic pages.                             |

### `plugins`

For request/response middleware, use `plugin(name, middleware)`. Use `definePlugin({ name, setup })`
when the same plugin also needs build lifecycle hooks.

### `middleware`

#### Builtin Middleware

```ts
middleware: {
  workers: 1,              // TypeScript middleware processes (1-8)
  timeoutMs: 30_000,       // per-hook timeout (1-300,000 ms)
  builtin: {
    timing: true,           // server-timing response headers
    log: true,              // request logging
    rate: {                 // rate limiting
      max: 100,
      window: 60,
      key: 'ip',
    },
    cors: {                 // CORS
      origins: ['https://myapp.com'],
      methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'],
      credentials: true,
      maxAge: 86400,
    },
    headers: {              // custom response headers
      'X-Powered-By': 'Ruvyxa',
    },
  },
}
```

`addMiddleware` accepts `onRequest` and `onResponse` callbacks using Fetch `Request` and `Response`
objects. `resolveId`, `transform`, and `onBuildComplete` are available beside middleware. All hooks
run in registration order through the persistent plugin runtime.

`workers` defaults to one because module state is process-local. `timeoutMs` defaults to 30 seconds;
a timed-out or protocol-corrupt worker is replaced without retrying that hook, while a worker that
exits before responding is restarted and retried once.

### `render`

| Field        | Type     | Default | Description                           |
| ------------ | -------- | ------- | ------------------------------------- |
| `strategy`   | `string` | `"ssr"` | Default rendering strategy            |
| `revalidate` | `number` | —       | Default revalidate interval (seconds) |

### `cache`

| Field    | Type      | Default                   | Description           |
| -------- | --------- | ------------------------- | --------------------- |
| `routes` | `boolean` | `true`                    | Cache route manifest  |
| `css`    | `boolean` | `true`                    | Cache collected CSS   |
| `dir`    | `string`  | `".ruvyxa/cache/bundler"` | Build cache directory |

### `debug`

| Field     | Type      | Default | Description          |
| --------- | --------- | ------- | -------------------- |
| `overlay` | `boolean` | `true`  | Error overlay in dev |
| `traces`  | `boolean` | `false` | Debug trace output   |

### `image`

| Field      | Type      | Default | Description                  |
| ---------- | --------- | ------- | ---------------------------- |
| `optimize` | `boolean` | `true`  | Convert PNG/JPEG to WebP     |
| `quality`  | `number`  | `82`    | WebP quality (1–100)         |
| `lossless` | `boolean` | `false` | Lossless WebP mode           |
| `workers`  | `number`  | `0`     | Thread count (0 = CPU count) |

### `security`

| Field             | Type              | Default                    | Description                                                                |
| ----------------- | ----------------- | -------------------------- | -------------------------------------------------------------------------- |
| `actionLimit`     | `number`          | `1048576` (1 MiB)          | Body size limit for actions                                                |
| `apiLimit`        | `number`          | `10485760` (10 MiB)        | Body size limit for API routes                                             |
| `pluginLimit`     | `number`          | `33554432` (32 MiB)        | Max buffered response for plugin response middleware                       |
| `actionRateLimit` | `{ max, window }` | `{ max: 600, window: 60 }` | Rate limit per client-action per window                                    |
| `sameOrigin`      | `boolean`         | `true`                     | Same-origin validation for actions                                         |
| `fetchMeta`       | `boolean`         | `true`                     | Fetch Metadata protection                                                  |
| `trustedProxyIps` | `string[]`        | `[]`                       | Exact non-loopback proxies trusted for forwarded identity/protocol headers |
| `headers`         | `boolean`         | `true`                     | Security response headers (CSP, etc.)                                      |

Security limits must be positive when set.

Loopback proxies are trusted without configuration. When a reverse proxy runs elsewhere, list its
exact IP in `trustedProxyIps`; private network ranges are not trusted implicitly.

### `adapter`

```ts
import { config } from 'ruvyxa/config'
import { vercelAdapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: vercelAdapter(),
})
```

An adapter's `build()` function is evaluated while configuration is loaded and again after the
production build to materialize its declared artifacts inside `.ruvyxa/`. The result is written as
`adapterArtifacts` in `.ruvyxa/build.json`. Node and Bun adapters create launchers. Cloudflare,
Netlify, and Vercel adapters are hybrid: they emit a static publish directory for pre-rendered pages
and client assets alongside a serverless function that serves SSR and API routes.

The function artifact contains `route-modules.mjs`, a compiled static registry bundle used by the
platform handler. Adapter handlers do not execute copied `.ts`/`.tsx` source files.

Each adapter declares the route kinds and render strategies it can deploy. Routes outside that set
are rejected with `RUV2202`, naming each unsupported route, before the adapter's `build()` runs:

| Adapter                      | Target                    | Deployable routes  |
| ---------------------------- | ------------------------- | ------------------ |
| `@ruvyxa/adapter-node`       | Node launcher             | all                |
| `@ruvyxa/adapter-bun`        | Bun launcher              | all                |
| `@ruvyxa/adapter-vercel`     | Vercel static + function  | all                |
| `@ruvyxa/adapter-netlify`    | Netlify static + function | all                |
| `@ruvyxa/adapter-cloudflare` | Worker + asset binding    | SSR, SSG, CSR, API |
| `@ruvyxa/adapter-static`     | Static files              | SSG, CSR           |

Cloudflare excludes ISR and PPR because a Worker's asset binding is read-only, so there is nowhere
to write a revalidated page. The static adapter has no server at all.

### `runtime`

```ts
export default config({
  runtime: 'bun', // 'node' or 'bun'; omitted means Node, then Bun if Node is unavailable
})
```

`runtime` selects the JavaScript runtime that executes Ruvyxa configuration, SSR, static rendering,
API routes, actions, and plugins. It does not change the Rust HTTP server. When omitted, Ruvyxa
prefers Node and automatically falls back to Bun if Node is unavailable.

Set `RUVYXA_RUNTIME=bun` in the app command when Bun must be used from the first configuration load,
for example `RUVYXA_RUNTIME=bun bunx ruvyxa dev`. This bootstrap override takes precedence over
`runtime` and is useful in CI.

For backward compatibility, `runtime: 'edge'` and `runtime: 'static'` remain build-target aliases
and execute JavaScript with Node. New deployment builds should use `ruvyxa build --target edge` or
`ruvyxa build --target static` instead.
