/**
 * Fail when a package's `dist/` holds an emitted file whose source is gone.
 *
 * `tsc` writes outputs; it never removes one. Deleting `src/plugin.ts` leaves
 * `dist/plugin.js`, `dist/plugin.d.ts`, and both maps exactly where they were,
 * and every package here lists `dist` in `files` — so `npm pack` ships modules
 * that import an API the release removed, from a working tree `git status`
 * calls clean, because `dist/` is ignored. That is how it happened:
 * `@ruvyxa/realtime` carried a `plugin` entry point for a plugin system that no
 * longer exists.
 *
 * Two packages already guard themselves by clearing `dist` before `tsc` runs
 * (`@ruvyxa/core`, `@ruvyxa/testing`). Twenty do not, and adding the same line
 * to each buys a rule that is correct only while the twenty-third package
 * remembers it. This reads the invariant instead: every file under `outDir`
 * traces back to a file under `rootDir`.
 *
 * Usage:
 *   node scripts/check-stale-dist.mjs         report and fail
 *   node scripts/check-stale-dist.mjs --fix   delete the orphans, then report
 */

import { existsSync, readdirSync, rmSync, statSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'

import { workspacePackageDirs } from './workspace-packages.mjs'

const REPO_ROOT = resolve(import.meta.dirname, '..')
const FIX = process.argv.includes('--fix')

/**
 * The suffixes `tsc` appends to one source file's path, longest first.
 *
 * Order matters: `a.d.ts.map` also ends in `.map`, and stripping the shorter
 * suffix would ask for a source named `a.d.ts`.
 */
const EMIT_SUFFIXES = ['.d.ts.map', '.d.ts', '.js.map', '.js', '.d.mts', '.mjs']

/** Extensions a source file may carry for one emitted base path. */
const SOURCE_EXTENSIONS = ['.ts', '.tsx', '.mts', '.cts']

/**
 * Files under `outDir` that are not emitted from a single source module and so
 * have no source to trace back to.
 */
const NOT_EMITTED_FROM_A_MODULE = (name) => name.endsWith('.tsbuildinfo')

function walk(dir) {
  const files = []
  for (const entry of readdirSync(dir).sort()) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) files.push(...walk(full))
    else files.push(full)
  }
  return files
}

/** The source path an emitted file came from, or `null` when it names none. */
function sourceBase(relativeEmitPath) {
  for (const suffix of EMIT_SUFFIXES) {
    if (relativeEmitPath.endsWith(suffix)) {
      return relativeEmitPath.slice(0, -suffix.length)
    }
  }
  return null
}

const orphans = []
const unmapped = []

for (const dir of workspacePackageDirs().dirs) {
  const packagePath = join(REPO_ROOT, dir)
  const distPath = join(packagePath, 'dist')
  const srcPath = join(packagePath, 'src')
  // A package with no build, or one that has not been built in this tree, has
  // nothing to be stale.
  if (!existsSync(distPath) || !existsSync(srcPath)) continue

  for (const file of walk(distPath)) {
    const emitted = relative(distPath, file).split('\\').join('/')
    if (NOT_EMITTED_FROM_A_MODULE(emitted)) continue
    const base = sourceBase(emitted)
    if (base === null) {
      // An unrecognised output shape is reported rather than skipped: a silent
      // skip would let a whole emit format pass this gate unchecked.
      unmapped.push(`${dir}/dist/${emitted}`)
      continue
    }
    const hasSource = SOURCE_EXTENSIONS.some((extension) =>
      existsSync(join(srcPath, `${base}${extension}`)),
    )
    if (!hasSource) orphans.push({ path: `${dir}/dist/${emitted}`, file })
  }
}

if (unmapped.length > 0) {
  console.error('Files under dist/ that this check cannot trace to a source module:')
  for (const path of unmapped) console.error(`  ${path}`)
  console.error(
    '\nAdd the emit suffix to EMIT_SUFFIXES in scripts/check-stale-dist.mjs, or the file\n' +
      'name to NOT_EMITTED_FROM_A_MODULE if nothing produced it from src/.',
  )
  process.exit(1)
}

if (orphans.length === 0) {
  console.log('Every emitted file under a package dist/ has a source in that package src/.')
  process.exit(0)
}

if (FIX) {
  for (const orphan of orphans) rmSync(orphan.file, { force: true })
  console.log(`Removed ${orphans.length} stale build output(s):`)
  for (const orphan of orphans) console.log(`  ${orphan.path}`)
  process.exit(0)
}

console.error('Stale build output: these files have no source and would be published anyway.')
for (const orphan of orphans) console.error(`  ${orphan.path}`)
console.error(
  '\nA source file was deleted or renamed and tsc left its output behind. Run:\n' +
    '  node scripts/check-stale-dist.mjs --fix\n' +
    'or rebuild the package from a cleared dist/.',
)
process.exit(1)
