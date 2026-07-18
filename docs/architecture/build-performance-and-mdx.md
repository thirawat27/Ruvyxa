# Build performance and MDX architecture focus

## Pass and scope

**Pass:** Focus. The inspected flow spans `ruvyxa_cli` production orchestration and `ruvyxa_bundler`
content compilation. Staying in Scan Mode would miss persistent-cache and generated module consumers
that determine correctness.

**Evidence checked:** root manifests and tooling, `ruvyxa_cli/src/main.rs`, bundler context,
resolver/compiler/content paths, the packaged Node content compiler and tests, shared-route/cache
tests, EN/TH content guides, and the existing bundler modernization note. Generated output,
dependency source, unrelated dev-server/runtime modules, deployment adapters, and binary assets were
skipped.

## Confirmed flow

```text
ruvyxa build
  -> discover + validate routes
  -> collect/copy styles, app, server, and assets
  -> bundle each client page with a shared BundleContext
  -> identify and emit shared route modules
  -> rebuild affected route entries without those modules
  -> prerender static routes

page.md/page.mdx
  -> native path: YAML -> markdown-rs mdast -> generated React ESM
  -> Node SSR/SSG path: YAML -> @mdx-js/mdx + remark-gfm -> generated React ESM
  -> normal resolver/compiler/boundary/linker pipeline
```

## Findings

1. **High, Direct — duplicate and partly sequential route work.** Evidence: `emit_client_bundles`
   first bundles every page to collect module manifests, then rebuilds all pages sequentially after
   producing a shared chunk. Impact: route-split cold builds pay for two link/minify passes and do
   not use configured concurrency in the second pass. Authorized correction: retain the two
   compatibility-preserving passes but run both through one bounded deterministic executor; a future
   graph-scan API can remove the first render pass separately.
2. **High, Direct — artifact validation scales with route/module overlap.** Evidence:
   `load_client_artifact` reads and hashes every recorded file independently for each route. Impact:
   layouts and packages shared by many routes are re-read and re-hashed repeatedly. Authorized
   correction: one build-scoped, content-based fingerprint memo shared across workers.
3. **High, Direct — MDX ESM boundaries use line heuristics.** Evidence: `extract_mdx_esm` guesses
   continuation from trailing punctuation. Impact: valid multiline ESM can be treated as prose or
   prose can be consumed as JavaScript. Authorized correction: use the parser's MDX ESM construct
   with Oxc syntax feedback.
4. **Medium, Direct — documented syntax exceeds enabled/rendered syntax.** Evidence: MDX uses
   `ParseOptions::mdx()` alone; tables render every cell as `td`; frontmatter is a line parser.
   Impact: GFM and nested YAML are incomplete. Authorized correction: merge GFM+MDX constructs,
   render semantic table/reference/heading output, and parse YAML into JSON-compatible frontmatter.

## Implemented outcome

- Both client bundle passes use the configured bounded worker count and restore deterministic route
  order before emission.
- Warm artifact validation shares one build-scoped fingerprint snapshot, so a dependency referenced
  by many routes is read and hashed once during that build.
- Native MDX uses parser-backed ESM boundaries and combined GFM/MDX constructs. The packaged Node
  path enables the matching GFM extensions through `remark-gfm`.
- Native `serde_yaml_ng` and packaged `yaml` parsing support nested mapping/sequence/scalar values;
  both reject malformed or non-mapping frontmatter with `RUV1312`, and the Node path rejects values
  that cannot be serialized safely as JSON.
- Heading metadata is collected from each compiler's syntax tree, uses Unicode-aware stable
  duplicate slugs, and matches the IDs attached to rendered headings.

## Assumptions, questions, and risks

- **Assumption:** Source files do not intentionally mutate during one production build. The
  build-scoped fingerprint snapshot follows the existing staging/output snapshot model.
- **Open questions:** None identified.
- **Risk:** JS config plugins remain serialized through the existing Node worker mutex; this change
  does not claim plugin-heavy builds scale linearly.
- **Risk:** The compatibility-first two-pass shared-chunk design still performs extra linker work on
  cold builds. Removing that pass safely requires a dedicated graph-scan contract and stage metrics.

## Validation gates

- **Claim traceability:** Every finding above is direct from the named source paths.
- **Scope alignment:** Scope matches the requested build and MDX improvements; no adjacent runtime
  or deployment redesign is included.
- **Handoff readiness:** Focused tests, workspace checks, demo parity, residual risks, and safe
  future graph-scan work are named.
