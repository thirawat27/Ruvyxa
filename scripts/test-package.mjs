#!/usr/bin/env node
/**
 * Compile one package's TypeScript test suite and run it as JavaScript.
 *
 * Usage (from a package directory): node ../../../scripts/test-package.mjs <suite>
 *
 * Node is never asked to execute TypeScript: `tsc` emits the suite into
 * `.test-build/packages/<suite>/` and the test runner is pointed at that
 * output. The emitted tree sits three directories below the repository root,
 * exactly like `tests/packages/<suite>/`, so a relative import of a package's
 * built `dist/` resolves to the same file it did before compilation.
 *
 * The output directory is removed first. `tsc` overwrites but never deletes, so
 * a renamed or removed test would otherwise keep running from a stale artifact.
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
const outDir = path.join(repoRoot, '.test-build', 'packages', suite)

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

rmSync(outDir, { recursive: true, force: true })
run([tsc, '-p', project])
run(['--test', `${outDir.split(path.sep).join('/')}/**/*.test.js`])

function run(args) {
  const result = spawnSync(process.execPath, args, { stdio: 'inherit', cwd: packageDir })
  if (result.error) {
    console.error(`${suite}: ${result.error.message}`)
    process.exit(1)
  }
  if (result.status !== 0) process.exit(result.status ?? 1)
}
