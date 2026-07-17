#!/usr/bin/env node
import { execFileSync, execSync } from 'node:child_process'
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { resolve } from 'node:path'
import { rm } from 'node:fs/promises'
import { arch, platform } from 'node:process'
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
  const packageDestination = resolve(
    destination,
    'packages',
    pkg.replaceAll('@', '').replaceAll('/', '-'),
  )
  mkdirSync(packageDestination, { recursive: true })
  execFileSync('pnpm', ['--filter', pkg, 'pack', '--pack-destination', packageDestination], {
    stdio: 'inherit',
    shell: process.platform === 'win32',
  })
  const tarballs = readdirSync(packageDestination).filter((file) => file.endsWith('.tgz'))
  assert(tarballs.length === 1, `${pkg} should produce exactly one tarball`)
  copyFileSync(`${packageDestination}/${tarballs[0]}`, `${destination}/${tarballs[0]}`)
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
    for (const typeFile of ['css.d.ts', 'index.d.ts', 'config.d.ts', 'server.d.ts']) {
      assert(
        listing.includes(`package/types/${typeFile}`),
        `ruvyxa package missing types/${typeFile}`,
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

const coreTgz = readdirSync(destination).find(
  (name) => name.startsWith('ruvyxa-core-') && name.endsWith('.tgz'),
)
if (!coreTgz) throw new Error('@ruvyxa/core tarball not found in ' + destination)

const reactTgz = readdirSync(destination).find(
  (name) => name.startsWith('ruvyxa-react-') && name.endsWith('.tgz'),
)
if (!reactTgz) throw new Error('@ruvyxa/react tarball not found in ' + destination)

const currentPlatformTgz = readdirSync(destination).find(
  (name) => name.startsWith(`ruvyxa-cli-${platform}-${arch}-`) && name.endsWith('.tgz'),
)
if (!currentPlatformTgz)
  throw new Error(`${currentPlatformPackage} tarball not found in ` + destination)

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
assert(
  !existsSync(`${extracted}/scaffolded-app/app/css.d.ts`),
  'scaffolded app should use the framework-owned CSS declaration',
)

// Verify the scaffolded template can install and type-check.
// Install freshly packed packages, not registry releases, so unpublished versions are covered.
const scaffoldPackageJsonPath = `${extracted}/scaffolded-app/package.json`
const scaffoldPackageJson = JSON.parse(readFileSync(scaffoldPackageJsonPath, 'utf8'))
const scaffoldTarball = (file) => `file:../../${destination}/${file}`
const workspaceTarball = (file) => `file:../${destination}/${file}`
scaffoldPackageJson.dependencies.ruvyxa = scaffoldTarball(ruvyxaTgz)
scaffoldPackageJson.dependencies['@ruvyxa/react'] = scaffoldTarball(reactTgz)
writeFileSync(scaffoldPackageJsonPath, JSON.stringify(scaffoldPackageJson, null, 2) + '\n')
writeFileSync(
  `${extracted}/scaffolded-app/app/framework-type-entries.ts`,
  ["import 'ruvyxa'", "import 'ruvyxa/config'", "import 'ruvyxa/server'", ''].join('\n'),
)
writeFileSync(
  `${extracted}/pnpm-workspace.yaml`,
  [
    'packages:',
    "  - 'scaffolded-app'",
    'overrides:',
    `  '@ruvyxa/core': ${JSON.stringify(workspaceTarball(coreTgz))}`,
    `  '${currentPlatformPackage}': ${JSON.stringify(workspaceTarball(currentPlatformTgz))}`,
    '',
  ].join('\n'),
)

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
