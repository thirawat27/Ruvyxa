#!/usr/bin/env node
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'

const args = process.argv.slice(2)
const pnpmBin = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm'
const npmBin = process.platform === 'win32' ? 'npm.cmd' : 'npm'
const dryRun = args.includes('--dry-run')
const packageNames = args.filter((arg) => arg !== '--dry-run')

if (packageNames.length === 0) {
  console.error('Usage: node scripts/publish-if-new.mjs [--dry-run] <package> [package...]')
  process.exit(1)
}

const packageDirs = [
  'packages/ruvyxa',
  'packages/create-ruvyxa',
  ...readdirSync('packages/@ruvyxa')
    .map((name) => `packages/@ruvyxa/${name}`)
    .filter((dir) => statSync(dir).isDirectory()),
]

const packages = new Map(
  packageDirs.map((dir) => {
    const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'))
    return [pkg.name, { dir, version: pkg.version }]
  }),
)

for (const name of packageNames) {
  const pkg = packages.get(name)
  if (!pkg) {
    console.error(`Unknown workspace package: ${name}`)
    process.exit(1)
  }

  if (await npmPackageExists(name, pkg.version)) {
    console.log(`${name}@${pkg.version} already exists on npm, skipping publish`)
    continue
  }

  console.log(`Publishing ${name}@${pkg.version}`)
  if (dryRun) {
    console.log(`[dry-run] pnpm --filter ${name} publish --access public --no-git-checks`)
    continue
  }

  const publish = spawnSync(
    pnpmBin,
    ['--filter', name, 'publish', '--access', 'public', '--no-git-checks'],
    {
      stdio: 'inherit',
      shell: process.platform === 'win32',
    },
  )

  if (publish.status !== 0) {
    process.exit(publish.status ?? 1)
  }
}

async function npmPackageExists(name, version) {
  const spec = `${name}@${version}`

  for (let attempt = 1; attempt <= 3; attempt++) {
    const view = spawnSync(npmBin, ['view', spec, 'version'], {
      encoding: 'utf8',
    })

    if (view.status === 0) return true

    const output = `${view.stdout ?? ''}\n${view.stderr ?? ''}`
    if (output.includes('E404') || output.includes('404 Not Found')) return false

    const delay = attempt * 20000
    console.warn(`npm view ${spec} failed on attempt ${attempt}; retrying in ${delay / 1000}s`)
    await sleep(delay)
  }

  console.error(`Could not verify whether ${spec} exists on npm`)
  process.exit(1)
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}
