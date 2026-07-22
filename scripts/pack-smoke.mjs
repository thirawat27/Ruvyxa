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
  '@ruvyxa/auth',
  '@ruvyxa/database',
  '@ruvyxa/realtime',
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

  if (['@ruvyxa/auth', '@ruvyxa/database', '@ruvyxa/realtime'].includes(packageJson.name)) {
    assert(listing.includes('package/dist/index.js'), `${packageJson.name} missing dist/index.js`)
    assert(listing.includes('package/dist/index.d.ts'), `${packageJson.name} missing declarations`)
  }
  if (['@ruvyxa/auth', '@ruvyxa/realtime'].includes(packageJson.name)) {
    assert(
      listing.includes('package/dist/client.js'),
      `${packageJson.name} missing client entrypoint`,
    )
    assert(
      listing.includes('package/dist/client.d.ts'),
      `${packageJson.name} missing client declarations`,
    )
  }

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
    assert(
      listing.includes('package/dist/plugins.js'),
      'ruvyxa package missing built-in plugin entrypoint',
    )
    const pluginDeclarations = execFileSync('tar', [
      '-xOf',
      tarball,
      'package/dist/plugins.d.ts',
    ]).toString()
    assert(
      pluginDeclarations.includes('contentEngine'),
      'ruvyxa package missing Content Engine declarations',
    )
    for (const typeFile of [
      'css.d.ts',
      'index.d.ts',
      'config.d.ts',
      'server.d.ts',
      'plugins.d.ts',
    ]) {
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

const ruvyxaTgz = packedTarball('ruvyxa')
const createRuvyxaTgz = packedTarball('create-ruvyxa')
const coreTgz = packedTarball('@ruvyxa/core')
const reactTgz = packedTarball('@ruvyxa/react')
const authTgz = packedTarball('@ruvyxa/auth')
const databaseTgz = packedTarball('@ruvyxa/database')
const realtimeTgz = packedTarball('@ruvyxa/realtime')
const currentPlatformTgz = packedTarball(currentPlatformPackage)

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
  packageJson.dependencies['@ruvyxa/auth'] = scaffoldTarball(authTgz)
  packageJson.dependencies['@ruvyxa/database'] = scaffoldTarball(databaseTgz)
  packageJson.dependencies['@ruvyxa/realtime'] = scaffoldTarball(realtimeTgz)
  writeFileSync(packageJsonPath, JSON.stringify(packageJson, null, 2) + '\n')
  writeFileSync(`${appDir}/app/type-check.module.scss`, '.typeCheck {}\n')
  writeFileSync(
    `${appDir}/app/framework-type-entries.ts`,
    [
      "import 'ruvyxa'",
      "import 'ruvyxa/config'",
      "import { contentEngine } from 'ruvyxa/plugins'",
      "import 'ruvyxa/server'",
      "import type { AuthSession } from '@ruvyxa/auth/client'",
      "import { databasePlugin } from '@ruvyxa/database'",
      "import { realtime } from '@ruvyxa/realtime'",
      "import { createRealtimeClient } from '@ruvyxa/realtime/client'",
      "import type { AnswerProps, SeoProps } from '@ruvyxa/react'",
      "import styles from './type-check.module.scss'",
      'const moduleClass: string = styles.typeCheck',
      "const contentPlugin = contentEngine({ siteUrl: 'https://example.com', title: 'Example', description: 'Articles' })",
      'const databaseBuildPlugin = databasePlugin()',
      'const realtimePlugin = realtime()',
      'const realtimeClient = createRealtimeClient()',
      'const authSession: AuthSession | null = null',
      "const answerProps: AnswerProps = { question: 'Is it typed?', answer: 'Yes.' }",
      "const seoProps: SeoProps = { title: 'Guide', article: { authors: [{ name: 'Ada' }] } }",
      'void moduleClass',
      'void contentPlugin',
      'void databaseBuildPlugin',
      'void realtimePlugin',
      'void realtimeClient',
      'void authSession',
      'void answerProps',
      'void seoProps',
      '',
    ].join('\n'),
  )

  if (starter === 'minimal') {
    const configPath = `${appDir}/ruvyxa.config.ts`
    const configSource = readFileSync(configPath, 'utf8')
    writeFileSync(
      configPath,
      `import { databasePlugin } from '@ruvyxa/database'\nimport { realtime } from '@ruvyxa/realtime'\nimport { contentEngine } from 'ruvyxa/plugins'\n${configSource.replace(
        'const settings: RuvyxaConfig = {',
        `const settings: RuvyxaConfig = {
  plugins: [
    databasePlugin(),
    realtime(),
    contentEngine({
      siteUrl: 'https://example.com',
      title: 'Example',
      description: 'Articles',
      locale: 'en',
    }),
  ],`,
      )}`,
    )
    mkdirSync(`${appDir}/app/guide`, { recursive: true })
    writeFileSync(
      `${appDir}/app/guide/page.md`,
      '---\ntitle: Packed Content Engine\ntags: [release, smoke]\n---\n# Packed guide\n',
    )
  }
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
execFileSync(pnpmBin, ['exec', 'ruvyxa', 'check', '--root', '.'], {
  cwd: `${extracted}/scaffolded-minimal`,
  stdio: 'inherit',
  shell: process.platform === 'win32',
})
const packedManifest = JSON.parse(
  readFileSync(`${extracted}/scaffolded-minimal/.ruvyxa/assets/content.json`, 'utf8'),
)
assert(
  packedManifest.entries.some((entry) => entry.route === '/guide'),
  'packed Content Engine did not emit the Markdown route',
)
const packedLlms = readFileSync(`${extracted}/scaffolded-minimal/.ruvyxa/assets/llms.txt`, 'utf8')
assert(
  packedLlms.includes('[Packed Content Engine](<https://example.com/guide>)'),
  'packed Content Engine did not emit the llms.txt route',
)

await rmWithRetry(extracted)

console.log('npm pack smoke passed.')

function assert(condition, message) {
  if (!condition) {
    throw new Error(message)
  }
}

function packedTarball(packageName) {
  for (const file of readdirSync(destination).filter((name) => name.endsWith('.tgz'))) {
    const manifest = JSON.parse(
      execFileSync('tar', ['-xOf', `${destination}/${file}`, 'package/package.json']).toString(),
    )
    if (manifest.name === packageName) return file
  }
  throw new Error(`${packageName} tarball not found in ${destination}`)
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
