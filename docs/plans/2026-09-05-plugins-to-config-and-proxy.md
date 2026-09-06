# Plan: retire the plugin system for config-shaped extension points

Status: implemented 2026-09-06 (owner chose full parity with the app-router model, as one breaking
change rather than four phases). Two decisions differ from the table below: the proxy lives inside
`ruvyxa.config.ts` as `proxy: { matcher, handler }` rather than a root `proxy.ts`, and `fonts`,
`config env` validation, and `build.budget` were not carried over — `instrumentation.ts` and
`tsconfig` `paths` cover the first two needs. Shipped surface: `docs/en/08-request-pipeline.md`.

## Why

The app-router model has no plugin system. Its extension surface is three things: configuration keys
on one config object (`headers()`, `redirects()`, `rewrites()`, `images`, `env`, …, composed by
higher-order functions such as `withMDX(config)`), root-level file conventions (`proxy.ts` — the
renamed `middleware.ts` —, `instrumentation.ts`), and files inside `app/` (`route.ts`, `sitemap.ts`,
`robots.ts`, `manifest.ts`). Ruvyxa instead has `plugins: RuvyxaPlugin[]` with
`definePlugin({ http, build, dev, diagnostics, native, head, headers, register(api) })`: seven
sockets, two spellings, about 50 exported types, and roughly 4,700 lines of machinery (`plugin.ts`,
`plugin-registration.ts`, `plugin-harness.ts`, `plugin-http.mjs`, `plugin-registration.mjs`,
`plugin-runtime.mjs`, `plugins.rs`, `plugin_host.rs`, `plugin_bridge.rs`) carrying another ~4,700
lines of first-party plugins, most of which are config-shaped features (`redirects`, `headers`,
`securityHeaders`, `cacheRules`, `sitemap`, `robots`, `pwa`, `fonts`, `alias`, `requireEnv`,
`bundleBudget`, `openApi`, …).

What that costs today, measured:

- Every request matching an `http.onRequest`/`onResponse` scope crosses a process boundary on the
  Axum host: JSON over stdio, body base64 both ways, response buffered up to `security.pluginLimit`,
  30 s hook timeout, 1–8 workers sharing no state.
- Every plugin behaviour exists twice — Axum (`plugin_bridge.rs`) and the deployed handler
  (`serverless-handler.mjs` `pluginHttp`) — and is held together by
  `tests/fixtures/framework-endpoint-conformance.json` and
  `tests/fixtures/plugin-path-scope-conformance.json`.
- `build.onResolve/onLoad/onTransform` run only in the Rust bundler (browser half), so a transform
  that reaches markup produces React #418; `docs/en/08-plugins-middleware.md` spends a section
  warning about it.

## Target shape

| Today                                                                                         | Target                                                                                                                                                                                                                                                                                                                                                   |
| --------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `plugins: []`, `definePlugin`, `ruvyxa/plugin`, `ruvyxa/plugins`, `ruvyxa/plugin-harness`     | removed; `config()` also accepts `(phase, { defaultConfig }) => config \| Promise<config>`; third-party composition is `withX(config)`                                                                                                                                                                                                                   |
| `http.onRequest` / `http.onResponse` / `http.routes` per plugin                               | one root `proxy.ts`: `export function proxy(request)` + `export const config = { matcher }` (path-to-regexp source strings, `has`/`missing`), returns `Response` or `next({ request })`. Matcher compiled to a regex in both hosts so the Axum host crosses the process only for matching paths, and only once                                           |
| `redirects`, `headers`, `securityHeaders`, `cacheRules` plugins; `middleware.builtin.headers` | config keys `redirects()`, `headers()`, `rewrites()` with the conventional entry shapes (`source`, `destination`, `permanent` → 308/307, `has`, `missing`, `basePath`, `locale`); evaluated natively in Rust and in `serverless-handler.mjs`; fixed order `headers → redirects → proxy → rewrites.beforeFiles → files → afterFiles → dynamic → fallback` |
| `sitemap`, `robots`, `feed`, `llmsTxt`, `wellKnown`, `pwa` manifest                           | `app/sitemap.ts`, `app/robots.ts`, `app/manifest.ts`, `app/feed.xml/route.ts`, `app/.well-known/security.txt/route.ts`, `app/llms.txt/route.ts`                                                                                                                                                                                                          |
| `healthCheck`, `openApi`, `webVitals` collector, `originGuard`                                | `app/**/route.ts` handlers (already supported); `webVitals` client half → `instrumentation-client.ts` + a `@ruvyxa/react` hook; `originGuard` → `proxy.ts` recipe or a per-route helper in `ruvyxa/server`                                                                                                                                               |
| `alias`, `requireEnv`, `bundleBudget`                                                         | `tsconfig` `paths` (already honoured), config `env` with required-name validation, config `build.budget`                                                                                                                                                                                                                                                 |
| `fonts`                                                                                       | `ruvyxa/font` module — separate phase, largest new surface                                                                                                                                                                                                                                                                                               |
| `contentEngine`, `searchIndex`                                                                | config `content` (exists; `contentEngineFromConfig` already builds from it)                                                                                                                                                                                                                                                                              |
| `@ruvyxa/auth` plugin (`http.onRequest` on `/__ruvyxa/auth/*`)                                | `app/__ruvyxa/auth/[...path]/route.ts` calling `auth.handle(request)`; `auth.plugin` removed                                                                                                                                                                                                                                                             |
| `@ruvyxa/realtime` `native.claim('realtime@1' \| 'presence@1')`                               | config keys `realtime` and `collab` (`RealtimePluginOptions` / `PresencePluginOptions` shapes unchanged); Axum reads them from the rendered config, not from a plugin descriptor                                                                                                                                                                         |
| `@ruvyxa/database` plugin (`requiredEnv`)                                                     | config `env` validation                                                                                                                                                                                                                                                                                                                                  |
| `build.onResolve/onLoad/onTransform`, `dev.onFileChange`, `head`, `diagnostics`               | removed — the model has no equivalent outside bundler loaders                                                                                                                                                                                                                                                                                            |

Not a plugin, must survive: the plugin worker process also serves `content.compile` (MDX through
unified) and the React compiler (`react_compiler_enabled` in `plugins.rs`). The worker stays as a
build worker; only the plugin hooks leave it.

## Phases

Each phase lands green on the full battery in `AGENTS.md` and is independently revertable.

1. **New surfaces beside the old.** `proxy.ts` discovery (mirror `instrumentation.ts` discovery in
   `crates/ruvyxa_dev_server/src/lib.rs` ~790 and `runtime/compiler.mjs` ~848), a shared
   path-to-regexp subset compiler in `@ruvyxa/core` and Rust with a
   `tests/fixtures/*-conformance.json` replayed by both, `headers()`/`redirects()`/`rewrites()`
   config keys through `config-schema.mjs` → `config-renderer.mjs` →
   `crates/ruvyxa_cli/src/config.rs` (the `middleware` key is the precedent), evaluated in
   `ruvyxa_middleware` and `serverless-handler.mjs`, `config()` accepting a function, metadata file
   conventions in `ruvyxa_graph` discovery. Both hosts covered by
   `framework-endpoint-conformance.json` rows.
2. **Migrate consumers.** First-party plugins one by one into the target column; `examples/demo`
   (`plugins/` directory, `plugin-lab` route), `templates/plugin` (delete), `create-ruvyxa`
   starters, `@ruvyxa/auth|database|realtime`, `docs/en|th/08` rewritten as "Configuration, proxy,
   and file conventions", `docs/en|th/17`, `ARCHITECTURE.md`, `AGENTS.md` (plugin rows).
3. **Delete.** `packages/@ruvyxa/core/src/plugin*.ts`, `packages/ruvyxa/runtime/plugin-*.mjs` (keep
   the worker protocol for `content.compile` and the React compiler under a new name),
   `crates/ruvyxa_middleware/src/plugin_host.rs` HTTP half,
   `crates/ruvyxa_dev_server/src/plugin_bridge.rs`, `plugins` config key, `RuvyxaPlugin*` types,
   entry points in `packages/ruvyxa/package.json` `exports`, `plugin-path-scope-conformance.json`,
   `RUV21xx` codes (registry gate must be updated).
4. **Release.** Major version; migration note listing every removed name and its target.

## Open questions for the owner

- Name: `proxy.ts` or `middleware.ts`. Recommendation: `proxy.ts`. Decided: `proxy` inside the
  config.
- Whether `fonts` ships in this programme or waits for a `ruvyxa/font` design.
- Whether any third party depends on `ruvyxa/plugin` today (a `templates/plugin` scaffold exists, so
  published plugins may exist).
