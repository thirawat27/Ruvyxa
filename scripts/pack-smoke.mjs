#!/usr/bin/env node
import { execFileSync, execSync } from 'node:child_process'
import { existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs'
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
