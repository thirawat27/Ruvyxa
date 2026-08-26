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

// Reading and decoding only. Serializing a value this process already owns is
// deliberately out of scope: `serde_json::to_string` of a `&str`, a `BTreeMap`,
// or a `serde_json::Value` cannot fail, so every such site would need an
// allowlist entry saying so and the list would become a wall of excuses. The
// class that has actually shipped wrong answers here is input from outside the
// process -- a file, a child's output, bytes of unknown provenance -- where the
// failure is real and says something.
/** A call whose failure carries information the caller cannot reconstruct. */
const FALLIBLE =
  /\b(read_to_string|fs::read\(|from_str|from_slice|from_utf8)\b|\.parse::<|\.parse\(\)/
/** Combinators that invent a value rather than reporting the failure. */
const FABRICATE = /unwrap_or_default\(\)|unwrap_or_else\(\s*\|_\|/
/** How many lines a single statement may span before the two stop being one. */
const STATEMENT_LINES = 4

// Each entry: the file it applies to, a substring of the matched statement, and
// why inventing a value is correct there. Keyed by substring rather than line
// number so ordinary edits above it do not invalidate the reason.
const ALLOWED = [
  {
    file: 'crates/ruvyxa_bundler/src/ast.rs',
    contains: '" ".repeat(bytes.len())',
    reason:
      'The masker must return a string of exactly the input length or every byte offset after it shifts. Blanking to the same length is the only answer that keeps the scan correct.',
  },
  {
    file: 'crates/ruvyxa_bundler/src/ast.rs',
    contains: 'from_utf8(&bytes[start..=end])',
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

const tracked = execFileSync('git', ['ls-files', 'crates/**/*.rs'], { encoding: 'utf8' })
  .split('\n')
  .filter(Boolean)
  // Test code may fabricate freely: it is asserting on values it wrote itself.
  .filter((file) => !file.includes('/tests/') && !/tests(_visual)?\.rs$/.test(file))

const failures = []
const used = new Set()

for (const file of tracked) {
  const lines = (await readFile(file, 'utf8')).split('\n')
  // The test module, not merely the first `#[cfg(test)]`. Several files gate a
  // single item on it near the top -- `framework_endpoints.rs` gates an import
  // at line 29 -- and stopping there skipped the whole file, which is exactly
  // how the first draft of this check reported no findings in it.
  const end = lines.findIndex(
    (line, index) => line.startsWith('#[cfg(test)]') && /^mod tests\b/.test(lines[index + 1] ?? ''),
  )
  const limit = end === -1 ? lines.length : end

  for (let index = 0; index < limit; index += 1) {
    if (!FALLIBLE.test(lines[index])) continue
    const statement = lines
      .slice(index, Math.min(limit, index + STATEMENT_LINES))
      .map((line) => line.trim())
      .join(' ')
    if (!FABRICATE.test(statement)) continue

    const allowed = ALLOWED.find(
      (entry) => entry.file === file && statement.includes(entry.contains),
    )
    if (allowed) {
      used.add(`${allowed.file}::${allowed.contains}`)
      continue
    }
    failures.push(`${file}:${index + 1}\n      ${statement}`)
  }
}

for (const entry of ALLOWED) {
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
    `Checked ${tracked.length} Rust source files; no failed read invents a value outside ${ALLOWED.length} reviewed sites.`,
  )
}
