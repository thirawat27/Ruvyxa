# Observability and performance

> **Tutorial goal:** observe a real request before tuning its rendering or cache behavior. **Start
> from:** the security baseline in [Security](13-security.md). **Checkpoint:** capture a trace or
> metric signal, then change only the bottleneck it identifies.

## Observability

Use the first-party `observability()` plugin to add a request identifier, W3C `traceparent`,
`Server-Timing`, and a structured record per response. Default request-id header is `x-request-id`;
trace context, server timing, and logging default to enabled. You can scope it to
exact/trailing-star routes and provide a custom logger.

```ts
import { config } from 'ruvyxa/config'
import { observability } from 'ruvyxa/plugins'

export default config({
  plugins: [
    observability({ routes: ['/api/*'], logger: (entry) => console.info(JSON.stringify(entry)) }),
  ],
})
```

The record has `requestId`, `traceparent`, `method`, `pathname`, `status`, and `durationMs`. A
failed logger is isolated so it cannot turn a valid response into an HTTP failure. Treat this as a
foundation for your telemetry sink, not a complete metrics/tracing backend. In a generated
application, `npm run analyze:html` provides a local build/route analysis page; `npm run trace -- /`
inspects a route manifest entry.

For correlated development traces, enable `debug.traces` and run `ruvyxa dev`. The existing
`/__ruvyxa/trace?path=/docs` response continues to inspect one route. Use
`/__ruvyxa/trace?kind=edits` to inspect the bounded process-local edit history, or add `path=` to
filter it by a project-relative changed file. Each edit carries one `traceId` across graph
classification, cache invalidation, worker invalidation/replacement, HMR broadcast, and browser
receipt. The browser acknowledgement is same-origin, size-bounded, and available only while both
watch mode and `debug.traces` are enabled. Trace responses are `no-store`; they are diagnostics, not
a durable telemetry backend.

## `instrumentation.ts`

A file named `instrumentation.ts` (or `.js`/`.mjs`) at the project root is run once per server
process, before the first request is served. It is where a process-wide observability SDK is
installed — the `observability()` plugin above shapes individual responses, while this runs the
setup an SDK needs before anything is shaped.

```ts
// instrumentation.ts
export async function register(): Promise<void> {
  const { NodeSDK } = await import('@opentelemetry/sdk-node')
  new NodeSDK({ serviceName: 'my-app' }).start()
}
```

Only an exported `register` is called. It runs:

- in the render worker under `ruvyxa dev` and `ruvyxa start`, once per worker process;
- in each function instance after a deploy, as a top-level `await` in the generated route registry,
  before any route module is used.

That placement is the point. Telemetry has to be installed in the process that actually renders, so
running it in the CLI that spawns workers would instrument the wrong process.

Failures are logged and swallowed, and a file exporting no `register` is called out on stderr rather
than ignored. Both are deliberate: telemetry exists to observe a working site, so a misconfigured
exporter must not be the reason the site stops serving — but a hook that silently does nothing looks
exactly like a hook that works.

`register` is `await`ed, so a request is never served before it has finished. Keep it quick; the
first request pays for it.

Inside `register()`, write to `console.error` rather than `console.log`. In the Node worker,
standard output is the NDJSON channel the worker uses to answer requests; a line written to it from
anywhere else corrupts the response a request is waiting on.

## Performance controls

- Select the route strategy intentionally: SSR for request-fresh HTML; SSG for immutable build
  output; ISR for time-bounded freshness; CSR for browser-only UI; PPR for a static shell with
  streamed dynamic sections.
- Use `cache(key).ttl(...).swr(...)` for bounded process-local data reuse and invalidate after
  writes. It has no cross-process coherence.
- Prefer `build.split: 'route'` when route-level code splitting is desired; measure before forcing
  `single` or `manual`.
- Build controls include `minify`, `treeShake`, `map`, `workers`, `warm`, and `prerenderCache`.
  Image controls include quality, lossless mode, variants, worker count, and on-demand transforms.
- `minify` drops ordinary and JSDoc comments but keeps legal ones — anything opening `/*!` or `//!`,
  or containing `@license` or `@preserve` — and collects them at the end of each bundle. Dependency
  licence notices therefore ship with the code that needs them.
- The worker runtime has request coalescing and operational environment controls. Start with
  defaults, then use load tests and memory/latency data before changing pool size, concurrency,
  queue capacity, timeout, or memory limit. `RUVYXA_WORKER_MAX_CONCURRENCY` bounds active work per
  process and `RUVYXA_WORKER_MAX_QUEUE` bounds waiting work; overload is rejected as `RUV1705`
  instead of retaining requests without limit.

For framework diagnostics, the internal worker `ping` snapshot reports active and queued requests,
their configured limits, cumulative rejections, retained module URLs, and cache sizes. A queue that
stays non-zero or a rising rejection count indicates saturation. Measure CPU, heap, tail latency,
and rejection rate together before raising a bound: a larger queue absorbs a longer burst but also
retains request bodies and increases wait time.

The same snapshot includes `cacheBudget` and `compilerCache`. `cacheBudget` reports hard, soft, and
hysteresis-target bytes, current heap pressure, pressure events, and eviction counters by owner.
`RUVYXA_MEMORY_LIMIT_MB` sets the worker hard limit (512 MiB by default). Soft pressure evicts
unpinned LRU bundle entries and clears derived module/compiler memory; hard pressure additionally
stops speculative warmups. Keys with active build locks are not eviction candidates.

## Cache and concurrency cautions

The core cache prevents unbounded growth at 1024 entries and can serve stale values while one
background refresh runs. A stale producer error keeps stale data when present; a cold failure still
throws. Plugin middleware workers do not share module state. Realtime reconnect behavior is
client-side and a serverless adapter cannot host native WebSocket realtime. These constraints matter
when scaling past one process.

**Previous:** [Security](13-security.md) · **Next:**
[Deploy, run, and operate in production](15-deploy-run-and-operate.md)
