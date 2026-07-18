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

| Field       | Type      | Default          | Options                                                                                                                                                                                        |
| ----------- | --------- | ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `minify`    | `boolean` | `true`           | Oxc-powered JavaScript minification                                                                                                                                                            |
| `map`       | `boolean` | `false`          | Emit source maps                                                                                                                                                                               |
| `treeShake` | `boolean` | `true`           | Linker-aware tree shaking                                                                                                                                                                      |
| `split`     | `string`  | `"route"`        | `"single"`, `"route"`, `"manual"`                                                                                                                                                              |
| `workers`   | `number`  | CPU count (auto) | Bounded concurrency for initial and shared-route bundle passes plus prerendering. Example `workers: 4` is an explicit override; prerendering remains capped to avoid excessive Node processes. |
| `jsx`       | `string`  | `"automatic"`    | JSX runtime mode; use `"classic"` only for code that provides a React global/import                                                                                                            |
| `target`    | `string`  | `"es2022"`       | `"es2018"`, `"es2019"`, `"es2020"`, `"es2022"`, `"esnext"`                                                                                                                                     |
| `manifest`  | `boolean` | `false`          | Emit chunk manifest                                                                                                                                                                            |
| `warm`      | `boolean` | `true`           | Pre-bundle dependencies                                                                                                                                                                        |

### `plugins`

Build-time plugins for resolve and transform hooks:

```ts
plugins: [
  {
    name: 'my-plugin',
    enforce: 'pre', // optional: 'pre' | 'post'
    resolveId: true, // intercept module resolution
    transform: true, // transform source code
  },
]
```

### `middleware`

#### Builtin Middleware

```ts
middleware: {
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

#### Wasm Plugins

```ts
middleware: {
  plugins: [
    {
      name: 'auth-guard',
      path: 'plugins/auth-guard.wasm',
      phase: 'request',             // 'request' | 'response'
      routes: ['/api/*'],           // scope to specific routes
      config: { apiKeyHeader: 'X-Api-Key' },
      allow: {
        env: ['AUTH_SECRET'],       // explicit environment access
        timeout: 5000,              // fuel-based execution timeout (ms)
        memory: 67108864,           // 64 MiB memory limit
      },
    },
  ],
}
```

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
| `pluginLimit`     | `number`          | `33554432` (32 MiB)        | Max buffered response for Wasm plugins                                     |
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
import { adapter } from '@ruvyxa/adapter-vercel'

export default config({
  adapter: adapter(),
  adapterOptions: { regions: ['iad1'] },
})
```

An adapter's `build()` function is executed while Ruvyxa loads configuration. Output and
`adapterOptions` are written to `.ruvyxa/build.json`. An adapter declaration alone does not create
or publish platform functions — verify platform output and routing yourself.

### `runtime`

```ts
export default config({
  runtime: 'node', // 'node', 'edge', or 'static'
})
```

Override via CLI: `npx ruvyxa build --target edge`
