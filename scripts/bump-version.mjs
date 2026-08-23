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
import { execSync } from 'node:child_process'
import { join } from 'node:path'
import { workspacePackageDirs } from './workspace-packages.mjs'

const rootPkg = JSON.parse(readFileSync('package.json', 'utf8'))
const newVersion = process.argv[2] || rootPkg.version

// Update root package.json
if (rootPkg.version !== newVersion) {
  rootPkg.version = newVersion
  writeFileSync('package.json', JSON.stringify(rootPkg, null, 2) + '\n')
  console.log(`root package.json → ${newVersion}`)
}

// Update all workspace package.json files
const { dirs: packageDirs } = workspacePackageDirs()

for (const dir of packageDirs) {
  const file = join(dir, 'package.json')
  const pkg = JSON.parse(readFileSync(file, 'utf8'))
  if (pkg.version !== newVersion) {
    pkg.version = newVersion
    writeFileSync(file, JSON.stringify(pkg, null, 2) + '\n')
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
      writeFileSync(templatePkg, JSON.stringify(tmpl, null, 2) + '\n')
      console.log(`${dir} framework deps → ^${newVersion}`)
    }
  } catch {
    // Non-application template directories do not need a package manifest.
  }
}

// Regenerate Cargo.lock so --locked CI checks pass
try {
  execSync('cargo update --workspace', { stdio: 'inherit' })
  console.log('Cargo.lock regenerated')
} catch (err) {
  console.error('Warning: failed to update Cargo.lock — run `cargo update --workspace` manually')
  console.error(err.message)
}

console.log(`\nAll versions synced to ${newVersion}`)
