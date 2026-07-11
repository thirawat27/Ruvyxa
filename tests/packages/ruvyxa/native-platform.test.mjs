import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

import {
  nativeBinaryPackageName,
  supportedPlatforms,
} from '../../../packages/ruvyxa/scripts/native-platform.mjs'

const ruvyxaPackage = readJson('../../../packages/ruvyxa/package.json')
const windowsArmPackage = readJson('../../../packages/@ruvyxa/cli-win32-arm64/package.json')

describe('native CLI platforms', () => {
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
})

function readJson(relativePath) {
  return JSON.parse(readFileSync(new URL(relativePath, import.meta.url), 'utf8'))
}
