# Performance

Ruvyxa is built for speed at every layer. The CLI is compiled Rust, route discovery uses `walkdir`
without spawning child processes, and production builds emit route-level bundles with tree-shaking
and content-addressed hashing.

---

## Benchmarking

Measure framework hot paths with the built-in benchmark command:

```bash
ruvyxa bench --root .
```

### What it measures

| Benchmark            | What's timed                                                       |
| -------------------- | ------------------------------------------------------------------ |
| `route-discovery`    | Walking `app/` and building the route manifest                     |
| `analyze-validation` | Route discovery + full server/client boundary validation           |
| `production-build`   | Complete `.ruvyxa/` output: server copy, client bundles, manifests |

### Options

```bash
ruvyxa bench --samples 5          # Run each benchmark 5 times (default: 3)
ruvyxa bench --samples 5 --json   # JSON output for CI integration
```

### Example output

```
route-discovery     avg 2.1ms   min 1.8ms   max 2.4ms   (3 samples)
analyze-validation  avg 8.3ms   min 7.9ms   max 8.8ms   (3 samples)
production-build    avg 142ms   min 138ms   max 147ms   (3 samples)
```

---

## Build Optimization

### Route-Level Code Splitting

Client bundles are split per route. Each page gets its own hydration bundle:

```
.ruvyxa/client/
├── 1814aa4c9e53cc6e.js   ← /
├── 1c189acd2b180745.js   ← /about
├── 2c080f001adec62c.js   ← /blog/:slug
├── fba6658aa08c2ee6.js   ← /todos
└── manifest.json
```

### Tree Shaking

Dead-code elimination per route bundle, enabled by default. Only imports used by the page are
included. Disable with `build.treeShaking: false` when debugging.

### Minification

Production bundles are minified with whitespace removal, identifier shortening, and dead-code
elimination.

### Content Hashing

File names are BLAKE3 hashes (first 16 hex characters). Enables:

- Immutable caching (`Cache-Control: public, max-age=31536000, immutable`)
- Automatic cache-busting on content change
- Deterministic builds — same input always produces the same hash

### Build Stats

`client/manifest.json` records per-route metrics:

- `moduleCount`, `outputBytes`, `estimatedGzBytes`
- `durationMs`, `cacheHits`, `treeShakenModules`
- `cache.compileEntries`, `cache.compileBytes`

Set `build.emitChunkManifest: true` to also write `.ruvyxa/client/chunk-manifest.json` with dynamic
import split points for deployment adapters.

When route splitting identifies modules shared by multiple pages, each route entry also records a
`sharedChunks` list. Production responses and pre-rendered HTML emit route-scoped
`<link rel="modulepreload">` hints before loading the hashed hydration bundle.

### Parallel Production Bundling

Client bundles are emitted concurrently using `std::thread::scope` with scoped threads. The
parallelism level defaults to `available_parallelism()` and is configurable via `build.parallelism`.
Bundles are written in deterministic route order.

### Build Plugin Worker

JavaScript `resolveId` and `transform` config hooks share one persistent Node process per build
context. The newline-delimited JSON protocol keeps plugin module state alive and avoids paying Node
startup and config compilation cost for every module. A transform result may include a Source Map v3
object or JSON string in `map`; valid mappings are forwarded into the emitted bundle source map.

---

## Runtime Performance

### Dev Server

- Route discovery runs in Rust, not JavaScript.
- File watching uses native OS notifications (`notify` crate).
- HMR events classified by type (CSS update, component/file update, full reload).
- **Render cache**: default capacity 1024 (dev) / 512 (prod), TTL 5 min (dev) / 30 min (prod).
  Configurable via `RUVYXA_RENDER_CACHE_SIZE` env var.
- **Worker pool**: auto-sizes to `available_parallelism()` (clamped 2–8). Configurable via
  `RUVYXA_WORKER_POOL_SIZE` env var. Default 10s timeout for dead-worker detection.
- **Async file I/O**: hot-path file reads use `tokio::task::spawn_blocking`.

### Production Server

- Static assets and client bundles served with immutable cache headers.
- Route matching reuses the same algorithm as dev — no production-only code paths.
- Security headers applied once per response.

---

## Monitoring in CI

```bash
ruvyxa bench --samples 5 --json > bench-results.json
```

CI integration:

```yaml
- name: Performance benchmark
  run: |
    npx ruvyxa bench --samples 5 --json > bench.json
```

---

## Tips

- Run benchmarks before and after changes to route discovery, build, or HMR logic.
- Use `ruvyxa clean` before benchmarking for cold-start measurements.
- The `route-discovery` benchmark is the best indicator of CLI startup latency.
- For app-level performance (response times, TTFB), use standard HTTP benchmarking tools against
  `ruvyxa start`.

---

## Related

- [Debugging](debugging.md) — the `bench` command and diagnostics
- [Production Readiness](production-readiness.md) — release performance baselines
- [Dev/Prod Parity](parity.md) — ensuring both modes behave identically
