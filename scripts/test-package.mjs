#!/usr/bin/env node
/**
 * Compile one package's TypeScript test suite and run it as JavaScript.
 *
 * Usage (from a package directory): node ../../../scripts/test-package.mjs <suite>
 *
 * Node is never asked to execute TypeScript: `tsc` emits the suite and the test
 * runner is pointed at that output. The emitted test file sits three
 * directories below the repository root, exactly like `tests/packages/<suite>/`,
 * so the `../../../packages/@ruvyxa/<name>/dist/index.js` every suite imports
 * resolves to the same file it did before compilation. That depth is the
 * constraint on where output may go.
 *
 * **Each suite gets its own output root, and that is not tidiness.** Every
 * `tsconfig.test.json` extends `tsconfig.test-base.json`, which sets
 * `rootDir: tests` — so a helper at `tests/deployed-function.ts`, imported by
 * eleven adapter suites, is emitted by *each* of them to the same path. With a
 * single output root that path was `.test-build/deployed-function.js`, written
 * concurrently by however many suites `pnpm -r test` had in flight, while other
 * suites were importing it. `tsc` truncates before it writes, so a reader
 * landing in that window sees a partial module:
 *
 *     SyntaxError: The requested module '../../deployed-function.js'
 *     does not provide an export named 'ECHO_BINARY_BODY'
 *
 * — from a file that plainly exports it. It failed on CI, where more suites run
 * at once, and not locally. Removing only `packages/<suite>` from the shared
 * root did not help: the collision was on the files *above* it.
 *
 * The output root is removed first. `tsc` overwrites but never deletes, so a
 * renamed or removed test would otherwise keep running from a stale artifact.
 */
import { spawnSync } from 'node:child_process'
import { existsSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import path from 'node:path'

const suite = process.argv[2]
if (!suite) {
  console.error('usage: node scripts/test-package.mjs <suite>')
  process.exit(1)
}

const repoRoot = path.resolve(import.meta.dirname, '..')
const packageDir = process.cwd()
const project = path.join(packageDir, 'tsconfig.test.json')
// Directly below the repository root, not nested inside one shared directory:
// the emitted test has to stay exactly three levels down for its
// `../../../packages/...` imports to resolve, and `.test-build/<suite>/` would
// put it four. Ignored by `.test-build-*/` in `.gitignore`.
const outRoot = path.join(repoRoot, `.test-build-${suite}`)
const outDir = path.join(outRoot, 'packages', suite)

if (!existsSync(project)) {
  console.error(`${suite}: no tsconfig.test.json in ${packageDir}`)
  process.exit(1)
}

// Located rather than spawned by name: `tsc` on PATH is a shell shim whose
// extension differs per platform, and spawnSync does not consult PATHEXT. The
// bin itself is not declared in the package's `exports`, so it is resolved
// through the one subpath that always is.
const tsc = path.join(
  path.dirname(createRequire(import.meta.url).resolve('typescript/package.json')),
  'bin',
  'tsc',
)

rmSync(outRoot, { recursive: true, force: true })
// `--outDir` on the command line overrides the shared base config, so no
// per-package `tsconfig.test.json` has to repeat it and none can drift.
run([tsc, '-p', project, '--outDir', outRoot])
run(['--test', `${outDir.split(path.sep).join('/')}/**/*.test.js`])

function run(args) {
  const result = spawnSync(process.execPath, args, { stdio: 'inherit', cwd: packageDir })
  if (result.error) {
    console.error(`${suite}: ${result.error.message}`)
    process.exit(1)
  }
  if (result.status !== 0) process.exit(result.status ?? 1)
}
