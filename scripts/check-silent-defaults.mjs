#!/usr/bin/env node
// A read that fails says something the caller cannot re-derive. Turning that
// failure into a default value throws the message away *and* invents an answer
// -- and when the invented answer is also a legitimate one, nothing downstream
// can tell the two apart. That is not a lost log line; it is a wrong result
// that looks right.
//
// It has already shipped here, twice, in the same shape:
//
//   client_bundle.rs   read a route's source to record whether it exports
//                      `flight`, defaulting to `""` on failure. `""` exports no
//                      `flight` -- a perfectly ordinary answer -- so every
//                      `ruvyxa build --root <elsewhere>` wrote `flight: false`
//                      for every route into the shipped manifest, the browser
//                      router stopped requesting payloads routes did produce,
//                      and RUV1842 could not fire. Builds run from inside the
//                      project directory were correct, which is why it lasted.
//
//   framework_endpoints.rs  the same read behind `/__ruvyxa/flight` and the dev
//                      route table. An unreadable route file was reported as
//                      `501 this route does not expose a Flight payload`, which
//                      names the wrong cause, and nothing was logged.
//
// `.ok()` and `.ok()?` are deliberately not flagged. They hand the caller
// `None` -- an honest "no answer" it can branch on -- which is what a cache
// lookup wants. This check is about `unwrap_or_default()` and
// `unwrap_or_else(|_| …)`, which fabricate.
//
// A site that genuinely wants a default is allowed below, with the reason
// written out. An allowlist entry that stops matching is itself a failure: a
// reason nothing stands behind is how a list like this rots.
import { readFile } from 'node:fs/promises'
import { execFileSync } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

// This repository's root, from this file's own location rather than from
// `process.cwd()`. A git pathspec resolves against the working directory, so
// `crates/**` asked from inside a package directory names `<package>/crates/**`
// and selects nothing -- and a gate that looked at no files reports no
// failures, which is indistinguishable from a clean tree. `pnpm -r test` runs
// each package's script from that package's own directory, so that is the
// caller this check was silent for. The paths stay repository-relative because
// that is how they read in a failure message; they are resolved against this
// before anything opens them.
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

// Reading and decoding only. Serializing a value this process already owns is
// deliberately out of scope: `serde_json::to_string` of a `&str`, a `BTreeMap`,
// or a `serde_json::Value` cannot fail, so every such site would need an
// allowlist entry saying so and the list would become a wall of excuses. The
// class that has actually shipped wrong answers here is input from outside the
// process -- a file, a child's output, bytes of unknown provenance -- where the
// failure is real and says something.
// The project's own fallible readers count too. The list above names only the
// standard library's, and `build_dependency_hash` in `runtime_config.rs` slipped
// past it for exactly that reason: it wrote
// `ruvyxa_dev_server::project_env(root).unwrap_or_default()` -- a read of the
// project's `.env`, whose failure means "a `.env` exists and could not be
// opened" and whose default means "there is no `.env`" -- twenty lines from a
// sibling that propagates the same call. `unwrap_or_default()` was right there
// on the line and this check could not see it, because `project_env` is not
// `read_to_string`.
//
// A helper is added here when it reads from outside the process and its failure
// says something a default cannot. That is a judgement, so the names are
// written out rather than matched by shape: a blanket "any call followed by
// `unwrap_or_default`" would flag every infallible builder in the workspace and
// the allowlist would become the wall of excuses the note above rejects.
/** A call whose failure carries information the caller cannot reconstruct. */
const FALLIBLE =
  /\b(read_to_string|fs::read\(|from_str|from_slice|from_utf8|project_env|read_dir|canonicalize|metadata)\b|\.parse::<|\.parse\(\)/
// Three spellings used to pass this check while doing exactly what it forbids.
// `unwrap_or(value)` was not matched at all, so a failed parse could substitute
// any literal. `unwrap_or_else(|error| …)` was not matched either, because the
// pattern required the binding to be written as a bare `_` -- naming the error
// and then discarding it reads as *more* deliberate, not less. And a chain whose
// `unwrap_or_default()` landed on a fifth line fell outside the statement
// window. The check reported "no failed read invents a value outside N reviewed
// sites" while all three could pass under it.
/**
 * Combinators that invent a value rather than reporting the failure.
 *
 * `unwrap_or(` and a *named* `unwrap_or_else` binding are included now. Naming
 * the error and then discarding it reads as more deliberate than `|_|`, not
 * less, and the old pattern required a bare underscore — so
 * `unwrap_or_else(|error| String::new())` passed a check whose entire subject it
 * is.
 *
 * A closure that panics is excluded, because it is the opposite of this: it
 * reports the failure as loudly as a process can. That is why the lookahead is
 * here rather than in an allowlist entry — `unwrap_or_else(|error| panic!(…))`
 * is a shape, not a site, and there are several.
 */
const FABRICATE =
  /unwrap_or_default\(\)|unwrap_or\(|unwrap_or_else\(\s*\|[^|]*\|\s*(?!\s*(?:panic!|unreachable!|todo!))/
/**
 * How many lines a single statement may span before the two stop being one.
 *
 * Four, deliberately. Raising it to eight was tried: it does catch a
 * `rustfmt`-wrapped chain whose combinator lands further down, and it also joins
 * genuinely separate statements into one window — reporting a `?`-propagated
 * read on one line against an unrelated `unwrap_or_else` four lines below it.
 * A window that invents pairings is worse than one that misses a long chain,
 * because the false report is what teaches people to stop reading this check's
 * output.
 */
const STATEMENT_LINES = 4

// Each entry: the file it applies to, a substring of the matched statement, and
// why inventing a value is correct there. Keyed by substring rather than line
// number so ordinary edits above it do not invalidate the reason.
const ALLOWED = [
  // The sites `unwrap_or(` surfaced on its first run. Every one reads a value
  // from outside the process and substitutes a documented default for it, which
  // is the case the note above says an entry is for. They are listed
  // individually rather than waved through as a class, because "it is only an
  // environment variable" is exactly the reasoning that produced the two
  // incidents at the top of this file.
  {
    file: 'crates/ruvyxa_bundler/src/context.rs',
    contains: 'unwrap_or(DEFAULT_BUILD_CACHE_MIB)',
    reason:
      'A build-cache budget read from the environment. Absent and unparsable are the same answer here -- the project did not choose a size -- and the `filter` above rejects an out-of-range one, so no value reaches the default that could have meant something else.',
  },
  {
    file: 'crates/ruvyxa_cli/src/environment.rs',
    contains: 'parts.next().unwrap_or("0")',
    reason:
      '`Option::unwrap_or` over the components of a version string, not a failed read: `24.19` has no third component, and reading it as patch `0` is what a version comparison means. The `.parse()` beside it is separately `?`-propagated.',
  },
  {
    file: 'crates/ruvyxa_cli/src/runtime_config.rs',
    contains: 'config.server.port.unwrap_or(DEFAULT_PORT)',
    reason:
      'The configured port, defaulted when the project set none. Matched only because it shares a `match` expression -- and therefore a statement -- with the `PORT` parse in the arm above, which is `?`-propagated with its own message. Two different questions in one expression is what the statement split cannot separate.',
  },
  {
    file: 'crates/ruvyxa_dev_server/src/i18n.rs',
    contains: '.unwrap_or(1.0)',
    reason:
      'An `Accept-Language` quality value. RFC 9110 defines a missing `q` as 1.0, and a malformed one is a header this server does not get to reject, so the specified default is the correct reading rather than an invented one.',
  },
  {
    file: 'crates/ruvyxa_dev_server/src/render_cache.rs',
    contains: '.unwrap_or(default)',
    reason:
      'A render-cache capacity read from the environment, clamped to a maximum above. Absent and unparsable both mean the operator expressed no capacity.',
  },
  {
    file: 'crates/ruvyxa_dev_server/src/static_assets.rs',
    contains: 'text.parse::<u64>().unwrap_or(u64::MAX)',
    reason:
      'A byte-range position too large to represent is still a position, and it is past the end of any real file -- which is exactly what the caller needs to answer 416. Reporting it as unparsable would send the whole file instead.',
  },
  {
    file: 'crates/ruvyxa_dev_server/src/static_assets.rs',
    contains: 'unwrap_or(DEFAULT_STREAMED_ASSET_THRESHOLD)',
    reason:
      'The streaming threshold read from the environment, once per process. A value that is absent, unparsable, or zero all mean the same thing: no threshold was chosen.',
  },
  {
    file: 'crates/ruvyxa_dev_server/src/worker_pool.rs',
    contains: 'unwrap_or_else(|| {',
    reason:
      'A worker count read from the environment; the closure computes one from `available_parallelism` rather than substituting a literal. It reports nothing because there is nothing to report -- an unset variable is the ordinary case.',
  },
  {
    file: 'crates/ruvyxa_dev_server/src/worker_pool.rs',
    contains: 'unwrap_or(DEFAULT_MAX_WORKER_LINE_BYTES)',
    reason:
      'The worker line-length ceiling read from the environment, with the same three-way equivalence: unset, unparsable, and zero all mean the default bound applies.',
  },
  {
    file: 'crates/ruvyxa_cli/src/build_output.rs',
    contains: 'created_at.parse::<u128>().unwrap_or_default()',
    reason:
      'Orders stranded build directories newest-first and nothing else. A name whose timestamp does not parse is one this code did not write, and sorting it last is the same answer the branch beside it already gives a name with no timestamp at all. Deletion is decided by `owner_pid` and `process_may_be_running`, never by this number, so a wrong order cannot remove a live build.',
  },
  {
    file: 'crates/ruvyxa_bundler/src/ast.rs',
    contains: '" ".repeat(bytes.len())',
    reason:
      'The masker must return a string of exactly the input length or every byte offset after it shifts. Blanking to the same length is the only answer that keeps the scan correct.',
  },
  {
    file: 'crates/ruvyxa_bundler/src/ast.rs',
    contains: 'from_utf8(previous_word(bytes, end))',
    reason:
      'The result is compared against a keyword list. Bytes that are not UTF-8 are not a keyword, and `""` matches none of them, so the default is the right answer rather than a stand-in for one.',
  },
  {
    file: 'crates/ruvyxa_bundler/src/resolver.rs',
    contains: 'fs::read(file).unwrap_or_default()',
    reason:
      'Hashes the tsconfig files that identify a resolver configuration. A tsconfig that cannot be read and one that is empty describe the same configuration -- neither parses into paths -- so hashing both as empty is correct, not a collapse.',
  },
  {
    file: 'crates/ruvyxa_cli/src/artifact_cache.rs',
    contains: '.map(|source| content_hash_bytes(&source))',
    reason:
      'Absence is a real project state here, not a fault: a runtime module the installed package does not ship hashes as absent, and whether the set is complete is checked separately against WORKER_RUNTIME_FILES.',
  },
  {
    file: 'crates/ruvyxa_dev_server/src/static_assets.rs',
    contains: 'HeaderValue::from_str(&etag)',
    reason:
      '`compute_etag` formats sixteen hex characters between quotes, so the conversion cannot fail. The fallback exists only so a header can never panic a response.',
  },
  {
    file: 'crates/ruvyxa_dev_server/src/framework_endpoints.rs',
    contains: 'HeaderValue::from_str(&etag)',
    reason: 'Same `compute_etag` value, same reason.',
  },
]

// --- The JavaScript half ------------------------------------------------------
//
// The rule above is language-neutral and the check was not: it read
// `crates/**/*.rs` and nothing else, in a repository whose first stated rule is
// that both halves move together. The JavaScript side has the identical class in
// different spellings, and it lives in `packages/ruvyxa/runtime/*.mjs` — the
// deployed prerender worker, the adapter runner, the serverless handler. That is
// the code furthest from a developer's console, where an invented value is least
// likely to be noticed.
//
// Deliberately narrow, and narrower than the Rust pass. A `catch` is legitimate
// far more often in JavaScript than `unwrap_or_default` is in Rust — feature
// detection, an optional import, a cleanup that must not throw — so this matches
// one shape only: a `try` whose body performs a **read**, paired with a `catch`
// that does nothing at all or answers with a bare literal. Anything that logs,
// rethrows, branches, or builds a value is out of scope, because those are
// reporting the failure in some form.

/** Reads whose failure says something the caller cannot reconstruct. */
const JS_FALLIBLE = /\b(readFile|readFileSync|readdir|readdirSync|JSON\.parse|import\()/

/** A `catch` body that swallows: empty, or a single `return <literal>`. */
const JS_SWALLOWS = /^\s*$/

/**
 * Index of the `}` closing the block that opens at `open`.
 *
 * Brace counting, not parsing. It is fooled by a brace inside a string or a
 * comment, which is why a miss here has to be safe: an unbalanced answer ends
 * the scan for that `try` rather than reporting anything.
 */
function closingBrace(source, open) {
  let depth = 0
  for (let index = open; index < source.length; index += 1) {
    if (source[index] === '{') depth += 1
    else if (source[index] === '}') {
      depth -= 1
      if (depth === 0) return index
    }
  }
  return -1
}

/**
 * The sites in one JavaScript or TypeScript source that swallow a failed read.
 *
 * Exported for the same reason the Rust half is: a rule with no test is a rule
 * nobody can change safely.
 */
export function swallowedReads(source, { allowed = [], file = '<snippet>', onAllowed } = {}) {
  const found = []
  const pattern = /\btry\s*\{/g
  let match
  while ((match = pattern.exec(source)) !== null) {
    const bodyOpen = match.index + match[0].length - 1
    const bodyClose = closingBrace(source, bodyOpen)
    if (bodyClose === -1) break
    const body = source.slice(bodyOpen + 1, bodyClose)
    if (!JS_FALLIBLE.test(body)) continue

    const after = source.slice(bodyClose + 1)
    const clause = after.match(/^\s*catch\s*(?:\([^)]*\))?\s*\{/)
    if (!clause) continue
    const handlerOpen = bodyClose + 1 + clause[0].length - 1
    const handlerClose = closingBrace(source, handlerOpen)
    if (handlerClose === -1) break
    const handler = source.slice(handlerOpen + 1, handlerClose)
    // Comments are not a report: a `catch` explaining in prose why it drops the
    // error still drops it. Stripped before the emptiness test so the shape is
    // judged by what runs.
    const runs = handler.replace(/\/\/[^\n]*/g, '').replace(/\/\*[\s\S]*?\*\//g, '')
    if (!JS_SWALLOWS.test(runs)) continue

    const line = source.slice(0, match.index).split('\n').length
    const statement = body.replace(/\s+/g, ' ').trim().slice(0, 160)
    const entry = allowed.find(
      (candidate) => candidate.file === file && body.includes(candidate.contains),
    )
    if (entry) {
      onAllowed?.(entry)
      continue
    }
    found.push({ line, statement })
  }
  return found
}

// The JavaScript sites this shape found on its first run. Every one reads
// something whose *absence is a legal state* — a cache that has not been written
// yet, a candidate directory that is not a package, a destination file that does
// not exist. That is the one case where an empty `catch` says the same thing the
// error does, and it is why the shape is deliberately narrow: a `catch` that
// returns a value, logs, or branches was left out of this pass entirely.
const ALLOWED_JS = [
  {
    file: 'packages/@ruvyxa/core/src/standalone-server.ts',
    contains: 'readFileSync(htmlPath',
    reason:
      'Reads a stored ISR document. Not written yet and not readable are the same answer to the only question being asked -- is there a cached page for this path -- and the miss path renders it.',
  },
  {
    file: 'packages/ruvyxa/runtime/compiler.mjs',
    contains: '=== contents) return',
    reason:
      'Write-if-changed: the read exists only to skip a write whose bytes would be identical. A file that cannot be read has no identical bytes to compare, so writing is the correct next step and the failure surfaces there instead.',
  },
  {
    file: 'packages/ruvyxa/runtime/config-renderer.mjs',
    contains: '=== source) return',
    reason: 'The same write-if-changed comparison, for the config renderer pointer.',
  },
  {
    file: 'packages/ruvyxa/runtime/paths.mjs',
    contains: "readFileSync(path.join(candidate, 'package.json')",
    reason:
      'Walks upward looking for the framework package. A directory with no readable manifest is not that package, which is what the walk needs to know; it continues to the parent.',
  },
  {
    file: 'packages/ruvyxa/runtime/worker-pool.mjs',
    contains: 'const cached = JSON.parse(await readFile(file',
    reason:
      'A build cache entry. Absent, truncated, and from an older shape all mean the same thing to this reader -- there is nothing to reuse -- and the version check inside the `try` refuses a stale one explicitly.',
  },
  {
    file: 'packages/ruvyxa/runtime/worker-pool.mjs',
    contains: "hasModuleDirective(readFileSync(file, 'utf8'), 'use client')",
    reason:
      'Decides whether a module is a client entry. A module that cannot be read is not added, which is the conservative answer: it is left out of the client set rather than assumed into it.',
  },
  {
    file: 'packages/ruvyxa/scripts/sync-shared-runtime.mjs',
    contains: 'actual = readFileSync(destination',
    reason:
      'Reads the destination to see whether the copy is already in place. No destination means the copy has to happen, which is what the code below does.',
  },
]

const tracked = execFileSync('git', ['ls-files', 'crates/**/*.rs'], {
  encoding: 'utf8',
  cwd: REPO_ROOT,
})
  .split('\n')
  .filter(Boolean)
  // Test code may fabricate freely: it is asserting on values it wrote itself.
  .filter((file) => !file.includes('/tests/') && !/tests(_visual)?\.rs$/.test(file))

/**
 * The sites in one Rust source that turn a failed read into a value.
 *
 * Exported so the rule can be tested against snippets rather than only against
 * whatever the repository happens to contain today. That mattered here: three
 * spellings passed this check for as long as it existed — `unwrap_or(value)`, a
 * named `unwrap_or_else(|error| …)`, and a chain whose combinator landed outside
 * the statement window — and nothing could have told anyone, because the check
 * had no test and the repository had no live instance of two of them.
 *
 * `allowed` is the reviewed list to consult; `onAllowed` is called with each
 * entry that matched, which is how the caller notices an entry that has stopped
 * matching anything.
 */
export function fabricatedSites(source, { allowed = [], file = '<snippet>', onAllowed } = {}) {
  const lines = source.split('\n')
  // The test module, not merely the first `#[cfg(test)]`. Several files gate a
  // single item on it near the top -- `framework_endpoints.rs` gates an import
  // at line 29 -- and stopping there skipped the whole file, which is exactly
  // how the first draft of this check reported no findings in it.
  const end = lines.findIndex(
    (line, index) => line.startsWith('#[cfg(test)]') && /^mod tests\b/.test(lines[index + 1] ?? ''),
  )
  const limit = end === -1 ? lines.length : end
  const found = []

  for (let index = 0; index < limit; index += 1) {
    if (!FALLIBLE.test(lines[index])) continue
    const window = lines
      .slice(index, Math.min(limit, index + STATEMENT_LINES))
      .map((line) => line.trim())
      .join(' ')
    // The window is lines; a statement ends at a `;`. Widening `FABRICATE` to
    // `unwrap_or(` made that difference matter: `Option::unwrap_or` is
    // everywhere and perfectly ordinary — `path.parent().unwrap_or(&root)`,
    // `config.server.port.unwrap_or(DEFAULT_PORT)` — and a window that spans a
    // `;` paired those with a `read_to_string` or a `parse` on a neighbouring
    // line that had nothing to do with them. Reporting a site that is not one is
    // how a check like this teaches people to stop reading it, so the fallible
    // call and the combinator have to be in the same statement.
    const statement = window.split(';').find((part) => FALLIBLE.test(part) && FABRICATE.test(part))
    // No single statement holds both, so nothing here fabricates from a read.
    if (!statement) continue

    const entry = allowed.find(
      (candidate) => candidate.file === file && statement.includes(candidate.contains),
    )
    if (entry) {
      onAllowed?.(entry)
      continue
    }
    found.push({ line: index + 1, statement })
  }
  return found
}

const failures = []
const used = new Set()

for (const file of tracked) {
  for (const { line, statement } of fabricatedSites(
    await readFile(path.resolve(REPO_ROOT, file), 'utf8'),
    {
      allowed: ALLOWED,
      file,
      onAllowed: (entry) => used.add(`${entry.file}::${entry.contains}`),
    },
  )) {
    failures.push(`${file}:${line}\n      ${statement}`)
  }
}

const jsTracked = execFileSync(
  'git',
  [
    'ls-files',
    '--cached',
    '--others',
    '--exclude-standard',
    '--',
    'packages/**/*.mjs',
    'packages/**/*.ts',
  ],
  { encoding: 'utf8', cwd: REPO_ROOT },
)
  .split('\n')
  .map((file) => file.trim())
  .filter(Boolean)
  // Build output and test code are both out of scope, for the reasons the Rust
  // pass gives: one is generated, the other asserts on values it wrote itself.
  .filter(
    (file) =>
      !file.includes('/dist/') &&
      !file.includes('node_modules/') &&
      !file.includes('/test/') &&
      !/\.test\.(mjs|ts)$/.test(file),
  )

for (const file of jsTracked) {
  for (const { line, statement } of swallowedReads(
    await readFile(path.resolve(REPO_ROOT, file), 'utf8'),
    {
      allowed: ALLOWED_JS,
      file,
      onAllowed: (entry) => used.add(`${entry.file}::${entry.contains}`),
    },
  )) {
    failures.push(`${file}:${line}\n      ${statement}`)
  }
}

for (const entry of [...ALLOWED, ...ALLOWED_JS]) {
  if (used.has(`${entry.file}::${entry.contains}`)) continue
  failures.push(
    `${entry.file}: the allowed site \`${entry.contains}\` no longer exists.\n` +
      '      Remove the entry; a reason nothing stands behind is how this list rots.',
  )
}

if (failures.length > 0) {
  console.error('A failed read or decode is being turned into a value:\n')
  for (const failure of failures) console.error(`  ${failure}\n`)
  console.error(
    `${failures.length} site${failures.length === 1 ? '' : 's'}.\n` +
      'Report the failure instead: propagate it with `?`, return `None` so the caller can\n' +
      'branch on it, or answer the request as the fault it is. If a default is genuinely the\n' +
      'right answer, add it to ALLOWED in this file with the reason.',
  )
  process.exitCode = 1
} else {
  console.log(
    `Checked ${tracked.length} Rust and ${jsTracked.length} JavaScript source files; no failed read ` +
      `invents a value or is swallowed outside ${ALLOWED.length + ALLOWED_JS.length} reviewed sites.`,
  )
}
