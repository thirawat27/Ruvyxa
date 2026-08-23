# Ruvyxa Monorepo Agent Guide

You are working in the Ruvyxa framework monorepo. Treat this file as the source of truth for agent
work in the repository.

## Repository Shape

- `crates/` contains the Rust workspace.
- `crates/ruvyxa_cli` owns the Ruvyxa CLI commands: `dev`, `build`, `check`, `start`, `preview`,
  `routes`, `analyze`, `doctor`, `clean`, `trace`, `bench`, and `test:parity`.
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
  Guidance). `crates/ruvyxa_tui` owns terminal layout, spinners, progress, the mascot, and theme.
- `packages/` contains the npm packages:
  - `ruvyxa` — the framework package. `runtime/*.mjs` are the modules the Rust CLI resolves by path
    and spawns or imports; `src/plugins/` is the first-party plugin API behind the `plugins.ts`
    barrel; `scripts/sync-shared-runtime.mjs` regenerates the committed copies of shared modules.
  - `create-ruvyxa` — the scaffolder, with its own copy of `template/minimal`.
  - `@ruvyxa/core` — primitives both the Rust host and a deployed build need: `route-match`,
    `origin-policy`, the standalone server, shared server utilities.
  - `@ruvyxa/react` — the client router, `not-found`, and the React-facing surface.
  - `@ruvyxa/auth`, `@ruvyxa/database`, `@ruvyxa/realtime`, `@ruvyxa/testing` — optional
    integrations.
  - `@ruvyxa/adapter-*` — eleven deploy adapters; `@ruvyxa/cli-*` — five prebuilt CLI binaries.
- `examples/demo/` is the broad integration fixture. It is deliberately **not** deployable: it
  includes a dynamic server-components route, and every adapter refuses one with `RUV2213`, so
  `ruvyxa build --adapter <name>` against it always fails.
- `examples/deploy-smoke/` is the smallest application every self-hosted adapter _can_ deploy, and
  is what CI builds and then launches on real Node, Bun, and Deno through
  `scripts/smoke-runtime-adapter.mjs`. Keep it deployable: a route or feature no adapter supports
  belongs in the demo instead.
- `templates/` holds five scaffolds — `minimal`, `blog`, `crud`, `api-backend`, `plugin`.
  `templates/minimal/` is what `create-ruvyxa` copies into a new project, and it is mirrored into
  `packages/create-ruvyxa/template/minimal/`; `scripts/check-template-mirrors.mjs` keeps the two in
  step and `pnpm release:validate` fails on drift.
- `tests/` holds the Node suites, one directory per package under `tests/packages/`, plus the
  cross-language tables in `tests/fixtures/`. Rust tests live beside the crate they cover.
- `docs/` is the user-facing guide, numbered `01`–`11` and mirrored in `docs/en/` and `docs/th/`.
  `scripts/check-doc-links.mjs` resolves every markdown link and gates a release.
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

| The cache                                  | Its identity                                                                                                |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| every artifact under `.ruvyxa/cache/`      | `versioned_key()` in `artifact_cache.rs` — `env!("CARGO_PKG_VERSION")` mixed into the key                   |
| a client route plan                        | `client_route_plan_variant()` — the serialized `BundleOptions` that produced it                             |
| compiled modules (`ruvyxa_bundler::cache`) | `COMPILER_VERSION` — the crate version, nothing appended                                                    |
| optimized images                           | `ENCODER_IDENTITY` — the crate version, mixed into the content-addressed key                                |
| the config-load cache                      | `CONFIG_CACHE_TOOLCHAIN` — the crate version, at a fixed path with nothing else to distinguish it           |
| the bundle-input manifest (`compiler.mjs`) | the sha256 of `compiler.mjs` itself: the file that defines the format is what changes when the format does  |
| the incremental module graph               | `MANIFEST_VERSION`, a constant string with **no** counter — compatibility lives in the entry format instead |

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

It also bans `localeCompare` outright. Ordering in this repository decides cache keys, content
fingerprints, and the bytes of files the build writes, and `localeCompare` answers by the host's ICU
locale — so two machines building the same project disagreed, and the JavaScript and Rust graphs
sorted the same glob differently. Sort with `compareCodeUnits`/`compareEntryKeys` from
`packages/ruvyxa/runtime/order.mjs`, `compareStable` in `packages/ruvyxa/src/plugins/shared.ts`, or
a bare `.sort()` for an array of strings. Code emitted into a function artifact writes the
comparison out inline, because a deployed function directory resolves no sibling specifiers.

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

| Command                    | When                                                                                                   |
| -------------------------- | ------------------------------------------------------------------------------------------------------ |
| `pnpm check:cargo-lock`    | after any `Cargo.toml` edit — the lockfile has to describe the manifests                               |
| `pnpm check:oxc-lockstep`  | after touching `oxc` or `oxc-transform` — the Rust bundler and the Node runtime must be on one version |
| `pnpm verify:reproducible` | after a change to emitted bytes, ordering, or hashing — two builds of one input must agree             |
| `pnpm test:full-flow`      | before a release; scaffolds, builds, and runs a project end to end (PowerShell)                        |
| `pnpm publish:dry-run`     | before a release, to see what would actually be published                                              |

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
  script — `scripts/sync-shared-runtime.mjs` takes a table of modules, so a new shared runtime
  module is one entry there rather than a new script. The Rust router cannot share the module, so
  both languages are held to `tests/fixtures/route-match-conformance.json` — add a case there before
  changing match behavior.
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
    React. When a doc comment names the test that holds a contract, open that test.
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
- Documentation changes should describe actual supported behavior, not intended future behavior.
- If a check was already failing before your work, report it as baseline and do not weaken tests to
  pass.
