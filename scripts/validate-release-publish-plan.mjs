#!/usr/bin/env node
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { workspacePackageDirs } from './workspace-packages.mjs'

const orderedPackages = [
  '@ruvyxa/core',
  '@ruvyxa/react',
  '@ruvyxa/auth',
  '@ruvyxa/database',
  '@ruvyxa/realtime',
  '@ruvyxa/testing',
  '@ruvyxa/adapter-aws',
  '@ruvyxa/adapter-bun',
  '@ruvyxa/adapter-cloudflare',
  '@ruvyxa/adapter-deno',
  '@ruvyxa/adapter-firebase',
  '@ruvyxa/adapter-netlify',
  '@ruvyxa/adapter-node',
  '@ruvyxa/adapter-railway',
  '@ruvyxa/adapter-render',
  '@ruvyxa/adapter-static',
  '@ruvyxa/adapter-vercel',
  'ruvyxa',
  'create-ruvyxa',
]

const packageDirs = workspacePackageDirs().dirs.map((dir) => ({
  dir,
  manifest: JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')),
}))

const expectedPackages = packageDirs
  .map(({ manifest }) => manifest)
  .filter((manifest) => !manifest.name.startsWith('@ruvyxa/cli-'))
  .map((manifest) => manifest.name)
  .sort()
const actualPackages = [...orderedPackages].sort()
const failures = []

check(
  new Set(orderedPackages).size === orderedPackages.length,
  'release publish plan contains duplicate package names',
)
check(
  JSON.stringify(actualPackages) === JSON.stringify(expectedPackages),
  `release publish plan does not match workspace packages. Expected: ${expectedPackages.join(', ')}; configured: ${orderedPackages.join(', ')}`,
)

const manifests = new Map(packageDirs.map(({ manifest }) => [manifest.name, manifest]))
for (const name of orderedPackages) {
  const manifest = manifests.get(name)
  check(manifest !== undefined, `${name} in release publish plan is not a workspace package`)
  check(
    manifest?.publishConfig?.access === 'public',
    `${name} in release publish plan must have publishConfig.access=public`,
  )
}

const ruvyxaIndex = orderedPackages.indexOf('ruvyxa')
const ruvyxaManifest = manifests.get('ruvyxa')
for (const dependencyName of Object.keys(ruvyxaManifest?.dependencies ?? {})) {
  if (!dependencyName.startsWith('@ruvyxa/')) continue
  const dependencyIndex = orderedPackages.indexOf(dependencyName)
  if (dependencyIndex === -1) {
    failures.push(`ruvyxa dependency ${dependencyName} is missing from the release publish plan`)
  } else if (dependencyIndex >= ruvyxaIndex) {
    failures.push(`ruvyxa dependency ${dependencyName} must be published before ruvyxa`)
  }
}

if (failures.length > 0) {
  console.error(failures.map((failure) => `- ${failure}`).join('\n'))
  process.exit(1)
}

if (process.argv.includes('--list')) {
  process.stdout.write(`${orderedPackages.join('\n')}\n`)
} else {
  console.log(`Validated release publish plan for ${orderedPackages.length} JavaScript packages.`)
}

function check(condition, message) {
  if (!condition) failures.push(message)
}
