#!/usr/bin/env node
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { workspacePackageDirs } from './workspace-packages.mjs'

const rootPkg = JSON.parse(readFileSync('package.json', 'utf8'))
const expectedVersion = rootPkg.version
const expectedLicense = rootPkg.license
const requiredRuntimeNodeEngine = rootPkg.engines?.node
const requiredRuntimeNodeVersion = requiredRuntimeNodeEngine.replace(/^>=/, '')
const requiredRuntimeNodeMajor = requiredRuntimeNodeVersion.split('.')[0]
const workspaceNodeTypesVersion = rootPkg.devDependencies?.['@types/node']
const repoUrl = 'git+https://github.com/thirawat27/ruvyxa.git'
const { dirs: packageDirs, ignored: ignoredPackageDirs } = workspacePackageDirs()

const failures = []

check(
  workspaceNodeTypesVersion?.split('.')[0] === requiredRuntimeNodeMajor,
  `workspace @types/node must match the engines.node major (${requiredRuntimeNodeMajor})`,
)
check(
  readFileSync('.node-version', 'utf8').trim() === requiredRuntimeNodeMajor,
  `.node-version must track the engines.node major (${requiredRuntimeNodeMajor})`,
)
check(
  readFileSync('.nvmrc', 'utf8').trim() === requiredRuntimeNodeVersion,
  `.nvmrc must equal the engines.node floor (${requiredRuntimeNodeVersion})`,
)

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
  const nodeTypes = pkg.devDependencies?.['@types/node']
  if (nodeTypes !== undefined) {
    check(
      nodeTypes === workspaceNodeTypesVersion,
      `${pkg.name} @types/node must match the workspace version (${workspaceNodeTypesVersion})`,
    )
  }
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
  // Checked before the plugin branch below. The plugin template used to skip
  // every shared check because that branch `continue`s, and it was the one
  // template that shipped no Node floor at all.
  check(
    pkg.engines?.node === requiredRuntimeNodeEngine,
    `${dir} Node engine must match the framework requirement (${requiredRuntimeNodeEngine})`,
  )
  if (dir === 'templates/plugin') {
    check(
      pkg.peerDependencies?.ruvyxa === `^${expectedVersion}`,
      `${dir} ruvyxa peer dependency must be ^${expectedVersion}`,
    )
    check(
      pkg.devDependencies?.ruvyxa === `^${expectedVersion}`,
      `${dir} ruvyxa development dependency must be ^${expectedVersion}`,
    )
    check(pkg.ruvyxa === undefined, `${dir} must not include package-level Ruvyxa metadata`)
    check(pkg.devDependencies?.typescript === '^7.0.2', `${dir} must use TypeScript ^7.0.2`)
    check(
      pkg.dependencies?.['@ruvyxa/react'] === undefined,
      `${dir} plugin must not depend on @ruvyxa/react`,
    )
    continue
  }
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

// The README states the Node floor in prose and in a badge image. Both are
// hand-maintained copies of `engines.node`, which is the only enforced source:
// npm rejects an install below it. A commit lowered the badge to 22.12 while
// every manifest, doc page, and CI job still required 22.13.0, so the first
// thing a reader saw was the one number that would fail their install. This is
// the same "a cross-language fact belongs in a replayed check, not a comment
// promising the two stay in sync" rule the repo already applies to template
// mirrors and conformance fixtures.
const readme = readFileSync('README.md', 'utf8')
// A full manifest floor such as `24.19.0` is written `24.19` in prose and badges.
const [major, minor] = requiredRuntimeNodeVersion.split('.')
const displayFloor = `${major}.${minor}`

const badgeMatches = [...readme.matchAll(/img\.shields\.io\/badge\/node-%3E%3D([\d.]+)-/g)]
check(badgeMatches.length > 0, 'README must carry a Node version badge')
for (const [, badgeVersion] of badgeMatches) {
  check(
    badgeVersion === displayFloor,
    `README Node badge says ${badgeVersion} but engines.node requires ${requiredRuntimeNodeEngine} (expected ${displayFloor})`,
  )
}

for (const [claim, claimed] of readme.matchAll(/Node\.js\s\*{0,2}(\d+\.\d+)\+/g)) {
  check(
    claimed === displayFloor,
    `README claims "${claim.trim()}" but engines.node requires ${requiredRuntimeNodeEngine} (expected ${displayFloor})`,
  )
}

if (failures.length > 0) {
  console.error(failures.map((failure) => `- ${failure}`).join('\n'))
  process.exit(1)
}

// Reported rather than ignored. A directory under `packages/` with no manifest
// is not a workspace package and is skipped, but it is invisible to
// `git status` when everything inside it is ignored — which is how one sat
// there long enough to crash this script on a clean tree.
for (const dir of ignoredPackageDirs) {
  console.log(`Ignored ${dir}: no package.json, so it is not a workspace package.`)
}

console.log(`Validated README Node floor against engines.node (${requiredRuntimeNodeEngine}).`)

function check(condition, message) {
  if (!condition) failures.push(message)
}
