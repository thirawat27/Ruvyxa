# Build performance and MDX architecture audit

## Pass and scope

**Pass:** Full. The production build crosses CLI orchestration, the native resolver/compiler/linker,
the persistent Node compiler workers, prerendering, and the on-disk artifact cache. A narrower pass
would miss repeated work across those boundaries.

**Evidence checked:** workspace manifests and tooling, `ruvyxa_cli/src/main.rs`, bundler context,
resolver/compiler/content/linker paths, the packaged Node compiler and tests, shared-route/cache
tests, EN/TH configuration and content guides, and cold/warm `examples/demo` phase reports.
Generated output, dependency source, unrelated request-time dev-server behavior, deployment
adapters, and binary assets were excluded.

## Confirmed flow

```text
ruvyxa build
  -> discover + validate routes
  -> collect/copy styles, app, server, and assets
  -> prepare each client page with one build-scoped BundleContext
  -> identify shared modules from prepared graphs
  -> emit/cache one shared route registry
  -> emit each route once without duplicated shared modules
  -> prerender static routes through the bounded Node worker pool

page.md/page.mdx
  -> native path: YAML -> markdown-rs mdast -> generated React ESM
  -> Node SSR/SSG path: YAML -> @mdx-js/mdx + remark-gfm -> generated React ESM
  -> normal resolver/compiler/boundary/linker pipeline
```

## Findings and corrections

1. **High · Direct evidence · High confidence — route graphs were transformed and scanned more than
   once.** The Node compiler transformed source during graph discovery and again during rewriting,
   while the native resolver repeatedly walked the same plugin-free dependency closure. Impact: CPU
   and filesystem work grew with routes × shared modules. Correction: a bounded content-keyed Node
   transform cache, reuse of the discovery transform, plugin-free dependency memoization, and a
   production-only immutable source snapshot.
2. **High · Direct evidence · High confidence — native Markdown/MDX could compile twice per route.**
   Resolution compiled content to discover imports and the compiler compiled the same source again
   for output. Impact: large content sites duplicated parsing and code generation. Correction: a
   bounded successful-result cache keyed by extension and source content; errors remain uncached.
3. **High · Direct evidence · High confidence — shared-route output was rebuilt despite available
   prepared graphs and valid warm inputs.** The legacy synthetic entry resolved and compiled shared
   dependencies again. Impact: cold builds discarded prepared compiler work and warm builds still
   linked the registry. Correction: cold builds emit from prepared modules; warm builds load a
   versioned artifact validated against the dependency namespace and complete module fingerprints.
   Plugin builds retain the legacy shared hook pass so invocation behavior does not change.
4. **Medium · Direct evidence · High confidence — prerender setup repeated immutable work per
   route.** Every job re-read and parsed the client manifest and cloned the complete stylesheet.
   Impact: manifest lookup approached O(routes²) and CSS memory copying scaled with concurrent
   routes. Correction: load one route-indexed asset map and share CSS as `Arc<str>`.
5. **High · Direct evidence · High confidence — earlier route output and cache validation scaled
   with overlap.** Route splitting previously emitted a base bundle before the final shared-aware
   bundle, and artifact validation re-hashed common files for every route. Correction: prepare once,
   emit once, persist lightweight route plans, and share one build-scoped fingerprint memo.
6. **High/Medium · Direct evidence · High confidence — MDX parsing and documentation had contract
   gaps.** Line-based ESM extraction, scalar-only frontmatter, and incomplete GFM/table/heading
   rendering did not cover the documented surface. Correction: parser-backed MDX ESM boundaries, Oxc
   syntax feedback, combined GFM+MDX constructs, structured YAML, semantic tables/references, stable
   duplicate heading slugs, JSX member/spread support, and Node/native parity tests.

## Implemented outcome

- Route preparation, shared-registry emission, and final route emission reuse the configured bounded
  worker count while restoring deterministic manifest order. Plugin-free prepared/legacy shared
  output has a byte-equivalence regression; plugin builds keep the legacy shared compilation pass.
- Route plans, final route artifacts, and the shared registry use content validation. Dynamic
  imports participate in invalidation, and a shared-source edit is proven to invalidate both route
  artifacts and the shared chunk.
- The resolver's stable source snapshot is build-scoped. Normal reusable resolver caches retain
  metadata validation, and dependency memoization is disabled when plugins are installed so plugin
  hook behavior is unchanged.
- Node transforms and native content results are bounded to prevent unbounded long-lived worker or
  process memory growth. Cache keys include every transform-affecting input used by those paths.
- Prerender workers share immutable manifest/style state without changing HTML injection, CSR
  fallback, worker limits, or client-manifest schemas.
- Warm prerender startup is now lazy: builds with only valid artifact-cache hits (or CSR-only
  output) do not spawn or health-check Node workers. When rendering is required, worker warmup is
  dispatched concurrently across the pool instead of multiplying startup latency by pool size.
- Project-relative SSG input metadata is resolved against the project root before fingerprinting;
  this keeps the artifact cache portable when the CLI is launched outside the project directory.
- Prerender context hashing ignores shell/tooling session noise (`PATH`, Cargo/Rust, Codex,
  pnpm/npm, and prompt-session variables) while retaining project `.env` and stable application
  environment values. This prevents false cache misses between otherwise identical invocations.
- Native and packaged Node MDX paths support structured frontmatter, GFM, semantic headings/tables,
  and parser-backed ESM while retaining the existing generated-module contract.

## Comparable benchmark

The same debug CLI binary and `examples/demo` fixture (16 routes, 14 pages) used isolated fresh
cache directories. Timings come from `.ruvyxa/build.json`, so Cargo compilation time is excluded.

| Phase              |      Before |             After |     Change |
| ------------------ | ----------: | ----------------: | ---------: |
| Cold total         | 13,609.6 ms |        4,024.1 ms | **-70.4%** |
| Cold client bundle |    808.2 ms |          561.5 ms | **-30.5%** |
| Cold prerender     |  9,994.0 ms |        1,076.0 ms | **-89.2%** |
| Warm total         |  1,944.1 ms | 1,619.7 ms median | **-16.7%** |
| Warm client bundle |    345.5 ms |    23.1 ms median | **-93.3%** |
| Warm prerender     |  1,234.0 ms | 1,252.4 ms median |      +1.5% |

The after warm median is from three consecutive cache-hit builds (1,828.9, 1,619.7, and 1,499.0 ms).
The small warm-prerender movement is treated as worker/OS noise; no prerender speedup is claimed for
that sample. The structural gains matter more on large projects because repeated work was removed
from route/module loops.

### Incremental hardening benchmark (2026-07-19)

On the same `examples/demo` fixture after the plugin/content/style hardening, a clean production
build measured 6076.5 ms total (732.0 ms client, 1359.3 ms prerender). A subsequent build with the
fingerprint-validated prerender artifacts measured 811.8 ms total (22.0–25.3 ms client and 214.7 ms
prerender), with 13 of 14 routes served from the artifact cache; the remaining CSR shell is
generated directly. These timings exclude Cargo compilation and vary with the local OS/Node process
scheduler.

### Reliability/performance follow-up benchmark (2026-07-19)

After correcting project-relative worker input normalization and filtering volatile shell/tooling
environment keys from the prerender context, two consecutive `examples/demo` builds measured 1830.0
ms (cache population) and 443.2 ms (warm). The warm build completed prerendering in 160.1 ms with 13
of 14 routes served from the validated artifact cache; no Node pool was started for those cache
hits. The CSR shell remains direct output by design. These numbers exclude Cargo compilation and are
Windows x64 observations, not a cross-platform guarantee.

## Assumptions, debt, and residual risks

- **Assumption:** project source does not intentionally mutate during one production build. This
  matches the existing staged-output snapshot model.
- **Resolved in this pass:** JS config plugins remain serialized by default, while an explicit
  `parallel: true` capability on every active hook enables a bounded pool of isolated persistent
  Node workers. This keeps stateful-plugin behavior unchanged and makes plugin-heavy builds scale
  without sharing mutable process state.
- **Resolved in this pass:** warm prerender now reuses final HTML artifacts only after validating
  the config/render context, environment, styles, client assets, Node renderer dependency hash, and
  all source fingerprints returned by the worker. `build.prerenderCache: false` is the escape hatch
  for intentionally non-deterministic pages.
- **Resolved in follow-up:** worker startup is deferred until a static-parameter lookup or a real
  artifact miss requires Node; relative worker input paths are rooted at the project; volatile shell
  variables no longer invalidate otherwise identical artifacts.
- **Residual risk:** dynamic data fetched from external systems is not observable by the source
  fingerprint contract; such routes must opt out of `prerenderCache` when that data is part of the
  build result.
- **Residual risk:** bounded FIFO caches can evict hot entries on projects larger than their limits;
  this affects performance only, not correctness.
- **Open questions:** None identified.

## Validation and handoff

- **Claim traceability:** Each finding names its observed boundary; focused regressions cover cache
  bounds, prepared/legacy equivalence, warm reuse, dynamic/shared invalidation, and MDX parity.
- **Scope alignment:** Only production build/content compilation paths and their documentation
  changed. Public CLI, config, manifest, route, plugin, boundary, source-map, and output contracts
  remain intact.
- **Handoff readiness:** Plugin concurrency and incremental prerender invalidation are now explicit
  contracts. Follow-up work should focus on richer external-data dependency declarations rather than
  weakening the safe defaults.
