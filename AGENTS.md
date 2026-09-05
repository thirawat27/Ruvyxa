# Ruvyxa Monorepo Agent Guide

You are working in the Ruvyxa framework monorepo. Treat this file as the source of truth for agent
work in the repository.

## Repository Shape

- `crates/` contains the Rust workspace.
- `crates/ruvyxa_cli` owns the Ruvyxa CLI commands: `dev`, `build`, `check`, `start`, `preview`,
  `routes`, `analyze`, `adds`, `doctor`, `clean`, `trace`, `bench`, `test:parity`, and
  `plugin create`. The `Command` enum in `crates/ruvyxa_cli/src/main.rs` is the list; nothing else
  enumerates them.
- `crates/ruvyxa_bundler` owns TypeScript/JSX compilation, module resolution, linking, minification,
  source maps, and server/client boundary checks.
- `crates/ruvyxa_dev_server` owns Axum serving, HMR, render cache, router, worker pool, style
  collection, and action/API/client endpoints. Its `lib.rs` is the crate root — configuration,
  `serve`, the router table, and the request path for project pages and API routes. Handlers for the
  reserved `/__ruvyxa/*` paths live in `framework_endpoints.rs`, the WebSockets in
  `realtime_endpoints.rs`, and the development file watcher in `watcher.rs`. Add a new framework
  endpoint to `framework_endpoints.rs` and its route to the table in `lib.rs`, not to `lib.rs`
  alone; the conformance test there checks the two agree.
- `crates/ruvyxa_graph` owns file-system route discovery, validation, rendering strategy detection,
  and route manifests. It depends on `ruvyxa_bundler` for one reason: route validation reuses the
  bundler's source scanner rather than running a second text scan that could disagree.
- `crates/ruvyxa_middleware` owns the Tower layer stack — CORS, rate limiting, request logging,
  response timing, custom headers, client-IP resolution — and the TypeScript plugin host. Its crate
  docs carry the contract shared by every rate limiter in the workspace; read it before adding a
  fifth.
- `crates/ruvyxa_diagnostics` owns error codes, `Result`, and the path helpers every other crate
  agrees on (`normalized_canonical_path` in particular — see the Windows note under Change
  Guidance). `crates/ruvyxa_tui` owns terminal layout, spinners, progress, the mascot, the command
  header and success banner, the colour roles, and the decorative gradients.
  `cargo run -p ruvyxa_tui --example preview` draws every one of those surfaces in one screen, which
  is the only way to see the animated ones without rebuilding an application.
- `packages/` contains the npm packages:
  - `ruvyxa` — the framework package. `runtime/*.mjs` are the modules the Rust CLI resolves by path
    and spawns or imports; `src/plugins/` is the first-party plugin API behind the `plugins.ts`
    barrel; `packages/ruvyxa/scripts/sync-shared-runtime.mjs` regenerates the committed copies of
    shared modules.
  - `create-ruvyxa` — the scaffolder. Its `template/` directory holds a generated copy of every
    starter, not just `minimal`.
  - `@ruvyxa/core` — primitives both the Rust host and a deployed build need: `route-match`,
    `origin-policy`, the standalone server, shared server utilities.
  - `@ruvyxa/react` — the client router, `not-found`, and the React-facing surface.
  - `@ruvyxa/auth`, `@ruvyxa/database`, `@ruvyxa/realtime`, `@ruvyxa/testing` — optional
    integrations.
  - `@ruvyxa/adapter-*` — eleven deploy adapters; `@ruvyxa/cli-*` — five prebuilt CLI binaries.
- `examples/demo/` is the broad integration fixture, and it **is** deployable — `RUV2213` is gone,
  along with the refusal of a dynamic server-components route. A failure there is usually a question
  about the feature rather than about the emitted server, which is what the smoke fixture is for.
- `examples/deploy-smoke/` is the smallest application every self-hosted adapter can deploy, and is
  what CI builds and then launches on real Node, Bun, and Deno through
  `scripts/smoke-runtime-adapter.mjs`. Keep it deployable: a route or feature no adapter supports
  belongs in the demo instead. `/rsc` there is a **dynamic** server-components route on purpose — a
  pre-rendered one proves nothing about a deployment, because its payload is already in the file the
  adapter copies and no renderer runs.
- `templates/` holds five scaffolds — `minimal`, `blog`, `crud`, `api`, `plugin`.
  `templates/minimal/` is what `create-ruvyxa` copies into a new project.
  `packages/create-ruvyxa/template/` is **generated, not committed**: it is gitignored, and
  `packages/create-ruvyxa/scripts/prepare-template.mjs` deletes and rebuilds it from `templates/*`
  on `prepack`, renaming `.gitignore` to `gitignore` because npm strips a packaged dotfile. Edit
  `templates/`; never that copy, and never add a second mechanism to keep the two in step — this
  file used to say `scripts/check-template-mirrors.mjs` did, and it does not. That script holds one
  genuinely hand-maintained pair: `ruvyxa-runner.tsx` in the template and in `examples/demo`, so the
  fixture exercises what a scaffolded project ships with. `pnpm release:validate` fails on drift
  there.

  A scaffold's `ruvyxa.config.ts` carries only the decisions a new project has to make. Restating a
  default there costs twice: it teaches a newcomer that the key is required, and it pins the value,
  so a release that improves the default never reaches the projects already scaffolded. Four
  templates each restated fifteen of them. `scripts/pack-smoke.mjs` rewrites the scaffolded config
  by text replace to add plugins, and a replace that matches nothing returns the source unchanged —
  so trimming the template left three unused imports and a `tsc` error naming none of it. It asserts
  its anchor now; anything else that edits a template by string has to do the same.

- `tests/` holds the Node suites, one directory per package under `tests/packages/`, plus the
  cross-language tables in `tests/fixtures/`. Rust tests live beside the crate they cover.
- `docs/` is the user-facing guide, numbered `01`–`21` and mirrored chapter-for-chapter in
  `docs/en/` and `docs/th/`. `scripts/check-doc-links.mjs` resolves every markdown link and gates a
  release.
- `scripts/` holds the repository's own tooling: release validation, packaging smoke tests, adapter
  sync, template mirrors, git hooks, and the benchmark harness. They are entry points to Knip, so a
  new one needs no registration but a deleted one does.

## Operating Rules

- Preserve public CLI, config, package, and route behavior unless the task explicitly changes it.
- Read the existing module and its tests before editing shared framework behavior.
- Keep Rust and TypeScript contracts aligned when changing config, runtime files, package exports,
  or generated template behavior.
- Do not commit generated output such as `.ruvyxa/`, `dist/`, `.npm-pack/`, `.npm-smoke/`,
  `target/`, or `node_modules/`.
- Keep browser-safe env vars prefixed with `RUVYXA_PUBLIC_`; private env vars must stay server-only.
- Preserve the server/client boundary: `server-only`, `client-only`, `server/` imports, and private
  env access must continue to be caught by validation.
- For styling changes, keep dev, build, HMR, prerender, docs, and templates in agreement. Imported
  project CSS may live outside `app/`; unimported global styles should use `css.entries`.
- For npm packaging changes, verify that packed tarballs do not include tests or `workspace:`
  protocol dependencies and that runtime files needed by the CLI are included.

### The child-process stdio protocol

Six `runtime/*.mjs` files answer a Rust caller over stdout: `adapter-runner`, `api-renderer`,
`config-renderer`, `css-runner`, `plugin-runtime`, and `ssr-renderer`, plus `worker-pool` for the
persistent worker. Each carries its own small writer rather than importing a shared one, because
`api-renderer.mjs` and `css-runner.mjs` deliberately import no sibling module and a shared file
would have to be registered in `package.json` `files`, in `WORKER_RUNTIME_FILES`, and in the
standalone-copy tests to buy four lines. Local copies are fine; **diverging semantics are not**, and
they did diverge — three awaited the flush and three did not. Two rules hold them level:

- **Never `process.exit()` after an unflushed `process.stdout.write()`.** Exit does not drain a
  pending asynchronous write, and stdout is a pipe. Either `await` the write, or write the final
  message with `writeSync(1, …)` and exit — `css-runner.mjs`'s `respondAndExit` and
  `adapter-runner.mjs`'s `exitWithResponse` are the pattern. Setting `process.exitCode` and
  returning needs neither, because Node drains stdout on a natural exit.
- **A loop that keeps writing must respect backpressure.** Check `write()`'s return value and
  `await once(process.stdout, 'drain')`, as `worker-pool.mjs`'s `writeWorkerMessage` and
  `plugin-runtime.mjs`'s persistent mode do. Ignoring it buffers every unread response in memory for
  as long as the host is slow.

### Cache identity is derived, never stamped

**No hand-maintained version literal decides whether a cached thing may be reused.** Not
`const CACHE_VERSION: u8 = 1`, not `"route-v2-manifest-{}"`, not `':ast-build-hooks'`, not
`BUNDLE_INPUT_MANIFEST_VERSION = 1`. Every one of those existed here, each with a comment telling
the next person to bump it, and the comment is the whole problem: the stamp is only correct while
somebody remembers, and forgetting it is **silent** — the build serves an artifact the current code
would not produce, and nothing points at the stamp as the reason.

Derive the identity from something that already changes when the answer changes:

| The cache                                  | Its identity                                                                                               |
| ------------------------------------------ | ---------------------------------------------------------------------------------------------------------- |
| every artifact under `.ruvyxa/cache/`      | `versioned_key()` in `artifact_cache.rs` — `env!("CARGO_PKG_VERSION")` mixed into the key                  |
| a client route plan                        | `client_route_plan_variant()` — the serialized `BundleOptions` that produced it                            |
| compiled modules (`ruvyxa_bundler::cache`) | `COMPILER_VERSION` — the crate version, nothing appended                                                   |
| optimized images                           | `ENCODER_IDENTITY` — the crate version, mixed into the content-addressed key                               |
| the config-load cache                      | `CONFIG_CACHE_TOOLCHAIN` — the crate version, at a fixed path with nothing else to distinguish it          |
| the bundle-input manifest (`compiler.mjs`) | the sha256 of `compiler.mjs` itself: the file that defines the format is what changes when the format does |
| the incremental module graph               | `MANIFEST_VERSION` — the crate version, nothing appended                                                   |
| the artifact task graph                    | `ARTIFACT_GRAPH_IDENTITY` — the crate version, nothing appended                                            |

Two rules follow, and `a_client_plan_key_follows_every_option_that_shapes_it` in
`crates/ruvyxa_cli/src/tests.rs` holds the first:

- **A key names every input that changes the output.** The plan key named `emitChunkManifest` alone,
  so a `jsx` change reused a plan whose module set no longer matched — the automatic runtime imports
  `react/jsx-runtime` and the classic one does not.
- **Within one release, compatibility is the entry format's job.** A field added later is `Option`
  so "absent" stays distinguishable from "empty"; a field whose _meaning_ changes is renamed, so an
  entry that cannot answer the new question fails to deserialize instead of being trusted. See
  `CachedModuleEntry::aliases` in `incremental.rs` for the worked example.

Version numbers that are **not** this, and must stay literal: the framework's own version
(`env!("CARGO_PKG_VERSION")`, `package.json`), a spec constant (a source map is always
`version: 3`), a platform's config schema (`@ruvyxa/adapter-vercel` writes what Vercel expects), a
wire protocol two independently-shipped sides negotiate (`FLIGHT_PROTOCOL_VERSION`, the HMR
`protocolVersion`, `REFERENCE_MANIFEST_SCHEMA_VERSION`, `schemaVersion` in `tests/fixtures/*.json`),
a runtime floor (`MINIMUM_BUN_VERSION`), and a user-facing config value (`pwa`'s `version`). Those
describe a contract with something outside this repository. A cache key describes only us.

## Verification

Use the narrowest useful check while iterating. Before handing off broad framework, runtime,
template, or packaging changes, run the relevant subset of:

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --locked -- -D warnings
pnpm -r build
pnpm -r check
pnpm -r test
pnpm lint
pnpm format:check
pnpm check:unused
pnpm release:validate
pnpm pack:smoke
```

`pnpm lint` runs Oxlint over `packages/`, `templates/`, `examples/`, `scripts/`, and `tests/`.
`pnpm -r check` and `pnpm release:validate` both call it, so it gates a release. The rules it
enforces and, just as importantly, the ones it deliberately turns off are in `.oxlintrc.json`, each
with the reason beside it: sequential `await` in a loop is how the bundler applies backpressure,
`void` before a floating promise is the marker TypeScript itself recommends, and `__RUVYXA_*`
globals are a contract with the generated entry, not a naming slip. Add a rule to the off list only
with the reason it does not apply here, never to clear a finding.

It also bans three host-locale methods outright: `localeCompare`, `toLocaleLowerCase`, and
`toLocaleUpperCase`. Ordering and case-folding in this repository decide cache keys, content
fingerprints, heading slugs, and the bytes of files the build writes, and all three answer by the
host's ICU locale — so two machines building the same project disagreed, the JavaScript and Rust
graphs sorted the same glob differently, and a Turkish host folds `I` to `ı` where every other host
and the Rust compiler give `i`. Case-fold with `toLowerCase`/`toUpperCase`. Sort with
`compareCodeUnits`/`compareEntryKeys` from `packages/ruvyxa/runtime/order.mjs`, `compareStable` in
`packages/ruvyxa/src/plugins/shared.ts`, or a bare `.sort()` for an array of strings. Code emitted
into a function artifact writes the comparison out inline, because a deployed function directory
resolves no sibling specifiers.

It also caps structure directly: `complexity` at 30, `max-depth` at 4, `max-nested-callbacks` at 4,
and `max-params` at 8. `max-lines-per-function` is deliberately off — a long flat function is a
reading cost, a branchy one is a correctness risk, and only the second is worth failing a build
over. When a function trips `complexity`, split it along the seam it already has (a strategy per
branch, a section per validator) rather than hoisting fragments out to move the number.

The Rust side has the matching gate: `.clippy.toml` sets `cognitive-complexity-threshold` and
`[workspace.lints.clippy]` in `Cargo.toml` turns the lint on, so a function that grows past what one
screen holds fails `cargo clippy -- -D warnings`. The threshold sits well above Clippy's default
because a `match` over twenty token shapes is one decision to a reader; where the metric is inflated
by macro expansion rather than real branching, `#[allow]` it on that function with the reason.

`pnpm check:unused` runs Knip over the JavaScript/TypeScript workspaces and fails on unused files,
exports, types, and dependencies. `pnpm release:validate` runs it too, so it gates a release. Ruvyxa
loads a lot of code by convention rather than by import — `app/` routes, `plugins/`,
`ruvyxa.config.ts`, the `runtime/*.mjs` modules the Rust CLI resolves by path, and adapters resolved
from a `@ruvyxa/adapter-${name}` template string — so `knip.json` declares those as entry points or
ignored dependencies. When a new runtime module or dynamically loaded package reports as unused, add
it there rather than deleting it; check for a dynamic or path-based loader first. Knip must stay on
version 6 or newer: version 5 crashes against this repository's TypeScript 7.

For demo behavior changes, also run:

```bash
cargo run -p ruvyxa_cli -- check --root examples/demo
cargo run -p ruvyxa_cli -- test:parity --root examples/demo
```

The rest of the scripts, and when each is worth running:

| Command                               | When                                                                                                               |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `pnpm check:cargo-lock`               | after any `Cargo.toml` edit — the lockfile has to describe the manifests                                           |
| `pnpm check:oxc-lockstep`             | after touching `oxc` or `oxc-transform` — the Rust bundler and the Node runtime must be on one version             |
| `pnpm check:source-refs`              | after moving or renaming a test, fixture, or script — comments naming it are gates, and a stale one points nowhere |
| `pnpm check:silent-defaults`          | after adding a read or a decode — a failure turned into a default is a wrong answer that looks right               |
| `pnpm check:doc-attachment`           | after moving a doc comment — a `///` block that drifted off its item documents the wrong thing and still compiles  |
| `pnpm check:cross-language-constants` | after changing a value the Rust and JavaScript halves must both hold — the two copies drift silently               |
| `pnpm check:template-mirrors`         | after editing `templates/` or a `create-ruvyxa` template — the two trees are one source published twice            |
| `pnpm verify:reproducible`            | after a change to emitted bytes, ordering, or hashing — two builds of one input must agree                         |
| `pnpm test:full-flow`                 | before a release; scaffolds, builds, and runs a project end to end (PowerShell)                                    |
| `pnpm publish:dry-run`                | before a release, to see what would actually be published                                                          |

`pnpm release:bump` writes every manifest with `JSON.stringify`, which Prettier disagrees with — run
`pnpm format` after it or `pnpm format:check` fails on twenty-four `package.json` files.

Run the narrowest thing that can fail. `cargo test -p <crate> <name>` and
`node --test --test-name-pattern="<name>" tests/packages/ruvyxa/<suite>.test.mjs` are seconds;
`pnpm -r test` is minutes. A JavaScript suite needs the package built first — a
`core/build is not exported` style error almost always means a stale `dist/`, so run `pnpm -r build`
before believing it.

## Change Guidance

- Route matching has one JavaScript implementation, `packages/@ruvyxa/core/src/route-match.ts`.
  `packages/ruvyxa/runtime/route-match.mjs` is a committed copy of its compiled output, because the
  serverless handler is copied into function bundles that resolve no bare specifiers. After changing
  the shared module run `pnpm --filter ruvyxa sync:runtime` and commit both; `ruvyxa`'s build fails
  on a stale copy. `packages/@ruvyxa/core/src/origin-policy.ts` is copied the same way, by the same
  script — `packages/ruvyxa/scripts/sync-shared-runtime.mjs` takes a `SYNCED_MODULES` table, so a
  new shared runtime module is one entry there rather than a new script. The Rust router cannot
  share the module, so both languages are held to `tests/fixtures/route-match-conformance.json` —
  add a case there before changing match behavior.
- `packages/ruvyxa/src/plugins.ts` is a barrel, not an implementation file: it re-exports the public
  plugin API by name from `packages/ruvyxa/src/plugins/`, one module per plugin family (`http`,
  `pwa`, `seo`, `search`, `content-engine`, `openapi`, `build`) plus `shared` for helpers two or
  more families use and `sitemap-xml` for the sitemap document builder. The barrel lists names
  explicitly rather than re-exporting `*` because the family modules also export helpers to each
  other; a `*` would publish those as package API. A new plugin goes in the family module and gets
  one line in the barrel — adding it only to the module leaves it unreachable from `ruvyxa/plugins`.
- Rust shared behavior needs Rust tests near the changed crate.
- Runtime/config/package behavior needs Node tests under `tests/packages/**`. TypeScript suites go
  through `scripts/test-package.mjs <suite>`, which compiles them with `tsc -p tsconfig.test.json`
  into `.test-build/packages/<suite>/` and runs that output, so they are type-checked and never rely
  on a runtime that strips types. A new suite needs a `tsconfig.test.json` in its package (extending
  `tsconfig.test-base.json`) and a `test` script calling that runner. Import the package under test
  through its built `dist/*.js` — a `src/*.ts` import would be compiled into `.test-build/` as a
  second copy, and any module that resolves its own `import.meta.url` (adapter `package.json` reads,
  the `create-ruvyxa` template lookup) would then resolve against the wrong directory. Use
  `repoPath()` from `tests/repo-root.ts` to reach a repository file rather than walking up from
  `import.meta.url`.
- Template changes should stay package-manager neutral and must match
  `templates/minimal/package.json`.
- `templates/minimal/app/components/ruvyxa-runner.tsx` and
  `examples/demo/app/components/ruvyxa-runner.tsx` are required to be byte-identical, so the demo
  fixture exercises exactly what a scaffolded project ships with. After editing either, run
  `node scripts/check-template-mirrors.mjs` to resync and commit both; `pnpm release:validate` fails
  on drift. Five commits edited both copies by hand before this existed, and a real defect (a
  projectile scoring against two targets in one frame) lived in both copies as a result.
- A cross-language table that cannot be shared as code — a Content-Type map, a security-header list,
  a route-matching rule — belongs in `tests/fixtures/*-conformance.json` with a test in each
  language that replays it, not in a comment promising the two stay in sync. `env_read_is_private`
  (private env var policy), `STATIC_CONTENT_TYPES` (static asset Content-Type), and
  `DEFAULT_SECURITY_HEADERS` all drifted silently before gaining this. `env_read_is_private` was
  listed here while `tests/fixtures/env-policy-conformance.json` did not yet exist and the rule was
  still held by a comment alone — when this list names a fixture, check that the file is there.
- Count the copies before trusting a fixture. `STATIC_ASSET_EXTENSIONS` and
  `DEFAULT_SECURITY_HEADERS` each had a third copy in
  `packages/ruvyxa/runtime/serverless-handler.mjs` — the one that runs in every deployed build —
  while the fixture was replayed only by the Rust host and `@ruvyxa/core`, so two implementations
  agreeing proved nothing about the third. `tests/packages/ruvyxa/serverless-shared-tables.test.mjs`
  holds that host now. The built-in CORS layer had no fixture at all and had already drifted: the
  Rust config filled `methods` with an implicit `GET, POST, PUT, DELETE, OPTIONS` that the
  serverless handler never had, so a project that configured only `origins` answered a cross-origin
  `PUT` under `ruvyxa dev` and had the browser block it in production.
  `tests/fixtures/cors-conformance.json` holds both hosts now.
- A `cache.handler` `read()` answer is interpreted in three places — `handleDocumentRead` in
  `packages/ruvyxa/runtime/worker-pool.mjs`, `normalizeCacheEntry` in
  `packages/ruvyxa/runtime/serverless-handler.mjs`, and `stored_document_from_response` plus
  `serve_stored_document` in `crates/ruvyxa_dev_server/src/render_pipeline.rs` — and they drifted
  twice: the worker called a bare string fresh while the deployed handler called it stale, and the
  Axum host treated a stale document as a miss (falling through to the build's older copy, with no
  refresh) while the deployed host served it and refreshed behind the response. The same file
  exposed a third: the deployed host rendered a PPR page in full on every request while still
  writing the forced render to the store — a shell it never read back — so a deployment paying for a
  shared store got per-request renders, and the Axum host, which serves the stored shell, gave a
  different answer for the same URL. PPR is a stored shell like SSG and CSR on every host now,
  served with its `no-store` row and no validator. `tests/fixtures/stored-document-conformance.json`
  holds all three implementations; add a case there before changing what an answer means or what a
  strategy does with it.
- A rule the two module graphs both enforce needs one table, not two implementations that happen to
  agree today. The client/server module lane is the newest example: `references.rs` read a module's
  leading directive and then its file stem, while `compiler.mjs` matched the single literal filename
  `server.ts` — so `server.js`, every `action.ts`, and every `'use server'` module were compiled
  into the browser bundle under `ruvyxa dev` and refused by `ruvyxa build` with RUV1820. Server-only
  source reached a browser exactly where nobody checks, and the error arrived only at the end.
  `tests/fixtures/module-lane-conformance.json` holds both now.
- A route's element tree is composed by two generators — `route_tree_function` in
  `crates/ruvyxa_bundler/src/output.rs` and `routeTreeFunction()` in
  `packages/ruvyxa/runtime/entry-templates.mjs` — and a project renders through whichever one built
  it. Layouts, `template.tsx`, and parallel-route slots all merge onto one list of directory levels
  (`route_wrapper_levels` / `wrapperLevels()`), because they interleave:
  `layout > template > children` at each level, with that level's slots as the layout's props.
  Flattening them into "every template inside every layout" is the tempting shortcut and it is
  wrong. Add a case to `tests/fixtures/entry-composition-conformance.json` before changing what
  either emits, and keep a route that uses none of these features on the byte-identical loop it
  always had.
- A position in generated output has to be recorded where it is produced, never derived from the
  emit format. Source maps were built from three constants describing how many lines the output
  wrapper, the linker header, and the per-module preamble take; all three were wrong, and
  tree-shaking, minification, and a line the CLI prepended afterwards were not accounted for at all.
  The linker reports line provenance now, tree-shaking carries it, the minifier hands back oxc's
  positions, and `sourcemap::shift_generated_lines` moves a finished map when a caller prepends to
  the bundle. When adding a pass that rewrites bundle text, carry the provenance through it or the
  map describes a document nobody shipped.
- A Next.js convention that Ruvyxa does not implement must fail loudly or work, never silently do
  nothing. `export const dynamic` and `generateStaticParams` were read by nothing: a page that asked
  for `force-dynamic` was pre-rendered anyway and a route that declared its parameters pre-rendered
  none, with no diagnostic in either case. Both are honoured now. `export const metadata` is
  deliberately not aliased to `meta` — the shapes differ, and a name that half-works is worse than
  one that does not. When adding an alias, check that the _contract_ matches, not just the intent.
- A specifier the route graph cannot follow is not a specifier with nothing behind it.
  `detect_render_strategy` pre-renders a route whose reachable graph shows no request-dependent
  data, and `ModuleCache::edges` followed relative imports only — so an aliased import produced no
  edge and a page fetching through `@/lib/data` was baked at build time while the same page written
  `../../lib/data` stayed dynamic. The walk uses the bundler's `TsConfigPaths` now. Bare package
  specifiers stay outside it deliberately, and `an_aliased_import_is_followed_like_a_relative_one`
  asserts that boundary so it stays a decision.
- Marker scans over source text need a word boundary. `has_dynamic_data_markers` used `contains`,
  and `prefetch(` contains `fetch(` — so `router.prefetch()`, an API this framework ships, took
  automatic pre-rendering away from the page that called it. When a marker decides something, check
  which direction a wrong answer errs in and make the loose end the safe one.
- A list that mirrors a code construct has to be checked against that construct, in the direction
  the guard reads. `RESERVED_FRAMEWORK_ROUTES` protects plugins from registering a path axum has
  already taken, and two tests compared it with `tests/fixtures/framework-endpoint-conformance.json`
  — but nothing read `build_app_router`'s route chain _inwards_, so two registered paths were
  missing from the list and a plugin transport on either panicked the router at startup.
  `every_registered_route_is_reserved` parses the chain now. When a doc comment says "must stay in
  sync with X", the test has to read X.
- A failed read is not a value. Turning one into a default throws the message away _and_ invents an
  answer, and when the invented answer is also a legitimate one nothing downstream can tell them
  apart. `client_bundle.rs` read a route's source to record whether it exports `flight` and
  defaulted to `""`; `""` exports no `flight`, so every `ruvyxa build --root <elsewhere>` wrote
  `flight: false` for every route into the shipped manifest and the browser router stopped
  requesting payloads routes did produce. The same read behind `/__ruvyxa/flight` reported an
  unreadable file as `501 this route does not expose a Flight payload`. `write_style_asset` parsed
  `route-manifest.json` with a default of `{"routes": []}` and then wrote the document back, so a
  partial file — which an interrupted build leaves — was silently replaced by a manifest naming no
  routes at all. `.ok()` is fine: it hands the caller `None`, which is an honest "no answer".
  `unwrap_or_default()` and `unwrap_or_else(|_| …)` fabricate, and
  `scripts/check-silent-defaults.mjs` fails on them outside an allowlist that carries the reason for
  each accepted site.
- An empty cache slot is a question the cache cannot answer, not a "no". `RuntimeCache`'s generation
  counter exists so a collection that started before a watcher event cannot install its result, and
  `invalidate_styles_for_paths` skipped the bump when the slot was empty — which is exactly the
  in-flight state. Before deciding a change is irrelevant, check that the evidence for that exists.
- Output a machine parses needs a test that parses it. `site_discovery.rs` concatenates XML and
  every assertion matched its own output text, so nothing held `sitemap_header`'s conditional
  namespace declarations to the prefixes `sitemap_entry_xml` emits — a mismatch no parser accepts,
  reported by nothing until a search engine drops the sitemap. Neither ecosystem has an XML parser
  and one was not added; the rules are written out in the test module and held by
  `assert_the_checker_rejects_what_a_parser_would`, since a checker that accepts everything makes
  every test using it pass.
- Write the shared table down _while_ adding the second implementation, not after both are working.
  Byte-range parsing was added to the Rust server and to the generated standalone server in one go,
  and `tests/fixtures/byte-range-conformance.json` immediately caught a disagreement neither side
  would have noticed alone: both had a permissive number parser, and they were permissive about
  different things — `u64::from_str` takes a leading `+`, `Number()` takes `1e1` and `0x2`. The
  fixture, not either host language, decides what a byte position is.
- A worker the pool has retired is still the pool's. `NodeWorkerPool::shutdown` walked `workers`
  only, so a worker taken out of selection — every `RUVYXA_PRERENDER_RECYCLE_AFTER` isolated
  prerenders, and the whole generation on `recycle` — was owned by nothing but its detached drain
  task. A process that exits does not unwind that task, so nothing drops the `Child` and
  `kill_on_drop` never runs. Anything spawning a child into a detached task needs a registry the
  shutdown path reads.
- Any pass that _deletes_ code needs its output parsed by a test, not matched. The `NODE_ENV` fold
  in `crates/ruvyxa_bundler/src/minifier.rs` runs while a production client graph is being resolved
  and reports nothing, and it cut the `if` out of an `else if` chain — a bundle that does not parse,
  from a stage nobody watches. Text-matching its output would have passed.
- `exports` condition order is the package author's, and the first _supported_ condition wins, so
  which conditions a target supports is the whole decision. `require` beside `import` handed a
  browser bundle a CommonJS build; it is a second pass now. When touching `resolve_exports_value`,
  note that two divergences from Node are intentional and pinned by
  `package_exports_resolution_matches_the_documented_rules`: an unlisted subpath falls through to
  the legacy fields, and only an explicit `null` blocks.
- A test that has never been seen red has not been shown to hold anything. Sabotage the thing it
  guards and watch it fail before trusting it. Three tests written during this audit passed a broken
  implementation on the first attempt: an `ast.rs` corpus whose regexes held nothing consequential,
  a render-cache stress test that could not reach the race it named, and a prerender traversal test
  that covered the path rule but not the containment check behind it. Each needed a different case
  before it bit — a regex carrying a whole `import` statement, an inverted lock order, a symlink.
- - `ast.rs` answers to oxc. `the_scanner_finds_the_same_static_edges_as_the_real_parser` runs a
    corpus through both the byte scanner and the real parser and requires the static edges to match;
    add a case there before changing the scan. Two things decide whether a case is worth adding:
    `skip_string` stops at a newline by design, so a mis-scan cannot cross one — a regex holding
    nothing consequential proves nothing — and the corpus must stay parseable, because the
    differential asserts oxc accepts it.
- Both linkers rewrite ESM one line at a time, and every decision about what a line _is_ must go
  through `ast::masked_code` / `ModuleAst::is_code_offset` rather than a substring test. Four
  ordinary shapes failed builds because it did not: `export const note = "copied from here"` (a
  quoted `from` chose the re-export branch), `{ import: "./x" }` (a reserved word is a legal
  property name, and `reject_surviving_esm` flagged the token anywhere), `export function* gen()`
  (both graphs listed declaration forms with a trailing space), and a Prettier-wrapped
  `export { a, }` in a `.js` module — that last one emitted a bundle that does not parse with no
  build error at all. When adding a shape, add it to the adversarial suite in `linker.rs` that links
  and then **parses** the result; matching the output text passes on exactly the bugs this class
  produces.
- - The JavaScript graph has the same single owner: `packages/ruvyxa/runtime/scanner.mjs`, the port
    of `ast.rs`. It exports `createCodeIndex` (is this offset code?), `findInCode` (every code-only
    occurrence of a marker), `maskNonCode` (the mirror of `ast::masked_code` — same length, `\n` in
    place, everything else blanked), and `directivePrologueEnd`. **Route every new text walk through
    it.** `compiler.mjs` carried a second scanner beside it for a long time and the two differed in
    exactly one place: interpolations. `scanner.mjs` walks `${…}` as code; the private copy scanned
    to the next backtick, so an odd number of backticks inside a template — one hidden in a string
    or a regex inside an interpolation — ended the template early and desynchronised the rest of the
    file. `` const fence = `x${"`"}y` `` above an `import` dropped that dependency from the bundle
    with no diagnostic at all, because `extractSpecifiers` **is** the JavaScript module graph.
    Guarded by `keeps scanning code after a template literal that hides a backtick`.
  - A scan that needs the _value_ of a literal cannot read masked code, because masking blanks the
    literal. The answer is not "read raw": find the position in the mask, slice the value out of the
    raw source. `ruvyxa_graph::export_const_value` and `compiler.mjs`'s `privateEnvReads` are the
    worked examples.
- Path comparison on Windows goes through `ruvyxa_diagnostics::normalized_canonical_path`, never
  `Path::canonicalize` directly. `canonicalize` returns the extended-length `\\?\` prefix, and a key
  built from it never equals one built from a path the user typed — which broke every
  server-components build until the prefix was stripped in one place. Anything that becomes a map
  key, a cache key, or a manifest entry normalizes first.
- - A gate a comment promises has to exist. `output.rs` said
    `tests/packages/ruvyxa/entry-templates.test.mjs` asserted that its generated-entry preludes
    agreed with `entry-templates.mjs`; that test never read `output.rs`, so the routing-context and
    error-boundary preludes were two hand-maintained copies with nothing holding them together.
    `tests/packages/ruvyxa/entry-prelude-parity.test.mjs` executes both copies against one stand-in
    React. When a doc comment names the test that holds a contract, open that test — and
    `scripts/check-source-path-refs.mjs` now checks that you can. It reads the comments out of every
    `.rs`/`.mjs`/`.ts` file git knows about — tracked, or merely not ignored — and fails when a
    repository path named in one does not resolve, because `check-doc-links.mjs` only ever saw
    Markdown. Three pointers had already rotted behind it: a test that moved from `.mjs` to `.ts`,
    one naming a `tests/packages` react directory that has never existed, and a fixture called
    `endpoint-contract.json` that was never created at all. Paths written in string literals are
    ignored, so only comments are held.
- A shared record that several workers may be finishing at once is decided by **how many are still
  in flight**, not by what its state currently reads. `ArtifactTaskGraph::publish` is `begin` then
  `complete` with the lock released between them, so route builds that share one module overlap
  there; `begin` chose join-or-restart from `state` alone, and the first sibling to finish flipped
  the record to `Ready` while the others were still running. The next arrival then opened generation
  N+1, and every in-flight sibling was told
  `artifact <id> completed after its generation was invalidated` — a build failure with no bad input
  behind it, on a route that varied with scheduling. The condition is `active_builders > 0` now, and
  the interleaving is pinned deterministically by
  `a_sibling_that_joined_still_completes_after_the_first_one_finishes` rather than by a thread
  stress test, which passed against the broken code sixty-four times in a row.
- A deployed build renders its own documents, so it must write everything a document needs. The
  generated route registry produced markup and nothing else — no bootstrap block, no module
  preloads, no `<script type="module">` — because both writers of those are Rust
  (`client_hydration_script` for a live render, `inject_prerender_client_assets` for a baked page).
  So **no live-rendered page in any deployed build hydrated**: an SSR route never, and an ISR route
  from its first revalidation, since the revalidation persists what the registry rendered over the
  file the build had injected into. `documentAssetsPrelude()` in `entry-templates.mjs` is the fourth
  writer of that block and the JavaScript twin of `safe_json_for_script`, `escape_html`,
  `hydration_loader_url`, and the head/tail placement. The check that matters is not "is a script
  present" but "does the live render byte-match the baked file" — they are the same page.
- One physical package reached by two paths is two modules unless the graph says otherwise. pnpm
  links a package into every dependent's `node_modules`, and keying the module graph by the walked
  path put **five React instances** in one server bundle. Ordinary SSR survived it by luck; the
  server-components SSR pass did not, and every client component in a deployed RSC route threw
  `Cannot read properties of null (reading 'useRef')`. `moduleGraphKey()` normalizes the key with
  `realpathSync` and only the key — `filePath` stays the resolved path because client-reference ids
  are measured from it. The bundle also got 36% smaller.
- A finished bundle is an artifact, not a source file. The linker numbers its modules `__m0` upward,
  so inlining one linked bundle into another put a second `const __m1` in the same scope as the
  first, and the outer module's `const __ext1 = __m1` hit its temporal dead zone — the whole
  deployment failed to import. `identifierPrefix` exists for the one case that has to inline its own
  output: the deployed server-components registry is compiled with React external so it shares the
  renderer's instance, which it can only get by being linked _into_ the module that has it. The
  `react-server` bundle beside it carries its own React on purpose and stays a sibling file.
- An extension point is only open as far as its narrowest gate. `ruvyxa build --adapter <package>`
  has always resolved an arbitrary adapter package, and two things downstream closed it again:
  `AdapterOutput['platform']` was a union of the eleven platforms in this repository, and
  project-scope artifacts were checked against an allowlist of eleven hosting paths — so a community
  adapter could name no platform and write no config file for its own target. The allowlist also
  read as a security boundary and was not one: an adapter is a function the project installed and
  named, so it already has `node:fs`. It is a deny-list of the project's own source, manifests, and
  configured `appDir`/`outDir` now, matched on whole path segments — `apphosting.yaml` is a real
  file name that a prefix test reads as living inside `app`. When adding an extension point, check
  every gate its output passes through, not just the one that admits the caller.
- Rebuilding a `Response` costs more than its headers on Bun. The standalone server compresses
  text-shaped responses now, and the first version added `Vary: Accept-Encoding` by constructing a
  new `Response` around `response.body` — which, for a sliced `BunFile`, hands back the whole file
  rather than the slice. Every byte range over a compressible asset became the entire asset behind a
  `206`. When nothing about the body changes, set the header in place and return the same response;
  build a new one only on the path that actually replaces the body. The same rule is why the range
  and `HEAD` cases are excluded from compression rather than handled inside it.
- `ruvyxa.config`'s option names live in `packages/ruvyxa/runtime/config-schema.mjs`, and
  `RuvyxaConfig` in `@ruvyxa/core` is the second description of the same object. Nothing held them
  together and they drifted: `build.target` was accepted by the renderer, validated by the Rust
  config, applied by both compilers, and written up in `docs/*/07-configuration.md`, while the
  public type never declared it — so a project that set it failed `tsc` against a build that
  honoured the value. `tests/packages/core/config-schema.test.ts` replays the table against a
  `RuvyxaConfig`-typed literal: a key the type does not declare fails compilation, and a key the
  literal omits fails the comparison, so neither side can grow a key alone. `react` and `typescript`
  are the two deliberate exceptions and are named in `DEPRECATED_CONFIG_KEYS` rather than skipped
  quietly. A new `runtime/*.mjs` also needs its line in `package.json` `files`; `pack-smoke.mjs` now
  walks the `config-renderer.mjs` graph as well, so forgetting one fails the packaging smoke.
- A generated handler is held by running it, never by matching its text. Four adapters write their
  own platform wrapper — vercel, netlify, firebase, cloudflare — and each translates a platform
  request into a `Request` and a `Response` back. Only vercel's was ever executed; the other three
  had `assert.match(source, /createHandler/)`, which passes on every bug that class actually has.
  Changing cloudflare's manifest import to `./manifest.json` left six text assertions green while
  the Worker could not load at all. `tests/deployed-function.ts` assembles the bundle the way
  `adapter-runner.mjs` does and imports it; its `echoRouteModules` exists to make three specific
  wrapper bugs visible — a folded `Set-Cookie`, a body that arrived parsed instead of raw, and
  binary bytes round-tripped through a string. The runtime CI smoke covers only the standalone
  server (node, bun, deno, and the railway/render/aws adapters that reuse it), so for these four
  that suite is the only thing that runs the code. The same rule reached the HMR client.
  `hmr_client_script()` in `html_document.rs` emits the JavaScript every `ruvyxa dev` browser runs,
  and it was held by twelve `contains` assertions across two tests — which pass on an inverted
  comparison, a dropped `await`, a renamed cache-busting parameter, and on a script that does not
  parse at all. `tests/packages/ruvyxa/hmr-client-behaviour.test.mjs` parses the literal out of the
  Rust source and executes it in a `vm` against stand-in browser globals, so the questions are
  behavioural: a stale sequence stays ignored, a CSS update does not reload, an apply that a newer
  sequence overtook abandons itself rather than writing stale CSS, a refresh boundary that throws
  still falls back to a reload, and no update ever calls `document.createElement`. What stays in
  Rust is only the shape that suite needs to find the script. When a generated script is the
  product, the stand-ins are the test — not the substrings.
- A config key reaches its consumer or it does not exist. `adapterOptions` was declared in
  `RuvyxaConfig`, validated by `config-renderer.mjs`, written into `build.json`, and documented in
  the configuration guide — while `adapter-runner.mjs` called the adapter factory as `factory()`
  with no arguments. Every zero-config deploy selects its adapter by name (`--adapter`,
  `RUVYXA_ADAPTER`, platform detection), so those deployments could not configure their adapter at
  all, and the key that existed to fix that was inert. Passing it on is half the answer; the other
  half is `configuredAdapter`, which refuses `adapterOptions` beside an already-constructed
  `config.adapter` instead of ignoring it a second way. When adding a config key, follow it to the
  code that reads it and write the test that fails when the wiring is removed.
- A setting the deployment environment owns must be read from the environment, and the environment
  outranks the committed config file. `ruvyxa start` resolved its bind address from
  `--host`/`--port` and `ruvyxa.config.ts` alone, so it ignored `PORT` and bound `localhost` — while
  the standalone server the same build generates has always read `PORT`/`HOST` and bound `0.0.0.0`.
  A container routes to the container's address, not its loopback, so that deployment answered
  nothing and the platform reported a crash loop with no error in the log. `resolve_bind_address`
  owns the ordering for `dev`, `start`, and `preview` together, takes the environment through a
  closure so it is testable, and fails on an unparseable `PORT` rather than falling back to 3000 — a
  silent fallback surfaces only as a health check that never passes.
- Terminal colour is two systems with one boundary between them. A **role** in `ruvyxa_tui::theme`
  is the only carrier of a distinction — `ok` against `warn`, a page route against an API route — so
  every role stays inside the sixteen colours, because a terminal that cannot render a 24-bit code
  approximates it and two roles become one colour on somebody else's machine. **Decoration** in
  `ruvyxa_tui::gradient` carries no distinction, names the single role it collapses to, and may
  therefore be as rich as `ColorDepth` allows. Adding a 24-bit colour to a role looks like an
  improvement and is a regression; a gradient with no `fallback` simply vanishes below 256 colours.
  Anything that writes its own escapes has to ask `capabilities()` first — `tracing_subscriber`
  enables ANSI by default and consulted neither the terminal nor `NO_COLOR`, so every warning in a
  redirected build log carried a literal escape sequence while every other line in the same file was
  plain.
- Results go to stdout and everything else goes to stderr, and `--json` is what makes that a
  contract rather than a preference. That same `tracing_subscriber` also defaulted its writer to
  stdout, which is where `write_machine_report` writes; one warning during `ruvyxa bench --json` put
  ` WARN ...` ahead of the opening bracket and the output stopped parsing as JSON at character one.
  Diagnostics, progress frames, and spinners share the other stream for the same reason. When adding
  output, settle which of the two streams it belongs on before deciding what it should look like.
- A column is measured in characters, never in bytes. `display_width` exists for this, and
  `field_line`, `phase_line`, `track_line`, and `spinner_line` all pad through it. Every label the
  CLI prints today is ASCII, where `len()` gives the same answer — which is exactly why the byte
  count survived in four places until a label with a middle dot in it landed a column short of its
  neighbours. Anything drawn onto a live line is also sized against eighty columns and pinned by a
  test: the runner is an emoji and occupies two cells, and a frame that overflows cannot be erased
  by `[2K`, which clears one line and not a wrapped one.
- Documentation changes should describe actual supported behavior, not intended future behavior.
- If a check was already failing before your work, report it as baseline and do not weaken tests to
  pass.
