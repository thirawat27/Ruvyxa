# Ruvyxa Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

> **Status, 2026-08-30: this plan is complete, and its checkboxes are not the record.**
>
> Not one of the 159 boxes was ever ticked, so reading them says "nothing was done" about work that
> is finished and committed. They are left as written rather than back-filled: ticking a box now
> would assert a step-level verification nobody performed at the time, which is the same
> unearned-confidence failure this programme kept finding in the code.
>
> The record is `.superpowers/sdd/progress.md`, and the tree itself. Every phase was afterwards
> walked finding by finding against the source rather than against this file, which is how three
> tasks were caught half-landed — `Task 11` (`RUV-H6`), `BUNF-07`, and `CORE-10`. All three failed
> the same way: the finding names one defect, the code holds several copies of it, and fixing the
> copy the finding points at looks exactly like fixing the defect. A plan whose unit is a finding
> cannot see that; only the source can.

**Goal:** Fix all 141 findings in [`SYSTEM_AUDIT_REPORT.md`](../../../SYSTEM_AUDIT_REPORT.md),
highest-harm first, in small isolated batches that each end verified.

**Architecture:** Every task targets one coherent defect or one tightly-coupled pair. Where a rule
lives in two languages, the task fixes **both halves in the same task** — a half-fix converts a
shared defect into a divergence, which is worse. Where a fixture can hold the two halves level, the
task adds the fixture case _before_ the fix, so the fixture fails first.

**Tech Stack:** Rust (7 crates, `cargo` workspace), TypeScript/ESM (25 packages, `pnpm` workspace),
`node:test` for JavaScript suites, inline `#[cfg(test)]` for Rust, shared JSON fixtures under
`tests/fixtures/` replayed from both languages.

## Global Constraints

These bind **every** task. They are not repeated per task.

- **Never run `git commit`, `git add`, or `git push`.** The repository owner commits their own work.
  Every task ends at _verified and reported_, not at a commit. Where a generic TDD workflow would
  say "commit", this plan says "report".
- **Do not create a branch or switch branches.** Work in the existing `main` working tree.
- **Preserve changes you did not make.** Check `git diff HEAD`, not just `git diff` — staged changes
  do not appear in the latter.
- **One task, one batch of files.** Do not edit a file another in-flight task owns. The task list
  below is ordered so that no two adjacent tasks share a file.
- **Fix both halves of a cross-language rule in the same task.** Named explicitly where it applies.
- **A fixture case comes before the fix it gates.** Add the `tests/fixtures/*.json` entry, watch
  both replays fail, then fix both sides.
- **Never widen a public API without saying so.** Any change to `packages/@ruvyxa/core`'s exported
  surface, to `AuthUser`, or to an adapter contract needs a `CHANGELOG.md` entry in the same task.
- **Do not pin a version number in prose.** Point at the manifest. (Two gated exceptions exist in
  the root `README.md`; do not add a third.)
- **`localeCompare`, `toLocaleUpperCase`, and `toLocaleLowerCase` are banned.** They answer by the
  host's ICU locale. The oxlint rule enforces this; do not add a suppression.
- **Anything used as a lookup key goes through `normalized_canonical_path`.**
  `std::fs::canonicalize` returns the `\\?\` verbatim prefix on Windows.
- **Cache identity is derived, never stamped.** No new `CACHE_VERSION`, `-v2`, or `vN` constant.
- **The two byte scanners are `crates/ruvyxa_bundler/src/ast.rs` and
  `packages/ruvyxa/runtime/scanner.mjs`.** Do not add a third. If a task needs to walk source, route
  it through `ast::masked_code` / `maskNonCode`.

## Verification commands

Per-task verification uses the narrowest command that can fail. The full battery runs at the end of
each phase.

```bash
cargo test -p <crate> <test_name>
```

```bash
node --test tests/packages/<pkg>/<file>.test.mjs
```

Full battery, in this order, at each phase gate:

```bash
cargo fmt --all -- --check && cargo clippy --workspace --locked -- -D warnings && cargo test --workspace --locked
```

```bash
pnpm -r build && pnpm -r check && pnpm -r test && pnpm lint && pnpm format:check && pnpm check:unused
```

`pnpm -r build` first if a JavaScript suite fails on a missing export — that is almost always a
stale `dist/`, not a deleted file.

---

# Phase 1 — Data loss and corruption

Nothing in this phase is optional and nothing later should start until it is green. These four
findings silently change or destroy data the user owns.

### Task 1: Build-commit crash recovery (`RUV-C4`)

**Files:**

- Modify: `crates/ruvyxa_cli/src/build_output.rs:73` (`create_build_temp_dir`), `:87`
  (`commit_staged_build_outputs`)
- Test: `crates/ruvyxa_cli/src/tests.rs` (beside
  `staged_build_commit_replaces_outputs_and_preserves_cache_directory`, line 3106)

**Interfaces:**

- Produces: `recover_stranded_build_outputs(out_dir: &Path) -> anyhow::Result<()>`, called from the
  top of `commit_staged_build_outputs`. Task 2 does not depend on it.

- [ ] **Step 1: Write the failing test**

Simulate an interrupted commit by performing only its first half, then assert the next commit
restores rather than leaving `dist/` empty.

```rust
#[test]
fn an_interrupted_commit_is_recovered_by_the_next_build() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("dist");
    let staging = temp.path().join("staging");
    std::fs::create_dir_all(out_dir.join("server")).unwrap();
    std::fs::write(out_dir.join("server").join("index.mjs"), "previous").unwrap();
    std::fs::create_dir_all(staging.join("server")).unwrap();
    std::fs::write(staging.join("server").join("index.mjs"), "next").unwrap();

    // The first half of a commit, then a simulated kill: outputs are in the
    // rollback directory and `dist/` is empty.
    let backup = crate::build_output::create_build_temp_dir(&out_dir, ".build-rollback").unwrap();
    crate::build_output::move_named_build_outputs(&out_dir, &backup).unwrap();
    assert!(!out_dir.join("server").exists());

    crate::build_output::commit_staged_build_outputs(&staging, &out_dir).unwrap();

    assert_eq!(
        std::fs::read_to_string(out_dir.join("server").join("index.mjs")).unwrap(),
        "next"
    );
    let strays: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".build-rollback"))
        .collect();
    assert!(strays.is_empty(), "a stale rollback directory was left behind");
}
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cargo test -p ruvyxa_cli an_interrupted_commit_is_recovered_by_the_next_build` Expected: FAIL
— `dist/server/index.mjs` does not exist, because nothing looks for the stranded rollback directory.

- [ ] **Step 3: Write the marker file when the rollback directory is created**

In `commit_staged_build_outputs`, after `move_named_build_outputs(out_dir, &backup_dir)` succeeds,
write `backup_dir.join(".ruvyxa-rollback.json")` containing the process id and the `moved_existing`
list, using `ruvyxa_bundler::atomic_file::write_atomic`. Recovery must not have to guess which
outputs the directory holds.

- [ ] **Step 4: Add the recovery sweep**

Add `recover_stranded_build_outputs(out_dir)` and call it as the first statement of
`commit_staged_build_outputs`. For each `.build-rollback-*` directory in `out_dir`: read the marker;
if the pid it names is still alive, leave the directory alone (a concurrent build owns it);
otherwise, if the outputs it lists are **absent** from `out_dir`, restore them with
`restore_named_build_outputs` before removing the directory; if they are present, just remove it.
Sweep `.build-staging-*` the same way, removing only those whose recorded pid is dead.

Keying on a dead pid rather than on age is the whole point — two `ruvyxa build` invocations against
one `dist/` already race, and an age-based sweep would make that worse.

- [ ] **Step 5: Run the test and the neighbouring commit tests**

Run: `cargo test -p ruvyxa_cli build_output` Expected: PASS, including
`staged_build_commit_replaces_outputs_and_preserves_cache_directory`.

- [ ] **Step 6: Report** — files changed, tests run, output.

---

### Task 2: The default-export template-literal corruption (`RUV-C3`)

**Files:**

- Modify: `packages/ruvyxa/runtime/compiler.mjs:2094` (`writeRewrittenDefaultExport`), `:2318`
  (`isBalancedDefaultExpression`)
- Modify: `tests/fixtures/module-syntax-conformance.json`
- Test: `tests/packages/ruvyxa/module-syntax.test.mjs` (the existing replay — it **executes** its
  output, which is what makes it the right home)

**Interfaces:**

- Consumes: `maskNonCode(source)` from the same file, already used by the clause collector.
- Produces: nothing other tasks depend on.

- [ ] **Step 1: Add the fixture cases first**

Add three cases to `tests/fixtures/module-syntax-conformance.json`, each asserting the exact string
via `JSON.stringify` rather than a line count — the current "template literal spanning lines" case
asserts a **count**, which this defect preserves:

```json
{
  "name": "export default of a multi-line template literal",
  "source": "export default `line one\n  indented two\nline three`\n",
  "expect": "\"line one\\n  indented two\\nline three\""
},
{
  "name": "export default of a multi-line tagged template",
  "source": "const tag = (s) => s.raw.join('')\nexport default tag`x\n  y`\n",
  "expect": "\"x\\n  y\""
},
{
  "name": "export default of an object holding a multi-line template",
  "source": "export default { css: `a {\n  color: red;\n}` }\n",
  "expect": "\"a {\\n  color: red;\\n}\""
}
```

- [ ] **Step 2: Run the replay and confirm all three fail**

Run: `node --test tests/packages/ruvyxa/module-syntax.test.mjs` Expected: FAIL — the first shows a
`;` inside the string (`"line one;\n    indented two\n  line three"`), the second the same, the
third an indentation shift.

- [ ] **Step 3: Make completeness literal-aware**

`isBalancedDefaultExpression` counts only `()[]{}`, so an open template literal reads as balanced
and the collector stops on line 0. Replace the bracket-depth test with one that asks the mask: the
statement is complete only when the masked join has balanced brackets **and** no unterminated
template or string. `maskNonCode` already computes exactly that and is used one function earlier by
`isCompleteClauseStatement`.

- [ ] **Step 4: Stop trimming the collected lines, and place the `;` correctly**

`collectedRaw` stores `sourceLines[sourceLine].trim()`, which strips the author's indentation from
every continuation line. Join **untrimmed** raw lines, and emit the statement as its own multi-line
block with the `;` after the real end of the expression rather than appended to line 0.

- [ ] **Step 5: Run the replay**

Run: `node --test tests/packages/ruvyxa/module-syntax.test.mjs` Expected: PASS, including the
pre-existing `export default an object literal spanning lines` and
`export default arrow spanning lines` cases, which guard the semantics of the whitespace change.

- [ ] **Step 6: Report.** Note in the report that emitted whitespace for every multi-line
      `export default` has changed, so any golden-output assertion elsewhere moves once.

---

### Task 3: The linker indents inside template literals — both languages (`RUV-H11`)

**Files:**

- Modify: `packages/ruvyxa/runtime/compiler.mjs:1687` (`linkModules`)
- Modify: `crates/ruvyxa_bundler/src/linker.rs:1382-1390`
- Modify: `tests/fixtures/module-syntax-conformance.json`
- Test: `tests/packages/ruvyxa/module-syntax.test.mjs`, plus a Rust test in `linker.rs`'s test
  module

**Interfaces:**

- Consumes: `maskNonCode` (JS) and `ModuleAst.text_spans` (Rust) — both already carried for exactly
  this class of question.

**This task fixes both halves.** Fixing only one converts a shared defect into a divergence.

- [ ] **Step 1: Add the fixture case**

```json
{
  "name": "a module body's multi-line template literal keeps its own indentation",
  "source": "export const css = `a {\n  color: red;\n}`\n",
  "expect": "\"a {\\n  color: red;\\n}\""
},
{
  "name": "a backslash-continued string keeps its own indentation",
  "source": "export const s = 'x\\\n  y'\n",
  "expect": "\"x  y\""
}
```

- [ ] **Step 2: Run both replays and confirm both fail**

Run: `node --test tests/packages/ruvyxa/module-syntax.test.mjs` Expected: FAIL —
`"a {\n    color: red;\n  }"`, two spaces added to each continuation line.

- [ ] **Step 3: Make the JS emit literal-aware**

In `linkModules`, compute `maskNonCode(rewritten.code)` once before the loop. Indent line _n_ only
when the mask's line _n_ begins in code. The mask preserves offsets and newlines, so
`masked.split('\n')[index]` lines up one-to-one with `codeLines[index]`.

- [ ] **Step 4: Make the Rust emit literal-aware**

`linker.rs:1382` does the same unconditional `out.push_str("  ")`. Gate it on the line's first byte
not falling inside a `ModuleAst.text_spans` range.

- [ ] **Step 5: Add the Rust twin test**

In `linker.rs`'s test module, link a module exporting a multi-line template literal and assert the
emitted body contains `\n  color: red;\n` and not `\n    color: red;\n`.

- [ ] **Step 6: Run both**

Run: `node --test tests/packages/ruvyxa/module-syntax.test.mjs` Run:
`cargo test -p ruvyxa_bundler linker` Expected: PASS both.

- [ ] **Step 7: Report.** Note that the emitted bytes of every bundle containing a multi-line
      template change once, so a reproducible-build baseline moves.

---

### Task 4: Minified ESM statements erased by the hoister (`RUV-H8`)

**Files:**

- Modify: `crates/ruvyxa_bundler/src/linker.rs:1635` (`collect_external_imports`), `:1685`,
  `:2140-2153` (`declares_esm_syntax`)
- Test: `crates/ruvyxa_bundler/src/linker.rs` test module, beside
  `server_link_hoists_external_imports` (line 3389) and
  `client_link_replaces_unresolvable_bare_imports_with_throwing_bindings` (line 3405)

**Interfaces:**

- Produces: `fn normalized_statement(line: &str) -> Cow<'_, str>`, used by all three readers so a
  fourth cannot be added without it.

- [ ] **Step 1: Write the failing tests**

Add the minified spellings to the two existing hoist tests: a module whose line is
`import{React}from"react"` and one whose line is `import*as R from"react"`, each asserting the same
hoist / `RUV1611` outcome the spaced spelling already gets. Add a third asserting that a module
whose only export line is `export{a as default}` gets the `__esModule` marker.

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p ruvyxa_bundler linker` Expected: FAIL — the import is neither hoisted nor
stubbed; it is replaced with an empty line by the rewriter, and the `__esModule` case is missing its
marker.

- [ ] **Step 3: Extract the shared normaliser**

Add `normalized_statement`, wrapping the existing `normalize_esm_statement` and returning
`Cow::Borrowed` when it returns `None`.

- [ ] **Step 4: Use it in the two blind readers**

In `collect_external_imports`, normalise before the `starts_with("import ")` test, before
`split_from_specifier`, and before the `strip_prefix("import ")` at `:1688`; hoist
`ensure_semicolon(statement)` so the emitted statement is the normalised one. In
`declares_esm_syntax`, normalise before the `starts_with("export ")` test.

- [ ] **Step 5: Run**

Run: `cargo test -p ruvyxa_bundler linker` Expected: PASS. No existing bundle's bytes change —
normalisation only inserts spaces at token boundaries it has already proven are boundaries, and
returns `None` for the spaced common case.

- [ ] **Step 6: Report.**

### Phase 1 gate

- [ ] Run the full Rust battery and the JavaScript battery. Both must be green before Phase 2
      starts.

---

# Phase 2 — Security, Critical then High

### Task 5: Request-scoped renders must never be shared-cacheable (`RUV-C1`)

**Files:**

- Modify: `packages/ruvyxa/runtime/serverless-handler.mjs:1336` (`handlePage`)
- Modify: `crates/ruvyxa_dev_server/src/render_pipeline.rs:441` (the `insert_document_cache_control`
  caller)
- Test: `tests/packages/ruvyxa/serverless-handler.test.mjs`, beside the document-validator block at
  line 1420

**Both hosts, same task.**

- [ ] **Step 1: Write the failing tests** — for each of `ssg`, `csr`, `isr`, a handler whose
      `importPage` returns a render that calls `cookies()`. Assert the response carries `no-store`,
      carries no `s-maxage`, and that `writePrerendered` was **not** called. Pinning both halves
      together is deliberate: the store guarantee already holds and must keep holding.
- [ ] **Step 2: Run and confirm failure.** Run:
      `node --test tests/packages/ruvyxa/serverless-handler.test.mjs`. Expected: FAIL —
      `public, max-age=0, s-maxage=60, stale-while-revalidate=…`.
- [ ] **Step 3: Fix the deployed host.** Replace the `documentCacheControl(...)` argument with
      `rendered.requestScoped ? 'no-store' : documentCacheControl(...)`, mirroring the `formData`
      branch two functions up at `:1372`, which already does exactly this.
- [ ] **Step 4: Fix the native host.** Apply the same guard at `render_pipeline.rs:441`.
- [ ] **Step 5: Run both suites.** `node --test …serverless-handler.test.mjs` and
      `cargo test -p ruvyxa_dev_server render_pipeline`. Expected: PASS.
- [ ] **Step 6: Add a CHANGELOG entry.** An ISR route that reads `headers()` on every request stops
      being CDN-cacheable — correct, and an origin-load change operators must be told about.
- [ ] **Step 7: Report.**

---

### Task 6: One answer to "what is this request's path" (`RUV-C2`)

**Files:**

- Modify: `packages/ruvyxa/runtime/plugin-http.mjs:357` (`decodedRequestPathname`), and its two call
  sites at `:159` and `:204`, and the `entry.path !== pathname` comparison at `:161`
- Modify: `crates/ruvyxa_dev_server/src/lib.rs:1761` (hand the plugin the canonical path, not
  `request_target`)
- Create: `tests/fixtures/plugin-path-scope-conformance.json`
- Test: `tests/packages/ruvyxa/plugins.test.mjs`, and the `plugin_bridge.rs` suite

- [ ] **Step 1: Write the fixture.** A table of request targets —
      `['/api/users', '//api/users', '/api//users', '/admin', '/admin/', '//admin', '/files/my%20doc']`
      — each with the canonical path the router resolves and whether it is in scope for `['/api/*']`
      and for `['/admin']`.
- [ ] **Step 2: Write both replays and confirm both fail.** The JS replay drives
      `matchesPatterns(decodedRequestPathname(request), patterns)` against `canonicalRoutePath`; the
      Rust replay drives the plugin bridge's path against `canonical_request_path`. Expected: FAIL
      on `//api/users`, `/api//users`, `/admin/`, `//admin`.
- [ ] **Step 3: Fix the deployed half.** Import `canonicalRoutePath` from `./route-match.mjs` —
      already in `HANDLER_RUNTIME_FILES` and already copied beside `plugin-http.mjs` in every
      function bundle — and fall back to the raw pathname when it returns `null` (a path the router
      rejects with 400 anyway).
- [ ] **Step 4: Fix the native half.** Pass the canonical `request_path` to `apply_request_plugins`
      instead of the raw `request_target`.
- [ ] **Step 5: Run both replays.** Expected: PASS.
- [ ] **Step 6: Report.** Note the widening: a hook matching a trailing-slash URL through `'/x/*'`
      now also matches the canonical `/x`, which is the correct behaviour and what the router has
      always done.

---

### Task 7: Refuse a `javascript:` navigation (`RUV-C5`)

**Files:**

- Modify: `packages/@ruvyxa/react/src/router.ts:226` (`resolveInternalUrl`), `:751` (`navigate`),
  `:891` (`prefetch`)
- Test: `packages/@ruvyxa/react/test/router.test.mjs` — it already stubs `window.location.assign`

- [ ] **Step 1: Write the failing tests.**

```js
test('refuses a javascript: navigation', async () => {
  const { router, assigned } = createTestRouter()
  await router.navigate('javascript:globalThis.__pwned = 1')
  assert.deepEqual(assigned, [])
  assert.equal(globalThis.__pwned, undefined)
})

test('still hands mailto: to the browser', async () => {
  const { router, assigned } = createTestRouter()
  await router.navigate('mailto:a@b.c')
  assert.deepEqual(assigned, ['mailto:a@b.c'])
})
```

- [ ] **Step 2: Run and confirm the first fails.** Run:
      `node --test packages/@ruvyxa/react/test/router.test.mjs`.
- [ ] **Step 3: Make the refusal explicit.** Change `resolveInternalUrl` to return
      `{ kind: 'internal', url } | { kind: 'external', url } | { kind: 'refused' }`. Allow `http:`,
      `https:`, `mailto:`, `tel:`, `sms:`, and same-page `#` for the external arm; refuse everything
      else with a `console.error` and an early return.
- [ ] **Step 4: Pass the parsed value, not the raw string.** In `navigate`, call
      `location.assign(url.href)` for the external arm. Apply the same guard at the top of
      `prefetch`, which already discards non-internal URLs but should not be the only place the rule
      is written down.
- [ ] **Step 5: Run.** Expected: PASS both.
- [ ] **Step 6: CHANGELOG.** A custom scheme (`web+foo:`, an app deep link) passed to
      `router.push()` now needs to be in the allow-list. `<Link>` is unaffected — the plain
      `<a href>` still hands those to the browser.
- [ ] **Step 7: Report.**

---

### Task 8: Client identity, all four decisions (`RUV-H1`, `RUV-H2`, `RUV-H3`)

**Files:**

- Modify: `crates/ruvyxa_middleware/src/client_ip.rs:154` (`forwarded_client_ip`)
- Modify: `crates/ruvyxa_dev_server/src/action_security.rs:541` (`forwarded_scheme`)
- Modify: `packages/@ruvyxa/core/src/standalone-server.ts` — the three transports at `:962`, the Bun
  handler, the Deno handler
- Modify: `packages/@ruvyxa/adapter-netlify/src/index.ts:166`
- Modify: `tests/fixtures/client-ip-conformance.json`, `tests/fixtures/adapter-contract.json`
- Test: `client_ip.rs` tests, `tests/packages/core/standalone-server-conformance.test.ts`,
  `tests/packages/adapter-netlify/index.test.ts`

These are three independent defects in one decision. They share the fixture, so they share a task.

- [ ] **Step 1: Extend the fixture to express duplicate header lines.** The current `headers` field
      is a JSON object, so a repeated field name is not representable — which is why no existing
      test can reach `RUV-H1`. Add an optional `headerLines: [[name, value], …]` form alongside it.
- [ ] **Step 2: Write the failing Rust test.** Build the `HeaderMap` with `append` twice rather than
      `insert` and assert the **last** line's rightmost untrusted hop wins. Expected: FAIL — `get`
      returns the client-written first line.
- [ ] **Step 3: Fix `forwarded_client_ip`.** Use `headers.get_all("x-forwarded-for")`, iterate the
      values in reverse (last field line first, then right-to-left within each), falling back to
      `get_all("x-real-ip")` only when the first list is empty. Keep the `parse::<IpAddr>()` filter.
      Mirror in `forwarded_scheme`.
- [ ] **Step 4: Write the failing standalone-server test.** A real Node child on `127.0.0.1` with a
      configured **non-loopback** trusted list (loopback is trusted, so the naive test cannot fail),
      asserting a request carrying `X-Forwarded-For: 203.0.113.7` is bucketed with one that does not
      carry it.
- [ ] **Step 5: Make the trust decision in the transport.** Add a shared helper to
      `sharedServerSource` that parses `runtimePolicy.security?.trustedProxyIps` once. In each
      transport, when the peer is neither loopback nor inside a configured prefix, delete
      `x-forwarded-for` and `x-real-ip` before the request reaches `handleAdmitted` — Node by
      skipping them while rebuilding `headers`, Bun via `server.requestIP(request)`, Deno via the
      second `handleRequest(request, info)` argument. `createHandler` needs no change: a stripped
      header makes `clientAddress()` fall through to `'unknown'`, which the fixture already calls
      the safe direction.
- [ ] **Step 6: Declare Netlify's ingress header.** Add
      `clientIpHeaders: ['x-nf-client-connection-ip']` beside `supportedStrategies`. **Do not guess
      Firebase's** — for Cloud Functions v2 the client is the _left_ end of `X-Forwarded-For`, which
      the right-to-left scan will not take, so Firebase gets a `security.trustedProxyIps` guidance
      change instead and is tracked as a follow-up, not fixed by a header guess here.
- [ ] **Step 7: Close the hole in the contract.** Add `clientIpHeaders` to
      `tests/fixtures/adapter-contract.json` as a required key per adapter — the way
      `onDemandImages` is required "so a new adapter has to decide" — and assert in the Netlify test
      that the emitted handler declares the expected value.
- [ ] **Step 8: Run.** `cargo test -p ruvyxa_middleware client_ip`,
      `node --test tests/packages/core/standalone-server-conformance.test.ts`,
      `node --test tests/packages/adapter-netlify/index.test.ts`. Expected: PASS.
- [ ] **Step 9: CHANGELOG and adapter READMEs.** A standalone server behind an _unlisted_ proxy
      collapses to one bucket until `trustedProxyIps` is configured. That is what `ruvyxa start` has
      always done, but it is a visible change for self-hosted deployments.
- [ ] **Step 10: Report,** naming the Firebase follow-up explicitly as not fixed.

---

### Task 9: The rate limiter must evict, not refuse (`RUV-H4`)

**Files:**

- Modify: `crates/ruvyxa_middleware/src/builtin.rs:22`, `:469` (`extract_key`), `:499`
- Test: `builtin.rs` tests, beside `evicts_expired_buckets_only_when_capacity_is_reached` (line 941)

- [ ] **Step 1: Write the failing tests.** Fill the map with `MAX_TRACKED_RATE_LIMIT_KEYS`
      **unexpired** buckets and assert a brand-new key is still admitted. Add a second asserting
      `extract_key` returns a bounded-length key for a 16 KB header value.
- [ ] **Step 2: Run and confirm both fail.** Run: `cargo test -p ruvyxa_middleware builtin`.
- [ ] **Step 3: Bound the key.** Hash it with `blake3` (already a workspace dependency) before it
      becomes a map key, so an attacker-chosen header cannot retain kilobytes per bucket.
- [ ] **Step 4: Evict at capacity instead of refusing.** After the expired sweep fails to free room,
      evict the bucket with the oldest `last_refill` and admit the new client. Refusing is only
      correct when the limiter genuinely cannot answer; here it can.
- [ ] **Step 5: Run.** Expected: PASS, and the existing eviction test still passes.
- [ ] **Step 6: Report.** Note that a 429 can no longer name the client in a log — nothing reads the
      key back out today.

---

### Task 10: Guard the second mutation endpoint (`RUV-H5`)

**Files:**

- Modify: `crates/ruvyxa_dev_server/src/framework_endpoints.rs:538`
  (`resolve_server_components_route`), and the two RSC handlers so they take
  `ConnectInfo<SocketAddr>`
- Modify: `tests/fixtures/framework-endpoint-conformance.json` — add `requiredOrigin` and
  `rateLimited` per endpoint
- Test: `framework_endpoints.rs` beside `the_rsc_gate_headers_match_the_shared_endpoint_contract`
  (line 1337); rate-limit test mirroring `rate_limits_action_keys` (`lib.rs:3247`)

- [ ] **Step 1: Write the failing tests.** Assert `resolve_server_components_route` refuses a
      request carrying `Origin: https://evil.test` against `Host: app.test`; and that the 601st RSC
      action call in a minute is refused, as `/__ruvyxa/action` already is.
- [ ] **Step 2: Run and confirm failure.**
- [ ] **Step 3: Add the origin and fetch-metadata checks.** After the header check, apply
      `config.same_origin_actions && action_origin_is_cross_site(headers, config, peer.ip())` and
      `config.fetch_metadata_actions && action_fetch_site_is_cross_site(headers)` → 403. Both config
      fields already default to `true` and this endpoint simply never read them.
- [ ] **Step 4: Add the rate limiter.** Give `rsc_action_endpoint` an `ActionRateLimiter` key shaped
      like `action_rate_limit_key`, keyed on client + `query.path` + the `x-ruvyxa-action`
      reference. Do not key it so tightly that a page issuing several server-function calls per
      interaction trips it.
- [ ] **Step 5: Extend the fixture so the deployed host is held to the same thing.**
- [ ] **Step 6: Update the existing probes.** `tests/packages/ruvyxa/framework-endpoints.test.mjs`
      sends no `Origin`, and the origin check is fail-closed when both `Origin` and `Sec-Fetch-Site`
      are absent. Update the probes to send a same-origin `Origin`.
- [ ] **Step 7: Run both suites.** Expected: PASS.
- [ ] **Step 8: Report.**

---

### Task 11: Surface a skipped platform config, and unfreeze Netlify's headers (`RUV-H6`)

**Files:**

- Modify: `crates/ruvyxa_cli/src/build.rs:419` (read `AdapterArtifactReport::skipped`)
- Modify: `packages/@ruvyxa/adapter-netlify/src/index.ts:301` (the `frameworksApi` gate)
- Test: `tests/packages/adapter-netlify/index.test.ts`,
  `tests/packages/ruvyxa/adapter-runner.test.mjs`, a Rust test in `ruvyxa_cli`

- [ ] **Step 1: Write the failing tests.** (a) `adapter-runner.test.mjs`: a project-scope artifact
      with `skipIfExists: true` whose destination exists produces a report carrying `skipped: true`.
      (b) A Rust test: a build whose adapter report contains a skipped project-scope file emits a
      warning naming that file. (c) `adapter-netlify`: `.netlify/v1/config.json` is emitted even
      when `projectConfig` is true.
- [ ] **Step 2: Run and confirm (b) and (c) fail.** `skipped` is deserialized today and read by
      nothing — `grep -rn "\.skipped" crates/ruvyxa_cli/src/*.rs` returns nothing.
- [ ] **Step 3: Warn on a skipped project-scope artifact.** In `build.rs`, name each skipped file
      and say whether its current contents still contain the security-header block that would have
      been written. A one-line "kept your `netlify.toml`; it does not contain the current security
      header block — delete it to regenerate" is the whole fix from a user's point of view.
- [ ] **Step 4: Split the Netlify `frameworksApi` gate.** The gate exists because two _functions_
      would collide. The config half collides with nothing: emit `.netlify/v1/config.json` — which
      is rewritten every build and already carries `headerRules` verbatim — unconditionally, and
      gate only the function artifact on `frameworksApi`.
- [ ] **Step 5: Confirm the precedence before landing step 4.** Netlify's documented precedence
      between `netlify.toml` `[[headers]]` and `.netlify/v1/config.json` `headers` decides which
      wins. If the generated config wins, a project that deliberately relaxed `X-Frame-Options`
      would silently get it back — in that case ship step 3 only and record step 4 as blocked.
      **This is a real blocker; do not guess.**
- [ ] **Step 6: Run.** Expected: PASS.
- [ ] **Step 7: Report,** stating explicitly whether step 4 landed or was blocked on step 5.

---

### Task 12: Magic-link sign-in must work in a browser (`RUV-H7`)

**Files:**

- Modify: `packages/@ruvyxa/auth/src/index.ts:664` (`htmlPage`)
- Test: `tests/packages/auth/index.test.ts`, extending the "consumes magic links exactly once" case

- [ ] **Step 1: Write the failing test.** POST the callback with `origin: 'null'` — the value a real
      browser sends from a `no-referrer` document — and assert 303, not 403.
- [ ] **Step 2: Run and confirm it fails** with `RUV3101`.
- [ ] **Step 3: Change the referrer policy.** Replace `<meta name="referrer" content="no-referrer">`
      with `content="same-origin"`. Per WHATWG Fetch "Append a request `Origin` header" step 3.1, a
      form-POST navigation from a `no-referrer` document sends `Origin: null`; `same-origin` keeps
      the token out of cross-origin Referers while leaving `Origin` intact. **Do not delete
      `assertSameOrigin`.**
- [ ] **Step 4: Run.** Expected: PASS, and the existing single-use and expiry cases still pass.
- [ ] **Step 5: Add the durable guard.** A check that no `htmlPage` rendering a `<form` also emits
      `no-referrer`, so the two correct decisions cannot collide again.
- [ ] **Step 6: Report.**

### Phase 2 gate

- [ ] Full battery, both languages.

---

# Phase 3 — Reliability, High

Ten findings. Each is its own task; none shares a file with its neighbour.

- [ ] **Task 13 — `RUV-H9`:** widen the re-print trigger in
      `crates/ruvyxa_bundler/src/compiler.rs:312` from "the brace is first after the keyword" to
      "the line opens a brace it does not close", counted over `masked_code`. Keep the two
      documented exceptions (`export default {`, `export const x = {`) by requiring no `=` on the
      line and, for `export`, not starting `export default`/`export const|let|var`. Extend
      `reject_surviving_esm`'s hint at `linker.rs:1052` with a third case naming an unclosed clause
      brace. **Tests:** add `import React, {\n useState,\n} from "react"` and
      `export {\n a,\n}\nfrom "./m.js"` to `only_a_multiline_clause_asks_for_a_re_print`
      (`lib.rs:2708`), plus a bundle-level test putting the default-plus-named form in a
      `node_modules` `.js` file.
- [ ] **Task 14 — `RUV-H10`, both languages:** delete `javascriptTokens` (`compiler.mjs:3789`) and
      `javascript_tokens` (`content.rs:387`). Derive `hasNamedExport` from
      `maskNonCode(source, { preserveImportExportSpecifiers: true })` plus the existing `findInCode`
      / `exportListBinds` walk; reimplement `has_named_export` on `ast::parse_module` +
      `ast::has_named_runtime_export`, adding a clause helper for `export { … as NAME }` beside
      `named_clause_exports_default`. **Tests:** a new content fixture with `.mdx` cases carrying a
      regex containing `'`, `"`, and a backtick above a user-written `export const frontmatter`,
      plus a combining-mark identifier case on the Rust side. Cover
      `export { x as frontmatter } from './y'` before switching, since `has_named_runtime_export`
      ignores re-export forms.
- [ ] **Task 15 — `RUV-H12`:** change `ResolveGraphCache::dependencies` (`resolver.rs:144`) to store
      the whole `ResolvedDependencies` and return both halves on a hit. **Do not** fix this by
      making `record_module` skip alias-less entries — that hides the defect while leaving the link
      broken. **Tests:** extend `shared_graph_cache_reuses_source_reads_across_routes`
      (`resolver.rs:2658`), which today asserts only entry counts, so the shared module carries a
      `tsconfig` alias import and both routes assert equal `dependency_aliases`; plus a test that
      the persisted entry's aliases are non-empty.
- [ ] **Task 16 — `RUV-H13`:** apply `substitute_public_env` to the `js|mjs|cjs` fast-path result in
      `compiler.rs:443`, before constructing the `CompiledModule`. It already short-circuits on
      `!code.contains(MARKER)`, so the cost is one substring scan per module. **Tests:** a Rust test
      over a `.mjs` source containing `import.meta.env.RUVYXA_PUBLIC_X`; better, an `importMetaEnv`
      fixture section listing the extensions that must be substituted, replayed from both graphs.
- [ ] **Task 17 — `RUV-H14`:** delete `resolve_relative_import` (`ruvyxa_graph/src/lib.rs:1131`) and
      call the bundler's resolver. `ModuleCache::aliases()` already depends on
      `ruvyxa_bundler::resolver`, so the crate boundary is not the obstacle; if the entry point is
      not public enough, export `resolve_file_candidate` and `PROBE_EXTENSIONS`. **Tests:** a
      `./db.config` case backed by `db.config.ts` and a `./queue` case backed by `queue.mjs`, both
      asserted present in `reachable_project_modules` with a `fetch(` inside them keeping the route
      SSR; better, a third replay of `tests/fixtures/module-resolution-conformance.json`'s
      `fileProbe` section from this crate. **Expect previously-hidden RUV1007/1008/1010 diagnostics
      on real projects** — correct but noisy on the first run; say so in the report.
- [ ] **Task 18 — `RUV-H15`:** give the Rust response timeout headroom over the worker's. Keep the
      value written into `WORKER_TIMEOUT_ENV` and wait `response_timeout + WORKER_TIMEOUT_GRACE` on
      the Rust side, so the worker's own watchdog answers first with an ordinary `ok: false` frame.
      Return a typed error from `Worker::send` distinguishing timeout from transport failure, so
      `replace_failed_worker` runs only when the channel actually closed. Bound the new leniency:
      replace the worker after N consecutive timeouts. **Tests:** a stub worker answering request A
      after a delay longer than the Rust timeout and B immediately — assert B succeeds and
      `pool.workers[0]` is the same `Arc`; plus a test that the Rust deadline is strictly greater
      than the env value.
- [ ] **Task 19 — `RUV-H16`:** add a `cancel { id }` variant to `WorkerRequest`
      (`worker_protocol.rs:33`), send it from `WorkerBodyStream::drop` and from a `Drop` guard
      around the non-streaming `Worker::send` await via the existing non-blocking `try_send`, and on
      the worker side keep an `AbortController` per in-flight id, abort on `cancel`, and pass its
      `signal` into the `Request` the route handler receives. Keep the pending entry until a
      terminal frame so `in_flight` stops under-reporting. **Tests:** a stub worker that streams
      forever — build the body, drop it, assert a `cancel` frame for that id; plus an integration
      test asserting the worker's `ping` reports `activeRequests: 0` after a dropped streamed
      response. Assert that ids are never reused, since the abort must not reach a retried request.
- [ ] **Task 20 — `RUV-H17`:** add `'maxWidth'` to `CONFIG_KEY_SCHEMA['config.image']`
      (`config-schema.mjs:69`), add `maxWidth: numberValue(image?.maxWidth)` to `imageValue`
      (`config-renderer.mjs:253`), and add `maxWidth: 3840` to the `authored` literal in
      `tests/packages/core/config-schema.test.ts`. Then close the ungated pair: emit
      `ProjectConfig`'s serde field set — and each nested options struct's — to
      `tests/fixtures/config-surface-conformance.json` from a Rust test, and assert
      `CONFIG_KEY_SCHEMA` equals it in **both** directions. **The new test may fail immediately on
      other fields; each such failure is another instance of this same finding, not a broken test**
      — report them rather than suppressing them.
- [ ] **Task 21 — `RUV-H18`:** in `scripts/check-cross-language-constants.mjs:236`, collect
      **every** declaration per name (`Map<string, Array<{file, value}>>`) instead of the first, and
      for `sameValue` entries require all Rust values and all JS values to normalize equal. Report
      the file list in the failure message. Replace the `found.has(name)` guard with de-duplication
      at comparison time. **Tests:** a unit test asserting a name declared in two JS files yields
      two entries, plus a negative test that a divergent second copy is reported. There is no test
      for this script today.

### Phase 3 gate

- [ ] Full battery, both languages, plus `node scripts/check-cross-language-constants.mjs`.

---

# Phase 4 — Medium

Fifty-six findings. Group them into these batches; each batch is one task, and no two batches in the
same row share a file.

- [ ] **Task 22 — Bundler front-end:** `BUNF-04`, `BUNF-05`, `BUNF-06`, `BUNF-07`.
- [ ] **Task 23 — Bundler back-end:** `BUNB-03`, `BUNB-04`, `BUNB-05`.
- [ ] **Task 24 — Server request path, security:** `DEVR-01`, `DEVR-05`.
- [ ] **Task 25 — Server lifecycle parity:** `DEVR-02`, `DEVR-11` — one task, because both are the
      same divergence from `standalone-server.ts` and both touch `serve_until_shutdown`.
- [ ] **Task 26 — Server request path, correctness:** `DEVR-04`, `DEVR-06`.
- [ ] **Task 27 — Worker and watcher:** `DEVC-03`, `DEVC-04`, `DEVC-05`.
- [ ] **Task 28 — Assets and documents:** `ASSET-01`, `ASSET-02`, `ASSET-05`.
- [ ] **Task 29 — Asset and image performance:** `ASSET-03`, `ASSET-04`.
- [ ] **Task 30 — CLI build:** `CLIB-02`, `CLIB-03`, `CLIB-05`, `CLIB-06`.
- [ ] **Task 31 — CLI build performance:** `CLIB-04`.
- [ ] **Task 32 — CLI config and commands:** `CLIC-02`, `CLIC-03`, `CLIC-04`.
- [ ] **Task 33 — Graph correctness:** `GMDT-04`, `GMDT-06` — both halves of `GMDT-06` (Rust and
      `compiler.mjs`) in this task.
- [ ] **Task 34 — Middleware ordering and diagnostics:** `GMDT-05`, `GMDT-08`.
- [ ] **Task 35 — Graph decomposition:** `GMDT-07`. **Land this as one commit with no logic
      change**, verified by `cargo test --workspace` being byte-identical in outcome. Tasks 17 and
      33 must be green first.
- [ ] **Task 36 — Runtime compiler parity:** `RTMC-04`, `RTMC-06`.
- [ ] **Task 37 — Runtime server:** `RTMS-03`, `RTMS-04`, `RTMS-05`, `RTMS-07`.
- [ ] **Task 38 — Core server transports:** `CORE-03`, `CORE-05`, `CORE-06`.
- [ ] **Task 39 — Test doubles:** `CORE-04`, `CORE-07`. Both widen a published surface — CHANGELOG
      entries required.
- [ ] **Task 40 — Auth and plugins:** `SEC-02`, `SEC-03`, `SEC-07`.
- [ ] **Task 41 — PWA and content:** `SEC-04`, `SEC-05`.
- [ ] **Task 42 — Realtime client:** `SEC-06`.
- [ ] **Task 43 — Vercel ISR:** `ADP-03`.
- [ ] **Task 44 — CI and release gates:** `DEP-02`, `DEP-03`, `DEP-04`, `DEP-05`, `DEP-06`.

Each Phase 4 task follows the same shape as Phase 1–3: write the failing test named in the finding's
**Required tests** field, run it and watch it fail, apply the fix named in the finding's
**Recommended fix** field, run it again, then run the narrowest suite that covers the changed files.
The finding entry in `SYSTEM_AUDIT_REPORT.md` carries the quoted code, the exact line numbers, the
regression risk to watch, and the test to write — read it before starting the task.

### Phase 4 gate

- [ ] Full battery, both languages, plus `pnpm release:validate` and `pnpm pack:smoke`.

---

# Phase 5 — Low

Sixty-two findings, batched by file ownership. Same task shape as Phase 4.

- [ ] **Task 45:** `BUNF-08`, `BUNF-09`, `BUNF-10`.
- [ ] **Task 46:** `BUNB-06`, `BUNB-07`.
- [ ] **Task 47:** `DEVR-07`, `DEVR-08`, `DEVR-09`, `DEVR-10`.
- [ ] **Task 48:** `DEVC-06`, `DEVC-07`, `DEVC-08`, `DEVC-09`, `DEVC-10`.
- [ ] **Task 49:** `ASSET-06`, `ASSET-07`, `ASSET-08`.
- [ ] **Task 50:** `ASSET-09` — **both languages in this task** (`html_document.rs` and
      `entry-templates.mjs`), or the two hosts emit different bytes for one input. `ASSET-10`,
      `ASSET-11`, `ASSET-12`.
- [ ] **Task 51:** `CLIB-07`, `CLIB-08`, `CLIB-09`.
- [ ] **Task 52:** `CLIC-05`, `CLIC-06`, `CLIC-07`, `CLIC-08`, `CLIC-09`, `CLIC-10`, `CLIC-11`.
- [ ] **Task 53:** `GMDT-09`, `GMDT-10`, `GMDT-11`, `GMDT-12`, `GMDT-13`.
- [ ] **Task 54:** `RTMC-07`, `RTMC-08`.
- [ ] **Task 55:** `RTMS-06`, `RTMS-08`, `RTMS-09`.
- [ ] **Task 56:** `CORE-08`, `CORE-09`, `CORE-10`.
- [ ] **Task 57:** `SEC-08`, `SEC-09`, `SEC-10`, `SEC-11`.
- [ ] **Task 58:** `ADP-04`, `ADP-05`, `ADP-06`.
- [ ] **Task 59:** `DEP-07`, `DEP-08`, `DEP-09`, `DEP-10`, `DEP-11`, `DEP-12`, `DEP-13`, `DEP-14`,
      `DEP-15`. `DEP-08` and `DEP-15` each surface a batch of new sites on their first run; land
      each as its own change with the allowlist populated deliberately, not folded into another.

### Phase 5 gate

- [ ] Full battery. Then `pnpm verify:reproducible` (newly wired in Task 44) against
      `examples/deploy-smoke`.

---

# Findings that need a decision, not a fix

Four findings are **SPECULATIVE** and must be confirmed or killed before any code changes. Do not
fix them blind.

- [ ] **`DEVC-10`** — needs a stub worker that never reads stdin. `worker-pool.mjs` uses `readline`
      on a resumed stdin and never pauses it, which probably kills this finding. Confirm, then fix
      or close.
- [ ] **`RTMS-09`** — needs a build carrying two copies of `request-context.mjs`. Confirm the load
      order can invert before changing the module's contract.
- [ ] **`CORE-11`** — needs a browser. Scroll, navigate, press Back, observe. A partial
      `scrollRestoration = 'manual'` implementation is **worse** than the current behaviour, so do
      not start unless the finding is confirmed and the full manual path is in scope.
- [ ] **`ASSET-10`** — needs a shared host with a world-writable `/tmp`. The `tempfile` fix is cheap
      and correct regardless, so this one may be fixed without confirming the exploit; but
      `tempfile` moves from a dev-dependency to a dependency, which the pinning rules must be
      checked against.

One finding is **blocked on external documentation**: `RUV-H6` step 4 (Netlify header precedence).
One is **deliberately not fixed by guess**: the Firebase half of `RUV-H3`.

---

# Self-review

**Spec coverage.** All 141 findings appear: 5 Critical in Tasks 1–2 and 5–7, 18 High in Tasks 3–4
and 8–21, 56 Medium in Tasks 22–44, 62 Low in Tasks 45–59. Four SPECULATIVE findings are
additionally listed under "need a decision" — `DEVC-10` and `RTMS-09` appear in Tasks 48 and 55 and
must not be started until confirmed; `CORE-11` appears in no task by design; `ASSET-10` appears in
Task 50.

**Cross-language pairs are single tasks.** Verified for `RUV-H11` (Task 3), `RUV-C1` (Task 5),
`RUV-C2` (Task 6), `RUV-H10` (Task 14), `GMDT-06` (Task 33), `ASSET-09` (Task 50).

**File collisions.** No two tasks inside one phase edit the same file. Task 35 (the `ruvyxa_graph`
split) is explicitly sequenced after Tasks 17 and 33, which both edit `ruvyxa_graph/src/lib.rs`.

**Ordering hazard.** Task 2 and Task 3 both change emitted whitespace in `compiler.mjs`; Task 2's
fixture cases assert values that Task 3's indentation fix also affects. Task 2 must land first, and
Task 3's fixture additions must be written against Task 2's output.

## Phase 6 — the follow-up queue

Everything below was **found while fixing something else**, so none of it is in the audit's numbered
findings. Each item is either a residual the implementing agent could not reach without stepping
into another agent's files, or a new defect the fix exposed. This list is the reason the programme
can be declared done without pretending these do not exist.

### Carried by a file-ownership boundary, not by difficulty

- [ ] **F-01 — Single-source the plugin registration normaliser.** Not attempted 2026-08-30; see the
      section at the end for why and what it needs. `plugin-harness.ts` and
      `runtime/plugin-http.mjs` now state the rules twice, each commenting the other. The move needs
      `plugin-http.mjs`, `sync-shared-runtime.mjs` (`SYNCED_MODULES`), and the registration lists in
      one change. Carry `RESERVED_FRAMEWORK_PATHS` and the `normalizeRealtime`/`normalizePresence`
      range checks along with it — both were deliberately left out to avoid a third ungated copy.
- [ ] **F-02 — Rename the route-extension constant and register it.** The JavaScript mirror is
      `componentExtensions` rather than `ROUTE_COMPONENT_EXTENSIONS` purely so
      `check-cross-language-constants.mjs` does not fail an unregistered pair. Rename both halves
      and add the registry entry; the shared fixture holds them level meanwhile.
- [ ] **F-03 — `adapter-static`'s protected list is not registered with the cross-language gate.**

### New defects the fixes exposed

- [ ] **F-04 — `resolve_package_relative` joins drive-relative paths on Windows.** `C:` is read as a
      package name plus a relative path and lands back on the same file. Latent today because the
      project-root probe that masked it is gone, not because the join is right.
- [ ] **F-05 — Deployed builds answer CORS preflights before the rate limiter**, while `dev`/`start`
      now charge a token for one. `rateLimit.max` therefore means slightly different things per
      deployment. Wants a row in `tests/fixtures/rate-limit-conformance.json`, which already holds
      the two hosts level on everything else.
- [ ] **F-06 — `SOURCE_DATE_EPOCH` and the PWA cache name are in direct conflict.** Real and
      unfixed; it will redden the new reproducibility CI lane the moment a fixture enables `pwa`.
      Investigated 2026-08-30 and it is a decision, not a defect — see the section below.
- [ ] **F-07 — Start-time rollback recovery** (from Task 63's scope, never dispatched).

### Untested corners left behind by a fix

- [ ] **F-08 — `prefixed_path`'s `"/"` branch is untested in combination with a query.** The obvious
      fixture route (`/[lang]`) breaks the table for an unrelated reason: one dynamic segment
      matches `/about`, so the handler replay never reaches the redirect while the native replay
      does. Reason recorded in the fixture's `$routesNote`.
- [ ] **F-09 — Query bytes are verbatim on the native host and `URL.search`-normalised on the
      deployed one.** They differ for characters `URL` percent-encodes. No case depends on it yet;
      settling it means picking a normalisation rather than discovering one.
- [ ] **F-10 — `render_pipeline.rs`'s synthesised `"app/layout"` for `layout.jsx`** is now
      resolvable but has no test of its own.
- [ ] **F-11 — Retry-After rounding parity** between the two hosts.
- [ ] **F-12 — `examples/demo` should actually use a `~/*` alias**, so CI exercises `RUV-H12`
      instead of trusting a unit test.

### Documentation debt

- [ ] **F-13 — `ARCHITECTURE.md` and CHANGELOG for the cancel frame and `request.signal`.**
- [ ] **F-14 — Thai mirror of `docs/en/20-platform-adapter-guide.md`.**

### Found while re-verifying the tree

- [ ] **F-15 — A `.ruvyxa` directory written by an earlier binary was reused by the current one and
      failed the build.** Evidence: with the pre-existing `examples/demo/.ruvyxa` in the tree,
      `cd examples/demo && ruvyxa build --root .` died with
      `Failed to read route entry .\examples\demo\app\page.tsx while recording its Flight     capability`
      — a repo-root-relative route file joined onto a root of `.`. `rm -rf .ruvyxa` fixed it, and
      the same repo-root-then-project-root sequence no longer reproduces, because every route path
      the current binary writes into `cache/bundler/client-routes` and `client-route-plans` is
      absolute. So the stale cache held a _cwd-relative_ spelling that this binary no longer
      produces, and nothing in the derived cache identity noticed the shape had changed. The failure
      was only visible at all because the `unwrap_or_default()` at the Flight read site became a
      hard error in this programme; before that it was a silent `flight: false`. Settling it means
      deciding what in the cache identity must cover a stored path's _spelling_, not writing a
      version stamp — see the derived-cache-identity rule.

### Found while decomposing `ruvyxa_graph` (Task 35 / `GMDT-07`)

- [ ] **F-16 — Two doc comments in `crates/ruvyxa_graph` are attached to the wrong item.** Both were
      moved verbatim by the `GMDT-07` split, which was a pure move, and both are the same accretion
      failure the finding describes: a new item was inserted between a doc comment and the item it
      describes, and nothing gates that. (a) In `render.rs`, the block beginning "Parse the additive
      route hydration export while preserving boolean input" documents `parse_hydration_mode`, but
      it now runs straight into "Node built-ins an edge route may not reach" and the combined block
      is attached to `EDGE_UNAVAILABLE_BUILTINS`; `parse_hydration_mode` itself is undocumented. (b)
      In `validate.rs`, the block beginning "Every project module the routes reach that does **not**
      live in `app/`" documents `reachable_project_modules`, but it runs into the "Routes that would
      render one of `modules` on the server _and_ hydrate it in the browser" block and the pair is
      attached to `hydrated_routes_reaching`; `reachable_project_modules` is undocumented. Neither
      is a behaviour defect. Fixing them is splitting each block and moving the first half down to
      the item it names — deliberately not done inside a commit whose whole claim is that nothing
      changed. Worth asking whether a lint or a small check can see a doc block that changes subject
      mid-way, since this happened twice in one file.

### Left open by a landed fix

- [ ] **F-17 — the broken-pipe class is not closed.** Task 53 routed `ruvyxa_tui`'s 15 stdout print
      macros through `print_line`/`print_fragment`/`print_blank_line`, where `BrokenPipe` is quiet
      and every other failure still raises. They are 15 of **91**: `ruvyxa_cli` has 58 and
      `ruvyxa_dev_server` 18, and a closing pipe is felt by whichever site prints next, so
      `ruvyxa check | head -1` can still panic from `commands.rs`. Closing it site by site means 76
      edits; one process-level policy in `ruvyxa_cli`'s `main` is cheaper and is the only option
      that also reaches `ruvyxa_dev_server`. Related: `CLIC-04`, whose gate is a source scan for
      `println!` near `serde_json::to_string`.
- [ ] **F-18 — roughly twenty stale `crates/ruvyxa_graph/src/lib.rs` pointers.** GMDT-07 moved that
      file's contents into ten modules. `check-source-path-refs` and `check-doc-links` both still
      pass, because the path resolves — it is the _description_ that is now wrong. Known sites:
      `ARCHITECTURE.md` (two), `docs/{en,th}/18-documentation-scope-and-sources.md`,
      `packages/ruvyxa/runtime/compiler.mjs`, `packages/ruvyxa/runtime/worker-pool.mjs`, and several
      tests. One was load-bearing and is already fixed: `static-params-names.test.mjs` read `lib.rs`
      for `STATIC_PARAMS_EXPORTS` and was failing in the tree. Worth checking whether any other
      cross-language gate reads a moved file by path.
- [ ] **F-19 — a third pair of misattached doc comments.** Same shape as F-16, in
      `crates/ruvyxa_diagnostics/src/lib.rs`: the block beginning "Spell a path already in hand
      without its Windows extended-length prefix" sits on `label_with_code`, and its tail sentence
      sits on `without_verbatim_prefix`.
- [ ] **F-20 — a test reads `sourcesContent` with `unwrap_or_default()`.**
      `crates/ruvyxa_cli/src/tests.rs:1855` does `.as_str().unwrap_or_default()`. Pre-existing, and
      harmless while that fixture's sources all carry content — but BUNB-07 made `null` a legal
      entry meaning "content not available", so this is now the one place a missing source could be
      read as an empty line list. It is in test code, which `check-silent-defaults` deliberately
      does not scan, so no gate will ever say so.

### F-06 in detail: why it is a decision and not a defect

The obvious fix — project the build manifest to its content-bearing fields and drop `createdAtUnix`
and `timing` before hashing — was implemented and then reverted, because it reintroduces a bug the
current design documents avoiding.

`pwa()`'s own doc comment states the trade: the per-build cache name exists "so a change to an
unfingerprinted asset reaches a returning visitor", against the alternative of "a cache-first worker
serving an unfingerprinted asset from the install-time copy forever". `precache` holds
author-supplied paths such as `/logo.png`, not content-hashed URLs, so a content-only identity
cannot see that file change. A test already pins the current behaviour —
`derives the cache name from the build instead of a stamp` — and the reverted change turned it red,
which is the design defending itself.

`SOURCE_DATE_EPOCH` is not the answer either. It would make the timestamp _stable_ without making it
_mean_ anything, so the name would stop changing between two real deploys and the unfingerprinted
asset would go stale again.

What satisfies both is a digest of what the build actually emitted — the `assets` and `prerender`
trees under `outDir`, hashed at `build.onComplete` before the worker is written. That changes when
any output changes and does not change when none does, which is what both requirements are really
asking for. It costs one walk of the emitted tree per build and changes behaviour for every project
using `pwa()`, so it wants an owner's decision rather than being folded into a follow-up sweep.

### F-01 in detail: why it was left, and what it needs

Two modules of about seven hundred lines each — `packages/@ruvyxa/core/src/plugin-harness.ts` and
`packages/ruvyxa/runtime/plugin-http.mjs` — state the same registration rules and each comments the
other. That is a real duplication and the repository has a written rule against it.

It was still left alone, for three reasons that are about blast radius rather than difficulty.

The rules being moved are a security surface. `RESERVED_FRAMEWORK_PATHS` is what stops a plugin
claiming a framework endpoint, and `normalizeRealtime` and `normalizePresence` carry the range
checks beside it. A consolidation that lands the move but drops one range check produces a
registration that is accepted where it used to be refused, and nothing about the diff would say so.

The move is not self-contained. It needs `plugin-http.mjs`, `SYNCED_MODULES` in
`packages/ruvyxa/scripts/sync-shared-runtime.mjs`, and the three registration lists a runtime module
has to appear in — `package.json` `files`, `WORKER_RUNTIME_FILES` in
`crates/ruvyxa_cli/src/artifact_cache.rs`, and the standalone-copy tests. Missing any one of those
produces a module that is absent exactly where nobody looks, which is a failure mode this repository
has already recorded.

And a half-finished consolidation is worse than the duplication. Two copies that agree are a
maintenance cost; one copy plus a stale caller is a defect.

What it wants is its own change: move the rules, carry `RESERVED_FRAMEWORK_PATHS` and both range
checks with them in the same commit, update the three registration lists, and prove the result with
the plugin suites before deleting either copy — the same order `RTMS-08` used, where the copies were
shown identical before any of them was removed.
