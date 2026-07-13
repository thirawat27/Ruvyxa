# Configuration

`ruvyxa.config.ts` — ใช้ `config()` เพื่อ TypeScript validation:

```ts
import { config } from 'ruvyxa/config'

export default config({
  appDir: 'app',
  outDir: '.ruvyxa',
  server: { host: 'localhost', port: 3000 },
  build: {
    minify: true,
    split: 'route',
    target: 'es2022',
  },
  render: { strategy: 'ssr', revalidate: 60 },
  cache: { routes: true, css: true },
  debug: { overlay: true, traces: false },
  image: { optimize: true, quality: 82 },
  security: {
    actionLimit: 1024 * 1024,
    apiLimit: 10 * 1024 * 1024,
    pluginLimit: 32 * 1024 * 1024,
    trustedProxyIps: ['10.0.0.2'],
  },
  middleware: {
    builtin: { log: true, rate: true },
  },
})
```

## รายละเอียดแต่ละ Section

### server

`{ host: "localhost", port: 3000 }`

### build

| Field     | Default    | Options                           |
| --------- | ---------- | --------------------------------- |
| `minify`  | `true`     | Oxc minification                  |
| `split`   | `"route"`  | `"single"`, `"route"`, `"manual"` |
| `target`  | `"es2022"` | `es2018`–`esnext`                 |
| `workers` | CPU count  | จำนวน threads (override ได้)      |

### render

| Field        | Default |
| ------------ | ------- |
| `strategy`   | `"ssr"` |
| `revalidate` | —       |

### security

| Field             | Default |
| ----------------- | ------- |
| `actionLimit`     | 1 MiB   |
| `apiLimit`        | 10 MiB  |
| `pluginLimit`     | 32 MiB  |
| `trustedProxyIps` | `[]`    | IP ของ reverse proxy ที่อนุญาตให้ส่ง forwarded headers |

ดูฉบับเต็ม (อังกฤษ): [Configuration (EN)](../en/configuration.md)
