#!/usr/bin/env node
/**
 * Bump all workspace package.json and Cargo.toml versions to match root package.json,
 * then regenerate Cargo.lock so CI passes with --locked.
 *
 * Usage:
 *   node scripts/bump-version.mjs          # sync all to root version
 *   node scripts/bump-version.mjs 1.2.0    # set all to 1.2.0
 */
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { execFileSync } from 'node:child_process'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { workspacePackageDirs } from './workspace-packages.mjs'

/**
 * Prettier's CLI entry, resolved from this workspace's own installed copy.
 *
 * Run as `node <bin> --write <file>...` rather than `npx prettier ...` inside a
 * shell, and with the file list as argv entries rather than interpolated into a
 * command string. A manifest path is a directory name off disk, so the shell
 * form let a workspace directory containing a quote, a space, or a `$` decide
 * what the bump executed -- and `npx` is a `.cmd` on Windows, which is why the
 * shell was there at all. Addressing the JavaScript entry through `node` needs
 * no shell on any host, so both problems leave together.
 */
function prettierBin() {
  const resolve = createRequire(import.meta.url).resolve
  return join(dirname(resolve('prettier/package.json')), 'bin', 'prettier.cjs')
}

const rootPkg = JSON.parse(readFileSync('package.json', 'utf8'))
const newVersion = process.argv[2] || rootPkg.version

// Every file this run rewrote, so the reformat below is over exactly those.
const rewritten = []

/**
 * Write a manifest and remember it.
 *
 * `JSON.stringify(value, null, 2)` is not the formatting this repository keeps:
 * Prettier collapses an array that fits inside `printWidth`, and `JSON.stringify`
 * never does. So a bump left every manifest it touched failing `pnpm
 * format:check` -- a CI step on all five platforms and a `verify-release` step
 * -- and the failure named ~20 files, none of which said "bump".
 */
function writeManifest(file, value) {
  writeFileSync(file, JSON.stringify(value, null, 2) + '\n')
  rewritten.push(file)
}

// Update root package.json
if (rootPkg.version !== newVersion) {
  rootPkg.version = newVersion
  writeManifest('package.json', rootPkg)
  console.log(`root package.json → ${newVersion}`)
}

// Update all workspace package.json files
const { dirs: packageDirs } = workspacePackageDirs()

for (const dir of packageDirs) {
  const file = join(dir, 'package.json')
  const pkg = JSON.parse(readFileSync(file, 'utf8'))
  if (pkg.version !== newVersion) {
    pkg.version = newVersion
    writeManifest(file, pkg)
    console.log(`${pkg.name} → ${newVersion}`)
  }
}

// Update all Cargo.toml files
const crateDirs = readdirSync('crates')
  .map((name) => `crates/${name}`)
  .filter((dir) => statSync(dir).isDirectory())

for (const dir of crateDirs) {
  const file = join(dir, 'Cargo.toml')
  const content = readFileSync(file, 'utf8')
  const updated = content.replace(/^version\s*=\s*"[^"]+"/m, `version = "${newVersion}"`)
  if (content !== updated) {
    writeFileSync(file, updated)
    console.log(`${dir} → ${newVersion}`)
  }
}

// Update framework dependencies in every source starter template. The
// create-ruvyxa package copies application templates during prepack, while
// the CLI embeds templates/plugin when it scaffolds a plugin package.
const templateDirs = readdirSync('templates')
  .map((name) => `templates/${name}`)
  .filter((dir) => statSync(dir).isDirectory())

/**
 * Point a template's framework dependencies at the version being released.
 *
 * Only entries that already exist are touched: a template that does not depend
 * on `@ruvyxa/react` must not gain the dependency because a release happened.
 * Returns whether anything moved, so an unchanged template is not rewritten.
 */
function repinFrameworkDeps(manifest, version) {
  const pin = `^${version}`
  let changed = false
  for (const dependency of ['ruvyxa', '@ruvyxa/react']) {
    for (const group of ['dependencies', 'peerDependencies', 'devDependencies']) {
      if (!manifest[group]?.[dependency] || manifest[group][dependency] === pin) continue
      manifest[group][dependency] = pin
      changed = true
    }
  }
  return changed
}

for (const dir of templateDirs) {
  const templatePkg = join(dir, 'package.json')
  try {
    const tmpl = JSON.parse(readFileSync(templatePkg, 'utf8'))
    const changed = repinFrameworkDeps(tmpl, newVersion)
    if (changed) {
      writeManifest(templatePkg, tmpl)
      console.log(`${dir} framework deps → ^${newVersion}`)
    }
  } catch {
    // Non-application template directories do not need a package manifest.
  }
}

// Regenerate Cargo.lock so --locked CI checks pass
try {
  execFileSync('cargo', ['update', '--workspace'], { stdio: 'inherit' })
  console.log('Cargo.lock regenerated')
} catch (err) {
  console.error('Warning: failed to update Cargo.lock — run `cargo update --workspace` manually')
  console.error(err.message)
}

// Reformat what was written, so the bump leaves a tree that passes the gate it
// has to pass. Over the touched files rather than the whole repository: a bump
// should not quietly reformat something it did not change.
if (rewritten.length > 0) {
  try {
    execFileSync(process.execPath, [prettierBin(), '--write', ...rewritten], {
      stdio: 'inherit',
    })
    console.log(`Formatted ${rewritten.length} manifest(s)`)
  } catch (err) {
    console.error('Warning: failed to format the rewritten manifests — run `pnpm format` manually')
    console.error(err.message)
  }
}

console.log(`\nAll versions synced to ${newVersion}`)
