#!/usr/bin/env node
import { spawnSync } from 'node:child_process'
import { chmod, cp, mkdir, readFile, rm } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { currentPlatform } from './native-platform.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const ruvyxaPackageRoot = resolve(here, '..')
const repoRoot = resolve(ruvyxaPackageRoot, '../..')
const platformPackageRoot = process.cwd()
const packageJson = JSON.parse(await readFile(resolve(platformPackageRoot, 'package.json'), 'utf8'))
const expectedKey = packageJson.name.replace('@ruvyxa/cli-', '')
const platform = currentPlatform()

if (platform.key !== expectedKey) {
  throw new Error(
    `${packageJson.name} must be packed on ${expectedKey}, current platform is ${platform.key}`,
  )
}

const rustTarget = process.env.RUVYXA_RUST_TARGET?.trim() || undefined
const requireStaticLinux = process.env.RUVYXA_REQUIRE_STATIC_LINUX === '1'
if (requireStaticLinux && platform.os === 'linux' && rustTarget !== platform.rustTarget) {
  throw new Error(
    `${packageJson.name} release must use ${platform.rustTarget}; received ${rustTarget ?? 'the host default target'}`,
  )
}

const cargoArgs = ['build', '--release', '-p', 'ruvyxa_cli']
if (rustTarget) cargoArgs.push('--target', rustTarget)
const build = spawnSync('cargo', cargoArgs, {
  cwd: repoRoot,
  stdio: 'inherit',
  shell: process.platform === 'win32',
})

if (build.status !== 0) {
  process.exit(build.status ?? 1)
}

await rm(resolve(platformPackageRoot, 'bin'), { recursive: true, force: true })
await mkdir(resolve(platformPackageRoot, 'bin'), { recursive: true })
const source = resolve(
  repoRoot,
  'target',
  ...(rustTarget ? [rustTarget] : []),
  'release',
  platform.executable,
)
if (requireStaticLinux && platform.os === 'linux') {
  assertStaticLinuxBinary(source)
}

const target = resolve(platformPackageRoot, 'bin', platform.executable)
await cp(source, target)

if (process.platform !== 'win32') {
  await chmod(target, 0o755)
}

/** Reject a release artifact that can acquire a host glibc version requirement. */
function assertStaticLinuxBinary(binary) {
  const dynamicSection = spawnSync('readelf', ['--dynamic', binary], { encoding: 'utf8' })
  if (dynamicSection.error || dynamicSection.status !== 0) {
    throw new Error(
      `Could not inspect ${binary} with readelf: ${dynamicSection.error?.message ?? dynamicSection.stderr}`,
    )
  }
  if (/\(NEEDED\)/.test(dynamicSection.stdout)) {
    throw new Error(
      `${binary} is dynamically linked; Linux CLI releases must be static musl binaries`,
    )
  }
}
