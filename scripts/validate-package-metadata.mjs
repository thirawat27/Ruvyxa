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
  if (
    pkg.name === 'ruvyxa' ||
    pkg.name === 'create-ruvyxa' ||
    pkg.name.startsWith('@ruvyxa/cli-')
  ) {
    check(
      pkg.engines?.node === requiredRuntimeNodeEngine,
      `${pkg.name} Node engine must match the framework requirement (${requiredRuntimeNodeEngine})`,
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
