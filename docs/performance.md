# Performance

Ruvyxa is built for speed at every layer. The CLI is compiled Rust, route discovery uses `walkdir` without spawning child processes, and production builds emit route-level bundles with tree-shaking and content-addressed hashing.

---

## Benchmarking

Measure framework hot paths with the built-in benchmark command:

```bash
ruvyxa bench --root .
```

### What it measures

| Benchmark | What's timed |
|-----------|-------------|
| `route-discovery` | Walking `app/` and building the route manifest |
| `analyze-validation` | Route discovery + full server/client boundary validation |
| `production-build` | Complete `.ruvyxa` output: server copy, client bundles, manifests |

### Options

```bash
ruvyxa bench --samples 5          # Run each benchmark 5 times (default: 3)
ruvyxa bench --samples 5 --json   # Output JSON for CI integration
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

Client bundles are split per route, not per app. Each page gets its own hydration bundle containing only the code that page needs.

```
.ruvyxa/client/
├── 1814aa4c9e53cc6e.js   ← /
├── 1c189acd2b180745.js   ← /about
├── 2c080f001adec62c.js   ← /blog/:slug
├── fba6658aa08c2ee6.js   ← /todos
└── manifest.json
```

### Tree Shaking

The Ruvyxa bundler eliminates dead code from each route bundle. Only imports actually used by the page are included in the output.
This is enabled by default and can be disabled with `build.treeShaking: false`
when debugging optimizer behavior.

### Minification

Production bundles are minified by the Ruvyxa minifier with whitespace removal, identifier shortening, and dead-code elimination.

### Content Hashing

File names are BLAKE3 hashes of their content (first 16 hex characters). This enables:
- Immutable caching (`Cache-Control: public, max-age=31536000, immutable`)
- Automatic cache-busting on content change
- Deterministic builds — same input always produces the same hash

### Build Stats

`client/manifest.json` records per-route and aggregate build metrics:

- `moduleCount`
- `outputBytes`
- `estimatedGzBytes`
- `durationMs`
- `cacheHits`
- `treeShakenModules`
- `cache.compileEntries`
- `cache.compileBytes`

Set `build.emitChunkManifest: true` to also write
`.ruvyxa/client/chunk-manifest.json` for deployment adapters and performance
tooling.

---

## Runtime Performance

### Dev Server

- Route discovery runs in Rust, not JavaScript.
- File watching uses native OS notifications (`notify` crate).
- HMR events are classified by type (CSS update, component update, full reload) to minimize browser work.
- **Render cache**: FIFO eviction via `VecDeque`, capacity 1024, TTL 5 min dev / 30 min prod. Configurable via `RUVYXA_RENDER_CACHE_SIZE`.
- **Worker pool**: Auto-sizes to `available_parallelism()` (clamped 2–8). Configurable via `RUVYXA_WORKER_POOL_SIZE`. Timeout is 10s for fast dead-worker detection.
- **Async file I/O**: SSR hot-path file reads use `tokio::task::spawn_blocking` to avoid blocking the async runtime.

### Production Server

- Static assets and client bundles are served with immutable cache headers.
- Route matching reuses the same algorithm as dev — no production-only code paths.
- Security headers are applied once per response without middleware chains.

---

## Monitoring in CI

Use JSON output to track performance over time:

```bash
ruvyxa bench --samples 5 --json > bench-results.json
```

Integrate with your CI pipeline to detect regressions:

```yaml
- name: Performance benchmark
  run: |
    npx ruvyxa bench --samples 5 --json > bench.json
    # Compare against baseline, fail on regression
```

---

## Tips

- Run benchmarks before and after changes to route discovery, build, or HMR logic.
- Use `ruvyxa clean` before benchmarking to ensure cold-start measurements.
- The `route-discovery` benchmark is the best indicator of CLI startup latency.
- For app-level performance (response times, TTFB), use standard HTTP benchmarking tools against `ruvyxa start`.

---

## Related

- [Debugging](debugging.md) — the `bench` command and diagnostics
- [Bundler Comparison](bundler-comparison.md) — comparison with Vite, Rollup, webpack, Turbopack, and related bundlers
- [Production Readiness](production-readiness.md) — release performance baselines
- [Dev/Prod Parity](parity.md) — ensuring both modes behave identically
