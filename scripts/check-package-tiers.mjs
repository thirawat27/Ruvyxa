#!/usr/bin/env node
/**
 * The experimental tier, enforced.
 *
 * `@ruvyxa/auth`, `@ruvyxa/database`, `@ruvyxa/realtime`, and `@ruvyxa/testing`
 * ship without a production guarantee. That is only honest while two things
 * hold: each says so where a reader looks first, and nothing in Tier 1 depends
 * on one — otherwise the guarantee a Tier 1 package makes silently includes code
 * that carries none.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

import { REPO_ROOT, workspacePackageDirs } from './workspace-packages.mjs'

export const EXPERIMENTAL_WARNING = '> **Experimental.**'

const MUST_BE_EXPERIMENTAL = new Set([
  '@ruvyxa/auth',
  '@ruvyxa/database',
  '@ruvyxa/realtime',
  '@ruvyxa/testing',
])
const DEPENDENCY_FIELDS = ['dependencies', 'peerDependencies', 'optionalDependencies']

function byCodePoint(left, right) {
  if (left < right) return -1
  return left > right ? 1 : 0
}

/**
 * Whether the warning sits in the README's opening: before the first code
 * fence. The package READMEs open with an HTML logo/title block rather than a
 * `# title` line, so "the line after the title" is not a position they have.
 */
function opensWithWarning(readme) {
  for (const line of readme.split(/\r?\n/)) {
    if (line.startsWith('```')) return false
    if (line.startsWith(EXPERIMENTAL_WARNING)) return true
  }
  return false
}

export function tierFailures(packages) {
  const failures = []
  const experimental = new Set()
  for (const { name, manifest, readme } of packages) {
    const tier = manifest.ruvyxa?.tier
    if (tier !== undefined && tier !== 'experimental') {
      failures.push(
        `${name} declares ruvyxa.tier "${tier}"; the only tier a package may declare is "experimental"`,
      )
      continue
    }
    if (tier === 'experimental') {
      experimental.add(name)
      if (!opensWithWarning(readme)) {
        failures.push(
          `${name} is experimental but its README does not open with "${EXPERIMENTAL_WARNING}"`,
        )
      }
    } else if (MUST_BE_EXPERIMENTAL.has(name)) {
      failures.push(`${name} must declare "ruvyxa": { "tier": "experimental" }`)
    }
  }
  for (const { name, manifest } of packages) {
    if (experimental.has(name)) continue
    for (const field of DEPENDENCY_FIELDS) {
      for (const dependency of Object.keys(manifest[field] ?? {})) {
        if (experimental.has(dependency)) {
          failures.push(`${name} is Tier 1 but its ${field} names experimental ${dependency}`)
        }
      }
    }
  }
  return failures.sort(byCodePoint)
}

function main() {
  const { dirs } = workspacePackageDirs()
  const packages = dirs.map((dir) => {
    const root = join(REPO_ROOT, dir)
    const manifest = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'))
    let readme = ''
    try {
      readme = readFileSync(join(root, 'README.md'), 'utf8')
    } catch (error) {
      // A package with no README cannot carry the warning; tierFailures says so.
      // Anything else (permissions, a directory named README.md) is a real fault.
      if (error?.code !== 'ENOENT') throw error
    }
    return { name: manifest.name, manifest, readme }
  })
  const failures = tierFailures(packages)
  for (const failure of failures) console.error(`check-package-tiers: ${failure}`)
  if (failures.length > 0) process.exit(1)
  console.log(`Package tiers hold across ${packages.length} packages.`)
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) main()
