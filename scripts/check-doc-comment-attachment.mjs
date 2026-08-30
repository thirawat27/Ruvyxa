#!/usr/bin/env node
// A Rust doc comment split by an attribute reads out of order.
//
// `///` lines become `#[doc]` attributes and concatenate in source order, so
// writing
//
//   /// A no-op on other platforms.
//   #[must_use]
//   /// Spell a path without its Windows extended-length prefix.
//
// compiles, and rustdoc then shows "A no-op on other platforms." as the item's
// summary line -- the sentence the author wrote as a footnote is what every
// index page, every search result, and every `use` completion displays. The
// item's real summary is buried in the body.
//
// It happened three times before this check existed, twice inside one commit
// whose whole claim was that nothing changed:
//
//   ruvyxa_graph/src/render.rs     a doc block for `parse_hydration_mode` ran
//                                  into the next item's and the pair attached
//                                  to `EDGE_UNAVAILABLE_BUILTINS`.
//   ruvyxa_graph/src/validate.rs   the same, for `reachable_project_modules`.
//   ruvyxa_diagnostics/src/lib.rs  one line stranded above `#[must_use]`, which
//                                  became the rendered summary of
//                                  `without_verbatim_prefix`.
//
// Only the third of those is mechanically visible, and it is the one this check
// sees: an attribute standing between two halves of one item's doc block. The
// first two are a doc block that changes subject halfway, which needs to know
// what the prose is about and is deliberately not attempted here.
//
// It errs toward silence, the same way `check-source-path-refs.mjs` does. A run
// is only reported when it ends on something that looks like an item
// declaration, so a `///` inside a macro body or a raw string is not read as a
// doc comment, and a file whose attribute brackets do not balance is skipped
// rather than guessed at.
import { readFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { execFileSync } from 'node:child_process'

const repoRoot = resolve(
  dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')),
  '..',
)

// What a doc block is allowed to end on. A run that ends on anything else is
// not an item's preamble and is left alone.
const ITEM_START =
  /^(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?(?:fn|struct|enum|union|trait|impl|type|mod|use|const|static|macro_rules!|let)\b/

const files = execFileSync(
  'git',
  ['ls-files', '--cached', '--others', '--exclude-standard', '--', '*.rs'],
  { encoding: 'utf8', cwd: repoRoot },
)
  .split('\n')
  .map((file) => file.trim())
  .filter(Boolean)

/**
 * Find every doc block in one file that an attribute stands inside.
 *
 * Walks the lines once. A run of `///` opens a block; an attribute inside that
 * run is remembered rather than reported, because an attribute *after* the last
 * `///` is the ordinary shape and only a `///` that follows one is the defect.
 * Anything else closes the run.
 *
 * @param {string} source raw file text
 * @returns {Array<{line: number, attribute: string}>} one entry per split block
 */
function splitDocBlocks(source) {
  const lines = source.split('\n')
  const found = []

  let sawDoc = false
  let attribute = null
  let splitAt = null
  let depth = 0

  for (let index = 0; index < lines.length; index += 1) {
    const text = lines[index].trim()

    // Inside a multi-line attribute such as `#[cfg(all(
    if (depth > 0) {
      depth += countBrackets(text)
      continue
    }

    if (text.startsWith('///')) {
      if (attribute !== null && splitAt === null) splitAt = { line: index + 1, attribute }
      sawDoc = true
      continue
    }

    if (text.startsWith('#[') || text.startsWith('#![')) {
      if (sawDoc && attribute === null) attribute = text.slice(0, 48)
      depth = countBrackets(text)
      continue
    }

    // A blank line or a `//` aside does not end the preamble: both appear
    // between an attribute and the item it applies to.
    if (text === '' || text.startsWith('//')) continue

    // Anything else terminates the run. Report it only if it is an item, so a
    // `///` that was really the contents of a string or a macro is not read as
    // documentation.
    if (splitAt !== null && ITEM_START.test(text)) found.push(splitAt)
    sawDoc = false
    attribute = null
    splitAt = null
  }

  return found
}

/**
 * Net bracket depth a line contributes, ignoring brackets inside its strings.
 *
 * `#[doc = "a]b"]` balances only when the quoted `]` is skipped, and an
 * attribute whose depth is misread would swallow the rest of the file.
 *
 * @param {string} text one trimmed source line
 * @returns {number} opens minus closes outside any string literal
 */
function countBrackets(text) {
  let depth = 0
  let quote = null
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index]
    if (quote) {
      if (char === '\\') index += 1
      else if (char === quote) quote = null
      continue
    }
    if (char === '"') quote = '"'
    else if (char === '[') depth += 1
    else if (char === ']') depth -= 1
  }
  return depth
}

const failures = []

await Promise.all(
  files.map(async (file) => {
    const source = await readFile(join(repoRoot, file), 'utf8')
    if (!source.includes('///')) return
    for (const split of splitDocBlocks(source)) {
      failures.push(
        `${file}:${split.line}: doc comment resumes after \`${split.attribute}\`, ` +
          'so the lines above it become the rendered summary',
      )
    }
  }),
)

if (failures.length > 0) {
  failures.sort()
  console.error('Doc comments split by an attribute:\n')
  for (const failure of failures) console.error(`  ${failure}`)
  console.error(
    `\n${failures.length} doc block${failures.length === 1 ? ' is' : 's are'} interrupted.` +
      '\nMove the attribute below the whole doc comment, directly above the item it applies to.',
  )
  process.exitCode = 1
} else {
  console.log(`Checked ${files.length} Rust files; no doc comment is split by an attribute.`)
}
