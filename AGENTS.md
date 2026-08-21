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
  and route manifests.
- `packages/` contains npm packages: `ruvyxa`, `create-ruvyxa`, `@ruvyxa/core`, `@ruvyxa/react`,
  adapters, and platform CLI packages.
- `examples/demo/` is the broad integration fixture.
- `templates/minimal/` is copied into new projects by `create-ruvyxa`.
- `docs/` is user-facing documentation.

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
- - A gate a comment promises has to exist. `output.rs` said
    `tests/packages/ruvyxa/entry-templates.test.mjs` asserted that its generated-entry preludes
    agreed with `entry-templates.mjs`; that test never read `output.rs`, so the routing-context and
    error-boundary preludes were two hand-maintained copies with nothing holding them together.
    `tests/packages/ruvyxa/entry-prelude-parity.test.mjs` executes both copies against one stand-in
    React. When a doc comment names the test that holds a contract, open that test.
- Documentation changes should describe actual supported behavior, not intended future behavior.
- If a check was already failing before your work, report it as baseline and do not weaken tests to
  pass.
