import assert from 'node:assert/strict'
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

import {
  nativeBinaryPackageName,
  exitCodeForSpawnResult,
  supportedPlatforms,
} from '../../../packages/ruvyxa/scripts/native-platform.mjs'

const ruvyxaPackage = readJson('../../../packages/ruvyxa/package.json')
const workspacePackage = readJson('../../../package.json')
const windowsArmPackage = readJson('../../../packages/@ruvyxa/cli-win32-arm64/package.json')

describe('Ruvyxa CLI platforms', () => {
  it('maps every supported platform to its optional binary package', () => {
    for (const platformKey of Object.keys(supportedPlatforms)) {
      assert.equal(nativeBinaryPackageName(platformKey), `@ruvyxa/cli-${platformKey}`)
    }
  })

  it('supports the Windows arm64 executable and package', () => {
    assert.deepEqual(supportedPlatforms['win32-arm64'], {
      os: 'win32',
      cpu: 'arm64',
      executable: 'ruvyxa.exe',
    })
    assert.equal(nativeBinaryPackageName('win32-arm64'), '@ruvyxa/cli-win32-arm64')
    assert.equal(ruvyxaPackage.optionalDependencies['@ruvyxa/cli-win32-arm64'], 'workspace:^')
    assert.deepEqual(windowsArmPackage.os, ['win32'])
    assert.deepEqual(windowsArmPackage.cpu, ['arm64'])
  })

  it('builds Linux release packages against static musl targets', () => {
    assert.equal(supportedPlatforms['linux-x64'].rustTarget, 'x86_64-unknown-linux-musl')
    assert.equal(supportedPlatforms['linux-arm64'].rustTarget, 'aarch64-unknown-linux-musl')

    const releaseWorkflow = readFileSync(
      new URL('../../../.github/workflows/release.yml', import.meta.url),
      'utf8',
    )
    assert.match(releaseWorkflow, /rust_target: x86_64-unknown-linux-musl/)
    assert.match(releaseWorkflow, /rust_target: aarch64-unknown-linux-musl/)
    assert.match(releaseWorkflow, /RUVYXA_REQUIRE_STATIC_LINUX:/)
  })

  it('publishes and verifies every official adapter required by ruvyxa', () => {
    const releaseWorkflow = readFileSync(
      new URL('../../../.github/workflows/release.yml', import.meta.url),
      'utf8',
    )
    const releasePlan = readFileSync(
      new URL('../../../scripts/validate-release-publish-plan.mjs', import.meta.url),
      'utf8',
    )
    const adapterDependencies = Object.keys(ruvyxaPackage.dependencies).filter((name) =>
      name.startsWith('@ruvyxa/adapter-'),
    )

    assert.equal(
      releaseWorkflow.match(/node scripts\/validate-release-publish-plan\.mjs/g)?.length,
      2,
      'release and verification steps must both validate the shared publish plan',
    )
    for (const adapterName of adapterDependencies) {
      assert.match(
        releasePlan,
        new RegExp(`['"]${escapeRegExp(adapterName)}['"]`),
        `${adapterName} must appear in the shared release publish plan`,
      )
    }
  })

  it('does not resolve an optional package for unsupported platforms', () => {
    assert.equal(nativeBinaryPackageName('freebsd-x64'), null)
  })

  it('preserves child exit status and fails when the child is terminated by a signal', () => {
    assert.equal(exitCodeForSpawnResult({ status: 0, signal: null }), 0)
    assert.equal(exitCodeForSpawnResult({ status: 42, signal: null }), 42)
    assert.equal(exitCodeForSpawnResult({ status: null, signal: 'SIGTERM' }), 1)
    assert.equal(exitCodeForSpawnResult({ status: null, signal: null }), 1)
  })

  it('keeps executable packages aligned with the framework Node requirement', () => {
    const expectedEngine = ruvyxaPackage.engines.node
    const packagePaths = [
      '../../../packages/create-ruvyxa/package.json',
      ...readdirSync(new URL('../../../packages/@ruvyxa/', import.meta.url), {
        withFileTypes: true,
      })
        .filter((entry) => entry.isDirectory() && entry.name.startsWith('cli-'))
        .map((entry) => `../../../packages/@ruvyxa/${entry.name}/package.json`),
    ]

    for (const packagePath of packagePaths) {
      assert.equal(readJson(packagePath).engines.node, expectedEngine, packagePath)
    }
  })

  it('tests the declared Rust and Node compatibility in CI', () => {
    const workspaceManifest = readFileSync(new URL('../../../Cargo.toml', import.meta.url), 'utf8')
    const ciWorkflow = readFileSync(
      new URL('../../../.github/workflows/ci.yml', import.meta.url),
      'utf8',
    )

    assert.match(workspaceManifest, /rust-version = "1\.96"/)
    assert.equal(workspacePackage.engines.node, '>=24.19.0')
    assert.equal(workspacePackage.packageManager, 'pnpm@11.22.0')
    assert.equal(ruvyxaPackage.engines.node, '>=24.19.0')
    assert.match(ciWorkflow, /toolchain: 1\.96\.0/)
    assert.match(ciWorkflow, /node: '24\.19\.0'/)
    assert.equal([...ciWorkflow.matchAll(/node: '24\.19\.0'/g)].length, 5)
    assert.match(ciWorkflow, /node-version: \$\{\{ matrix\.node \}\}/)
  })

  // TypeScript test sources are compiled by `tsc` before `node --test` runs
  // them, so the suite never depends on a runtime that can strip types. Node
  // 24.19 — the floor declared above — cannot, and an unflagged release that
  // can would make CI green on a Node the framework claims to support but
  // never exercises.
  it('runs the TypeScript suites as compiled JavaScript, on no experimental runtime flag', () => {
    for (const workflow of readdirSync(new URL('../../../.github/workflows/', import.meta.url))) {
      const contents = readFileSync(
        new URL(`../../../.github/workflows/${workflow}`, import.meta.url),
        'utf8',
      )
      assert.doesNotMatch(contents, /--experimental-/, workflow)
    }

    // The runner is what keeps `node --test` pointed at compiled output, so a
    // package that grew a suite of its own has to go through it too.
    const runner = readFileSync(
      new URL('../../../scripts/test-package.mjs', import.meta.url),
      'utf8',
    )
    assert.match(runner, /tsconfig\.test\.json/)
    assert.match(runner, /\.test\.js/)
    assert.doesNotMatch(runner, /--experimental-/)
    assert.doesNotMatch(runner, /\.test\.ts/)

    let compiledSuites = 0
    for (const [name, manifest] of workspacePackages()) {
      const script = manifest.scripts?.test
      if (!script) continue
      assert.doesNotMatch(script, /\.test\.ts/, name)
      if (!script.includes('test-package.mjs')) continue
      assert.match(script, /scripts\/test-package\.mjs [\w-]+$/, name)
      compiledSuites += 1
    }
    assert.ok(compiledSuites > 0, 'no package runs a compiled TypeScript suite')
  })

  it('does not publish an Intel macOS binary package', () => {
    const removedPlatformKey = ['darwin', 'x64'].join('-')
    const removedPackageName = `@ruvyxa/cli-${removedPlatformKey}`
    assert.equal(supportedPlatforms[removedPlatformKey], undefined)
    assert.equal(nativeBinaryPackageName(removedPlatformKey), null)
    assert.equal(ruvyxaPackage.optionalDependencies[removedPackageName], undefined)
  })
})

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function readJson(relativePath) {
  return JSON.parse(readFileSync(new URL(relativePath, import.meta.url), 'utf8'))
}

/**
 * Every workspace package manifest, as `[directory, manifest]` pairs.
 *
 * Directories without a manifest are skipped rather than read: `packages/` also
 * holds whatever the package manager and prepack steps leave behind, and that
 * set differs between a developer checkout and a CI runner.
 */
function workspacePackages() {
  const packagesDir = new URL('../../../packages/', import.meta.url)
  const directories = []
  for (const entry of readdirSync(packagesDir, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.name === 'node_modules') continue
    if (!entry.name.startsWith('@')) {
      directories.push(entry.name)
      continue
    }
    for (const child of readdirSync(new URL(`${entry.name}/`, packagesDir), {
      withFileTypes: true,
    })) {
      if (child.isDirectory() && child.name !== 'node_modules') {
        directories.push(`${entry.name}/${child.name}`)
      }
    }
  }

  return directories
    .filter((directory) =>
      existsSync(new URL(`../../../packages/${directory}/package.json`, import.meta.url)),
    )
    .map((directory) => [directory, readJson(`../../../packages/${directory}/package.json`)])
}
