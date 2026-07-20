#!/usr/bin/env node
import { execFileSync } from 'node:child_process'
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
const pnpmBin = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm'
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
  execFileSync(pnpmBin, ['--filter', pkg, 'pack', '--pack-destination', packageDestination], {
    stdio: 'inherit',
    // Windows exposes pnpm through a .cmd shim; Node requires shell dispatch for that shim.
    shell: process.platform === 'win32',
  })
  const tarballs = readdirSync(packageDestination).filter((file) => file.endsWith('.tgz'))
  assert(tarballs.length === 1, `${pkg} should produce exactly one tarball`)
  copyFileSync(`${packageDestination}/${tarballs[0]}`, `${destination}/${tarballs[0]}`)
}

for (const file of readdirSync(destination).filter((name) => name.endsWith('.tgz'))) {
  const tarball = `${destination}/${file}`
  const listing = execFileSync('tar', ['-tf', tarball]).toString()
  const verboseListing = execFileSync('tar', ['-tvf', tarball]).toString()
  const packageJson = JSON.parse(
    execFileSync('tar', ['-xOf', tarball, 'package/package.json']).toString(),
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
    for (const runtimeFile of ['compiler.mjs', 'worker-pool.mjs']) {
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
    for (const template of ['minimal', 'blog', 'crud', 'api-backend']) {
      assert(
        listing.includes(`package/template/${template}/package.json`),
        `create-ruvyxa package missing ${template} template`,
      )
      assert(
        listing.includes(`package/template/${template}/gitignore`),
        `create-ruvyxa package missing ${template} scaffold ignore template`,
      )
    }
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

execFileSync('tar', ['-xzf', `${destination}/${ruvyxaTgz}`, '-C', extracted])
execFileSync('node', [`${extracted}/package/bin/ruvyxa.js`, '--help'], {
  stdio: 'inherit',
})
mkdirSync(`${extracted}/create-ruvyxa`)
execFileSync('tar', [
  '-xzf',
  `${destination}/${createRuvyxaTgz}`,
  '-C',
  `${extracted}/create-ruvyxa`,
])
const starters = ['minimal', 'blog', 'crud', 'api-backend']
const scaffoldTarball = (file) => `file:../../${destination}/${file}`
const workspaceTarball = (file) => `file:../${destination}/${file}`

// Verify every packaged starter can scaffold, install, and type-check against the freshly packed
// unpublished packages. A single temporary workspace keeps this release gate reasonably fast.
for (const starter of starters) {
  const appDir = `${extracted}/scaffolded-${starter}`
  const createArgs = [`${extracted}/create-ruvyxa/package/bin/create-ruvyxa.js`, appDir]
  if (starter !== 'minimal') createArgs.push('--template', starter)
  execFileSync('node', createArgs, {
    stdio: 'inherit',
  })
  assert(existsSync(`${appDir}/.gitignore`), `${starter} scaffold missing .gitignore`)
  assert(
    !existsSync(`${appDir}/app/css.d.ts`),
    `${starter} scaffold should use the framework-owned CSS declaration`,
  )

  const packageJsonPath = `${appDir}/package.json`
  const packageJson = JSON.parse(readFileSync(packageJsonPath, 'utf8'))
  packageJson.dependencies.ruvyxa = scaffoldTarball(ruvyxaTgz)
  packageJson.dependencies['@ruvyxa/react'] = scaffoldTarball(reactTgz)
  writeFileSync(packageJsonPath, JSON.stringify(packageJson, null, 2) + '\n')
  writeFileSync(`${appDir}/app/type-check.module.scss`, '.typeCheck {}\n')
  writeFileSync(
    `${appDir}/app/framework-type-entries.ts`,
    [
      "import 'ruvyxa'",
      "import 'ruvyxa/config'",
      "import 'ruvyxa/server'",
      "import styles from './type-check.module.scss'",
      'const moduleClass: string = styles.typeCheck',
      'void moduleClass',
      '',
    ].join('\n'),
  )
}

writeFileSync(
  `${extracted}/pnpm-workspace.yaml`,
  [
    'packages:',
    "  - 'scaffolded-*'",
    'overrides:',
    `  '@ruvyxa/core': ${JSON.stringify(workspaceTarball(coreTgz))}`,
    `  '${currentPlatformPackage}': ${JSON.stringify(workspaceTarball(currentPlatformTgz))}`,
    'allowBuilds:',
    "  '@parcel/watcher': false",
    '',
  ].join('\n'),
)

execFileSync(pnpmBin, ['install', '--no-lockfile'], {
  cwd: extracted,
  stdio: 'inherit',
  shell: process.platform === 'win32',
})
for (const starter of starters) {
  execFileSync(pnpmBin, ['run', 'typecheck'], {
    cwd: `${extracted}/scaffolded-${starter}`,
    stdio: 'inherit',
    shell: process.platform === 'win32',
  })
}

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
