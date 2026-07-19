import assert from 'node:assert/strict'
import { readdirSync, readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

import {
  nativeBinaryPackageName,
  exitCodeForSpawnResult,
  supportedPlatforms,
} from '../../../packages/ruvyxa/scripts/native-platform.mjs'

const ruvyxaPackage = readJson('../../../packages/ruvyxa/package.json')
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

  it('does not publish an Intel macOS binary package', () => {
    const removedPlatformKey = ['darwin', 'x64'].join('-')
    const removedPackageName = `@ruvyxa/cli-${removedPlatformKey}`
    assert.equal(supportedPlatforms[removedPlatformKey], undefined)
    assert.equal(nativeBinaryPackageName(removedPlatformKey), null)
    assert.equal(ruvyxaPackage.optionalDependencies[removedPackageName], undefined)
  })
})

function readJson(relativePath) {
  return JSON.parse(readFileSync(new URL(relativePath, import.meta.url), 'utf8'))
}
