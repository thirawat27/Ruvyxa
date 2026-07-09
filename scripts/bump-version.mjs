#!/usr/bin/env node
/**
 * Bump all workspace package.json and Cargo.toml versions to match root package.json.
 *
 * Usage:
 *   node scripts/bump-version.mjs          # sync all to root version
 *   node scripts/bump-version.mjs 1.2.0    # set all to 1.2.0
 */
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const rootPkg = JSON.parse(readFileSync('package.json', 'utf8'))
const newVersion = process.argv[2] || rootPkg.version

// Update root package.json
if (rootPkg.version !== newVersion) {
  rootPkg.version = newVersion
  writeFileSync('package.json', JSON.stringify(rootPkg, null, 2) + '\n')
  console.log(`root package.json → ${newVersion}`)
}

// Update all workspace package.json files
const packageDirs = [
  'packages/ruvyxa',
  'packages/create-ruvyxa',
  ...readdirSync('packages/@ruvyxa')
    .map((name) => `packages/@ruvyxa/${name}`)
    .filter((dir) => statSync(dir).isDirectory()),
]

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

// Update template dependency
const templatePkg = 'templates/minimal/package.json'
try {
  const tmpl = JSON.parse(readFileSync(templatePkg, 'utf8'))
  if (tmpl.dependencies?.ruvyxa) {
    tmpl.dependencies.ruvyxa = `^${newVersion}`
    writeFileSync(templatePkg, JSON.stringify(tmpl, null, 2) + '\n')
    console.log(`template ruvyxa dep → ^${newVersion}`)
  }
} catch {
  // template may not exist in all contexts
}

console.log(`\nAll versions synced to ${newVersion}`)
