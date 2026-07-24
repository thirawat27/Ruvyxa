#!/usr/bin/env node
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'

const rootPkg = JSON.parse(readFileSync('package.json', 'utf8'))
const expectedVersion = rootPkg.version
const expectedLicense = rootPkg.license
const requiredRuntimeNodeEngine = rootPkg.engines?.node
const repoUrl = 'git+https://github.com/thirawat27/ruvyxa.git'
const packageDirs = [
  'packages/ruvyxa',
  'packages/create-ruvyxa',
  ...readdirSync('packages/@ruvyxa')
    .map((name) => `packages/@ruvyxa/${name}`)
    .filter((dir) => statSync(dir).isDirectory()),
]

const failures = []

for (const dir of packageDirs) {
  const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
  check(pkg.version === expectedVersion, `${pkg.name} version must be ${expectedVersion}`)
  check(pkg.description?.length >= 40, `${pkg.name} needs a useful npm description`)
  check(pkg.license === expectedLicense, `${pkg.name} license must be ${expectedLicense}`)
  check(pkg.repository?.url === repoUrl, `${pkg.name} repository must point to thirawat27/ruvyxa`)
  check(
    pkg.bugs?.url === 'https://github.com/thirawat27/ruvyxa/issues',
    `${pkg.name} bugs URL is invalid`,
  )
  check(
    pkg.homepage === 'https://github.com/thirawat27/ruvyxa#readme',
    `${pkg.name} homepage is invalid`,
  )
  check(pkg.publishConfig?.access === 'public', `${pkg.name} must publish with public access`)
  check(Array.isArray(pkg.files) && pkg.files.length > 0, `${pkg.name} must declare package files`)
  // Every published package states the same floor. A package that advertised a
  // lower one was making a promise the framework it ships with cannot keep:
  // the packages are only usable together, so a split floor is a false claim
  // that npm enforces against the wrong number.
  check(
    pkg.engines?.node === requiredRuntimeNodeEngine,
    `${pkg.name} Node engine must match the framework requirement (${requiredRuntimeNodeEngine})`,
  )
  // A published declaration map points at `src/`, so `src` must be in the
  // tarball or every "go to definition" and every stack frame resolves to a
  // file that was never shipped.
  if (Array.isArray(pkg.files) && pkg.files.includes('dist')) {
    check(
      pkg.files.includes('src'),
      `${pkg.name} publishes dist with declaration maps, so it must also publish src`,
    )
  }
}

if (failures.length > 0) {
  console.error(failures.map((failure) => `- ${failure}`).join('\n'))
  process.exit(1)
}

console.log(`Validated ${packageDirs.length} npm package manifests for ${expectedVersion}.`)

const templateDirs = readdirSync('templates')
  .map((name) => `templates/${name}`)
  .filter((dir) => statSync(dir).isDirectory())

for (const dir of templateDirs) {
  const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
  for (const dependency of ['ruvyxa', '@ruvyxa/react']) {
    check(
      pkg.dependencies?.[dependency] === `^${expectedVersion}`,
      `${dir} ${dependency} dependency must be ^${expectedVersion}`,
    )
  }
}

if (failures.length > 0) {
  console.error(failures.map((failure) => `- ${failure}`).join('\n'))
  process.exit(1)
}

console.log(`Validated ${templateDirs.length} starter template manifests for ${expectedVersion}.`)

// Validate Rust crate versions match
const crateDirs = readdirSync('crates')
  .map((name) => `crates/${name}`)
  .filter((dir) => statSync(dir).isDirectory())

for (const dir of crateDirs) {
  const cargoToml = readFileSync(join(dir, 'Cargo.toml'), 'utf8')
  const versionMatch = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)
  if (versionMatch) {
    const crateVersion = versionMatch[1]
    if (crateVersion !== expectedVersion) {
      failures.push(`${dir} Cargo.toml version "${crateVersion}" must be "${expectedVersion}"`)
    }
  }
}

if (failures.length > 0) {
  console.error(failures.map((failure) => `- ${failure}`).join('\n'))
  process.exit(1)
}

console.log(`Validated ${crateDirs.length} Rust crate manifests for ${expectedVersion}.`)

function check(condition, message) {
  if (!condition) failures.push(message)
}
