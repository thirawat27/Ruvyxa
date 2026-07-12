#!/usr/bin/env node
import { execFileSync, execSync } from 'node:child_process'
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { rm } from 'node:fs/promises'
import { arch, platform } from 'node:process'
import { resolve } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'

const destination = '.npm-pack'
const currentPlatformPackage = `@ruvyxa/cli-${platform}-${arch}`
const packages = [
  '@ruvyxa/core',
  '@ruvyxa/react',
  '@ruvyxa/adapter-node',
  '@ruvyxa/adapter-vercel',
  '@ruvyxa/adapter-cloudflare',
  '@ruvyxa/adapter-netlify',
  '@ruvyxa/adapter-bun',
  '@ruvyxa/adapter-static',
  currentPlatformPackage,
  'ruvyxa',
  'create-ruvyxa',
]

rmSync(destination, { recursive: true, force: true })
mkdirSync(destination, { recursive: true })

for (const pkg of packages) {
  execFileSync('pnpm', ['--filter', pkg, 'pack', '--pack-destination', destination], {
    stdio: 'inherit',
    shell: process.platform === 'win32',
  })
}

for (const file of readdirSync(destination).filter((name) => name.endsWith('.tgz'))) {
  const listing = execSync(`tar -tf ${destination}/${file}`).toString()
  const verboseListing = execSync(`tar -tvf ${destination}/${file}`).toString()
  const packageJson = JSON.parse(
    execSync(`tar -xOf ${destination}/${file} package/package.json`).toString(),
  )
  const serialized = JSON.stringify(packageJson)

  assert(!serialized.includes('workspace:'), `${file} contains workspace protocol`)
  assert(!listing.includes('.test.'), `${file} includes test files`)

  if (packageJson.name === 'ruvyxa') {
    assert(
      /-rwxr-xr-x\s+.*package\/bin\/ruvyxa\.js/.test(verboseListing),
      'ruvyxa package launcher must be executable',
    )
    const executable = platform === 'win32' ? 'ruvyxa.exe' : 'ruvyxa'
    assert(
      listing.includes(`package/native-bin/${platform}-${arch}/${executable}`),
      'ruvyxa package missing native binary',
    )
    for (const runtimeFile of ['ssg-renderer.mjs', 'worker-pool.mjs']) {
      assert(
        listing.includes(`package/runtime/${runtimeFile}`),
        `ruvyxa package missing runtime/${runtimeFile}`,
      )
    }
  }

  if (packageJson.name === 'create-ruvyxa') {
    assert(
      listing.includes('package/template/minimal/package.json'),
      'create-ruvyxa package missing template',
    )
    assert(
      listing.includes('package/template/minimal/gitignore'),
      'create-ruvyxa package missing scaffold ignore template',
    )
  }
}

const extracted = '.npm-smoke'
rmSync(extracted, { recursive: true, force: true })
mkdirSync(extracted, { recursive: true })

const ruvyxaTgz = readdirSync(destination).find(
  (name) =>
    name.startsWith('ruvyxa-') &&
    name.endsWith('.tgz') &&
    !name.includes('adapter') &&
    !name.includes('cli') &&
    !name.includes('core') &&
    !name.includes('react') &&
    !name.includes('create'),
)
if (!ruvyxaTgz) throw new Error('ruvyxa tarball not found in ' + destination)

const createRuvyxaTgz = readdirSync(destination).find(
  (name) => name.startsWith('create-ruvyxa-') && name.endsWith('.tgz'),
)
if (!createRuvyxaTgz) throw new Error('create-ruvyxa tarball not found in ' + destination)

execSync(`tar -xzf ${destination}/${ruvyxaTgz} -C ${extracted}`)
execFileSync('node', [`${extracted}/package/bin/ruvyxa.js`, '--help'], {
  stdio: 'inherit',
  shell: process.platform === 'win32',
})
mkdirSync(`${extracted}/create-ruvyxa`)
execSync(`tar -xzf ${destination}/${createRuvyxaTgz} -C ${extracted}/create-ruvyxa`)
execFileSync(
  'node',
  [`${extracted}/create-ruvyxa/package/bin/create-ruvyxa.js`, `${extracted}/scaffolded-app`],
  {
    stdio: 'inherit',
    shell: process.platform === 'win32',
  },
)
assert(existsSync(`${extracted}/scaffolded-app/.gitignore`), 'scaffolded app missing .gitignore')

// Verify the scaffolded template can install and type-check.
// This catches version mismatches (e.g. @ruvyxa/react version drift) early.
// Write an empty pnpm-workspace.yaml so pnpm treats the scaffolded app as its own
// workspace root, preventing it from inheriting the monorepo workspace context.
writeFileSync(`${extracted}/scaffolded-app/pnpm-workspace.yaml`, '')

// Override dependencies to install from local tarballs instead of the registry.
// The version being tested may not be published yet.
const scaffoldedPkgPath = `${extracted}/scaffolded-app/package.json`
const scaffoldedPkg = JSON.parse(readFileSync(scaffoldedPkgPath, 'utf8'))
const tarballs = readdirSync(destination)
const packDir = resolve(destination)

function findTarball(pkgName) {
  // Tarball names use the flat npm convention: scoped "@ruvyxa/core" → "ruvyxa-core-1.0.11.tgz"
  const prefix = pkgName.replace(/^@/, '').replace(/\//, '-')
  // Match "prefix-<version>.tgz" precisely to avoid e.g. "ruvyxa-" matching "ruvyxa-core-"
  return tarballs.find((f) => {
    if (!f.startsWith(prefix + '-') || !f.endsWith('.tgz')) return false
    // The character after the prefix + '-' must be a digit (start of version)
    const rest = f.slice(prefix.length + 1)
    return /^\d/.test(rest)
  })
}

for (const depGroup of ['dependencies', 'devDependencies']) {
  if (!scaffoldedPkg[depGroup]) continue
  for (const dep of Object.keys(scaffoldedPkg[depGroup])) {
    const tgz = findTarball(dep)
    if (tgz) {
      scaffoldedPkg[depGroup][dep] = `file:${resolve(packDir, tgz)}`
    }
  }
}

// Also set pnpm.overrides to catch transitive dependencies (e.g. ruvyxa → @ruvyxa/core)
// that reference the unpublished version.
const overrides = {}
for (const pkg of packages) {
  const tgz = findTarball(pkg)
  if (tgz) {
    overrides[pkg] = `file:${resolve(packDir, tgz)}`
  }
}
scaffoldedPkg.pnpm = { ...scaffoldedPkg.pnpm, overrides }

writeFileSync(scaffoldedPkgPath, JSON.stringify(scaffoldedPkg, null, 2) + '\n')

execFileSync('pnpm', ['install', '--no-lockfile'], {
  cwd: `${extracted}/scaffolded-app`,
  stdio: 'inherit',
  shell: process.platform === 'win32',
})
execFileSync('pnpm', ['run', 'typecheck'], {
  cwd: `${extracted}/scaffolded-app`,
  stdio: 'inherit',
  shell: process.platform === 'win32',
})

await rmWithRetry(extracted)

console.log('npm pack smoke passed.')

function assert(condition, message) {
  if (!condition) {
    throw new Error(message)
  }
}

async function rmWithRetry(path) {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      await rm(path, { recursive: true, force: true })
      return
    } catch (error) {
      if (attempt === 4) throw error
      await sleep(250)
    }
  }
}
