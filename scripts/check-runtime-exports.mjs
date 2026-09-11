/**
 * Fail when a `packages/ruvyxa/runtime/*.mjs` module exports a name nothing
 * reads.
 *
 * Knip is the repository's dead-code gate, and these modules are outside it by
 * construction: `knip.json` declares `runtime/*.mjs` as the `packages/ruvyxa`
 * entry points, because the Rust CLI resolves them by path and spawns or
 * imports them rather than importing them from a package specifier. An entry
 * point's exports are the boundary Knip measures everything else against, so
 * an export nobody imports is exactly what it cannot report — in the one place
 * that has no other reader either. `runtime/*.mjs` is not in the `ruvyxa`
 * package's `exports` map, so a name there is not public API by any route: it
 * is read by a sibling runtime module, by a test, by the Rust source that
 * writes it into generated code, or by nothing at all.
 *
 * It was nothing at all thirty-one times. Eight were found by hand and
 * un-exported — `ACTION_CONTENT_TYPES`, `actionContentType`, `isActionExport`
 * and `realtimeRouteChannel` in `action-runtime.mjs`, `CLIENT_VENDOR_PATH` and
 * `VENDOR_REGISTRY_GLOBAL` in `compiler.mjs`, `DEFAULT_FLIGHT_LIMIT` in
 * `flight.mjs`, `PACKAGE_EXPORT_CONDITIONS` in `package-exports.mjs` — and
 * that sweep left nothing behind to find the next one, so twenty-three were
 * still standing in six other modules when this was written. This is what
 * finds the thirty-second.
 *
 * Two things make the answer trustworthy rather than a grep:
 *
 * - **Declarations come from masked code.** `entry-templates.mjs` is a
 *   generator: its template literals contain `export const linkedModules = …`
 *   as *output text*, and a plain regex reads that as a declaration of this
 *   module. Everything here is read out of `maskNonCode` from
 *   `packages/ruvyxa/runtime/scanner.mjs`, which is the single owner of the
 *   question "is this offset code?" for the JavaScript half.
 * - **Usage counts a string, not a comment.** A name written into generated
 *   source, a manifest key, or a Rust literal is a real reader; a name in a
 *   doc comment is not. Comment-only lines are dropped and everything else is
 *   searched raw, which errs toward silence: a name mentioned only in prose
 *   goes unreported rather than failing a build on a false positive.
 *
 * A deliberately public export carries `@public` in its doc comment, which is
 * the tag `knip.json` already excludes — one convention, not two.
 *
 * Usage:
 *   node scripts/check-runtime-exports.mjs
 */

import { existsSync, readFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { maskNonCode } from '../packages/ruvyxa/runtime/scanner.mjs'

/**
 * This repository's root, from this file's own location rather than from
 * `process.cwd()`. A git pathspec resolves against the working directory, so a
 * gate run from inside a package directory would select no files at all and
 * report a clean tree — the failure mode `check-silent-defaults.mjs` shipped
 * with.
 */
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

/** The modules this gate covers: exactly `knip.json`'s blind spot. */
const RUNTIME_PREFIX = 'packages/ruvyxa/runtime/'

/**
 * Exports kept for a reason other than a reader in this repository.
 *
 * `file` and `name` must both match, and an entry that stops matching fails:
 * a reason nothing stands behind is how a list like this rots.
 *
 * The shape to expect here is a synced copy. `route-match.mjs` and
 * `origin-policy.mjs` are generated from `@ruvyxa/core` by
 * `packages/ruvyxa/scripts/sync-shared-runtime.mjs`, so an unused export in
 * one of them is fixed in the source module and re-synced — never by editing
 * the copy, which `ruvyxa`'s build refuses.
 */
export const ALLOWED = []

/** Text this gate treats as "not code": a whole-line comment. */
const COMMENT_LINE = /^\s*(?:\/\/|\/\*|\*)/

/** A binding an `export` keyword can introduce: the keyword, then the name. */
const DECLARATION =
  /\bexport\s+(?:async\s+)?(function|class|const|let|var)\s*\*?\s+([A-Za-z_$][\w$]*)/g

/** `export { a, b as c }`, with or without a `from` clause, over any number of lines. */
const EXPORT_LIST = /\bexport\s*\{([^}]*)\}/g

/**
 * The remaining declarators of `export const a = 1, b = 2`, from just past `a`.
 *
 * Only a `const`/`let`/`var` statement has them, and reading the commas out of
 * a regex window instead is wrong in a way that looks right: the first draft
 * of this gate searched the head of every declaration for `, name =` and so
 * reported the **default-valued parameters** of `export function
 * metaSourceImports(importPaths, namePrefix = …)` as exported bindings. A
 * parameter list is bracket depth, so depth is what this tracks.
 *
 * The statement ends at a `;`, at a closing bracket that was never opened, or
 * — this repository writes no semicolons — at a newline that is not continuing
 * a comma.
 */
function extraDeclarators(masked, start) {
  const names = []
  let depth = 0
  let expectName = false
  for (let index = start; index < masked.length; index += 1) {
    const character = masked[index]
    if (character === '(' || character === '[' || character === '{') depth += 1
    else if (character === ')' || character === ']' || character === '}') {
      if (depth === 0) break
      depth -= 1
    } else if (depth > 0) continue
    else if (character === ';') break
    else if (character === ',') expectName = true
    else if (character === '\n') {
      if (!expectName) break
    } else if (expectName && character.trim() !== '') {
      const next = /^[A-Za-z_$][\w$]*/.exec(masked.slice(index))
      if (next) {
        names.push(next[0])
        index += next[0].length - 1
      }
      expectName = false
    }
  }
  return names
}

/**
 * Every name one runtime module exports, with the line it is declared on.
 *
 * `source` is the module as written; masking happens here so a caller cannot
 * forget it. Exported so the rule can be tested against snippets rather than
 * only against whatever the repository happens to contain today.
 */
export function exportedNames(source) {
  const masked = maskNonCode(source)
  const lineOf = (index) => masked.slice(0, index).split('\n').length
  const found = new Map()

  for (const match of masked.matchAll(DECLARATION)) {
    const [text, keyword, name] = match
    const line = lineOf(match.index)
    if (!found.has(name)) found.set(name, line)
    if (keyword === 'function' || keyword === 'class') continue
    for (const extra of extraDeclarators(masked, match.index + text.length)) {
      if (!found.has(extra)) found.set(extra, line)
    }
  }

  for (const match of masked.matchAll(EXPORT_LIST)) {
    const line = lineOf(match.index)
    for (const clause of match[1].split(',')) {
      const parts = clause.trim().split(/\s+as\s+/)
      const name = (parts.at(-1) ?? '').trim()
      // A trailing comma leaves an empty clause, and `default` is not a name a
      // reader can reference by identifier.
      if (name === '' || name === 'default') continue
      if (!found.has(name)) found.set(name, line)
    }
  }

  return [...found].map(([name, line]) => ({ name, line })).sort((a, b) => a.line - b.line)
}

/**
 * Whether the doc comment immediately above `line` marks the export public.
 *
 * `@public` is the tag `knip.json` already honours through `"tags": ["-@public"]`,
 * so a module that opts one export out of Knip opts out of this gate with the
 * same word.
 */
export function isTaggedPublic(source, line) {
  const lines = source.split('\n')
  let index = line - 2
  while (index >= 0 && lines[index].trim() === '') index -= 1
  if (index < 0 || !lines[index].trim().endsWith('*/')) return false
  const end = index
  while (index >= 0 && !lines[index].includes('/**')) index -= 1
  if (index < 0) return false
  return lines
    .slice(index, end + 1)
    .join('\n')
    .includes('@public')
}

/** Whether `name` appears as an identifier in `source`, outside comment lines. */
function referencesName(source, name) {
  if (!source.includes(name)) return false
  const word = new RegExp(`(?<![\\w$])${name}(?![\\w$])`)
  return source.split('\n').some((line) => !COMMENT_LINE.test(line) && word.test(line))
}

const tracked = execFileSync('git', ['ls-files', '--cached', '--others', '--exclude-standard'], {
  encoding: 'utf8',
  cwd: REPO_ROOT,
  maxBuffer: 1 << 26,
})
  .split('\n')
  .map((file) => file.trim())
  .filter(Boolean)
  // A file tracked but deleted in the working tree reads as ENOENT; a gate is
  // about the tree as it is.
  .filter((file) => existsSync(path.join(REPO_ROOT, file)))
  .filter((file) => /\.(mjs|js|cjs|ts|tsx|rs|json|toml)$/.test(file))

const corpus = new Map(
  tracked.map((file) => [file, readFileSync(path.join(REPO_ROOT, file), 'utf8')]),
)

// A gate that looked at no files reports no findings, which reads exactly like
// a clean tree. Say so instead — this is how a pathspec that stopped matching
// stays invisible.
const modules = tracked.filter((file) => file.startsWith(RUNTIME_PREFIX) && file.endsWith('.mjs'))
if (modules.length === 0) {
  console.error(`No runtime modules found under ${RUNTIME_PREFIX}; this gate looked at nothing.`)
  process.exitCode = 1
}

const unread = []
const used = new Set()

for (const file of modules) {
  const source = corpus.get(file) ?? ''
  for (const { name, line } of exportedNames(source)) {
    if (isTaggedPublic(source, line)) continue
    const entry = ALLOWED.find((candidate) => candidate.file === file && candidate.name === name)
    if (entry) {
      used.add(`${entry.file}::${entry.name}`)
      continue
    }
    const readElsewhere = [...corpus].some(
      ([other, text]) => other !== file && referencesName(text, name),
    )
    if (!readElsewhere) unread.push({ file, line, name })
  }
}

// `process.exitCode` rather than `process.exit()`, for the same reason
// `check-silent-defaults.mjs` uses it: the rule above is exported and the test
// that holds it imports this module, and an exiting import takes the test
// process with it — reported as a file that passed without running a case.
const stale = ALLOWED.filter((entry) => !used.has(`${entry.file}::${entry.name}`))
if (stale.length > 0) {
  console.error('Allowlisted runtime exports that no longer exist or are read now:')
  for (const entry of stale) console.error(`  ${entry.file}  ${entry.name}`)
  console.error('\nRemove the entry from ALLOWED in scripts/check-runtime-exports.mjs.')
  process.exitCode = 1
} else if (unread.length > 0) {
  console.error(
    'Runtime exports nothing reads. Knip cannot see these: the modules are its entries.',
  )
  for (const { file, line, name } of unread) console.error(`  ${file}:${line}  ${name}`)
  console.error(
    '\nDrop the `export` keyword if the name is only used inside its own module, delete it if\n' +
      'nothing uses it at all, tag the doc comment `@public` if it is deliberate surface, or add\n' +
      'it to ALLOWED in scripts/check-runtime-exports.mjs with the reason.',
  )
  process.exitCode = 1
} else {
  console.log(
    `Checked ${modules.length} runtime modules; every exported name is read outside its own file.`,
  )
}
