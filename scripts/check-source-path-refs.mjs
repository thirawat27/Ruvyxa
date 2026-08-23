#!/usr/bin/env node
// A doc comment that names a repository path is a promise the reader will act
// on: `AGENTS.md` says "when a doc comment names the test that holds a
// contract, open that test", and that instruction is only worth giving while
// the path still resolves. Nothing checked them. `scripts/check-doc-links.mjs`
// resolves every relative link in Markdown, but a path written in a `.rs`,
// `.mjs`, or `.ts` comment is invisible to it, so a moved test or a renamed
// fixture leaves the pointer behind silently.
//
// Three had already drifted when this check was written, each naming a gate a
// reader could not open. They are described rather than quoted, because a dead
// path written here is one this check would report against itself:
//
//   static_assets.rs        named the core static-asset contract test with a
//                           `.mjs` extension; that suite is a `.ts` file.
//   router.rs               named a route-match test under a `tests/packages`
//                           react directory that has never existed. The suite
//                           lives in the react package's own `test/` folder.
//   serverless-handler.mjs  named an `endpoint-contract` fixture that was never
//                           created. The table it means is the framework
//                           endpoint conformance one.
//
// This check reads the comments out of every source file git knows about --
// tracked or merely not ignored -- and fails when a repository path named in one
// does not resolve.
//
// It errs toward silence. A comment region it fails to recognise is simply not
// scanned, because the cost of the two directions is not symmetric: a missed
// stale pointer leaves today's behaviour unchanged, while a false positive
// fails a release over a path that is fine.
import { existsSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { dirname, join, normalize, resolve } from 'node:path'
import { execFileSync } from 'node:child_process'

// Only paths anchored at one of the repository's real top-level directories are
// checked. Anything else -- `app/page.tsx`, `src/index.ts`, a path inside a
// scaffolded project -- describes a user's tree rather than this one.
const ANCHORS = ['crates', 'packages', 'scripts', 'tests', 'docs', 'examples', 'templates']
// A reference is only checkable when it names a file, so an extension is
// required. A bare directory reference is left alone.
//
// Sorted longest-first, and followed by a boundary. Regular-expression
// alternation takes the first branch that matches rather than the longest, so
// listing `js` before `json` truncated every `*-conformance.json` in the tree to
// a `.js` file that does not exist -- 60 clean fixtures reported as stale on
// this check's first run.
const EXTENSIONS = [
  'rs',
  'mjs',
  'cjs',
  'js',
  'tsx',
  'ts',
  'json',
  'md',
  'toml',
  'yaml',
  'yml',
].sort((left, right) => right.length - left.length || (left < right ? -1 : 1))

const PATH_PATTERN = new RegExp(
  String.raw`(?:^|[\s\`'"(\[<])((?:${ANCHORS.join('|')})/[A-Za-z0-9_@./-]*\.(?:${EXTENSIONS.join('|')}))(?![A-Za-z0-9])`,
  'g',
)

const repoRoot = resolve(
  dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')),
  '..',
)

// Tracked files *and* untracked ones that are not ignored. `ls-files` alone
// lists only what is already committed, so a file added in the working tree --
// including the one that introduces a stale pointer -- would pass this check and
// fail for whoever ran it next. `--exclude-standard` keeps `dist/`,
// `node_modules/`, and the rest of `.gitignore` out.
const files = [
  ...new Set(
    execFileSync(
      'git',
      [
        'ls-files',
        '--cached',
        '--others',
        '--exclude-standard',
        '--',
        '*.rs',
        '*.mjs',
        '*.ts',
        '*.tsx',
      ],
      { encoding: 'utf8', cwd: repoRoot },
    )
      .split('\n')
      .map((file) => file.trim())
      .filter((file) => file && !file.includes('node_modules/')),
  ),
]

/**
 * Collect the comment text out of a Rust or JavaScript/TypeScript source file.
 *
 * Walks the source once, tracking which literal the cursor is inside so a `//`
 * in a string or a template is not read as a comment. Regular-expression
 * literals are deliberately not modelled: distinguishing `/` as division from
 * `/` as the start of a pattern needs the parse this script does not do, and
 * guessing wrong in that direction invents a comment. A `//` inside a regex
 * therefore ends the scan of that line early, which under-reports.
 *
 * @param {string} source raw file text
 * @returns {string[]} the text of each comment found, in source order
 */
function commentRegions(source) {
  const regions = []
  let index = 0

  while (index < source.length) {
    const char = source[index]
    const next = source[index + 1]

    if (char === '/' && next === '/') {
      const end = source.indexOf('\n', index)
      const stop = end === -1 ? source.length : end
      regions.push(source.slice(index + 2, stop))
      index = stop
      continue
    }

    if (char === '/' && next === '*') {
      const end = source.indexOf('*/', index + 2)
      const stop = end === -1 ? source.length : end
      regions.push(source.slice(index + 2, stop))
      index = stop === source.length ? stop : stop + 2
      continue
    }

    if (char === '"' || char === "'" || char === '`') {
      index = skipLiteral(source, index, char)
      continue
    }

    index += 1
  }

  return regions
}

/**
 * Advance past a string or template literal that starts at `start`.
 *
 * Stops at a newline for `"` and `'` the way `ast.rs`'s `skip_string` does, so
 * an unterminated quote cannot swallow the rest of the file and hide every
 * comment below it.
 *
 * @param {string} source raw file text
 * @param {number} start offset of the opening quote
 * @param {string} quote the opening quote character
 * @returns {number} offset just past the literal
 */
function skipLiteral(source, start, quote) {
  let index = start + 1
  while (index < source.length) {
    const char = source[index]
    if (char === '\\') {
      index += 2
      continue
    }
    if (char === quote) return index + 1
    if (char === '\n' && quote !== '`') return index
    index += 1
  }
  return index
}

/**
 * Resolve a path named in a comment against the tree.
 *
 * Tried repository-root first, then relative to the nearest package root above
 * the file that mentions it, because a package's own scripts are written the
 * way that package runs them: the generated header in the `ruvyxa` package's
 * `runtime/route-match.mjs` names `sync-shared-runtime.mjs` under a bare
 * `scripts/`, which is real inside that package rather than at the root.
 *
 * @param {string} candidate path text taken from a comment
 * @param {string} sourceFile repository-relative path of the file it appeared in
 * @returns {boolean} whether the reference lands on something in the tree
 */
function resolves(candidate, sourceFile) {
  if (existsSync(join(repoRoot, candidate))) return true

  let directory = dirname(sourceFile)
  while (directory && directory !== '.' && directory !== '/') {
    if (existsSync(join(repoRoot, directory, 'package.json'))) {
      if (existsSync(join(repoRoot, directory, candidate))) return true
    }
    const parent = dirname(directory)
    if (parent === directory) break
    directory = parent
  }

  return false
}

const failures = []

await Promise.all(
  files.map(async (file) => {
    const source = await readFile(join(repoRoot, file), 'utf8')
    // Cheap reject: most files never name a repository path at all.
    if (!ANCHORS.some((anchor) => source.includes(`${anchor}/`))) return

    for (const region of commentRegions(source)) {
      for (const match of region.matchAll(PATH_PATTERN)) {
        const candidate = normalize(match[1]).split('\\').join('/')
        // A glob or a template placeholder describes a set, not a file.
        if (candidate.includes('*') || candidate.includes('$') || candidate.includes('{')) continue
        if (resolves(candidate, file)) continue
        failures.push(`${file}: names \`${candidate}\`, which is not in the tree`)
      }
    }
  }),
)

if (failures.length > 0) {
  failures.sort()
  console.error('Stale repository paths named in source comments:\n')
  for (const failure of failures) console.error(`  ${failure}`)
  console.error(
    `\n${failures.length} reference${failures.length === 1 ? '' : 's'} did not resolve.` +
      '\nUpdate the comment to the path the file actually lives at, or create the file it promises.',
  )
  process.exitCode = 1
} else {
  console.log(
    `Checked ${files.length} source files; every repository path named in a comment resolves.`,
  )
}
